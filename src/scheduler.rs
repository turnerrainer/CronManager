//! Scheduler — one Tokio task per non-manual job, plus a
//! per-job "running instance" guard so `/execute` is idempotent
//! and `/stop` has something to signal.
//!
//! The scheduler is intentionally single-instance. Running two
//! processes against the same DSL directory will double-fire
//! every job. Cross-replica coordination is deferred to a
//! future release (see docs/DESIGN.md §5).

use crate::dsl::{JobKey, JobSpec, Trigger};
use crate::error::CronManagerError;
use crate::executor::{DispatchExtras, DispatchOutcome, ExecutorBundle};
use chrono::Utc;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

/// One JSON row in `/jobs` or `/running`.
#[derive(Debug, Clone, Serialize)]
pub struct JobDescriptor {
    pub name: String,
    pub schedule: String,
    #[serde(rename = "lastExecution")]
    pub last_execution: i64,
    #[serde(rename = "nextExecution")]
    pub next_execution: i64,
    #[serde(rename = "lastResult")]
    pub last_result: String,
}

/// Per-job runtime state — updated as fires happen.
#[derive(Debug, Default, Clone)]
struct JobRuntimeState {
    pub last_execution_ms: Option<i64>,
    pub last_result: String,
}

/// Per-job registration. `_task` is kept so the JoinHandle is
/// dropped on `unregister` (which cancels the scheduling loop
/// via drop; the running instance, if any, is signalled
/// separately).
struct RegisteredJob {
    spec: JobSpec,
    state: Arc<Mutex<JobRuntimeState>>,
    _task: Option<JoinHandle<()>>,
}

/// Running instance — one at most per JobKey. `/stop` looks up
/// the entry and calls `notify_waiters()` on `cancel`; the
/// executors are listening.
#[derive(Clone)]
struct RunningInstance {
    started_at_ms: i64,
    schedule: String,
    cancel: Arc<Notify>,
}

#[derive(Clone)]
pub struct Scheduler {
    inner: Arc<SchedulerInner>,
}

struct SchedulerInner {
    jobs: Mutex<HashMap<JobKey, RegisteredJob>>,
    running: Mutex<HashMap<JobKey, RunningInstance>>,
    bundle: ExecutorBundle,
}

impl Scheduler {
    pub fn new(bundle: ExecutorBundle) -> Self {
        Self {
            inner: Arc::new(SchedulerInner {
                jobs: Mutex::new(HashMap::new()),
                running: Mutex::new(HashMap::new()),
                bundle,
            }),
        }
    }

    /// Register a new job. Spawns a scheduling task if the
    /// trigger is a cron expression; manual-only jobs are just
    /// held in the registry so `/execute` can find them.
    ///
    /// Duplicate keys replace the existing entry.
    pub fn register(&self, spec: JobSpec) {
        let key = spec.key.clone();
        let state = Arc::new(Mutex::new(JobRuntimeState::default()));

        // Drop any existing registration first so its scheduling
        // task ends and we don't leak a duplicate loop.
        self.inner.jobs.lock().unwrap().remove(&key);

        let task = match &spec.trigger {
            Trigger::Cron(expr) => {
                let scheduler = self.clone();
                let spec_for_task = spec.clone();
                let state_for_task = state.clone();
                let expr_owned = expr.clone();
                Some(tokio::spawn(async move {
                    scheduler
                        .scheduling_loop(spec_for_task, expr_owned, state_for_task)
                        .await
                }))
            }
            Trigger::Manual => None,
        };

        self.inner.jobs.lock().unwrap().insert(
            key,
            RegisteredJob {
                spec,
                state,
                _task: task,
            },
        );
    }

    /// Register a batch of jobs. Convenience wrapper.
    pub fn register_all(&self, specs: impl IntoIterator<Item = JobSpec>) {
        for s in specs {
            self.register(s);
        }
    }

    /// Fire a job now. Fails with `JobNotFound` or
    /// `JobAlreadyRunning`. The dispatch runs on a spawned task
    /// so `/execute` returns as soon as the run is registered.
    pub fn trigger_now(
        &self,
        key: &JobKey,
        extras: DispatchExtras,
    ) -> Result<(), CronManagerError> {
        let (spec, state, schedule_str) = {
            let jobs = self.inner.jobs.lock().unwrap();
            let entry = jobs.get(key).ok_or_else(|| CronManagerError::JobNotFound {
                group: key.group.clone(),
                name: key.name.clone(),
            })?;
            let schedule_str = match &entry.spec.trigger {
                Trigger::Cron(e) => e.clone(),
                Trigger::Manual => String::new(),
            };
            (entry.spec.clone(), entry.state.clone(), schedule_str)
        };

        self.begin_running(key.clone(), schedule_str.clone())?;
        let cancel = self
            .inner
            .running
            .lock()
            .unwrap()
            .get(key)
            .expect("just inserted")
            .cancel
            .clone();

        let inner = self.inner.clone();
        let key_owned = key.clone();
        let extras_owned = extras;
        tokio::spawn(async move {
            let outcome = inner.bundle.dispatch(&spec, extras_owned, cancel).await;
            record_outcome(&state, &outcome);
            inner.running.lock().unwrap().remove(&key_owned);
        });
        Ok(())
    }

    /// Signal the currently-running instance of a job (if any)
    /// to abort. Returns false if the job isn't running.
    pub fn stop(&self, key: &JobKey) -> Result<bool, CronManagerError> {
        // Confirm the job is registered — surface JobNotFound
        // instead of silently returning false for a typo.
        if !self.inner.jobs.lock().unwrap().contains_key(key) {
            return Err(CronManagerError::JobNotFound {
                group: key.group.clone(),
                name: key.name.clone(),
            });
        }
        let running = self.inner.running.lock().unwrap();
        if let Some(inst) = running.get(key) {
            inst.cancel.notify_waiters();
            return Ok(true);
        }
        Ok(false)
    }

    /// Snapshot of every registered job grouped by group name.
    /// `group_filter` narrows to a single group.
    pub fn describe(&self, group_filter: Option<&str>) -> BTreeMap<String, Vec<JobDescriptor>> {
        let jobs = self.inner.jobs.lock().unwrap();
        let mut out: BTreeMap<String, Vec<JobDescriptor>> = BTreeMap::new();
        for (key, entry) in jobs.iter() {
            if let Some(g) = group_filter {
                if key.group != g {
                    continue;
                }
            }
            let (schedule_str, next) = match &entry.spec.trigger {
                Trigger::Cron(expr) => (expr.clone(), next_fire_ms(expr)),
                Trigger::Manual => (String::new(), 0),
            };
            let state = entry.state.lock().unwrap();
            out.entry(key.group.clone())
                .or_default()
                .push(JobDescriptor {
                    name: key.name.clone(),
                    schedule: schedule_str,
                    last_execution: state.last_execution_ms.unwrap_or(0),
                    next_execution: next,
                    last_result: state.last_result.clone(),
                });
        }
        for jobs in out.values_mut() {
            jobs.sort_by(|a, b| a.name.cmp(&b.name));
        }
        out
    }

    /// Snapshot of currently-running instances.
    pub fn describe_running(
        &self,
        group_filter: Option<&str>,
    ) -> BTreeMap<String, Vec<JobDescriptor>> {
        let running = self.inner.running.lock().unwrap();
        let jobs = self.inner.jobs.lock().unwrap();
        let mut out: BTreeMap<String, Vec<JobDescriptor>> = BTreeMap::new();
        for (key, inst) in running.iter() {
            if let Some(g) = group_filter {
                if key.group != g {
                    continue;
                }
            }
            let last_result = jobs
                .get(key)
                .and_then(|j| j.state.lock().ok().map(|s| s.last_result.clone()))
                .unwrap_or_default();
            out.entry(key.group.clone())
                .or_default()
                .push(JobDescriptor {
                    name: key.name.clone(),
                    schedule: inst.schedule.clone(),
                    last_execution: inst.started_at_ms,
                    next_execution: 0,
                    last_result,
                });
        }
        for v in out.values_mut() {
            v.sort_by(|a, b| a.name.cmp(&b.name));
        }
        out
    }

    /// Number of registered jobs. Used in the startup log.
    pub fn job_count(&self) -> usize {
        self.inner.jobs.lock().unwrap().len()
    }

    /// Reload every job from disk. Rebuilds the registry
    /// (currently a full teardown — see task 005).
    pub fn reload_from(
        &self,
        loaded: Vec<crate::dsl::LoadedJob>,
    ) -> Result<usize, CronManagerError> {
        // Signal any running instances then drop the registry.
        {
            let running = self.inner.running.lock().unwrap();
            for inst in running.values() {
                inst.cancel.notify_waiters();
            }
        }
        self.inner.jobs.lock().unwrap().clear();
        let count = loaded.len();
        for j in loaded {
            self.register(j.spec);
        }
        Ok(count)
    }

    // ------- internals -------

    fn begin_running(&self, key: JobKey, schedule: String) -> Result<(), CronManagerError> {
        let mut running = self.inner.running.lock().unwrap();
        if running.contains_key(&key) {
            return Err(CronManagerError::JobAlreadyRunning {
                group: key.group,
                name: key.name,
            });
        }
        running.insert(
            key,
            RunningInstance {
                started_at_ms: Utc::now().timestamp_millis(),
                schedule,
                cancel: Arc::new(Notify::new()),
            },
        );
        Ok(())
    }

    async fn scheduling_loop(
        self,
        spec: JobSpec,
        expr: String,
        state: Arc<Mutex<JobRuntimeState>>,
    ) {
        let schedule = match cron::Schedule::from_str(&expr) {
            Ok(s) => s,
            Err(e) => {
                // Should never happen — the loader validated the
                // expression at load time. Log defensively and
                // exit the task.
                tracing::error!(
                    "scheduling loop {}/{}: cron re-parse failed: {e}",
                    spec.key.group,
                    spec.key.name
                );
                return;
            }
        };

        loop {
            let now = Utc::now();
            let Some(next) = schedule.upcoming(Utc).next() else {
                tracing::warn!(
                    "scheduling loop {}/{}: no future fires — task exiting",
                    spec.key.group,
                    spec.key.name
                );
                return;
            };
            let wait = (next - now)
                .to_std()
                .unwrap_or_else(|_| Duration::from_millis(0));
            tokio::time::sleep(wait).await;

            // Window check before firing — a job outside its
            // window records a SKIPPED history row and does not
            // invoke the executor.
            let now_ms = Utc::now().timestamp_millis();
            if !spec.window.allows(now_ms) {
                self.inner
                    .bundle
                    .history
                    .record(crate::history::HistoryEntry {
                        execution_time: Utc::now(),
                        job_name: spec.key.name.clone(),
                        job_group: spec.key.group.clone(),
                        job_type: spec.kind.type_label().to_string(),
                        duration_ms: Some(0),
                        status: crate::history::ExecutionStatus::Skipped,
                        http_method: None,
                        http_url: None,
                        http_status_code: None,
                        attempt_number: 1,
                        max_attempts: 1,
                        response_body: None,
                        error_message: Some("outside time window".into()),
                    })
                    .await;
                record_skip(&state, now_ms);
                continue;
            }

            // Skip the fire silently if a previous invocation is
            // still running. Mirrors JVM's default concurrency
            // behaviour minus the double-fire bug (docs/DESIGN.md §3).
            if let Err(e) = self.begin_running(spec.key.clone(), expr.clone()) {
                tracing::warn!(
                    "scheduling loop {}/{}: skipping fire — {}",
                    spec.key.group,
                    spec.key.name,
                    e
                );
                continue;
            }
            let cancel = self
                .inner
                .running
                .lock()
                .unwrap()
                .get(&spec.key)
                .expect("just inserted")
                .cancel
                .clone();
            let outcome = self
                .inner
                .bundle
                .dispatch(&spec, DispatchExtras::default(), cancel)
                .await;
            record_outcome(&state, &outcome);
            self.inner.running.lock().unwrap().remove(&spec.key);
        }
    }
}

fn record_outcome(state: &Mutex<JobRuntimeState>, outcome: &DispatchOutcome) {
    let mut s = state.lock().unwrap();
    s.last_execution_ms = Some(Utc::now().timestamp_millis());
    s.last_result = outcome.status.as_str().to_string();
}

fn record_skip(state: &Mutex<JobRuntimeState>, now_ms: i64) {
    let mut s = state.lock().unwrap();
    s.last_execution_ms = Some(now_ms);
    s.last_result = "SKIPPED".to_string();
}

fn next_fire_ms(expr: &str) -> i64 {
    match cron::Schedule::from_str(expr)
        .ok()
        .and_then(|s| s.upcoming(Utc).next())
    {
        Some(dt) => dt.timestamp_millis(),
        None => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use crate::dsl::{JobKey, JobKind, JobSpec, RetryPolicy, TimeWindow, Trigger};
    use crate::history::NoopRecorder;
    use std::time::Duration as StdDuration;

    fn bundle() -> ExecutorBundle {
        // Override app_root_path so the shell executor doesn't
        // try to cwd into `/app` on the test host.
        let mut cfg = AppConfig::default();
        cfg.app_root_path = std::env::temp_dir();
        ExecutorBundle::new(&cfg, Arc::new(NoopRecorder)).unwrap()
    }

    fn manual_http_job() -> JobSpec {
        JobSpec {
            key: JobKey::new("g", "n"),
            trigger: Trigger::Manual,
            window: TimeWindow::default(),
            retry: RetryPolicy::default(),
            kind: JobKind::Http {
                method: "GET".into(),
                url: "http://127.0.0.1:1/nowhere".into(),
            },
        }
    }

    /// A shell job that reliably takes ~3 seconds. `/bin/sleep`
    /// is present on every Linux distribution CI ever runs on;
    /// the absolute path bypasses `env_clear` PATH stripping.
    fn slow_shell_job() -> JobSpec {
        JobSpec {
            key: JobKey::new("g", "slow"),
            trigger: Trigger::Manual,
            window: TimeWindow::default(),
            retry: RetryPolicy::default(),
            kind: JobKind::Exec {
                command: "/bin/sleep 3".into(),
                allowed_envs: vec![],
            },
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn describe_lists_registered_jobs() {
        let sched = Scheduler::new(bundle());
        sched.register(manual_http_job());
        let out = sched.describe(None);
        assert!(out.contains_key("g"));
        assert_eq!(out["g"][0].name, "n");
        assert_eq!(out["g"][0].schedule, "");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn trigger_now_unknown_returns_not_found() {
        let sched = Scheduler::new(bundle());
        let err = sched
            .trigger_now(&JobKey::new("g", "n"), DispatchExtras::default())
            .unwrap_err();
        assert!(matches!(err, CronManagerError::JobNotFound { .. }));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stop_of_unknown_returns_not_found() {
        let sched = Scheduler::new(bundle());
        let err = sched.stop(&JobKey::new("g", "n")).unwrap_err();
        assert!(matches!(err, CronManagerError::JobNotFound { .. }));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stop_of_not_running_returns_false() {
        let sched = Scheduler::new(bundle());
        sched.register(manual_http_job());
        assert!(!sched.stop(&JobKey::new("g", "n")).unwrap());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn double_trigger_returns_already_running() {
        // Slow shell job guarantees the first trigger is still
        // in-flight when the second one arrives — no timing
        // race regardless of CI load.
        let sched = Scheduler::new(bundle());
        sched.register(slow_shell_job());
        sched
            .trigger_now(&JobKey::new("g", "slow"), DispatchExtras::default())
            .unwrap();
        // Wait for the spawned dispatch to reach the running
        // registry — 300ms is generous even on a cold VM.
        tokio::time::sleep(StdDuration::from_millis(300)).await;
        let err = sched
            .trigger_now(&JobKey::new("g", "slow"), DispatchExtras::default())
            .unwrap_err();
        assert!(matches!(err, CronManagerError::JobAlreadyRunning { .. }));

        // Clean up so the sleep child doesn't outlive the test.
        let _ = sched.stop(&JobKey::new("g", "slow"));
    }
}

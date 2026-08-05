//! Job executors — one impl per job kind.
//!
//! `dispatch` is the entry point the scheduler calls per fire:
//! it picks the right executor based on `spec.kind`, walks the
//! retry loop, and returns the final outcome. Every attempt is
//! recorded via the shared history recorder.

use crate::config::AppConfig;
use crate::dsl::{JobKind, JobSpec};
use crate::error::CronManagerError;
use crate::history::{ExecutionStatus, HistoryEntry, HistoryRecorder};
use chrono::Utc;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Notify;

pub mod http;
pub mod shell;

/// Result of a full retry sequence.
#[derive(Debug, Clone)]
pub struct DispatchOutcome {
    pub status: ExecutionStatus,
    pub attempts: u32,
    pub last_error: Option<String>,
}

/// Extra environment for a shell invocation. Comes from
/// query params on `POST /execute/{group}/{job}`; each key is
/// exported to the child only if the job's `allowedEnvs` opts
/// in.
#[derive(Debug, Clone, Default)]
pub struct DispatchExtras {
    pub env_overrides: Vec<(String, String)>,
}

/// Shared executor bundle. Cloned into each per-job scheduler
/// task; every inner client is `Arc`-backed so cloning is cheap.
#[derive(Clone)]
pub struct ExecutorBundle {
    pub http: http::HttpExecutor,
    pub shell: shell::ShellExecutor,
    pub history: Arc<dyn HistoryRecorder>,
}

impl ExecutorBundle {
    pub fn new(
        cfg: &AppConfig,
        history: Arc<dyn HistoryRecorder>,
    ) -> Result<Self, CronManagerError> {
        Ok(Self {
            http: http::HttpExecutor::new(&cfg.limits)?,
            shell: shell::ShellExecutor::new(cfg),
            history,
        })
    }

    /// Execute one full dispatch (all retries, all history rows).
    /// `cancel` is signalled by `POST /stop`; the retry loop
    /// checks it between attempts and shell/HTTP executors race
    /// it against the in-flight future.
    pub async fn dispatch(
        &self,
        spec: &JobSpec,
        extras: DispatchExtras,
        cancel: Arc<Notify>,
    ) -> DispatchOutcome {
        let max_attempts = spec.retry.count + 1;
        let mut last_error: Option<String> = None;

        // Snapshot for logs — group/name/method/url are cheap to
        // clone once, avoids re-borrowing spec inside the loop.
        let group = spec.key.group.clone();
        let name = spec.key.name.clone();
        let (http_method_log, http_url_log): (Option<String>, Option<String>) = match &spec.kind {
            JobKind::Http { method, url } => (Some(method.clone()), Some(url.clone())),
            JobKind::Exec { .. } => (None, None),
        };

        for attempt in 1..=max_attempts {
            let started = Instant::now();
            let attempt_at = Utc::now();

            // Per-attempt DEBUG log — mirrors JVM
            // `HttpHelper.java:39`.
            match &spec.kind {
                JobKind::Http { method, url } => tracing::debug!(
                    "http: attempt {}/{} for {} {} ({}/{})",
                    attempt,
                    max_attempts,
                    method,
                    url,
                    group,
                    name
                ),
                JobKind::Exec { command, .. } => tracing::debug!(
                    "shell: attempt {}/{} for {} ({}/{})",
                    attempt,
                    max_attempts,
                    command,
                    group,
                    name
                ),
            }

            let result = match &spec.kind {
                JobKind::Http { method, url } => self
                    .http
                    .execute(method, url, cancel.clone())
                    .await
                    .map(HttpDispatchResult::from_ok)
                    .map_err(HttpDispatchResult::from_err),
                JobKind::Exec {
                    command,
                    allowed_envs,
                } => self
                    .shell
                    .execute(command, allowed_envs, &extras.env_overrides, cancel.clone())
                    .await
                    .map(HttpDispatchResult::from_shell_ok)
                    .map_err(HttpDispatchResult::from_shell_err),
            };
            let duration_ms = started.elapsed().as_millis() as i64;

            // Structured logs mirroring the JVM `HttpHelper` /
            // shell log lines: INFO on retry-success, WARN per
            // failed retry, ERROR/WARN at end of the retry loop.
            match (&result, attempt, max_attempts) {
                (Ok(_), a, _) if a > 1 => match (&http_method_log, &http_url_log) {
                    (Some(m), Some(u)) => tracing::info!(
                        "http: request succeeded on attempt {}/{} for {} {} ({}/{})",
                        a,
                        max_attempts,
                        m,
                        u,
                        group,
                        name
                    ),
                    _ => tracing::info!(
                        "shell: request succeeded on attempt {}/{} ({}/{})",
                        a,
                        max_attempts,
                        group,
                        name
                    ),
                },
                (
                    Err(HttpDispatchResult {
                        error_message: Some(msg),
                        ..
                    }),
                    a,
                    max,
                ) if a < max => match (&http_method_log, &http_url_log) {
                    (Some(m), Some(u)) => tracing::warn!(
                        "http: attempt {}/{} failed for {} {} ({}/{}): {}",
                        a,
                        max,
                        m,
                        u,
                        group,
                        name,
                        msg
                    ),
                    _ => tracing::warn!(
                        "shell: attempt {}/{} failed ({}/{}): {}",
                        a,
                        max,
                        group,
                        name,
                        msg
                    ),
                },
                _ => {}
            }

            let (status_for_history, err_for_next_iter, response_body, http_status_code) =
                match &result {
                    Ok(HttpDispatchResult {
                        response_body,
                        http_status_code,
                        ..
                    }) => (
                        ExecutionStatus::Success,
                        None,
                        response_body.clone(),
                        *http_status_code,
                    ),
                    Err(HttpDispatchResult {
                        error_message,
                        http_status_code,
                        ..
                    }) => {
                        let s = if attempt < max_attempts {
                            ExecutionStatus::Retrying
                        } else {
                            ExecutionStatus::Failed
                        };
                        (s, error_message.clone(), None, *http_status_code)
                    }
                };

            // Special-case the shell timeout so it lands in
            // history as TIMEOUT, not FAILED.
            let status_for_history = if let Err(HttpDispatchResult {
                is_shell_timeout: true,
                ..
            }) = &result
            {
                ExecutionStatus::Timeout
            } else {
                status_for_history
            };

            let (method, url) = match &spec.kind {
                JobKind::Http { method, url } => (Some(method.clone()), Some(url.clone())),
                JobKind::Exec { .. } => (None, None),
            };

            self.history
                .record(HistoryEntry {
                    execution_time: attempt_at,
                    job_name: spec.key.name.clone(),
                    job_group: spec.key.group.clone(),
                    job_type: spec.kind.type_label().to_string(),
                    duration_ms: Some(duration_ms),
                    status: status_for_history,
                    http_method: method,
                    http_url: url,
                    http_status_code,
                    attempt_number: attempt as i32,
                    max_attempts: max_attempts as i32,
                    response_body,
                    error_message: err_for_next_iter.clone(),
                })
                .await;

            match result {
                Ok(_) => {
                    return DispatchOutcome {
                        status: ExecutionStatus::Success,
                        attempts: attempt,
                        last_error: None,
                    };
                }
                Err(HttpDispatchResult {
                    is_shell_timeout, ..
                }) => {
                    last_error = err_for_next_iter;
                    if is_shell_timeout {
                        // Timeouts don't get retried — the job
                        // just hit the wall clock, retrying
                        // would hit it again.
                        return DispatchOutcome {
                            status: ExecutionStatus::Timeout,
                            attempts: attempt,
                            last_error,
                        };
                    }
                    if attempt < max_attempts {
                        // Wait, but race against cancellation.
                        let delay = spec.retry.delay;
                        tokio::select! {
                            _ = tokio::time::sleep(delay) => {},
                            _ = cancel.notified() => {
                                return DispatchOutcome {
                                    status: ExecutionStatus::Failed,
                                    attempts: attempt,
                                    last_error: Some("aborted by /stop".into()),
                                };
                            }
                        }
                    }
                }
            }
        }

        // All attempts exhausted. `ignoreFailures: true` maps
        // FAILED → SKIPPED for the caller's outcome but the
        // per-attempt history rows retain their real status.
        let final_status = if spec.retry.ignore_failures {
            match (&http_method_log, &http_url_log) {
                (Some(m), Some(u)) => tracing::warn!(
                    "http: ignoring failure after {} attempts for {} {} ({}/{})",
                    max_attempts,
                    m,
                    u,
                    group,
                    name
                ),
                _ => tracing::warn!(
                    "shell: ignoring failure after {} attempts ({}/{})",
                    max_attempts,
                    group,
                    name
                ),
            }
            ExecutionStatus::Skipped
        } else {
            let err_desc = last_error
                .clone()
                .unwrap_or_else(|| "no message".to_string());
            match (&http_method_log, &http_url_log) {
                (Some(m), Some(u)) => tracing::error!(
                    "http: all {} attempts failed for {} {} ({}/{}): {}",
                    max_attempts,
                    m,
                    u,
                    group,
                    name,
                    err_desc
                ),
                _ => tracing::error!(
                    "shell: all {} attempts failed ({}/{}): {}",
                    max_attempts,
                    group,
                    name,
                    err_desc
                ),
            }
            ExecutionStatus::Failed
        };
        DispatchOutcome {
            status: final_status,
            attempts: max_attempts,
            last_error,
        }
    }
}

/// Intermediate carrier used inside `dispatch` to unify HTTP + shell
/// outcomes for the history record.
struct HttpDispatchResult {
    pub response_body: Option<String>,
    pub http_status_code: Option<i32>,
    pub error_message: Option<String>,
    pub is_shell_timeout: bool,
}

impl HttpDispatchResult {
    fn from_ok(res: http::HttpAttempt) -> Self {
        Self {
            response_body: Some(res.body),
            http_status_code: Some(res.status as i32),
            error_message: None,
            is_shell_timeout: false,
        }
    }
    fn from_err(err: CronManagerError) -> Self {
        let http_status_code = match &err {
            CronManagerError::UpstreamHttpError { status, .. } => Some(*status as i32),
            _ => None,
        };
        Self {
            response_body: None,
            http_status_code,
            error_message: Some(err.to_string()),
            is_shell_timeout: false,
        }
    }
    fn from_shell_ok(res: shell::ShellAttempt) -> Self {
        Self {
            response_body: Some(res.stdout),
            http_status_code: None,
            error_message: None,
            is_shell_timeout: false,
        }
    }
    fn from_shell_err(err: CronManagerError) -> Self {
        let is_shell_timeout = matches!(err, CronManagerError::ShellTimeout { .. });
        Self {
            response_body: None,
            http_status_code: None,
            error_message: Some(err.to_string()),
            is_shell_timeout,
        }
    }
}

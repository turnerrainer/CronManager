//! DSL types shared by loader + scheduler + executors.
//!
//! Wire shape: see book/src/configuration.md §"YAML job file
//! shape". Every JVM DSL field has a Rust equivalent here.

use serde::Serialize;
use std::time::Duration;

pub mod loader;

/// Fully-qualified job identity. `(group, name)` matches the
/// JVM CronManager's Quartz-style key. Group is derived from the
/// file's path relative to `dsl_path`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct JobKey {
    pub group: String,
    pub name: String,
}

impl JobKey {
    pub fn new(group: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            group: group.into(),
            name: name.into(),
        }
    }
}

/// Two firing modes: a Quartz-style cron expression, or "manual
/// only". The JVM version accepted the literals `off` and
/// `false` (both bool and string) for manual mode; the loader
/// normalises all four onto `Trigger::Manual`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trigger {
    /// The raw expression, retained so `/jobs` can display it
    /// unmodified. Validation happened at load time.
    Cron(String),
    Manual,
}

/// Optional wall-clock window. Both bounds are Unix epoch
/// millis, matching the JVM DSL's `startDate`/`endDate` fields.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TimeWindow {
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
}

impl TimeWindow {
    pub fn allows(&self, now_ms: i64) -> bool {
        if let Some(s) = self.start_ms {
            if now_ms < s {
                return false;
            }
        }
        if let Some(e) = self.end_ms {
            if now_ms > e {
                return false;
            }
        }
        true
    }
}

/// Retry policy per-attempt. `count` is the number of retries
/// AFTER the first attempt — total attempts = `count + 1`.
/// Matches JVM `HttpHelper.doRequestWithRetry`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryPolicy {
    pub count: u32,
    pub delay: Duration,
    pub ignore_failures: bool,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            count: 0,
            delay: Duration::from_millis(1000),
            ignore_failures: false,
        }
    }
}

/// The two job kinds CronManager knows about. Every other
/// dimension (schedule, window, retry) is shared via `JobSpec`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobKind {
    Http {
        method: String,
        url: String,
    },
    Exec {
        /// Executable + arguments as a single command string.
        /// The shell executor tokenises this the same way JVM
        /// `Runtime.exec(String)` does — whitespace-split, no
        /// shell interpretation.
        command: String,
        allowed_envs: Vec<String>,
    },
}

impl JobKind {
    pub fn type_label(&self) -> &'static str {
        match self {
            JobKind::Http { .. } => "http",
            JobKind::Exec { .. } => "exec",
        }
    }
}

/// One scheduled unit of work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobSpec {
    pub key: JobKey,
    pub trigger: Trigger,
    pub window: TimeWindow,
    pub retry: RetryPolicy,
    pub kind: JobKind,
}

/// Path from which this job was loaded — retained for error
/// messages and log lines. Not part of `JobSpec` equality.
#[derive(Debug, Clone)]
pub struct LoadedJob {
    pub spec: JobSpec,
    pub source_path: std::path::PathBuf,
}

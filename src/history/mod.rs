//! Execution history recorder. Two impls:
//!
//! * [`NoopRecorder`] — used when the operator has not configured
//!   a database. History is dropped after a DEBUG log line.
//! * [`postgres::PostgresRecorder`] — sqlx-backed writer against
//!   the schema shipped in `migrations/`.
//!
//! The recorder trait is `dyn`-friendly so the scheduler holds an
//! `Arc<dyn HistoryRecorder>` and the choice between impls is made
//! once at startup.

use async_trait::async_trait;
use chrono::{DateTime, Utc};

pub mod postgres;

/// One row in `job_execution_history`. Field naming mirrors the
/// JVM Liquibase changelog for schema fidelity (see the original
/// CronManager repository at https://github.com/buerokratt/CronManager).
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub execution_time: DateTime<Utc>,
    pub job_name: String,
    pub job_group: String,
    pub job_type: String,
    pub duration_ms: Option<i64>,
    pub status: ExecutionStatus,
    pub http_method: Option<String>,
    pub http_url: Option<String>,
    pub http_status_code: Option<i32>,
    pub attempt_number: i32,
    pub max_attempts: i32,
    pub response_body: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionStatus {
    Success,
    Failed,
    Retrying,
    Skipped,
    Timeout,
}

impl ExecutionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Success => "SUCCESS",
            Self::Failed => "FAILED",
            Self::Retrying => "RETRYING",
            Self::Skipped => "SKIPPED",
            Self::Timeout => "TIMEOUT",
        }
    }
}

#[async_trait]
pub trait HistoryRecorder: Send + Sync {
    async fn record(&self, entry: HistoryEntry);
}

/// Drops entries after logging. Used when `database.url` is not
/// set — jobs still run, history just isn't persisted.
#[derive(Debug, Default)]
pub struct NoopRecorder;

#[async_trait]
impl HistoryRecorder for NoopRecorder {
    async fn record(&self, entry: HistoryEntry) {
        tracing::debug!(
            "history (noop): {}/{} status={} attempt={}/{}",
            entry.job_group,
            entry.job_name,
            entry.status.as_str(),
            entry.attempt_number,
            entry.max_attempts
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_labels_match_jvm_enum() {
        assert_eq!(ExecutionStatus::Success.as_str(), "SUCCESS");
        assert_eq!(ExecutionStatus::Failed.as_str(), "FAILED");
        assert_eq!(ExecutionStatus::Retrying.as_str(), "RETRYING");
        assert_eq!(ExecutionStatus::Skipped.as_str(), "SKIPPED");
        assert_eq!(ExecutionStatus::Timeout.as_str(), "TIMEOUT");
    }

    #[tokio::test]
    async fn noop_recorder_is_infallible() {
        let r = NoopRecorder;
        r.record(HistoryEntry {
            execution_time: Utc::now(),
            job_name: "n".into(),
            job_group: "g".into(),
            job_type: "http".into(),
            duration_ms: Some(10),
            status: ExecutionStatus::Success,
            http_method: Some("GET".into()),
            http_url: Some("https://example.com".into()),
            http_status_code: Some(200),
            attempt_number: 1,
            max_attempts: 1,
            response_body: None,
            error_message: None,
        })
        .await;
    }
}

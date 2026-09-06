//! Postgres/TimescaleDB history recorder.
//!
//! Migrations under `migrations/` are applied at pool
//! construction time via `sqlx::migrate!()`. On plain Postgres,
//! the TimescaleDB-specific statements in migration 03 are
//! guarded and become no-ops.

use crate::error::CronManagerError;
use crate::history::{HistoryEntry, HistoryRecorder};
use async_trait::async_trait;
use sqlx::postgres::{PgPool, PgPoolOptions};
use std::time::Duration;

pub struct PostgresRecorder {
    pool: PgPool,
    /// Cap on the bytes stored per row for `response_body` /
    /// stdout. Above the cap we retain head+tail with a marker so
    /// a chatty upstream / script can't bloat the hypertable
    /// unboundedly. See `AUDIT.md` finding M5.
    stored_body_max_bytes: usize,
}

impl PostgresRecorder {
    /// Connect, run migrations, return the recorder. Fails
    /// fast if the DB is unreachable or migrations can't apply.
    /// `stored_body_max_bytes` is the per-row cap for
    /// `response_body`; use `SecurityConfig::default().stored_response_body_max_bytes`
    /// (64 KiB) unless the operator has an explicit reason to
    /// change it.
    pub async fn connect(
        dsn: &str,
        stored_body_max_bytes: usize,
    ) -> Result<Self, CronManagerError> {
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .acquire_timeout(Duration::from_secs(10))
            .connect(dsn)
            .await
            .map_err(|e| CronManagerError::Database(format!("connect: {e}")))?;
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .map_err(|e| CronManagerError::Database(format!("migrate: {e}")))?;
        Ok(Self {
            pool,
            stored_body_max_bytes,
        })
    }

    /// Borrow the underlying pool. Exposed so integration tests
    /// (and any future feature that needs richer queries) can
    /// share the same connection pool the recorder writes with.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

#[async_trait]
impl HistoryRecorder for PostgresRecorder {
    async fn record(&self, entry: HistoryEntry) {
        // Absorb any DB error into a WARN log. Losing a history
        // row is not worth failing the job — the operator will
        // see the log and investigate.
        let status_str = entry.status.as_str();
        // Cap the persisted response_body — 128 MiB of shell
        // stdout in a hypertable will page out something an
        // operator actually needs. See M5.
        let body_capped: Option<String> = entry
            .response_body
            .as_deref()
            .map(|s| crate::security::truncate_response_body(s, self.stored_body_max_bytes));
        let result = sqlx::query(
            "INSERT INTO job_execution_history \
             (execution_time, job_name, job_group, job_type, duration_ms, status, \
              http_method, http_url, http_status_code, attempt_number, max_attempts, \
              response_body, error_message) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
        )
        .bind(entry.execution_time)
        .bind(&entry.job_name)
        .bind(&entry.job_group)
        .bind(&entry.job_type)
        .bind(entry.duration_ms)
        .bind(status_str)
        .bind(entry.http_method.as_deref())
        .bind(entry.http_url.as_deref())
        .bind(entry.http_status_code)
        .bind(entry.attempt_number)
        .bind(entry.max_attempts)
        .bind(body_capped.as_deref())
        .bind(entry.error_message.as_deref())
        .execute(&self.pool)
        .await;
        if let Err(e) = result {
            tracing::warn!(
                "failed to persist history for {}/{}: {e}",
                entry.job_group,
                entry.job_name
            );
        }
    }
}

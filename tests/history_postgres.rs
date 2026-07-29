//! Integration test — Postgres/TimescaleDB history recorder.
//!
//! DEV-REQUIREMENTS §3: no mocking of databases. This test
//! requires `CRONMANAGER_TEST_DATABASE_URL` to be set to a live
//! Postgres (or TimescaleDB) DSN. It is skipped otherwise so
//! that `cargo test` on a fresh clone still passes.
//!
//! CI provides the DSN via a Postgres service container in
//! `.github/workflows/tests.yml`. Locals can run:
//!
//! ```bash
//! docker run -d --rm --name ct-pg \
//!   -e POSTGRES_USER=cronmanager \
//!   -e POSTGRES_PASSWORD=cronmanager_test_password \
//!   -e POSTGRES_DB=cronmanager_test \
//!   -p 5432:5432 \
//!   timescale/timescaledb:2.17.2-pg16
//! CRONMANAGER_TEST_DATABASE_URL=postgres://cronmanager:cronmanager_test_password@localhost:5432/cronmanager_test \
//!   cargo test --test history_postgres -- --nocapture
//! ```

use chrono::Utc;
use cronmanager_on_rust::history::postgres::PostgresRecorder;
use cronmanager_on_rust::history::{ExecutionStatus, HistoryEntry, HistoryRecorder};

fn dsn() -> Option<String> {
    std::env::var("CRONMANAGER_TEST_DATABASE_URL").ok()
}

fn sample_entry(job_name: &str) -> HistoryEntry {
    HistoryEntry {
        execution_time: Utc::now(),
        job_name: job_name.into(),
        job_group: "test".into(),
        job_type: "http".into(),
        duration_ms: Some(42),
        status: ExecutionStatus::Success,
        http_method: Some("GET".into()),
        http_url: Some("https://example.com".into()),
        http_status_code: Some(200),
        attempt_number: 1,
        max_attempts: 1,
        response_body: Some("ok".into()),
        error_message: None,
    }
}

#[tokio::test]
async fn record_and_read_back_a_row() {
    let Some(dsn) = dsn() else {
        eprintln!("skipping: CRONMANAGER_TEST_DATABASE_URL not set");
        return;
    };
    let recorder = PostgresRecorder::connect(&dsn)
        .await
        .expect("connect + migrate");
    let entry = sample_entry("row_read_back");
    recorder.record(entry.clone()).await;

    // Read it back to prove the write hit the table.
    let count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM job_execution_history WHERE job_name = $1")
            .bind(&entry.job_name)
            .fetch_one(recorder.pool())
            .await
            .expect("select count");
    assert!(count.0 >= 1);
}

#[tokio::test]
async fn migrations_are_idempotent() {
    let Some(dsn) = dsn() else {
        eprintln!("skipping: CRONMANAGER_TEST_DATABASE_URL not set");
        return;
    };
    // Two connects → migrate() runs twice; second run must be a
    // no-op. If migrations aren't idempotent, this panics.
    let _first = PostgresRecorder::connect(&dsn)
        .await
        .expect("first connect");
    let _second = PostgresRecorder::connect(&dsn)
        .await
        .expect("second connect");
}

#[tokio::test]
async fn every_status_variant_persists() {
    let Some(dsn) = dsn() else {
        eprintln!("skipping: CRONMANAGER_TEST_DATABASE_URL not set");
        return;
    };
    let recorder = PostgresRecorder::connect(&dsn).await.unwrap();
    for (name, status) in [
        ("status_success", ExecutionStatus::Success),
        ("status_failed", ExecutionStatus::Failed),
        ("status_retrying", ExecutionStatus::Retrying),
        ("status_skipped", ExecutionStatus::Skipped),
        ("status_timeout", ExecutionStatus::Timeout),
    ] {
        let mut e = sample_entry(name);
        e.status = status;
        recorder.record(e).await;

        let row: (String,) =
            sqlx::query_as("SELECT status FROM job_execution_history WHERE job_name = $1 LIMIT 1")
                .bind(name)
                .fetch_one(recorder.pool())
                .await
                .expect("fetch back");
        assert_eq!(row.0, status.as_str());
    }
}

-- 20260729000001 — create job_execution_history table.
--
-- Schema is a Rust-native replay of the JVM Liquibase changelog
-- (see the original CronManager repository at
-- https://github.com/buerokratt/CronManager/tree/main/src/main/resources/db/changelog).
-- All statements are idempotent so this migration is safe to
-- re-run against a partially-initialised database.

-- TimescaleDB extension (no-op if the base Postgres image already
-- carries it; the timescale/timescaledb image does).
CREATE EXTENSION IF NOT EXISTS timescaledb;

CREATE TABLE IF NOT EXISTS job_execution_history (
    id                 BIGSERIAL,
    execution_time     TIMESTAMPTZ NOT NULL,
    job_name           VARCHAR(255) NOT NULL,
    job_group          VARCHAR(255),
    job_type           VARCHAR(50),
    duration_ms        BIGINT,
    status             VARCHAR(50),
    http_method        VARCHAR(10),
    http_url           TEXT,
    http_status_code   INTEGER,
    attempt_number     INTEGER DEFAULT 1,
    max_attempts       INTEGER,
    retry_reason       TEXT,
    response_body      TEXT,
    error_message      TEXT,
    stack_trace        TEXT,
    PRIMARY KEY (id, execution_time)
);

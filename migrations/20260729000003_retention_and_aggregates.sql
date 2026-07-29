-- 20260729000003 — retention policy + continuous aggregates.
--
-- These are TimescaleDB-only. On plain Postgres, the CREATE
-- MATERIALIZED VIEW statements will succeed but the WITH clause
-- and add_*_policy() calls fail. We wrap in a DO block that
-- checks for the extension so the migration is a no-op on
-- non-Timescale deployments.

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'timescaledb') THEN
        PERFORM add_retention_policy(
            'job_execution_history',
            INTERVAL '90 days',
            if_not_exists => TRUE
        );
    END IF;
END $$;

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'timescaledb') THEN
        EXECUTE $ca$
            CREATE MATERIALIZED VIEW IF NOT EXISTS job_execution_hourly_stats
            WITH (timescaledb.continuous) AS
            SELECT
                time_bucket('1 hour', execution_time) AS hour,
                job_name,
                job_type,
                COUNT(*) AS total_executions,
                COUNT(*) FILTER (WHERE status = 'SUCCESS') AS successful_executions,
                COUNT(*) FILTER (WHERE status = 'FAILED')  AS failed_executions,
                COUNT(*) FILTER (WHERE status = 'RETRYING') AS retrying_executions,
                AVG(duration_ms) AS avg_duration_ms,
                MAX(duration_ms) AS max_duration_ms,
                MIN(duration_ms) AS min_duration_ms,
                AVG(attempt_number) AS avg_attempts
            FROM job_execution_history
            GROUP BY hour, job_name, job_type
            WITH NO DATA;
        $ca$;

        PERFORM add_continuous_aggregate_policy(
            'job_execution_hourly_stats',
            start_offset => INTERVAL '3 hours',
            end_offset   => INTERVAL '1 hour',
            schedule_interval => INTERVAL '1 hour',
            if_not_exists => TRUE
        );
    END IF;
END $$;

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'timescaledb') THEN
        EXECUTE $ca$
            CREATE MATERIALIZED VIEW IF NOT EXISTS job_execution_daily_stats
            WITH (timescaledb.continuous) AS
            SELECT
                time_bucket('1 day', execution_time) AS day,
                job_name,
                job_type,
                COUNT(*) AS total_executions,
                COUNT(*) FILTER (WHERE status = 'SUCCESS') AS successful_executions,
                COUNT(*) FILTER (WHERE status = 'FAILED')  AS failed_executions,
                AVG(duration_ms) AS avg_duration_ms,
                PERCENTILE_CONT(0.5)  WITHIN GROUP (ORDER BY duration_ms) AS median_duration_ms,
                PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY duration_ms) AS p95_duration_ms,
                PERCENTILE_CONT(0.99) WITHIN GROUP (ORDER BY duration_ms) AS p99_duration_ms
            FROM job_execution_history
            GROUP BY day, job_name, job_type
            WITH NO DATA;
        $ca$;

        PERFORM add_continuous_aggregate_policy(
            'job_execution_daily_stats',
            start_offset => INTERVAL '7 days',
            end_offset   => INTERVAL '1 day',
            schedule_interval => INTERVAL '1 day',
            if_not_exists => TRUE
        );
    END IF;
END $$;

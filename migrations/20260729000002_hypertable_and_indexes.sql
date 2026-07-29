-- 20260729000002 — convert to hypertable, add indexes.

SELECT create_hypertable(
    'job_execution_history',
    'execution_time',
    if_not_exists => TRUE
);

CREATE INDEX IF NOT EXISTS idx_job_name
    ON job_execution_history (job_name);

CREATE INDEX IF NOT EXISTS idx_execution_time
    ON job_execution_history (execution_time DESC);

CREATE INDEX IF NOT EXISTS idx_status
    ON job_execution_history (status);

CREATE INDEX IF NOT EXISTS idx_job_name_time
    ON job_execution_history (job_name, execution_time DESC);

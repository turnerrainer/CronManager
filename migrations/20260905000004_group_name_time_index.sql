-- 20260905000004 — add compound (group, name, time) index.
--
-- Rationale: the JVM CronController's history pages issued
-- `WHERE job_group = $1 AND job_name = $2 ORDER BY execution_time DESC`
-- style queries, and the current schema forces a scan for that
-- pattern (only `job_name` alone and `execution_time` alone are
-- indexed). Adding the compound index lets those pages return in
-- O(log n) as the hypertable grows.
--
-- See AUDIT.md finding M5.

CREATE INDEX IF NOT EXISTS idx_group_name_time
    ON job_execution_history (job_group, job_name, execution_time DESC);

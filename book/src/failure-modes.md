# Failure modes

Every response CronManager emits carries a JSON body of the shape
`{ "error": "<code>", "message": "<human>" }` on failure.

## HTTP status codes

| Status | `error` code | When |
|---|---|---|
| `200` | *(no error field)* | Success. Body carries the resource (list, snapshot, etc.). |
| `400` | `bad_request` | Malformed JSON body. |
| `400` | `invalid_job_definition` | A `.yaml` under `dsl_path` referenced an unknown `type`, unknown HTTP `method`, missing `url`/`command`, or `trigger: true`. |
| `400` | `invalid_cron` | A `trigger:` string that isn't parseable as a 6-field Quartz cron expression. |
| `400` | `invalid_config` | `cronmanager.yaml` contained a top-level JVM Spring wrapper (`application:`, `spring:`, …) or an unknown field. |
| `404` | `job_not_found` | `/execute/{group}/{job}` or `/stop/{group}/{job}` referenced a job that isn't scheduled. |
| `409` | `job_already_running` | `/execute` fired on a job whose previous invocation is still running. |
| `413` | `request_too_large` | Inbound body exceeded `limits.max_request_bytes`. |
| `500` | `internal_error` | Unexpected failure — check server logs. |
| `500` | `shell_failed` | Shell job exited non-zero. |
| `500` | `shell_timeout` | Shell job exceeded `limits.shell_timeout_secs` wall clock. |
| `500` | `io_error` | Filesystem or process I/O error (e.g. `command` script not found). |
| `500` | `yaml_parse_error` | A `.yaml` under `dsl_path` didn't parse — line/column in the message. |
| `500` | `database_error` | History persistence write failed. |
| `502` | `upstream_http_error` | HTTP job's target returned a non-2xx status after all retries exhausted. |
| `502` | `upstream_body_too_large` | HTTP job's target response exceeded `limits.max_response_bytes`. |
| `504` | `upstream_timeout` | HTTP job's target did not respond within `limits.request_timeout_secs`. |

For `413` and `502 upstream_body_too_large` the JSON body also
carries a `"limit": <bytes>` field so clients can render the
actual cap in their error UI.

## Job execution outcomes

Written to the `job_execution_history.status` column when
persistence is enabled:

| Value | Meaning |
|---|---|
| `SUCCESS` | Attempt completed. HTTP: 2xx response. Shell: exit status 0. |
| `RETRYING` | Attempt failed, another retry is queued. |
| `FAILED` | Final attempt failed and `ignoreFailures` was false. |
| `SKIPPED` | Fire suppressed by `startDate`/`endDate` window, or `ignoreFailures: true` swallowed a failure. |
| `TIMEOUT` | Shell job exceeded `limits.shell_timeout_secs` and was killed. |

## Common causes

- **Job not firing on schedule**: check that `trigger` is a valid
  6-field Quartz cron expression. `0 */5 * * * ?` (every five
  minutes) is a good sanity template. If the expression parses but
  the fire time never arrives, remember the scheduler evaluates
  in UTC — see [Timezone](#timezone) below.
- **Boot fails with `unknown field 'X'`**: a config or DSL typo.
  The error message lists the fields the loader expected — pick
  the closest match. Unlike the JVM CronManager, unknown fields
  are never silently ignored.
- **Boot fails with `top-level 'application:' is the JVM Spring
  wrapper`**: you pasted a JVM `application.yml` verbatim. The
  Rust config schema is flat — unwrap the child fields to the top
  level. See [JVM porting quickref](./jvm-porting.md).
- **Shell job fails immediately with `io_error`**: verify the
  script is executable (`chmod +x`) and the path is resolved
  relative to `app_root_path` (default `/app`).
- **HTTP job times out**: raise `limits.request_timeout_secs` in
  `cronmanager.yaml`, or reduce upstream latency. The default 30 s
  is generous; timeouts usually indicate a real upstream issue.
- **History not being written**: confirm `database.url` is set and
  the env var named by `database.password_env` is exported. The
  startup log line `history: enabled` / `history: disabled` tells
  you which mode the process is in.
- **CORS preflight fails despite `allowed_origins: ["*"]`**: `*`
  is treated as an *exact origin* by the CORS layer, not a
  wildcard. List every origin explicitly. A boot WARN fires when
  `"*"` is present so this isn't a silent surprise.

## Timezone

The scheduler evaluates in UTC. A job that must fire at "09:00
Europe/Tallinn" needs its cron hour field set to `7` (winter,
UTC+2) or `6` (summer, UTC+3). Per-schedule TZ context is
tracked as a backlog item.

## Boot-time diagnostics

The process emits one INFO line at startup summarising every
config field, plus WARNs for likely-footgun values. Reading the
first 20 lines of the log is usually enough to answer "why is
this behaving unexpectedly?":

```
INFO cronmanager v0.1.0-alpha.2 starting
INFO loaded config from ./cronmanager.yaml
INFO config: port=8080 dsl_path=./DSL app_root_path=/app origins=0 shell_env_keys=2 history_db=true limits[req=1048576,resp=16777216,http_to=30s,shell_to=300s]
INFO history: enabled (migrations applied)
INFO shell: baseline env has 2 entries
INFO dsl: loading ./DSL/samples/http/health-check.yaml
INFO dsl: group samples-http-health-check → 1 job(s)
INFO scheduler: adding samples-http-health-check/health_check trigger 0 */5 * * * ?
INFO scheduler: 1 job(s) registered
INFO listening on 0.0.0.0:8080
```

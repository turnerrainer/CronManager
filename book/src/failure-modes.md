# Failure modes

Every response CronManager emits carries a JSON body of the shape
`{ "error": "<code>", "message": "<human>" }` on failure.

## HTTP status codes

| Status | `error` code | When |
|---|---|---|
| `200` | *(no error field)* | Success. Body carries the resource (list, snapshot, etc). |
| `400` | `bad_request` | Malformed JSON body, unknown HTTP method in a job def, unparseable cron expression at `/reload`. |
| `404` | `job_not_found` | `/execute/{group}/{job}` or `/stop/{group}/{job}` referenced a job that isn't scheduled. |
| `409` | `job_already_running` | `/execute` fired on a job whose previous invocation is still running. |
| `413` | `request_too_large` | Inbound body exceeded `limits.max_request_bytes`. |
| `500` | `internal_error` | Unexpected failure — check server logs. |
| `502` | `upstream_http_error` | HTTP job's target returned a non-2xx status after all retries exhausted. |
| `502` | `upstream_body_too_large` | HTTP job's target response exceeded `limits.max_response_bytes`. |
| `504` | `upstream_timeout` | HTTP job's target did not respond within `limits.request_timeout_secs`. |

## Job execution outcomes

Written to the `job_execution_history.status` column when
persistence is enabled:

| Value | Meaning |
|---|---|
| `SUCCESS` | Attempt completed. HTTP: 2xx response. Shell: exit status 0. |
| `RETRYING` | Attempt failed, another retry is queued. |
| `FAILED` | Final attempt failed and `ignoreFailures` was false. |
| `SKIPPED` | Fire suppressed by `startDate`/`endDate` window, or `ignoreFailures` swallowed a failure. |
| `TIMEOUT` | Shell job exceeded `limits.shell_timeout_secs` and was killed. |

## Common causes

- **Job not firing on schedule**: check that `trigger` is a valid
  6-field Quartz cron expression. `0 */5 * * * ?` (every five
  minutes) is a good sanity template.
- **Shell job fails immediately**: verify the script is
  executable (`chmod +x`) and the path is resolved from
  `app_root_path` (default `/app`).
- **HTTP job times out**: raise `limits.request_timeout_secs` in
  `cronmanager.yaml`, or reduce upstream latency. The default 30 s
  is generous; timeouts usually indicate a real upstream issue.
- **History not being written**: confirm `database.url` is set and
  the env var named by `database.password_env` is exported. The
  startup log line `history: enabled` / `history: disabled` tells
  you which mode the process is in.

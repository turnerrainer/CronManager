# Configuration

Everything is one YAML file: `cronmanager.yaml`. Search order:

1. `--config <path>` CLI flag
2. `CRONMANAGER_CONFIG` env var
3. `./cronmanager.yaml` or `./cronmanager.yml`
4. Built-in defaults

## Full example

```yaml
port: 8080
dsl_path: /app/DSL/samples
app_root_path: /app

allowed_origins:
  - "http://localhost"
  - "http://127.0.0.1"

shell_environment:
  BACKUP_DIR: /tmp/backups
  RETENTION_DAYS: "7"

limits:
  max_request_bytes: 1048576
  max_response_bytes: 16777216
  request_timeout_secs: 30
  shell_timeout_secs: 300

database:
  url: postgres://cronmanager@timescaledb:5432/cronmanager
  password_env: CRONMANAGER_DB_PASSWORD
```

## Field reference

| Field | Type | Default | Meaning |
|---|---|---|---|
| `port` | integer | `8080` | TCP port the axum server binds on `0.0.0.0`. |
| `dsl_path` | path | `./DSL` | Directory scanned recursively for `.yaml`/`.yml` job files at startup. |
| `app_root_path` | path | `/app` | Working directory used when a shell job executes. |
| `allowed_origins` | list of strings | `[]` | CORS allow-list. Empty list disables the CORS layer. |
| `shell_environment` | map of string→string | `{}` | Baseline env exposed to shell jobs. Each job still whitelists via `allowedEnvs`. |
| `limits.max_request_bytes` | integer | `1048576` (1 MiB) | Cap for inbound REST bodies. Overflow → `413`. |
| `limits.max_response_bytes` | integer | `16777216` (16 MiB) | Cap for HTTP responses stored in history. Overflow → truncated with marker. |
| `limits.request_timeout_secs` | integer | `30` | HTTP outbound timeout for HTTP jobs. |
| `limits.shell_timeout_secs` | integer | `300` | Shell job wall-clock cap. Overrun → SIGKILL + `TIMEOUT` history entry. |
| `database.url` | connection string | *(unset)* | PostgreSQL/TimescaleDB DSN. When unset, execution history is disabled. |
| `database.password_env` | env var name | `CRONMANAGER_DB_PASSWORD` | Env var to read the DB password from. Startup refuses if `database.url` is set but the env var is missing. |

## YAML job file shape

Each `.yaml` file under `dsl_path` is a map from job name to a job
definition. Example:

```yaml
health_check:
  trigger: "0 */5 * * * ?"
  type: http
  method: GET
  url: https://httpbin.org/status/200
  retryCount: 3
  retryDelay: 1000
  ignoreFailures: true

daily_backup:
  trigger: "0 0 2 * * ?"
  type: exec
  command: ./scripts/samples/backup.sh
  allowedEnvs:
    - BACKUP_DIR
    - RETENTION_DAYS
```

Common job fields:

| Field | Applies to | Meaning |
|---|---|---|
| `trigger` | all | Quartz cron expression (6 fields) OR the literal `off` / `false` for manual-only jobs. |
| `type` | all | `http` or `exec`. |
| `startDate` | all | Unix millis; job silently skips fires before this. |
| `endDate` | all | Unix millis; job silently skips fires after this. |
| `retryCount` | all | Number of retries after the first attempt. Default 0. |
| `retryDelay` | all | Sleep between retries in milliseconds. Default 1000. |
| `ignoreFailures` | all | If true, absorb all-attempts-failed. Default false. |
| `method` | http | `GET` / `POST` / `PUT` / `DELETE` / `PATCH`. |
| `url` | http | Full HTTPS URL. |
| `command` | exec | Path (relative to `app_root_path`) or absolute path to the executable. |
| `allowedEnvs` | exec | List of env var names to export from `shell_environment` into the child process. |

Group names are derived from the file's path relative to
`dsl_path`, with directory separators replaced by `-` and the
extension dropped. `DSL/samples/http/health-check.yaml` yields
group `samples-http-health-check`.

## REST API surface

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/health` | Liveness probe. |
| `GET` | `/` | Sentinel string. |
| `GET` | `/jobs`, `/jobs/{group}` | List scheduled jobs. |
| `GET` | `/running`, `/running/{group}` | List jobs currently executing. |
| `POST` | `/execute/{group}/{job}` | Fire a job now. Optional query params for shell jobs are matched against `allowedEnvs`. |
| `POST` | `/stop/{group}/{job}` | Best-effort abort. |
| `POST` | `/reload/{group}` | Re-scan `dsl_path` and re-register any changed jobs. Group is a filter only. |

See [Failure modes](./failure-modes.md) for the status codes each
endpoint can return.

## Environment overrides

Two env vars are read directly by the process (never a config
field):

| Env var | Purpose |
|---|---|
| `CRONMANAGER_CONFIG` | Absolute path to `cronmanager.yaml`. |
| `CRONMANAGER_DB_PASSWORD` (or whatever `database.password_env` names) | The DB password. |
| `RUST_LOG` | `tracing_subscriber` filter — e.g. `info`, `cronmanager=debug`. |

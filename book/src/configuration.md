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
| `allowed_origins` | list of strings **or** comma-separated string | `[]` | CORS allow-list. Empty list disables the CORS layer. Exact-match only (no wildcards). |
| `shell_environment` | map of string→string | `{}` | Baseline env exposed to shell jobs. Each job still whitelists via `allowedEnvs`. |
| `limits.max_request_bytes` | integer | `1048576` (1 MiB) | Cap for inbound REST bodies. Overflow → `413`. |
| `limits.max_response_bytes` | integer | `16777216` (16 MiB) | Cap for HTTP responses stored in history. Overflow → truncated with marker. |
| `limits.request_timeout_secs` | integer | `30` | HTTP outbound timeout for HTTP jobs. `0` disables the cap (a WARN is emitted at boot). |
| `limits.shell_timeout_secs` | integer | `300` | Shell job wall-clock cap. Overrun → SIGKILL + `TIMEOUT` history entry. `0` disables the cap (a WARN is emitted at boot). |
| `database.url` | connection string | *(unset)* | PostgreSQL/TimescaleDB DSN without credentials. When unset, execution history is disabled. |
| `database.password_env` | env var name | `CRONMANAGER_DB_PASSWORD` | Env var to read the DB password from. Startup refuses if `database.url` is set but the env var is missing. |
| `admin.bearer_token_env` | env var name | `CRONMANAGER_ADMIN_TOKEN` | Env var containing the bearer token that gates `POST /execute`, `POST /stop`, `POST /reload`. Empty/unset → gate disabled. |
| `admin.trust_network` | bool | `false` | Skip the boot-time refusal to start on a non-loopback bind without a token. Only safe when a reverse proxy / service mesh authenticates every request before it reaches this process. |
| `security.block_private_networks` | bool | `true` | Refuse HTTP job URLs whose host is a private / loopback / link-local / ULA IP, both at load (literal IPs) and at fire time (DNS resolution). |
| `security.allow_dangerous_env_overrides` | bool | `false` | Bypass the shell dangerous-env blacklist (`PATH`, `LD_*`, `DYLD_*`, `PYTHONPATH`, `NODE_OPTIONS`, …). |
| `security.max_query_params` | integer | `64` | Cap on the number of `?k=v` pairs on `POST /execute/…`. Overflow → `413 too_many_query_params`. |
| `security.max_dsl_file_bytes` | integer | `1048576` (1 MiB) | Per-file cap on YAML DSL size, refused at load with a diagnostic that names the file. |
| `security.dsl_load_timeout_secs` | integer | `15` | Wall-clock cap on `load_all`. Above → boot / reload aborts with a `dsl load exceeded …s` error. |
| `security.max_retry_count` | integer | `10` | Cap on the `retryCount` DSL field. Above → load rejected. WARN emitted at `retryCount > 5`. |
| `security.min_cron_interval_secs` | integer | `10` | WARN if a cron expression fires more often than this. `0` disables the warning. |
| `security.stored_response_body_max_bytes` | integer | `65536` (64 KiB) | Cap on `response_body` per history row. Above → head+tail slice with an inline truncation marker. |
| `security.reload_min_interval_secs` | integer | `60` | Minimum seconds between two `POST /reload/…` calls for the same group. Above → `429 too_many_requests`. `0` disables the throttle. |

### Security posture

Zero-config on loopback stays open (the gate short-circuits when
no token is set) so `docker run -p 127.0.0.1:8080:8080` still gives
you the JVM-like local dev experience. Bind to `0.0.0.0` without a
token and the process refuses to start with:

```
Error: refusing to start on non-loopback bind 0.0.0.0:8080 without
  an admin token: set env var CRONMANAGER_ADMIN_TOKEN to enable the
  /execute + /stop + /reload gate, or set admin.trust_network=true
  if a reverse proxy / service mesh authenticates every request
  before it reaches this process
```

Callers present the token as `Authorization: Bearer <token>`. The
comparison is constant-time (`subtle::ConstantTimeEq`) and the
scheme name is case-insensitive per RFC 6750 §2.1. Read-only
endpoints (`/health`, `/jobs`, `/running`, `/actuator/*`) stay open
so operator dashboards keep working.

### Unknown fields are hard errors

Every config struct is validated with `serde(deny_unknown_fields)`.
A typo like `dslpath:` (missing underscore) or an accidental
`retryDelayy:` in a job file fails at load, not at first fire.
The error names the offending field:

```
error: invalid config file ./cronmanager.yaml: unknown field
  `dslpath`, expected one of `port`, `dsl_path`, `app_root_path`,
  `allowed_origins`, `shell_environment`, `limits`, `database`
```

### JVM-compat aliases

Copied a JVM `application.yml` field-by-field? The camelCase names
also bind:

| JVM name           | Rust name          |
|--------------------|--------------------|
| `configPath`       | `dsl_path`         |
| `appRootPath`      | `app_root_path`    |
| `allowedOrigins`   | `allowed_origins`  |
| `shellEnvironment` | `shell_environment`|

`allowed_origins` additionally accepts the JVM comma-separated
string form:

```yaml
allowed_origins: "localhost,192.168.10.1,127.0.0.1"
```

...deserialises to the same value as:

```yaml
allowed_origins:
  - localhost
  - 192.168.10.1
  - 127.0.0.1
```

### JVM Spring wrappers are rejected

If you paste a JVM `application.yml` verbatim, the top-level
`application:` / `spring:` / `management:` / `logging:` keys are
caught at boot with a diagnostic that names the wrapper and shows
the fix:

```
error: invalid config file ./cronmanager.yaml: top-level `spring:`
  is the JVM Spring wrapper. CronManager uses a flat config
  schema — unwrap the child fields to the top level (e.g.
  `application.configPath: X` becomes `dsl_path: X` at the top
  level; `spring.datasource.*` becomes `database.url` + env-var
  `CRONMANAGER_DB_PASSWORD`).
```

## Boot-time diagnostics

At startup CronManager emits one INFO line summarising every
field, plus WARNs for values likely to surprise an operator:

```
INFO config: port=8080 dsl_path=./DSL app_root_path=/app origins=1 shell_env_keys=2 history_db=true limits[req=1048576,resp=16777216,http_to=30s,shell_to=300s]
WARN config: allowed_origins contains "*" — the CORS layer matches exactly and does NOT treat "*" as a wildcard; list every allowed origin explicitly
WARN config: limits.shell_timeout_secs=0 disables the shell wall-clock cap — a runaway process will not be SIGKILLed
```

A quick boot-log read tells you whether the process is running on
defaults, whether history persistence is on, and whether any
config value is nudging you toward a footgun.

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
| `trigger` | all | Quartz cron expression (6 fields) OR one of the manual-mode literals (see below). |
| `type` | all | `http` or `exec`. Any other value fails at load. |
| `startDate` | all | Unix millis; job silently skips fires before this. |
| `endDate` | all | Unix millis; job silently skips fires after this. |
| `retryCount` | all | Number of retries after the first attempt. Default `0`. |
| `retryDelay` | all | Sleep between retries in milliseconds. Default `1000`. |
| `ignoreFailures` | all | If true, absorb all-attempts-failed. Default `false`. |
| `method` | http | One of `GET`, `POST`, `PUT`, `DELETE`, `PATCH`, `HEAD`, `OPTIONS`. Any other value fails at load. |
| `url` | http | Full URL. |
| `command` | exec | Whitespace-tokenised command (matches JVM `Runtime.exec(String)`). No shell interpretation. |
| `allowedEnvs` | exec | List of env var names to export from `shell_environment` into the child process. |

### Manual-mode literals

Any of the following makes a job manual-only (fires only via
`POST /execute/...`):

- YAML `false` (bool)
- YAML `off` (bare word — YAML 1.2 parses this as a string)
- Case-insensitive strings `"off"`, `"false"`, `"no"`, `"disabled"`

`trigger: true` is explicitly rejected — it's almost always a
typo for either a cron expression or one of the manual literals.

Group names are derived from the file's path relative to
`dsl_path`, with directory separators replaced by `-` and the
extension dropped. `DSL/samples/http/health-check.yaml` yields
group `samples-http-health-check`.

For a large bank of copy-paste job samples, see
[Samples cookbook](./samples.md).

## REST API surface

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/` | Sentinel string `CronManager started`. |
| `GET` | `/health` | Liveness probe. Returns `{"status":"ok"}`. |
| `GET` | `/actuator/health` | Alias for `/health` — kept so operators porting from the JVM version keep the same URL. |
| `GET` | `/actuator/info` | Minimal build info: `{"name":"cronmanager","version":"…"}`. |
| `GET` | `/jobs`, `/jobs/`, `/jobs/{group}` | List scheduled jobs. Trailing slash tolerated. |
| `GET` | `/running`, `/running/`, `/running/{group}` | List jobs currently executing. Trailing slash tolerated. |
| `POST` | `/execute/{group}/{job}` | Fire a job now. Optional query params for shell jobs are matched against `allowedEnvs`. Returns the running-jobs snapshot. |
| `POST` | `/stop/{group}/{job}` | Best-effort abort. Returns `{"stopped": bool, "running": [...]}`. |
| `POST` | `/reload/{group}` | Re-scan `dsl_path` and re-register jobs. Returns `{"reloaded": N}`. Group segment is currently accepted-but-ignored (full-tree reload). |

See [Failure modes](./failure-modes.md) for the status codes each
endpoint can return.

## Environment overrides

Three env vars are read directly by the process (never a config
field):

| Env var | Purpose |
|---|---|
| `CRONMANAGER_CONFIG` | Absolute path to `cronmanager.yaml`. Overrides the search order. |
| `CRONMANAGER_DB_PASSWORD` (or whatever `database.password_env` names) | The DB password. Absence fails startup when `database.url` is set. |
| `RUST_LOG` | `tracing_subscriber` filter — e.g. `info`, `cronmanager=debug`, `info,cronmanager=trace`. |

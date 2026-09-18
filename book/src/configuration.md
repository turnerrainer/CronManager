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
| `security.expose_jobs_publicly` | bool | `true` | When `false`, `GET /jobs`, `/jobs/`, `/jobs/{group}` require the admin bearer token (401 otherwise). Backward-compat default is `true` so an upgrade from `0.1.4-alpha` keeps operator dashboards working; a boot WARN fires when this is left `true` AND a token is configured. |
| `security.expose_running_publicly` | bool | `true` | Same posture as `expose_jobs_publicly` but for `/running*`. Time-attack signal — flip to `false` for internet-exposed deployments. |
| `security.per_group_token_envs` | map of `group → env var name` | `{}` | Optional scoped tokens. When populated, requests to `/execute/:group/:job`, `/stop/:group/:job`, `/reload/:group` are accepted with EITHER the master admin token OR the group-specific token (constant-time compare on both). Groups not in the map only accept the master. Values are env var names — the file never contains a raw token. |

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
scheme name is case-insensitive per RFC 6750 §2.1.

Every admin-gate reject emits a structured WARN with a PII-safe
client-IP hash (SHA-256, 4-byte prefix) and the matched route
pattern, so a burst attack pattern is derivable from logs without
exposing raw IPs:

```
WARN auth: rejected admin request
     client_ip_hash=a1b2c3d4 route=/execute/:group/:job
     reason="token does not match expected"
```

Read-only endpoints (`/health`, `/healthz`, `/actuator/*`) always
stay public regardless of any other setting. `/jobs*` and
`/running*` default to public but can be gated per
`security.expose_jobs_publicly` / `security.expose_running_publicly`.

`admin.trust_network=true` opts out of the boot-time refuse-to-
start on non-loopback binds without a token; it does NOT disable
the admin gate itself when a token IS set. A deployment behind a
mesh authenticating every request can safely flip
`trust_network=true` and STILL enforce the bearer token as
belt-and-braces.

### Per-group admin tokens

For deployments where multiple integration partners share access
to the scheduler, `security.per_group_token_envs` scopes credential
blast radius:

```yaml
security:
  per_group_token_envs:
    payroll:   PAYROLL_ADMIN_TOKEN
    logistics: LOGISTICS_ADMIN_TOKEN
```

Each group listed above accepts either the master `CRONMANAGER_ADMIN_TOKEN`
OR the group-specific token. Groups not in the map only accept the
master. Missing env vars emit a WARN at boot and the group falls
back to master-only (never silently 401s an ops caller who thinks
they have a group token).

### Environment-aware boot safety gates

Set `APP_ENV` (or `ENVIRONMENT` / `DEPLOY_ENV`) to `production`,
`staging`, or `test` for any deployment above local dev. In those
environments boot REFUSES when:

- **Weak or default credentials are detected** — admin token
  matches `changeit` / `password` / `01234` / `dev-` / `-test` /
  `test-admin-token` / etc; or is shorter than 32 chars (admin
  token) / 12 chars (DB password); or is empty.
- **Unsafe posture flags are set** — `admin.trust_network=true`,
  `security.block_private_networks=false`, or
  `security.allow_dangerous_env_overrides=true`.

Both refusals emit a multi-line diagnostic naming each failing
item with a redacted value (`01****ef`) and its remediation. In
`APP_ENV=dev` (or unset) the same conditions produce WARN lines
and boot continues — preserving the zero-config loopback UX.

Unknown values (`APP_ENV=prroduction`) fail-safe to `Production`.

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
| `command` | exec | Accepts EITHER a whitespace-tokenised string (JVM `Runtime.exec(String)` parity — no shell interpretation, no quoting) OR an explicit YAML list. See below. |
| `allowedEnvs` | exec | List of env var names to export from `shell_environment` into the child process. |

### `command:` — two forms

The string form matches JVM parity: whitespace-split, no shell.
Values containing spaces cannot be quoted; if you need shell
semantics, use the list form with an explicit `sh -c` prefix.

```yaml
# String form — one argv element per whitespace-separated token.
command: /bin/echo hello world
command: ./scripts/samples/backup.sh /var/lib/data
```

```yaml
# List form — each element is one argv entry, verbatim.
command: ['/bin/sh', '-c', 'sleep 30; echo done']
command:
  - /usr/bin/python3
  - -c
  - "import time; time.sleep(30); print('done')"
```

The string `"/bin/sh -c \"sleep 30\""` in string form tokenises to
`["/bin/sh", "-c", "\"sleep", "30\""]` and the child shell exits
with `Syntax error: Unterminated quoted string`. That's the
signature of "use the list form here".

Empty string / empty list is refused at load with `command field
is empty`. Both forms normalise to a single argv vector before the
shell executor sees them.

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

| Method | Path | Auth | Purpose |
|---|---|---|---|
| `GET` | `/` | none | Sentinel string `CronManager started`. |
| `GET` | `/health` | none | Liveness probe. Returns `{"status":"ok"}`. |
| `GET` | `/healthz` | none | K8s convention alias for `/health` (identical body). Registered so `livenessProbe: httpGet` defaults work without a token. |
| `GET` | `/actuator/health` | none | JVM URL alias for `/health` (identical body). |
| `GET` | `/actuator/info` | none | Minimal build info: `{"name":"cronmanager","version":"…"}`. |
| `GET` | `/jobs`, `/jobs/`, `/jobs/{group}` | conditional | Public by default; require admin bearer token when `security.expose_jobs_publicly=false`. |
| `GET` | `/running`, `/running/`, `/running/{group}` | conditional | Public by default; require admin bearer token when `security.expose_running_publicly=false`. |
| `POST` | `/execute/{group}/{job}` | admin gate | Fire a job now. Optional query params for shell jobs are matched against `allowedEnvs`. Returns the running-jobs snapshot. |
| `POST` | `/stop/{group}/{job}` | admin gate | Best-effort abort. Returns `{"stopped": bool, "running": [...]}`. |
| `POST` | `/reload/{group}` | admin gate | Re-scan `dsl_path` and re-register jobs. Returns `{"reloaded": N}`. Throttled per group via `security.reload_min_interval_secs`. |

### Every response carries

- **Five defense-in-depth security headers** — `Content-Security-Policy: default-src 'none'; frame-ancestors 'none'`, `Strict-Transport-Security: max-age=63072000; includeSubDomains; preload`, `X-Frame-Options: DENY`, `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`. Emitted regardless of route so a proxy-bypassing dev bind still ships defense-in-depth.
- **W3C Trace Context headers** — `traceparent: 00-<trace_id>-<span_id>-01` and `x-trace-id: <trace_id>`. Inbound `traceparent` is inherited when well-formed; otherwise a fresh 32-hex uuid is generated.

### Every completed request logs one line

`INFO http_request_completed method=X route=Y status=Z duration_us=... trace_id=...` — matched route pattern only (never raw URI), never headers, never bodies. See the "Access logging" section below.

See [Failure modes](./failure-modes.md) for the status codes each
endpoint can return.

## Environment overrides

Three env vars are read directly by the process (never a config
field):

| Env var | Purpose |
|---|---|
| `CRONMANAGER_CONFIG` | Absolute path to `cronmanager.yaml`. Overrides the search order. |
| `CRONMANAGER_ADMIN_TOKEN` (or whatever `admin.bearer_token_env` names) | Admin bearer token gating `/execute` + `/stop` + `/reload`. Empty/unset → gate short-circuits (only safe on loopback). |
| `CRONMANAGER_DB_PASSWORD` (or whatever `database.password_env` names) | The DB password. Absence fails startup when `database.url` is set. |
| Any env var named in `security.per_group_token_envs` | Optional per-group tokens (see above). Missing envs WARN at boot; groups fall back to master-only. |
| `APP_ENV` / `ENVIRONMENT` / `DEPLOY_ENV` | Deployment classifier — `production`, `staging`, `test`, `dev` (default). Non-`dev` values REFUSE to boot on weak credentials or unsafe posture flags. Unknown values fail-safe to `Production`. |
| `CRONMANAGER_OFFLINE` | When set to `true` / `1` / `yes` / `on` (case-insensitive), every job dispatch — HTTP and shell alike — is short-circuited. Each fire records one history row with `status=OFFLINE`; no network call, no child process. Boot emits a WARN so the operator sees the lever is engaged. Reset by unsetting the variable and restarting. |
| `RUST_LOG` | `tracing_subscriber` filter — e.g. `info`, `cronmanager=debug`, `info,cronmanager=trace`. |

## `cronmanager doctor` — offline config audit

Run `cronmanager doctor` (no server binding, no DB access) to
replay every check the boot path runs. It prints one severity-
prefixed line per finding and exits `1` on any `FATAL`:

```
cronmanager doctor — production environment
[INFO ] port: bind port 8080
[INFO ] dsl_path: DSL directory: /app/DSL/samples
[INFO ] admin.token: admin bearer token resolved from env
[FATAL] refuse_to_start: process would REFUSE to boot: non-loopback bind 0.0.0.0:8080 without an admin token and admin.trust_network=false
[WEAK ] security.expose_jobs_publicly: GET /jobs is anonymous even though an admin token is configured
summary: 1 fatal, 0 break, 1 weak, 3 info
```

Severity ladder:

- `FATAL` — refuse-to-start conditions in the target env
  (`env_safety` + `refuse_to_start_without_token`). Exit code 1.
- `BREAK` — the process WILL fail at runtime even if it boots
  (missing env var referenced by config, unreachable DSL directory).
- `WEAK` — insecure default that boot only WARNs about.
- `INFO` — descriptive facts (bind, dsl_path, history).

Wire into CI or container `HEALTHCHECK` for pre-deploy auditing.

## Graceful shutdown

The process installs SIGTERM (unix) and SIGINT (Ctrl+C) handlers.
On either signal `axum::serve` drains in-flight requests before
resolving, then the process exits cleanly. `docker stop` / k8s pod
eviction get the full grace window (default 10s in Docker,
`terminationGracePeriodSeconds` in k8s) instead of ripping sockets
mid-request. Look for `INFO shutdown: SIGTERM received, draining
in-flight requests` and `INFO shutdown complete` in the log stream.

## Access logging

Every completed HTTP request emits one INFO line (SOC 2 CC7.2 /
ISO 27001 A.12.4 access-log compliance):

```
INFO http_request_completed method=POST route=/execute/:group/:job
     status=200 duration_us=1234 trace_id=<32-hex>
```

- **Route pattern only** — never raw URI, so query strings and
  path traversal attempts stay out of logs.
- **Never logs headers or bodies** — no `Authorization`, no body
  content, no client-controlled data.
- **trace_id** is inherited from the inbound `traceparent` header
  when present, else generated. Used to correlate with Ruuter and
  downstream services in the Buerostack topology.

Auth failures on `/execute`, `/stop`, `/reload` additionally emit
a WARN with a PII-safe SHA-256 short-hash of the client IP and the
matched route pattern — enough to build a blocklist from log data
without exposing raw IPs.

## Log stream is plain-text under Docker / systemd

`tracing_subscriber::fmt()` only emits ANSI colour codes when
stderr is a TTY. Container / systemd logs are ANSI-free — SIEM
parsers (Splunk, Datadog, Loki) see clean bytes.

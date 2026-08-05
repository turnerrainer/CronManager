# Samples cookbook

Copy-paste ready snippets covering every feature. Drop each into a
`.yaml` file under `dsl_path` — filenames are free-form, group
names are derived from the file's path relative to `dsl_path`.

For the field reference behind every key, see
[Configuration](./configuration.md#yaml-job-file-shape).

---

## HTTP jobs

### Simplest possible HTTP job

```yaml
health_check:
  trigger: "0 */5 * * * ?"    # every 5 minutes
  type: http
  method: GET
  url: https://httpbin.org/status/200
```

### HTTP job with retry + ignoreFailures

```yaml
external_service:
  trigger: "0 */2 * * * ?"    # every 2 minutes
  type: http
  method: GET
  url: https://httpbin.org/status/503
  retryCount: 3               # 3 retries after the first attempt = 4 attempts total
  retryDelay: 2000            # 2 s between attempts
  ignoreFailures: true        # don't mark the schedule as failing
```

### POST with a payload URL

CronManager's HTTP jobs send requests but don't shape the body —
if you need a payload, use a target that echoes / accepts a static
request, or point at an intermediary that constructs it (Ruuter,
DataMapper).

```yaml
post_webhook:
  trigger: "0 0 * * * ?"      # top of every hour
  type: http
  method: POST
  url: https://httpbin.org/post
  retryCount: 3
  retryDelay: 2000
```

### PUT / DELETE / PATCH

Every method in `GET / POST / PUT / DELETE / PATCH / HEAD / OPTIONS`
is accepted:

```yaml
put_update:
  trigger: "0 30 */6 * * ?"   # every 6 hours at :30
  type: http
  method: PUT
  url: https://httpbin.org/put
  retryCount: 5
  retryDelay: 3000

delete_cleanup:
  trigger: "0 0 3 * * ?"      # daily at 03:00 UTC
  type: http
  method: DELETE
  url: https://httpbin.org/delete
```

### HTTP job with a start / end window

Only fires while `now` is within `[startDate, endDate]` (Unix
milliseconds). Outside the window, the fire is recorded as
`SKIPPED` in history.

```yaml
time_bounded:
  trigger: "0 0 */2 * * ?"    # every 2 hours
  type: http
  method: GET
  url: https://httpbin.org/get
  startDate: 1704067200000    # 2024-01-01T00:00:00Z
  endDate:   1735689600000    # 2025-01-01T00:00:00Z
```

### Manual-only HTTP job

```yaml
manual_api_call:
  trigger: off                # bare word 'off' — never fires automatically
  type: http
  method: GET
  url: https://httpbin.org/uuid
```

Manual-mode variants that all mean the same thing:

```yaml
also_manual_a:
  trigger: false              # bool
  type: http
  method: GET
  url: https://x

also_manual_b:
  trigger: "disabled"         # case-insensitive string
  type: http
  method: GET
  url: https://x
```

Fire the manual job via:

```bash
curl -X POST http://localhost:8080/execute/<group>/<name>
```

---

## Shell jobs

### Simplest possible shell job

```yaml
say_hello:
  trigger: "0 * * * * ?"      # every minute
  type: exec
  command: /bin/echo hello world
```

The command is whitespace-tokenised — no shell interpretation.
For pipes / redirects / substitution, invoke `/bin/sh -c "..."`
explicitly.

### Shell job with baseline environment

`shell_environment` in `cronmanager.yaml` provides the pool;
`allowedEnvs` in the job spec is the opt-in whitelist.

```yaml
# cronmanager.yaml
shell_environment:
  BACKUP_DIR: /var/backups/cronmanager
  RETENTION_DAYS: "7"
```

```yaml
# DSL/samples/shell/backup.yaml
daily_backup:
  trigger: "0 0 2 * * ?"      # daily at 02:00 UTC
  type: exec
  command: ./scripts/samples/backup.sh
  allowedEnvs:
    - BACKUP_DIR
    - RETENTION_DAYS
```

The script sees only `BACKUP_DIR` and `RETENTION_DAYS`. Any other
key in `shell_environment` is *not* exported.

### Shell job with query-param env overrides

`POST /execute/<group>/<job>?FOO=bar` treats query params as env
overrides — but *only* if the key is in `allowedEnvs`:

```yaml
# job
seed_data:
  trigger: off
  type: exec
  command: ./scripts/seed.sh
  allowedEnvs:
    - SEED_COUNT
```

```bash
# Fire it with an override
curl -X POST 'http://localhost:8080/execute/samples-shell-seed_data/seed_data?SEED_COUNT=1000'

# Query params not in allowedEnvs are silently dropped:
curl -X POST 'http://localhost:8080/execute/samples-shell-seed_data/seed_data?SEED_COUNT=1000&SECRET_KEY=abc'
# SEED_COUNT reaches the child; SECRET_KEY does not.
```

### Long-running shell job with timeout override

A job whose runtime routinely exceeds the default 5-minute wall
clock — configure a larger cap globally in `cronmanager.yaml`:

```yaml
# cronmanager.yaml
limits:
  shell_timeout_secs: 3600    # 1 hour
```

The wall-clock cap is process-wide, not per-job. Individual jobs
that need to run *longer* than the process cap have to split
themselves into smaller units, or the cap has to be raised.

### Manual-only shell job

Same pattern as HTTP:

```yaml
manual_script:
  trigger: off
  type: exec
  command: /usr/local/bin/rebuild-cache.sh
```

---

## Schedule pattern cookbook

Every cron here parses with the `cron` crate the same way it
does under Quartz. The trailing `?` in day-of-month or day-of-week
means "no specific value" and is required whenever the *other*
day field is a literal.

```yaml
every_minute:
  trigger: "0 * * * * ?"
  type: http
  method: GET
  url: https://httpbin.org/get

every_30_seconds:
  trigger: "*/30 * * * * ?"
  type: http
  method: GET
  url: https://httpbin.org/get

hourly_on_the_hour:
  trigger: "0 0 * * * ?"
  type: http
  method: GET
  url: https://httpbin.org/get

daily_at_0230:
  trigger: "0 30 2 * * ?"
  type: http
  method: GET
  url: https://httpbin.org/get

weekday_morning:
  trigger: "0 0 9 ? * MON-FRI"
  type: http
  method: GET
  url: https://httpbin.org/get

monday_only:
  trigger: "0 0 8 ? * MON"
  type: http
  method: GET
  url: https://httpbin.org/get

first_of_month:
  trigger: "0 0 0 1 * ?"
  type: http
  method: GET
  url: https://httpbin.org/get

sunday_maintenance:
  trigger: "0 0 3 ? * SUN"
  type: http
  method: GET
  url: https://httpbin.org/get

twice_daily:
  trigger: "0 0 6,18 * * ?"    # 06:00 and 18:00 UTC
  type: http
  method: GET
  url: https://httpbin.org/get
```

The scheduler runs in **UTC**. Adjust hour fields for your local
offset — a job that should fire at 09:00 Europe/Tallinn (UTC+2 in
winter, UTC+3 in summer) needs its `hour` cron field set to `7`
(winter) or `6` (summer).

---

## Config cookbook

### Minimal `cronmanager.yaml` (defaults)

Every field has a sensible default. This is the smallest working
file:

```yaml
dsl_path: /app/DSL
```

Omitting the file entirely also works — CronManager falls back to
built-in defaults and logs `using built-in defaults`.

### Enable execution history

```yaml
database:
  url: postgres://cronmanager@timescaledb:5432/cronmanager
```

Set the password out-of-band:

```bash
export CRONMANAGER_DB_PASSWORD='...'
```

Startup will fail-loud if `database.url` is set and the env var
is missing — no silent history-disabled state.

### Tighter resource caps

```yaml
limits:
  max_request_bytes:    65536      # 64 KiB inbound REST bodies
  max_response_bytes:  1048576     # 1 MiB captured HTTP responses
  request_timeout_secs:   10       # fast-fail slow upstreams
  shell_timeout_secs:     60       # kill any shell job > 1 min
```

### Loose resource caps (aggregators, long jobs)

```yaml
limits:
  max_response_bytes:  67108864    # 64 MiB
  request_timeout_secs:  120       # tolerate slow scrapers
  shell_timeout_secs:   3600       # backups can take up to 1 h
```

### CORS enabled (browser callers)

```yaml
allowed_origins:
  - https://dashboard.example.com
  - https://admin.example.com
```

Or, JVM-style:

```yaml
allowed_origins: "https://dashboard.example.com,https://admin.example.com"
```

`*` is **not** treated as a wildcard by the CORS layer — it's an
exact-match origin. A boot-time WARN fires if you set it, because
that's almost never what you meant.

### Manual-only deployment

For a CronManager used only as an operator-controlled runner
(no automatic schedules), every job carries `trigger: off` and
fires via `POST /execute/...`. Nothing in `cronmanager.yaml`
changes — the schedule is per-job.

---

## Docker Compose sample

```yaml
services:
  timescaledb:
    image: timescale/timescaledb:latest-pg16
    environment:
      POSTGRES_USER: cronmanager
      POSTGRES_PASSWORD: cronmanager_password
      POSTGRES_DB: cronmanager
    volumes:
      - timescale-data:/var/lib/postgresql/data

  cronmanager:
    image: turnerrainer/cronmanager:alpha
    ports: ["8080:8080"]
    environment:
      CRONMANAGER_DB_PASSWORD: cronmanager_password
      RUST_LOG: info,cronmanager=debug
    volumes:
      - ./cronmanager.yaml:/app/cronmanager.yaml:ro
      - ./DSL:/app/DSL:ro
      - ./scripts:/app/scripts:ro
    depends_on:
      - timescaledb
    restart: unless-stopped

volumes:
  timescale-data:
```

The container is non-root (uid 1000) — make sure `./DSL` and
`./scripts` are world-readable, or match ownership.

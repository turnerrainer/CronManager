# DESIGN — CronManager

Domain design for the Rust rewrite of the Buerokratt JVM
CronManager. Written 2026-07-29 by reading the JVM source at
[buerokratt/CronManager](https://github.com/buerokratt/CronManager)
(commit history through mid-2025) and the Buerostack fork's
Docker-first configuration.

Sibling to the authoritative
[`../../DEV-REQUIREMENTS.md`](../../DEV-REQUIREMENTS.md); this
file is the *what* and *why*, not the *how of the build*.

---

## 1. Executive summary

CronManager schedules and executes two kinds of jobs:

1. **HTTP requests** — GET/POST/PUT/DELETE/PATCH to a fixed URL
   on a cron schedule, with configurable retry and time-window.
2. **Shell commands** — one-off process launches on a cron
   schedule with a whitelist of environment variables and a
   wall-clock timeout.

Jobs are declared in YAML files under a configurable `dsl_path`
directory. Each file may hold one or many jobs. The directory
layout controls "group" naming — every job is addressed as
`{group}/{jobName}` in the REST API.

Execution history is optional; when a Postgres/TimescaleDB DSN is
configured, every attempt (including retries and skips) writes a
row into `job_execution_history`.

The service is single-instance. Running two replicas against the
same DSL directory will double-fire every job. Cross-replica
coordination is deliberately out of scope for v0.x.

---

## 2. Anatomy of the JVM ancestor

Read of `github.com/buerokratt/CronManager` @ mid-2025:

**Endpoints (all under `/`)**:

- `GET /` — sentinel `"CronManager started"`
- `GET /jobs`, `GET /jobs/{group}` — JSON per group of
  `{name, schedule, lastExecution, nextExecution, lastResult}`
- `GET /running`, `GET /running/{group}` — same shape, filtered
  to jobs currently executing
- `POST /execute/{group}/{job}` — trigger; body query params
  become env exports for shell jobs (filtered by the job's
  `allowedEnvs`)
- `POST /stop/{group}/{job}` — interrupt (best-effort)
- `POST /reload/{group}` — walk the DSL tree again
- `GET /actuator/health` — Spring Boot Actuator liveness

**YAML DSL** (one map per file, key = job name):

- Common: `trigger` (Quartz cron string OR the literal `off` /
  `false`), `startDate`, `endDate` (both Unix millis),
  `retryCount`, `retryDelay`, `ignoreFailures`
- `type: http` → adds `method`, `url`
- `type: exec` → adds `command`, `allowedEnvs`

**Scheduling**: Quartz `Scheduler`, one `JobDetail` per YAML entry,
`CronScheduleBuilder.cronSchedule(trigger)`. `trigger: off` (or
false) short-circuits to `addJob(durably)` with no trigger — the
job is only fireable via `POST /execute`.

**Time-window semantics** (baked into `YamlJob.execute`):
`context.getResult()` is set to `"<"` if `startDate` is in the
future, `">"` if `endDate` is in the past, `""` otherwise. HTTP
and Shell subclasses throw `JobExecutionException` if the result
is non-empty — effectively a skip.

**HTTP execution** (`HttpHelper.doRequestWithRetry`):
`RestClient` from Spring Web 6.x, attempt loop of `retryCount +
1`, `Thread.sleep(retryDelay)` between attempts, history recorded
per attempt via optional `JobExecutionHistoryService`.

**Shell execution** (`ShellExecuteJob.execProcess` +
`ShellExecutionHelper`): `Runtime.exec(command, envArray, /app)`.
env array is built from the config's `shell_environment` map,
filtered by the job's `allowedEnvs` list.

**Persistence** (Liquibase master changelog):
`job_execution_history` (TimescaleDB hypertable, primary key on
`(id, execution_time)`), retention policy 90 days, continuous
aggregates for hourly + daily stats.

**Config surface** (`ApplicationProperties`):
`configPath`, `allowedOrigins`, `shellEnvironment`,
`appRootPath`. Postgres pieces live under standard Spring
`spring.datasource.*`.

---

## 3. Known rough edges in the JVM version (not reimplemented)

Read of the source flagged several sharp edges. They are noted
so the Rust rewrite does not silently reproduce them:

1. **`RuntimeException`-wrapping everywhere.** Every controller
   handler catches `SchedulerException` and rethrows as
   `RuntimeException`, which surfaces to callers as a bare
   Spring `500` with the stack trace. Rust rewrite emits
   structured JSON errors (`{ "error": <code>, "message": … }`)
   with the right HTTP status per error class.

2. **`stopJob` filter targets the wrong job.** The JVM
   implementation finds "some currently-running job in the same
   group" and interrupts that one, not necessarily the requested
   `jobName`. Rust rewrite filters on both group AND name.

3. **`ShellExecuteJob.execProcess` deep-copies the parameter map
   naively.** `params.split(",")` mishandles env values that
   contain commas. Rust rewrite keeps env values as
   `Vec<(String, String)>` end-to-end.

4. **Trigger `false` is normalised at scheduling time only, not
   at parse time.** A typo like `trigger: no` silently becomes
   an unparseable cron expression and blows up at scheduling.
   Rust rewrite validates the trigger at load time and rejects
   the file with a `400`-worthy error message.

5. **`retryDelay` uses `Thread.sleep` on the Quartz worker
   thread**, which blocks the pool. Rust rewrite uses
   `tokio::time::sleep`, so retries don't starve the executor.

6. **No wall-clock cap on shell jobs.** A hanging script pins
   its thread forever. Rust rewrite kills processes exceeding
   `limits.shell_timeout_secs` and records the outcome as
   `TIMEOUT`.

7. **HTTP response bodies are never size-capped.** A pathological
   1 GiB response blows the heap. Rust rewrite caps via
   `limits.max_response_bytes` and truncates with a marker in
   history.

---

## 4. Rust-side design

### 4.1 Crate layout

```
src/
├── main.rs              # entry: load config, build state, spawn server + scheduler
├── lib.rs               # re-exports so integration tests can wire pieces
├── config.rs            # AppConfig + Limits + DatabaseCfg + load_or_default()
├── error.rs             # CronManagerError enum + IntoResponse mapping
├── dsl/
│   ├── mod.rs           # types: JobSpec, HttpJob, ShellJob, Trigger, RetryPolicy
│   └── loader.rs        # walk dsl_path, parse each file, validate cron
├── executor/
│   ├── mod.rs           # ExecutionOutcome enum
│   ├── http.rs          # reqwest client + retry loop + history recorder call
│   └── shell.rs         # tokio::process spawn + env allow-list + timeout
├── scheduler.rs         # Scheduler struct: register, trigger_now, describe, stop
├── history/
│   ├── mod.rs           # HistoryRecorder trait + NoopRecorder
│   └── postgres.rs      # sqlx-backed recorder (opt-in)
└── router.rs            # axum routes + handlers + AppState
```

### 4.2 Scheduling model

One Tokio task per scheduled job:

```
loop {
    let next = schedule.upcoming(chrono::Utc).next()?;
    tokio::time::sleep_until(next).await;
    if within_window(now, spec) {
        record_and_execute(spec).await;
    } else {
        record_skip(spec).await;
    }
}
```

Manual-only jobs (`trigger: off` / `false`) don't get this task.
They live in the same registry so `/execute` can find them, but
they never self-fire.

### 4.3 Retry loop

```
for attempt in 1..=spec.retry.count + 1 {
    let outcome = attempt_once(spec).await;
    record(outcome).await;
    if outcome.is_success() || attempt == spec.retry.count + 1 { break; }
    tokio::time::sleep(spec.retry.delay).await;
}
```

`ignoreFailures: true` catches the final `FAILED` and translates
it to `SKIPPED` at the history layer — same wire behaviour as
the JVM version.

### 4.4 Executor split

`HttpExecutor::run(&JobSpec) -> ExecutionOutcome` owns the
`reqwest::Client` (built once with `limits.request_timeout_secs`).
`ShellExecutor::run(&JobSpec, extra_env: &[(String,String)]) ->
ExecutionOutcome` owns the `shell_environment` map.

The scheduler dispatches on `spec.kind` (an enum of `Http` /
`Exec`).

### 4.5 History recorder

```
#[async_trait]
trait HistoryRecorder: Send + Sync {
    async fn record(&self, entry: HistoryEntry);
}
```

Two impls: `NoopRecorder` (used when `database.url` is unset;
just logs at DEBUG) and `PostgresRecorder` (opens a `sqlx::PgPool`
at startup, runs migrations, writes async).

### 4.6 REST API

Faithful to the JVM surface:

| Method | Path | JVM equivalent |
|---|---|---|
| `GET` | `/health` | `/actuator/health` |
| `GET` | `/` | `CronController.index()` |
| `GET` | `/jobs` and `/jobs/{group}` | `CronController.jobs()` |
| `GET` | `/running` and `/running/{group}` | `CronController.runningJobs()` |
| `POST` | `/execute/{group}/{job}` | `CronController.executeJob()` |
| `POST` | `/stop/{group}/{job}` | `CronController.stopJob()` |
| `POST` | `/reload/{group}` | `CronController.reloadJobs()` |

Structured error bodies per
[`../book/src/failure-modes.md`](../book/src/failure-modes.md).

### 4.7 Config surface

Full field reference in
[`../book/src/configuration.md`](../book/src/configuration.md).
Highlights that differ from the JVM `application.yml`:

- `database` block is opt-in; absent → history disabled.
- Passwords come from env vars only; no plain `password:` field
  is accepted at any level.
- `limits.shell_timeout_secs` is new (bug #6 above).
- `limits.max_response_bytes` is new (bug #7 above).

---

## 5. Non-goals (v0.1.x)

- Multi-replica coordination (leader election, distributed
  locks). Single-instance only.
- Job graphs / dependencies. Every job is independent.
- Persistent-queue semantics for missed fires
  (`misfire-instruction`-equivalent). Missed fires are dropped.
- Metrics endpoint. Deferred to task 004.
- Auto-generated OpenAPI. Deferred to task 003.

---

## 6. Cross-references

- Original JVM source: <https://github.com/buerokratt/CronManager>
- Buerostack fork used as configuration reference:
  <https://github.com/Buerostack/CronManager>
- Ruuter-on-Rust / XTR-on-Rust — sibling projects that follow
  the same `DEV-REQUIREMENTS.md` ruleset. When in doubt about a
  build/CI/docs decision, look at how they did it.

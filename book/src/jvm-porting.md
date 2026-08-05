# JVM porting quickref

For operators moving from
[buerokratt/CronManager](https://github.com/buerokratt/CronManager)
(Spring Boot + Quartz). One page, field-by-field. If you copy
your JVM `application.yml` verbatim, boot will fail with a
diagnostic that names the change to make.

## Config file

The Rust config schema is **flat** — no Spring `application:` /
`spring:` / `management:` / `logging:` wrapper. If those top-level
keys are present, boot refuses with a message that names the
wrapper and shows the fix.

| JVM field                                | Rust field                                      | Notes |
|------------------------------------------|-------------------------------------------------|-------|
| `application.configPath`                 | `dsl_path` (alias `configPath` also accepted)   | snake_case rename; alias binds either way |
| `application.appRootPath`                | `app_root_path` (alias `appRootPath`)           | same |
| `application.allowedOrigins` (String)    | `allowed_origins` (list **or** comma-separated string; alias `allowedOrigins`) | Both forms work |
| `application.shellEnvironment` (Map)     | `shell_environment` (alias `shellEnvironment`)  | same |
| `spring.datasource.url`                  | `database.url`                                  | Scheme `postgres://`, embed username in userinfo |
| `spring.datasource.username`             | *(embed in `database.url` userinfo)*            | e.g. `postgres://cronmanager@host/db` |
| `spring.datasource.password`             | env var `CRONMANAGER_DB_PASSWORD`               | **Never** in the file — env-only |
| `spring.jpa.*`, `spring.liquibase.*`     | *(no target equivalent)*                        | Rust runs `sqlx::migrate!` automatically |
| `management.endpoints.web.exposure.include` | *(no target equivalent)*                     | Actuator URLs are directly registered — see [Endpoints](#endpoints) |
| `logging.level.*`                        | env var `RUST_LOG`                              | e.g. `info,cronmanager=debug` |
| *(no JVM equivalent — Spring default)*   | `port: 8080`                                    | Now explicit; default matches Spring |
| *(no JVM equivalent)*                    | `limits.max_request_bytes` (1 MiB)              | Inbound REST body cap (`413` on overflow) |
| *(no JVM equivalent)*                    | `limits.max_response_bytes` (16 MiB)            | HTTP + shell output cap |
| *(no JVM equivalent — RestClient default)*| `limits.request_timeout_secs` (30)             | Raise if your upstreams routinely exceed 30 s |
| *(no JVM equivalent — no cap)*           | `limits.shell_timeout_secs` (300)               | Wall-clock SIGKILL cap for shell jobs |

## Env vars

| JVM                        | Rust                          |
|----------------------------|-------------------------------|
| `SPRING_PROFILES_ACTIVE`   | *(no equivalent — single YAML per env)* |
| `TZ`                       | *(scheduler is UTC — adjust cron hours; see [Timezone](#timezone))* |
| `SPRING_DATASOURCE_URL`    | Move into `database.url` |
| `SPRING_DATASOURCE_USERNAME` | Embed in `database.url` userinfo |
| `SPRING_DATASOURCE_PASSWORD` | `CRONMANAGER_DB_PASSWORD` |
| *(no equivalent)*          | `CRONMANAGER_CONFIG` — override config file path |
| *(no equivalent)*          | `RUST_LOG` — logging filter |

## DSL / job YAML

**Every JVM sample job file loads unchanged.** No field renames,
no default changes, no semantic drift. The JVM sample corpus
(`DSL/samples/**`) is mirrored under `compat/dsl/` and parsed by
CI on every build — that's how "unchanged" is enforced, not
promised.

Two hardening changes to be aware of, both **fail-loud**:

1. **Unknown DSL fields are hard errors.** A typo like
   `retryCoun: 3` — silently ignored on JVM — fails at load.
2. **Unknown HTTP methods are hard errors at load.** Only
   `GET/POST/PUT/DELETE/PATCH/HEAD/OPTIONS` are accepted. A typo
   (`GETT`) or unusual method (`TRACE`, `CONNECT`) fails
   immediately instead of at first fire. Add a method to the
   whitelist if you need it — one line of Rust.

Additional trigger normalisations are **additive** (no JVM script
breaks):

- `trigger: false` (bool) still works.
- Strings `off`, `false`, `no`, `disabled` all normalise to manual
  mode (case-insensitive).
- `trigger: true` is explicitly rejected — it would have been a
  parse-error on JVM too.

## Endpoints

| JVM URL                | Rust URL                                    |
|------------------------|---------------------------------------------|
| `GET /`                | `GET /` — same body: `CronManager started` |
| `GET /jobs`, `/jobs/`  | `GET /jobs`, `/jobs/` (both work)          |
| `GET /jobs/{group}`    | `GET /jobs/{group}` |
| `GET /running`, `/running/` | Both work |
| `POST /execute/.../...`| Same |
| `POST /stop/.../...`   | Same URL. Response shape: `{"stopped": bool, "running": [...]}` — the JVM's `running` list is preserved on the same field name, plus a new `stopped` flag. |
| `POST /reload/{group}` | Same URL. Response shape: `{"reloaded": N}` — full-tree reload; the group segment is currently accepted-but-ignored (diff-based reload is a backlog item). |
| `GET /actuator/health` | `GET /health` **and** `/actuator/health` — both return `{"status":"ok"}`. **No component tree** — alerts that grep for `components.db.status` will silently miss. |
| `GET /actuator/info`   | Same URL. Body: `{"name":"cronmanager","version":"…"}` — no `git`, no `build` tree yet. |

## Database

CronManager-on-Rust reads the exact same Liquibase schema. Point
the Rust binary at an existing JVM database and it works, **with
one column-type ALTER**:

```sql
ALTER TABLE job_execution_history
    ALTER COLUMN execution_time TYPE TIMESTAMPTZ
    USING execution_time AT TIME ZONE 'Europe/Tallinn';
```

Substitute `'Europe/Tallinn'` for your JVM `TZ`. The JVM stored
`TIMESTAMP` (no timezone, interpreted as system-local); Rust
uses `TIMESTAMPTZ` (UTC-normalised). Skipping the ALTER causes
historical rows to be read at the wrong wall-clock time.

The two schema columns that neither impl populates
(`retry_reason`, `stack_trace`) stay in place.

## Timezone

The Rust scheduler is UTC. A job that meant "09:00
Europe/Tallinn" needs its cron `hour` field manually adjusted.

| JVM cron (Tallinn, UTC+2 winter) | Meant             | Rust cron (UTC) |
|----------------------------------|-------------------|-----------------|
| `0 0 9 ? * MON-FRI`              | 09:00 weekdays    | `0 0 7 ? * MON-FRI` |
| `0 0 2 * * ?`                    | 02:00 daily       | `0 0 0 * * ?` |
| `0 */5 * * * ?`                  | every 5 min       | unchanged |

DST is not modelled — a job that must fire "at 02:00 Tallinn
local irrespective of DST" is not expressible in the current
scheduler. Per-schedule TZ context is a backlog item.

## Deploy diff

Docker Compose:

```diff
   cronmanager:
     image: turnerrainer/cronmanager:alpha
-    environment:
-      SPRING_PROFILES_ACTIVE: docker
-      TZ: Europe/Tallinn
-      SPRING_DATASOURCE_URL: jdbc:postgresql://timescaledb:5432/cronmanager
-      SPRING_DATASOURCE_USERNAME: cronmanager
-      SPRING_DATASOURCE_PASSWORD: cronmanager_password
+    environment:
+      CRONMANAGER_DB_PASSWORD: cronmanager_password
+      RUST_LOG: info,cronmanager=debug
     volumes:
       - ./cronmanager.yaml:/app/cronmanager.yaml:ro
       - ./DSL:/app/DSL:ro
       - ./scripts:/app/scripts:ro
```

Kubernetes:

```yaml
env:
  - name: CRONMANAGER_DB_PASSWORD
    valueFrom:
      secretKeyRef:
        name: cronmanager-db
        key: password
  - name: RUST_LOG
    value: "info,cronmanager=debug"
```

## Rollback

Reversible in-place: revert the ALTER above, restore the JVM
`application.yml`, redeploy the JVM binary. Existing rows stay
readable on either side (Postgres coerces).

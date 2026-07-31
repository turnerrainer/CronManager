# Getting started

Install, run, verify, and add your first job. Aim: under five
minutes from clone to first fire.

## Prerequisites

- Docker + Docker Compose (any recent version)
- OR Rust 1.88+ if building from source

## Run it

**With Docker (fastest):**

```bash
docker run -d --name cronmanager -p 8080:8080 \
  turnerrainer/cronmanager:alpha
```

**With Docker Compose (includes TimescaleDB for execution history):**

```bash
git clone -b dev https://github.com/turnerrainer/cronmanager.git
cd cronmanager
docker compose up -d
```

**From source:**

```bash
git clone -b dev https://github.com/turnerrainer/cronmanager.git
cd cronmanager
cargo run --release
```

## Verify it

```bash
curl http://localhost:8080/health
```

```json
{"status":"ok"}
```

List loaded jobs:

```bash
curl -s http://localhost:8080/jobs
```

The response is `{ "<group>": [ { "name": …, "schedule": …, "nextExecution": … }, … ] }`.
Groups are derived from the directory layout under `dsl_path` —
`DSL/samples/http/health-check.yaml` becomes group
`samples-http-health-check`.

## Add your first HTTP job

Create `DSL/mine/ping.yaml`:

```yaml
ping_google:
  trigger: "0 */5 * * * ?"
  type: http
  method: GET
  url: https://www.google.com/
  retryCount: 2
  retryDelay: 1000
  ignoreFailures: true
```

Restart the container (or send `POST /reload/samples-mine-ping` if
already running):

```bash
docker compose restart cronmanager
curl -s http://localhost:8080/jobs | grep ping_google
```

## Add your first shell job

Put your script under `scripts/mine/hello.sh`:

```bash
#!/usr/bin/env bash
echo "hello from $(hostname), backup dir is ${BACKUP_DIR:-unset}"
```

Register it in `DSL/mine/hello.yaml`:

```yaml
say_hello:
  trigger: "0 * * * * ?"
  type: exec
  command: ./scripts/mine/hello.sh
  allowedEnvs:
    - BACKUP_DIR
```

`allowedEnvs` is an opt-in whitelist. Only listed variables from
`shell_environment` in `cronmanager.yaml` are exported to the
process. See [Configuration](./configuration.md) for the full env
model.

## Trigger a job manually

Every job — even one with `trigger: off` — can be fired ad hoc:

```bash
curl -X POST http://localhost:8080/execute/samples-mine-hello/say_hello
```

## Read the execution history

When a Postgres URL is configured (see [Configuration](./configuration.md)),
every attempt is recorded. Query it directly:

```bash
docker exec -it cronmanager-timescaledb \
  psql -U cronmanager -d cronmanager \
  -c 'SELECT job_name, status, execution_time FROM job_execution_history ORDER BY execution_time DESC LIMIT 10;'
```

## Where to next

- [Configuration](./configuration.md) — every field of `cronmanager.yaml`
- [Failure modes](./failure-modes.md) — what each HTTP status means

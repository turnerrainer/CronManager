# CronManager

YAML-driven cron scheduler for HTTP requests and shell scripts.
Rust reimplementation of
[buerokratt/CronManager](https://github.com/buerokratt/CronManager)
(originally a Spring Boot + Quartz application built at the
Estonian Information System Authority).

Point CronManager at a folder of YAML job files. Each job becomes
a scheduled task — a Quartz-style cron expression fires either an
outbound HTTP call or a shell command. Manual triggers, per-job
retry, and time-window (start/end) constraints are first-class.
Execution history is persisted to TimescaleDB when configured.

**Version:** 0.1.0-rc.1 · **License:** Apache-2.0
· **Repo:** [Buerostack/CronManager-on-Rust](https://github.com/Buerostack/CronManager-on-Rust)
· **Images:** `docker.io/buerostack/cronmanager-on-rust:rc`, `ghcr.io/buerostack/cronmanager-on-rust:rc`

## One-command demo

```bash
docker run -d --name cronmanager -p 8080:8080 \
  buerostack/cronmanager-on-rust:rc
curl http://localhost:8080/health          # {"status":"ok"}
curl -s http://localhost:8080/jobs | head  # loaded jobs, JSON
```

The demo image bakes in a small set of sample HTTP + shell jobs
under `/app/DSL/samples/`. They tick immediately so you can watch
them fire in the logs:

```bash
docker logs -f cronmanager
```

## Read in order

1. [Getting started](./getting-started.md) — install, run, add your own job
2. [Configuration](./configuration.md) — `cronmanager.yaml` field reference
3. [Failure modes](./failure-modes.md) — every HTTP status CronManager emits

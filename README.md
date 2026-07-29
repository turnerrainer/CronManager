# CronManager

YAML-driven cron scheduler for HTTP + shell jobs. Rust
reimplementation of [buerokratt/CronManager](https://github.com/buerokratt/CronManager).

**Version:** 0.1.0-rc.1 · **License:** Apache-2.0
· **Docs:** [buerostack.github.io/CronManager-on-Rust](https://buerostack.github.io/CronManager-on-Rust/)
· **Images:** `docker.io/buerostack/cronmanager-on-rust:rc`, `ghcr.io/buerostack/cronmanager-on-rust:rc`

Point CronManager at a folder of YAML job files. Each job becomes
a cron-scheduled outbound HTTP call or shell command, with retry,
manual-trigger, and time-window support. Execution history writes
to TimescaleDB when configured.

## One-command demo

```bash
docker run -d --name cronmanager -p 8080:8080 \
  buerostack/cronmanager-on-rust:rc
curl http://localhost:8080/health          # {"status":"ok"}
curl -s http://localhost:8080/jobs | head  # loaded jobs, JSON
```

## Build from source

```bash
git clone -b dev https://github.com/Buerostack/CronManager-on-Rust.git
cd CronManager-on-Rust
docker compose up -d --build
```

## Documentation

- **Book** — [buerostack.github.io/CronManager-on-Rust](https://buerostack.github.io/CronManager-on-Rust/)
  (getting started, config, failure modes)
- **Design** — [`docs/DESIGN.md`](./docs/DESIGN.md) — what CronManager does and why
- **Standards** — [`STANDARDS.md`](./STANDARDS.md) — every generic
  build/docs/test/publish rule the project meets
- **Changelog** — [`CHANGELOG.md`](./CHANGELOG.md)
- **Original JVM CronManager** — <https://github.com/buerokratt/CronManager>

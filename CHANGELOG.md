# Changelog

All notable changes to CronManager-on-Rust will be documented in
this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0-rc.1] - 2026-07-29

Initial release candidate. Rust reimplementation of the Spring
Boot / Quartz [CronManager](https://github.com/buerokratt/CronManager)
scaffolded per the Buerostack `DEV-REQUIREMENTS.md` ruleset.

### Added

- **Scheduler** — Quartz-style cron expression firing per job,
  built on `cron::Schedule` + per-job Tokio tasks. Supports the
  `?` day-of-month / day-of-week wildcards used by the JVM
  ancestor.
- **HTTP job executor** — GET/POST/PUT/DELETE/PATCH via `reqwest`
  with configurable timeout, retry-with-delay, and
  `ignoreFailures` semantics faithful to the JVM implementation.
- **Shell job executor** — spawns via `tokio::process::Command`
  with an opt-in `allowedEnvs` whitelist and a `shell_timeout_secs`
  wall-clock cap that SIGKILLs runaway processes.
- **YAML DSL loader** — walks `dsl_path` recursively; group names
  are derived from the relative path (`DSL/samples/http/x.yaml` →
  `samples-http-x`), matching JVM behaviour. Manual-only jobs
  supported via `trigger: off` (or `trigger: false`).
- **Time-window jobs** — `startDate`/`endDate` (Unix millis)
  silently skip fires outside the window. History records the
  skip as `SKIPPED`.
- **REST API** — `/health`, `/`, `/jobs`, `/jobs/{group}`,
  `/running`, `/running/{group}`, `POST /execute/{group}/{job}`,
  `POST /stop/{group}/{job}`, `POST /reload/{group}`. Structured
  JSON error bodies on every failure path.
- **Body size caps + timeouts** — `limits.max_request_bytes`,
  `limits.max_response_bytes`, `limits.request_timeout_secs`,
  `limits.shell_timeout_secs` per DEV-REQUIREMENTS §5.3.
- **CORS** — `tower_http::cors::CorsLayer` gated on
  `allowed_origins`. Empty list disables CORS entirely.
- **Execution history** — optional TimescaleDB persistence via
  `sqlx`. Schema mirrors the JVM Liquibase changelog: hypertable
  on `execution_time`, retention policy (90 days), hourly + daily
  continuous aggregates.
- **Container image** — multi-stage `rust:1.88-slim` →
  `debian:bookworm-slim`, non-root uid 1000, tini PID 1,
  `HEALTHCHECK` hitting `/health`.
- **Hardened compose file** — `cap_drop: ALL`,
  `no-new-privileges:true`, `tmpfs: /tmp`, cpu/memory limits.
- **CI** — four workflows: `tests.yml` (fmt + clippy + build +
  test on amd64 + arm64, docs-build with linkcheck), `security.yml`
  (`cargo audit` + `cargo deny` daily), `publish.yml` (multi-arch
  build, Trivy scan, per-arch smoke test, cosign keyless signing),
  `docs.yml` (mdBook → GitHub Pages).
- **Docs** — mdBook with introduction, getting-started,
  configuration, failure-modes, changelog. Everything runnable
  as-shown.

### Structural

- Full Buerostack Rust project layout per
  [`../DEV-REQUIREMENTS.md`](https://github.com/Buerostack/Ruuter-on-Rust/blob/dev/DEV-REQUIREMENTS.md):
  `HANDOFF.md`, `SECURITY.md`, `STANDARDS.md`, `NOTICE`,
  `VERSION`, `deny.toml`, `.cargo/audit.toml`, `book/`, `tasks/`.

[Unreleased]: https://github.com/Buerostack/CronManager-on-Rust/compare/v0.1.0-rc.1...HEAD
[0.1.0-rc.1]: https://github.com/Buerostack/CronManager-on-Rust/releases/tag/v0.1.0-rc.1

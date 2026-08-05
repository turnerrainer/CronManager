# Changelog

All notable changes to CronManager will be documented in
this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0-alpha.3] - 2026-08-05

CI housekeeping. No product changes — same binary behaviour, same
image contents, same 90-test suite (0 failures).

### Changed

- Bumped all workflow actions to Node-24-native majors, silencing
  the "Node.js 20 is deprecated" annotation surfaced on the
  alpha.2 publish run:
  - `actions/checkout`              v4 → v5
  - `actions/cache`                 v4 → v6
  - `actions/upload-pages-artifact` v3 → v5
  - `actions/deploy-pages`          v4 → v5
  - `docker/build-push-action`      v6 → v7
  - `docker/login-action`           v3 → v4
  - `docker/setup-buildx-action`    v3 → v4
  - `docker/setup-qemu-action`      v3 → v4

## [0.1.0-alpha.2] - 2026-08-05

JVM-compatibility hardening pass. No breaking changes for existing
CronManager-on-Rust users. Every JVM sample DSL still loads
unchanged; the JVM `application.yml` shape now fails loudly with a
diagnostic that shows the fix.

### Added

- **JVM `application.yml` compat aliases** — the four JVM camelCase
  field names (`configPath`, `appRootPath`, `allowedOrigins`,
  `shellEnvironment`) bind to their Rust snake_case counterparts
  via `#[serde(alias)]`.
- **String-form `allowed_origins`** — the JVM comma-separated
  string form is accepted in addition to the YAML list form.
- **JVM Spring wrapper rejection** — pasting a JVM
  `application.yml` verbatim (top-level `application:` / `spring:` /
  `management:` / `logging:`) is caught at boot with a diagnostic
  that names the wrapper and shows the inline fix.
- **Actuator alias endpoints** — `GET /actuator/health` (aliased
  to `/health`) and `GET /actuator/info`.
- **Trailing-slash routes** — `GET /jobs/` and `GET /running/` now
  work in addition to the no-slash form, matching JVM Spring's
  auto-normalisation.
- **Inbound request body cap wired** — `limits.max_request_bytes`
  is now enforced via `tower_http::limit::RequestBodyLimitLayer`.
  Bodies over the cap return `413 request_too_large`. (The field
  was previously parsed but unused.)
- **Boot-time diagnostic pass** — one INFO summary of every config
  field at startup, plus WARNs for likely-footgun values
  (`allowed_origins` containing `"*"`, disabled timeouts).
- **Log-line grep parity with JVM** — per-DSL-file INFO on load,
  per-group INFO summary, per-job scheduler INFO, per-attempt
  DEBUG, per-retry WARN, success-after-retry INFO,
  all-attempts-failed ERROR, ignore-failures WARN.
- **mdBook: samples cookbook + JVM porting quickref** — two new
  book pages covering every feature and every JVM field rename.
- **JVM sample corpus in CI** — the upstream JVM `DSL/samples/**`
  fixtures mirrored verbatim under `compat/dsl/` and parsed on
  every build.

### Changed — silent-drop hardening

- `#[serde(deny_unknown_fields)]` on every config and DSL struct
  (`AppConfig`, `Limits`, `DatabaseCfg`, DSL job). A typo like
  `dslpath:` or `retryCoun: 3` is a hard load error, not a silent
  no-op.
- HTTP method whitelist enforced at load
  (`GET/POST/PUT/DELETE/PATCH/HEAD/OPTIONS` only).
- `trigger: true` explicitly rejected.

### Fixed

- `InvalidConfig` error variant replaces generic 500s when the
  config file structurally can't load. Returns
  `400 invalid_config`.

### Tests

- +7 tests in `tests/compat_parse.rs` (JVM sample corpus).
- +10 tests in `tests/deny_unknown.rs` (silent-drop regressions).
- +5 tests in `tests/router_integration.rs` (actuator aliases,
  trailing-slash routes, `413` on oversized body).
- +11 unit tests in `src/config.rs`.

Total suite: **90 tests, 0 failures.**

## [0.1.0-alpha.1] - 2026-07-31

Initial alpha. Rust reimplementation of the Spring Boot / Quartz
[CronManager](https://github.com/buerokratt/CronManager) scaffolded
per the Buerostack `DEV-REQUIREMENTS.md` ruleset.

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

[Unreleased]: https://github.com/turnerrainer/cronmanager/compare/v0.1.0-alpha.3...HEAD
[0.1.0-alpha.3]: https://github.com/turnerrainer/cronmanager/releases/tag/v0.1.0-alpha.3
[0.1.0-alpha.2]: https://github.com/turnerrainer/cronmanager/releases/tag/v0.1.0-alpha.2
[0.1.0-alpha.1]: https://github.com/turnerrainer/cronmanager/releases/tag/v0.1.0-alpha.1

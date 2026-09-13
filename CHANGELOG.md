# Changelog

All notable changes to CronManager will be documented in
this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0-alpha] - 2026-09-13

h2ck.me v2 audit round + Buerostack fleet-strongholds adoption.
Closes every break-test finding surfaced after `0.1.4-alpha`
merged (FN1, FN2, FN3, FN5, FN9, FN-LOG-1..4, F-CM-1, F-CM-4),
the F-CM-2 per-group-token feature request, the FN7/FN8 DSL
list-form request, and all four PR-review v2 backlog nits. Also
adopts four cross-service `FLEET-STRONGHOLDS.md` patterns (§1.6
W3C traceparent propagation, §5.1 default security response
headers, §11.1 credential-safety gate, §11.2 posture-safety
gate).

223 tests pass (was 156); `cargo audit` clean; `cargo deny check
all` clean; `cargo clippy --all-targets -- -D warnings` clean;
`mdbook build` + linkcheck clean. Minor version bump because
`JobKind::Exec` changes shape (`command: String` →
`argv: Vec<String>`) — any downstream consumer constructing a
`JobSpec` directly must update the fixture. Config / DSL / HTTP
surface stays backward-compatible: every new field defaults to
the pre-existing behaviour.

Ships on `dev` only. `main` remains at `0.1.4-alpha` until the
project reaches prod-ready state.

### Added

- **`/healthz` public alias** for `/health` — Kubernetes
  `livenessProbe` default binds cleanly without a token.
  (h2ck.me FN1 / F-CM-4.)
- **`security.expose_jobs_publicly` / `security.expose_running_publicly`**
  — optional gate on `/jobs*` and `/running*` recon endpoints.
  Default `true` for backward compat; boot WARN when left `true`
  with a token configured. (h2ck.me FN3 / F-CM-1.)
- **`security.per_group_token_envs: BTreeMap<String, String>`** —
  optional scoped tokens per group. When populated,
  `/execute/:g/:j`, `/stop/:g/:j`, `/reload/:g` accept EITHER the
  master admin token OR the group-specific token. Values are
  env-var names — no raw tokens in the config file. Missing envs
  WARN at boot and the group falls back to master-only.
  (h2ck.me F-CM-2.)
- **DSL `command:` accepts YAML list form** — explicit-argv
  passthrough for shell jobs that need `sh -c "…"` semantics.
  String form (JVM parity: whitespace-tokenised) remains
  supported. (h2ck.me FN7 + FN8.)
- **`env_safety` module** — reads `APP_ENV` / `ENVIRONMENT` /
  `DEPLOY_ENV` (first match wins, unknown values fail-safe to
  `Production`) and REFUSES to boot in non-`dev` when weak
  credentials or unsafe posture flags are present. Weak-pattern
  list covers `changeit`, `password`, `01234`, `dev-`, `-test`,
  `example`, `test-admin-token`, plus min-length checks (32 for
  tokens, 12 for passwords). In `dev` the same conditions
  produce WARN and boot continues. (FLEET-STRONGHOLDS §11.1 +
  §11.2.)
- **Default security response headers** — Content-Security-Policy,
  Strict-Transport-Security, X-Frame-Options DENY,
  X-Content-Type-Options nosniff, Referrer-Policy no-referrer on
  every response, including error paths. (FLEET §5.1.)
- **W3C Trace Context response headers** — `traceparent` +
  `x-trace-id` on every response. Inbound `traceparent` is
  inherited when well-formed (version=00, 32-hex trace-id
  ≠ all-zeros); fresh 32-hex uuid otherwise. (FLEET §1.6.)
- **Per-request access log** — one INFO line per completed
  request: `http_request_completed method=X route=Y status=Z
  duration_us=… trace_id=…`. Matched route pattern only (never
  raw URI), never headers/bodies, never client IP. SOC 2 CC7.2 /
  ISO 27001 A.12.4 compliance. (h2ck.me FN-LOG-3.)
- **Auth-fail WARN with client_ip_hash + route + reason** — every
  admin-gate reject emits a structured line with a PII-safe
  SHA-256 short-hash of the client IP, the matched route
  pattern, and a distinguishable reason (missing header vs
  wrong token). Enables blocklist derivation from logs without
  exposing raw IPs. (h2ck.me FN-LOG-2.)
- **`tower_http::TimeoutLayer`** wraps the whole request/body-
  read/handler/response-write against `limits.request_timeout_secs`.
  Kills slow-drip and slow-loris style requests that used to
  bypass the executor-level timeout. (h2ck.me FN5.)
- **`sha2` direct dep + `security::short_client_hash`** — PII-safe
  short-hash helper. `tower-http` gains the `timeout` feature.

### Changed

- **⚠️ `JobKind::Exec.command: String` → `argv: Vec<String>`** —
  shape change to the executor API. Loader normalises both DSL
  forms (string / list) to argv before the executor sees them.
  Any Rust fixture constructing a `JobSpec` directly must update
  the field. `JobKind::exec_command_display()` returns the argv
  space-joined for logs / display. (h2ck.me FN7 + FN8.)
- **DSL loader skips-and-warns on per-file errors** — one bad
  file no longer aborts the whole boot / `/reload`. Bad files
  log at ERROR (naming source path + reason); sibling good files
  register. If EVERY file fails, boot succeeds with 0 jobs plus
  a distinguishable WARN. Only `read_dir` failures remain fatal.
  (h2ck.me FN2.)
- **`PostgresRecorder` sanitises `response_body` BEFORE persist**
  — new `security::sanitize_for_persistence` runs first, then
  `truncate_response_body` applies the 64 KiB head+tail cap.
  Downstream DB-row viewers see escaped CR/LF/ANSI; raw bytes
  no longer land in the row. (h2ck.me PR-review v1 nit #3.)
- **`ReloadGate` memory bounded** — opportunistic stale eviction
  (entries older than `10 × min_interval_secs`) plus hard cap
  at 1024 distinct groups on the write path. Behavioural no-op
  for deployments with < 1024 groups. (h2ck.me PR-review v1
  nit #1.)
- **Log stream is plain-text under Docker / systemd** —
  `tracing_subscriber::fmt()` only emits ANSI colour codes when
  stderr is a TTY. SIEM parsers see clean bytes. (h2ck.me
  FN-LOG-1.)
- **`admin_gate` extracts group from URL path** and checks both
  master AND group-specific tokens in parallel via
  `subtle::ConstantTimeEq` so a caller can't time which token
  was consulted. (F-CM-2.)
- **`docker-compose.yml`**: `read_only: true` on the cronmanager
  service — rootfs is immutable; `/tmp` stays writable via
  `tmpfs` (64 MiB). Any shell job needing persistent scratch
  points at a bind-mounted volume added by the operator.
  (h2ck.me FN9.) Image tag bumped from the stale `0.1.0-alpha.1`
  to track the crate version.
- **`main.rs` uses `into_make_service_with_connect_info::<SocketAddr>`**
  so `admin_gate` receives the peer address for the auth-fail
  log line.

### Tests / hardening (in commits, not surfaced to end users)

- 14 new `env_safety::tests` unit tests (env classification,
  weak-value detection, enforce_* behaviour).
- 5 new `traceparent::tests` unit tests.
- 7 new integration tests for the F-CM-2 per-group-token accept /
  reject matrix.
- 5 new integration tests for FN3 recon-endpoint gating.
- 4 new loader tests for the two DSL command forms.
- 2 new shell-executor tests for the argv shape.
- Regression pin `trust_network_true_with_token_set_still_enforces_gate`
  (h2ck.me PR-review nit #4) locks in the invariant that
  `admin.trust_network=true` never silently disables the runtime
  token check.
- Regression pin `slow_body_upload_hits_request_deadline` drives
  a raw TCP socket to verify the FN5 fix.
- Regression pin `every_response_carries_security_headers` walks
  four routes and validates all five FLEET §5.1 headers.
- Regression pins `every_response_carries_traceparent_headers` +
  `traceparent_inherits_inbound_trace_id`.

### Documentation

- `CLAUDE.md`: new "Breaking / non-obvious changes in `0.2.0-alpha`"
  section with 15 numbered items covering every code / config /
  DSL surface change; expanded boot-log triage tables; two-form
  `command:` guide; Rust-fixture update pattern; hard rule that
  version bumps / tags / GitHub Releases require explicit
  maintainer approval regardless of invocation mode. Records
  that `main` is intentionally frozen at `0.1.4-alpha` until
  prod-ready.
- `book/src/configuration.md`: field reference gains the three
  new SecurityConfig knobs; new "Per-group admin tokens" and
  "Environment-aware boot safety gates" subsections; two-form
  `command:` documentation with worked "why the string form
  breaks for shell quoting" example; REST-API table gains an
  Auth column and the `/healthz` alias; new "Every response
  carries" / "Access logging" / "Log stream is plain-text under
  Docker / systemd" subsections.
- `book/src/failure-modes.md`: status table gains 401 / 403 / 408 /
  413 too_many_query_params / 429 too_many_requests; common
  causes gains skip-and-warn / env-safety / slow-body triage.

## [0.1.4-alpha] - 2026-09-06

Security hardening pass in response to the h2ck.me pre-publication
audit (`../h2ck.me/projects/CronManager-on-Rust/v1/AUDIT.md`). All
findings addressed and validated by h2ck.me
(`v1/PR-REVIEWS/3-89bce00.md` — ✅ approve). 156 tests pass
(was 90); `cargo audit` clean; `cargo deny check` clean;
`cargo clippy --all-targets -- -D warnings` clean; `mdbook build`
+ linkcheck clean. Also bundles the Snyk-flagged runtime base
image bump `debian:bookworm-slim` → `debian:13.6-slim` (PR #2).

Version jump `0.1.0-alpha.3` → `0.1.4-alpha` is intentional
(alignment with the fleet's shared version counter); no
intermediate alpha tags exist.

### Added

- **`admin` config block** — bearer-token gate on
  `POST /execute`, `POST /stop`, `POST /reload`. Token comes from
  the env var named by `admin.bearer_token_env`
  (default `CRONMANAGER_ADMIN_TOKEN`). Constant-time comparison
  via `subtle::ConstantTimeEq`. Case-insensitive scheme name per
  RFC 6750. Read-only endpoints (`/health`, `/jobs`, `/running`,
  `/actuator/*`) stay open. `admin.trust_network=true` disables
  the boot-time refusal to start on non-loopback binds without a
  token. (Finding C1.)
- **`security` config block** — every hardening cap is
  operator-tunable, and every cap emits a WARN at boot if
  loosened from the default. Fields: `block_private_networks`,
  `allow_dangerous_env_overrides`, `max_query_params`,
  `max_dsl_file_bytes`, `dsl_load_timeout_secs`, `max_retry_count`,
  `min_cron_interval_secs`, `stored_response_body_max_bytes`,
  `reload_min_interval_secs`.
- **Bounded query-string extractor** — `POST /execute/…` refuses
  requests with more than `security.max_query_params` pairs
  (default 64). Structured `413` naming the count + cap.
  (Finding C2.)
- **SSRF pre-flight** — HTTP job URLs are refused at load if the
  host is a literal private / loopback / link-local / ULA IP
  (IPv4, IPv6, and IPv4-mapped IPv6). At fire time, hostnames
  are DNS-resolved and refused if any returned address is
  non-routable. Reqwest redirects are DISABLED so a legit upstream
  can't 302 the executor into a metadata endpoint.
  (Finding H1.)
- **Log-injection sanitiser** — captured shell stderr / non-2xx
  HTTP response bodies are stripped of CR/LF/ANSI/control bytes
  before landing in error strings that get logged. Cap at 4 KiB
  per log stanza. DB rows retain a larger head+tail slice via
  `truncate_response_body`. (Finding H2.)
- **Dangerous-env blacklist** — `/execute` refuses env overrides
  for `PATH`, `LD_*`, `DYLD_*`, `PYTHONPATH`, `NODE_OPTIONS`,
  `RUBYOPT`, `PERL5OPT`, `JAVA_TOOL_OPTIONS`, and the loader-hook
  families. Case-insensitive. Response is `403 forbidden` naming
  the offending key. (Finding H3.)
- **`/reload` per-group throttle** — second reload within
  `security.reload_min_interval_secs` for the same group returns
  `429 too_many_requests` with `retry_after_secs`. (Finding H4.)
- **DSL file-size cap + load wall-clock timeout** — per-file YAML
  refused at load if bigger than `security.max_dsl_file_bytes`
  (default 1 MiB). Full walk wrapped in a `tokio::time::timeout`
  under `spawn_blocking`, default 15s. (Finding M1.)
- **retryCount cap** — DSL with `retryCount >
  security.max_retry_count` rejected at load. WARN above 5.
  (Finding M2.)
- **High-frequency cron WARN** — jobs whose next two fires are
  less than `security.min_cron_interval_secs` apart emit a WARN
  at load naming the job + interval. (Finding M3.)
- **Override audit log** — INFO log per `/execute` call lists the
  applied override KEYS (never values). (Finding M4.)
- **History row body truncation** — `response_body` capped at
  `security.stored_response_body_max_bytes` (default 64 KiB) per
  row via head+tail slice with an inline marker. New migration
  adds a compound `(job_group, job_name, execution_time DESC)`
  index. (Finding M5.)

### Fixed

- Reqwest client no longer follows redirects (was up to 10 by
  default). See the SSRF pre-flight addition.
- HTTP upstream error bodies were logged verbatim including
  attacker-controlled CRLF / ANSI. Now sanitised.

### Migration

Existing operators on loopback / private-network deployments need
no changes — the gate short-circuits when no token is configured
and every new default is safe. Public / internet-exposed
deployments now MUST either:

1. Set `CRONMANAGER_ADMIN_TOKEN` and pass
   `Authorization: Bearer <token>` on every state-changing call, or
2. Set `admin.trust_network=true` if a reverse proxy / service
   mesh authenticates every request before it reaches the process.

Otherwise the process refuses to start with an explanatory error.

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

[Unreleased]: https://github.com/turnerrainer/cronmanager/compare/v0.1.4-alpha...HEAD
[0.1.4-alpha]: https://github.com/turnerrainer/cronmanager/releases/tag/v0.1.4-alpha
[0.1.0-alpha.3]: https://github.com/turnerrainer/cronmanager/releases/tag/v0.1.0-alpha.3
[0.1.0-alpha.2]: https://github.com/turnerrainer/cronmanager/releases/tag/v0.1.0-alpha.2
[0.1.0-alpha.1]: https://github.com/turnerrainer/cronmanager/releases/tag/v0.1.0-alpha.1

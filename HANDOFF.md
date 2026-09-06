# HANDOFF

**Written**: 2026-08-05 · **Last refreshed**: 2026-09-07 for the
`v0.1.4-alpha` release.

**Current release**: `v0.1.4-alpha` — h2ck.me v1 audit fixes +
Snyk-flagged base-image bump (`debian:bookworm-slim` →
`debian:13.6-slim`, PR #2). Merged from
`security/h2ck-audit-v1-hardening` (PR #3, verdict ✅ pass).
Verification captured in `CHANGELOG.md`: 156 tests / 0 failures,
`cargo audit` clean, `cargo deny check` clean, `cargo clippy
--all-targets -- -D warnings` clean, `mdbook build` + linkcheck
clean. Book live at <https://turnerrainer.github.io/cronmanager/>.

**Previously verified green** (2026-08-05, `v0.1.0-alpha.3`):
cargo test 90/0/0 (61 unit + 29 integration; 3 postgres-conditional
skipped without DB); fmt + clippy `-D warnings` clean; mdbook +
linkcheck build clean; `v0.1.0-alpha.3` published via CI in 41m3s
with no deprecation annotations
(`turnerrainer/cronmanager:alpha` on Docker Hub + GHCR, digest
`sha256:6a761e9273bcbf205b7ae75917bd9336938cb865a4a63c4d499307698bbeeaeb`
— identical across both registries); full local endpoint sweep
passed: `/`, `/health`, `/actuator/health` alias, `/actuator/info`,
`/jobs` + `/jobs/` trailing-slash pair, `/running` + `/running/`
trailing-slash pair, `/jobs/{group}`, `POST /execute/…`,
`POST /stop/…` (returns `{"stopped":bool,"running":…}`),
`POST /reload/…` (returns `{"reloaded":N}`, N=11 in demo image),
`413` on body over `limits.max_request_bytes`,
`404 job_not_found` structured JSON on unknown job.
Boot-diagnostic INFO summary + per-file DSL load + per-group
summary + per-job scheduler INFO + retry-parity ERROR log line all
verified live in container logs.

**Branch**: `dev` — tagged `v0.1.4-alpha` and pushed.

**LLM / new-dev orientation**: [`CLAUDE.md`](./CLAUDE.md) at the
repo root carries the breaking-changes summary, the
problematic-config triage table, the fix cookbook, and the
best-practice config baselines. Start there if you're landing
cold.

## What this repo IS today

Working Rust reimplementation of the JVM CronManager. YAML DSL
identical to the JVM version, so existing job files drop straight
in. JVM `application.yml` camelCase field names accepted as
`serde(alias)`; JVM Spring wrappers are rejected at boot with a
diagnostic that shows the fix.

- `POST /execute/{group}/{job}` — manual trigger of any scheduled
  or manual-only job (bearer-token gated as of `v0.1.4-alpha`)
- `GET /jobs`, `/running` — introspection
- `POST /stop/{group}/{job}`, `POST /reload/{group}` — control
  (bearer-token gated; `/reload` also per-group rate-limited)
- HTTP + shell executors with retry, `ignoreFailures`, time-window
  gating (`startDate`/`endDate`), SSRF pre-flight, and shell
  dangerous-env blacklist
- Optional TimescaleDB history (hypertable + retention +
  continuous aggregates), same schema as the JVM version's
  Liquibase changelog

## Breaking changes since `v0.1.0-alpha.3`

Full detail in [`CHANGELOG.md`](./CHANGELOG.md) under
`[0.1.4-alpha]`. LLM-friendly summary + fix cookbook in
[`CLAUDE.md`](./CLAUDE.md). Headline for anyone upgrading:

- **Public-facing bind now refuses to start without an admin
  token** (or `admin.trust_network=true` behind an authenticating
  proxy). Loopback deployments still boot with zero config.
- HTTP jobs may no longer target private / loopback / link-local /
  ULA hosts by default (`security.block_private_networks=true`).
- Shell `/execute` refuses env overrides for `PATH`, `LD_*`,
  `DYLD_*`, `PYTHONPATH`, `NODE_OPTIONS`, `RUBYOPT`, `PERL5OPT`,
  `JAVA_TOOL_OPTIONS`, and loader-hook families → `403`.
- New caps: `security.max_query_params` (413),
  `security.max_dsl_file_bytes` (load-time refusal),
  `security.max_retry_count` (load-time refusal),
  `security.reload_min_interval_secs` (`/reload` 429).
- History rows truncate `response_body` at
  `security.stored_response_body_max_bytes` (default 64 KiB).

## Verification set (all should exit 0)

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo build --release --locked --bin cronmanager
cargo test --no-fail-fast
cargo audit --deny warnings
cargo deny check all
( cd book && mdbook build )
docker build -t cronmanager:0.1.4-alpha .
```

Live smoke (once you have compose up):

```bash
docker compose up -d
curl -fsS http://localhost:9010/health
curl -s http://localhost:9010/jobs | head
```

## Open backlog

| Task | Location | Notes |
|---|---|---|
| 001 | `tasks/done/001-domain-deep-dive.md` | ✅ Landed — this scaffold |
| 002 | `tasks/backlog/002-postgres-integration-test-suite.md` | Add real-DB integration tests via Testcontainers-rs |
| 003 | `tasks/backlog/003-openapi-endpoint.md` | Auto-generated OpenAPI spec at `GET /api` |
| 004 | `tasks/backlog/004-metrics-endpoint.md` | Prometheus `/metrics` (behind config flag) |
| 005 | `tasks/backlog/005-graceful-reload.md` | Diff-based reload — currently `/reload` re-loads everything |
| 006 | *(unfiled)* | Enrich `/actuator/info` with git-SHA + build-time via `vergen` |
| 007 | *(unfiled)* | Per-schedule TZ context — scheduler currently evaluates in UTC only |

## Where to look for more detail

| Topic | File |
|---|---|
| LLM / new-dev orientation (breaking changes + fix cookbook + best-practice configs) | [`./CLAUDE.md`](./CLAUDE.md) |
| Cross-project ruleset (authoritative) | [`../DEV-REQUIREMENTS.md`](../DEV-REQUIREMENTS.md) |
| Domain design (this project) | [`./docs/DESIGN.md`](./docs/DESIGN.md) |
| Project-specific standards addendum | [`./STANDARDS.md`](./STANDARDS.md) |
| Public docs | https://turnerrainer.github.io/cronmanager/ |
| Full change history | [`./CHANGELOG.md`](./CHANGELOG.md) |
| Private security disclosure | [`./SECURITY.md`](./SECURITY.md) |
| CI workflows | [`.github/workflows/`](./.github/workflows/) |

---

## h2ck.me security-audit pipeline

**Added**: 2026-09-06. Describes the ongoing pre-publication security audit + fix + review flow with the `h2ckme` private GitHub org. If you land in this repo cold and see an open `security/h2ck-audit-*` PR, start here.

### What it is

h2ck.me runs a versioned audit → fix → validate cycle against every Bürostack-fleet service before it goes public. Each round is a `vN/` folder in the corresponding private repo under [`github.com/h2ckme`](https://github.com/h2ckme):

- `vN/AUDIT.md` — findings by severity, file:line pointers, attack scenarios.
- `vN/FIX-KIT.md` — runnable attack sandbox, diff-shaped fix code, per-finding acceptance criteria.
- `vN/PR-REVIEWS/<pr-number>-<head-sha7>.md` — one per PR reviewed (append-only across force-pushes).

**Fleet-wide index** — [`h2ckme/security-fleet` → `REVIEW-INDEX.md`](https://github.com/h2ckme/security-fleet/blob/main/REVIEW-INDEX.md).

### Where feedback lives (hybrid pipeline as of 2026-09-06)

1. **The open v1 audit PR carries a comment** starting with `## h2ck.me v1 review`. It includes: verdict (✅ / ⚠️ / ❌), one-paragraph summary, link to the full write-up in h2ckme.
2. **Full per-PR write-up** at [`h2ckme/CronManager-on-Rust/v1/PR-REVIEWS/`](https://github.com/h2ckme/CronManager-on-Rust/tree/main/v1/PR-REVIEWS).
3. **Audit + fix-kit context**: [`h2ckme/CronManager-on-Rust/v1/AUDIT.md`](https://github.com/h2ckme/CronManager-on-Rust/blob/main/v1/AUDIT.md) + [`v1/FIX-KIT.md`](https://github.com/h2ckme/CronManager-on-Rust/blob/main/v1/FIX-KIT.md).

**h2ckme access**: private org; your GitHub account has read via org membership. `git clone git@github.com:h2ckme/CronManager-on-Rust.git`.

### Open v1 PR on this repo

| PR | Branch | Findings | h2ck.me verdict |
|---|---|---|---|
| [#3](https://github.com/turnerrainer/CronManager/pull/3) | `security/h2ck-audit-v1-hardening` | C1 bearer gate, C2 query-param DoS, H1 SSRF + no-redirect, H2 log-injection sanitiser, H3 dangerous-env blacklist, H4 reload throttle, M1-M5 hardening | ✅ pass (11 findings, 156 tests / 0 failures, `cargo audit` + `cargo deny` + `clippy -D warnings` + `mdbook build` all clean) |

### Standout in the fix

The PR lands **`src/security.rs`** — a new cross-cutting shared primitives module (`is_dangerous_env`, `is_private_or_local`, `sanitize_for_log`, `truncate_response_body`). h2ck.me flagged this as the seed of a future workspace crate (`buerostack-security`) that would deduplicate the same primitives currently drifting across Ruuter, FileFerry, XTR, and TIM. See the review file for extraction notes.

### Next action for a maintainer landing here

1. **Open [PR #3](https://github.com/turnerrainer/CronManager/pull/3)** and read the `## h2ck.me v1 review` comment.
2. Follow the link to the full write-up for the acceptance-marker table + break-the-fix probe results.
3. **Merge** on your release cadence (verdict is ✅ pass; no blockers). The two unchecked test-plan boxes (container smoke, live sweep) are nice-to-haves — automated regressions already cover the wire-level behaviour.
4. Bump `Cargo.toml` + `CHANGELOG.md`, tag, push image.
5. **Wait ~2 weeks**, then h2ck.me opens `v2/` as an adversarial re-audit of the merged branch.

### v2 backlog (from the review)

Four items filed for the next iteration: `ReloadGate` LRU/time-based eviction (unbounded map growth under dynamic group names); `bind_is_loopback` completeness sweep (127.0.0.0/8, `localhost` hostname, `[::1]`, `::ffff:127.0.0.1`); H2 sanitiser applied at DB persist (not just log render); `src/security.rs` extraction to workspace crate.

### h2ck.me does NOT touch this repo

Explicit boundary: h2ck.me writes only to `h2ckme/*` (private org) + PR comment threads. It never pushes code, opens PRs, or edits files in `turnerrainer/*`. All fixes come from you or a fixer of your choice.

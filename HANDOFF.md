# HANDOFF

**Written**: 2026-07-31
**Last verified green**: 2026-07-31 — cargo test 57/0/0 (39 unit
+ 15 integration + 3 postgres-conditional skipped without DB);
fmt + clippy `-D warnings` clean; cargo audit clean (1
documented ignore, `RUSTSEC-2023-0071` in transitive `rsa` via
`sqlx-postgres`, no upstream fix, mirrored to `deny.toml` with
review date 2027-01-31); cargo deny check all clean; mdbook +
linkcheck build clean; docker image `cronmanager:0.1.0-alpha.1`
builds locally.
**Branch**: `dev` — tagged `v0.1.0-alpha.1` locally. Publish flow
(GitHub + Docker Hub + GHCR) runs on tag push.

## What this repo IS today

Working Rust reimplementation of the JVM CronManager. YAML DSL
identical to the JVM version, so existing job files drop straight
in.

- `POST /execute/{group}/{job}` — manual trigger of any scheduled
  or manual-only job
- `GET /jobs`, `/running` — introspection
- `POST /stop/{group}/{job}`, `POST /reload/{group}` — control
- HTTP + shell executors with retry, `ignoreFailures`, time-window
  gating (`startDate`/`endDate`)
- Optional TimescaleDB history (hypertable + retention +
  continuous aggregates), same schema as the JVM version's
  Liquibase changelog

## Verification set (all should exit 0)

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo build --release --locked --bin cronmanager
cargo test --no-fail-fast
cargo audit --deny warnings
( cd book && mdbook build )
docker build -t cronmanager:0.1.0-alpha.1 .
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

## Where to look for more detail

| Topic | File |
|---|---|
| Cross-project ruleset (authoritative) | [`../DEV-REQUIREMENTS.md`](../DEV-REQUIREMENTS.md) |
| Domain design (this project) | [`./docs/DESIGN.md`](./docs/DESIGN.md) |
| Project-specific standards addendum | [`./STANDARDS.md`](./STANDARDS.md) |
| Public docs | https://turnerrainer.github.io/cronmanager/ |
| Full change history | [`./CHANGELOG.md`](./CHANGELOG.md) |
| Private security disclosure | [`./SECURITY.md`](./SECURITY.md) |
| CI workflows | [`.github/workflows/`](./.github/workflows/) |

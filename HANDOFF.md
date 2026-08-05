# HANDOFF

**Written**: 2026-08-05
**Last verified green**: 2026-08-05 — cargo test 90/0/0 (61 unit
+ 29 integration; 3 postgres-conditional skipped without DB); fmt
+ clippy `-D warnings` clean; mdbook + linkcheck build clean;
`v0.1.0-alpha.2` published via CI in 41m44s
(`turnerrainer/cronmanager:alpha` on Docker Hub + GHCR, digest
`sha256:c9e26a4cc909e8bdf8ec0a1b021d533ba0d5d21fe34ef7b7b5a7d4158826ab69`
— identical across both registries); docker pull + `/health` +
`/actuator/health` alias + `/actuator/info` smoke passed; boot
diagnostic pass verified in container logs.
**Branch**: `dev` — tagged `v0.1.0-alpha.2` and pushed. Book live
at <https://turnerrainer.github.io/cronmanager/>.

## What this repo IS today

Working Rust reimplementation of the JVM CronManager. YAML DSL
identical to the JVM version, so existing job files drop straight
in. JVM `application.yml` camelCase field names accepted as
`serde(alias)`; JVM Spring wrappers are rejected at boot with a
diagnostic that shows the fix.

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
| 006 | *(unfiled)* | Enrich `/actuator/info` with git-SHA + build-time via `vergen` |
| 007 | *(unfiled)* | Per-schedule TZ context — scheduler currently evaluates in UTC only |

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

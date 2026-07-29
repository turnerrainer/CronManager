# Standards for CronManager-on-Rust

This file is a **thin extension** of the authoritative Buerostack
ruleset at
[`../DEV-REQUIREMENTS.md`](../DEV-REQUIREMENTS.md). If a rule
below conflicts with `DEV-REQUIREMENTS.md`, the ruleset wins —
this file only carries product-specific extras.

## 0. Product identity

| Variable | Value |
|---|---|
| Product name | `CronManager-on-Rust` |
| Cargo crate name | `cronmanager-on-rust` |
| Binary name | `cronmanager-on-rust` |
| GitHub repo | `github.com/Buerostack/CronManager-on-Rust` |
| Docker Hub image | `buerostack/cronmanager-on-rust` |
| GHCR image | `ghcr.io/buerostack/cronmanager-on-rust` |
| License | Apache-2.0 |
| Book title | `CronManager-on-Rust` |
| First stable target | `v1.0.0` on `main` |
| Author | Rainer Türner |
| Namespace on Buerostack | `Buerostack/CronManager-on-Rust` |

## 1. Everything else

Follow [`../DEV-REQUIREMENTS.md`](../DEV-REQUIREMENTS.md)
front-to-back. Every section applies as written:

- §1 Repository layout — matches; see the current tree.
- §2 Rust code standards — MSRV 1.88, Apache-2.0, `cargo fmt`,
  `cargo clippy -D warnings`, `thiserror` for library errors,
  `anyhow` at `main.rs` only, `tracing` for logging.
- §3 Testing — inline unit tests + `tests/` integration binaries.
  Postgres tests require a real DB (Testcontainers or a CI
  service — no mocking).
- §4 Documentation — 5 required mdBook chapters, zero
  duplication, every snippet runnable.
- §5 Security — `cargo audit`, `cargo deny`, Trivy, cosign,
  reproducible layer timestamps. Password-via-env-var only.
  Body caps + timeouts on every HTTP surface.
- §6 CI/CD — four workflows: `tests.yml`, `security.yml`,
  `publish.yml`, `docs.yml`. Matrix on amd64 + arm64.
- §7 Container — multi-stage `rust:1.88-slim` →
  `debian:bookworm-slim`, non-root uid 1000, tini PID 1.
- §8 Release — version bumped atomically across `Cargo.toml`,
  `Cargo.lock`, `VERSION`, `docker-compose.yml`, `README.md`,
  `book/src/introduction.md`, `HANDOFF.md`, `CHANGELOG.md`.
- §9 Publishing — Docker Hub + GHCR, tag drives everything.
- §10 Task tracking — `tasks/backlog/NNN-<slug>.md` →
  `tasks/done/`.
- §11 Git — `dev` for active work, `main` for stable.

## 2. CronManager-specific notes

**Domain of "what CronManager does" lives in
[`docs/DESIGN.md`](./docs/DESIGN.md), not here.**

- The scheduler is single-instance. Running two containers
  against the same DSL directory will double-fire every job.
  Cross-replica coordination is out of scope for v0.x.
- Shell jobs run in the container's own filesystem namespace —
  bind-mount only the script directories you trust, and
  configure `allowedEnvs` narrowly.
- History persistence assumes TimescaleDB (via the
  `timescale/timescaledb:2.17.2-pg16` image or equivalent). A
  plain Postgres works but retention + continuous aggregates
  become no-ops.

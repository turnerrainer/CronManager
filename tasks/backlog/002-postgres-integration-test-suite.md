# 002 — Real-DB integration tests via Testcontainers-rs

## Filed
2026-07-29 — surfaced during the v0.1.0-rc.1 scaffold. The current
integration tests exercise the HTTP surface with `NoopRecorder`;
history-layer semantics are only covered when a `CRONMANAGER_TEST_DATABASE_URL`
env var is exported (CI does this via a service container).

## Severity
Medium. The persistence layer works — the scaffold tests confirm
the schema migrates and rows insert — but the TimescaleDB-specific
behaviour (retention policy, continuous aggregate refresh) is not
exercised in-process.

## Motivation
DEV-REQUIREMENTS §3 forbids mocking of databases. `sqlx::PgPool`
tests currently rely on a shared CI service container; they don't
run under a bare `cargo test` without setting env. That's a
friction point for contributors on their own machines.

## Fix / Design
Add `testcontainers-rs` as a dev-dep, spin up
`timescale/timescaledb:2.17.2-pg16` per test module (once via a
`OnceCell`), run migrations against it, expose a `pg_pool()`
fixture. Every history test uses this fixture, no env var
required.

Test coverage additions:
- Retention policy actually drops old rows (insert with backdated
  `execution_time`, advance TimescaleDB's internal clock via
  `SET timescaledb.time_travel = …`, trigger retention job,
  assert row count).
- Continuous aggregate refresh materialises new rows on demand.
- Concurrent inserts don't deadlock the hypertable.

## Acceptance
- [ ] `cargo test` (no env vars) exercises the full history
      layer against a real TimescaleDB.
- [ ] Test count reported in HANDOFF grows by ≥5 tests.
- [ ] CI `tests.yml` service container becomes optional (kept
      for speed; Testcontainers is the correctness gate).

## Estimated effort
1 day.

## Dependencies
- Docker daemon available on the test host (already required
  by other DEV-REQUIREMENTS §3 checks).

## Non-scope
- Replacing the CI service container. Testcontainers is slower
  than a pre-provisioned service, so CI keeps both paths.

## Risks
- Testcontainers on macOS/Windows without Docker Desktop is a
  friction point. Mitigation: document the workaround (colima /
  podman-machine) in the CONTRIBUTING section of README when
  we file task 006.

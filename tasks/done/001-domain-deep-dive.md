# 001 — Deep-dive on the JVM CronManager to define domain surface

## Filed
2026-07-29 — first task on the CronManager-on-Rust roadmap.

## Landed
2026-07-29 — [`docs/DESIGN.md`](../../docs/DESIGN.md) shipped.
Direct read of `github.com/buerokratt/CronManager` at its then-current
`main` and the Buerostack fork's Docker-first configuration.
Sections:

1. Executive summary
2. Anatomy of the JVM ancestor (endpoints, DSL, scheduler,
   executors, persistence, config)
3. Known rough edges in the JVM version (not reimplemented)
4. Rust-side design (crate layout, scheduling, retry, executors,
   history, REST, config)
5. Non-goals for v0.1.x
6. Cross-references

Cross-linked from:
- `HANDOFF.md` — "read `docs/DESIGN.md`"
- `README.md` — Documentation list
- `book/src/introduction.md` — first paragraph

## Notes for the next task

Task 002 covers real-DB integration tests via Testcontainers-rs
so the history layer exercises actual TimescaleDB semantics
(retention policy behaviour, continuous aggregate refresh).

## Severity
High — nothing else can move without a clear picture of the
domain surface being reproduced.

## Motivation
Every subsequent decision (crate layout, dependency choices,
error taxonomy, REST paths) hinges on knowing exactly what the
JVM CronManager exposes today. Without this pass, the Rust
rewrite risks either omitting features silently or inventing
new ones that don't match operator expectations.

## Fix / Design
Read every Java file + Liquibase changelog + application.yml +
DSL samples. Distil into `docs/DESIGN.md` §§1–6. Enumerate the
JVM rough edges we deliberately DON'T reproduce.

## Acceptance
- [x] `docs/DESIGN.md` covers all six sections above.
- [x] Every JVM endpoint has a documented Rust equivalent.
- [x] Every JVM YAML DSL field has a documented Rust
      equivalent.
- [x] Every JVM rough edge is either fixed in the Rust design
      or explicitly deferred.
- [x] TimescaleDB schema fidelity (hypertable, retention,
      continuous aggregates) preserved in `migrations/`.

## Estimated effort
0.5 day.

## Dependencies
None.

## Non-scope
- Implementing the Rust code itself (task 002+).
- Documenting operator workflows beyond the DSL surface (that
  is the book's job).

## Risks
- Missing an undocumented behaviour of Quartz's cron parser.
  Mitigation: the Rust `cron` crate is a superset of Quartz for
  the expression shapes present in the JVM sample DSLs; any
  behavioural difference will show up in the integration test
  suite (task 002).

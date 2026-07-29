# 005 — Diff-based `/reload` (currently rebuilds everything)

## Filed
2026-07-29 — noted during scaffold implementation.

## Severity
Low.

## Motivation
`POST /reload/{group}` currently re-scans `dsl_path` and rebuilds
the entire scheduler. That has two side effects:

1. Currently-running jobs get their task cancelled and
   re-spawned, resetting per-job state (e.g. the
   `next_fire_time` calculation restarts from *now*).
2. History correlation IDs (once we add them) would break.

## Fix / Design
On reload, compute the diff between old registry and new
registry:
- Same spec bytes → leave the running task alone.
- New job → spawn a new task.
- Removed job → cancel its task cleanly (wait for in-flight
  execution to finish, up to `shell_timeout_secs`).
- Changed spec → cancel + respawn with new spec.

Reuse the JVM's group-filter semantic: `/reload/{group}` only
diffs jobs whose group matches; other groups untouched.

## Acceptance
- [ ] A no-op reload (no file changes) leaves every scheduler
      task alive.
- [ ] Adding a job via new file + reload spawns exactly one new
      task; existing tasks untouched.
- [ ] Removing a job cancels its task without racing an
      in-flight execution.
- [ ] Integration test covers all three paths.

## Estimated effort
1 day.

## Dependencies
- Task 002 (real-DB test harness) if we want to assert history
  correlation-ID stability.

## Non-scope
- File-watching-based auto-reload (inotify / kqueue). Explicit
  `POST /reload` is the operator contract.

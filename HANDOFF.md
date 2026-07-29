# HANDOFF

**Written**: 2026-07-29
**Last verified green**: 2026-07-29 — cargo test 57/0/0 (39 unit
+ 15 integration + 3 postgres-conditional skipped without DB);
fmt + clippy `-D warnings` clean; cargo audit clean (1
documented ignore, `RUSTSEC-2023-0071` in transitive `rsa` via
`sqlx-postgres`, no upstream fix, mirrored to deny.toml with
review date 2027-01-31); cargo deny check all clean; mdbook +
linkcheck build clean; docker image
`cronmanager-on-rust:0.1.0-rc.1` (106 MB) builds, starts,
loads 11 shipped sample jobs, and answers `/health` +
`/jobs` on port 8080.
**Branch**: `dev` — released as `v0.1.0-rc.1` locally; **NOT yet
pushed to GitHub or Docker Hub / GHCR**. See "Post-AFK checklist"
below for the remaining publish steps that require account access.

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
cargo build --release --locked --bin cronmanager-on-rust
cargo test --no-fail-fast
cargo audit --deny warnings
( cd book && mdbook build )
docker build -t cronmanager-on-rust:0.1.0-rc.1 .
```

Live smoke (once you have compose up):

```bash
docker compose up -d
curl -fsS http://localhost:9010/health
curl -s http://localhost:9010/jobs | head
```

## Post-AFK checklist (Rainer to run)

The scaffold intentionally does **not** touch GitHub or Docker Hub.
When you're back:

1. **Create the GitHub repo** (empty, public):
   ```
   gh repo create Buerostack/CronManager-on-Rust --public \
     --description "YAML-driven cron scheduler (HTTP + shell). Rust reimplementation of buerokratt/CronManager."
   ```
2. **Push the branch**:
   ```
   git remote add origin git@github.com:Buerostack/CronManager-on-Rust.git
   git push -u origin dev
   ```
3. **Enable GitHub Pages via workflow**:
   ```
   gh api repos/Buerostack/CronManager-on-Rust/pages -X POST \
     -f 'build_type=workflow'
   ```
4. **Bump Actions permissions to read+write**:
   ```
   gh api repos/Buerostack/CronManager-on-Rust/actions/permissions/workflow \
     -X PUT \
     -F 'default_workflow_permissions=write' \
     -F 'can_approve_pull_request_reviews=false'
   ```
5. **Docker Hub setup** (per DEV-REQUIREMENTS §9.1):
   - Create the repo at
     https://hub.docker.com/repositories/buerostack → New
     repository → `cronmanager-on-rust` → Public.
   - Generate a Restricted-access token (Read + Write + Delete)
     scoped to that repo only.
   - Set secrets:
     ```
     gh secret set DOCKERHUB_USERNAME --repo Buerostack/CronManager-on-Rust --body 'buerostack'
     echo -n '<token>' | gh secret set DOCKERHUB_TOKEN --repo Buerostack/CronManager-on-Rust
     ```
6. **Push the tag** to trigger the publish workflow:
   ```
   git push origin v0.1.0-rc.1
   ```
   Watch it green, then link the GHCR package to the repo per
   DEV-REQUIREMENTS §9.1 step 7.
7. **Update this file**: replace the "Last verified green" note
   with the tag build's timestamp and `docker pull` from a
   fresh machine.

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
| Public docs (once Pages is live) | https://buerostack.github.io/CronManager-on-Rust/ |
| Full change history | [`./CHANGELOG.md`](./CHANGELOG.md) |
| Private security disclosure | [`./SECURITY.md`](./SECURITY.md) |
| CI workflows | [`.github/workflows/`](./.github/workflows/) |

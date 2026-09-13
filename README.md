# CronManager

YAML-driven cron scheduler for HTTP + shell jobs. Rust
reimplementation of [buerokratt/CronManager](https://github.com/buerokratt/CronManager).

**Version:** 0.2.0-alpha · **License:** Apache-2.0
· **Docs:** [turnerrainer.github.io/cronmanager](https://turnerrainer.github.io/cronmanager/)
· **Images:** `docker.io/turnerrainer/cronmanager:alpha`, `ghcr.io/turnerrainer/cronmanager:alpha`

Point CronManager at a folder of YAML job files. Each job becomes
a cron-scheduled outbound HTTP call or shell command, with retry,
manual-trigger, and time-window support. Execution history writes
to TimescaleDB when configured.

## One-command demo (loopback)

```bash
docker run -d --name cronmanager -p 127.0.0.1:8080:8080 \
  turnerrainer/cronmanager:alpha
curl http://localhost:8080/health          # {"status":"ok"}
curl -s http://localhost:8080/jobs | head  # loaded jobs, JSON
```

Bind to a non-loopback address and the process refuses to start
without an admin bearer token — see [Security posture](#security-posture)
below.

## Build from source

```bash
git clone -b dev https://github.com/turnerrainer/cronmanager.git
cd cronmanager
docker compose up -d --build
```

## Security posture

`v0.2.0-alpha` closes the h2ck.me v2 audit round (break-test
findings FN1–FN9, FN-LOG-1..4, F-CM-1/2/4) and adopts four
Buerostack fleet-stronghold patterns (§1.6 W3C traceparent,
§5.1 default security response headers, §11.1/11.2 environment-
aware boot safety gates) on top of the v0.1.4-alpha baseline
below.

- `POST /execute`, `POST /stop`, `POST /reload` are gated behind
  `Authorization: Bearer <token>` when
  `CRONMANAGER_ADMIN_TOKEN` (or the env var named by
  `admin.bearer_token_env`) is set. Optional per-group tokens
  scope credential blast radius via
  `security.per_group_token_envs`.
- Non-loopback bind without a token → refuses to start. Override
  with `admin.trust_network: true` if you terminate auth at a
  reverse proxy / service mesh (the runtime gate STILL enforces
  the token when one is set).
- Non-`dev` env (`APP_ENV=production` / `staging` / `test`) →
  refuses to boot on weak / default credentials or unsafe posture
  flags. `APP_ENV=dev` (or unset) preserves the zero-config
  loopback UX.
- HTTP jobs are refused if their host resolves to a private /
  loopback / link-local / ULA address (SSRF pre-flight).
- Shell `/execute` refuses env overrides on the dangerous-env
  blacklist (`PATH`, `LD_*`, `PYTHONPATH`, …) → `403`. Shell
  `command:` accepts either a whitespace-tokenised string or an
  explicit YAML list.
- `/jobs*` and `/running*` recon endpoints can be gated behind
  the admin token via `security.expose_jobs_publicly=false` /
  `security.expose_running_publicly=false` (default `true` for
  backward compat with v0.1.4-alpha operator dashboards).
- Every response carries CSP + HSTS + X-Frame-Options DENY +
  X-Content-Type-Options nosniff + Referrer-Policy no-referrer
  and W3C `traceparent` + `x-trace-id` for cross-service
  correlation.
- Docker rootfs is `read_only: true` in the shipped compose;
  `/tmp` is `tmpfs` for shell scratch.

For internet-exposed deployments, use the public baseline
config in [`CLAUDE.md` § Best-practice configs](./CLAUDE.md#best-practice-configs).

## Documentation

- **[`CLAUDE.md`](./CLAUDE.md)** — start here if you're an LLM
  assistant or a new developer landing cold. Covers breaking
  changes since `0.1.0-alpha.3` AND the additional shape /
  posture changes introduced in `0.2.0-alpha`, how to spot a
  broken config, how to fix common problems, and best-practice
  config baselines (loopback dev, public/internet, JVM-compat
  port).
- **Book** — [turnerrainer.github.io/cronmanager](https://turnerrainer.github.io/cronmanager/)
  (getting started, configuration reference, failure modes)
- **Design** — [`docs/DESIGN.md`](./docs/DESIGN.md) — what
  CronManager does and why
- **Standards** — [`STANDARDS.md`](./STANDARDS.md) — every generic
  build/docs/test/publish rule the project meets
- **Security** — [`SECURITY.md`](./SECURITY.md) — reporting
  channel + defensive-posture summary
- **Changelog** — [`CHANGELOG.md`](./CHANGELOG.md)
- **Original JVM CronManager** — <https://github.com/buerokratt/CronManager>

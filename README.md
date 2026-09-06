# CronManager

YAML-driven cron scheduler for HTTP + shell jobs. Rust
reimplementation of [buerokratt/CronManager](https://github.com/buerokratt/CronManager).

**Version:** 0.1.4-alpha · **License:** Apache-2.0
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

As of `v0.1.4-alpha` the h2ck.me v1 audit findings are all
addressed:

- `POST /execute`, `POST /stop`, `POST /reload` are gated behind
  `Authorization: Bearer <token>` when
  `CRONMANAGER_ADMIN_TOKEN` (or the env var named by
  `admin.bearer_token_env`) is set.
- Non-loopback bind without a token → refuses to start. Override
  with `admin.trust_network: true` if you terminate auth at a
  reverse proxy / service mesh.
- HTTP jobs are refused if their host resolves to a private /
  loopback / link-local / ULA address (SSRF pre-flight).
- Shell `/execute` refuses env overrides on the dangerous-env
  blacklist (`PATH`, `LD_*`, `PYTHONPATH`, …) → `403`.

For internet-exposed deployments, use the public baseline
config in [`CLAUDE.md` § Best-practice configs](./CLAUDE.md#best-practice-configs).

## Documentation

- **[`CLAUDE.md`](./CLAUDE.md)** — start here if you're an LLM
  assistant or a new developer landing cold. Covers breaking
  changes since `0.1.0-alpha.3`, how to spot a broken config, how
  to fix common problems, and best-practice config baselines
  (loopback dev, public/internet, JVM-compat port).
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

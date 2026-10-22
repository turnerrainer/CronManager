# CronManager

YAML-driven cron scheduler for HTTP + shell jobs. Rust
reimplementation of [buerokratt/CronManager](https://github.com/buerokratt/CronManager).

**Version:** 0.3.0-alpha · **License:** Apache-2.0
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

`v0.3.0-alpha` adopts **FLEET-STRONGHOLDS §11.3** — the cross-service
gate against dev-fixture DSLs shipped to non-dev environments (closes
#54). Any file under `dsl_path` whose path matches a documented
fixture pattern (`dev-login`, `mock-`, `-mock`, `/test/`, `example-`,
`-example`, `/dev/`, `/mocks/`) now refuses load under
`APP_ENV=production`/`staging`/`test`. In `dev`, same files WARN and
load. The opt-out `security.allow_dev_fixture_dsls: true` downgrades
everywhere to WARN and is itself treated as unsafe-in-prod by
env_safety §11.2 + a doctor WEAK. **Potentially breaking** for
operators with such filenames — migration notes in CLAUDE.md and
CHANGELOG. 303/303 tests pass (was 289 on 0.2.4-alpha; +14: env_safety
pattern/action matrix, loader-integration e2e, YAML parse coverage).

`v0.2.4-alpha` adds `security.allowed_private_hosts` on top of
`v0.2.3-alpha` (closes #49). Per-hostname SSRF exception so
cluster-internal targets (`http://ruuter-internal:8080`,
`http://my-svc.ns.svc.cluster.local`) work under
`APP_ENV=production` without flipping `block_private_networks`
off — previously the only options were "every cluster-internal
call blocked" or "refuses to boot". Exact match (no wildcards);
link-local / cloud metadata / `fd00:ec2::254` stay refused even
for listed hosts; IP literals and malformed entries ignored with
a doctor WEAK + boot WARN. Boot WARN also fires when the
allowlist is set alongside `block_private_networks: false`
(the allowlist is inert in that combination). 289/289 tests
pass (was 275 on 0.2.3-alpha; +14 invariant / pentest /
end-to-end coverage).

`v0.2.3-alpha` adds one feature + CI unblockers on top of
`v0.2.2-alpha`: HTTP jobs accept `headers:` and `headerEnvs:`
so cron jobs can call services behind a bearer / service-token
guard without an `exec` + `curl` wrapper (closes #39). Secrets
stay out of YAML — `headerEnvs` holds env-var NAMES only, values
are resolved at fire time, never logged, never persisted, with
HTTP/2 HPACK never-indexed via `set_sensitive(true)`. Forbidden-
header list (`host`, `content-length`, `transfer-encoding`,
`connection`, `upgrade`, `te`, `trailer`) is case-insensitive;
HTTP/2 pseudo-headers (`:authority`, `:method`, …) rejected; CRLF
injection via literal or env is caught at BOTH load and fire with
anti-leak error strings. One loader tightening: an env var set to
empty string is now rejected at load, matching the "fail early,
not on the first fire" contract. 266/266 tests pass (was 244 on
0.2.2-alpha; +22 header coverage).

`v0.2.2-alpha` closes the h2ck.me v1 audit-cycle carried over
from the v2 round: scheduler lock-order isolation (T-5), a
structured JSON body on tower_http body-limit 413s (T-15), 405
Method Not Allowed regression pins (T-17), graceful shutdown on
SIGTERM / SIGINT (T-18), a `CRONMANAGER_OFFLINE=true` env lever
that stubs every dispatch (U15), and the `cronmanager doctor`
offline config-audit subcommand (T-19). Additive on top of the
0.2.1-alpha posture below — no config, DSL, or HTTP surface
changes.

`v0.2.1-alpha` republished `v0.2.0-alpha` with a Dockerfile
`apt-get upgrade` on the runtime layer, absorbing 12 upstream-
fixed Debian security-channel CVEs (9 HIGH + 3 CRITICAL in
gzip / libpcre2 / libsqlite3 / perl-base) that Trivy flagged in
the v0.2.0-alpha publish. **v0.2.0-alpha is yanked** — do not
pull `docker.io/turnerrainer/cronmanager:0.2.0-alpha` or its
`ghcr.io` mirror.

The `0.2.0-alpha` line closed the h2ck.me v2 audit round
(break-test findings FN1–FN9, FN-LOG-1..4, F-CM-1/2/4) and
adopted four Buerostack fleet-stronghold patterns (§1.6 W3C
traceparent, §5.1 default security response headers, §11.1/11.2
environment-aware boot safety gates) on top of the v0.1.4-alpha
baseline below.

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
  posture changes introduced in `0.2.0-alpha` / `0.2.1-alpha` /
  `0.2.2-alpha` / `0.2.3-alpha` / `0.2.4-alpha` / `0.3.0-alpha`,
  how to spot a
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

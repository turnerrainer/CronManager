# CLAUDE.md

Orientation for LLM assistants and new developers landing in this
repo cold. Aim: after reading this file you know what the project
is, which behaviours changed recently, how to spot a broken
config, how to fix one, and which config shape is the current
best-practice baseline.

**Current release**: `0.2.0-alpha` (see `VERSION`, `Cargo.toml`,
`CHANGELOG.md`). Branch of record for active work: `dev` — this is
also the default branch on GitHub. There is no `main` branch yet;
alphas are cut as git tags on `dev` (`v0.1.4-alpha` → `fe1f517`,
`v0.2.0-alpha` → `7d72714`, both on dev's history) and published
as GitHub Releases (`v0.1.4-alpha` marked Latest / stable,
`v0.2.0-alpha` marked Pre-release). `main` will be created and
tagged only when the project reaches prod-ready state, targeted
at `v1.0.0`. **Never push a `main` branch or move code there
without explicit maintainer approval.**

## What the project is

Rust reimplementation of the JVM
[buerokratt/CronManager](https://github.com/buerokratt/CronManager).
Point it at a folder of YAML job files; each job becomes a
cron-scheduled outbound HTTP call or shell command with retry,
manual-trigger, and time-window support. Optional TimescaleDB
history. Single-instance scheduler — running two containers on the
same DSL doubles every fire.

Deeper reading, in order of usefulness for a maintainer:

- `book/src/configuration.md` — every `cronmanager.yaml` field with
  its type, default, and meaning.
- `book/src/failure-modes.md` — HTTP status → `error` code table.
- `docs/DESIGN.md` — domain rationale for what CronManager does.
- `STANDARDS.md` + `../DEV-REQUIREMENTS.md` — the ruleset the code
  is expected to satisfy.
- `SECURITY.md` — reporting channel + defensive-posture summary.

## Breaking changes since `0.1.0-alpha.3`

The `0.1.4-alpha` release landed the h2ck.me v1 audit fixes.
Operators upgrading from `0.1.0-alpha.*` need to know about these,
and an LLM editing config files needs to recognise the new shape.

1. **Admin bearer-token gate on state-changing endpoints**
   (`POST /execute`, `POST /stop`, `POST /reload`). Zero-config
   loopback deployments keep working (the gate short-circuits when
   no token is configured), but binding to a non-loopback address
   without a token now **refuses to start**:
   ```
   Error: refusing to start on non-loopback bind 0.0.0.0:8080
     without an admin token: set env var CRONMANAGER_ADMIN_TOKEN
     to enable the /execute + /stop + /reload gate, or set
     admin.trust_network=true if a reverse proxy / service mesh
     authenticates every request before it reaches this process
   ```
2. **SSRF pre-flight on HTTP jobs.** Literal private / loopback /
   link-local / ULA IPs (IPv4, IPv6, IPv4-mapped IPv6) are refused
   at DSL load; hostnames are DNS-resolved at fire time and
   refused if any returned address is non-routable. Reqwest
   redirects are disabled — a legitimate upstream can no longer
   302 the executor into a metadata endpoint.
3. **Dangerous-env blacklist** on `POST /execute?…` query params
   for shell jobs: `PATH`, `LD_*`, `DYLD_*`, `PYTHONPATH`,
   `NODE_OPTIONS`, `RUBYOPT`, `PERL5OPT`, `JAVA_TOOL_OPTIONS`,
   and the loader-hook families → `403 forbidden`.
4. **Bounded query string** on `/execute` — above
   `security.max_query_params` (default 64) → `413`.
5. **`/reload` per-group throttle** — second reload within
   `security.reload_min_interval_secs` (default 60s) → `429` with
   `retry_after_secs`.
6. **DSL load caps** — per-file YAML above
   `security.max_dsl_file_bytes` (default 1 MiB) is refused at
   load; whole walk aborts after
   `security.dsl_load_timeout_secs` (default 15s).
7. **`retryCount` cap** — DSL with `retryCount >
   security.max_retry_count` (default 10) is refused at load;
   WARN above 5.
8. **History body truncation** — `response_body` capped at
   `security.stored_response_body_max_bytes` (default 64 KiB) per
   row via head+tail slice with an inline marker.
9. **Log-injection sanitiser** — captured shell stderr and non-2xx
   HTTP response bodies are stripped of CR/LF/ANSI/control bytes
   before landing in logged error strings.
10. **Runtime base image** — `debian:bookworm-slim` →
    `debian:13.6-slim` (Snyk-flagged upgrade).

None of the new fields are **required**; every one has a safe
default. But when reading logs or writing a config for an
internet-exposed deployment, expect the two hardest-hitting
diagnostics to be the boot refusal above and the SSRF WARN if
`block_private_networks=false`.

Older prose to be aware of: `#[serde(deny_unknown_fields)]` has
been on every config and DSL struct since `0.1.0-alpha.2`. A typo
like `dslpath:` (missing underscore) or `retryCoun: 3` is a hard
load error, not a silent no-op.

## Breaking / non-obvious changes in `0.2.0-alpha` (since `0.1.4-alpha`)

**None of these are required by upgraders** — every new field
defaults to the pre-existing behaviour. But an LLM editing this
tree must recognise them; some (11, 12) are load-bearing shape
changes to the DSL and executor API.

1. **`/healthz` alias for k8s** (h2ck.me FN1 / F-CM-4). Registered
   alongside `/health` and `/actuator/health`, unauthenticated.
   All three routes return the same body — livenessProbe defaults
   in most Helm charts hit `/healthz`.

2. **Loader is skip-and-warn per file** (h2ck.me FN2). A single
   bad DSL file no longer aborts the whole boot / `/reload`; it
   logs at ERROR (naming the source_path and reason) and the walk
   continues. Sibling good files still register. If EVERY file
   fails, boot succeeds with 0 jobs plus a distinguishable WARN
   ("scheduler has 0 jobs registered"). Only `read_dir` failures
   remain fatal.

3. **Docker compose `read_only: true`** (h2ck.me FN9). Rootfs is
   immutable in the shipped compose; `/tmp` is `tmpfs` (64 MiB)
   for shell-job scratch. Any job needing persistent scratch must
   point at a bind-mounted volume added by the operator.

4. **Rich auth-failure WARN log** (h2ck.me FN-LOG-2). Every
   admin-gate reject now emits:
   ```
   WARN auth: rejected admin request
        client_ip_hash=a1b2c3d4 route=/execute/:group/:job
        reason="token does not match expected"
   ```
   `client_ip_hash` is a 4-byte SHA-256 prefix (PII-safe). The
   old generic WARN in `error::IntoResponse` is DEBUG for
   Unauthorized so the same reject doesn't double-log.

5. **`security.expose_jobs_publicly` + `security.expose_running_publicly`**
   (h2ck.me FN3 / F-CM-1). Both default `true` (backward-compat).
   Set either to `false` to require the admin bearer on the
   corresponding recon endpoint. `/health`, `/healthz`,
   `/actuator/health`, `/actuator/info`, `/` always stay public.
   Boot WARN if either is left `true` AND a token is configured.

6. **Slow-body deadline via `TimeoutLayer`** (h2ck.me FN5). The
   whole request/response (headers + body-read + handler +
   response-write) is bounded by `limits.request_timeout_secs`.
   Setting the value to 0 disables the layer (documented WARN).

7. **`ReloadGate` memory bounded** (h2ck.me PR-review nit #1).
   The last-fired-per-group map opportunistically evicts entries
   older than `10 × min_interval_secs` and hard-caps at 1024
   distinct groups (oldest evicted). Zero behaviour change for
   deployments with < 1024 groups.

8. **History `response_body` sanitised BEFORE persist** (h2ck.me
   PR-review nit #3). `security::truncate_response_body` now runs
   `sanitize_for_persistence` first so downstream DB-row viewers
   don't see raw CR/LF/ANSI. Escape rules match `sanitize_for_log`
   but without the 4 KiB cap — the persist path uses
   `stored_response_body_max_bytes` (64 KiB) as its size bound.

9. **Access-log middleware** (h2ck.me FN-LOG-3). Every completed
   request emits one INFO line:
   `http_request_completed method= route= status= duration_us= trace_id=`.
   Never logs headers / bodies / raw URIs — matched route pattern
   only.

10. **W3C Trace Context response headers** (FLEET §1.6). Every
    response now carries `traceparent: 00-<trace_id>-<span_id>-01`
    and `x-trace-id: <trace_id>`. Inbound `traceparent` is
    inherited when well-formed; otherwise a fresh 32-hex uuid is
    generated.

11. **⚠️ DSL `command:` field accepts EITHER string OR list**
    (h2ck.me FN7 + FN8). The string form is whitespace-tokenised
    exactly as before (JVM parity). The **list form**
    (`command: ['/bin/sh', '-c', 'sleep 30; echo done']`) is
    passed through verbatim as argv. Both normalise to an argv
    `Vec<String>` at load time — **`JobKind::Exec` now holds
    `argv: Vec<String>`, NOT `command: String`.** Any tool or
    test that constructs a `JobSpec` directly must use the new
    shape. `JobKind::exec_command_display()` returns the argv
    space-joined for logs.

12. **`security.per_group_token_envs: BTreeMap<String, String>`**
    (h2ck.me F-CM-2). Optional map from group name to env-var
    name. When populated, a request to a group-scoped admin
    endpoint (`/execute/:g/:j`, `/stop/:g/:j`, `/reload/:g`) is
    accepted with EITHER the master admin token OR the group-
    specific token. Groups not in the map only accept the
    master. Values are env-var names, not raw tokens.

13. **Default security response headers on every response**
    (FLEET §5.1). CSP `default-src 'none'; frame-ancestors 'none'`,
    HSTS 2y+includeSubDomains+preload, X-Frame-Options DENY,
    X-Content-Type-Options nosniff, Referrer-Policy no-referrer.
    Handler-set values are preserved (`entry().or_insert()`).

14. **Log stream is plain-text under Docker / systemd**
    (h2ck.me FN-LOG-1). `tracing_subscriber::fmt()` only emits
    ANSI when stderr is a TTY. SIEM parsers see clean bytes.

15. **Env-aware boot safety gates** (FLEET §11.1 + §11.2). New
    module `src/env_safety.rs`:
    - Reads `APP_ENV` / `ENVIRONMENT` / `DEPLOY_ENV` (first match
      wins). Unknown values fail-safe to `Production`.
    - In non-`dev`, REFUSES to boot when weak / default
      credentials are detected (admin token, DB password) or when
      documented-unsafe posture flags are set
      (`admin.trust_network`, `security.block_private_networks=false`,
      `security.allow_dangerous_env_overrides=true`).
    - In `dev`, same conditions produce WARN and boot continues.
    - Weak-pattern list covers substrings seen in real audits:
      `changeit`, `password`, `01234`, `dev-`, `-test`, `example`,
      `test-admin-token`, plus min-length checks (32 for tokens,
      12 for passwords).

**Boot log signals introduced by the above.** Diagnostics table
below adds these substrings on top of the `0.1.4-alpha` set:

| WARN substring | Cause / meaning |
|---|---|
| `weak credential (dev env, permitted)` | Env-safety gate found a weak default; boot continues because `APP_ENV=dev` (or unset). |
| `unsafe posture (dev env, permitted)` | Env-safety gate found `trust_network` / SSRF-off / dangerous-env-on in dev. |
| `expose_jobs_publicly=true` | Recon endpoints `/jobs*` are open despite a token being configured — flip `security.expose_jobs_publicly: false`. |
| `expose_running_publicly=true` | Same for `/running*`. |
| `per_group_token_envs[X]=Y is unset or empty` | Env var named in `security.per_group_token_envs` isn't exported. Group X falls back to master-only. |
| `dsl: N/M file(s) failed to load — see preceding ERROR lines` | Skip-and-warn triggered. Look up-log for the per-file `dsl: skipping file` ERROR lines. |
| `dsl: no jobs loaded — every DSL file … failed to parse` | Distinct signal that the tree is fully broken even though boot succeeded. |
| `MYSVC_OFFLINE=true` (n/a here) | Not implemented in CronManager; FLEET §9.1 reference. |

**Hard boot failures introduced by env-safety:**

| Error prefix | Cause | Fix |
|---|---|---|
| `REFUSING TO START in Production: N weak/default credential(s) detected` | Env-safety §11.1: admin token / DB password matched a weak pattern or is too short. | Rotate the affected env var(s) to a strong value; the error names each redacted (`01****ef`). Or set `APP_ENV=dev` to permit dev defaults locally. |
| `REFUSING TO START in Production: N unsafe posture item(s)` | Env-safety §11.2: `admin.trust_network=true` / `block_private_networks=false` / `allow_dangerous_env_overrides=true` in a non-dev env. | Flip each named flag to its safe default. |

## Finding problematic configs

**Fast triage: read the first 20 lines of the boot log.** The
process emits one INFO summary of every field plus one WARN per
loosened default. If any of the WARN lines below appear, the
config is worth reviewing.

| WARN substring | What's wrong |
|---|---|
| `allowed_origins contains "*"` | CORS layer treats `*` as an *exact* origin, not a wildcard. |
| `limits.request_timeout_secs=0 disables` | Slow upstream can pin the executor indefinitely. |
| `limits.shell_timeout_secs=0 disables` | Runaway shell process will not be SIGKILLed. |
| `admin bearer token not configured` | `/execute` + `/stop` + `/reload` are UNAUTHENTICATED. |
| `admin.trust_network=true` | Non-loopback bind without a token allowed — safe only behind an authenticating proxy. |
| `block_private_networks=false` | HTTP jobs can reach `169.254.169.254` and other metadata endpoints. |
| `allow_dangerous_env_overrides=true` | `PATH`, `LD_*`, `PYTHONPATH` … can be set from `/execute` query params. |

**Hard boot errors that name the fix:**

| Error prefix | Cause | Fix |
|---|---|---|
| `refusing to start on non-loopback bind` | Public-facing bind with no admin token and `trust_network=false`. | Set `CRONMANAGER_ADMIN_TOKEN` env var OR set `admin.trust_network=true` behind an authenticating proxy OR bind to `127.0.0.1`. |
| `top-level 'application:' is the JVM Spring wrapper` (also `spring:`, `management:`, `logging:`) | JVM `application.yml` pasted verbatim. | Unwrap the child fields to the top level; camelCase names alias to their snake_case counterparts. |
| `unknown field 'X'` on config load | Typo in `cronmanager.yaml`. Fields validated with `deny_unknown_fields`. | Read the error — it lists every accepted field. Fix the spelling. |
| `unknown field 'X'` on DSL load | Typo in a job YAML under `dsl_path`. | Same: fix the spelling per the accepted-fields list in the error. |
| `retryCount N exceeds cap M` | DSL asks for more retries than `security.max_retry_count`. | Lower `retryCount`, or raise the cap deliberately and accept the WARN. |
| `dsl file X exceeds N bytes` | Single YAML above `security.max_dsl_file_bytes`. | Split the file, or raise the cap. |
| `env var X is unset but database.url is configured` | `database.password_env` names an env var that isn't exported. | Export the env var (or unset `database` to disable history). |

**Runtime problem signals in responses (all bodies:
`{"error": "<code>", "message": "<human>"}`):**

| Status | `error` | Meaning |
|---|---|---|
| `401` | `unauthorized` | Missing / wrong `Authorization: Bearer <token>` on a gated endpoint. |
| `403` | `forbidden` | Dangerous env key on `/execute?…`; or SSRF-blocked URL. |
| `413` | `request_too_large` | Body over `limits.max_request_bytes`. |
| `413` | `too_many_query_params` | Query pairs over `security.max_query_params`. |
| `429` | `too_many_requests` | `/reload` throttle for the group. |
| `400` | `invalid_config` | Config file structurally can't load. |
| `400` | `invalid_job_definition` | DSL problem — unknown `type`, missing `url`, `trigger: true`, etc. |
| `400` | `invalid_cron` | Not a 6-field Quartz cron expression. |

See `book/src/failure-modes.md` for the full status → code table.

## Fixing common problems

### Config won't load: unknown field

```
error: invalid config file ./cronmanager.yaml: unknown field
  `dslpath`, expected one of `port`, `dsl_path`, `app_root_path`,
  `allowed_origins`, `shell_environment`, `limits`, `admin`,
  `security`, `database`
```

- Fix the typo — the error lists every valid field.
- JVM camelCase forms are also accepted: `configPath`,
  `appRootPath`, `allowedOrigins`, `shellEnvironment`.

### Config won't load: JVM Spring wrapper

Pasting a JVM `application.yml` verbatim fails with a diagnostic
that names the wrapper and shows the inline fix. Unwrap the child
fields to the top level. See `book/src/jvm-porting.md` for the
field-by-field port.

### Process refuses to start on a public bind

```
Error: refusing to start on non-loopback bind 0.0.0.0:8080 without
  an admin token
```

Pick one:

1. **Recommended for internet-exposed deployments.** Export the
   token and pass it on every state-changing call:
   ```bash
   export CRONMANAGER_ADMIN_TOKEN="$(openssl rand -hex 32)"
   docker run -d -p 8080:8080 \
     -e CRONMANAGER_ADMIN_TOKEN \
     turnerrainer/cronmanager:alpha
   curl -X POST http://host:8080/reload/mygroup \
     -H "Authorization: Bearer $CRONMANAGER_ADMIN_TOKEN"
   ```
2. **Behind an authenticating reverse proxy / service mesh.**
   Set `admin.trust_network: true` in `cronmanager.yaml`. A boot
   WARN fires so this doesn't disappear silently.
3. **Loopback only.** Publish the port as
   `-p 127.0.0.1:8080:8080` and drop the token requirement.

### HTTP job fails with SSRF-blocked URL

Default posture (`security.block_private_networks: true`) refuses
private / loopback / link-local / ULA targets. If the target is
legitimately on your private network:

```yaml
security:
  block_private_networks: false   # WARN at boot; re-enables SSRF lane
```

Prefer a narrower fix (host allowlist at the network layer, or
proxy) if you can.

### Shell job `/execute` returns `403 forbidden` on env override

The key you tried to override (`PATH`, `LD_*`, `PYTHONPATH`, …) is
on the dangerous-env blacklist. Either:

- Rename the env var to something outside the blacklist and update
  the job to read the new name, or
- Set `security.allow_dangerous_env_overrides: true` (boot WARN).

### `/reload` returns `429 too_many_requests`

The per-group throttle (default 60s) fired. Either wait for
`retry_after_secs`, or lower `security.reload_min_interval_secs`
if your ops flow legitimately reloads more often.

## Best-practice configs

### Loopback / local dev (safe default)

Zero-config: the gate short-circuits, history is disabled, jobs
run under safe defaults. This is what `docker run … -p
127.0.0.1:8080:8080 turnerrainer/cronmanager:alpha` gives you.

If you want to check in a config anyway:

```yaml
port: 8080
dsl_path: /app/DSL/samples

# Server-to-server posture — no browser CORS lane.
allowed_origins: []

limits:
  max_request_bytes: 1048576      # 1 MiB
  max_response_bytes: 16777216    # 16 MiB
  request_timeout_secs: 30
  shell_timeout_secs: 300
```

Bind: `127.0.0.1:8080` (or map with `-p 127.0.0.1:8080:8080`).

### Public / internet-exposed (recommended baseline)

Assumes `CRONMANAGER_ADMIN_TOKEN` and `CRONMANAGER_DB_PASSWORD`
are set in the environment (secret-manager, docker secret, k8s
secret — never in the file).

```yaml
port: 8080
dsl_path: /app/DSL
app_root_path: /app

# List every allowed origin exactly. "*" is treated as a literal
# origin, NOT a wildcard.
allowed_origins:
  - "https://ops.example.com"

# Only whitelist keys that a shell job actually needs.
shell_environment:
  BACKUP_DIR: /var/lib/cronmanager/backups
  RETENTION_DAYS: "7"

limits:
  max_request_bytes: 1048576
  max_response_bytes: 16777216
  request_timeout_secs: 30
  shell_timeout_secs: 300

admin:
  bearer_token_env: CRONMANAGER_ADMIN_TOKEN
  trust_network: false            # keep the refusal on unless behind an auth proxy

security:
  block_private_networks: true
  allow_dangerous_env_overrides: false
  max_query_params: 64
  max_dsl_file_bytes: 1048576
  dsl_load_timeout_secs: 15
  max_retry_count: 10
  min_cron_interval_secs: 10
  stored_response_body_max_bytes: 65536
  reload_min_interval_secs: 60
  # h2ck.me v2 hardening (post-0.1.4-alpha). Both public flags
  # default true for backward compat; flip to false whenever the
  # operator dashboard doesn't need anonymous read access.
  expose_jobs_publicly: false
  expose_running_publicly: false
  # Optional per-group tokens (h2ck.me F-CM-2). Empty by default.
  # per_group_token_envs:
  #   payroll:   PAYROLL_ADMIN_TOKEN
  #   logistics: LOGISTICS_ADMIN_TOKEN

database:
  url: postgres://cronmanager@timescaledb:5432/cronmanager
  password_env: CRONMANAGER_DB_PASSWORD
```

Every `security.*` value shown here is the current default (except
the two `expose_*_publicly` flags, which default `true` for
backward compat — flipping them to `false` in the file makes the
tightened posture visible). The block is written out explicitly so
an operator reading the file sees the posture at a glance.

**Environment marker.** For any deployment above `dev` set exactly
one of these env vars so `env_safety` treats weak defaults and
unsafe posture flags as fatal:

```bash
APP_ENV=production   # or ENVIRONMENT=production / DEPLOY_ENV=production
```

Unknown / missing values fail-safe to `Production` — a typo like
`APP_ENV=prroduction` refuses weak creds too.

### Behind an authenticating reverse proxy or service mesh

Same as the public baseline, but flip `admin.trust_network: true`
if you cannot pass through the bearer token. Boot log will
carry the WARN reminding you that the network path is the only
control left in front of `/execute`, `/stop`, `/reload`.

### JVM-compat port

Copied a JVM `application.yml`? The camelCase field names bind
via `#[serde(alias)]` and `allowed_origins` accepts the JVM
comma-separated string form:

```yaml
configPath: /app/DSL
appRootPath: /app
allowedOrigins: "https://ops.example.com,https://ops.example.net"
shellEnvironment:
  BACKUP_DIR: /var/lib/cronmanager/backups
```

Anything under the JVM top-level wrappers (`application:`,
`spring:`, `management:`, `logging:`) needs to be unwrapped one
level up — the boot diagnostic will name the wrapper and show the
fix inline.

## Verification set

Run these on any change; every one should exit 0.

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo build --release --locked --bin cronmanager
cargo test --no-fail-fast
cargo audit --deny warnings
cargo deny check all
( cd book && mdbook build )
```

Live smoke (once compose is up):

```bash
docker compose up -d
curl -fsS http://localhost:9010/health
curl -s http://localhost:9010/jobs | head
```

Public-bind gate smoke (should fail without a token, succeed with
one):

```bash
# Refuses to start:
docker run --rm -p 8080:8080 turnerrainer/cronmanager:alpha

# Boots:
docker run --rm -p 8080:8080 \
  -e CRONMANAGER_ADMIN_TOKEN=t3st turnerrainer/cronmanager:alpha
```

## When editing this repo

- Follow `STANDARDS.md` (which defers to `../DEV-REQUIREMENTS.md`
  front-to-back).
- Version bumps touch **every** file listed in DEV-REQUIREMENTS §8
  atomically: `Cargo.toml`, `Cargo.lock`, `VERSION`,
  `docker-compose.yml`, `README.md`, `book/src/introduction.md`,
  `CHANGELOG.md` (and the local internal HANDOFF, if present).
  Miss one and the release audit will catch it, but so will an
  LLM re-reading the tree.
- **Never bump the version, cut a tag, or create a GitHub Release
  without explicit maintainer approval.** These are the ONLY
  actions in this repo that require confirmation regardless of
  the invocation mode. When a release IS approved, tag on `main`,
  push the tag, and use `gh release create <tag> --title <tag>
  --notes-file <notes>` so the "Releases" page mirrors the tag.
- Work on `dev`. Merge to `main` only when tagging a release.
- New security controls belong in `src/security.rs` — keep the
  primitives generic so a future extraction into a shared
  `buerostack-security` workspace crate stays cheap.

### Writing / editing a DSL `command:` field (h2ck.me FN7 + FN8)

Two accepted shapes. Pick the RIGHT one for what the job actually
needs — the tokenisation rules are different and NOT interchange-
able.

**String form (JVM parity).** Whitespace-split, no shell
interpretation, no quoting. Use when the program takes only
simple positional arguments.

```yaml
command: /bin/echo hello world
command: ./scripts/samples/backup.sh /var/lib/data
```

`"/bin/sh -c \"sleep 30\""` in string form does NOT work — the
double quotes are literal characters, not shell metacharacters.
Attempting this fails with `Syntax error: Unterminated quoted
string` at the child exec. If you need shell semantics, use the
LIST form.

**List form (explicit argv).** Passed through verbatim; each list
element becomes one argv entry.

```yaml
command: ['/bin/sh', '-c', 'sleep 30; echo done']
command:
  - /usr/bin/python3
  - -c
  - "import time; time.sleep(30); print('done')"
```

Either form normalises to `Vec<String>` at load time. Empty strings
and empty lists are refused at load with `command field is empty`.

### Writing a Rust test that constructs `JobKind::Exec` directly

The field is `argv: Vec<String>`, NOT `command: String`. If you're
editing a fixture that predates the FN7/FN8 refactor, update the
shape:

```rust
// After (correct):
kind: JobKind::Exec {
    argv: vec!["/bin/sleep".into(), "3".into()],
    allowed_envs: vec![],
},

// Before (broken — will not compile):
kind: JobKind::Exec {
    command: "/bin/sleep 3".into(),
    allowed_envs: vec![],
},
```

`JobKind::exec_command_display()` returns the space-joined form
for logs when you need the human-readable version.

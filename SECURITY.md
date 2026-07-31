# Security policy

## Reporting a vulnerability

Please **do not open a public GitHub issue** for security-sensitive
findings. Instead:

1. **Preferred**: use GitHub's private vulnerability reporting for
   this repo — Security tab → **Report a vulnerability**. That
   routes the report to maintainers via a private thread with
   tracking.
2. **Fallback**: email `rainer.turner@gmail.com` with
   `[CronManager-security]` in the subject line.

Include, when you can:

- Affected version (image tag or git ref)
- Reproduction steps or PoC
- Impact assessment (what an attacker gains)
- Any suggested mitigation

## Response commitments

- **Acknowledgement**: within 3 business days of the report reaching
  a maintainer.
- **Triage decision** (accepted / needs-more-info / not-a-vuln):
  within 7 business days.
- **Fix + coordinated disclosure**: target 30 days for CRITICAL and
  HIGH severity, 90 days for MEDIUM. Extension is negotiable if a
  fix requires a coordinated upstream change.
- **Credit**: reporters are credited in the release notes unless
  they ask to remain anonymous.

## Supported versions

Only the latest published release receives security fixes.
CronManager is pre-1.0 and follows SemVer — minor bumps
are the norm, patch releases are cut only for critical fixes on
the current line.

| Version   | Support status                                     |
|-----------|----------------------------------------------------|
| `0.1.x`   | ✅ Supported (initial scaffold)                    |
| `< 0.1.0` | n/a                                                |

## What we do to reduce supply-chain risk

Every rule below is documented in [`STANDARDS.md`](./STANDARDS.md).

- **`cargo audit --deny warnings`** — every push, every PR, daily
  at 06:00 UTC. Advisory exceptions live in `.cargo/audit.toml`
  with a rationale and a review date; blind ignores are a code
  smell.
- **`cargo deny check all`** — enforces license allow-list
  (Apache-2.0 compatible only, no GPL/AGPL/SSPL), refuses git-URL
  deps and wildcard version specs, warns on duplicate crate
  versions. Config: [`deny.toml`](./deny.toml).
- **Trivy image scan** on every release-tag publish, gated on
  `HIGH` and `CRITICAL` fixed vulnerabilities. Blocks signing.
- **cosign keyless signatures** on every published image digest
  via Sigstore OIDC.
- **In-toto provenance + SPDX SBOM** attached to every multi-arch
  manifest.
- **Reproducible image layer timestamps** (`SOURCE_DATE_EPOCH` +
  `rewrite-timestamp=true`) so the same commit produces the same
  image digest.
- **Multi-arch smoke test** — every release image is booted under
  QEMU on both `linux/amd64` and `linux/arm64` and probed with
  `/health` before it's signed.
- **Non-root container user** (uid 1000), `cap_drop: ALL`,
  `no-new-privileges: true` in the shipped `docker-compose.yml`.

## Application-layer defensive posture

- **Request/response size caps** on every HTTP surface —
  `limits.max_request_bytes` (inbound REST) and
  `limits.max_response_bytes` (upstream HTTP responses captured
  into history). Overflow → structured `413` (inbound) or `502`
  (upstream).
- **Explicit timeouts** on every outbound HTTP call
  (`limits.request_timeout_secs`) and shell command
  (`limits.shell_timeout_secs`). Never unbounded.
- **Shell env allow-list** — `allowedEnvs` on each shell job
  whitelists which entries from `shell_environment` are exposed
  to the child process. Nothing leaks by default.
- **No admin HTTP endpoints in the same process** as job APIs.
  Any operational admin (metrics, remote pause, secrets rotation)
  belongs at the infra layer per DEV-REQUIREMENTS §5.3.
- **Passwords come from env vars only.** `database.password_env`
  names the env var; a plain `password:` field is rejected at
  startup. No default password is ever shipped.

## What is out of scope

- Secret fetching (Vault / KMS / Docker secrets) — operator
  responsibility; supply via env vars.
- Authentication / authorisation on the REST API — terminate at
  a reverse proxy.
- Rate limiting — terminate at a reverse proxy.
- Persistent state / cross-replica coordination — Postgres is
  the shared substrate when history is enabled; job scheduling
  itself is single-instance.

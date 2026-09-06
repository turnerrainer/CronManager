//! CronManager runtime configuration.
//!
//! Wire shape (`cronmanager.yaml`): see book/src/configuration.md.
//!
//! Search order for the file:
//! 1. `--config <path>` CLI flag
//! 2. `CRONMANAGER_CONFIG` env var
//! 3. `./cronmanager.yaml` or `./cronmanager.yml`
//! 4. Built-in defaults if nothing is found
//!
//! JVM-compat notes:
//!
//! * `#[serde(deny_unknown_fields)]` at every level — a typo in
//!   the operator's file is a hard load error, not a silent no-op.
//! * `#[serde(alias = "…")]` on every field the JVM
//!   `application.yml` spelled differently, so a copy-paste port
//!   binds without a rename.
//! * `allowed_origins` accepts both a YAML list (native form) and
//!   the JVM-style comma-separated string, via a custom
//!   deserialiser.
//! * The top-level JVM Spring wrappers (`application:`, `spring:`,
//!   `management:`, `logging:`) are caught by a preflight and
//!   rejected with a diagnostic that names the wrapper so the
//!   operator knows what to unwrap.

use crate::error::CronManagerError;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AppConfig {
    #[serde(default = "default_port")]
    pub port: u16,

    #[serde(default = "default_dsl_path", alias = "configPath")]
    pub dsl_path: PathBuf,

    #[serde(default = "default_app_root_path", alias = "appRootPath")]
    pub app_root_path: PathBuf,

    /// Empty list disables the CORS layer entirely. Accepts either
    /// a YAML list (`["a", "b"]`) or a JVM-style comma-separated
    /// string (`"a,b"`).
    #[serde(default, alias = "allowedOrigins", deserialize_with = "de_origins")]
    pub allowed_origins: Vec<String>,

    /// Baseline environment map for shell jobs. Each job still
    /// whitelists specific keys via `allowedEnvs`.
    #[serde(default, alias = "shellEnvironment")]
    pub shell_environment: BTreeMap<String, String>,

    #[serde(default)]
    pub limits: Limits,

    /// Admin auth for state-changing endpoints (`/execute`,
    /// `/stop`, `/reload`). Default posture is loopback-only:
    /// binding to a non-loopback address without a bearer token
    /// (and without `admin.trust_network=true`) is refused at boot.
    /// See book/src/security.md.
    #[serde(default)]
    pub admin: AdminConfig,

    /// Security hardening knobs — SSRF/DoS defence-in-depth. Every
    /// field ships with a safe default; overrides exist for
    /// operators who really need them (with a boot-time WARN).
    #[serde(default)]
    pub security: SecurityConfig,

    /// Optional persistence. Absent → NoopRecorder (history
    /// disabled, jobs still run).
    #[serde(default)]
    pub database: Option<DatabaseCfg>,
}

/// Resource ceilings. Defaults sized for typical HTTP + shell
/// workloads. Increase carefully — every raise widens the DoS
/// surface at the boundary.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    #[serde(default = "default_max_request_bytes")]
    pub max_request_bytes: usize,

    #[serde(default = "default_max_response_bytes")]
    pub max_response_bytes: usize,

    #[serde(default = "default_request_timeout_secs")]
    pub request_timeout_secs: u64,

    #[serde(default = "default_shell_timeout_secs")]
    pub shell_timeout_secs: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_request_bytes: default_max_request_bytes(),
            max_response_bytes: default_max_response_bytes(),
            request_timeout_secs: default_request_timeout_secs(),
            shell_timeout_secs: default_shell_timeout_secs(),
        }
    }
}

/// Admin bearer-token gate. Applied to every state-changing
/// endpoint (`/execute`, `/stop`, `/reload`) when a token is
/// configured. Read-only endpoints (`/health`, `/jobs`, …) stay
/// unauthenticated so operator dashboards keep working.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminConfig {
    /// Env var name to read the bearer token from at boot. Empty
    /// or unset env var → no token configured. Default:
    /// `CRONMANAGER_ADMIN_TOKEN`.
    #[serde(default = "default_admin_token_env", alias = "bearerTokenEnv")]
    pub bearer_token_env: String,

    /// Opt-out for private-network / mesh-authenticated
    /// deployments. When true, the boot-time refusal to start on a
    /// non-loopback bind without a token is skipped. Logged as a
    /// WARN so operators notice.
    #[serde(default, alias = "trustNetwork")]
    pub trust_network: bool,
}

impl Default for AdminConfig {
    fn default() -> Self {
        Self {
            bearer_token_env: default_admin_token_env(),
            trust_network: false,
        }
    }
}

/// Security hardening knobs. Defaults are safe; overrides are
/// per-field so an operator can loosen a single control without
/// silently loosening everything.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SecurityConfig {
    /// SSRF: refuse HTTP job URLs whose host is a private,
    /// loopback, link-local, ULA, or otherwise non-routable IP.
    /// Applied at DSL load time (literal IPs) AND at fire time
    /// (hostname DNS resolution). Default: true.
    #[serde(default = "default_true", alias = "blockPrivateNetworks")]
    pub block_private_networks: bool,

    /// Bypass the shell dangerous-env blacklist (`PATH`, `LD_*`,
    /// `DYLD_*`, `PYTHONPATH`, `NODE_OPTIONS`, …). Only enable if
    /// you deliberately need one of those vars overridable via
    /// `/execute?…` query string. WARN at boot when true.
    #[serde(default, alias = "allowDangerousEnvOverrides")]
    pub allow_dangerous_env_overrides: bool,

    /// Cap on the number of query parameters accepted by
    /// `POST /execute/…`. Above the cap → 413. A shell job
    /// legitimately needs a handful; the cap kills the
    /// `?a=1&b=2&…` memory-DoS lane without hurting real users.
    /// Default: 64.
    #[serde(default = "default_max_query_params", alias = "maxQueryParams")]
    pub max_query_params: usize,

    /// Per-file cap on DSL YAML size, in bytes. Files above the
    /// cap are refused at load with a diagnostic that names the
    /// file + byte count. Kills the YAML anchor-bomb amplification
    /// vector. Default: 1 MiB.
    #[serde(default = "default_max_dsl_file_bytes", alias = "maxDslFileBytes")]
    pub max_dsl_file_bytes: u64,

    /// Overall wall-clock cap on `loader::load_all`, in seconds.
    /// The per-file byte cap is the main defence; this timeout is
    /// belt-and-braces against an unforeseen amplification in
    /// `serde_yaml_ng`. Default: 15s.
    #[serde(
        default = "default_dsl_load_timeout_secs",
        alias = "dslLoadTimeoutSecs"
    )]
    pub dsl_load_timeout_secs: u64,

    /// Cap on the `retryCount` DSL field. Retries above the cap
    /// are refused at load. Default: 10 (WARN above 5). Protects
    /// against a misconfigured `retryCount: 4294967295` pinning
    /// an executor slot for hours.
    #[serde(default = "default_max_retry_count", alias = "maxRetryCount")]
    pub max_retry_count: u32,

    /// Minimum acceptable interval between two consecutive cron
    /// fires. Jobs whose expression fires more often than this
    /// emit a WARN at load. Set to 0 to disable. Default: 10s.
    #[serde(
        default = "default_min_cron_interval_secs",
        alias = "minCronIntervalSecs"
    )]
    pub min_cron_interval_secs: i64,

    /// Cap on the bytes of `response_body` (HTTP) / stdout (shell)
    /// stored in the history table. Above the cap → head+tail
    /// slice with an inline truncation marker. Prevents unbounded
    /// row growth on chatty upstreams / scripts. Default: 64 KiB.
    #[serde(
        default = "default_stored_response_body_max_bytes",
        alias = "storedResponseBodyMaxBytes"
    )]
    pub stored_response_body_max_bytes: usize,

    /// Minimum seconds between two `POST /reload/…` calls for the
    /// same group. Second request within the window → 429. Legit
    /// ops workflows reload once per commit; the cap kills the
    /// `/reload` flood amplifier for load-time bugs. Default: 60s.
    #[serde(
        default = "default_reload_min_interval_secs",
        alias = "reloadMinIntervalSecs"
    )]
    pub reload_min_interval_secs: u64,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            block_private_networks: true,
            allow_dangerous_env_overrides: false,
            max_query_params: default_max_query_params(),
            max_dsl_file_bytes: default_max_dsl_file_bytes(),
            dsl_load_timeout_secs: default_dsl_load_timeout_secs(),
            max_retry_count: default_max_retry_count(),
            min_cron_interval_secs: default_min_cron_interval_secs(),
            stored_response_body_max_bytes: default_stored_response_body_max_bytes(),
            reload_min_interval_secs: default_reload_min_interval_secs(),
        }
    }
}

/// Persistence config. Password comes from the env var named by
/// `password_env` — a plain `password:` field is intentionally
/// NOT accepted (DEV-REQUIREMENTS §5.2).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DatabaseCfg {
    /// DSN without credentials, e.g.
    /// `postgres://cronmanager@timescaledb:5432/cronmanager`.
    /// The password from `password_env` is spliced in at pool
    /// construction time.
    pub url: String,

    #[serde(default = "default_password_env")]
    pub password_env: String,
}

fn default_port() -> u16 {
    8080
}
fn default_dsl_path() -> PathBuf {
    PathBuf::from("./DSL")
}
fn default_app_root_path() -> PathBuf {
    PathBuf::from("/app")
}
fn default_max_request_bytes() -> usize {
    1_048_576
}
fn default_max_response_bytes() -> usize {
    16_777_216
}
fn default_request_timeout_secs() -> u64 {
    30
}
fn default_shell_timeout_secs() -> u64 {
    300
}
fn default_password_env() -> String {
    "CRONMANAGER_DB_PASSWORD".to_string()
}
fn default_admin_token_env() -> String {
    "CRONMANAGER_ADMIN_TOKEN".to_string()
}
fn default_true() -> bool {
    true
}
fn default_max_query_params() -> usize {
    64
}
fn default_max_dsl_file_bytes() -> u64 {
    1_048_576
}
fn default_dsl_load_timeout_secs() -> u64 {
    15
}
fn default_max_retry_count() -> u32 {
    10
}
fn default_min_cron_interval_secs() -> i64 {
    10
}
fn default_stored_response_body_max_bytes() -> usize {
    65_536
}
fn default_reload_min_interval_secs() -> u64 {
    60
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            port: default_port(),
            dsl_path: default_dsl_path(),
            app_root_path: default_app_root_path(),
            allowed_origins: Vec::new(),
            shell_environment: BTreeMap::new(),
            limits: Limits::default(),
            admin: AdminConfig::default(),
            security: SecurityConfig::default(),
            database: None,
        }
    }
}

impl AppConfig {
    /// Resolve, load, and return the operator's AppConfig. Falls
    /// back to defaults if no file is found on any conventional
    /// path. Returns `(config, source_path_or_none)`.
    pub fn load_or_default() -> Result<(Self, Option<PathBuf>), CronManagerError> {
        for path in config_search_paths() {
            if path.exists() {
                let body = std::fs::read_to_string(&path)?;
                reject_jvm_wrappers(&body, &path)?;
                let cfg: AppConfig =
                    serde_yaml_ng::from_str(&body).map_err(|e| CronManagerError::YamlParse {
                        path: path.display().to_string(),
                        source: e,
                    })?;
                return Ok((cfg, Some(path)));
            }
        }
        Ok((Self::default(), None))
    }

    /// Compose the sqlx-compatible DSN by splicing the password
    /// from the env var into the configured DSN. Fails if
    /// `database` is set but the env var is missing.
    pub fn database_dsn(&self) -> Result<Option<String>, CronManagerError> {
        let Some(db) = &self.database else {
            return Ok(None);
        };
        let password = std::env::var(&db.password_env).map_err(|_| {
            CronManagerError::Internal(format!(
                "env var {} is unset but database.url is configured",
                db.password_env
            ))
        })?;
        Ok(Some(splice_password(&db.url, &password)))
    }

    /// Read the admin bearer token from the env var named in
    /// `admin.bearer_token_env`. `None` when the env var is unset
    /// or empty. Called once at boot; the resulting `Option<String>`
    /// is stashed in `AppState` for the middleware.
    pub fn resolve_admin_token(&self) -> Option<String> {
        std::env::var(&self.admin.bearer_token_env)
            .ok()
            .filter(|s| !s.is_empty())
    }

    /// Emit one INFO-level diagnostic line per config field that
    /// meaningfully differs from a "fresh install" baseline, and
    /// one WARN per field that is parsed-but-unwired or set to a
    /// value likely to surprise the operator. Called once at boot.
    /// Kept side-effect-free apart from log output so tests can
    /// exercise it against arbitrary configs.
    pub fn boot_diagnostics(&self) {
        tracing::info!(
            "config: port={} dsl_path={} app_root_path={} origins={} shell_env_keys={} history_db={} limits[req={},resp={},http_to={}s,shell_to={}s]",
            self.port,
            self.dsl_path.display(),
            self.app_root_path.display(),
            self.allowed_origins.len(),
            self.shell_environment.len(),
            self.database.is_some(),
            self.limits.max_request_bytes,
            self.limits.max_response_bytes,
            self.limits.request_timeout_secs,
            self.limits.shell_timeout_secs,
        );

        if self.limits.request_timeout_secs == 0 {
            tracing::warn!(
                "config: limits.request_timeout_secs=0 disables the HTTP timeout — a slow upstream can pin the executor indefinitely"
            );
        }
        if self.limits.shell_timeout_secs == 0 {
            tracing::warn!(
                "config: limits.shell_timeout_secs=0 disables the shell wall-clock cap — a runaway process will not be SIGKILLed"
            );
        }
        if self.allowed_origins.iter().any(|o| o == "*") {
            tracing::warn!(
                "config: allowed_origins contains \"*\" — the CORS layer matches exactly and does NOT treat \"*\" as a wildcard; list every allowed origin explicitly"
            );
        }
        if self.database.is_none() {
            tracing::info!(
                "config: history persistence disabled (no `database.url` set); job execution rows are logged at DEBUG only"
            );
        }

        // Security-posture diagnostics — mirror the ones from the
        // limits block so an operator sees the full picture in a
        // single boot dump.
        let token_present = self.resolve_admin_token().is_some();
        tracing::info!(
            "security: admin_token={} trust_network={} block_private_networks={} allow_dangerous_env_overrides={} max_query_params={} reload_min_interval_secs={}",
            token_present,
            self.admin.trust_network,
            self.security.block_private_networks,
            self.security.allow_dangerous_env_overrides,
            self.security.max_query_params,
            self.security.reload_min_interval_secs,
        );
        if !token_present {
            tracing::warn!(
                "security: admin bearer token not configured — state-changing endpoints /execute, /stop, /reload are UNAUTHENTICATED. Set env {} for the token, or admin.trust_network=true if the deployment authenticates at a reverse proxy or service mesh.",
                self.admin.bearer_token_env,
            );
        }
        if self.admin.trust_network {
            tracing::warn!(
                "security: admin.trust_network=true — the boot-time refusal to start on non-loopback binds without a token is DISABLED. Only safe if a reverse proxy / service mesh authenticates every request BEFORE it reaches this process."
            );
        }
        if !self.security.block_private_networks {
            tracing::warn!(
                "security: block_private_networks=false — HTTP jobs may target link-local (169.254.0.0/16), loopback, private (RFC-1918), or ULA addresses. This re-enables the SSRF lane against cloud metadata endpoints."
            );
        }
        if self.security.allow_dangerous_env_overrides {
            tracing::warn!(
                "security: allow_dangerous_env_overrides=true — shell env override blacklist (PATH, LD_*, DYLD_*, PYTHONPATH, NODE_OPTIONS, …) is DISABLED. Any allow-listed dangerous var can be set from /execute query params."
            );
        }
    }
}

/// Emit a helpful error if the operator pasted a JVM
/// `application.yml` — the whole schema lives one level down under
/// `application:` / `spring:` / `management:` / `logging:` in JVM
/// and the flat Rust loader would otherwise reject those with a
/// generic "unknown field" error that doesn't hint at the fix.
fn reject_jvm_wrappers(body: &str, path: &std::path::Path) -> Result<(), CronManagerError> {
    for line in body.lines() {
        // Only inspect unindented, non-comment lines — nested keys
        // of the same name are not JVM wrappers.
        if line.starts_with(char::is_whitespace) || line.starts_with('#') {
            continue;
        }
        for wrapper in ["application:", "spring:", "management:", "logging:"] {
            if line.starts_with(wrapper) {
                return Err(CronManagerError::InvalidConfig {
                    path: path.display().to_string(),
                    reason: format!(
                        "top-level `{wrapper}` is the JVM Spring wrapper. CronManager uses a flat config schema — unwrap the child fields to the top level (e.g. `application.configPath: X` becomes `dsl_path: X` at the top level; `spring.datasource.*` becomes `database.url` + env-var `CRONMANAGER_DB_PASSWORD`)."
                    ),
                });
            }
        }
    }
    Ok(())
}

/// Deserialise `allowed_origins` from either a YAML sequence or a
/// JVM-style comma-separated string. Empty strings and whitespace-
/// only entries are dropped to match JVM's `String.split(",")` +
/// `trim` post-processing.
fn de_origins<'de, D>(d: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }
    let val = OneOrMany::deserialize(d)?;
    let out = match val {
        OneOrMany::One(s) => s
            .split(',')
            .map(|part| part.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect(),
        OneOrMany::Many(list) => list
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect(),
    };
    Ok(out)
}

/// Splice a password into a Postgres URL:
///
/// * `postgres://user@host/db`          → `postgres://user:PASS@host/db`
/// * `postgres://user:x@host/db`        → `postgres://user:PASS@host/db` (existing password replaced)
/// * `postgres://host/db`               → `postgres://:PASS@host/db` (rare — no user)
///
/// The password is percent-encoded so shell-friendly characters
/// like `@` and `/` inside it don't corrupt the URL.
fn splice_password(url: &str, password: &str) -> String {
    let encoded = percent_encode(password);
    // Split into scheme://authority/rest.
    let Some(scheme_end) = url.find("://") else {
        return url.to_string();
    };
    let (scheme, after) = url.split_at(scheme_end + 3);
    let (authority, rest) = match after.find('/') {
        Some(i) => (&after[..i], &after[i..]),
        None => (after, ""),
    };
    let (userinfo, host) = match authority.rfind('@') {
        Some(i) => (&authority[..i], &authority[i + 1..]),
        None => ("", authority),
    };
    let user = match userinfo.find(':') {
        Some(i) => &userinfo[..i],
        None => userinfo,
    };
    let new_authority = if user.is_empty() {
        format!(":{}@{}", encoded, host)
    } else {
        format!("{}:{}@{}", user, encoded, host)
    };
    format!("{}{}{}", scheme, new_authority, rest)
}

/// Percent-encode a Postgres password: encode everything that is
/// not an unreserved URL character (RFC 3986). Conservative on
/// purpose — anything that isn't [A-Za-z0-9._~-] gets escaped.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

/// Classify a listen host string as loopback / non-loopback. Used
/// by the boot-time refuse-to-start check: a non-loopback bind
/// with no admin token AND `trust_network=false` is refused with a
/// diagnostic that names the env var and the address. Anything the
/// parser can't classify is treated as non-loopback (fail-safe).
pub fn bind_is_loopback(bind: &str) -> bool {
    let host = if let Some(rest) = bind.strip_prefix('[') {
        // IPv6 literal in `[::1]:8080` shape.
        rest.split(']').next().unwrap_or("").to_string()
    } else {
        // Take everything before the last `:` — leaves the host
        // part intact when the bind is `host:port`. Bare host
        // (no port) is used as-is.
        match bind.rsplit_once(':') {
            Some((h, _p)) => h.to_string(),
            None => bind.to_string(),
        }
    };
    if host.is_empty() || host == "*" {
        return false;
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return ip.is_loopback();
    }
    matches!(host.as_str(), "localhost")
}

fn config_search_paths() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--config" && i + 1 < args.len() {
            out.push(PathBuf::from(&args[i + 1]));
            break;
        }
        if let Some(rest) = args[i].strip_prefix("--config=") {
            out.push(PathBuf::from(rest));
            break;
        }
        i += 1;
    }
    if let Ok(p) = std::env::var("CRONMANAGER_CONFIG") {
        if !p.is_empty() {
            out.push(PathBuf::from(p));
        }
    }
    out.push(PathBuf::from("./cronmanager.yaml"));
    out.push(PathBuf::from("./cronmanager.yml"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_reasonable() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.port, 8080);
        assert_eq!(cfg.limits.request_timeout_secs, 30);
        assert_eq!(cfg.limits.shell_timeout_secs, 300);
        assert!(cfg.database.is_none());
    }

    #[test]
    fn splice_password_appends_when_userinfo_lacks_one() {
        let out = splice_password(
            "postgres://cronmanager@timescaledb:5432/cronmanager",
            "secret",
        );
        assert_eq!(
            out,
            "postgres://cronmanager:secret@timescaledb:5432/cronmanager"
        );
    }

    #[test]
    fn splice_password_replaces_existing_one() {
        let out = splice_password(
            "postgres://cronmanager:old@timescaledb:5432/cronmanager",
            "new",
        );
        assert_eq!(
            out,
            "postgres://cronmanager:new@timescaledb:5432/cronmanager"
        );
    }

    #[test]
    fn splice_password_url_encodes_special_chars() {
        let out = splice_password("postgres://u@h/d", "p@ss/word");
        assert_eq!(out, "postgres://u:p%40ss%2Fword@h/d");
    }

    #[test]
    fn splice_password_handles_url_without_userinfo() {
        let out = splice_password("postgres://h:5432/d", "secret");
        assert_eq!(out, "postgres://:secret@h:5432/d");
    }

    #[test]
    fn database_dsn_needs_env_var() {
        let cfg = AppConfig {
            database: Some(DatabaseCfg {
                url: "postgres://u@h/d".into(),
                password_env: "CRONMANAGER_TEST_MISSING_ENV".into(),
            }),
            ..AppConfig::default()
        };
        // Ensure the env var really is unset.
        std::env::remove_var("CRONMANAGER_TEST_MISSING_ENV");
        assert!(cfg.database_dsn().is_err());
    }

    #[test]
    fn database_dsn_none_when_no_db_configured() {
        let cfg = AppConfig::default();
        assert!(cfg.database_dsn().unwrap().is_none());
    }

    #[test]
    fn unknown_top_level_field_is_hard_error() {
        // R2.2: a typo like `dslpath:` must fail loudly.
        let yaml = "dslpath: DSL\n";
        let err = serde_yaml_ng::from_str::<AppConfig>(yaml).unwrap_err();
        // serde error should mention the offending key.
        let msg = err.to_string();
        assert!(msg.contains("dslpath"), "error was: {msg}");
    }

    #[test]
    fn jvm_camelcase_aliases_still_bind() {
        // R2.4: preserve JVM field aliases for a copy-paste port.
        let yaml = "configPath: /var/lib/cron\nappRootPath: /app\nallowedOrigins: [x, y]\nshellEnvironment: {A: '1'}\n";
        let cfg: AppConfig = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(cfg.dsl_path, PathBuf::from("/var/lib/cron"));
        assert_eq!(cfg.app_root_path, PathBuf::from("/app"));
        assert_eq!(cfg.allowed_origins, vec!["x", "y"]);
        assert!(cfg.shell_environment.contains_key("A"));
    }

    #[test]
    fn allowed_origins_accepts_jvm_comma_string() {
        // JVM shipped this as a String, not a list.
        let yaml = "allowed_origins: \"localhost,192.168.10.1,127.0.0.1\"\n";
        let cfg: AppConfig = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(
            cfg.allowed_origins,
            vec!["localhost", "192.168.10.1", "127.0.0.1"]
        );
    }

    #[test]
    fn allowed_origins_string_drops_empty_segments() {
        let yaml = "allowed_origins: \" , a , ,b, \"\n";
        let cfg: AppConfig = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(cfg.allowed_origins, vec!["a", "b"]);
    }

    #[test]
    fn top_level_application_wrapper_is_rejected_with_hint() {
        // Reject the JVM Spring wrapper form with a diagnostic
        // that names the wrapper AND shows the fix inline.
        let yaml = "application:\n  configPath: DSL/samples\n";
        let err = reject_jvm_wrappers(yaml, std::path::Path::new("test.yaml")).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("application"), "err was: {msg}");
        assert!(
            msg.contains("flat config schema") || msg.contains("dsl_path"),
            "err was: {msg}"
        );
    }

    #[test]
    fn top_level_spring_wrapper_is_rejected() {
        let yaml = "spring:\n  datasource:\n    url: x\n";
        assert!(reject_jvm_wrappers(yaml, std::path::Path::new("t.yaml")).is_err());
    }

    #[test]
    fn top_level_management_wrapper_is_rejected() {
        let yaml = "management:\n  endpoints: {}\n";
        assert!(reject_jvm_wrappers(yaml, std::path::Path::new("t.yaml")).is_err());
    }

    #[test]
    fn nested_application_key_is_not_rejected() {
        // Only top-level wrappers are the failure mode we care
        // about — a nested `application:` inside a legit block is
        // fine (imaginary but the guard mustn't fire on it).
        let yaml = "shell_environment:\n  application: legit\n";
        assert!(reject_jvm_wrappers(yaml, std::path::Path::new("t.yaml")).is_ok());
    }

    #[test]
    fn unknown_limits_field_is_hard_error() {
        let yaml = "limits:\n  max_response_bytes: 100\n  banana: 1\n";
        let err = serde_yaml_ng::from_str::<AppConfig>(yaml).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("banana"), "err was: {msg}");
    }

    #[test]
    fn unknown_database_field_is_hard_error() {
        let yaml = "database:\n  url: postgres://x\n  password: nope\n";
        let err = serde_yaml_ng::from_str::<AppConfig>(yaml).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("password"), "err was: {msg}");
    }

    #[test]
    fn boot_diagnostics_does_not_panic() {
        // Smoke: exercise every warn branch to catch fmt regressions.
        let mut cfg = AppConfig::default();
        cfg.allowed_origins.push("*".into());
        cfg.limits.request_timeout_secs = 0;
        cfg.limits.shell_timeout_secs = 0;
        cfg.security.block_private_networks = false;
        cfg.security.allow_dangerous_env_overrides = true;
        cfg.admin.trust_network = true;
        cfg.boot_diagnostics();
    }

    #[test]
    fn security_defaults_are_safe() {
        // Regression pin: SSRF block and dangerous-env blacklist
        // both on by default, so a zero-config deploy is safe.
        let cfg = AppConfig::default();
        assert!(cfg.security.block_private_networks);
        assert!(!cfg.security.allow_dangerous_env_overrides);
        assert_eq!(cfg.security.max_query_params, 64);
        assert_eq!(cfg.security.max_dsl_file_bytes, 1_048_576);
        assert_eq!(cfg.security.max_retry_count, 10);
        assert_eq!(cfg.security.reload_min_interval_secs, 60);
    }

    #[test]
    fn admin_defaults_use_conventional_env_var() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.admin.bearer_token_env, "CRONMANAGER_ADMIN_TOKEN");
        assert!(!cfg.admin.trust_network);
    }

    #[test]
    fn bind_is_loopback_recognises_ipv4_loopback() {
        assert!(bind_is_loopback("127.0.0.1:8080"));
        assert!(bind_is_loopback("127.1.2.3:8080"));
    }

    #[test]
    fn bind_is_loopback_recognises_ipv6_loopback() {
        assert!(bind_is_loopback("[::1]:8080"));
    }

    #[test]
    fn bind_is_loopback_accepts_localhost() {
        assert!(bind_is_loopback("localhost:8080"));
    }

    #[test]
    fn bind_is_loopback_rejects_wildcard() {
        assert!(!bind_is_loopback("0.0.0.0:8080"));
        assert!(!bind_is_loopback("[::]:8080"));
        assert!(!bind_is_loopback("*:8080"));
    }

    #[test]
    fn bind_is_loopback_rejects_public_ip() {
        assert!(!bind_is_loopback("10.0.0.1:8080"));
        assert!(!bind_is_loopback("1.2.3.4:8080"));
    }

    #[test]
    fn resolve_admin_token_reads_env_var() {
        // Random name so parallel-test runs don't collide on the
        // process-wide env table.
        let key = format!("CRONMANAGER_TEST_ADMIN_{}", std::process::id());
        let cfg = AppConfig {
            admin: AdminConfig {
                bearer_token_env: key.clone(),
                trust_network: false,
            },
            ..AppConfig::default()
        };
        std::env::remove_var(&key);
        assert!(cfg.resolve_admin_token().is_none());
        std::env::set_var(&key, "");
        assert!(
            cfg.resolve_admin_token().is_none(),
            "empty env must be treated as unset"
        );
        std::env::set_var(&key, "s3cret");
        assert_eq!(cfg.resolve_admin_token().as_deref(), Some("s3cret"));
        std::env::remove_var(&key);
    }
}

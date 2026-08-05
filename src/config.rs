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

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            port: default_port(),
            dsl_path: default_dsl_path(),
            app_root_path: default_app_root_path(),
            allowed_origins: Vec::new(),
            shell_environment: BTreeMap::new(),
            limits: Limits::default(),
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
        cfg.boot_diagnostics();
    }
}

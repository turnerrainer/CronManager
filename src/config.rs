//! CronManager runtime configuration.
//!
//! Wire shape (`cronmanager.yaml`): see book/src/configuration.md.
//!
//! Search order for the file:
//! 1. `--config <path>` CLI flag
//! 2. `CRONMANAGER_CONFIG` env var
//! 3. `./cronmanager.yaml` or `./cronmanager.yml`
//! 4. Built-in defaults if nothing is found

use crate::error::CronManagerError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AppConfig {
    #[serde(default = "default_port")]
    pub port: u16,

    #[serde(default = "default_dsl_path")]
    pub dsl_path: PathBuf,

    #[serde(default = "default_app_root_path")]
    pub app_root_path: PathBuf,

    /// Empty list disables the CORS layer entirely.
    #[serde(default)]
    pub allowed_origins: Vec<String>,

    /// Baseline environment map for shell jobs. Each job still
    /// whitelists specific keys via `allowedEnvs`.
    #[serde(default)]
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
}

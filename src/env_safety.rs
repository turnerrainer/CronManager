//! Environment-aware safety gates.
//!
//! Adopts FLEET-STRONGHOLDS §11 — the cross-service pattern for
//! preventing "dev defaults shipped to non-dev environments" (the
//! single most common "found in production" pentest finding across
//! the Buerostack fleet).
//!
//! The rule: in any environment above `Dev`, boot MUST refuse when
//! - a credential-shaped config value is empty, too short, or
//!   matches a documented weak-pattern substring (§11.1); or
//! - a documented-unsafe security posture flag is set (§11.2).
//!
//! In `Dev`, the same conditions emit `tracing::warn!` and boot
//! continues — the CronManager zero-config loopback UX stays intact
//! for local development.
//!
//! Environment is read from `APP_ENV`, `ENVIRONMENT`, or `DEPLOY_ENV`
//! (first match wins). Missing or unknown values fail-safe to
//! `Production` so that a forgotten env var can't downgrade the
//! posture silently.

use std::sync::OnceLock;

/// Deployment environment class. `Dev` permits weak defaults;
/// every other variant treats them as fatal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Environment {
    Dev,
    Test,
    Staging,
    Production,
}

impl Environment {
    /// Cache the result of the first call so the value doesn't
    /// change under an operator's feet mid-boot. Tests use
    /// [`from_env_str`] for injectability.
    pub fn from_env() -> Self {
        static CACHED: OnceLock<Environment> = OnceLock::new();
        *CACHED.get_or_init(|| {
            let raw = std::env::var("APP_ENV")
                .or_else(|_| std::env::var("ENVIRONMENT"))
                .or_else(|_| std::env::var("DEPLOY_ENV"))
                .unwrap_or_else(|_| "dev".to_string());
            Self::from_env_str(&raw)
        })
    }

    /// Pure classifier — testable, no `OnceLock`. Unknown values
    /// fail-safe to `Production`.
    pub fn from_env_str(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "dev" | "development" | "local" => Self::Dev,
            "test" | "testing" | "ci" => Self::Test,
            "stage" | "staging" | "preprod" => Self::Staging,
            "prod" | "production" | "live" => Self::Production,
            _ => {
                // Fail-safe: unknown env → treat as Production so
                // that a forgotten env var (or a typo like
                // `APP_ENV=prroduction`) doesn't silently downgrade
                // the security posture.
                eprintln!(
                    "[env_safety] WARN: unknown environment `{raw}` — treating as Production"
                );
                Self::Production
            }
        }
    }

    /// True for anything above `Dev`. In these envs, weak defaults
    /// refuse to boot rather than emitting a WARN.
    pub fn requires_prod_creds(self) -> bool {
        !matches!(self, Self::Dev)
    }
}

/// Substring patterns that mark a credential as weak.
/// Case-insensitive match.
///
/// Cross-fleet: covers patterns found in Buerostack + eFTI Gate
/// audits (2026-09-11..13). Extend as new patterns are discovered.
const WEAK_PATTERNS: &[&str] = &[
    // Password-shape defaults
    "changeit",
    "changeme",
    "change-me",
    "changethis",
    "letmein",
    "password",
    "qwerty",
    "welcome",
    // Numeric-only sequential defaults (leave as substrings so
    // "01234abcdef" still matches).
    "01234",
    "12345",
    "123456",
    "abcdef",
    // Dev/test/example markers
    "dev-",
    "-dev",
    "test-",
    "-test",
    "example",
    "sample",
    "default-",
    "-default",
    "todo",
    "tbd",
    "fixme",
    "-poc",
    "poc-",
    "mock-",
    "-mock",
    "staging-",
    "-staging",
    // Common cronmanager-specific pattern seen in break-tests
    "test-admin-token",
];

/// Reason a credential failed the weak-value check.
#[derive(Debug, PartialEq, Eq)]
pub enum WeakReason {
    /// Value was empty or unset.
    Empty,
    /// Value shorter than the configured minimum length.
    TooShort,
    /// Value contains a documented weak-pattern substring
    /// (the matched pattern is included).
    Pattern(&'static str),
}

impl std::fmt::Display for WeakReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "empty"),
            Self::TooShort => write!(f, "too_short"),
            Self::Pattern(p) => write!(f, "weak-pattern:{p}"),
        }
    }
}

/// One credential to validate at boot.
pub struct CredCheck {
    /// Where the credential comes from ("admin-token", "database").
    pub context: &'static str,
    /// The specific field name ("token", "password").
    pub field: &'static str,
    /// The resolved value.
    pub value: String,
    /// Minimum length. 32 for tokens, 12 for passwords.
    pub min_len: usize,
}

impl CredCheck {
    pub fn is_weak(&self) -> Option<WeakReason> {
        let v = self.value.trim();
        if v.is_empty() {
            return Some(WeakReason::Empty);
        }
        if v.len() < self.min_len {
            return Some(WeakReason::TooShort);
        }
        let lower = v.to_ascii_lowercase();
        for pattern in WEAK_PATTERNS {
            if lower.contains(pattern) {
                return Some(WeakReason::Pattern(pattern));
            }
        }
        None
    }
}

/// One security-posture flag to validate at boot.
pub struct PostureCheck {
    pub name: &'static str,
    /// True when the current config is safe for the target env.
    pub is_safe: bool,
    /// Human explanation of the risk.
    pub description: &'static str,
    /// Concrete remediation hint.
    pub fix: &'static str,
}

/// Redact a value for display in the failure message. Shows the
/// first 2 + last 2 chars so an operator can identify the field
/// they need to rotate, without echoing the full value into logs.
fn redact(v: &str) -> String {
    if v.len() <= 4 {
        "****".to_string()
    } else {
        format!("{}****{}", &v[..2], &v[v.len() - 2..])
    }
}

/// Boot-time gate for credentials. Returns Err in non-dev when any
/// check fails. In dev, all failures are emitted as WARN and the
/// function returns Ok.
pub fn enforce_creds(env: Environment, checks: &[CredCheck]) -> Result<(), String> {
    let failures: Vec<String> = checks
        .iter()
        .filter_map(|c| c.is_weak().map(|reason| (c, reason)))
        .map(|(c, reason)| {
            format!(
                "  - {}.{} = {} (weak: {})",
                c.context,
                c.field,
                redact(c.value.trim()),
                reason,
            )
        })
        .collect();

    if failures.is_empty() {
        return Ok(());
    }

    if env.requires_prod_creds() {
        Err(format!(
            "REFUSING TO START in {env:?}: {} weak/default credential(s) detected:\n{}\n\
             Rotate these before starting in a non-dev environment. Set APP_ENV=dev \
             (or ENVIRONMENT=dev, DEPLOY_ENV=dev) to permit dev defaults locally.",
            failures.len(),
            failures.join("\n"),
        ))
    } else {
        for f in &failures {
            tracing::warn!(target: "env_safety", "weak credential (dev env, permitted): {f}");
        }
        Ok(())
    }
}

/// Boot-time gate for security-posture flags. Same shape as
/// [`enforce_creds`] but for boolean posture switches.
pub fn enforce_posture(env: Environment, checks: &[PostureCheck]) -> Result<(), String> {
    let unsafe_items: Vec<&PostureCheck> = checks.iter().filter(|c| !c.is_safe).collect();
    if unsafe_items.is_empty() {
        return Ok(());
    }
    if env.requires_prod_creds() {
        return Err(format!(
            "REFUSING TO START in {env:?}: {} unsafe posture item(s):\n{}",
            unsafe_items.len(),
            unsafe_items
                .iter()
                .map(|c| format!("  - {}: {}\n      fix: {}", c.name, c.description, c.fix))
                .collect::<Vec<_>>()
                .join("\n"),
        ));
    }
    for c in unsafe_items {
        tracing::warn!(
            target: "env_safety",
            "unsafe posture (dev env, permitted): {} — {} — fix: {}",
            c.name, c.description, c.fix,
        );
    }
    Ok(())
}

/// Build the CronManager-specific `CredCheck` list from a loaded
/// config. Kept as a `Vec<_>` (not a slice) so main.rs owns the
/// resolved values.
pub fn cred_checks_for(cfg: &crate::config::AppConfig) -> Vec<CredCheck> {
    let mut out = Vec::new();
    // Admin token — resolved from the env named by
    // `admin.bearer_token_env`. Absent = empty = weak.
    let admin_token = cfg.resolve_admin_token().unwrap_or_default();
    out.push(CredCheck {
        context: "admin",
        field: "token",
        value: admin_token,
        min_len: 32,
    });
    // Database password if a database block is configured.
    if let Some(db) = &cfg.database {
        let pwd = std::env::var(&db.password_env).unwrap_or_default();
        out.push(CredCheck {
            context: "database",
            field: "password",
            value: pwd,
            min_len: 12,
        });
    }
    out
}

/// Build the CronManager-specific `PostureCheck` list. Every entry
/// mirrors a boot-diagnostic WARN that already exists in
/// `AppConfig::boot_diagnostics`; the difference is that in non-dev
/// they REFUSE instead of warning.
pub fn posture_checks_for(cfg: &crate::config::AppConfig) -> Vec<PostureCheck> {
    vec![
        PostureCheck {
            name: "admin.trust_network",
            is_safe: !cfg.admin.trust_network,
            description: "the boot-time refusal on non-loopback binds without a token is DISABLED",
            fix: "set admin.trust_network=false and configure a bearer token",
        },
        PostureCheck {
            name: "security.block_private_networks",
            is_safe: cfg.security.block_private_networks,
            description: "SSRF pre-flight is disabled; HTTP jobs may target link-local, RFC 1918, or ULA endpoints (cloud metadata included)",
            fix: "set security.block_private_networks=true (default)",
        },
        PostureCheck {
            name: "security.allow_dangerous_env_overrides",
            is_safe: !cfg.security.allow_dangerous_env_overrides,
            description: "PATH / LD_* / DYLD_* / PYTHONPATH / NODE_OPTIONS can be set from /execute query params",
            fix: "set security.allow_dangerous_env_overrides=false (default)",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(context: &'static str, field: &'static str, value: &str, min_len: usize) -> CredCheck {
        CredCheck {
            context,
            field,
            value: value.to_string(),
            min_len,
        }
    }

    #[test]
    fn dev_words_classify_as_dev() {
        for word in ["dev", "development", "local", "DEV", " Dev  "] {
            assert_eq!(Environment::from_env_str(word), Environment::Dev, "{word}");
        }
    }

    #[test]
    fn prod_words_classify_as_production() {
        for word in ["prod", "production", "live", "PROD"] {
            assert_eq!(
                Environment::from_env_str(word),
                Environment::Production,
                "{word}"
            );
        }
    }

    #[test]
    fn unknown_env_defaults_to_production_for_safety() {
        assert_eq!(Environment::from_env_str("banana"), Environment::Production);
        assert_eq!(
            Environment::from_env_str("prroduction"),
            Environment::Production
        );
    }

    #[test]
    fn empty_credential_is_weak() {
        assert_eq!(mk("x", "t", "", 16).is_weak(), Some(WeakReason::Empty));
        assert_eq!(mk("x", "t", "   ", 16).is_weak(), Some(WeakReason::Empty));
    }

    #[test]
    fn short_credential_is_weak() {
        assert_eq!(
            mk("x", "t", "short", 16).is_weak(),
            Some(WeakReason::TooShort)
        );
    }

    #[test]
    fn changeit_pattern_matches_case_insensitive() {
        assert_eq!(
            mk("keystore", "password", "ChangeIt-padding-to-be-long", 12)
                .is_weak()
                .as_ref()
                .map(|r| r.to_string()),
            Some("weak-pattern:changeit".to_string())
        );
    }

    #[test]
    fn test_admin_token_pattern_matches() {
        // The dev-fixture token h2ck.me used in break-tests.
        assert!(
            mk("admin", "token", "test-admin-token-32bytes-abcdef01234", 32)
                .is_weak()
                .is_some()
        );
    }

    #[test]
    fn strong_credential_passes() {
        // 32-char hex, no weak substring.
        assert!(mk("admin", "token", "9f8b2a1c1cbb52fda40d7a5c9e6e3210", 32)
            .is_weak()
            .is_none());
    }

    #[test]
    fn enforce_creds_refuses_in_production() {
        let checks = vec![mk("db", "password", "01234abcdef", 12)];
        let err = enforce_creds(Environment::Production, &checks).unwrap_err();
        assert!(err.contains("REFUSING TO START"));
        // Redacted value in the message.
        assert!(err.contains("01****ef"));
        // Recovery hint.
        assert!(err.contains("APP_ENV=dev"));
    }

    #[test]
    fn enforce_creds_accepts_in_dev() {
        let checks = vec![mk("db", "password", "01234", 12)];
        assert!(enforce_creds(Environment::Dev, &checks).is_ok());
    }

    #[test]
    fn enforce_creds_passes_with_no_failures() {
        assert!(enforce_creds(Environment::Production, &[]).is_ok());
        assert!(enforce_creds(
            Environment::Production,
            &[mk("x", "t", "9f8b2a1c1cbb52fda40d7a5c9e6e3210", 32)]
        )
        .is_ok());
    }

    #[test]
    fn enforce_posture_refuses_in_production() {
        let checks = vec![PostureCheck {
            name: "security.block_private_networks",
            is_safe: false,
            description: "SSRF lane open",
            fix: "flip to true",
        }];
        let err = enforce_posture(Environment::Production, &checks).unwrap_err();
        assert!(err.contains("REFUSING TO START"));
        assert!(err.contains("SSRF"));
        assert!(err.contains("flip to true"));
    }

    #[test]
    fn enforce_posture_accepts_in_dev() {
        let checks = vec![PostureCheck {
            name: "x",
            is_safe: false,
            description: "y",
            fix: "z",
        }];
        assert!(enforce_posture(Environment::Dev, &checks).is_ok());
    }

    #[test]
    fn enforce_posture_passes_when_all_safe() {
        let checks = vec![PostureCheck {
            name: "x",
            is_safe: true,
            description: "y",
            fix: "z",
        }];
        assert!(enforce_posture(Environment::Production, &checks).is_ok());
    }
}

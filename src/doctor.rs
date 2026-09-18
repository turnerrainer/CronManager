//! `cronmanager doctor` — offline configuration audit.
//!
//! h2ck.me v1 T-19 / FLEET-STRONGHOLDS §8.2 (adoption of the XTR
//! `doctor` pattern). Reads the config the process would boot with,
//! runs every check that boot itself would run, and prints one
//! severity-prefixed line per finding. Exits non-zero on FATAL so it
//! can be wired into a container `HEALTHCHECK` or a pre-deploy CI
//! gate.
//!
//! Severity ladder (aligned with FLEET-STRONGHOLDS §8.2):
//!
//! - `FATAL`: refuse-to-start conditions in the target env.
//!   `cronmanager doctor` exits 1 if any are present.
//! - `BREAK`: the process WILL fail at runtime even if it starts
//!   (missing env var referenced by config, unreachable DSL directory).
//! - `WEAK`: insecure default that boot only WARNs about.
//! - `INFO`: descriptive facts (bind, dsl_path, N jobs).
//!
//! The renderer is intentionally plain text — SIEM-friendly and
//! grep-friendly. `doctor` never prints secret material; credential
//! values are always redacted via `env_safety::redact`.

use crate::config::AppConfig;
use crate::env_safety::{self, Environment};

/// Severity ladder in enum form. Ordering matters: any `Fatal` in
/// the report forces a non-zero exit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Weak,
    Break,
    Fatal,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Self::Fatal => "FATAL",
            Self::Break => "BREAK",
            Self::Weak => "WEAK ",
            Self::Info => "INFO ",
        }
    }
}

/// One finding produced by a check.
#[derive(Debug, Clone)]
pub struct Finding {
    pub severity: Severity,
    /// Short check name, e.g. `admin.token`, `refuse_to_start`.
    pub name: &'static str,
    /// One-line summary rendered after the severity label.
    pub message: String,
}

impl Finding {
    fn info(name: &'static str, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Info,
            name,
            message: message.into(),
        }
    }
    fn weak(name: &'static str, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Weak,
            name,
            message: message.into(),
        }
    }
    fn brk(name: &'static str, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Break,
            name,
            message: message.into(),
        }
    }
    fn fatal(name: &'static str, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Fatal,
            name,
            message: message.into(),
        }
    }
}

/// Full doctor result. `render` produces the multi-line text output;
/// tests assert against the `findings` list to avoid coupling to the
/// exact rendering.
#[derive(Debug, Clone)]
pub struct DoctorReport {
    pub env: Environment,
    pub findings: Vec<Finding>,
}

impl DoctorReport {
    pub fn fatal_count(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.severity == Severity::Fatal)
            .count()
    }

    /// The exit code `cronmanager doctor` should return. Any
    /// `Fatal` → 1; everything else → 0.
    pub fn exit_code(&self) -> i32 {
        if self.fatal_count() > 0 {
            1
        } else {
            0
        }
    }

    /// Render the report as newline-separated `LEVEL name: message`
    /// lines followed by a one-line summary. Not indented — the
    /// prefix is fixed-width for easy `awk`-style filtering.
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "cronmanager doctor — {} environment\n",
            format!("{:?}", self.env).to_lowercase(),
        ));
        for f in &self.findings {
            out.push_str(&format!(
                "[{}] {}: {}\n",
                f.severity.label(),
                f.name,
                f.message
            ));
        }
        let fatal = self.fatal_count();
        let breaks = self
            .findings
            .iter()
            .filter(|f| f.severity == Severity::Break)
            .count();
        let weaks = self
            .findings
            .iter()
            .filter(|f| f.severity == Severity::Weak)
            .count();
        let infos = self
            .findings
            .iter()
            .filter(|f| f.severity == Severity::Info)
            .count();
        out.push_str(&format!(
            "summary: {fatal} fatal, {breaks} break, {weaks} weak, {infos} info\n"
        ));
        out
    }
}

/// Run every check against the given config. Pure function — no
/// process-wide side effects beyond reading env vars via the same
/// paths `boot_diagnostics` uses.
pub fn run(cfg: &AppConfig, env: Environment) -> DoctorReport {
    let mut findings: Vec<Finding> = Vec::new();

    // ---------- INFO block ----------
    findings.push(Finding::info("port", format!("bind port {}", cfg.port)));
    if crate::executor::ExecutorBundle::offline_from_env() {
        findings.push(Finding::info(
            "offline_mode",
            "CRONMANAGER_OFFLINE=true — every dispatch will be stubbed to status=OFFLINE",
        ));
    }
    findings.push(Finding::info(
        "dsl_path",
        format!("DSL directory: {}", cfg.dsl_path.display()),
    ));
    findings.push(Finding::info(
        "history",
        if cfg.database.is_some() {
            "database persistence configured".to_string()
        } else {
            "history persistence disabled (NoopRecorder)".to_string()
        },
    ));
    let admin_token_present = cfg.resolve_admin_token().is_some();
    findings.push(Finding::info(
        "admin.token",
        format!(
            "admin bearer token {}",
            if admin_token_present {
                "resolved from env"
            } else {
                "NOT set"
            }
        ),
    ));

    // ---------- BREAK block ----------
    if !cfg.dsl_path.exists() {
        findings.push(Finding::brk(
            "dsl_path",
            format!(
                "DSL directory does not exist: {} — scheduler will boot with 0 jobs",
                cfg.dsl_path.display(),
            ),
        ));
    } else if !cfg.dsl_path.is_dir() {
        findings.push(Finding::brk(
            "dsl_path",
            format!("DSL path is not a directory: {}", cfg.dsl_path.display(),),
        ));
    }

    if let Some(db) = &cfg.database {
        match std::env::var(&db.password_env) {
            Ok(v) if v.is_empty() => findings.push(Finding::brk(
                "database.password_env",
                format!(
                    "env var `{}` is empty; database.url is configured",
                    db.password_env
                ),
            )),
            Err(_) => findings.push(Finding::brk(
                "database.password_env",
                format!(
                    "env var `{}` is not set; database.url is configured",
                    db.password_env
                ),
            )),
            _ => {}
        }
    }

    // Refuse-to-start replay — behaves the same as router::refuse_to_start_without_token
    // for a wildcard bind on `0.0.0.0:cfg.port`.
    let bind = format!("0.0.0.0:{}", cfg.port);
    if crate::router::refuse_to_start_without_token(cfg, &bind, admin_token_present).is_err() {
        findings.push(Finding::fatal(
            "refuse_to_start",
            format!(
                "process would REFUSE to boot: non-loopback bind {} without an admin token and admin.trust_network=false",
                bind,
            ),
        ));
    }

    // ---------- WEAK block ----------
    if cfg.limits.request_timeout_secs == 0 {
        findings.push(Finding::weak(
            "limits.request_timeout_secs",
            "request timeout disabled (0) — slow upstream can pin the executor",
        ));
    }
    if cfg.limits.shell_timeout_secs == 0 {
        findings.push(Finding::weak(
            "limits.shell_timeout_secs",
            "shell timeout disabled (0) — runaway process will not be SIGKILLed",
        ));
    }
    if cfg.allowed_origins.iter().any(|o| o == "*") {
        findings.push(Finding::weak(
            "allowed_origins",
            "`*` is treated as an exact origin string, not a wildcard — CORS will not match browsers",
        ));
    }
    if !admin_token_present {
        findings.push(Finding::weak(
            "admin.token",
            format!(
                "admin bearer token not configured — set env {} for the token, or admin.trust_network=true if a proxy authenticates every request",
                cfg.admin.bearer_token_env,
            ),
        ));
    }
    if cfg.admin.trust_network {
        findings.push(Finding::weak(
            "admin.trust_network",
            "trust_network=true — the loopback-only boot refusal is DISABLED; only safe behind an authenticating proxy",
        ));
    }
    if !cfg.security.block_private_networks {
        findings.push(Finding::weak(
            "security.block_private_networks",
            "block_private_networks=false — HTTP jobs can reach cloud metadata endpoints",
        ));
    }
    if cfg.security.allow_dangerous_env_overrides {
        findings.push(Finding::weak(
            "security.allow_dangerous_env_overrides",
            "allow_dangerous_env_overrides=true — PATH / LD_* / PYTHONPATH may be set from /execute query params",
        ));
    }
    if cfg.security.expose_jobs_publicly && admin_token_present {
        findings.push(Finding::weak(
            "security.expose_jobs_publicly",
            "GET /jobs is anonymous even though an admin token is configured",
        ));
    }
    if cfg.security.expose_running_publicly && admin_token_present {
        findings.push(Finding::weak(
            "security.expose_running_publicly",
            "GET /running is anonymous even though an admin token is configured",
        ));
    }
    for (group, env_var) in &cfg.security.per_group_token_envs {
        match std::env::var(env_var) {
            Ok(v) if !v.is_empty() => {}
            _ => findings.push(Finding::weak(
                "security.per_group_token_envs",
                format!(
                    "group {group}: env var {env_var} is unset or empty — falls back to master-only",
                ),
            )),
        }
    }

    // ---------- FATAL block (env-safety replay) ----------
    if let Err(msg) = env_safety::enforce_creds(env, &env_safety::cred_checks_for(cfg)) {
        findings.push(Finding::fatal("env_safety.creds", msg));
    }
    if let Err(msg) = env_safety::enforce_posture(env, &env_safety::posture_checks_for(cfg)) {
        findings.push(Finding::fatal("env_safety.posture", msg));
    }
    if cfg.security.allow_dangerous_env_overrides && env.requires_prod_creds() {
        // Redundant with env_safety.posture in prod, but the posture
        // list can be trimmed independently — belt and braces.
        findings.push(Finding::fatal(
            "security.allow_dangerous_env_overrides",
            "allow_dangerous_env_overrides=true in a non-dev environment",
        ));
    }

    DoctorReport { env, findings }
}

/// CLI entry point used by `cronmanager doctor`. Loads the same
/// config the boot path would, runs `run`, prints the report to
/// stdout, and returns the exit code.
///
/// Returns an anyhow::Result so config-load errors surface as
/// process exit 1 with the error message (main.rs converts).
pub fn cli_entry() -> anyhow::Result<i32> {
    let (cfg, cfg_source) = AppConfig::load_or_default()?;
    match cfg_source {
        Some(p) => println!("loaded config from {}", p.display()),
        None => println!("using built-in defaults (no cronmanager.yaml found)"),
    }
    let env = Environment::from_env();
    let report = run(&cfg, env);
    print!("{}", report.render());
    Ok(report.exit_code())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AdminConfig;
    use std::path::PathBuf;

    fn cfg_with_dsl(tmp: &tempfile::TempDir) -> AppConfig {
        AppConfig {
            dsl_path: tmp.path().to_path_buf(),
            ..AppConfig::default()
        }
    }

    #[test]
    fn dev_env_default_bind_without_token_reports_refuse_to_start_fatal() {
        // main.rs binds `0.0.0.0:cfg.port`. Without an admin token
        // that bind is refused at boot. Doctor must FATAL so the
        // operator gets the same signal before shipping.
        let tmp = tempfile::tempdir().unwrap();
        let cfg = cfg_with_dsl(&tmp);
        let r = run(&cfg, Environment::Dev);
        assert!(
            r.findings
                .iter()
                .any(|f| f.severity == Severity::Fatal && f.name == "refuse_to_start"),
            "expected refuse_to_start FATAL in default config: {:#?}",
            r.findings,
        );
        assert_eq!(r.exit_code(), 1);
    }

    #[test]
    fn dev_env_with_trust_network_true_clears_refuse_to_start_fatal() {
        // Operators behind an authenticating proxy legitimately
        // opt out with admin.trust_network=true. Doctor must not
        // then report the refuse_to_start FATAL — but should still
        // list trust_network=true as a WEAK.
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = cfg_with_dsl(&tmp);
        cfg.admin.trust_network = true;
        let r = run(&cfg, Environment::Dev);
        assert!(
            !r.findings.iter().any(|f| f.name == "refuse_to_start"),
            "refuse_to_start should have cleared when trust_network=true: {:#?}",
            r.findings,
        );
        assert_eq!(r.fatal_count(), 0);
        assert!(r
            .findings
            .iter()
            .any(|f| f.severity == Severity::Weak && f.name == "admin.trust_network"));
    }

    #[test]
    fn missing_dsl_directory_reports_break() {
        let cfg = AppConfig {
            dsl_path: PathBuf::from("/nonexistent/cronmanager-doctor-test"),
            ..AppConfig::default()
        };
        let r = run(&cfg, Environment::Dev);
        assert!(
            r.findings
                .iter()
                .any(|f| f.severity == Severity::Break && f.name == "dsl_path"),
            "missing dsl_path must be a BREAK: {:#?}",
            r.findings,
        );
    }

    #[test]
    fn prod_env_without_admin_token_reports_fatal() {
        // Config resolves an admin token via env — pick a name that
        // definitely isn't set, so the check catches it.
        let unique = format!("CRONMANAGER_DOCTOR_UNSET_{}", std::process::id());
        std::env::remove_var(&unique);
        let tmp = tempfile::tempdir().unwrap();
        let cfg = AppConfig {
            dsl_path: tmp.path().to_path_buf(),
            admin: AdminConfig {
                bearer_token_env: unique.clone(),
                trust_network: false,
            },
            ..AppConfig::default()
        };
        let r = run(&cfg, Environment::Production);
        assert!(
            r.fatal_count() >= 1,
            "expected at least one FATAL in prod without a token: {:#?}",
            r.findings
        );
        assert_eq!(r.exit_code(), 1);
    }

    #[test]
    fn per_group_token_env_missing_is_weak() {
        let unique = format!("CRONMANAGER_DOCTOR_MISSING_GROUP_{}", std::process::id());
        std::env::remove_var(&unique);
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = cfg_with_dsl(&tmp);
        cfg.security
            .per_group_token_envs
            .insert("payroll".to_string(), unique.clone());
        let r = run(&cfg, Environment::Dev);
        assert!(
            r.findings.iter().any(|f| f.severity == Severity::Weak
                && f.name == "security.per_group_token_envs"
                && f.message.contains("payroll")),
            "expected WEAK finding for unset per-group env var: {:#?}",
            r.findings,
        );
    }

    #[test]
    fn render_starts_with_environment_line_and_ends_with_summary() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = cfg_with_dsl(&tmp);
        let r = run(&cfg, Environment::Dev);
        let out = r.render();
        assert!(out.starts_with("cronmanager doctor — dev environment"));
        assert!(out.trim_end().ends_with("info"));
        assert!(out.contains("summary:"));
    }

    #[test]
    fn severity_ordering_puts_fatal_highest() {
        assert!(Severity::Fatal > Severity::Break);
        assert!(Severity::Break > Severity::Weak);
        assert!(Severity::Weak > Severity::Info);
    }
}

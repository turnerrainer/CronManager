//! CronManager entry point.
//!
//! Assembles: config → history recorder → executor bundle →
//! DSL loader → scheduler → axum router → serve.

use cronmanager::{
    config::AppConfig, dsl::loader, executor::ExecutorBundle, history::postgres::PostgresRecorder,
    history::NoopRecorder, router, scheduler::Scheduler,
};
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let version = env!("CARGO_PKG_VERSION");
    tracing::info!("cronmanager v{} starting", version);

    let (cfg, cfg_source) = AppConfig::load_or_default()?;
    match cfg_source {
        Some(p) => tracing::info!("loaded config from {}", p.display()),
        None => tracing::info!("using built-in defaults (no cronmanager.yaml found)"),
    }
    // Boot-time diagnostic pass — a single INFO summary of every
    // config field plus WARNs for values that would surprise an
    // operator (e.g. `allowed_origins: ["*"]`, disabled timeouts,
    // missing admin token, permissive SSRF posture).
    cfg.boot_diagnostics();

    let bind = format!("0.0.0.0:{}", cfg.port);
    let admin_token = cfg.resolve_admin_token();
    // Refuse-to-start check: if the bind is non-loopback AND
    // there's no admin token AND the operator hasn't opted out
    // via `admin.trust_network=true`, abort with an error that
    // names the env var + the bind so the fix is obvious. See
    // src/router.rs::refuse_to_start_without_token.
    if let Err(msg) = router::refuse_to_start_without_token(&cfg, &bind, admin_token.is_some()) {
        return Err(anyhow::anyhow!(msg));
    }

    // History recorder: Postgres if a DSN is configured and
    // reachable, otherwise Noop.
    let history: Arc<dyn cronmanager::history::HistoryRecorder> = match cfg.database_dsn()? {
        Some(dsn) => {
            let redacted = redact_password(&dsn);
            tracing::info!("history: connecting to {}", redacted);
            match PostgresRecorder::connect(&dsn, cfg.security.stored_response_body_max_bytes).await
            {
                Ok(r) => {
                    tracing::info!("history: enabled (migrations applied)");
                    Arc::new(r)
                }
                Err(e) => {
                    // A misconfigured DB should not kill the
                    // scheduler entirely — degrade to Noop
                    // with a loud WARN so ops can fix it.
                    tracing::error!("history: falling back to noop — DB unreachable: {e}");
                    Arc::new(NoopRecorder)
                }
            }
        }
        None => {
            tracing::info!("history: disabled (no database.url configured)");
            Arc::new(NoopRecorder)
        }
    };

    let bundle = ExecutorBundle::new(&cfg, history)?;
    let scheduler = Scheduler::new(bundle);

    let jobs = loader::load_all_bounded(
        &cfg.dsl_path,
        cfg.security.max_dsl_file_bytes,
        std::time::Duration::from_secs(cfg.security.dsl_load_timeout_secs),
        cfg.security.max_retry_count,
        cfg.security.min_cron_interval_secs,
        cfg.security.block_private_networks,
    )
    .await?;
    let job_count = jobs.len();
    scheduler.register_all(jobs.into_iter().map(|j| j.spec));
    tracing::info!("scheduler: {} job(s) registered", job_count);

    let cfg_arc = Arc::new(cfg);
    let mut state = router::AppState::new(cfg_arc.clone(), scheduler);
    if let Some(token) = admin_token {
        state = state.with_admin_token(token);
    }
    let app = router::build(state);

    tracing::info!("listening on {}", bind);
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

/// Redact the password field of a Postgres URL for log lines —
/// belt-and-braces defense against a config typo dumping the
/// secret into the container log. Handles both `postgres://` (the
/// scheme we build) and `jdbc:postgresql://` (in case an operator
/// pastes a JVM-style DSN while diagnosing).
fn redact_password(dsn: &str) -> String {
    // Strip the `jdbc:` prefix if present so `://` splitting works
    // on `jdbc:postgresql://host/db` as well as `postgres://…`.
    let (prefix, rest) = match dsn.strip_prefix("jdbc:") {
        Some(inner) => ("jdbc:", inner),
        None => ("", dsn),
    };
    if let Some(idx) = rest.find("://") {
        let (scheme, after) = rest.split_at(idx + 3);
        if let Some(at) = after.find('@') {
            let (userinfo, tail) = after.split_at(at);
            let user = match userinfo.find(':') {
                Some(i) => &userinfo[..i],
                None => userinfo,
            };
            return format!("{}{}{}:***{}", prefix, scheme, user, tail);
        }
    }
    dsn.to_string()
}

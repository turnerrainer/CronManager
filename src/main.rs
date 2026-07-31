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

    // History recorder: Postgres if a DSN is configured and
    // reachable, otherwise Noop.
    let history: Arc<dyn cronmanager::history::HistoryRecorder> = match cfg.database_dsn()? {
        Some(dsn) => {
            let redacted = redact_password(&dsn);
            tracing::info!("history: connecting to {}", redacted);
            match PostgresRecorder::connect(&dsn).await {
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

    let jobs = loader::load_all(&cfg.dsl_path)?;
    let job_count = jobs.len();
    scheduler.register_all(jobs.into_iter().map(|j| j.spec));
    tracing::info!("scheduler: {} job(s) registered", job_count);

    let state = router::AppState {
        cfg: Arc::new(cfg.clone()),
        scheduler,
    };
    let app = router::build(state);

    let addr = format!("0.0.0.0:{}", cfg.port);
    tracing::info!("listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

/// Redact the password field of a Postgres URL for log lines —
/// belt-and-braces defense against a config typo dumping the
/// secret into the container log.
fn redact_password(dsn: &str) -> String {
    if let Some(idx) = dsn.find("://") {
        let (scheme, after) = dsn.split_at(idx + 3);
        if let Some(at) = after.find('@') {
            let (userinfo, rest) = after.split_at(at);
            let user = match userinfo.find(':') {
                Some(i) => &userinfo[..i],
                None => userinfo,
            };
            return format!("{}{}:***{}", scheme, user, rest);
        }
    }
    dsn.to_string()
}

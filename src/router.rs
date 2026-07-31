//! Axum router — the HTTP surface of CronManager.
//!
//! Routes faithful to the JVM `CronController`:
//!
//! * `GET  /`
//! * `GET  /health`
//! * `GET  /jobs`, `GET /jobs/{group}`
//! * `GET  /running`, `GET /running/{group}`
//! * `POST /execute/{group}/{job}`
//! * `POST /stop/{group}/{job}`
//! * `POST /reload/{group}`
//!
//! Every error carries a structured JSON body — see
//! book/src/failure-modes.md.

use crate::config::AppConfig;
use crate::dsl::{loader, JobKey};
use crate::error::CronManagerError;
use crate::executor::DispatchExtras;
use crate::scheduler::Scheduler;
use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tower_http::cors::{AllowOrigin, CorsLayer};

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<AppConfig>,
    pub scheduler: Scheduler,
}

pub fn build(state: AppState) -> Router {
    let cors = build_cors(&state.cfg.allowed_origins);
    let mut router = Router::new()
        .route("/", get(index))
        .route("/health", get(health))
        .route("/jobs", get(jobs_all))
        .route("/jobs/:group", get(jobs_group))
        .route("/running", get(running_all))
        .route("/running/:group", get(running_group))
        .route("/execute/:group/:job", post(execute_job))
        .route("/stop/:group/:job", post(stop_job))
        .route("/reload/:group", post(reload_jobs))
        .with_state(state);
    if let Some(cors) = cors {
        router = router.layer(cors);
    }
    router
}

fn build_cors(origins: &[String]) -> Option<CorsLayer> {
    if origins.is_empty() {
        return None;
    }
    // `AllowOrigin::list` requires HeaderValues; skip any origin
    // that isn't a valid header value and log it so operators
    // notice.
    let list: Vec<axum::http::HeaderValue> = origins
        .iter()
        .filter_map(|o| match o.parse() {
            Ok(hv) => Some(hv),
            Err(e) => {
                tracing::warn!("skipping invalid CORS origin '{o}': {e}");
                None
            }
        })
        .collect();
    if list.is_empty() {
        return None;
    }
    Some(CorsLayer::new().allow_origin(AllowOrigin::list(list)))
}

async fn index() -> impl IntoResponse {
    // JVM returned the plain string "CronManager started". Keep
    // that shape byte-for-byte.
    "CronManager started"
}

async fn health() -> impl IntoResponse {
    Json(json!({"status": "ok"}))
}

async fn jobs_all(State(s): State<AppState>) -> Result<Json<Value>, CronManagerError> {
    Ok(Json(
        serde_json::to_value(s.scheduler.describe(None))
            .map_err(|e| CronManagerError::Internal(e.to_string()))?,
    ))
}

async fn jobs_group(
    State(s): State<AppState>,
    Path(group): Path<String>,
) -> Result<Json<Value>, CronManagerError> {
    let filter = if group.is_empty() {
        None
    } else {
        Some(group.as_str())
    };
    Ok(Json(
        serde_json::to_value(s.scheduler.describe(filter))
            .map_err(|e| CronManagerError::Internal(e.to_string()))?,
    ))
}

async fn running_all(State(s): State<AppState>) -> Result<Json<Value>, CronManagerError> {
    Ok(Json(
        serde_json::to_value(s.scheduler.describe_running(None))
            .map_err(|e| CronManagerError::Internal(e.to_string()))?,
    ))
}

async fn running_group(
    State(s): State<AppState>,
    Path(group): Path<String>,
) -> Result<Json<Value>, CronManagerError> {
    let filter = if group.is_empty() {
        None
    } else {
        Some(group.as_str())
    };
    Ok(Json(
        serde_json::to_value(s.scheduler.describe_running(filter))
            .map_err(|e| CronManagerError::Internal(e.to_string()))?,
    ))
}

async fn execute_job(
    State(s): State<AppState>,
    Path((group, job)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Value>, CronManagerError> {
    let key = JobKey::new(group.clone(), job.clone());
    let extras = DispatchExtras {
        env_overrides: params.into_iter().collect(),
    };
    s.scheduler.trigger_now(&key, extras)?;
    // JVM returned the running-jobs snapshot after the trigger;
    // do the same.
    Ok(Json(
        serde_json::to_value(s.scheduler.describe_running(Some(&group)))
            .map_err(|e| CronManagerError::Internal(e.to_string()))?,
    ))
}

async fn stop_job(
    State(s): State<AppState>,
    Path((group, job)): Path<(String, String)>,
) -> Result<Json<Value>, CronManagerError> {
    let key = JobKey::new(group.clone(), job);
    let aborted = s.scheduler.stop(&key)?;
    Ok(Json(json!({
        "stopped": aborted,
        "running": serde_json::to_value(s.scheduler.describe_running(Some(&group)))
            .map_err(|e| CronManagerError::Internal(e.to_string()))?,
    })))
}

async fn reload_jobs(
    State(s): State<AppState>,
    Path(_group): Path<String>,
) -> Result<Json<Value>, CronManagerError> {
    // The `_group` param mirrors JVM's URL shape; behavioural
    // parity: full-tree reload, group filter has no effect at
    // load-time. Task 005 will implement true diff-based reload.
    let jobs = loader::load_all(&s.cfg.dsl_path)?;
    let count = s.scheduler.reload_from(jobs)?;
    Ok(Json(json!({"reloaded": count})))
}

//! Axum router — the HTTP surface of CronManager.
//!
//! Routes faithful to the JVM `CronController`:
//!
//! * `GET  /`
//! * `GET  /health`         (native)
//! * `GET  /healthz`        (K8s convention, aliased to `/health`)
//! * `GET  /actuator/health` (JVM URL, aliased to `/health`)
//! * `GET  /actuator/info`   (JVM URL, minimal build-info body)
//! * `GET  /jobs`, `GET /jobs/` (trailing slash), `GET /jobs/{group}`
//!   — public by default; gated when `security.expose_jobs_publicly=false`
//! * `GET  /running`, `GET /running/` (trailing slash), `GET /running/{group}`
//!   — public by default; gated when `security.expose_running_publicly=false`
//! * `POST /execute/{group}/{job}`   ← bearer-token gated
//! * `POST /stop/{group}/{job}`       ← bearer-token gated
//! * `POST /reload/{group}`           ← bearer-token gated + per-group throttle
//!
//! State-changing routes go through [`admin_gate`], the token
//! comes from the env var named by `admin.bearer_token_env`
//! (default `CRONMANAGER_ADMIN_TOKEN`). Read-only routes stay
//! open so operator dashboards keep working. See
//! `book/src/security.md` for the deployment recipe.

use crate::config::AppConfig;
use crate::dsl::{loader, JobKey};
use crate::error::CronManagerError;
use crate::executor::DispatchExtras;
use crate::scheduler::Scheduler;
use axum::extract::{ConnectInfo, DefaultBodyLimit, FromRequestParts, MatchedPath, Path, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{from_fn_with_state, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::time::Instant;
use subtle::ConstantTimeEq;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;

/// Per-group throttle for `POST /reload/…`. Legit ops workflows
/// reload once per commit; the throttle kills the "flood /reload
/// to amplify a load-time bug" lane (H4). Not part of AppState so
/// tests can construct a fresh one.
///
/// **Memory bound (h2ck.me PR-review v1 nit #1)** — the map used
/// to grow one entry per group name ever seen. Deployments with
/// dynamically-named groups (per-tenant, per-branch) would leak
/// memory in proportion to the group-name churn. The `admit` path
/// now opportunistically evicts entries older than
/// [`ReloadGate::EVICT_STALE_MULTIPLIER`]× the window, and hard-
/// caps the total entry count at [`ReloadGate::MAX_ENTRIES`] by
/// evicting the oldest entry when full. Both bounds are internal
/// details — the throttle behaviour for any given group is
/// unchanged.
#[derive(Default)]
pub struct ReloadGate {
    last: Mutex<HashMap<String, Instant>>,
}

impl ReloadGate {
    /// Entries older than `EVICT_STALE_MULTIPLIER * min_interval_secs`
    /// can be dropped: the throttle would let them pass anyway.
    /// The multiplier is > 1 to give short-window ops loops
    /// (e.g. `reload_min_interval_secs=5`) headroom against
    /// clock skew.
    const EVICT_STALE_MULTIPLIER: u64 = 10;

    /// Hard cap on distinct group names tracked simultaneously.
    /// A deployment with more than this many distinct groups
    /// actively reloading is well outside normal ops shape and
    /// evicting the oldest entry is safe (throttle is best-effort,
    /// not a security control).
    const MAX_ENTRIES: usize = 1024;

    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `Ok(())` if a reload for `group` is allowed now,
    /// or `Err(retry_after_secs)` if the caller must wait. The
    /// admission mutates the map — treat the call as
    /// permission+commit combined.
    pub fn admit(&self, group: &str, min_interval_secs: u64) -> Result<(), u64> {
        if min_interval_secs == 0 {
            return Ok(());
        }
        let mut guard = self.last.lock().unwrap();
        let now = Instant::now();
        if let Some(last) = guard.get(group) {
            let elapsed = now.duration_since(*last).as_secs();
            if elapsed < min_interval_secs {
                return Err(min_interval_secs - elapsed);
            }
        }
        // Opportunistic eviction on the write path. Cheap for
        // typical fleets (single-digit groups); bounded work for
        // larger ones because we drop entries above the stale
        // threshold as we scan.
        let stale_after = min_interval_secs.saturating_mul(Self::EVICT_STALE_MULTIPLIER);
        if stale_after > 0 {
            guard.retain(|_, &mut t| now.duration_since(t).as_secs() < stale_after);
        }
        // Hard cap defense: if the map is still full after stale
        // eviction, drop the single oldest entry to make room.
        if guard.len() >= Self::MAX_ENTRIES && !guard.contains_key(group) {
            if let Some(oldest_key) = guard.iter().min_by_key(|(_, &t)| t).map(|(k, _)| k.clone()) {
                guard.remove(&oldest_key);
            }
        }
        guard.insert(group.to_string(), now);
        Ok(())
    }

    /// Test-only inspection helper — number of entries currently
    /// tracked. Not exposed publicly beyond the crate.
    #[cfg(test)]
    pub fn entry_count(&self) -> usize {
        self.last.lock().unwrap().len()
    }
}

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<AppConfig>,
    pub scheduler: Scheduler,
    /// Admin bearer token resolved from the env var at boot. When
    /// `None`, the [`admin_gate`] middleware short-circuits and
    /// allows the request through — that preserves the
    /// zero-config-loopback UX for local development. Boot-time
    /// diagnostics + [`refuse_to_start_without_token`] make sure
    /// a non-loopback bind with `None` is caught before we ever
    /// serve a request.
    pub admin_token: Option<Arc<String>>,
    /// Optional per-group tokens (h2ck.me F-CM-2). Empty map =
    /// current behaviour (master admin token is the only credential
    /// accepted). When populated, group-scoped admin endpoints
    /// accept EITHER the master token OR the group-specific token.
    /// Keyed by group name; values are the resolved token strings
    /// (never the env-var name). Missing groups only accept the
    /// master.
    pub per_group_tokens: Arc<BTreeMap<String, Arc<String>>>,
    pub reload_gate: Arc<ReloadGate>,
}

impl AppState {
    /// Canonical builder. Callers configure the token separately
    /// via `with_admin_token` if they want the gate active — this
    /// mirrors the CLI wiring done in `main.rs` and keeps existing
    /// tests boilerplate-free.
    pub fn new(cfg: Arc<AppConfig>, scheduler: Scheduler) -> Self {
        Self {
            cfg,
            scheduler,
            admin_token: None,
            per_group_tokens: Arc::new(BTreeMap::new()),
            reload_gate: Arc::new(ReloadGate::new()),
        }
    }

    /// Install a bearer token. When set, [`admin_gate`] rejects
    /// requests to `/execute`, `/stop`, and `/reload` that don't
    /// present a matching `Authorization: Bearer <token>` header.
    pub fn with_admin_token(mut self, token: impl Into<String>) -> Self {
        self.admin_token = Some(Arc::new(token.into()));
        self
    }

    /// Install per-group tokens. Each entry allows the given group's
    /// scoped endpoints to be authenticated with the group-specific
    /// token in addition to the master token. Group names must
    /// match the DSL-derived group (see `dsl::loader::group_from_path`).
    pub fn with_per_group_tokens<I>(mut self, tokens: I) -> Self
    where
        I: IntoIterator<Item = (String, String)>,
    {
        let map: BTreeMap<String, Arc<String>> =
            tokens.into_iter().map(|(k, v)| (k, Arc::new(v))).collect();
        self.per_group_tokens = Arc::new(map);
        self
    }
}

/// Boot-time refuse-to-start check. Called once from `main.rs`
/// before we bind the listener. Returns an error the operator can
/// read that names the env var AND the bind address. Callers on a
/// loopback bind (or with `admin.trust_network=true`) pass through
/// unconditionally.
pub fn refuse_to_start_without_token(
    cfg: &AppConfig,
    bind: &str,
    token_present: bool,
) -> Result<(), String> {
    if token_present {
        return Ok(());
    }
    if cfg.admin.trust_network {
        return Ok(());
    }
    if crate::config::bind_is_loopback(bind) {
        return Ok(());
    }
    Err(format!(
        "refusing to start on non-loopback bind {bind} without an admin token: set env var {} to enable the /execute + /stop + /reload gate, or set admin.trust_network=true if a reverse proxy / service mesh authenticates every request before it reaches this process",
        cfg.admin.bearer_token_env,
    ))
}

/// Extractor that enforces `security.max_query_params`. Returns
/// 413 (with a structured JSON body naming the count + cap) if the
/// request URL has too many key/value pairs. Duplicates each count
/// as their own pair, mirroring `Query<HashMap>`'s behaviour — a
/// caller who sends `?k=1&k=2` will see one entry in the resulting
/// map but two toward the cap.
pub struct BoundedQuery(pub Vec<(String, String)>);

#[axum::async_trait]
impl<S> FromRequestParts<S> for BoundedQuery
where
    S: Send + Sync,
    AppState: axum::extract::FromRef<S>,
{
    type Rejection = CronManagerError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app_state: AppState = axum::extract::FromRef::from_ref(state);
        let cap = app_state.cfg.security.max_query_params;
        let raw = parts.uri.query().unwrap_or("");
        let pairs: Vec<(String, String)> = form_urlencoded::parse(raw.as_bytes())
            .into_owned()
            .collect();
        if pairs.len() > cap {
            return Err(CronManagerError::TooManyQueryParams {
                count: pairs.len(),
                limit: cap,
            });
        }
        Ok(BoundedQuery(pairs))
    }
}

/// Middleware for state-changing endpoints. If the AppState has
/// `admin_token=Some(t)`, this rejects any request that doesn't
/// carry `Authorization: Bearer <t>`. The comparison is
/// constant-time (`subtle::ConstantTimeEq`) so wrong-token
/// responses don't leak timing information. When `admin_token=None`,
/// the middleware short-circuits — combined with the boot-time
/// refuse-to-start check, this keeps zero-config loopback dev
/// working without ever leaving a state-changing endpoint open on
/// a non-loopback bind by accident.
///
/// **Per-group tokens (h2ck.me F-CM-2)**: when
/// `state.per_group_tokens` has an entry for the group extracted
/// from the URL path, the presented token is accepted if it
/// matches EITHER the master admin token OR the group-specific
/// token. Both comparisons use `ConstantTimeEq` and run
/// unconditionally so a caller can't time which token was checked.
pub async fn admin_gate(
    State(state): State<AppState>,
    // ConnectInfo is populated by `into_make_service_with_connect_info`
    // in main.rs. Tests that use `axum::serve(listener, app)` without
    // that wrapping pass `None` — the auth-failure log line then omits
    // the client hash but still records the reject.
    client: Option<ConnectInfo<SocketAddr>>,
    headers: HeaderMap,
    req: axum::http::Request<axum::body::Body>,
    next: Next,
) -> Result<Response, CronManagerError> {
    let Some(master) = state.admin_token.as_deref() else {
        return Ok(next.run(req).await);
    };
    // Resolve the client-IP hash + matched route once so both
    // failure branches emit the same structured shape (h2ck.me
    // LOG-FINDINGS FN-LOG-2 — bare `WARN missing token` gives no
    // way to distinguish burst-traffic sources or derive a
    // blocklist from logs).
    let ip_hash = client
        .as_ref()
        .map(|c| crate::security::short_client_hash(c.0.ip().to_string().as_bytes()));
    let route = req
        .extensions()
        .get::<MatchedPath>()
        .map(|m| m.as_str().to_string());
    let reject = |reason: &'static str| -> CronManagerError {
        tracing::warn!(
            client_ip_hash = ip_hash.as_deref().unwrap_or("<absent>"),
            route = route.as_deref().unwrap_or("<unmatched>"),
            reason = reason,
            "auth: rejected admin request",
        );
        CronManagerError::Unauthorized
    };
    let presented = match headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(bearer_token_bytes)
    {
        Some(bytes) => bytes,
        None => return Err(reject("missing or malformed Authorization header")),
    };

    // Master-token check — always run so a wrong per-group-only
    // token can't be distinguished by timing from a wrong master
    // (subtle::ConstantTimeEq handles the byte comparison).
    let master_ok = ct_token_matches(&presented, master.as_bytes());

    // Per-group token check (h2ck.me F-CM-2) — only meaningful when
    // the URL has a group segment AND the per_group_tokens map has
    // an entry for it. Groups without an entry fall back to
    // master-only.
    let group_ok = extract_group_from_path(req.uri().path())
        .and_then(|g| state.per_group_tokens.get(g))
        .map(|group_token| ct_token_matches(&presented, group_token.as_bytes()))
        .unwrap_or(false);

    if !(master_ok || group_ok) {
        return Err(reject("token does not match expected"));
    }
    Ok(next.run(req).await)
}

/// Constant-time equality check for token bytes. Length mismatch is
/// safe to short-circuit — the token length isn't a secret (it's a
/// fixed operator-chosen constant).
fn ct_token_matches(presented: &[u8], expected: &[u8]) -> bool {
    if presented.len() != expected.len() {
        return false;
    }
    bool::from(presented.ct_eq(expected))
}

/// Extract the group segment from the URL path for the three
/// group-scoped admin routes (`/execute/{group}/{job}`,
/// `/stop/{group}/{job}`, `/reload/{group}`). Returns `None` for
/// any other path — group-less admin routes fall back to
/// master-only authentication.
fn extract_group_from_path(path: &str) -> Option<&str> {
    let mut parts = path.strip_prefix('/')?.splitn(3, '/');
    match parts.next()? {
        "execute" | "stop" | "reload" => {}
        _ => return None,
    }
    let group = parts.next()?;
    if group.is_empty() {
        return None;
    }
    Some(group)
}

/// Extract the token after `Bearer ` (case-insensitive on the
/// scheme name, per RFC 6750 §2.1). Returns the token verbatim
/// (leading/trailing whitespace trimmed) or `None` when the header
/// isn't a valid Bearer credential.
fn bearer_token_bytes(header: &str) -> Option<Vec<u8>> {
    let trimmed = header.trim();
    // Scheme is a case-insensitive prefix per RFC 6750. Split at
    // the first whitespace so we don't hardcode "Bearer " with a
    // single-space separator.
    let i = trimmed.find(char::is_whitespace)?;
    let (scheme, rest) = (&trimmed[..i], trimmed[i..].trim());
    if !scheme.eq_ignore_ascii_case("Bearer") {
        return None;
    }
    if rest.is_empty() {
        return None;
    }
    Some(rest.as_bytes().to_vec())
}

pub fn build(state: AppState) -> Router {
    let cors = build_cors(&state.cfg.allowed_origins);
    let body_limit = state.cfg.limits.max_request_bytes;
    let request_timeout_secs = state.cfg.limits.request_timeout_secs;
    let jobs_public = state.cfg.security.expose_jobs_publicly;
    let running_public = state.cfg.security.expose_running_publicly;

    // Two-tier router: state-changing routes go through admin_gate.
    // Recon endpoints (/jobs, /running) join the gated tree when
    // the operator flips security.expose_jobs_publicly=false or
    // security.expose_running_publicly=false (h2ck.me FN3 / F-CM-1).
    // The default keeps them open for backward compatibility with
    // v0.1.4-alpha operator dashboards.
    let mut admin_routes = Router::new()
        .route("/execute/:group/:job", post(execute_job))
        .route("/stop/:group/:job", post(stop_job))
        .route("/reload/:group", post(reload_jobs));
    if !jobs_public {
        admin_routes = admin_routes
            .route("/jobs", get(jobs_all))
            .route("/jobs/", get(jobs_all))
            .route("/jobs/:group", get(jobs_group));
    }
    if !running_public {
        admin_routes = admin_routes
            .route("/running", get(running_all))
            .route("/running/", get(running_all))
            .route("/running/:group", get(running_group));
    }
    let admin_routes = admin_routes.layer(from_fn_with_state(state.clone(), admin_gate));

    let mut public = Router::new()
        .route("/", get(index))
        // /health is the native name; /actuator/health is the JVM
        // URL kept for operators grepping the old Actuator path.
        // /healthz is the Kubernetes convention (default in most
        // Helm charts' livenessProbe / readinessProbe) — aliased
        // so a k8s deployment doesn't get 401 on the probe when the
        // admin gate is active. All three return the same body.
        .route("/health", get(health))
        .route("/healthz", get(health))
        .route("/actuator/health", get(health))
        .route("/actuator/info", get(info));
    if jobs_public {
        public = public
            .route("/jobs", get(jobs_all))
            // JVM `CronController.java:31` mapped both `/jobs` and
            // `/jobs/` — axum treats these as distinct so register
            // both explicitly.
            .route("/jobs/", get(jobs_all))
            .route("/jobs/:group", get(jobs_group));
    }
    if running_public {
        public = public
            .route("/running", get(running_all))
            .route("/running/", get(running_all))
            .route("/running/:group", get(running_group));
    }
    let mut router = public
        .merge(admin_routes)
        .with_state(state)
        // Wire `limits.max_request_bytes`. The default axum body
        // limit is 2 MiB; disable it and apply ours so the
        // operator's config wins.
        .layer(DefaultBodyLimit::disable())
        .layer(RequestBodyLimitLayer::new(body_limit))
        // FLEET-STRONGHOLDS §5.1 — defense-in-depth browser
        // headers on every response. See src/security_headers.rs
        // for the values + rationale.
        .layer(axum::middleware::from_fn(
            crate::security_headers::security_headers_middleware,
        ))
        // FLEET-STRONGHOLDS §1.6 — W3C `traceparent` +
        // `x-trace-id` response headers on every response so
        // downstream services can correlate.
        .layer(axum::middleware::from_fn(
            crate::traceparent::traceparent_middleware,
        ))
        // Audit LOG-v1 FN-LOG-3: emit one INFO line per completed
        // request for SOC2/ISO27001 access-log compliance. See
        // src/access_log.rs.
        .layer(axum::middleware::from_fn(
            crate::access_log::access_log_middleware,
        ));
    // Whole-request wall-clock deadline (h2ck.me RUNTIME-FINDINGS
    // v1 FN5). The upstream/executor timeouts only cover the
    // dispatch phase; a slow-drip body upload could hold a Tokio
    // task past `limits.request_timeout_secs` because the body-read
    // wasn't bounded. TimeoutLayer wraps the ENTIRE request/response
    // (headers + body-read + handler + response-write). Setting
    // `limits.request_timeout_secs=0` disables the layer to preserve
    // the documented escape hatch (WARN emitted at boot).
    if request_timeout_secs > 0 {
        router = router.layer(TimeoutLayer::new(Duration::from_secs(request_timeout_secs)));
    }
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

async fn info() -> impl IntoResponse {
    // JVM Actuator returned build info populated by the Spring
    // Boot plugin — Rust ships the CARGO_PKG_NAME/VERSION pair
    // only. Git-SHA / build-time enrichment is a backlog item.
    Json(json!({
        "name": env!("CARGO_PKG_NAME"),
        "version": env!("CARGO_PKG_VERSION"),
    }))
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
    BoundedQuery(params): BoundedQuery,
) -> Result<Json<Value>, CronManagerError> {
    let key = JobKey::new(group.clone(), job.clone());

    // Dangerous-env blacklist — reject an override for PATH /
    // LD_* / DYLD_* / interpreter-hook vars even when the DSL's
    // `allowedEnvs` includes them. Operators who really need one
    // of those overrides can set
    // `security.allow_dangerous_env_overrides: true` and accept
    // the boot-time WARN.
    if !s.cfg.security.allow_dangerous_env_overrides {
        for (k, _) in &params {
            if crate::security::is_dangerous_env(k) {
                return Err(CronManagerError::Forbidden(format!(
                    "env override '{k}' matches the dangerous-env blacklist (PATH, LD_*, DYLD_*, PYTHONPATH, NODE_OPTIONS, …); refusing to fire {group}/{job}"
                )));
            }
        }
    }

    // Audit trail — INFO log records the KEYS of applied
    // overrides (never values, since query params are attacker-
    // supplied and could carry secrets). Missing entry in logs
    // means no overrides for this dispatch.
    if !params.is_empty() {
        let keys: Vec<&str> = params.iter().map(|(k, _)| k.as_str()).collect();
        tracing::info!(
            job = %format!("{group}/{job}"),
            override_count = params.len(),
            override_keys = ?keys,
            "execute: env overrides applied"
        );
    }

    let extras = DispatchExtras {
        env_overrides: params,
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
    Path(group): Path<String>,
) -> Result<Json<Value>, CronManagerError> {
    // Per-group throttle — kills the "/reload flood" amplifier
    // for M1 (YAML bombs) and generally makes ops reloads
    // observable in the log stream instead of a noisy loop.
    if let Err(wait) = s
        .reload_gate
        .admit(&group, s.cfg.security.reload_min_interval_secs)
    {
        tracing::warn!(
            group = %group,
            retry_after_secs = wait,
            "reload: rejected — throttle window not elapsed"
        );
        return Err(CronManagerError::TooManyRequests {
            retry_after_secs: wait,
        });
    }
    // The `group` param mirrors JVM's URL shape; behavioural
    // parity: full-tree reload, group filter has no effect at
    // load-time. Task 005 will implement true diff-based reload.
    let jobs = loader::load_all_bounded(
        &s.cfg.dsl_path,
        s.cfg.security.max_dsl_file_bytes,
        std::time::Duration::from_secs(s.cfg.security.dsl_load_timeout_secs),
        s.cfg.security.max_retry_count,
        s.cfg.security.min_cron_interval_secs,
        s.cfg.security.block_private_networks,
    )
    .await?;
    let count = s.scheduler.reload_from(jobs)?;
    Ok(Json(json!({"reloaded": count})))
}

// Small compile-time nudge so `HeaderMap` doesn't get flagged as
// an unused import when the middleware is re-exported without a
// consumer inside this module.
#[allow(dead_code)]
fn _use_status_code(_s: StatusCode) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_token_bytes_accepts_case_insensitive_scheme() {
        assert_eq!(bearer_token_bytes("Bearer abc"), Some(b"abc".to_vec()));
        assert_eq!(bearer_token_bytes("bearer abc"), Some(b"abc".to_vec()));
        assert_eq!(bearer_token_bytes("BEARER abc"), Some(b"abc".to_vec()));
    }

    #[test]
    fn bearer_token_bytes_trims_padding() {
        // The spec doesn't require exactly one space; make sure a
        // stray leading/trailing whitespace or a tab separator
        // works too.
        assert_eq!(
            bearer_token_bytes("  Bearer   abc  "),
            Some(b"abc".to_vec())
        );
        assert_eq!(bearer_token_bytes("Bearer\tabc"), Some(b"abc".to_vec()));
    }

    #[test]
    fn bearer_token_bytes_rejects_other_schemes() {
        assert_eq!(bearer_token_bytes("Basic abc"), None);
        assert_eq!(bearer_token_bytes("Token abc"), None);
        assert_eq!(bearer_token_bytes("Bearer"), None);
        assert_eq!(bearer_token_bytes(""), None);
    }

    #[test]
    fn reload_gate_admits_first_call() {
        let g = ReloadGate::new();
        assert!(g.admit("samples", 60).is_ok());
    }

    #[test]
    fn reload_gate_rejects_second_call_within_window() {
        let g = ReloadGate::new();
        assert!(g.admit("samples", 60).is_ok());
        let e = g.admit("samples", 60).unwrap_err();
        assert!(e > 0 && e <= 60);
    }

    #[test]
    fn reload_gate_isolates_groups() {
        let g = ReloadGate::new();
        assert!(g.admit("a", 60).is_ok());
        assert!(g.admit("b", 60).is_ok());
    }

    #[test]
    fn reload_gate_disabled_when_interval_zero() {
        let g = ReloadGate::new();
        assert!(g.admit("x", 0).is_ok());
        // Zero interval = always admit, no state tracking.
        assert!(g.admit("x", 0).is_ok());
    }

    // PR-review v1 nit #1 — the last-hit map used to grow one
    // entry per group name ever seen. The hard cap guarantees a
    // bounded working set under dynamically-named-group churn.
    #[test]
    fn reload_gate_hard_caps_entry_count() {
        let g = ReloadGate::new();
        // Fill past the cap. Very large min_interval so eviction
        // via the stale-timeout path is not what limits us — the
        // hard cap must fire.
        for i in 0..(ReloadGate::MAX_ENTRIES + 200) {
            let _ = g.admit(&format!("tenant-{i}"), 3600);
        }
        assert!(
            g.entry_count() <= ReloadGate::MAX_ENTRIES,
            "expected <= {} entries, got {}",
            ReloadGate::MAX_ENTRIES,
            g.entry_count(),
        );
    }

    #[test]
    fn reload_gate_hard_cap_evicts_oldest_first() {
        let g = ReloadGate::new();
        for i in 0..ReloadGate::MAX_ENTRIES {
            let _ = g.admit(&format!("g{i}"), 3600);
        }
        // First key hit under the cap.
        {
            let guard = g.last.lock().unwrap();
            assert!(guard.contains_key("g0"));
        }
        // Add one more distinct key — this triggers the hard-cap
        // path which drops the oldest entry ("g0").
        let _ = g.admit("newcomer", 3600);
        let guard = g.last.lock().unwrap();
        assert!(
            !guard.contains_key("g0"),
            "oldest entry should have been evicted"
        );
        assert!(guard.contains_key("newcomer"));
        assert_eq!(guard.len(), ReloadGate::MAX_ENTRIES);
    }

    #[test]
    fn refuse_to_start_allows_loopback_without_token() {
        let cfg = AppConfig::default();
        assert!(refuse_to_start_without_token(&cfg, "127.0.0.1:8080", false).is_ok());
        assert!(refuse_to_start_without_token(&cfg, "[::1]:8080", false).is_ok());
        assert!(refuse_to_start_without_token(&cfg, "localhost:8080", false).is_ok());
    }

    #[test]
    fn refuse_to_start_allows_any_bind_with_token() {
        let cfg = AppConfig::default();
        assert!(refuse_to_start_without_token(&cfg, "0.0.0.0:8080", true).is_ok());
    }

    #[test]
    fn refuse_to_start_allows_trust_network_opt_out() {
        let mut cfg = AppConfig::default();
        cfg.admin.trust_network = true;
        assert!(refuse_to_start_without_token(&cfg, "0.0.0.0:8080", false).is_ok());
    }

    #[test]
    fn refuse_to_start_blocks_wildcard_bind_without_token() {
        let cfg = AppConfig::default();
        let err = refuse_to_start_without_token(&cfg, "0.0.0.0:8080", false).unwrap_err();
        assert!(err.contains("0.0.0.0"));
        assert!(err.contains(&cfg.admin.bearer_token_env));
    }

    // h2ck.me F-CM-2 — per-group tokens. Unit-level pins for the
    // pure helper functions. Router-level end-to-end pins live in
    // tests/security_hardening.rs.
    #[test]
    fn extract_group_from_execute_path() {
        assert_eq!(
            extract_group_from_path("/execute/samples/hello"),
            Some("samples")
        );
        assert_eq!(
            extract_group_from_path("/execute/group-a/nested/job"),
            Some("group-a")
        );
    }

    #[test]
    fn extract_group_from_stop_path() {
        assert_eq!(
            extract_group_from_path("/stop/samples/hello"),
            Some("samples")
        );
    }

    #[test]
    fn extract_group_from_reload_path() {
        assert_eq!(extract_group_from_path("/reload/samples"), Some("samples"));
    }

    #[test]
    fn extract_group_returns_none_for_non_admin_paths() {
        // Read-only and unknown paths — the gate short-circuits
        // to master-only.
        assert_eq!(extract_group_from_path("/health"), None);
        assert_eq!(extract_group_from_path("/jobs/samples"), None);
        assert_eq!(extract_group_from_path("/running/samples"), None);
        assert_eq!(extract_group_from_path("/"), None);
        assert_eq!(extract_group_from_path(""), None);
    }

    #[test]
    fn extract_group_returns_none_for_empty_group_segment() {
        // Guard against `/execute//job` which would otherwise slip
        // a caller past the per_group_tokens check with the empty
        // string as the group.
        assert_eq!(extract_group_from_path("/execute//job"), None);
        assert_eq!(extract_group_from_path("/reload/"), None);
    }

    #[test]
    fn ct_token_matches_length_mismatch_returns_false() {
        assert!(!ct_token_matches(b"short", b"longer-token"));
        assert!(!ct_token_matches(b"", b"nonempty"));
    }

    #[test]
    fn ct_token_matches_equal_bytes_returns_true() {
        assert!(ct_token_matches(b"exact", b"exact"));
    }

    #[test]
    fn ct_token_matches_different_same_length_returns_false() {
        assert!(!ct_token_matches(b"AAAA", b"BBBB"));
    }
}

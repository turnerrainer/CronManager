//! Regression pins for the h2ck.me v1 audit findings.
//!
//! Every test here corresponds to a finding in
//! `../../../h2ck.me/projects/CronManager-on-Rust/v1/AUDIT.md`.
//! The test name carries the finding ID (C1, C2, H1, …) so a
//! future audit round can grep for coverage.
//!
//! These tests build the router with a bearer token installed so
//! the admin-gated endpoints require the header. Read-only
//! endpoints stay open and are exercised without one.

use cronmanager::{
    config::{AdminConfig, AppConfig, SecurityConfig},
    executor::ExecutorBundle,
    history::NoopRecorder,
    router::{self, AppState},
    scheduler::Scheduler,
};
use serde_json::Value;
use std::sync::Arc;

const TOKEN: &str = "s3cret-token-for-tests";

async fn spawn_gated_app(cfg: AppConfig) -> String {
    let bundle = ExecutorBundle::new(&cfg, Arc::new(NoopRecorder)).unwrap();
    let scheduler = Scheduler::new(bundle);
    let state = AppState::new(Arc::new(cfg), scheduler.clone()).with_admin_token(TOKEN.to_string());
    let app = router::build(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

fn default_cfg() -> AppConfig {
    AppConfig {
        app_root_path: std::env::temp_dir(),
        ..AppConfig::default()
    }
}

// ---------- C1 — admin bearer gate ----------

#[tokio::test]
async fn c1_execute_without_bearer_returns_401() {
    let base = spawn_gated_app(default_cfg()).await;
    let resp = reqwest::Client::new()
        .post(format!("{base}/execute/nosuch/nope"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 401);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "unauthorized");
}

#[tokio::test]
async fn c1_execute_with_wrong_bearer_returns_401() {
    let base = spawn_gated_app(default_cfg()).await;
    let resp = reqwest::Client::new()
        .post(format!("{base}/execute/nosuch/nope"))
        .bearer_auth("nope-not-this")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 401);
}

#[tokio::test]
async fn c1_execute_with_right_bearer_passes_gate() {
    // Correct token → the gate is transparent. Job doesn't exist
    // so we get 404 (`job_not_found`), NOT 401.
    let base = spawn_gated_app(default_cfg()).await;
    let resp = reqwest::Client::new()
        .post(format!("{base}/execute/nosuch/nope"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_ne!(resp.status().as_u16(), 401);
    assert_eq!(resp.status().as_u16(), 404);
}

#[tokio::test]
async fn c1_stop_gate_requires_bearer() {
    let base = spawn_gated_app(default_cfg()).await;
    let no = reqwest::Client::new()
        .post(format!("{base}/stop/nosuch/nope"))
        .send()
        .await
        .unwrap();
    assert_eq!(no.status().as_u16(), 401);
    let yes = reqwest::Client::new()
        .post(format!("{base}/stop/nosuch/nope"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_ne!(yes.status().as_u16(), 401);
}

#[tokio::test]
async fn c1_reload_gate_requires_bearer() {
    let base = spawn_gated_app(default_cfg()).await;
    let no = reqwest::Client::new()
        .post(format!("{base}/reload/samples"))
        .send()
        .await
        .unwrap();
    assert_eq!(no.status().as_u16(), 401);
}

#[tokio::test]
async fn c1_read_only_routes_stay_open() {
    // Documented posture: /health, /jobs, /running stay
    // unauthenticated so operator dashboards keep working.
    let base = spawn_gated_app(default_cfg()).await;
    for path in ["/health", "/jobs", "/running", "/actuator/info"] {
        let s = reqwest::get(format!("{base}{path}"))
            .await
            .unwrap()
            .status();
        assert_eq!(s.as_u16(), 200, "{path} should be 200, got {s}");
    }
}

#[tokio::test]
async fn c1_case_insensitive_bearer_scheme() {
    let base = spawn_gated_app(default_cfg()).await;
    // RFC 6750 §2.1 — scheme is case-insensitive.
    let resp = reqwest::Client::new()
        .post(format!("{base}/execute/nosuch/nope"))
        .header("authorization", format!("bearer {TOKEN}"))
        .send()
        .await
        .unwrap();
    assert_ne!(resp.status().as_u16(), 401);
}

// ---------- C2 — query-param count cap ----------

#[tokio::test]
async fn c2_query_params_above_cap_return_413() {
    let base = spawn_gated_app(default_cfg()).await;
    // Default cap is 64; send 65.
    let qs: String = (0..65)
        .map(|i| format!("k{i}=v{i}"))
        .collect::<Vec<_>>()
        .join("&");
    let resp = reqwest::Client::new()
        .post(format!("{base}/execute/nosuch/nope?{qs}"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 413);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "too_many_query_params");
    assert_eq!(body["limit"], 64);
    assert_eq!(body["count"], 65);
}

#[tokio::test]
async fn c2_query_params_at_cap_pass_extractor() {
    let base = spawn_gated_app(default_cfg()).await;
    // 64 params — exactly at the cap. Extractor must pass; the
    // handler then 404s on the unknown job.
    let qs: String = (0..64)
        .map(|i| format!("k{i}=v{i}"))
        .collect::<Vec<_>>()
        .join("&");
    let resp = reqwest::Client::new()
        .post(format!("{base}/execute/nosuch/nope?{qs}"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 404);
}

// ---------- H3 — dangerous-env blacklist ----------

#[tokio::test]
async fn h3_path_override_is_rejected_403() {
    let base = spawn_gated_app(default_cfg()).await;
    let resp = reqwest::Client::new()
        .post(format!("{base}/execute/nosuch/nope?PATH=/tmp"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 403);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "forbidden");
    assert!(
        body["message"].as_str().unwrap().contains("PATH"),
        "message should name the offending key: {body}"
    );
}

#[tokio::test]
async fn h3_ld_preload_override_is_rejected_403() {
    let base = spawn_gated_app(default_cfg()).await;
    let resp = reqwest::Client::new()
        .post(format!("{base}/execute/nosuch/nope?LD_PRELOAD=/tmp/x.so"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 403);
}

#[tokio::test]
async fn h3_lowercase_path_still_rejected() {
    // Case-insensitive: `?path=/tmp` must be blocked too.
    let base = spawn_gated_app(default_cfg()).await;
    let resp = reqwest::Client::new()
        .post(format!("{base}/execute/nosuch/nope?path=/tmp"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 403);
}

#[tokio::test]
async fn h3_dangerous_env_bypass_when_opted_out() {
    let mut cfg = default_cfg();
    cfg.security.allow_dangerous_env_overrides = true;
    let base = spawn_gated_app(cfg).await;
    // Now the router lets the override through — the job doesn't
    // exist, so the handler 404s (not 403).
    let resp = reqwest::Client::new()
        .post(format!("{base}/execute/nosuch/nope?PATH=/tmp"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 404);
}

#[tokio::test]
async fn h3_non_dangerous_env_still_allowed() {
    let base = spawn_gated_app(default_cfg()).await;
    // `FOO` isn't in the blacklist — handler 404s.
    let resp = reqwest::Client::new()
        .post(format!("{base}/execute/nosuch/nope?FOO=bar"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 404);
}

// ---------- H4 — reload throttle ----------

#[tokio::test]
async fn h4_reload_throttle_returns_429_within_window() {
    let mut cfg = default_cfg();
    // Point at a tempdir that exists but has no DSL files so
    // load_all_bounded succeeds and updates the throttle map.
    let empty = tempfile::tempdir().unwrap();
    cfg.dsl_path = empty.path().to_path_buf();
    cfg.security.reload_min_interval_secs = 60;
    let base = spawn_gated_app(cfg).await;

    // First reload — succeeds.
    let first = reqwest::Client::new()
        .post(format!("{base}/reload/samples"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(first.status().as_u16(), 200);

    // Second reload for the same group within the window — 429.
    let second = reqwest::Client::new()
        .post(format!("{base}/reload/samples"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(second.status().as_u16(), 429);
    let body: Value = second.json().await.unwrap();
    assert_eq!(body["error"], "too_many_requests");
    assert!(body["retry_after_secs"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn h4_reload_throttle_per_group() {
    let mut cfg = default_cfg();
    let empty = tempfile::tempdir().unwrap();
    cfg.dsl_path = empty.path().to_path_buf();
    cfg.security.reload_min_interval_secs = 60;
    let base = spawn_gated_app(cfg).await;

    let a = reqwest::Client::new()
        .post(format!("{base}/reload/group-a"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(a.status().as_u16(), 200);
    let b = reqwest::Client::new()
        .post(format!("{base}/reload/group-b"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(b.status().as_u16(), 200);
}

// ---------- No-token loopback keeps zero-config UX ----------

#[tokio::test]
async fn no_token_bind_loopback_still_serves_state_changes() {
    // AppState::new without with_admin_token → gate short-circuits
    // → /execute reaches the handler. This documents the escape
    // hatch for local dev.
    let cfg = default_cfg();
    let bundle = ExecutorBundle::new(&cfg, Arc::new(NoopRecorder)).unwrap();
    let scheduler = Scheduler::new(bundle);
    let state = AppState::new(Arc::new(cfg), scheduler);
    let app = router::build(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/execute/nosuch/nope"))
        .send()
        .await
        .unwrap();
    // No token → gate is open → handler returns 404 for unknown job.
    assert_eq!(resp.status().as_u16(), 404);
}

// ---------- refuse-to-start check ----------

#[test]
fn refuse_to_start_nonloopback_no_token_is_hard_error() {
    let cfg = AppConfig::default();
    let err = router::refuse_to_start_without_token(&cfg, "0.0.0.0:8080", false).unwrap_err();
    assert!(err.contains("0.0.0.0"));
    assert!(err.contains(&cfg.admin.bearer_token_env));
}

#[test]
fn refuse_to_start_nonloopback_with_token_ok() {
    let cfg = AppConfig {
        admin: AdminConfig {
            bearer_token_env: "IGNORED_HERE".into(),
            trust_network: false,
        },
        security: SecurityConfig::default(),
        ..AppConfig::default()
    };
    assert!(router::refuse_to_start_without_token(&cfg, "0.0.0.0:8080", true).is_ok());
}

//! Integration tests for the axum router — build a real app,
//! send real HTTP over an ephemeral TCP port, assert on the
//! JSON responses. History is set to `NoopRecorder` so no DB is
//! required.
//!
//! DEV-REQUIREMENTS §3.3 — every code seam gets an integration
//! test.

use cronmanager::{
    config::AppConfig,
    dsl::{JobKey, JobKind, JobSpec, RetryPolicy, TimeWindow, Trigger},
    executor::ExecutorBundle,
    history::NoopRecorder,
    router::{self, AppState},
    scheduler::Scheduler,
};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

async fn spawn_app() -> (String, Scheduler) {
    let cfg = AppConfig {
        app_root_path: std::env::temp_dir(),
        ..AppConfig::default()
    };
    let bundle = ExecutorBundle::new(&cfg, Arc::new(NoopRecorder)).unwrap();
    let scheduler = Scheduler::new(bundle);

    let state = AppState::new(Arc::new(cfg), scheduler.clone());
    let app = router::build(state);

    // OS-assigned port so tests can run in parallel without
    // colliding.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), scheduler)
}

fn manual_http_job(name: &str) -> JobSpec {
    JobSpec {
        key: JobKey::new("test", name),
        trigger: Trigger::Manual,
        window: TimeWindow::default(),
        retry: RetryPolicy::default(),
        kind: JobKind::Http {
            method: "GET".into(),
            url: "http://127.0.0.1:1/nowhere".into(),
        },
    }
}

#[tokio::test]
async fn health_endpoint_returns_ok() {
    let (base, _sched) = spawn_app().await;
    let body: Value = reqwest::get(format!("{base}/health"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["status"], "ok");
}

#[tokio::test]
async fn index_returns_jvm_sentinel_string() {
    let (base, _sched) = spawn_app().await;
    let body = reqwest::get(format!("{base}/"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(body, "CronManager started");
}

#[tokio::test]
async fn jobs_endpoint_lists_registered_jobs() {
    let (base, sched) = spawn_app().await;
    sched.register(manual_http_job("first"));

    let body: Value = reqwest::get(format!("{base}/jobs"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        body.get("test").is_some(),
        "expected group 'test' in response: {body}"
    );
    let arr = body["test"].as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["name"], "first");
    assert_eq!(arr[0]["schedule"], "");
}

#[tokio::test]
async fn jobs_by_group_filters() {
    let (base, sched) = spawn_app().await;
    sched.register(manual_http_job("a"));
    sched.register(JobSpec {
        key: JobKey::new("other", "b"),
        ..manual_http_job("b")
    });

    let body: Value = reqwest::get(format!("{base}/jobs/test"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(body.get("test").is_some());
    assert!(body.get("other").is_none());
}

#[tokio::test]
async fn execute_unknown_returns_404_with_structured_body() {
    let (base, _sched) = spawn_app().await;
    let resp = reqwest::Client::new()
        .post(format!("{base}/execute/nosuch/nope"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 404);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "job_not_found");
}

#[tokio::test]
async fn stop_unknown_returns_404() {
    let (base, _sched) = spawn_app().await;
    let resp = reqwest::Client::new()
        .post(format!("{base}/stop/nosuch/nope"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 404);
}

#[tokio::test]
async fn stop_of_not_running_returns_200_with_stopped_false() {
    let (base, sched) = spawn_app().await;
    sched.register(manual_http_job("idle"));
    let resp = reqwest::Client::new()
        .post(format!("{base}/stop/test/idle"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["stopped"], false);
}

#[tokio::test]
async fn running_endpoint_returns_empty_when_nothing_running() {
    let (base, _sched) = spawn_app().await;
    let body: Value = reqwest::get(format!("{base}/running"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(body.is_object());
    assert!(body.as_object().unwrap().is_empty());
}

#[tokio::test]
async fn execute_slow_shell_job_appears_in_running_snapshot() {
    let (base, sched) = spawn_app().await;
    // Long shell job so /running has time to observe it.
    let spec = JobSpec {
        key: JobKey::new("test", "slow"),
        trigger: Trigger::Manual,
        window: TimeWindow::default(),
        retry: RetryPolicy::default(),
        kind: JobKind::Exec {
            command: "/bin/sleep 2".into(),
            allowed_envs: vec![],
        },
    };
    sched.register(spec);

    let resp = reqwest::Client::new()
        .post(format!("{base}/execute/test/slow"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);

    // Give the spawn a moment to enter the running registry.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let body: Value = reqwest::get(format!("{base}/running"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        body.get("test")
            .and_then(|g| g.as_array())
            .map(|a| !a.is_empty())
            .unwrap_or(false),
        "expected slow job to be in /running snapshot: {body}"
    );

    // Clean up.
    let _ = reqwest::Client::new()
        .post(format!("{base}/stop/test/slow"))
        .send()
        .await;
}

#[tokio::test]
async fn actuator_health_alias_matches_health() {
    // Rust honours the JVM URL `/actuator/health` with the same
    // body as `/health`.
    let (base, _sched) = spawn_app().await;
    let a: Value = reqwest::get(format!("{base}/health"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let b: Value = reqwest::get(format!("{base}/actuator/health"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(a, b, "actuator/health must be an exact alias of /health");
    assert_eq!(a["status"], "ok");
}

#[tokio::test]
async fn actuator_info_returns_name_and_version() {
    // Minimal build info at the JVM URL.
    let (base, _sched) = spawn_app().await;
    let body: Value = reqwest::get(format!("{base}/actuator/info"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["name"], "cronmanager");
    assert!(
        body["version"].as_str().unwrap().starts_with("0."),
        "version was: {}",
        body["version"]
    );
}

#[tokio::test]
async fn trailing_slash_jobs_route_matches_no_slash() {
    // JVM `CronController.java:31` mapped both `/jobs` and
    // `/jobs/`. Ensure the Rust router honours both.
    let (base, sched) = spawn_app().await;
    sched.register(manual_http_job("ts"));
    let a: Value = reqwest::get(format!("{base}/jobs"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let b: Value = reqwest::get(format!("{base}/jobs/"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(a, b, "trailing-slash variant must return identical body");
}

#[tokio::test]
async fn trailing_slash_running_route_matches_no_slash() {
    let (base, _sched) = spawn_app().await;
    let a: Value = reqwest::get(format!("{base}/running"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let b: Value = reqwest::get(format!("{base}/running/"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(a, b);
}

#[tokio::test]
async fn request_body_exceeding_limit_returns_413() {
    // `limits.max_request_bytes` is enforced via
    // `RequestBodyLimitLayer`. Any endpoint accepting a body
    // would trigger; a request over the cap must be rejected
    // before any handler sees it.
    use cronmanager::config::{AppConfig, Limits};

    let cfg = AppConfig {
        app_root_path: std::env::temp_dir(),
        limits: Limits {
            max_request_bytes: 128,
            ..Limits::default()
        },
        ..AppConfig::default()
    };
    let bundle = ExecutorBundle::new(&cfg, Arc::new(NoopRecorder)).unwrap();
    let scheduler = Scheduler::new(bundle);
    let state = AppState::new(Arc::new(cfg), scheduler);
    let app = router::build(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // 1 KiB body against a 128-byte cap.
    let payload = "x".repeat(1024);
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/execute/nosuch/nope"))
        .body(payload)
        .send()
        .await
        .unwrap();
    // tower_http's RequestBodyLimitLayer surfaces 413 for
    // Content-Length overshoot.
    assert_eq!(resp.status().as_u16(), 413);
}

#[tokio::test]
async fn double_execute_returns_409() {
    let (base, sched) = spawn_app().await;
    let spec = JobSpec {
        key: JobKey::new("test", "slow2"),
        trigger: Trigger::Manual,
        window: TimeWindow::default(),
        retry: RetryPolicy::default(),
        kind: JobKind::Exec {
            command: "/bin/sleep 3".into(),
            allowed_envs: vec![],
        },
    };
    sched.register(spec);

    reqwest::Client::new()
        .post(format!("{base}/execute/test/slow2"))
        .send()
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;

    let resp = reqwest::Client::new()
        .post(format!("{base}/execute/test/slow2"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 409);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "job_already_running");

    let _ = reqwest::Client::new()
        .post(format!("{base}/stop/test/slow2"))
        .send()
        .await;
}

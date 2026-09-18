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

// FLEET-STRONGHOLDS §1.6 — every response must carry the W3C
// traceparent + x-trace-id headers.
#[tokio::test]
async fn every_response_carries_traceparent_headers() {
    let (base, _sched) = spawn_app().await;
    let resp = reqwest::get(format!("{base}/health")).await.unwrap();
    let tp = resp
        .headers()
        .get("traceparent")
        .expect("traceparent header missing")
        .to_str()
        .unwrap()
        .to_string();
    let trace_id = resp
        .headers()
        .get("x-trace-id")
        .expect("x-trace-id header missing")
        .to_str()
        .unwrap()
        .to_string();
    // "00-<trace>-<span>-01" — 4 dash-separated segments.
    let parts: Vec<&str> = tp.split('-').collect();
    assert_eq!(parts.len(), 4, "malformed traceparent: {tp}");
    assert_eq!(parts[0], "00");
    assert_eq!(parts[1].len(), 32);
    assert_eq!(parts[2].len(), 16);
    // x-trace-id must equal the trace-id segment of traceparent.
    assert_eq!(trace_id, parts[1]);
}

#[tokio::test]
async fn traceparent_inherits_inbound_trace_id() {
    let (base, _sched) = spawn_app().await;
    let inbound = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";
    let resp = reqwest::Client::new()
        .get(format!("{base}/health"))
        .header("traceparent", inbound)
        .send()
        .await
        .unwrap();
    let x_trace = resp.headers().get("x-trace-id").unwrap().to_str().unwrap();
    assert_eq!(x_trace, "0af7651916cd43dd8448eb211c80319c");
    let tp = resp.headers().get("traceparent").unwrap().to_str().unwrap();
    // The trace-id segment must be preserved; the span-id may
    // differ (we regenerate per request).
    let parts: Vec<&str> = tp.split('-').collect();
    assert_eq!(parts[1], "0af7651916cd43dd8448eb211c80319c");
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

// h2ck.me RUNTIME-FINDINGS FN1 / PUBLIC-EXPOSURE F-CM-4:
// k8s livenessProbe defaults to `/healthz`; without this alias the
// probe hits the admin gate and receives 401, marking pods unhealthy
// in perpetuity. All three health URLs must return 200 unauth.
#[tokio::test]
async fn healthz_alias_returns_ok_unauthenticated() {
    let (base, _sched) = spawn_app().await;
    let resp = reqwest::get(format!("{base}/healthz")).await.unwrap();
    assert_eq!(resp.status(), 200, "/healthz must be public and 200");
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
}

#[tokio::test]
async fn actuator_health_alias_returns_ok_unauthenticated() {
    let (base, _sched) = spawn_app().await;
    let resp = reqwest::get(format!("{base}/actuator/health"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
}

// FLEET-STRONGHOLDS §5.1 — the five defense-in-depth security headers
// must be present on every response, regardless of route. Verifies
// the middleware wiring in src/router.rs::build.
#[tokio::test]
async fn every_response_carries_security_headers() {
    let (base, _sched) = spawn_app().await;
    for path in ["/health", "/", "/jobs", "/actuator/info"] {
        let resp = reqwest::get(format!("{base}{path}")).await.unwrap();
        for header in [
            "content-security-policy",
            "strict-transport-security",
            "x-frame-options",
            "x-content-type-options",
            "referrer-policy",
        ] {
            assert!(
                resp.headers().get(header).is_some(),
                "{path} response missing {header}",
            );
        }
        // Spot-check the canonical values so a regression that
        // silently blanks the header is caught.
        assert_eq!(
            resp.headers().get("x-frame-options").unwrap(),
            "DENY",
            "x-frame-options for {path}",
        );
        assert_eq!(
            resp.headers().get("x-content-type-options").unwrap(),
            "nosniff",
            "x-content-type-options for {path}",
        );
        assert_eq!(
            resp.headers().get("referrer-policy").unwrap(),
            "no-referrer",
            "referrer-policy for {path}",
        );
    }
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
            argv: vec!["/bin/sleep".into(), "2".into()],
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

// h2ck.me RUNTIME-FINDINGS v1 FN5 — slow-drip body upload used to
// hold a Tokio task past `limits.request_timeout_secs` because the
// body-read wasn't bounded. tower_http::TimeoutLayer now wraps the
// entire request/response future. The regression pin drives a raw
// TCP socket that promises a 100-byte body via Content-Length and
// then sends nothing — the server must close the connection within
// ~request_timeout_secs, not hang.
#[tokio::test]
async fn slow_body_upload_hits_request_deadline() {
    use cronmanager::config::{AppConfig, Limits};
    use std::time::{Duration, Instant};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let cfg = AppConfig {
        app_root_path: std::env::temp_dir(),
        limits: Limits {
            request_timeout_secs: 1,
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

    let mut sock = tokio::net::TcpStream::connect(addr).await.unwrap();
    // Promise a 100-byte body, send zero. If TimeoutLayer isn't
    // wired the read below hangs indefinitely and the test times
    // out at cargo's default 60s.
    let request = format!(
        "POST /execute/nosuch/nope HTTP/1.1\r\n\
         Host: {addr}\r\n\
         Content-Length: 100\r\n\
         Content-Type: text/plain\r\n\
         \r\n"
    );
    sock.write_all(request.as_bytes()).await.unwrap();
    sock.flush().await.unwrap();

    // Read whatever the server hands back (or EOF on connection
    // close). Bound at 5s — the TimeoutLayer fires at 1s so any
    // sane wiring finishes well before this.
    let start = Instant::now();
    let mut buf = Vec::with_capacity(1024);
    let read = tokio::time::timeout(Duration::from_secs(5), sock.read_to_end(&mut buf)).await;
    let elapsed = start.elapsed();
    assert!(
        read.is_ok(),
        "server never closed the slow-body connection — TimeoutLayer not wired"
    );
    assert!(
        elapsed < Duration::from_secs(4),
        "server took {}ms to close the slow-body connection — expected < ~1s + overhead",
        elapsed.as_millis()
    );
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
    // h2ck.me v1 T-15 / FLEET U10 — body must be structured JSON,
    // not the tower_http default "length limit exceeded" text.
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        ct.starts_with("application/json"),
        "413 content-type must be application/json, got: {ct}"
    );
    let body: Value = resp.json().await.expect("413 body must be valid JSON");
    assert_eq!(body["error"], "request_too_large");
    assert_eq!(body["limit"], 128);
    assert!(
        body["message"].as_str().unwrap_or("").contains("128"),
        "413 message must name the limit: {body}"
    );
}

// h2ck.me v1 T-17 / RFC 7231 §7.4.1 — a request whose path
// exists for a different method must be answered with 405
// Method Not Allowed and an `Allow:` header naming the valid
// method(s). Historically axum defaulted to 404 for the whole
// path miss; MethodRouter emits the correct 405 automatically
// when the path exists.
#[tokio::test]
async fn wrong_method_on_health_returns_405_with_allow_header() {
    let (base, _sched) = spawn_app().await;
    let resp = reqwest::Client::new()
        .post(format!("{base}/health"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 405, "POST /health must be 405");
    let allow = resp
        .headers()
        .get("allow")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        allow.split(',').any(|m| m.trim() == "GET"),
        "Allow header must include GET (was: {allow:?})"
    );
}

#[tokio::test]
async fn wrong_method_on_execute_returns_405() {
    let (base, _sched) = spawn_app().await;
    let resp = reqwest::Client::new()
        .get(format!("{base}/execute/g/j"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 405);
    let allow = resp
        .headers()
        .get("allow")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        allow.split(',').any(|m| m.trim() == "POST"),
        "Allow header must include POST (was: {allow:?})"
    );
}

#[tokio::test]
async fn unknown_path_still_returns_404() {
    let (base, _sched) = spawn_app().await;
    let resp = reqwest::Client::new()
        .get(format!("{base}/does-not-exist"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 404);
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
            argv: vec!["/bin/sleep".into(), "3".into()],
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

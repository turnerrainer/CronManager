//! End-to-end integration pin for `security.allowed_private_hosts`.
//!
//! Loads a real YAML config through `AppConfig`'s deserialiser,
//! constructs a real `ExecutorBundle`, fires an HTTP job at a
//! mockito server listening on `127.0.0.1`, and asserts both
//! wire-level reach and the posture-check contract. The unit
//! tests in `src/executor/http.rs::tests` and `src/security.rs::tests`
//! cover the matcher and the IP classifier in isolation; this file
//! bridges them so a future refactor that drops the wiring between
//! `SecurityConfig.allowed_private_hosts` and the `HttpExecutor`
//! builder is caught end-to-end.
//!
//! DEV-REQUIREMENTS §3.3 — every code seam gets an integration test.

use async_trait::async_trait;
use cronmanager::{
    config::AppConfig,
    dsl::{HttpHeader, JobKey, JobKind, JobSpec, RetryPolicy, TimeWindow, Trigger},
    env_safety::posture_checks_for,
    executor::{DispatchExtras, ExecutorBundle},
    history::{ExecutionStatus, HistoryEntry, HistoryRecorder},
};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

#[derive(Default)]
struct RecordingRecorder(Arc<Mutex<Vec<HistoryEntry>>>);

#[async_trait]
impl HistoryRecorder for RecordingRecorder {
    async fn record(&self, entry: HistoryEntry) {
        self.0.lock().unwrap().push(entry);
    }
}

fn http_job_against(url: &str) -> JobSpec {
    JobSpec {
        key: JobKey::new("integ", "allowlist"),
        trigger: Trigger::Manual,
        window: TimeWindow::default(),
        retry: RetryPolicy {
            count: 0,
            ..RetryPolicy::default()
        },
        kind: JobKind::Http {
            method: "GET".into(),
            url: url.into(),
            headers: Vec::<HttpHeader>::new(),
        },
    }
}

/// YAML config with an allowlisted hostname → real HTTP fire via
/// `localhost` reaches 127.0.0.1 even with `block_private_networks`
/// still true. The whole config → wire path is exercised.
#[tokio::test]
async fn allowlisted_host_fires_end_to_end_from_yaml() {
    let mut server = mockito::Server::new_async().await;
    let m = server
        .mock("GET", "/svc")
        .with_status(200)
        .with_body("ok")
        .create_async()
        .await;

    let yaml = concat!(
        "port: 8080\n",
        "security:\n",
        "  block_private_networks: true\n",
        "  allowed_private_hosts: [localhost]\n",
    );
    let cfg: AppConfig = serde_yaml_ng::from_str(yaml).expect("config parses");
    assert!(cfg.security.block_private_networks);
    assert_eq!(cfg.security.allowed_private_hosts, vec!["localhost"]);

    let rec = Arc::new(RecordingRecorder::default());
    let history: Arc<dyn HistoryRecorder> = rec.clone();
    let bundle = ExecutorBundle::new(&cfg, history).unwrap();

    let url = server.url().replace("127.0.0.1", "localhost") + "/svc";
    let outcome = bundle
        .dispatch(
            &http_job_against(&url),
            DispatchExtras::default(),
            Arc::new(Notify::new()),
        )
        .await;
    assert_eq!(outcome.status, ExecutionStatus::Success);
    m.assert_async().await;
}

/// Same config shape but the URL host is NOT on the allowlist →
/// fire is refused by the pre-flight. End-to-end counterpart to
/// the unit test `ssrf_pre_flight_refuses_private_hostname_not_on_allowlist`.
#[tokio::test]
async fn unlisted_private_host_is_still_refused_end_to_end() {
    let yaml = concat!(
        "port: 8080\n",
        "security:\n",
        "  allowed_private_hosts: [ruuter-internal]\n",
    );
    let cfg: AppConfig = serde_yaml_ng::from_str(yaml).unwrap();

    let rec = Arc::new(RecordingRecorder::default());
    let history: Arc<dyn HistoryRecorder> = rec.clone();
    let bundle = ExecutorBundle::new(&cfg, history).unwrap();

    let outcome = bundle
        .dispatch(
            &http_job_against("http://localhost:1/x"),
            DispatchExtras::default(),
            Arc::new(Notify::new()),
        )
        .await;
    // Pre-flight fails → attempt errors; history records failure.
    assert_ne!(outcome.status, ExecutionStatus::Success);
    let rows = rec.0.lock().unwrap();
    assert!(!rows.is_empty());
    let msg = rows[0].error_message.as_deref().unwrap_or("");
    assert!(
        msg.contains("SSRF pre-flight") && msg.contains("allowed_private_hosts"),
        "expected refusal with allowlist hint, got: {msg}"
    );
}

/// Production-posture contract: a Production `env_safety` sweep over
/// a config with the allowlist populated and `block_private_networks`
/// still true must report zero unsafe-posture items. This is the
/// core "allowlist unblocks multi-service deployments without
/// forcing APP_ENV=dev" guarantee from issue #49.
#[test]
fn allowlist_does_not_regress_production_env_safety() {
    let yaml = concat!(
        "port: 8080\n",
        "security:\n",
        "  block_private_networks: true\n",
        "  allowed_private_hosts: [ruuter-internal, svc.ns.svc.cluster.local]\n",
    );
    let cfg: AppConfig = serde_yaml_ng::from_str(yaml).unwrap();
    let checks = posture_checks_for(&cfg);
    let unsafe_names: Vec<&str> = checks
        .iter()
        .filter(|c| !c.is_safe)
        .map(|c| c.name)
        .collect();
    assert!(
        unsafe_names.is_empty(),
        "allowlist must not trip any posture check under Production, but these flagged: {unsafe_names:?}"
    );
}

/// Pentest end-to-end: a hand-rolled payload that tries to list the
/// AWS IMDS IP as an "approved" entry MUST be ignored by
/// normalisation (IP literal → None). The allowlist stays empty of
/// usable entries, so a URL hitting 169.254.169.254 is refused.
/// This is the main "did we really not create an IMDS backdoor?"
/// contract — pinned at the config layer, not just the unit tests.
#[tokio::test]
async fn ip_literal_in_allowlist_is_dropped_imds_stays_blocked() {
    let yaml = concat!(
        "port: 8080\n",
        "security:\n",
        "  allowed_private_hosts: ['169.254.169.254', '[fd00:ec2::254]']\n",
    );
    let cfg: AppConfig = serde_yaml_ng::from_str(yaml).unwrap();
    // Raw vector still contains both entries (we don't mutate the
    // config); the split is where the drop happens.
    assert_eq!(cfg.security.allowed_private_hosts.len(), 2);
    let (valid, invalid) = cfg.security.split_allowed_private_hosts();
    assert!(valid.is_empty(), "no entries should survive normalisation");
    assert_eq!(invalid.len(), 2);

    // Fire actually attempted: a URL at 169.254.169.254 is refused
    // because the allowlist is effectively empty.
    let rec = Arc::new(RecordingRecorder::default());
    let history: Arc<dyn HistoryRecorder> = rec.clone();
    let bundle = ExecutorBundle::new(&cfg, history).unwrap();
    let outcome = bundle
        .dispatch(
            // Literal-IP URL — DSL load would also refuse this, but
            // the direct JobSpec sidesteps the loader (as a /execute
            // override could). Pre-flight at fire time still kicks.
            &http_job_against("http://169.254.169.254/latest/meta-data"),
            DispatchExtras::default(),
            Arc::new(Notify::new()),
        )
        .await;
    assert_ne!(outcome.status, ExecutionStatus::Success);
}

/// Case + trailing-dot folding at the config layer: `RUUTER-INTERNAL.`
/// in the allowlist matches `ruuter-internal` in a URL. Pins
/// `normalize_allowed_host` + `is_allowlisted` collaborating as
/// documented.
#[tokio::test]
async fn allowlist_case_and_trailing_dot_fold_end_to_end() {
    let mut server = mockito::Server::new_async().await;
    let m = server
        .mock("GET", "/")
        .with_status(200)
        .create_async()
        .await;
    let yaml = concat!(
        "port: 8080\n",
        "security:\n",
        "  allowed_private_hosts: ['LocalHost.']\n",
    );
    let cfg: AppConfig = serde_yaml_ng::from_str(yaml).unwrap();
    let bundle = ExecutorBundle::new(
        &cfg,
        Arc::new(RecordingRecorder::default()) as Arc<dyn HistoryRecorder>,
    )
    .unwrap();

    let url = server.url().replace("127.0.0.1", "localhost") + "/";
    let outcome = bundle
        .dispatch(
            &http_job_against(&url),
            DispatchExtras::default(),
            Arc::new(Notify::new()),
        )
        .await;
    assert_eq!(outcome.status, ExecutionStatus::Success);
    m.assert_async().await;
}

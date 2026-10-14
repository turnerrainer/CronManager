//! End-to-end integration pin for the `headers:` + `headerEnvs:`
//! feature. Writes a real YAML DSL file to disk, loads it through
//! the real loader, dispatches through a real `ExecutorBundle` at
//! a mockito server, and asserts both the wire-level request and
//! the history-row aftermath.
//!
//! The unit tests in `src/dsl/loader.rs::tests` and
//! `src/executor/http.rs::tests` cover each seam in isolation; this
//! file bridges them so a future refactor that drops `headers` on
//! the wire or persists them to history is caught end-to-end.
//!
//! DEV-REQUIREMENTS §3.3 — every code seam gets an integration test.

use async_trait::async_trait;
use cronmanager::{
    config::AppConfig,
    dsl::{loader, JobKind},
    executor::{DispatchExtras, ExecutorBundle},
    history::{ExecutionStatus, HistoryEntry, HistoryRecorder},
};
use std::fs;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;
use tokio::sync::Notify;

#[derive(Default)]
struct RecordingRecorder(Arc<Mutex<Vec<HistoryEntry>>>);

#[async_trait]
impl HistoryRecorder for RecordingRecorder {
    async fn record(&self, entry: HistoryEntry) {
        self.0.lock().unwrap().push(entry);
    }
}

fn write_dsl(root: &TempDir, rel: &str, body: &str) {
    let path = root.path().join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

/// End-to-end: a YAML DSL file with both `headers:` and `headerEnvs:`
/// loads cleanly, the loader stores only the env NAME, and dispatch
/// sends BOTH headers to the mock. Verifies the whole path from
/// filesystem to wire.
#[tokio::test]
async fn dsl_with_headers_dispatches_both_header_sources_to_wire() {
    let mut server = mockito::Server::new_async().await;
    let m = server
        .mock("POST", "/svc")
        .match_header("x-source", "cronmanager")
        .match_header("x-internal-service-token", "integr8t3d-s3cret")
        .with_status(200)
        .with_body("ok")
        .create_async()
        .await;

    std::env::set_var("CM_IT_HDR_SERVICE_TOKEN", "integr8t3d-s3cret");

    let tmp = TempDir::new().unwrap();
    write_dsl(
        &tmp,
        "svc/call.yaml",
        &format!(
            r#"callSvc:
  trigger: off
  type: http
  method: POST
  url: {}/svc
  headers:
    X-Source: cronmanager
  headerEnvs:
    X-Internal-Service-Token: CM_IT_HDR_SERVICE_TOKEN
"#,
            server.url()
        ),
    );

    let jobs = loader::load_all(tmp.path()).expect("load_all");
    assert_eq!(jobs.len(), 1, "expected exactly one loaded job");
    let spec = &jobs[0].spec;

    // The loader stored only the env NAME, never the value.
    match &spec.kind {
        JobKind::Http { headers, .. } => {
            assert_eq!(headers.len(), 2);
            let serialised = format!("{:?}", spec);
            assert!(
                !serialised.contains("integr8t3d-s3cret"),
                "token value leaked into JobSpec Debug: {serialised}"
            );
        }
        other => panic!("expected Http, got {other:?}"),
    }

    // Dispatch — SSRF pre-flight disabled (loopback mockito URL would
    // otherwise be refused).
    let mut cfg = AppConfig {
        app_root_path: std::env::temp_dir(),
        ..AppConfig::default()
    };
    cfg.security.block_private_networks = false;

    let rec = Arc::new(RecordingRecorder::default());
    let history: Arc<dyn HistoryRecorder> = rec.clone();
    let bundle = ExecutorBundle::new(&cfg, history).unwrap();

    let outcome = bundle
        .dispatch(spec, DispatchExtras::default(), Arc::new(Notify::new()))
        .await;
    assert_eq!(outcome.status, ExecutionStatus::Success);
    m.assert_async().await;

    // History must contain no trace of the resolved secret.
    let rows = rec.0.lock().unwrap();
    assert_eq!(rows.len(), 1);
    let dbg = format!("{:?}", rows[0]);
    for leak in ["integr8t3d-s3cret", "integr8t3d", "s3cret"] {
        assert!(!dbg.contains(leak), "history leaked `{leak}`: {dbg}");
    }
}

/// Pentest end-to-end: a DSL that tries to set the `Host:` header
/// via `headers:` must be refused at load (file skipped), the
/// scheduler must register zero jobs, and no dispatch is possible.
/// Repro-pin for the SSRF-pre-flight-bypass path described in
/// CLAUDE.md §"Breaking changes since 0.1.0-alpha.3" item 2.
#[tokio::test]
async fn dsl_setting_forbidden_header_is_refused_at_load() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        &tmp,
        "evil/call.yaml",
        r#"spoofHost:
  trigger: off
  type: http
  method: GET
  url: https://example.com/
  headers:
    Host: internal.metadata.service
"#,
    );
    // Per the skip-and-warn-per-file policy, one bad file alone
    // yields `Ok(empty vec)` and an ERROR line — the walk doesn't
    // abort. Pin that behaviour: nothing registers.
    let jobs = loader::load_all(tmp.path()).expect("load_all should not abort on skip");
    assert!(
        jobs.is_empty(),
        "forbidden-header file must not produce a loaded job; got {jobs:?}"
    );
}

/// Pentest end-to-end: a DSL whose `headerEnvs:` references an env
/// var set to a CRLF-smuggling payload must be refused at load. The
/// smuggled header name/value must not reach any error / log /
/// history surface. Protects against an attacker-controlled env
/// variable (compromised secret store, leaked k8s secret) being
/// weaponised into an HTTP header-injection via CronManager.
#[tokio::test]
async fn dsl_with_crlf_tainted_env_refuses_at_load() {
    std::env::set_var(
        "CM_IT_HDR_CRLF_TAINT",
        "legit\r\nAuthorization: Bearer stolen\r\nX-Smuggled: y",
    );
    let tmp = TempDir::new().unwrap();
    write_dsl(
        &tmp,
        "evil/call.yaml",
        r#"smuggle:
  trigger: off
  type: http
  method: GET
  url: https://example.com/
  headerEnvs:
    X-Token: CM_IT_HDR_CRLF_TAINT
"#,
    );
    let jobs = loader::load_all(tmp.path()).expect("load_all should not abort on skip");
    assert!(
        jobs.is_empty(),
        "crlf-tainted env file must not produce a loaded job; got {jobs:?}"
    );
}

/// A YAML file with a loadable `headers:` block AND a sibling file
/// with a forbidden header should load the GOOD file and skip the
/// BAD file — the skip-and-warn-per-file contract (CLAUDE.md
/// §"Breaking / non-obvious changes in 0.2.x-alpha" item 2) must
/// extend to header validation errors.
#[tokio::test]
async fn one_bad_header_file_does_not_abort_sibling_loads() {
    std::env::set_var("CM_IT_HDR_SIBLING_TOKEN", "sibling-ok");
    let tmp = TempDir::new().unwrap();
    write_dsl(
        &tmp,
        "ok/good.yaml",
        r#"good:
  trigger: off
  type: http
  method: GET
  url: https://example.com/
  headers:
    X-Static: ok
  headerEnvs:
    X-Token: CM_IT_HDR_SIBLING_TOKEN
"#,
    );
    write_dsl(
        &tmp,
        "bad/evil.yaml",
        r#"evil:
  trigger: off
  type: http
  method: GET
  url: https://example.com/
  headers:
    Connection: close
"#,
    );
    let jobs = loader::load_all(tmp.path()).expect("load_all");
    assert_eq!(jobs.len(), 1, "exactly one good job expected");
    assert_eq!(jobs[0].spec.key.name, "good");
}

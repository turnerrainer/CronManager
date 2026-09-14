//! Integration test — walk the *real* shipped `DSL/samples/`
//! tree and confirm every file parses cleanly with the group
//! names an operator would see.
//!
//! DEV-REQUIREMENTS §3: real fixtures beat hand-rolled ones.

use cronmanager::dsl::loader::{self, LoadPolicy};
use cronmanager::dsl::{JobKind, Trigger};
use cronmanager::env_safety::{DevFixtureAction, Environment};
use std::path::PathBuf;

fn samples_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("DSL/samples")
}

#[test]
fn shipped_samples_all_load() {
    let jobs = loader::load_all(&samples_root()).expect("load_all failed");
    assert!(
        jobs.len() >= 5,
        "expected the shipped sample DSLs to yield at least 5 jobs, got {}",
        jobs.len()
    );
}

#[test]
fn shipped_samples_have_valid_group_prefix() {
    let jobs = loader::load_all(&samples_root()).unwrap();
    for j in &jobs {
        assert!(
            !j.spec.key.group.is_empty(),
            "empty group for {}",
            j.spec.key.name
        );
        assert!(
            !j.spec.key.name.is_empty(),
            "empty name in file {}",
            j.source_path.display()
        );
    }
}

#[test]
fn manual_trigger_off_parses_to_manual() {
    let jobs = loader::load_all(&samples_root()).unwrap();
    let manual_jobs: Vec<_> = jobs
        .iter()
        .filter(|j| j.spec.trigger == Trigger::Manual)
        .collect();
    assert!(
        !manual_jobs.is_empty(),
        "expected at least one shipped sample with `trigger: off`"
    );
}

#[test]
fn every_http_sample_has_supported_method() {
    let jobs = loader::load_all(&samples_root()).unwrap();
    for j in &jobs {
        if let JobKind::Http { method, .. } = &j.spec.kind {
            let ok = matches!(
                method.to_ascii_uppercase().as_str(),
                "GET" | "POST" | "PUT" | "DELETE" | "PATCH" | "HEAD" | "OPTIONS"
            );
            assert!(
                ok,
                "unsupported HTTP method in shipped sample {}: {}",
                j.source_path.display(),
                method
            );
        }
    }
}

#[test]
fn every_cron_sample_parses_as_a_schedule() {
    use std::str::FromStr;
    let jobs = loader::load_all(&samples_root()).unwrap();
    for j in &jobs {
        if let Trigger::Cron(expr) = &j.spec.trigger {
            cron::Schedule::from_str(expr).unwrap_or_else(|e| {
                panic!(
                    "cron expression '{}' in {} rejected: {e}",
                    expr,
                    j.source_path.display()
                )
            });
        }
    }
}

// ---------- FLEET-STRONGHOLDS §11.3 dev-fixture DSL gate ----------
//
// Belt-and-braces: the shipped `DSL/samples/` tree must NEVER trip
// the dev-fixture pattern list — otherwise a first-time operator
// bringing up CronManager in staging/prod with the default DSL
// path (which points at `samples/` inside the image) would get a
// hard boot failure. The unit tests in `env_safety::tests` pin the
// pattern list; this test pins the shipped tree.

#[test]
fn u18_shipped_samples_tree_loads_under_refuse_action() {
    // Simulate a prod boot: `Refuse` action + strict-ish policy.
    // Every shipped sample must load.
    let policy = LoadPolicy {
        max_file_bytes: 1024 * 1024,
        max_retry_count: 10,
        min_cron_interval_secs: 0,
        block_private_networks: false,
        dev_fixture_action: DevFixtureAction::Refuse,
    };
    let jobs = loader::load_all_with_policy(&samples_root(), &policy)
        .expect("shipped samples must load under prod-strict LoadPolicy");
    assert!(!jobs.is_empty(), "expected shipped samples to yield jobs");
}

#[test]
fn u18_dev_fixture_file_refused_in_prod() {
    use tempfile::TempDir;
    let tmp = TempDir::new().unwrap();
    // Put a legit job AND a dev-fixture file side-by-side.
    std::fs::create_dir_all(tmp.path().join("prod")).unwrap();
    std::fs::create_dir_all(tmp.path().join("mock-platform")).unwrap();
    std::fs::write(
        tmp.path().join("prod/health.yaml"),
        "hc:\n  trigger: off\n  type: http\n  method: GET\n  url: https://example.com/\n",
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("mock-platform/health.yaml"),
        "hc:\n  trigger: off\n  type: http\n  method: GET\n  url: https://example.com/\n",
    )
    .unwrap();
    // Simulate a Production boot: allow_dev_fixture_dsls=false.
    let action = DevFixtureAction::from_env(Environment::Production, false);
    assert_eq!(action, DevFixtureAction::Refuse);
    let policy = LoadPolicy {
        max_file_bytes: 1024 * 1024,
        max_retry_count: 10,
        min_cron_interval_secs: 0,
        block_private_networks: false,
        dev_fixture_action: action,
    };
    let err = loader::load_all_with_policy(tmp.path(), &policy).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("REFUSING TO LOAD"), "err: {msg}");
    assert!(msg.contains("mock-"), "err: {msg}");
}

#[test]
fn u18_dev_fixture_file_permitted_in_dev_with_warn() {
    use tempfile::TempDir;
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join("mock-platform")).unwrap();
    std::fs::write(
        tmp.path().join("mock-platform/health.yaml"),
        "hc:\n  trigger: off\n  type: http\n  method: GET\n  url: https://example.com/\n",
    )
    .unwrap();
    // Simulate a Dev boot: action collapses to Warn.
    let action = DevFixtureAction::from_env(Environment::Dev, false);
    assert_eq!(action, DevFixtureAction::Warn);
    let policy = LoadPolicy {
        max_file_bytes: 1024 * 1024,
        max_retry_count: 10,
        min_cron_interval_secs: 0,
        block_private_networks: false,
        dev_fixture_action: action,
    };
    let jobs = loader::load_all_with_policy(tmp.path(), &policy).unwrap();
    assert_eq!(jobs.len(), 1);
}

#[test]
fn u18_dev_fixture_file_permitted_in_prod_when_opted_out() {
    use tempfile::TempDir;
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join("mock-platform")).unwrap();
    std::fs::write(
        tmp.path().join("mock-platform/health.yaml"),
        "hc:\n  trigger: off\n  type: http\n  method: GET\n  url: https://example.com/\n",
    )
    .unwrap();
    // Simulate Prod boot with allow_dev_fixture_dsls=true.
    let action = DevFixtureAction::from_env(Environment::Production, true);
    assert_eq!(action, DevFixtureAction::Warn);
    let policy = LoadPolicy {
        max_file_bytes: 1024 * 1024,
        max_retry_count: 10,
        min_cron_interval_secs: 0,
        block_private_networks: false,
        dev_fixture_action: action,
    };
    let jobs = loader::load_all_with_policy(tmp.path(), &policy).unwrap();
    assert_eq!(jobs.len(), 1);
}

#[test]
fn u18_first_dev_fixture_match_is_fatal_even_after_good_files() {
    // Break-the-fix: if a good file loads first, then a
    // dev-fixture file is encountered, the walk must still abort
    // with the REFUSING TO LOAD error. Not partial success.
    use tempfile::TempDir;
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join("aa-prod")).unwrap();
    std::fs::create_dir_all(tmp.path().join("zz-mock-late")).unwrap();
    std::fs::write(
        tmp.path().join("aa-prod/health.yaml"),
        "hc:\n  trigger: off\n  type: http\n  method: GET\n  url: https://example.com/\n",
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("zz-mock-late/probe.yaml"),
        "p:\n  trigger: off\n  type: http\n  method: GET\n  url: https://example.com/\n",
    )
    .unwrap();
    let action = DevFixtureAction::from_env(Environment::Production, false);
    let policy = LoadPolicy {
        max_file_bytes: 1024 * 1024,
        max_retry_count: 10,
        min_cron_interval_secs: 0,
        block_private_networks: false,
        dev_fixture_action: action,
    };
    let err = loader::load_all_with_policy(tmp.path(), &policy).unwrap_err();
    assert!(err.to_string().contains("REFUSING TO LOAD"));
}

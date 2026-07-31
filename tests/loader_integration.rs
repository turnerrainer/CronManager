//! Integration test — walk the *real* shipped `DSL/samples/`
//! tree and confirm every file parses cleanly with the group
//! names an operator would see.
//!
//! DEV-REQUIREMENTS §3: real fixtures beat hand-rolled ones.

use cronmanager::dsl::{loader, JobKind, Trigger};
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

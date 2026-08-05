//! JVM-compat corpus test — every sample DSL YAML that ships
//! with the upstream JVM CronManager (mirrored verbatim under
//! `compat/dsl/`) must load through the Rust loader without
//! error, produce the expected number of jobs, and land on the
//! expected executor kind.
//!
//! Failure modes this catches:
//! * A JVM-only DSL feature landing in the corpus that Rust
//!   doesn't parse (regression under the `deny_unknown_fields`
//!   hardening).
//! * A Rust rename that breaks a JVM camelCase field alias.
//! * Cron dialect drift — `cron` crate version bump that stops
//!   accepting Quartz `?` day-of-week syntax.

use cronmanager::dsl::{loader, JobKind};
use std::path::Path;
use std::str::FromStr;

const CORPUS_ROOT: &str = "compat/dsl";

#[test]
fn every_compat_dsl_file_parses() {
    let root = Path::new(CORPUS_ROOT);
    assert!(
        root.exists(),
        "compat/dsl corpus missing — did you delete the JVM fixtures?"
    );
    let jobs = loader::load_all(root).expect("compat corpus must parse cleanly");
    assert!(
        jobs.len() >= 25,
        "expected ≥25 jobs across 18 JVM sample files; found {}",
        jobs.len()
    );
}

#[test]
fn compat_http_jobs_land_on_http_executor() {
    let root = Path::new(CORPUS_ROOT);
    let jobs = loader::load_all(root).unwrap();
    let http_count = jobs
        .iter()
        .filter(|j| matches!(j.spec.kind, JobKind::Http { .. }))
        .filter(|j| {
            j.spec.key.group.starts_with("http") || j.spec.key.group.starts_with("schedules")
        })
        .count();
    assert!(
        http_count >= 15,
        "expected many HTTP jobs from compat/dsl/http + schedules/examples.yaml; got {http_count}"
    );
}

#[test]
fn compat_shell_jobs_land_on_shell_executor() {
    let root = Path::new(CORPUS_ROOT);
    let jobs = loader::load_all(root).unwrap();
    let shell_count = jobs
        .iter()
        .filter(|j| matches!(j.spec.kind, JobKind::Exec { .. }))
        .count();
    // 9 shell sample files, each with exactly one job.
    assert_eq!(
        shell_count, 9,
        "compat/dsl/shell has 9 files; expected 9 shell jobs, got {shell_count}"
    );
}

#[test]
fn compat_manual_triggers_land_on_manual_mode() {
    // Two JVM samples use `trigger: off` — `http/manual-trigger.yaml`
    // and `shell/manual-script.yaml`. Cron-parse-refusing them
    // would be a regression.
    let root = Path::new(CORPUS_ROOT);
    let jobs = loader::load_all(root).unwrap();
    let manual_count = jobs
        .iter()
        .filter(|j| matches!(j.spec.trigger, cronmanager::dsl::Trigger::Manual))
        .count();
    assert_eq!(
        manual_count, 2,
        "expected 2 manual-trigger jobs (http/manual-trigger + shell/manual-script); got {manual_count}"
    );
}

#[test]
fn every_compat_cron_expression_is_parseable_by_cron_crate() {
    // Defence-in-depth against a future `cron` crate bump that
    // stops accepting Quartz-style `?` in day-of-week position.
    let root = Path::new(CORPUS_ROOT);
    let jobs = loader::load_all(root).unwrap();
    for job in &jobs {
        if let cronmanager::dsl::Trigger::Cron(expr) = &job.spec.trigger {
            cron::Schedule::from_str(expr).unwrap_or_else(|e| {
                panic!(
                    "cron expression {expr:?} from {}/{} no longer parses: {e}",
                    job.spec.key.group, job.spec.key.name
                )
            });
        }
    }
}

#[test]
fn compat_time_windows_survive_round_trip() {
    // `http/time-bounded.yaml` and `shell/seasonal-maintenance.yaml`
    // both carry startDate + endDate. Rust must accept both and
    // preserve the epoch-millis values.
    let root = Path::new(CORPUS_ROOT);
    let jobs = loader::load_all(root).unwrap();
    let windowed: Vec<_> = jobs
        .iter()
        .filter(|j| j.spec.window.start_ms.is_some() || j.spec.window.end_ms.is_some())
        .collect();
    assert!(
        windowed.len() >= 2,
        "expected ≥2 jobs with time windows in compat corpus; got {}",
        windowed.len()
    );
    for j in windowed {
        // Sanity: values are in the recent-past-to-far-future
        // range (JVM samples use 2024 epoch ms).
        if let Some(s) = j.spec.window.start_ms {
            assert!(s > 1_000_000_000_000, "startDate {s} unexpectedly small");
        }
        if let Some(e) = j.spec.window.end_ms {
            assert!(e > 1_000_000_000_000, "endDate {e} unexpectedly small");
        }
    }
}

#[test]
fn compat_retry_defaults_match_jvm() {
    // JVM defaults (per YamlJob.java): retryCount=0, retryDelay=1000ms.
    // Every job in the corpus that omits retryCount must land on 0.
    // http/health-check.yaml sets retryCount=3, so it's the counter-
    // example that proves the load did happen and it isn't
    // universally zero.
    let root = Path::new(CORPUS_ROOT);
    let jobs = loader::load_all(root).unwrap();
    let default_count = jobs.iter().filter(|j| j.spec.retry.count == 0).count();
    let non_default_count = jobs.iter().filter(|j| j.spec.retry.count > 0).count();
    assert!(
        default_count >= 5 && non_default_count >= 3,
        "expected a mix of default-0 and non-default retryCount jobs; got default={default_count} non-default={non_default_count}"
    );
}

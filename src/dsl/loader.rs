//! DSL loader — walks `dsl_path`, parses every `.yaml`/`.yml`
//! file, validates each job's cron expression, produces a list
//! of `LoadedJob`s.
//!
//! Group name derivation matches JVM CronManager:
//! `DSL/samples/http/health-check.yaml` under `dsl_path = DSL`
//! → group `samples-http-health-check`.

use crate::dsl::{JobKey, JobKind, JobSpec, LoadedJob, RetryPolicy, TimeWindow, Trigger};
use crate::error::CronManagerError;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

/// Read every YAML file under `root`, parse them, validate each
/// job's cron expression at load time (fail-fast per DEV-
/// REQUIREMENTS §2 "no unwrap()/expect() in library code").
pub fn load_all(root: &Path) -> Result<Vec<LoadedJob>, CronManagerError> {
    let mut out = Vec::new();
    if !root.exists() {
        tracing::warn!(
            "dsl_path {} does not exist; no jobs will be loaded",
            root.display()
        );
        return Ok(out);
    }
    walk(root, root, &mut out)?;
    tracing::info!("loaded {} job(s) from {}", out.len(), root.display());
    Ok(out)
}

fn walk(root: &Path, current: &Path, out: &mut Vec<LoadedJob>) -> Result<(), CronManagerError> {
    let entries = std::fs::read_dir(current)?;
    // Sort for deterministic load order — same DSL directory
    // always produces the same job list, which matters for
    // logging + fixture stability in tests.
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            walk(root, &path, out)?;
        } else if is_yaml(&path) {
            for loaded in load_file(root, &path)? {
                out.push(loaded);
            }
        }
    }
    Ok(())
}

/// Parse one YAML file into zero or more `LoadedJob`s. The file
/// is a top-level mapping from job name to job definition.
pub fn load_file(root: &Path, file: &Path) -> Result<Vec<LoadedJob>, CronManagerError> {
    let body = std::fs::read_to_string(file)?;
    let raw: BTreeMap<String, RawJob> =
        serde_yaml_ng::from_str(&body).map_err(|e| CronManagerError::YamlParse {
            path: file.display().to_string(),
            source: e,
        })?;
    let group = group_from_path(root, file);
    let mut out = Vec::with_capacity(raw.len());
    for (name, raw_job) in raw {
        let spec = raw_job.into_spec(JobKey::new(group.clone(), name), file)?;
        out.push(LoadedJob {
            spec,
            source_path: file.to_path_buf(),
        });
    }
    Ok(out)
}

/// Derive the group name from the file path. Matches JVM
/// `JobReaderService.pathToGroupName`.
pub fn group_from_path(root: &Path, file: &Path) -> String {
    let rel = file.strip_prefix(root).unwrap_or(file);
    let mut s = rel.with_extension("").to_string_lossy().into_owned();
    // Normalize both separators (Windows tolerance) and collapse
    // any accidental leading slash.
    s = s.replace(['\\', '/'], "-");
    s.trim_matches('-').to_string()
}

fn is_yaml(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|s| s.to_str()),
        Some("yaml") | Some("yml")
    )
}

/// YAML value shape as-loaded. The `#[serde(untagged)]` on
/// `RawTrigger` accepts both `trigger: "0 * * * * ?"` (string)
/// and `trigger: false` (bool). YAML 1.2 treats `off`/`no`/`yes`/`on`
/// as strings, not booleans; we normalise the strings too.
#[derive(Debug, Deserialize)]
struct RawJob {
    trigger: RawTrigger,
    #[serde(rename = "type")]
    ty: String,

    #[serde(default, rename = "startDate")]
    start_date: Option<i64>,
    #[serde(default, rename = "endDate")]
    end_date: Option<i64>,

    #[serde(default, rename = "retryCount")]
    retry_count: Option<u32>,
    #[serde(default, rename = "retryDelay")]
    retry_delay_ms: Option<u64>,
    #[serde(default, rename = "ignoreFailures")]
    ignore_failures: Option<bool>,

    // HTTP fields
    method: Option<String>,
    url: Option<String>,

    // Shell fields
    command: Option<String>,
    #[serde(default, rename = "allowedEnvs")]
    allowed_envs: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawTrigger {
    Bool(bool),
    Str(String),
}

impl RawJob {
    fn into_spec(self, key: JobKey, source: &Path) -> Result<JobSpec, CronManagerError> {
        let trigger = normalise_trigger(&self.trigger, source)?;
        // Validate cron at load time so a typo becomes a startup
        // error, not a first-fire crash. DEV-REQUIREMENTS §13.4.
        if let Trigger::Cron(expr) = &trigger {
            cron::Schedule::from_str(expr).map_err(|e| CronManagerError::InvalidCron {
                expression: expr.clone(),
                reason: e.to_string(),
            })?;
        }
        let window = TimeWindow {
            start_ms: self.start_date,
            end_ms: self.end_date,
        };
        let retry = RetryPolicy {
            count: self.retry_count.unwrap_or(0),
            delay: Duration::from_millis(self.retry_delay_ms.unwrap_or(1000)),
            ignore_failures: self.ignore_failures.unwrap_or(false),
        };
        let kind = match self.ty.as_str() {
            "http" => {
                let method = self
                    .method
                    .ok_or_else(|| CronManagerError::InvalidJobDefinition {
                        source_path: source.display().to_string(),
                        reason: format!("job '{}' has type: http but no method field", key.name),
                    })?;
                let url = self
                    .url
                    .ok_or_else(|| CronManagerError::InvalidJobDefinition {
                        source_path: source.display().to_string(),
                        reason: format!("job '{}' has type: http but no url field", key.name),
                    })?;
                // Validate method against the JVM-supported set.
                // reqwest::Method::from_bytes accepts any RFC-7230
                // token, so a typo like `FLOOP` would pass its
                // check but fail at first fire.
                const KNOWN_METHODS: &[&str] =
                    &["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS"];
                if !KNOWN_METHODS.contains(&method.to_ascii_uppercase().as_str()) {
                    return Err(CronManagerError::InvalidJobDefinition {
                        source_path: source.display().to_string(),
                        reason: format!("job '{}' has unknown HTTP method '{}'", key.name, method),
                    });
                }
                JobKind::Http { method, url }
            }
            "exec" => {
                let command =
                    self.command
                        .ok_or_else(|| CronManagerError::InvalidJobDefinition {
                            source_path: source.display().to_string(),
                            reason: format!(
                                "job '{}' has type: exec but no command field",
                                key.name
                            ),
                        })?;
                JobKind::Exec {
                    command,
                    allowed_envs: self.allowed_envs.unwrap_or_default(),
                }
            }
            other => {
                return Err(CronManagerError::InvalidJobDefinition {
                    source_path: source.display().to_string(),
                    reason: format!("job '{}' has unknown type: '{}'", key.name, other),
                });
            }
        };
        Ok(JobSpec {
            key,
            trigger,
            window,
            retry,
            kind,
        })
    }
}

fn normalise_trigger(raw: &RawTrigger, source: &Path) -> Result<Trigger, CronManagerError> {
    match raw {
        RawTrigger::Bool(false) => Ok(Trigger::Manual),
        RawTrigger::Bool(true) => Err(CronManagerError::InvalidJobDefinition {
            source_path: source.display().to_string(),
            reason: "trigger: true is not a valid schedule; use a cron string or `off`".into(),
        }),
        RawTrigger::Str(s) => match s.to_ascii_lowercase().as_str() {
            "off" | "false" | "no" | "disabled" => Ok(Trigger::Manual),
            _ => Ok(Trigger::Cron(s.clone())),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(root: &Path, rel: &str, body: &str) {
        let full = root.join(rel);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, body).unwrap();
    }

    #[test]
    fn group_derivation_matches_jvm_convention() {
        let root = Path::new("/dsl");
        let file = Path::new("/dsl/samples/http/health-check.yaml");
        assert_eq!(group_from_path(root, file), "samples-http-health-check");
    }

    #[test]
    fn group_derivation_flat_file() {
        let root = Path::new("/dsl");
        let file = Path::new("/dsl/simple.yml");
        assert_eq!(group_from_path(root, file), "simple");
    }

    #[test]
    fn loads_http_job_with_all_fields() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "http/health.yaml",
            "hc:\n  trigger: \"0 */5 * * * ?\"\n  type: http\n  method: GET\n  url: https://example.com\n  retryCount: 2\n  retryDelay: 500\n  ignoreFailures: true\n  startDate: 1000\n  endDate: 2000\n",
        );
        let jobs = load_all(tmp.path()).unwrap();
        assert_eq!(jobs.len(), 1);
        let spec = &jobs[0].spec;
        assert_eq!(spec.key.name, "hc");
        assert_eq!(spec.key.group, "http-health");
        assert!(matches!(&spec.trigger, Trigger::Cron(s) if s == "0 */5 * * * ?"));
        assert!(matches!(&spec.kind, JobKind::Http { method, url }
            if method == "GET" && url == "https://example.com"));
        assert_eq!(spec.retry.count, 2);
        assert_eq!(spec.retry.delay, Duration::from_millis(500));
        assert!(spec.retry.ignore_failures);
        assert_eq!(spec.window.start_ms, Some(1000));
        assert_eq!(spec.window.end_ms, Some(2000));
    }

    #[test]
    fn manual_trigger_string_off() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "http/m.yaml",
            "m:\n  trigger: off\n  type: http\n  method: GET\n  url: https://x\n",
        );
        let jobs = load_all(tmp.path()).unwrap();
        assert_eq!(jobs[0].spec.trigger, Trigger::Manual);
    }

    #[test]
    fn manual_trigger_bool_false() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "http/m.yaml",
            "m:\n  trigger: false\n  type: http\n  method: GET\n  url: https://x\n",
        );
        let jobs = load_all(tmp.path()).unwrap();
        assert_eq!(jobs[0].spec.trigger, Trigger::Manual);
    }

    #[test]
    fn manual_trigger_string_false_and_no() {
        for lit in ["false", "no", "disabled"] {
            let tmp = TempDir::new().unwrap();
            let body = format!(
                "m:\n  trigger: \"{lit}\"\n  type: http\n  method: GET\n  url: https://x\n"
            );
            write(tmp.path(), "http/m.yaml", &body);
            let jobs = load_all(tmp.path()).unwrap();
            assert_eq!(jobs[0].spec.trigger, Trigger::Manual, "lit={lit}");
        }
    }

    #[test]
    fn bad_cron_expression_fails_load() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "http/bad.yaml",
            "b:\n  trigger: \"not a cron\"\n  type: http\n  method: GET\n  url: https://x\n",
        );
        let err = load_all(tmp.path()).unwrap_err();
        assert!(matches!(err, CronManagerError::InvalidCron { .. }));
    }

    #[test]
    fn unknown_http_method_fails_load() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "http/bad.yaml",
            "b:\n  trigger: off\n  type: http\n  method: FLOOP\n  url: https://x\n",
        );
        let err = load_all(tmp.path()).unwrap_err();
        assert!(matches!(err, CronManagerError::InvalidJobDefinition { .. }));
    }

    #[test]
    fn unknown_job_type_fails_load() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "x.yaml",
            "b:\n  trigger: off\n  type: telnet\n  method: GET\n  url: https://x\n",
        );
        let err = load_all(tmp.path()).unwrap_err();
        assert!(matches!(err, CronManagerError::InvalidJobDefinition { .. }));
    }

    #[test]
    fn missing_dsl_path_is_warning_not_error() {
        let jobs = load_all(Path::new("/does/not/exist/anywhere")).unwrap();
        assert!(jobs.is_empty());
    }

    #[test]
    fn time_window_allows() {
        let win = TimeWindow {
            start_ms: Some(100),
            end_ms: Some(200),
        };
        assert!(!win.allows(99));
        assert!(win.allows(100));
        assert!(win.allows(150));
        assert!(win.allows(200));
        assert!(!win.allows(201));
    }

    #[test]
    fn time_window_open_ended() {
        let start_only = TimeWindow {
            start_ms: Some(100),
            end_ms: None,
        };
        assert!(!start_only.allows(50));
        assert!(start_only.allows(100));
        assert!(start_only.allows(i64::MAX));

        let end_only = TimeWindow {
            start_ms: None,
            end_ms: Some(100),
        };
        assert!(end_only.allows(i64::MIN));
        assert!(end_only.allows(100));
        assert!(!end_only.allows(101));

        let unbounded = TimeWindow::default();
        assert!(unbounded.allows(i64::MIN));
        assert!(unbounded.allows(i64::MAX));
    }
}

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

/// Per-load hardening knobs. Callers construct one of these from
/// `SecurityConfig` and pass it through — putting the caps on the
/// signature keeps the loader deterministic and unit-testable.
#[derive(Debug, Clone, Copy)]
pub struct LoadPolicy {
    pub max_file_bytes: u64,
    pub max_retry_count: u32,
    pub min_cron_interval_secs: i64,
    pub block_private_networks: bool,
}

impl LoadPolicy {
    /// Test/legacy default — high enough to be transparent to the
    /// existing test corpus. Production callers should build via
    /// `SecurityConfig` in main.rs / the /reload handler.
    pub fn permissive() -> Self {
        Self {
            max_file_bytes: 8 * 1024 * 1024,
            max_retry_count: u32::MAX,
            min_cron_interval_secs: 0,
            block_private_networks: false,
        }
    }
}

/// Sync legacy entry point — kept because a handful of unit tests
/// call it directly. Uses [`LoadPolicy::permissive`] and does NOT
/// enforce the runtime hardening knobs. Production code paths
/// (main.rs, the /reload handler) should always go through
/// [`load_all_bounded`].
pub fn load_all(root: &Path) -> Result<Vec<LoadedJob>, CronManagerError> {
    load_all_with_policy(root, &LoadPolicy::permissive())
}

/// Bounded load: applies per-file size cap, retry cap, cron freq
/// WARN, and SSRF pre-flight on every HTTP job URL. Wraps the
/// whole traversal in a wall-clock timeout so a pathological YAML
/// parser bug (or a very deep directory) can't pin the runtime.
///
/// The load itself is I/O-heavy and synchronous under the hood —
/// we hand it to `spawn_blocking` so the timeout is a real
/// wall-clock bound, not a courtesy await point.
pub async fn load_all_bounded(
    root: &Path,
    max_file_bytes: u64,
    load_timeout: Duration,
    max_retry_count: u32,
    min_cron_interval_secs: i64,
    block_private_networks: bool,
) -> Result<Vec<LoadedJob>, CronManagerError> {
    let policy = LoadPolicy {
        max_file_bytes,
        max_retry_count,
        min_cron_interval_secs,
        block_private_networks,
    };
    let root = root.to_path_buf();
    let handle = tokio::task::spawn_blocking(move || load_all_with_policy(&root, &policy));
    match tokio::time::timeout(load_timeout, handle).await {
        Ok(join_result) => join_result
            .map_err(|e| CronManagerError::Internal(format!("dsl load task panicked: {e}")))?,
        Err(_) => Err(CronManagerError::Internal(format!(
            "dsl load exceeded {}s wall clock — investigate DSL file sizes / anchor use",
            load_timeout.as_secs()
        ))),
    }
}

/// Shared traversal. Every caller flows through here so the
/// hardening rules are enforced exactly once and identically.
pub fn load_all_with_policy(
    root: &Path,
    policy: &LoadPolicy,
) -> Result<Vec<LoadedJob>, CronManagerError> {
    let mut out = Vec::new();
    if !root.exists() {
        tracing::warn!(
            "dsl_path {} does not exist; no jobs will be loaded",
            root.display()
        );
        return Ok(out);
    }
    walk(root, root, policy, &mut out)?;
    tracing::info!("loaded {} job(s) from {}", out.len(), root.display());
    Ok(out)
}

fn walk(
    root: &Path,
    current: &Path,
    policy: &LoadPolicy,
    out: &mut Vec<LoadedJob>,
) -> Result<(), CronManagerError> {
    let entries = std::fs::read_dir(current)?;
    // Sort for deterministic load order — same DSL directory
    // always produces the same job list, which matters for
    // logging + fixture stability in tests.
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            walk(root, &path, policy, out)?;
        } else if is_yaml(&path) {
            for loaded in load_file_with_policy(root, &path, policy)? {
                out.push(loaded);
            }
        }
    }
    Ok(())
}

/// Parse one YAML file into zero or more `LoadedJob`s. The file
/// is a top-level mapping from job name to job definition.
/// Legacy entry point — uses the permissive default policy.
pub fn load_file(root: &Path, file: &Path) -> Result<Vec<LoadedJob>, CronManagerError> {
    load_file_with_policy(root, file, &LoadPolicy::permissive())
}

/// Bounded per-file load. Enforces the file-size cap BEFORE
/// reading the body into memory so a 100 MiB YAML bomb never
/// reaches the parser.
pub fn load_file_with_policy(
    root: &Path,
    file: &Path,
    policy: &LoadPolicy,
) -> Result<Vec<LoadedJob>, CronManagerError> {
    tracing::info!("dsl: loading {}", file.display());
    let meta = std::fs::metadata(file)?;
    if meta.len() > policy.max_file_bytes {
        return Err(CronManagerError::InvalidJobDefinition {
            source_path: file.display().to_string(),
            reason: format!(
                "DSL file size {} bytes exceeds security.max_dsl_file_bytes cap {} — a YAML anchor bomb can amplify well past this in the parser; refuse to load",
                meta.len(),
                policy.max_file_bytes,
            ),
        });
    }
    let body = std::fs::read_to_string(file)?;
    let raw: BTreeMap<String, RawJob> =
        serde_yaml_ng::from_str(&body).map_err(|e| CronManagerError::YamlParse {
            path: file.display().to_string(),
            source: e,
        })?;
    let group = group_from_path(root, file);
    let mut out = Vec::with_capacity(raw.len());
    for (name, raw_job) in raw {
        let spec = raw_job.into_spec(JobKey::new(group.clone(), name), file, policy)?;
        out.push(LoadedJob {
            spec,
            source_path: file.to_path_buf(),
        });
    }
    tracing::info!("dsl: group {} → {} job(s)", group, out.len());
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
///
/// `deny_unknown_fields` hardens the JVM's silent-drop behaviour.
/// A typo like `retryCoun: 3` — silently ignored on JVM — is a
/// hard load error here.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
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
    fn into_spec(
        self,
        key: JobKey,
        source: &Path,
        policy: &LoadPolicy,
    ) -> Result<JobSpec, CronManagerError> {
        let trigger = normalise_trigger(&self.trigger, source)?;
        // Validate cron at load time so a typo becomes a startup
        // error, not a first-fire crash. DEV-REQUIREMENTS §13.4.
        if let Trigger::Cron(expr) = &trigger {
            let schedule =
                cron::Schedule::from_str(expr).map_err(|e| CronManagerError::InvalidCron {
                    expression: expr.clone(),
                    reason: e.to_string(),
                })?;
            // High-frequency WARN — a `* * * * * * *` cron fires
            // every second and will saturate the executor pool as
            // soon as any single fire takes longer than the
            // interval. Not fatal (some ops jobs genuinely poll
            // every 5s) but visible in the boot log.
            if policy.min_cron_interval_secs > 0 {
                let mut it = schedule.upcoming(chrono::Utc);
                if let (Some(a), Some(b)) = (it.next(), it.next()) {
                    let interval = (b - a).num_seconds();
                    if interval < policy.min_cron_interval_secs {
                        tracing::warn!(
                            job = %format!("{}/{}", key.group, key.name),
                            interval_secs = interval,
                            source = %source.display(),
                            "cron fires more than once every {}s — scheduler may saturate if fires overlap",
                            policy.min_cron_interval_secs,
                        );
                    }
                }
            }
        }
        let window = TimeWindow {
            start_ms: self.start_date,
            end_ms: self.end_date,
        };

        // retryCount cap — a `retryCount: 4294967295` locks up an
        // executor slot for hours even with per-attempt timeouts.
        // Cap at security.max_retry_count.
        let retry_count = self.retry_count.unwrap_or(0);
        if retry_count > policy.max_retry_count {
            return Err(CronManagerError::InvalidJobDefinition {
                source_path: source.display().to_string(),
                reason: format!(
                    "job '{}' has retryCount {} which exceeds security.max_retry_count cap {}",
                    key.name, retry_count, policy.max_retry_count,
                ),
            });
        }
        if retry_count > 5 {
            tracing::warn!(
                job = %format!("{}/{}", key.group, key.name),
                retry_count,
                source = %source.display(),
                "retryCount above 5 is unusual — pin any real workload at 3 or less unless upstream is known-flaky",
            );
        }
        let retry = RetryPolicy {
            count: retry_count,
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
                // SSRF pre-flight — reject non-http(s) schemes and
                // literal private IPs at load. Hostnames resolve
                // at fire time (see executor/http.rs) because DNS
                // can change between load and fire.
                validate_http_url(&url, policy, source, &key)?;
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

/// Reject non-http(s) schemes and literal private IPs at load
/// time. Hostnames are re-validated at fire time (DNS TOCTOU) by
/// the HTTP executor. See `AUDIT.md` finding H1.
fn validate_http_url(
    url_str: &str,
    policy: &LoadPolicy,
    source: &Path,
    key: &JobKey,
) -> Result<(), CronManagerError> {
    let parsed = url::Url::parse(url_str).map_err(|e| CronManagerError::InvalidJobDefinition {
        source_path: source.display().to_string(),
        reason: format!("job '{}' url '{url_str}' is not a valid URL: {e}", key.name),
    })?;
    match parsed.scheme() {
        "http" | "https" => {}
        s => {
            return Err(CronManagerError::InvalidJobDefinition {
                source_path: source.display().to_string(),
                reason: format!(
                    "job '{}' url uses scheme '{s}' — only http and https are allowed",
                    key.name
                ),
            });
        }
    }
    let host = parsed
        .host()
        .ok_or_else(|| CronManagerError::InvalidJobDefinition {
            source_path: source.display().to_string(),
            reason: format!("job '{}' url '{url_str}' has no host component", key.name),
        })?;
    if !policy.block_private_networks {
        return Ok(());
    }
    // Use the parsed `Host` enum directly — `host_str()` returns
    // bracket-wrapped IPv6 literals which don't round-trip through
    // `IpAddr::from_str`. Matching on the enum sidesteps that.
    match host {
        url::Host::Ipv4(v4) => {
            if crate::security::is_private_or_local(std::net::IpAddr::V4(v4)) {
                return Err(private_ip_error(key, source, &v4.to_string()));
            }
        }
        url::Host::Ipv6(v6) => {
            if crate::security::is_private_or_local(std::net::IpAddr::V6(v6)) {
                return Err(private_ip_error(key, source, &v6.to_string()));
            }
        }
        url::Host::Domain(_) => {
            // Hostname: defer to fire-time DNS resolution.
        }
    }
    Ok(())
}

fn private_ip_error(key: &JobKey, source: &Path, ip: &str) -> CronManagerError {
    CronManagerError::InvalidJobDefinition {
        source_path: source.display().to_string(),
        reason: format!(
            "job '{}' url host '{ip}' is a non-routable / private IP; set security.block_private_networks=false to allow (danger: SSRF against cloud metadata)",
            key.name,
        ),
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
    fn file_size_cap_refuses_oversize_dsl() {
        let tmp = TempDir::new().unwrap();
        // 1 MiB + 1 byte of junk after a legit job so the parser
        // never sees the padding — the size check fires first.
        let big = format!(
            "hc:\n  trigger: off\n  type: http\n  method: GET\n  url: https://x/\n\n# {}\n",
            "A".repeat(1024 * 1024)
        );
        write(tmp.path(), "http/big.yaml", &big);
        let policy = LoadPolicy {
            max_file_bytes: 1024 * 1024,
            max_retry_count: 10,
            min_cron_interval_secs: 10,
            block_private_networks: true,
        };
        let err = load_all_with_policy(tmp.path(), &policy).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("max_dsl_file_bytes") && msg.contains("YAML anchor bomb"),
            "err was: {msg}"
        );
    }

    #[test]
    fn retry_count_cap_refuses_oversize_retry() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "http/rc.yaml",
            "hc:\n  trigger: off\n  type: http\n  method: GET\n  url: https://x/\n  retryCount: 100\n",
        );
        let policy = LoadPolicy {
            max_file_bytes: 1024 * 1024,
            max_retry_count: 10,
            min_cron_interval_secs: 10,
            block_private_networks: true,
        };
        let err = load_all_with_policy(tmp.path(), &policy).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("retryCount") && msg.contains("max_retry_count"),
            "err was: {msg}"
        );
    }

    #[test]
    fn ssrf_load_refuses_literal_private_ip() {
        let tmp = TempDir::new().unwrap();
        // AWS metadata endpoint — the classic SSRF target.
        write(
            tmp.path(),
            "attack/meta.yaml",
            "meta:\n  trigger: off\n  type: http\n  method: GET\n  url: http://169.254.169.254/latest/meta-data/\n",
        );
        let policy = LoadPolicy {
            max_file_bytes: 1024 * 1024,
            max_retry_count: 10,
            min_cron_interval_secs: 10,
            block_private_networks: true,
        };
        let err = load_all_with_policy(tmp.path(), &policy).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("169.254.169.254"), "err was: {msg}");
        assert!(msg.contains("non-routable"), "err was: {msg}");
    }

    #[test]
    fn ssrf_load_refuses_ipv6_loopback() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "attack/loop.yaml",
            "loop:\n  trigger: off\n  type: http\n  method: GET\n  url: http://[::1]/\n",
        );
        let policy = LoadPolicy {
            max_file_bytes: 1024 * 1024,
            max_retry_count: 10,
            min_cron_interval_secs: 10,
            block_private_networks: true,
        };
        let err = load_all_with_policy(tmp.path(), &policy).unwrap_err();
        assert!(err.to_string().contains("non-routable"));
    }

    #[test]
    fn ssrf_load_refuses_ipv4_mapped_ipv6_private() {
        let tmp = TempDir::new().unwrap();
        // ::ffff:169.254.169.254 — same metadata endpoint via
        // IPv4-mapped IPv6. Must still be caught.
        write(
            tmp.path(),
            "attack/mapped.yaml",
            "m:\n  trigger: off\n  type: http\n  method: GET\n  url: http://[::ffff:169.254.169.254]/\n",
        );
        let policy = LoadPolicy {
            max_file_bytes: 1024 * 1024,
            max_retry_count: 10,
            min_cron_interval_secs: 10,
            block_private_networks: true,
        };
        let err = load_all_with_policy(tmp.path(), &policy).unwrap_err();
        assert!(err.to_string().contains("non-routable"));
    }

    #[test]
    fn ssrf_load_rejects_non_http_scheme() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "attack/file.yaml",
            "f:\n  trigger: off\n  type: http\n  method: GET\n  url: file:///etc/passwd\n",
        );
        let policy = LoadPolicy {
            max_file_bytes: 1024 * 1024,
            max_retry_count: 10,
            min_cron_interval_secs: 10,
            block_private_networks: true,
        };
        let err = load_all_with_policy(tmp.path(), &policy).unwrap_err();
        assert!(err.to_string().contains("scheme"));
    }

    #[test]
    fn ssrf_load_allows_public_hostname() {
        // A hostname is only DNS-resolved at fire time — load
        // must succeed for `example.com`.
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "ok/ex.yaml",
            "e:\n  trigger: off\n  type: http\n  method: GET\n  url: https://example.com/\n",
        );
        let policy = LoadPolicy {
            max_file_bytes: 1024 * 1024,
            max_retry_count: 10,
            min_cron_interval_secs: 10,
            block_private_networks: true,
        };
        let out = load_all_with_policy(tmp.path(), &policy).unwrap();
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn ssrf_bypass_disabled_when_block_private_networks_false() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "attack/meta.yaml",
            "meta:\n  trigger: off\n  type: http\n  method: GET\n  url: http://169.254.169.254/\n",
        );
        let policy = LoadPolicy {
            max_file_bytes: 1024 * 1024,
            max_retry_count: 10,
            min_cron_interval_secs: 10,
            block_private_networks: false,
        };
        let out = load_all_with_policy(tmp.path(), &policy).unwrap();
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn ssrf_load_rejects_userinfo_trick() {
        // http://legit.com@169.254.169.254/ — the host per RFC
        // 3986 §3.2 is 169.254.169.254, not legit.com. Common
        // SSRF filter bypass; check we're using the real host.
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "attack/ui.yaml",
            "ui:\n  trigger: off\n  type: http\n  method: GET\n  url: \"http://legit.com@169.254.169.254/\"\n",
        );
        let policy = LoadPolicy {
            max_file_bytes: 1024 * 1024,
            max_retry_count: 10,
            min_cron_interval_secs: 10,
            block_private_networks: true,
        };
        let err = load_all_with_policy(tmp.path(), &policy).unwrap_err();
        assert!(err.to_string().contains("169.254.169.254"));
    }

    #[tokio::test]
    async fn load_all_bounded_delegates_to_sync_loader() {
        // The wrapper's job is `spawn_blocking(load_all_with_policy)`
        // under a wall-clock cap. Test the happy path here; the
        // caps themselves have their own targeted tests above.
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "http/x.yaml",
            "hc:\n  trigger: off\n  type: http\n  method: GET\n  url: https://example.com/\n",
        );
        let out = load_all_bounded(
            tmp.path(),
            1024 * 1024,
            Duration::from_secs(15),
            10,
            10,
            true,
        )
        .await
        .unwrap();
        assert_eq!(out.len(), 1);
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

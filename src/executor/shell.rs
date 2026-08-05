//! Shell executor. Spawns via `tokio::process::Command` with a
//! filtered env map, a bounded stdout capture, and a wall-clock
//! timeout that SIGKILLs runaway processes.
//!
//! Fixes JVM rough edges #6 (no wall-clock cap) and #3 (env
//! value with a comma corrupted the param split).

use crate::config::AppConfig;
use crate::error::CronManagerError;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::Notify;

#[derive(Clone)]
pub struct ShellExecutor {
    baseline_env: Arc<BTreeMap<String, String>>,
    app_root: Arc<PathBuf>,
    timeout: Duration,
    max_output_bytes: usize,
}

/// A single successful run.
#[derive(Debug, Clone)]
pub struct ShellAttempt {
    pub exit_code: i32,
    pub stdout: String,
}

impl ShellExecutor {
    pub fn new(cfg: &AppConfig) -> Self {
        tracing::info!(
            "shell: baseline env has {} entries",
            cfg.shell_environment.len()
        );
        Self {
            baseline_env: Arc::new(cfg.shell_environment.clone()),
            app_root: Arc::new(cfg.app_root_path.clone()),
            timeout: Duration::from_secs(cfg.limits.shell_timeout_secs),
            // Reuse the HTTP response cap — the same reasoning
            // applies (unbounded stdout can OOM the process).
            max_output_bytes: cfg.limits.max_response_bytes,
        }
    }

    pub async fn execute(
        &self,
        command: &str,
        allowed_envs: &[String],
        overrides: &[(String, String)],
        cancel: Arc<Notify>,
    ) -> Result<ShellAttempt, CronManagerError> {
        let (program, args) = split_command(command)?;
        let mut cmd = Command::new(&program);
        cmd.args(&args);
        cmd.current_dir(&*self.app_root);
        cmd.env_clear();
        for name in allowed_envs {
            if let Some(v) = self.baseline_env.get(name) {
                cmd.env(name, v);
            }
        }
        // Overrides win over baseline — mirrors JVM's params
        // dictionary behaviour.
        for (k, v) in overrides {
            if allowed_envs.iter().any(|a| a == k) {
                cmd.env(k, v);
            }
        }
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        tracing::debug!("shell: {} {:?}", program, args);
        let mut child = cmd.spawn().map_err(CronManagerError::Io)?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| CronManagerError::Internal("child stdout not captured".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| CronManagerError::Internal("child stderr not captured".into()))?;

        let cancel_clone = cancel.clone();
        let timeout = self.timeout;
        let max_output_bytes = self.max_output_bytes;

        // Capture stdout in a bounded buffer concurrently with
        // the wait. Anything past the cap is dropped so a
        // pathological script can't OOM us.
        let out_task = tokio::spawn(read_bounded(stdout, max_output_bytes));
        let err_task = tokio::spawn(read_bounded(stderr, max_output_bytes));

        let wait = child.wait();

        let exit_status = tokio::select! {
            r = wait => r.map_err(CronManagerError::Io)?,
            _ = tokio::time::sleep(timeout) => {
                let _ = child.kill().await;
                return Err(CronManagerError::ShellTimeout {
                    seconds: timeout.as_secs(),
                });
            }
            _ = cancel_clone.notified() => {
                let _ = child.kill().await;
                return Err(CronManagerError::ShellFailed {
                    message: "aborted by /stop".into(),
                });
            }
        };

        let stdout_bytes = out_task
            .await
            .map_err(|e| CronManagerError::Internal(format!("stdout task: {e}")))??;
        let stderr_bytes = err_task
            .await
            .map_err(|e| CronManagerError::Internal(format!("stderr task: {e}")))??;

        if exit_status.success() {
            Ok(ShellAttempt {
                exit_code: exit_status.code().unwrap_or(0),
                stdout: String::from_utf8_lossy(&stdout_bytes).into_owned(),
            })
        } else {
            let stderr_text = String::from_utf8_lossy(&stderr_bytes);
            Err(CronManagerError::ShellFailed {
                message: format!(
                    "exit {}: {}",
                    exit_status.code().unwrap_or(-1),
                    truncate(&stderr_text, 512)
                ),
            })
        }
    }
}

async fn read_bounded<R>(mut r: R, limit: usize) -> Result<Vec<u8>, CronManagerError>
where
    R: AsyncReadExt + Unpin,
{
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = r
            .read(&mut chunk)
            .await
            .map_err(|e| CronManagerError::Internal(format!("reading child pipe: {e}")))?;
        if n == 0 {
            return Ok(buf);
        }
        if buf.len() + n > limit {
            // Truncate but keep the process running — we don't
            // want to break a working script just because it
            // was chatty. Callers see a truncation marker.
            let room = limit.saturating_sub(buf.len());
            buf.extend_from_slice(&chunk[..room]);
            buf.extend_from_slice(b"...\n[output truncated]\n");
            // Drain the rest.
            let mut sink = [0u8; 4096];
            loop {
                match r.read(&mut sink).await {
                    Ok(0) | Err(_) => break,
                    _ => {}
                }
            }
            return Ok(buf);
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut end = max.min(s.len());
        while !s.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        format!("{}…", &s[..end])
    }
}

/// Whitespace-split with no shell interpretation — matches JVM
/// `Runtime.exec(String)` behaviour. Values containing spaces
/// aren't quotable; callers who need that should invoke `sh -c
/// "…"` explicitly in their DSL.
fn split_command(s: &str) -> Result<(String, Vec<String>), CronManagerError> {
    let mut parts = s.split_whitespace();
    let program = parts
        .next()
        .ok_or_else(|| CronManagerError::BadRequest("empty command".into()))?
        .to_string();
    let args: Vec<String> = parts.map(|s| s.to_string()).collect();
    Ok((program, args))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;

    fn tempscript(body: &str) -> (tempfile::TempDir, String) {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("script.sh");
        std::fs::write(&path, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perm = std::fs::metadata(&path).unwrap().permissions();
            perm.set_mode(0o755);
            std::fs::set_permissions(&path, perm).unwrap();
        }
        let s = path.display().to_string();
        (dir, s)
    }

    fn cfg_with_root(root: PathBuf) -> AppConfig {
        let mut c = AppConfig {
            app_root_path: root,
            ..AppConfig::default()
        };
        c.shell_environment
            .insert("HELLO_WORLD".to_string(), "hi".to_string());
        c.limits.shell_timeout_secs = 5;
        c.limits.max_response_bytes = 65_536;
        c
    }

    #[tokio::test]
    async fn successful_script_returns_stdout() {
        let (dir, script) = tempscript("#!/usr/bin/env bash\necho done\n");
        let cfg = cfg_with_root(dir.path().to_path_buf());
        let exec = ShellExecutor::new(&cfg);
        let out = exec
            .execute(&script, &[], &[], Arc::new(Notify::new()))
            .await
            .unwrap();
        assert_eq!(out.exit_code, 0);
        assert!(out.stdout.contains("done"));
    }

    #[tokio::test]
    async fn nonzero_exit_becomes_shell_failed() {
        let (dir, script) = tempscript("#!/usr/bin/env bash\necho bad >&2\nexit 7\n");
        let cfg = cfg_with_root(dir.path().to_path_buf());
        let exec = ShellExecutor::new(&cfg);
        let err = exec
            .execute(&script, &[], &[], Arc::new(Notify::new()))
            .await
            .unwrap_err();
        assert!(matches!(err, CronManagerError::ShellFailed { .. }));
    }

    #[tokio::test]
    async fn timeout_kills_runaway_process() {
        let (dir, script) = tempscript("#!/usr/bin/env bash\nsleep 30\n");
        let mut cfg = cfg_with_root(dir.path().to_path_buf());
        cfg.limits.shell_timeout_secs = 1;
        let exec = ShellExecutor::new(&cfg);
        let err = exec
            .execute(&script, &[], &[], Arc::new(Notify::new()))
            .await
            .unwrap_err();
        assert!(matches!(err, CronManagerError::ShellTimeout { seconds: 1 }));
    }

    #[tokio::test]
    async fn env_allowlist_gates_variable_exposure() {
        let (dir, script) = tempscript(
            "#!/usr/bin/env bash\necho hello=${HELLO_WORLD-none}\necho secret=${SECRET_KEY-none}\n",
        );
        let mut cfg = cfg_with_root(dir.path().to_path_buf());
        cfg.shell_environment
            .insert("SECRET_KEY".to_string(), "shh".to_string());
        let exec = ShellExecutor::new(&cfg);
        let out = exec
            .execute(
                &script,
                &["HELLO_WORLD".to_string()],
                &[],
                Arc::new(Notify::new()),
            )
            .await
            .unwrap();
        // HELLO_WORLD is on the allow-list → exposed.
        assert!(out.stdout.contains("hello=hi"));
        // SECRET_KEY is NOT on the allow-list → hidden.
        assert!(out.stdout.contains("secret=none"));
    }

    #[tokio::test]
    async fn env_overrides_only_apply_when_allowlisted() {
        let (dir, script) =
            tempscript("#!/usr/bin/env bash\necho a=${FOO-none}\necho b=${BAR-none}\n");
        let cfg = cfg_with_root(dir.path().to_path_buf());
        let exec = ShellExecutor::new(&cfg);
        let out = exec
            .execute(
                &script,
                &["FOO".to_string()],
                &[
                    ("FOO".to_string(), "yes".to_string()),
                    ("BAR".to_string(), "no".to_string()),
                ],
                Arc::new(Notify::new()),
            )
            .await
            .unwrap();
        assert!(out.stdout.contains("a=yes"));
        assert!(out.stdout.contains("b=none"));
    }

    #[tokio::test]
    async fn override_wins_over_baseline_env() {
        let (dir, script) = tempscript("#!/usr/bin/env bash\necho v=${HELLO_WORLD-none}\n");
        let cfg = cfg_with_root(dir.path().to_path_buf());
        let exec = ShellExecutor::new(&cfg);
        let out = exec
            .execute(
                &script,
                &["HELLO_WORLD".to_string()],
                &[("HELLO_WORLD".to_string(), "override".to_string())],
                Arc::new(Notify::new()),
            )
            .await
            .unwrap();
        assert!(out.stdout.contains("v=override"));
    }

    #[test]
    fn split_command_whitespace() {
        let (p, args) = split_command("./bin/x --flag  1  2").unwrap();
        assert_eq!(p, "./bin/x");
        assert_eq!(args, vec!["--flag", "1", "2"]);
    }

    #[test]
    fn split_command_rejects_empty() {
        assert!(split_command("   ").is_err());
    }
}

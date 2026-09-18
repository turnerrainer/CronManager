//! Process-signal plumbing for graceful shutdown.
//!
//! h2ck.me v1 T-18 / FLEET §34.4 — before this module the binary
//! blocked on `axum::serve(..).await` with no signal handling, so
//! `docker stop` / a k8s pod eviction would arrive as a hard
//! SIGKILL after the 10s grace window and any in-flight requests
//! got their sockets ripped from under them. `shutdown_signal`
//! resolves on SIGTERM or SIGINT and is wired into
//! `axum::serve.with_graceful_shutdown` in `main.rs`.
//!
//! On Windows only Ctrl+C is available (SIGTERM has no analogue);
//! the tokio::signal::ctrl_c path covers both platforms.

/// Future that resolves when the process receives SIGTERM (unix)
/// or SIGINT / Ctrl+C. Passed to `axum::serve.with_graceful_shutdown`
/// so in-flight requests get their wall-clock finish before the
/// server stops accepting new connections.
pub async fn shutdown_signal() {
    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(e) => {
                tracing::warn!("shutdown: failed to install SIGTERM handler: {e}");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::warn!("shutdown: failed to listen for Ctrl+C: {e}");
            std::future::pending::<()>().await
        }
    };

    tokio::select! {
        _ = terminate => tracing::info!("shutdown: SIGTERM received, draining in-flight requests"),
        _ = ctrl_c => tracing::info!("shutdown: SIGINT/Ctrl+C received, draining in-flight requests"),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Send SIGTERM to the current process 50ms after arming
    /// `shutdown_signal`. The future must resolve within a short
    /// window — if it doesn't, either the SIGTERM handler is
    /// mis-wired or the runtime is deadlocked.
    ///
    /// Regression pin for h2ck.me v1 T-18.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_signal_resolves_on_sigterm() {
        // Spawn a task that sends SIGTERM to self after a short
        // delay. `libc::raise` avoids racing on process-id
        // reuse across pid namespaces.
        tokio::spawn(async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            // SAFETY: `raise` is async-signal-safe and thread-safe.
            unsafe {
                libc::raise(libc::SIGTERM);
            }
        });

        let done = tokio::time::timeout(Duration::from_secs(3), shutdown_signal()).await;
        assert!(
            done.is_ok(),
            "shutdown_signal did not resolve within 3s of SIGTERM"
        );
    }
}

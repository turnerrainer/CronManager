//! HTTP executor. One `reqwest::Client` per process, built with
//! the configured timeout. Each attempt is bounded and streamed
//! into a size-capped buffer.
//!
//! Redirects are DISABLED — see `AUDIT.md` finding H1. The
//! rationale: a legitimate upstream can return `302 Location:
//! http://169.254.169.254/…` and (without this) reqwest would
//! silently follow into the AWS metadata endpoint. Jobs that
//! legitimately want to follow a redirect can hit the destination
//! URL directly.

use crate::config::Limits;
use crate::error::CronManagerError;
use reqwest::{Client, Method};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;

#[derive(Clone)]
pub struct HttpExecutor {
    client: Client,
    max_response_bytes: usize,
    timeout: Duration,
    block_private_networks: bool,
}

/// A single successful attempt.
#[derive(Debug, Clone)]
pub struct HttpAttempt {
    pub status: u16,
    pub body: String,
}

impl HttpExecutor {
    pub fn new(limits: &Limits) -> Result<Self, CronManagerError> {
        Self::with_ssrf_policy(limits, true)
    }

    /// Build the executor with an explicit SSRF policy. Split out
    /// so the scheduler can wire in the `security.block_private_networks`
    /// flag without leaking the whole `AppConfig` into the
    /// executor. When true, [`execute`] resolves the URL host at
    /// fire time and refuses to send if it resolves to a
    /// non-routable IP.
    pub fn with_ssrf_policy(
        limits: &Limits,
        block_private_networks: bool,
    ) -> Result<Self, CronManagerError> {
        let timeout = Duration::from_secs(limits.request_timeout_secs);
        let client = Client::builder()
            .timeout(timeout)
            // SSRF: never auto-follow. See module docs.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| CronManagerError::Internal(format!("reqwest client build: {e}")))?;
        Ok(Self {
            client,
            max_response_bytes: limits.max_response_bytes,
            timeout,
            block_private_networks,
        })
    }

    pub async fn execute(
        &self,
        method: &str,
        url: &str,
        cancel: Arc<Notify>,
    ) -> Result<HttpAttempt, CronManagerError> {
        let method = Method::from_bytes(method.to_uppercase().as_bytes())
            .map_err(|_| CronManagerError::Internal(format!("unknown HTTP method '{method}'")))?;

        // SSRF fire-time re-check: hostnames are validated at
        // load, but DNS can change between load and fire (TOCTOU).
        // Re-resolve every host on every fire; refuse if any
        // resolved address is non-routable. Literal-IP URLs got
        // rejected at load if they were private.
        if self.block_private_networks {
            self.reject_private_dns(url).await?;
        }

        tracing::debug!("HTTP {} {}", method, url);
        let send = self.client.request(method, url).send();

        let resp = tokio::select! {
            r = send => r.map_err(|e| self.map_send_err(e))?,
            _ = cancel.notified() => {
                return Err(CronManagerError::Internal("aborted by /stop before response".into()));
            }
        };

        let status = resp.status().as_u16();
        let body = read_bounded(resp, self.max_response_bytes).await?;
        // With redirects disabled, a 3xx response reaches us
        // instead of being followed. Treat 3xx as an upstream
        // error the same way we treat 5xx — the job author can
        // spell out the destination URL if they meant to hit it.
        if !(200..300).contains(&status) {
            // Sanitise before truncation — the body is
            // attacker-controlled once you consider a legit
            // upstream that returns whatever a caller shoves at
            // it (e.g. a search API echoing the query). Any CRLF
            // / ANSI here would land in tracing! via
            // err.to_string() and downstream log viewers. See H2.
            let sanitised = crate::security::sanitize_for_log(&body);
            return Err(CronManagerError::UpstreamHttpError {
                status,
                body: truncate(&sanitised, 1024),
            });
        }
        Ok(HttpAttempt { status, body })
    }

    /// Resolve the URL's host at fire time and refuse if any
    /// returned address is in a non-routable range. Literal-IP
    /// URLs pass through — the loader already rejected literal
    /// private IPs.
    async fn reject_private_dns(&self, url_str: &str) -> Result<(), CronManagerError> {
        let parsed = url::Url::parse(url_str)
            .map_err(|e| CronManagerError::Internal(format!("re-parsing url '{url_str}': {e}")))?;
        let host = match parsed.host_str() {
            Some(h) => h,
            None => return Ok(()),
        };
        // Literal IP → loader already screened; the executor
        // trusts it (removes redundant work in the hot path).
        if host.parse::<std::net::IpAddr>().is_ok() {
            return Ok(());
        }
        // Any port is fine for the resolution; we just want the
        // addresses. Fall back to 80 when the URL omits one.
        let port = parsed.port().unwrap_or(match parsed.scheme() {
            "https" => 443,
            _ => 80,
        });
        let target = format!("{host}:{port}");
        let addrs = tokio::net::lookup_host(target.as_str())
            .await
            .map_err(|e| CronManagerError::Internal(format!("dns lookup {target}: {e}")))?;
        for addr in addrs {
            if crate::security::is_private_or_local(addr.ip()) {
                tracing::warn!(
                    host = %host,
                    resolved = %addr.ip(),
                    "http: SSRF pre-flight refused fire — hostname resolves to non-routable IP"
                );
                return Err(CronManagerError::BadRequest(format!(
                    "hostname '{host}' resolves to non-routable IP {} (SSRF pre-flight); set security.block_private_networks=false to allow",
                    addr.ip()
                )));
            }
        }
        Ok(())
    }

    fn map_send_err(&self, e: reqwest::Error) -> CronManagerError {
        if e.is_timeout() {
            CronManagerError::UpstreamTimeout {
                seconds: self.timeout.as_secs(),
            }
        } else {
            CronManagerError::Internal(format!("upstream request: {e}"))
        }
    }
}

/// Stream the response body chunk-by-chunk, refusing as soon as
/// the cumulative size crosses `limit`. Prevents a malicious or
/// misbehaving upstream from pinning arbitrary memory.
async fn read_bounded(
    mut resp: reqwest::Response,
    limit: usize,
) -> Result<String, CronManagerError> {
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| CronManagerError::Internal(format!("reading upstream body chunk: {e}")))?
    {
        if buf.len() + chunk.len() > limit {
            return Err(CronManagerError::UpstreamBodyTooLarge { limit });
        }
        buf.extend_from_slice(&chunk);
    }
    // Body may not be UTF-8 (binary payloads); keep the lossy
    // conversion so history still records something useful.
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        // Slice on a char boundary to avoid splitting a
        // multi-byte codepoint.
        let mut end = max.min(s.len());
        while !s.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        format!("{}… (truncated)", &s[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Limits;

    #[tokio::test]
    async fn successful_get_returns_body_and_status() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/ok")
            .with_status(200)
            .with_header("content-type", "text/plain")
            .with_body("hello world")
            .create_async()
            .await;

        // SSRF policy off — mockito binds to 127.0.0.1 which the
        // policy would otherwise reject. Loopback is used across
        // the existing test suite so we opt out here.
        let exec = HttpExecutor::with_ssrf_policy(&Limits::default(), false).unwrap();
        let attempt = exec
            .execute(
                "GET",
                &format!("{}/ok", server.url()),
                Arc::new(Notify::new()),
            )
            .await
            .unwrap();
        assert_eq!(attempt.status, 200);
        assert_eq!(attempt.body, "hello world");
    }

    #[tokio::test]
    async fn non_2xx_becomes_upstream_error() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/nope")
            .with_status(503)
            .with_body("service unavailable")
            .create_async()
            .await;

        let exec = HttpExecutor::with_ssrf_policy(&Limits::default(), false).unwrap();
        let err = exec
            .execute(
                "GET",
                &format!("{}/nope", server.url()),
                Arc::new(Notify::new()),
            )
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            CronManagerError::UpstreamHttpError { status: 503, .. }
        ));
    }

    #[tokio::test]
    async fn body_exceeding_cap_is_rejected() {
        let mut server = mockito::Server::new_async().await;
        let big = "x".repeat(2048);
        let _m = server
            .mock("GET", "/big")
            .with_status(200)
            .with_body(&big)
            .create_async()
            .await;

        let limits = Limits {
            max_response_bytes: 1024,
            ..Limits::default()
        };
        let exec = HttpExecutor::with_ssrf_policy(&limits, false).unwrap();
        let err = exec
            .execute(
                "GET",
                &format!("{}/big", server.url()),
                Arc::new(Notify::new()),
            )
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            CronManagerError::UpstreamBodyTooLarge { limit: 1024 }
        ));
    }

    #[tokio::test]
    async fn unknown_method_errors_cleanly() {
        let exec = HttpExecutor::with_ssrf_policy(&Limits::default(), false).unwrap();
        let err = exec
            .execute("FLOOP", "http://127.0.0.1:1", Arc::new(Notify::new()))
            .await
            .unwrap_err();
        assert!(matches!(err, CronManagerError::Internal(_)));
    }

    #[tokio::test]
    async fn redirect_is_not_followed() {
        // Upstream returns 302 to a second URL; we must NEVER
        // reach the second URL. Redirect policy is `none`, so
        // the 302 comes back to us as an upstream error status.
        let mut redirector = mockito::Server::new_async().await;
        let _dst = redirector
            .mock("GET", "/dst")
            .with_status(200)
            .with_body("would leak")
            .expect(0)
            .create_async()
            .await;
        let _src = redirector
            .mock("GET", "/src")
            .with_status(302)
            .with_header("location", &format!("{}/dst", redirector.url()))
            .create_async()
            .await;

        let exec = HttpExecutor::with_ssrf_policy(&Limits::default(), false).unwrap();
        let err = exec
            .execute(
                "GET",
                &format!("{}/src", redirector.url()),
                Arc::new(Notify::new()),
            )
            .await
            .unwrap_err();
        match err {
            CronManagerError::UpstreamHttpError { status, .. } => {
                assert_eq!(status, 302, "expected the 302 to reach us un-followed");
            }
            other => panic!("expected upstream 302, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn ssrf_pre_flight_refuses_private_hostname_dns() {
        // localhost resolves to 127.0.0.1 (and possibly ::1) —
        // both are private. Pre-flight must reject before any
        // network write happens.
        let exec = HttpExecutor::with_ssrf_policy(&Limits::default(), true).unwrap();
        let err = exec
            .execute(
                "GET",
                "http://localhost:1/whatever",
                Arc::new(Notify::new()),
            )
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("SSRF pre-flight") && msg.contains("localhost"),
            "got: {msg}"
        );
    }

    #[test]
    fn truncate_never_splits_codepoint() {
        // Two-byte character straddling the boundary.
        let s = "aä"; // 3 bytes: 0x61, 0xC3, 0xA4
        let t = truncate(s, 2);
        // Must NOT be "a\xC3" — that's invalid UTF-8.
        assert!(t.starts_with('a'));
    }
}

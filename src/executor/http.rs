//! HTTP executor. One `reqwest::Client` per process, built with
//! the configured timeout. Each attempt is bounded and streamed
//! into a size-capped buffer.

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
}

/// A single successful attempt.
#[derive(Debug, Clone)]
pub struct HttpAttempt {
    pub status: u16,
    pub body: String,
}

impl HttpExecutor {
    pub fn new(limits: &Limits) -> Result<Self, CronManagerError> {
        let timeout = Duration::from_secs(limits.request_timeout_secs);
        let client = Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| CronManagerError::Internal(format!("reqwest client build: {e}")))?;
        Ok(Self {
            client,
            max_response_bytes: limits.max_response_bytes,
            timeout,
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
        if !(200..300).contains(&status) {
            return Err(CronManagerError::UpstreamHttpError {
                status,
                body: truncate(&body, 1024),
            });
        }
        Ok(HttpAttempt { status, body })
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

        let exec = HttpExecutor::new(&Limits::default()).unwrap();
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

        let exec = HttpExecutor::new(&Limits::default()).unwrap();
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
        let exec = HttpExecutor::new(&limits).unwrap();
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
        let exec = HttpExecutor::new(&Limits::default()).unwrap();
        let err = exec
            .execute("FLOOP", "http://127.0.0.1:1", Arc::new(Notify::new()))
            .await
            .unwrap_err();
        assert!(matches!(err, CronManagerError::Internal(_)));
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

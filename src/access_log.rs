//! Per-request INFO access-log middleware.
//!
//! Audit LOG-v1 FN-LOG-3 — before this, CronManager at `RUST_LOG=info`
//! produced zero per-request log lines beyond boot + auth-failure WARNs.
//! 42 probes produced 3 non-boot log lines total. SOC2 CC7.2 / ISO27001
//! A.12.4 access-logging compliance gap.
//!
//! Emits exactly one INFO line per completed request:
//!   INFO http_request_completed method=POST route=/execute/:g/:j
//!        status=200 duration_us=1234 trace_id=<hex>
//!
//! **trace_id inheritance** — Buerostack topology: Ruuter is the fleet's
//! reverse proxy. Every request CronManager sees carries a W3C
//! `traceparent` header set by Ruuter. Extract the 32-char trace-id
//! from it so log entries in Ruuter and CronManager can be correlated
//! end-to-end. If missing, generate a fresh short id.
//!
//! Deliberate omissions: no headers, no body, no client IP (Ruuter's log
//! has the useful IP), matched route pattern only (not raw URI).

use axum::extract::{MatchedPath, Request};
use axum::middleware::Next;
use axum::response::Response;
use std::time::Instant;

pub async fn access_log_middleware(req: Request, next: Next) -> Response {
    let start = Instant::now();
    let method = req.method().clone();
    let route = req
        .extensions()
        .get::<MatchedPath>()
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| "<unmatched>".to_string());
    let trace_id = extract_trace_id(&req);

    let response = next.run(req).await;

    let status = response.status().as_u16();
    let duration_us = start.elapsed().as_micros();

    tracing::info!(
        method = %method,
        route = %route,
        status,
        duration_us,
        trace_id = %trace_id,
        "http_request_completed"
    );
    response
}

fn extract_trace_id(req: &Request) -> String {
    if let Some(tp) = req
        .headers()
        .get("traceparent")
        .and_then(|v| v.to_str().ok())
    {
        let parts: Vec<&str> = tp.split('-').collect();
        if parts.len() == 4
            && parts[1].len() == 32
            && parts[1].bytes().all(|b| b.is_ascii_hexdigit())
        {
            return parts[1].to_string();
        }
    }
    // CronManager already depends on uuid — use it for the fallback id.
    let full = uuid::Uuid::new_v4().simple().to_string();
    full[..16].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request as HttpRequest};

    #[test]
    fn inherits_trace_id_from_valid_traceparent() {
        let req = HttpRequest::builder()
            .header(
                "traceparent",
                "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01",
            )
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            extract_trace_id(&req),
            "0af7651916cd43dd8448eb211c80319c"
        );
    }

    #[test]
    fn generates_fresh_id_when_traceparent_absent() {
        let req = HttpRequest::builder().body(Body::empty()).unwrap();
        let tp = extract_trace_id(&req);
        assert_eq!(tp.len(), 16);
        assert!(tp.chars().all(|c| c.is_ascii_hexdigit()));
    }
}

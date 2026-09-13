//! W3C Trace Context response-header propagation.
//!
//! Adopts FLEET-STRONGHOLDS §1.6. Every response gets:
//!
//! - `traceparent: 00-<trace_id>-<span_id>-01`
//! - `x-trace-id: <trace_id>`
//!
//! `trace_id` is extracted from the inbound `traceparent` header
//! when a valid one is present (Ruuter sets this in the Buerostack
//! fleet topology). Otherwise a fresh 32-hex-char id is generated
//! so a directly-called deployment still emits correlatable IDs.
//!
//! `span_id` is a fresh 16-hex-char (8-byte) value per response —
//! CronManager has one span per HTTP handler for now, so a single
//! generated id per request is fine.
//!
//! The middleware is idempotent: if a downstream handler already
//! set either header, we don't overwrite it.

use axum::extract::Request;
use axum::http::{HeaderName, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;

const TRACEPARENT: HeaderName = HeaderName::from_static("traceparent");
const X_TRACE_ID: HeaderName = HeaderName::from_static("x-trace-id");

pub async fn traceparent_middleware(req: Request, next: Next) -> Response {
    let trace_id = extract_or_generate_trace_id(&req);
    let span_id = generate_span_id();

    let mut response = next.run(req).await;
    let headers = response.headers_mut();

    // Only insert if not already set — a downstream handler can
    // opt out by writing its own value.
    if !headers.contains_key(&TRACEPARENT) {
        // "00-<trace_id>-<span_id>-01" per W3C Trace Context §3.
        // Flag byte `01` means "sampled". CronManager has no
        // sampling decision surface — always sampled.
        let value = format!("00-{trace_id}-{span_id}-01");
        if let Ok(hv) = HeaderValue::from_str(&value) {
            headers.insert(TRACEPARENT.clone(), hv);
        }
    }
    if !headers.contains_key(&X_TRACE_ID) {
        if let Ok(hv) = HeaderValue::from_str(&trace_id) {
            headers.insert(X_TRACE_ID.clone(), hv);
        }
    }
    response
}

/// Return the inbound trace-id if the `traceparent` header is a
/// well-formed W3C value; otherwise generate a fresh 32-hex-char
/// id. A valid inbound header has the shape
/// `00-<32-hex trace_id>-<16-hex span_id>-<2-hex flags>`.
pub fn extract_or_generate_trace_id(req: &Request) -> String {
    if let Some(tp) = req
        .headers()
        .get("traceparent")
        .and_then(|v| v.to_str().ok())
    {
        let parts: Vec<&str> = tp.split('-').collect();
        if parts.len() == 4
            && parts[0] == "00"
            && parts[1].len() == 32
            && parts[1].bytes().all(|b| b.is_ascii_hexdigit())
            // Reject the "all zeros" sentinel per W3C §3.2.2.3.
            && parts[1] != "00000000000000000000000000000000"
        {
            return parts[1].to_string();
        }
    }
    // 32-hex fresh trace-id. `Uuid::new_v4().simple()` renders
    // exactly 32 hex chars — spec-compliant width.
    uuid::Uuid::new_v4().simple().to_string()
}

/// 16-hex-char (8-byte) span id. UUID v4 gives us 32 hex chars;
/// take the first half. Not cryptographically load-bearing (only
/// used for correlation) so the truncation is fine.
fn generate_span_id() -> String {
    let full = uuid::Uuid::new_v4().simple().to_string();
    full[..16].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request as HttpRequest};

    fn req_with_header(name: &'static str, value: &str) -> Request {
        HttpRequest::builder()
            .header(name, value)
            .body(Body::empty())
            .unwrap()
    }

    #[test]
    fn inherits_trace_id_from_valid_traceparent() {
        let req = req_with_header(
            "traceparent",
            "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01",
        );
        assert_eq!(
            extract_or_generate_trace_id(&req),
            "0af7651916cd43dd8448eb211c80319c"
        );
    }

    #[test]
    fn generates_fresh_id_when_traceparent_absent() {
        let req = HttpRequest::builder().body(Body::empty()).unwrap();
        let tp = extract_or_generate_trace_id(&req);
        assert_eq!(tp.len(), 32);
        assert!(tp.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn rejects_all_zeros_trace_id_and_generates_fresh() {
        let req = req_with_header(
            "traceparent",
            "00-00000000000000000000000000000000-b7ad6b7169203331-01",
        );
        let tp = extract_or_generate_trace_id(&req);
        assert_ne!(tp, "00000000000000000000000000000000");
        assert_eq!(tp.len(), 32);
    }

    #[test]
    fn rejects_malformed_traceparent_and_generates_fresh() {
        for value in [
            "not-a-traceparent",
            "01-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01", // bad version
            "00-shortid-b7ad6b7169203331-01",
            "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331", // 3 parts
        ] {
            let req = req_with_header("traceparent", value);
            let tp = extract_or_generate_trace_id(&req);
            assert_eq!(tp.len(), 32, "value {value:?} produced {tp:?}");
            assert_ne!(tp, "0af7651916cd43dd8448eb211c80319c");
        }
    }

    #[test]
    fn span_id_is_16_hex_chars() {
        let s = generate_span_id();
        assert_eq!(s.len(), 16);
        assert!(s.chars().all(|c| c.is_ascii_hexdigit()));
    }
}

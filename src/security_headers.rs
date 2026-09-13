//! Default security-response headers middleware.
//!
//! Adopts FLEET-STRONGHOLDS §5.1 (Buerostack cross-service pattern).
//! Emits the five baseline browser-side defenses on every response
//! so a misconfigured reverse proxy or a direct bind (dev, staging
//! bypass) still ships defense-in-depth.
//!
//! Headers set:
//! - Content-Security-Policy: default-src 'none'; frame-ancestors 'none'
//!   — the JSON API never loads scripts / images / iframes; blanket
//!   deny is the safe default. `frame-ancestors 'none'` denies
//!   clickjacking without needing X-Frame-Options.
//! - Strict-Transport-Security: max-age=63072000; includeSubDomains; preload
//!   — 2 years, subdomains covered, preload-eligible if the operator
//!   registers the domain. Safe to emit on http:// binds — browsers
//!   ignore HSTS from insecure origins.
//! - X-Frame-Options: DENY
//!   — belt-and-braces for older browsers that don't respect
//!   frame-ancestors.
//! - X-Content-Type-Options: nosniff
//!   — prevents MIME-sniffing attacks on JSON responses.
//! - Referrer-Policy: no-referrer
//!   — JSON API bodies shouldn't leak referring URLs to any next hop.
//!
//! Deliberately not configurable per-response. Operators who need
//! different values terminate at a proxy that overwrites.

use axum::extract::Request;
use axum::http::HeaderValue;
use axum::middleware::Next;
use axum::response::Response;

pub async fn security_headers_middleware(req: Request, next: Next) -> Response {
    let mut response = next.run(req).await;
    let h = response.headers_mut();
    // Only insert if the handler hasn't already set the header (some
    // future handler might legitimately need a different value).
    h.entry("content-security-policy")
        .or_insert(HeaderValue::from_static(
            "default-src 'none'; frame-ancestors 'none'",
        ));
    h.entry("strict-transport-security")
        .or_insert(HeaderValue::from_static(
            "max-age=63072000; includeSubDomains; preload",
        ));
    h.entry("x-frame-options")
        .or_insert(HeaderValue::from_static("DENY"));
    h.entry("x-content-type-options")
        .or_insert(HeaderValue::from_static("nosniff"));
    h.entry("referrer-policy")
        .or_insert(HeaderValue::from_static("no-referrer"));
    response
}

#[cfg(test)]
mod tests {
    // The header content itself is checked at the integration-test
    // layer (see tests/router_integration.rs::
    // `every_response_carries_security_headers`) because the middleware's
    // value is the wiring, not the constants.
}

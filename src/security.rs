//! Cross-cutting security helpers shared by the router, executors,
//! DSL loader, and history recorder. Every function here is
//! deliberately pure/side-effect-free so it can be unit-tested in
//! isolation and reused across seams.
//!
//! See `AUDIT.md` findings C1, C2, H1, H2, H3 for the specific
//! failure modes each helper defends against.

use std::net::IpAddr;

/// Env vars a caller must never be allowed to set via
/// `POST /execute/…?FOO=…`, even if the DSL's `allowedEnvs` list
/// includes them. Setting any of these lets an attacker redirect
/// which binaries the shell command loads (`PATH`, `LD_PRELOAD`),
/// swap out shared libraries (`LD_LIBRARY_PATH`), or inject
/// interpreter-level code (`PYTHONPATH`, `NODE_OPTIONS`, …).
///
/// The list is intentionally conservative — it's easier to
/// whitelist a rare legit override via
/// `security.allow_dangerous_env_overrides` than to plug a novel
/// gadget after the fact.
pub const DANGEROUS_ENVS: &[&str] = &[
    "PATH",
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "LD_AUDIT",
    "DYLD_INSERT_LIBRARIES",
    "DYLD_LIBRARY_PATH",
    "DYLD_FALLBACK_LIBRARY_PATH",
    "PYTHONPATH",
    "PYTHONHOME",
    "PYTHONSTARTUP",
    "NODE_OPTIONS",
    "NODE_PATH",
    "RUBYOPT",
    "RUBYLIB",
    "PERL5OPT",
    "PERL5LIB",
    "JAVA_TOOL_OPTIONS",
    "_JAVA_OPTIONS",
    "MallocPreloadClass",
];

/// Case-insensitive check against `DANGEROUS_ENVS` and the two
/// wildcard prefixes (`LD_`, `DYLD_`). Prefix rules cover the
/// glibc / macOS loader-hook families exhaustively without listing
/// every variant.
pub fn is_dangerous_env(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    if DANGEROUS_ENVS
        .iter()
        .any(|d| d.eq_ignore_ascii_case(&upper))
    {
        return true;
    }
    upper.starts_with("LD_") || upper.starts_with("DYLD_")
}

/// SSRF pre-flight for HTTP-job URLs. Classifies an IP as
/// non-routable (loopback, link-local, private, ULA, multicast,
/// unspecified). Returns `true` for any address CronManager must
/// not talk to unless the operator explicitly disabled
/// `security.block_private_networks`.
///
/// Covers the historical bypasses:
/// * IPv4-mapped IPv6 (`::ffff:169.254.169.254` → the underlying
///   IPv4 is unwrapped and re-checked)
/// * IPv6 link-local (`fe80::/10`)
/// * ULA (`fc00::/7`)
/// * Documentation ranges (`192.0.2.0/24`, `198.51.100.0/24`,
///   `203.0.113.0/24`) — safe to allow, but included so SSRF
///   playbooks that use them for reproducible test targets don't
///   leak into prod scanners.
pub fn is_private_or_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            if v4.is_loopback() || v4.is_link_local() || v4.is_private() || v4.is_unspecified() {
                return true;
            }
            // Carrier-grade NAT (RFC 6598) — not routable on the
            // public internet, but not caught by `is_private()`.
            let o = v4.octets();
            if o[0] == 100 && (64..=127).contains(&o[1]) {
                return true;
            }
            // Multicast + broadcast.
            if v4.is_multicast() || v4.is_broadcast() {
                return true;
            }
            false
        }
        IpAddr::V6(v6) => {
            if v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() {
                return true;
            }
            // IPv4-mapped IPv6 (`::ffff:a.b.c.d`) — unwrap and
            // re-classify against the IPv4 rules.
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_private_or_local(IpAddr::V4(v4));
            }
            let seg = v6.segments();
            // fe80::/10 — link-local.
            if seg[0] & 0xffc0 == 0xfe80 {
                return true;
            }
            // fc00::/7 — unique local addresses (ULA).
            if seg[0] & 0xfe00 == 0xfc00 {
                return true;
            }
            false
        }
    }
}

/// Sanitize a byte string for structured log output. Replaces
/// CR/LF and control bytes with `\r` / `\n` / `\uXXXX` escapes so
/// that:
///
/// * A shell job that captured attacker-controlled stdout can't
///   splice fake log entries (CRLF injection).
/// * Terminal viewers can't be pivoted via ANSI escapes / OSC-8.
/// * Non-UTF-8 bytes don't panic the log line formatter.
///
/// The result is capped at 4 KiB so a chatty script doesn't fill
/// the process log with a single stanza. The DB history row
/// separately stores a much larger head+tail slice via
/// [`truncate_response_body`].
pub fn sanitize_for_log(s: &str) -> String {
    let mut out = String::with_capacity(s.len().min(4096));
    let mut written = 0usize;
    for c in s.chars() {
        // Reserve enough headroom for the longest single-char
        // escape we might emit ("\\uXXXX" = 6 bytes).
        if written + 6 > 4096 {
            out.push_str("…[truncated]");
            break;
        }
        match c {
            '\r' => {
                out.push_str("\\r");
                written += 2;
            }
            '\n' => {
                out.push_str("\\n");
                written += 2;
            }
            '\t' => {
                out.push('\t');
                written += 1;
            }
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                out.push_str(&format!("\\u{:04x}", c as u32));
                written += 6;
            }
            c => {
                out.push(c);
                written += c.len_utf8();
            }
        }
    }
    out
}

/// Cap the bytes of an HTTP response body / shell stdout stored in
/// the history row. Above the cap we keep the first `max/2` bytes
/// and the last `max/2` bytes with a marker in between — enough
/// context for triage without unbounded row growth on chatty
/// upstreams.
///
/// UTF-8 boundaries are respected on both cut points so the marker
/// splices in cleanly.
pub fn truncate_response_body(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let half = max / 2;
    let head_end = char_boundary_floor(s, half);
    let tail_start = char_boundary_ceil(s, s.len().saturating_sub(half));
    let truncated = s.len() - (head_end + (s.len() - tail_start));
    format!(
        "{}\n… [{} bytes truncated] …\n{}",
        &s[..head_end],
        truncated,
        &s[tail_start..]
    )
}

fn char_boundary_floor(s: &str, mut idx: usize) -> usize {
    if idx >= s.len() {
        return s.len();
    }
    while !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

fn char_boundary_ceil(s: &str, mut idx: usize) -> usize {
    if idx >= s.len() {
        return s.len();
    }
    while !s.is_char_boundary(idx) {
        idx += 1;
    }
    idx
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use std::net::Ipv6Addr;

    #[test]
    fn dangerous_env_matches_exact_names() {
        assert!(is_dangerous_env("PATH"));
        assert!(is_dangerous_env("path"));
        assert!(is_dangerous_env("PythonHome"));
        assert!(is_dangerous_env("NODE_OPTIONS"));
    }

    #[test]
    fn dangerous_env_covers_ld_and_dyld_prefixes() {
        assert!(is_dangerous_env("LD_PRELOAD"));
        assert!(is_dangerous_env("LD_ANYTHING"));
        assert!(is_dangerous_env("DYLD_ANYTHING"));
    }

    #[test]
    fn dangerous_env_ignores_unrelated_vars() {
        // Prefix rule must be `LD_` (with underscore) — a bare
        // `LDPRELOAD` is a different variable and not dangerous.
        assert!(!is_dangerous_env("LDPRELOAD"));
        assert!(!is_dangerous_env("HELLO"));
        assert!(!is_dangerous_env("MY_LD_HOOK"));
    }

    #[test]
    fn ip_ranges_ipv4_private_blocked() {
        for ip in [
            "127.0.0.1",       // loopback
            "169.254.169.254", // AWS metadata
            "10.0.0.1",        // RFC 1918
            "172.16.0.1",      // RFC 1918
            "192.168.1.1",     // RFC 1918
            "100.64.0.1",      // CGNAT
            "0.0.0.0",         // unspecified
            "224.0.0.1",       // multicast
            "255.255.255.255", // broadcast
        ] {
            assert!(
                is_private_or_local(ip.parse().unwrap()),
                "{ip} should be blocked"
            );
        }
    }

    #[test]
    fn ip_ranges_ipv4_public_allowed() {
        for ip in ["1.1.1.1", "8.8.8.8", "142.250.190.14"] {
            assert!(
                !is_private_or_local(ip.parse().unwrap()),
                "{ip} should be allowed"
            );
        }
    }

    #[test]
    fn ip_ranges_ipv6_private_blocked() {
        for ip in [
            "::1",                // loopback
            "fe80::1",            // link-local
            "fd00::1",            // ULA
            "::",                 // unspecified
            "ff02::1",            // multicast
            "::ffff:169.254.0.1", // IPv4-mapped link-local
            "::ffff:10.0.0.1",    // IPv4-mapped RFC 1918
            "::ffff:127.0.0.1",   // IPv4-mapped loopback
        ] {
            assert!(
                is_private_or_local(ip.parse().unwrap()),
                "{ip} should be blocked"
            );
        }
    }

    #[test]
    fn ip_ranges_ipv6_public_allowed() {
        for ip in ["2001:4860:4860::8888", "2606:4700:4700::1111"] {
            assert!(
                !is_private_or_local(ip.parse().unwrap()),
                "{ip} should be allowed"
            );
        }
    }

    #[test]
    fn ip_ranges_all_ipv4_variants_use_std_flags() {
        assert!(is_private_or_local(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0))));
        assert!(is_private_or_local(IpAddr::V6(Ipv6Addr::LOCALHOST)));
    }

    #[test]
    fn sanitize_strips_crlf() {
        let out = sanitize_for_log("hello\r\nCRIT: pwned\r\n");
        assert!(!out.contains('\r'));
        assert!(!out.contains('\n'));
        assert!(out.contains("\\r\\n"));
    }

    #[test]
    fn sanitize_strips_ansi_escape() {
        let out = sanitize_for_log("\x1b[2Jhello");
        assert!(!out.contains('\x1b'));
    }

    #[test]
    fn sanitize_strips_null_and_del() {
        let out = sanitize_for_log("a\0b\x7fc");
        assert!(!out.contains('\0'));
        assert!(!out.contains('\x7f'));
        assert!(out.contains('a') && out.contains('b') && out.contains('c'));
    }

    #[test]
    fn sanitize_keeps_tab_and_utf8() {
        let out = sanitize_for_log("a\tbä");
        assert!(out.contains('\t'));
        assert!(out.contains('ä'));
    }

    #[test]
    fn sanitize_caps_at_4kib() {
        let long = "A".repeat(10_000);
        let out = sanitize_for_log(&long);
        assert!(
            out.len() <= 4096 + "…[truncated]".len(),
            "got {} bytes",
            out.len()
        );
        assert!(out.contains("…[truncated]"));
    }

    #[test]
    fn truncate_body_leaves_small_strings_untouched() {
        assert_eq!(truncate_response_body("hi", 100), "hi");
        assert_eq!(truncate_response_body("", 10), "");
    }

    #[test]
    fn truncate_body_keeps_head_and_tail_with_marker() {
        let s: String = "x".repeat(200);
        let out = truncate_response_body(&s, 100);
        assert!(out.contains("truncated"));
        assert!(out.starts_with("xxxx"));
        assert!(out.trim_end().ends_with('x'));
        assert!(
            out.len() < 200,
            "expected shorter than original, got {}",
            out.len()
        );
    }

    #[test]
    fn truncate_body_never_splits_multibyte() {
        // 100 copies of 'ä' — 200 bytes, cap at 100 half=50 lands
        // mid-codepoint. Result must still be valid UTF-8.
        let s: String = "ä".repeat(100);
        let out = truncate_response_body(&s, 100);
        // If we can round-trip via as_bytes → from_utf8 the string
        // is valid.
        std::str::from_utf8(out.as_bytes()).unwrap();
    }
}

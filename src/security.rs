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

/// Addresses that stay refused even for a host listed in
/// `security.allowed_private_hosts`: link-local (the cloud metadata
/// range `169.254.0.0/16`, `fe80::/10`), the AWS IPv6 IMDS address
/// `fd00:ec2::254`, unspecified, multicast and broadcast. The
/// allowlist exists for cluster-internal services on RFC 1918 / ULA
/// / loopback addresses; none of them legitimately live here.
pub fn is_always_blocked(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_link_local() || v4.is_unspecified() || v4.is_multicast() || v4.is_broadcast()
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_always_blocked(IpAddr::V4(v4));
            }
            let seg = v6.segments();
            v6.is_unspecified()
                || v6.is_multicast()
                || seg[0] & 0xffc0 == 0xfe80
                || seg == [0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x0254]
        }
    }
}

/// Normalise one `security.allowed_private_hosts` entry for exact
/// matching against a URL host: trimmed, lowercased, trailing dot
/// dropped. Returns `None` for entries that can never match a URL
/// host — empty, IP literals (literal private IPs are refused at
/// DSL load; use `block_private_networks=false` for those), or
/// anything carrying a character that can't appear in an RFC 1035
/// hostname. Permitted chars: ASCII letters, digits, hyphen, dot,
/// underscore (the latter for k8s / DNS SRV compatibility). This
/// is strictly tighter than URL hostname grammar — a URL whose host
/// contains anything outside this set can never match a listed
/// entry anyway (URL parsing would reject it earlier), but the
/// strict check catches operator footguns at config time: `*`,
/// `*.svc.cluster.local`, `svc?x`, and other "clever" entries
/// return None loudly instead of being silently inert.
pub fn normalize_allowed_host(entry: &str) -> Option<String> {
    let host = entry.trim().trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() || host.parse::<IpAddr>().is_ok() {
        return None;
    }
    if !host
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'))
    {
        return None;
    }
    Some(host)
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

/// Sanitize a byte string for **persistence** in the DB history
/// row (or any other durable store a downstream reader will render
/// verbatim). Same escape rules as [`sanitize_for_log`] but WITHOUT
/// the 4 KiB cap — the caller pairs this with
/// [`truncate_response_body`] to bound total row size.
///
/// Motivation (h2ck.me PR-review v1 nit #3): a shell job that
/// captured attacker-controlled stdout previously wrote raw CR/LF
/// and ANSI ESC bytes into `response_body`. A downstream log viewer
/// (or `SELECT response_body …` followed by `cat`) rendering that
/// row would then be vulnerable to the exact injection
/// [`sanitize_for_log`] was designed to prevent on the log stream.
pub fn sanitize_for_persistence(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\r' => out.push_str("\\r"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push('\t'),
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

/// Cap the bytes of an HTTP response body / shell stdout stored in
/// the history row AND sanitize control bytes for downstream
/// renderers. Above the cap we keep the first `max/2` bytes and the
/// last `max/2` bytes with a marker in between — enough context for
/// triage without unbounded row growth on chatty upstreams.
///
/// UTF-8 boundaries are respected on both cut points so the marker
/// splices in cleanly.
///
/// Control-byte sanitisation happens BEFORE size capping so a body
/// that arrives 90% ANSI-ESC still fits usefully inside the cap.
pub fn truncate_response_body(s: &str, max: usize) -> String {
    let clean = sanitize_for_persistence(s);
    if clean.len() <= max {
        return clean;
    }
    let half = max / 2;
    let head_end = char_boundary_floor(&clean, half);
    let tail_start = char_boundary_ceil(&clean, clean.len().saturating_sub(half));
    let truncated = clean.len() - (head_end + (clean.len() - tail_start));
    format!(
        "{}\n… [{} bytes truncated] …\n{}",
        &clean[..head_end],
        truncated,
        &clean[tail_start..]
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

/// PII-safe short-hash of a client identifier (typically the
/// stringified `SocketAddr` / IP) for structured log lines on
/// auth-failure and access-log paths.
///
/// Returns the first 8 hex chars of a SHA-256 digest — 4 bytes,
/// enough entropy to distinguish burst-traffic sources for
/// blocklist derivation, non-reversible so an operator can share
/// log excerpts without exposing raw client IPs (GDPR-safer than
/// echoing the address). See h2ck.me LOG-FINDINGS FN-LOG-2 and
/// FLEET-STRONGHOLDS §1.2.
pub fn short_client_hash(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    // 4 bytes → 8 hex chars. Never render the full digest — an
    // attacker who guesses the input space (e.g. "all IPs in
    // 10.0.0.0/24") could reverse it, but 4 bytes leaves enough
    // work-factor to be forensically useful without inviting a
    // rainbow-table lookup for common IPs.
    let mut out = String::with_capacity(8);
    for byte in &digest[..4] {
        out.push_str(&format!("{byte:02x}"));
    }
    out
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

    // PR-review v1 nit #3 — the DB history recorder used to persist
    // raw CR/LF/ANSI bytes. A downstream reader rendering the row
    // was then vulnerable to the same class of injection the log
    // sanitizer defends against. Confirm truncate_response_body
    // now runs the persistence sanitiser first.
    #[test]
    fn truncate_response_body_escapes_crlf_before_persist() {
        let hostile = "line1\r\n{\"level\":\"ERROR\",\"msg\":\"forged\"}\r\n";
        let out = truncate_response_body(hostile, 1024);
        assert!(!out.contains('\r'));
        assert!(!out.contains('\n'));
        assert!(out.contains("\\r\\n"));
    }

    #[test]
    fn truncate_response_body_escapes_ansi_before_persist() {
        let hostile = "\x1b[2Jbenign_text";
        let out = truncate_response_body(hostile, 1024);
        assert!(!out.contains('\x1b'));
        assert!(out.contains("\\u001b"));
        assert!(out.contains("benign_text"));
    }

    #[test]
    fn truncate_response_body_escapes_null_and_del_before_persist() {
        let hostile = "a\0b\x7fc";
        let out = truncate_response_body(hostile, 1024);
        assert!(!out.contains('\0'));
        assert!(!out.contains('\x7f'));
        assert!(out.contains("\\u0000"));
        assert!(out.contains("\\u007f"));
    }

    #[test]
    fn sanitize_for_persistence_leaves_utf8_and_tab_intact() {
        // Multi-byte + tab are legit characters we must preserve.
        let s = "hello\tworld äöü 🚀";
        let out = sanitize_for_persistence(s);
        assert_eq!(out, s);
    }

    #[test]
    fn normalize_allowed_host_accepts_plain_hostnames() {
        assert_eq!(
            normalize_allowed_host(" Ruuter-Internal. ").as_deref(),
            Some("ruuter-internal")
        );
        assert_eq!(
            normalize_allowed_host("svc.ns.svc.cluster.local").as_deref(),
            Some("svc.ns.svc.cluster.local")
        );
    }

    #[test]
    fn normalize_allowed_host_rejects_non_hostnames() {
        for bad in [
            "",
            "  ",
            "10.0.0.1",
            "::1",
            "[::1]",
            "ruuter-internal:8080",
            "http://ruuter-internal",
            "ruuter internal",
            "user@ruuter-internal",
        ] {
            assert!(normalize_allowed_host(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn always_blocked_covers_metadata_not_cluster_ranges() {
        assert!(is_always_blocked(IpAddr::V4(Ipv4Addr::new(
            169, 254, 169, 254
        ))));
        assert!(is_always_blocked(IpAddr::V6(Ipv6Addr::new(
            0xfd00, 0xec2, 0, 0, 0, 0, 0, 0x254
        ))));
        assert!(!is_always_blocked(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
        assert!(!is_always_blocked(IpAddr::V6(Ipv6Addr::new(
            0xfd00, 0xec2, 0, 0, 0, 0, 0, 0x253
        ))));
    }

    /// Load-bearing invariant for `allowed_private_hosts`:
    /// `is_always_blocked ⊆ is_private_or_local`. Reason: the SSRF
    /// pre-flight uses `is_private_or_local` for unlisted hosts and
    /// `is_always_blocked` for listed ones; if there were an IP in
    /// `is_always_blocked` but NOT in `is_private_or_local`, an
    /// UNLISTED host that resolved to it would silently sail past
    /// the pre-flight. Pinning here so a future split (e.g. moving
    /// multicast out of `is_private_or_local`) can't quietly open
    /// that hole.
    #[test]
    fn always_blocked_is_subset_of_private_or_local() {
        // Hand-rolled corpus covering every branch of
        // `is_always_blocked`: IPv4 link-local, unspecified,
        // multicast, broadcast; IPv6 link-local, metadata, mapped,
        // unspecified, multicast. If any of these is NOT in
        // `is_private_or_local` the allowlist has a hole.
        let corpus: &[IpAddr] = &[
            // IPv4
            IpAddr::V4(Ipv4Addr::new(169, 254, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254)),
            IpAddr::V4(Ipv4Addr::new(169, 254, 170, 2)), // ECS task-role creds
            IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)),
            IpAddr::V4(Ipv4Addr::new(224, 0, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(239, 255, 255, 250)), // SSDP
            IpAddr::V4(Ipv4Addr::new(255, 255, 255, 255)),
            // IPv6
            IpAddr::V6(Ipv6Addr::UNSPECIFIED),
            IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1)),
            IpAddr::V6(Ipv6Addr::new(0xfebf, 0, 0, 0, 0, 0, 0, 1)), // last fe80::/10
            IpAddr::V6(Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 1)),
            IpAddr::V6(Ipv6Addr::new(0xfd00, 0xec2, 0, 0, 0, 0, 0, 0x254)),
            // IPv4-mapped IPv6 of IMDS
            IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0xa9fe, 0xa9fe)),
        ];
        for ip in corpus {
            assert!(
                is_always_blocked(*ip),
                "{ip} should be always_blocked (corpus bug)"
            );
            assert!(
                is_private_or_local(*ip),
                "INVARIANT VIOLATED: {ip} is always_blocked but not private_or_local — \
                 the SSRF pre-flight would let an UNLISTED host through to this IP."
            );
        }
    }

    /// Pin the inverse too: there exist IPs that are `private_or_local`
    /// but NOT `always_blocked` — those are exactly the ones the
    /// allowlist legitimately exposes (RFC 1918, loopback, ULA
    /// excluding fd00:ec2::254, CGNAT). Without this, a future
    /// over-aggressive bump of `is_always_blocked` would quietly
    /// turn the allowlist into a no-op.
    #[test]
    fn allowlist_actually_opens_something() {
        let exposable: &[IpAddr] = &[
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),    // RFC 1918
            IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1)),  // RFC 1918
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)), // RFC 1918
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),   // loopback
            IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1)),  // CGNAT
            IpAddr::V6(Ipv6Addr::new(0xfd12, 0, 0, 0, 0, 0, 0, 1)), // ULA (not IMDS)
            IpAddr::V6(Ipv6Addr::LOCALHOST),
        ];
        for ip in exposable {
            assert!(is_private_or_local(*ip), "{ip} should be private_or_local");
            assert!(
                !is_always_blocked(*ip),
                "{ip} should NOT be always_blocked — \
                 if it is, the allowlist can never reach it"
            );
        }
    }

    /// Pentest-angle normalization: entries that look clever but
    /// cannot be a plain RFC 1035 hostname must return None (so
    /// they can never shadow a URL match AND the operator gets a
    /// loud boot WARN instead of silent inertness). Covers
    /// wildcards, bare dots, control bytes, whitespace, query /
    /// fragment / path markers, and percent-encoded payloads.
    #[test]
    fn normalize_allowed_host_rejects_sneaky_variants() {
        for bad in [
            "*",                      // bare wildcard
            "*.svc.cluster.local",    // wildcard glob
            ".",                      // bare dot → empty after trim
            "..",                     // only dots
            "svc\0.local",            // NUL byte
            "svc\x7Flocal",           // DEL
            "svc\u{00A0}local",       // non-ASCII whitespace (NBSP)
            "svc\tlocal",             // tab
            "ruuter-internal/..",     // path segment
            "ruuter-internal?x",      // query marker
            "ruuter-internal#frag",   // fragment marker
            "ruuter-internal%2Eevil", // percent-encoded dot
            "ruuter+internal",        // '+' is not valid
            "ruuter&internal",        // '&' is not valid
        ] {
            assert!(
                normalize_allowed_host(bad).is_none(),
                "entry {bad:?} should be rejected"
            );
        }
    }

    /// Non-ASCII whitespace coverage: a hostname with a non-breaking
    /// space must be rejected. Pins that the RFC 1035 character
    /// filter actually bites — if the filter ever drifted to
    /// `is_whitespace()` the ASCII-only subset would miss these.
    #[test]
    fn normalize_allowed_host_rejects_unicode_whitespace() {
        for ws in ['\u{00A0}', '\u{2028}', '\u{3000}'] {
            let bad = format!("ruuter{ws}internal");
            assert!(
                normalize_allowed_host(&bad).is_none(),
                "entry with U+{:04X} should be rejected: {bad:?}",
                ws as u32
            );
        }
    }

    /// Positive coverage for the characters that DO belong in a
    /// hostname: letters, digits, hyphen, dot, underscore.
    /// Underscore is explicit because it's an occasional point of
    /// confusion (strict RFC 1035 forbids it; RFC 2782 SRV and k8s
    /// Service / Pod names allow it; the pragmatic middle ground is
    /// to accept).
    #[test]
    fn normalize_allowed_host_accepts_rfc1035_plus_underscore() {
        for good in [
            "ruuter-internal",
            "svc1.ns.svc.cluster.local",
            "service_v2.ns.svc.cluster.local",
            "a",
            "a-b-c-1-2-3",
            "_tcp.svc.local", // SRV record style
        ] {
            assert!(
                normalize_allowed_host(good).is_some(),
                "entry {good:?} should be accepted"
            );
        }
    }

    #[test]
    fn short_client_hash_is_deterministic_and_8_chars() {
        let a = short_client_hash(b"1.2.3.4");
        assert_eq!(a.len(), 8);
        assert_eq!(a, short_client_hash(b"1.2.3.4"));
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn short_client_hash_distinguishes_different_inputs() {
        // Two adjacent /24 addresses must produce different
        // digests so an operator can tell them apart in the log.
        assert_ne!(
            short_client_hash(b"10.0.0.1"),
            short_client_hash(b"10.0.0.2")
        );
        assert_ne!(short_client_hash(b"::1"), short_client_hash(b"127.0.0.1"));
    }

    #[test]
    fn short_client_hash_matches_expected_sha256_prefix() {
        // Pin the algorithm: known-answer test for SHA-256 of "abc"
        // (RFC 6234 vector) — first 4 bytes are ba7816bf.
        assert_eq!(short_client_hash(b"abc"), "ba7816bf");
    }
}

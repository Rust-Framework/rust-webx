//! Fuzz logic for rust-webx public parse entry points.
//!
//! Every function in this module is an ordinary, side-effect-free Rust function:
//! it takes untrusted bytes and returns a typed outcome. The `#[test]` smoke
//! tests at the bottom run on stable (`cargo test --lib`), and the thin
//! `libfuzzer_sys::fuzz_target!` wrappers in `fuzz_targets/` only forward the
//! bytes here. Invariants live in these functions as `assert!`s: a violation is
//! a fuzzer crash, so the tested property is "the public parser never panics and
//! keeps its documented round-trip/containment contract on arbitrary input".
//!
//! Only public, network/DB-free APIs are fuzzed:
//!
//! | target | entry point |
//! |---|---|
//! | `cursor_decode` | `webx_core::keyset::Cursor::decode` |
//! | `config_bind` | `webx_core::config::try_bind_root` / `try_bind_config` |
//! | `client_ip` | `webx_core::client_info::{IpCidr, client_from_forwarded_for, resolve_client_info}` |
//! | `problem_details` | `webx_core::problem::ProblemDetails` serde round-trip |
//! | `file_name_sanitize` | `webx_core::form::sanitize_file_name` |
//! | `event_wire` | `webx_core::mediator::event::EventMessage::{from_wire, to_wire}` |

#![forbid(unsafe_code)]

use std::net::{IpAddr, SocketAddr};

use webx_core::client_info::{client_from_forwarded_for, resolve_client_info, IpCidr};
use webx_core::config::{try_bind_config, try_bind_root, AppOptions, AppSection, ServerSection};
use webx_core::form::sanitize_file_name;
use webx_core::keyset::{Cursor, CursorError, MAX_CURSOR_BYTES};
use webx_core::mediator::event::EventMessage;
use webx_core::problem::ProblemDetails;

/// Longest filename `sanitize_file_name` may return, in bytes (mirrors the
/// framework's documented bound; the fuzz target asserts it).
pub const MAX_SANITIZED_FILE_NAME_BYTES: usize = 200;

// ---------------------------------------------------------------------------
// 1. Keyset cursor (`Cursor::decode`)
// ---------------------------------------------------------------------------

/// Decode one client-supplied keyset cursor.
///
/// Invariants: a `TooLong` result only comes from an over-long (trimmed) input,
/// and every successfully decoded cursor re-encodes and decodes to itself.
pub fn fuzz_cursor_decode(data: &[u8]) -> Result<Vec<String>, CursorError> {
    let Ok(cursor) = std::str::from_utf8(data) else {
        // Cursors arrive from a UTF-8 query string; non-UTF-8 bytes cannot reach
        // the public entry point.
        return Err(CursorError::Encoding);
    };

    let decoded = Cursor::decode(cursor)?;
    let trimmed_len = cursor.trim().len();
    assert!(
        trimmed_len <= MAX_CURSOR_BYTES,
        "an accepted cursor must fit the documented {MAX_CURSOR_BYTES}-byte limit"
    );

    let refs: Vec<&str> = decoded.iter().map(String::as_str).collect();
    let re_encoded = Cursor::encode(&refs);
    assert_eq!(
        Cursor::decode(&re_encoded).as_deref(),
        Ok(decoded.as_slice()),
        "a decoded cursor must round-trip through Cursor::encode"
    );
    Ok(decoded)
}

// ---------------------------------------------------------------------------
// 2. Configuration binding (`try_bind_root` / `try_bind_config`)
// ---------------------------------------------------------------------------

/// How an arbitrary JSON document is handled by the strict config binder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigBindOutcome {
    /// The bytes are not JSON at all (the loader reports a parse error).
    InvalidJson,
    /// JSON, but the document is rejected by the strict binder.
    Rejected,
    /// JSON that binds to `AppOptions`.
    Bound,
}

/// Parse arbitrary bytes as JSON, then run the strict framework config binder.
///
/// Invariants: neither `serde_json` nor the strict binder may panic, and a
/// `Bound` document must keep binding when the same value is bound again (the
/// outcome has to be deterministic).
pub fn fuzz_config_bind(data: &[u8]) -> ConfigBindOutcome {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(data) else {
        return ConfigBindOutcome::InvalidJson;
    };

    let outcome = match try_bind_root::<AppOptions>(&value) {
        Ok(bound) => {
            // A framework section binds identically whether it is read through
            // the root document or through its own section; a mismatch means the
            // two public binders disagree about the same settings.
            let app: AppSection = try_bind_config(&value, "App")
                .expect("an `App` section accepted at the root must also bind on its own");
            assert_eq!(
                format!("{app:?}"),
                format!("{:?}", bound.app),
                "root and section binds must agree for `App`"
            );
            let server: ServerSection = try_bind_config(&value, "Server")
                .expect("a `Server` section accepted at the root must also bind on its own");
            assert_eq!(
                format!("{server:?}"),
                format!("{:?}", bound.server),
                "root and section binds must agree for `Server`"
            );
            ConfigBindOutcome::Bound
        }
        Err(_) => ConfigBindOutcome::Rejected,
    };

    let repeated = match try_bind_root::<AppOptions>(&value) {
        Ok(_) => ConfigBindOutcome::Bound,
        Err(_) => ConfigBindOutcome::Rejected,
    };
    assert_eq!(outcome, repeated, "config binding must be deterministic");
    outcome
}

// ---------------------------------------------------------------------------
// 3. Trusted client address parsing (`IpCidr` / `X-Forwarded-For`)
// ---------------------------------------------------------------------------

/// Canonicalise an address the same way the client-info module documents
/// (`::ffff:a.b.c.d` is reported as IPv4) so containment checks can be made.
fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        IpAddr::V4(_) => ip,
    }
}

/// Fuzz the forwarded-header parser.
///
/// Input format (documented so seeds can be handwritten): the first line is a
/// comma-separated trusted-proxy CIDR list, the rest is one `X-Forwarded-For`
/// value. A missing newline means "no trusted proxies" and the whole input is
/// the header.
///
/// Invariants: a returned client address must be one of the hops in the chain
/// (never a fabricated address), and when the direct peer is a trusted proxy the
/// reported `forwarded_for` chain must match the parsed hops.
pub fn fuzz_client_ip(data: &[u8]) -> Option<IpAddr> {
    let text = std::str::from_utf8(data).ok()?;
    let (trusted_part, header) = match text.split_once('\n') {
        Some((trusted, forwarded)) => (trusted, forwarded.trim()),
        None => ("", text.trim()),
    };

    let trusted: Vec<IpCidr> = trusted_part
        .split(',')
        .filter_map(|entry| IpCidr::parse(entry.trim()))
        .collect();

    let hops: Vec<IpAddr> = header
        .split(',')
        .filter_map(|part| part.trim().parse::<IpAddr>().ok())
        .map(canonical)
        .collect();

    let client = client_from_forwarded_for(header, &trusted);
    if let Some(client) = client {
        assert!(
            hops.contains(&client),
            "resolved client {client} is not a hop of the forwarded chain {hops:?}"
        );
    }

    // A peer taken from the configured trusted range exercises the full
    // resolver: the forwarded chain must then be surfaced verbatim.
    let peer_ip = trusted_part.split(',').find_map(|entry| {
        entry
            .trim()
            .split('/')
            .next()?
            .trim()
            .parse::<IpAddr>()
            .ok()
    });
    if let Some(peer_ip) = peer_ip {
        if trusted.iter().any(|cidr| cidr.contains(peer_ip)) {
            let peer = SocketAddr::new(peer_ip, 443);
            let info = resolve_client_info(Some(peer), Some(header), Some(header), &trusted);
            assert_eq!(info.peer_addr, Some(peer));
            assert_eq!(
                info.forwarded_for, hops,
                "a trusted peer must surface exactly the parsed X-Forwarded-For hops"
            );
            assert!(
                info.client_ip.is_some(),
                "the resolver always keeps a peer/client address"
            );
        }
    }

    client
}

// ---------------------------------------------------------------------------
// 4. Problem Details JSON round-trip (`ProblemDetails` serde)
// ---------------------------------------------------------------------------

/// Parse an RFC 9457 body as `ProblemDetails` and round-trip it.
///
/// Invariants: accepted JSON must serialize back and re-parse, and
/// `ProblemDetails::complete` must always install the `code`/`retryable`
/// extension keys the framework guarantees.
pub fn fuzz_problem_details(data: &[u8]) -> Option<ProblemDetails> {
    let mut problem: ProblemDetails = serde_json::from_slice(data).ok()?;

    let json = serde_json::to_vec(&problem).expect("ProblemDetails must serialize");
    let reparsed: ProblemDetails =
        serde_json::from_slice(&json).expect("serialized ProblemDetails must re-parse");
    assert_eq!(
        problem.status, reparsed.status,
        "status survives a ProblemDetails JSON round-trip"
    );
    assert_eq!(
        problem.title, reparsed.title,
        "title survives a ProblemDetails JSON round-trip"
    );

    problem.complete(Some("/fuzz"), Some("trace-fuzz"));
    let extensions = problem
        .extensions
        .as_ref()
        .expect("complete() must install extension members");
    assert!(
        extensions.contains_key("code"),
        "complete() must always fill the stable `code` extension"
    );
    assert!(
        extensions.contains_key("retryable"),
        "complete() must always fill the `retryable` extension"
    );
    let value = serde_json::to_value(&problem).expect("a completed problem serializes");
    assert!(value.get("title").is_some(), "problem body keeps `title`");
    assert!(value.get("status").is_some(), "problem body keeps `status`");
    Some(problem)
}

// ---------------------------------------------------------------------------
// 5. Upload filename sanitisation (`sanitize_file_name`)
// ---------------------------------------------------------------------------

/// Sanitise a client-supplied upload filename.
///
/// Invariants: the result is a non-empty, bounded basename with no path
/// separator, no control character, no Windows-reserved device name and no
/// trailing dot/space — and sanitising it again changes nothing.
pub fn fuzz_file_name_sanitize(data: &[u8]) -> Option<String> {
    let raw = std::str::from_utf8(data).ok()?;
    let sanitized = sanitize_file_name(raw);

    assert!(
        !sanitized.is_empty(),
        "sanitize_file_name never returns empty"
    );
    assert!(
        sanitized.len() <= MAX_SANITIZED_FILE_NAME_BYTES,
        "sanitize_file_name must honour the {MAX_SANITIZED_FILE_NAME_BYTES}-byte bound"
    );
    assert!(
        !sanitized.contains(['/', '\\']),
        "sanitize_file_name must strip path separators: {sanitized:?}"
    );
    assert!(
        !sanitized.chars().any(char::is_control),
        "sanitize_file_name must strip control characters: {sanitized:?}"
    );
    assert!(
        sanitized != "." && sanitized != "..",
        "sanitize_file_name must never produce a traversal component"
    );
    assert!(
        !sanitized.ends_with(['.', ' ']),
        "sanitize_file_name must not end with a dot or space: {sanitized:?}"
    );
    assert!(
        !is_windows_reserved(&sanitized),
        "sanitize_file_name must not return a Windows device name: {sanitized:?}"
    );
    assert_eq!(
        sanitize_file_name(&sanitized),
        sanitized,
        "sanitize_file_name must be idempotent"
    );
    Some(sanitized)
}

/// Mirrors the reserved-device rule the framework promises to remove; kept here
/// so the fuzz invariant is independent of the implementation's helper.
fn is_windows_reserved(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or(name)
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|suffix| suffix.len() == 1 && matches!(suffix.as_bytes()[0], b'1'..=b'9'))
}

// ---------------------------------------------------------------------------
// 6. Broker event envelope (`EventMessage::from_wire` / `to_wire`)
// ---------------------------------------------------------------------------

/// Decode one binary event envelope (`version || u32 BE name length || name ||
/// payload`) and prove it re-encodes byte-for-byte.
///
/// Invariants: an accepted frame round-trips exactly, so a decoded message can
/// never silently drop or rewrite payload bytes.
pub fn fuzz_event_wire(data: &[u8]) -> Option<EventMessage> {
    let message = EventMessage::from_wire(data).ok()?;
    let re_encoded = message
        .to_wire()
        .expect("a frame that decoded within the wire limits must re-encode");
    assert_eq!(
        re_encoded, data,
        "EventMessage::from_wire must preserve the exact wire bytes"
    );
    Some(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_round_trip_and_malformed_input() {
        let cursor = Cursor::encode(&["2026-11-22T12:00:00Z", "42"]);
        assert_eq!(
            fuzz_cursor_decode(cursor.as_bytes()),
            Ok(vec!["2026-11-22T12:00:00Z".into(), "42".into()])
        );
        assert_eq!(
            fuzz_cursor_decode(b"not base64!!"),
            Err(CursorError::Encoding)
        );
        let too_long = "a".repeat(MAX_CURSOR_BYTES + 1);
        assert_eq!(
            fuzz_cursor_decode(too_long.as_bytes()),
            Err(CursorError::TooLong)
        );
    }

    #[test]
    fn config_bind_classifies_documents() {
        assert_eq!(fuzz_config_bind(b"{"), ConfigBindOutcome::InvalidJson);
        assert_eq!(fuzz_config_bind(b"null"), ConfigBindOutcome::Rejected);
        assert_eq!(fuzz_config_bind(b"{}"), ConfigBindOutcome::Bound);
        let valid = br#"{
            "App": {"Name": "fuzz", "Urls": ["http://127.0.0.1:5000"], "MaxBodySize": 1024},
            "Server": {"Workers": 2, "RequestTimeoutSeconds": 5},
            "Jwt": {"ExpirationMinutes": 15}
        }"#;
        assert_eq!(fuzz_config_bind(valid), ConfigBindOutcome::Bound);
        // A typo inside a framework-owned section is a startup error by design.
        let typo = br#"{"RateLimit": {"RequestsPerSecondd": 100}}"#;
        assert_eq!(fuzz_config_bind(typo), ConfigBindOutcome::Rejected);
    }

    #[test]
    fn client_ip_resolves_the_rightmost_untrusted_hop() {
        let input = b"10.0.0.0/8,2001:db8::/32\n203.0.113.7, 10.0.0.2, 10.0.0.3";
        assert_eq!(
            fuzz_client_ip(input),
            Some("203.0.113.7".parse().expect("ip"))
        );
        assert_eq!(fuzz_client_ip(b"10.0.0.0/8\n10.0.0.1, 10.0.0.2"), None);
    }

    #[test]
    fn problem_details_round_trips() {
        let body = br#"{
            "type": "https://httpstatuses.com/404",
            "title": "Resource Not Found",
            "status": 404,
            "detail": "thing '1' not found",
            "instance": "/things/1",
            "code": "not_found",
            "retryable": false
        }"#;
        let parsed = fuzz_problem_details(body).expect("valid problem");
        assert_eq!(parsed.status, 404);
        assert_eq!(parsed.code(), Some("not_found"));
        assert!(fuzz_problem_details(b"{").is_none());
    }

    #[test]
    fn file_names_are_sanitized_to_a_safe_basename() {
        assert_eq!(
            fuzz_file_name_sanitize(b"../../etc/passwd").as_deref(),
            Some("passwd")
        );
        assert_eq!(
            fuzz_file_name_sanitize(br"C:\Windows\System32\CON").as_deref(),
            Some("_CON")
        );
        assert_eq!(fuzz_file_name_sanitize(b"..").as_deref(), Some("file"));
        assert_eq!(fuzz_file_name_sanitize(b"").as_deref(), Some("file"));
        let long = "a".repeat(MAX_SANITIZED_FILE_NAME_BYTES + 50);
        let sanitized = fuzz_file_name_sanitize(long.as_bytes()).expect("utf-8");
        assert!(sanitized.len() <= MAX_SANITIZED_FILE_NAME_BYTES);
    }

    #[test]
    fn event_frames_round_trip() {
        let message = EventMessage::new("UserCreatedEvent", br#"{"id":"1"}"#.to_vec());
        let wire = message.to_wire().expect("encode");
        let decoded = fuzz_event_wire(&wire).expect("decode");
        assert_eq!(decoded.event_name, "UserCreatedEvent");
        assert_eq!(fuzz_event_wire(b""), None);
        assert_eq!(fuzz_event_wire(&[2, 0, 0, 0, 0]), None);
    }

    /// Every committed seed must still be accepted by its target without
    /// tripping an invariant, so a broken/renamed seed fails `cargo test` instead
    /// of silently shrinking the fuzz corpus.
    #[test]
    fn committed_seed_corpus_is_exercised() {
        let cases: [(&str, fn(&[u8])); 6] = [
            ("cursor_decode", |data| {
                let _ = fuzz_cursor_decode(data);
            }),
            ("config_bind", |data| {
                let _ = fuzz_config_bind(data);
            }),
            ("client_ip", |data| {
                let _ = fuzz_client_ip(data);
            }),
            ("problem_details", |data| {
                let _ = fuzz_problem_details(data);
            }),
            ("file_name_sanitize", |data| {
                let _ = fuzz_file_name_sanitize(data);
            }),
            ("event_wire", |data| {
                let _ = fuzz_event_wire(data);
            }),
        ];

        let corpus = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("corpus");
        for (target, run) in cases {
            let dir = corpus.join(target);
            let mut seeds = 0usize;
            for entry in std::fs::read_dir(&dir)
                .unwrap_or_else(|error| panic!("corpus/{target} is missing: {error}"))
            {
                let path = entry.expect("corpus entry").path();
                if path.is_file() {
                    run(&std::fs::read(&path).expect("read seed"));
                    seeds += 1;
                }
            }
            assert!(seeds > 0, "corpus/{target} must commit at least one seed");
        }
    }

    /// Deterministic PRNG-driven smoke across every target, so `cargo test`
    /// exercises deep paths even where libFuzzer cannot run (for example
    /// Windows/MSVC). This is not a replacement for `cargo fuzz`; it only
    /// catches invariant mistakes early.
    #[test]
    fn randomized_inputs_do_not_trip_invariants() {
        fn next(seed: &mut u64) -> u64 {
            *seed ^= *seed << 13;
            *seed ^= *seed >> 7;
            *seed ^= *seed << 17;
            *seed
        }

        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        for _ in 0..20_000 {
            let len = (next(&mut seed) % 128) as usize;
            let mut buf = vec![0u8; len];
            for byte in buf.iter_mut() {
                let value = (next(&mut seed) & 0xff) as u8;
                // Most bytes stay printable: JSON/base64/UTF-8 entry points need
                // text-shaped input to reach the interesting branches.
                *byte = if next(&mut seed) % 8 == 0 {
                    value
                } else {
                    b' ' + (value % 95)
                };
            }
            let _ = fuzz_cursor_decode(&buf);
            let _ = fuzz_config_bind(&buf);
            let _ = fuzz_client_ip(&buf);
            let _ = fuzz_problem_details(&buf);
            let _ = fuzz_file_name_sanitize(&buf);
            let _ = fuzz_event_wire(&buf);
        }
    }
}

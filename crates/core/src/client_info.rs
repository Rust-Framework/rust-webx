//! Trusted client information for one request.
//!
//! [`ClientInfo`] answers "where did this request come from" for audit, login
//! throttling and risk scoring. It separates three things that are easy to
//! confuse:
//!
//! * `peer_addr` — the TCP peer the listener observed; always trustworthy but
//!   usually a reverse proxy in production;
//! * `client_ip` — the address business code should use: the peer, or (only when
//!   the peer is a configured trusted proxy) the right-most untrusted
//!   `X-Forwarded-For` hop;
//! * `forwarded_for` — the forwarded chain, **empty unless forwarding was
//!   trusted** (raw client-supplied headers are never exposed as if trusted).
//!
//! This module is the framework's single forwarded-header parser: the host's
//! `ClientInfo` middleware calls [`resolve_client_info`], stores the result on
//! the request context, and every other middleware (rate limiting included)
//! reads that resolved value instead of parsing headers a second time.
//!
//! Headers are only honoured when the host's `ClientInfo` configuration enables
//! `TrustForwardedHeaders` and the direct peer belongs to `TrustedProxies`; a
//! client that can reach the server directly can otherwise rotate
//! `X-Forwarded-For` to spoof its address.

use std::net::{IpAddr, SocketAddr};

/// Resolved client information for one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientInfo {
    /// TCP peer address observed by the listener.
    pub peer_addr: Option<SocketAddr>,
    /// Address business code should use (peer, or a trusted forwarded hop).
    pub client_ip: Option<IpAddr>,
    /// Forwarded chain (`X-Forwarded-For`), left-to-right; empty when the
    /// forwarded headers were not trusted.
    pub forwarded_for: Vec<IpAddr>,
}

impl ClientInfo {
    /// Info for a request that did not go through forwarded-header resolution.
    ///
    /// The peer is the client; the forwarded chain is deliberately empty. An
    /// IPv4-mapped IPv6 peer (`::ffff:a.b.c.d`, reported by dual-stack
    /// listeners) is reported in its IPv4 form so it matches IPv4 trusted-proxy
    /// ranges and rate-limit keys consistently.
    pub fn from_peer(peer_addr: Option<SocketAddr>) -> Self {
        Self {
            peer_addr,
            client_ip: peer_addr.map(|peer| canonical_ip(peer.ip())),
            forwarded_for: Vec::new(),
        }
    }
}

/// Canonical form of an address for trusted-proxy and client-key comparisons.
///
/// `Ipv6Addr::to_ipv4_mapped` covers the `::ffff:a.b.c.d` form that dual-stack
/// sockets report for IPv4 peers; treating it as IPv4 keeps a configured
/// `10.0.0.0/8` trusted proxy (and therefore forwarded-header trust) working.
fn canonical_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        IpAddr::V4(_) => ip,
    }
}

/// One CIDR range of trusted reverse proxies (for example `10.0.0.0/8`).
///
/// Parsing is deliberately strict: the value must be `address/prefix-length`,
/// the prefix must fit the address family (`0..=32` for IPv4, `0..=128` for
/// IPv6), and a bare address is rejected (spell it `/32` or `/128`). The host
/// skips a malformed entry with a warning instead of disabling trust
/// altogether, so one typo cannot silently turn every client into a proxy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IpCidr {
    network: IpAddr,
    prefix: u8,
}

impl IpCidr {
    /// Parse `address/prefix-length`.
    pub fn parse(value: &str) -> Option<Self> {
        let (addr, prefix) = value.split_once('/')?;
        let network: IpAddr = addr.trim().parse().ok()?;
        let prefix: u8 = prefix.trim().parse().ok()?;
        let max_prefix = match network {
            IpAddr::V4(_) => 32,
            IpAddr::V6(_) => 128,
        };
        (prefix <= max_prefix).then_some(Self { network, prefix })
    }

    /// Whether `ip` belongs to the range.
    ///
    /// An IPv4-mapped IPv6 address (`::ffff:a.b.c.d`) is compared in its IPv4
    /// form, so a dual-stack listener peer matches an IPv4 trusted-proxy range.
    /// Otherwise an address of a different family is never contained; a `/0`
    /// range contains every address of its own family.
    pub fn contains(&self, ip: IpAddr) -> bool {
        match (canonical_ip(self.network), canonical_ip(ip)) {
            (IpAddr::V4(network), IpAddr::V4(ip)) => {
                let mask = if self.prefix == 0 {
                    0
                } else {
                    u32::MAX << (32 - self.prefix)
                };
                (u32::from(network) & mask) == (u32::from(ip) & mask)
            }
            (IpAddr::V6(network), IpAddr::V6(ip)) => {
                let mask = if self.prefix == 0 {
                    0
                } else {
                    u128::MAX << (128 - self.prefix)
                };
                (u128::from(network) & mask) == (u128::from(ip) & mask)
            }
            _ => false,
        }
    }
}

/// Parse a comma-separated `X-Forwarded-For` value into hops (left to right).
///
/// Hops are canonicalised (IPv4-mapped IPv6 becomes IPv4) so trusted-range
/// checks and the reported client address are family-consistent.
fn parse_forwarded_for(value: &str) -> Vec<IpAddr> {
    value
        .split(',')
        .filter_map(|part| part.trim().parse::<IpAddr>().ok().map(canonical_ip))
        .collect()
}

/// The right-most hop that is not itself a trusted proxy.
fn rightmost_untrusted_hop(hops: &[IpAddr], trusted: &[IpCidr]) -> Option<IpAddr> {
    hops.iter()
        .rev()
        .find(|ip| !trusted.iter().any(|cidr| cidr.contains(**ip)))
        .copied()
}

/// The right-most address in `X-Forwarded-For` that is not a trusted proxy.
///
/// Proxies append, so the right-most entries are the ones closest to the
/// server; skipping trusted proxies from the right yields the real client.
///
/// Returns `None` when no hop is outside the trusted proxy ranges: such a chain
/// is either incomplete or forged (a client inside the trusted network could
/// prepend arbitrary addresses), so callers must fall back to the peer address
/// instead of trusting the left-most hop.
pub fn client_from_forwarded_for(value: &str, trusted: &[IpCidr]) -> Option<IpAddr> {
    rightmost_untrusted_hop(&parse_forwarded_for(value), trusted)
}

/// Resolve the trusted client information for one request.
///
/// This is the single implementation of the forwarded-header rules; the host's
/// `ClientInfo` middleware is a thin adapter over it.
///
/// * the peer is untrusted (or absent) → [`ClientInfo::from_peer`] semantics:
///   forwarded headers are ignored and the chain stays empty;
/// * the peer belongs to `trusted` → `X-Forwarded-For` is parsed into
///   [`ClientInfo::forwarded_for`], and `client_ip` becomes its right-most hop
///   outside the trusted ranges (a client that prepends a spoofed address never
///   wins);
/// * when that yields nothing, `X-Real-IP` is used;
/// * otherwise `client_ip` stays the peer address.
pub fn resolve_client_info(
    peer: Option<SocketAddr>,
    forwarded_for: Option<&str>,
    real_ip: Option<&str>,
    trusted: &[IpCidr],
) -> ClientInfo {
    let mut info = ClientInfo::from_peer(peer);
    let Some(peer) = peer else {
        return info;
    };
    let peer_ip = canonical_ip(peer.ip());
    if !trusted.iter().any(|cidr| cidr.contains(peer_ip)) {
        return info;
    }

    if let Some(header) = forwarded_for {
        let hops = parse_forwarded_for(header);
        if let Some(client) = rightmost_untrusted_hop(&hops, trusted) {
            info.client_ip = Some(client);
        }
        info.forwarded_for = hops;
    }
    if info.client_ip == Some(peer_ip) {
        // No usable forwarded address: fall back to `X-Real-IP` when the direct
        // peer is a trusted proxy.
        if let Some(real) = real_ip {
            if let Ok(client) = real.trim().parse::<IpAddr>() {
                info.client_ip = Some(canonical_ip(client));
            }
        }
    }
    info
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn peer(ip: &str) -> SocketAddr {
        SocketAddr::new(ip.parse().expect("valid ip"), 5000)
    }

    fn trusted(values: &[&str]) -> Vec<IpCidr> {
        values
            .iter()
            .map(|value| IpCidr::parse(value).expect("valid cidr"))
            .collect()
    }

    #[test]
    fn an_ipv4_mapped_peer_is_trusted_and_resolved_as_ipv4() {
        // Dual-stack listener: the trusted proxy connects as ::ffff:10.0.0.5.
        let peer = SocketAddr::new("::ffff:10.0.0.5".parse().expect("peer"), 4433);
        let info = resolve_client_info(
            Some(peer),
            Some("203.0.113.7"),
            None,
            &trusted(&["10.0.0.0/8"]),
        );
        assert_eq!(
            info.client_ip,
            Some("203.0.113.7".parse().unwrap()),
            "the forwarded client must be trusted for an IPv4-mapped proxy peer"
        );
        // A trusted chain only (no untrusted hop) keeps the canonical peer.
        let chain_only = resolve_client_info(
            Some(peer),
            Some("10.0.0.9, ::ffff:10.0.0.5"),
            None,
            &trusted(&["10.0.0.0/8"]),
        );
        assert_eq!(chain_only.client_ip, Some("10.0.0.5".parse().unwrap()));
    }

    #[test]
    fn from_peer_never_exposes_forwarded_headers() {
        let peer = SocketAddr::from((Ipv4Addr::new(10, 0, 0, 1), 5000));
        let info = ClientInfo::from_peer(Some(peer));
        assert_eq!(info.peer_addr, Some(peer));
        assert_eq!(info.client_ip, Some(peer.ip()));
        assert!(info.forwarded_for.is_empty());

        let anonymous = ClientInfo::from_peer(None);
        assert_eq!(anonymous.peer_addr, None);
        assert_eq!(anonymous.client_ip, None);
    }

    #[test]
    fn cidr_matches_only_inside_the_range() {
        let cidr = IpCidr::parse("10.0.0.0/8").expect("valid cidr");
        assert!(cidr.contains("10.1.2.3".parse().unwrap()));
        assert!(!cidr.contains("11.0.0.1".parse().unwrap()));

        // A non-normalised network still matches by mask.
        let cidr = IpCidr::parse("192.168.1.77/24").expect("valid cidr");
        assert!(cidr.contains("192.168.1.1".parse().unwrap()));
        assert!(!cidr.contains("192.168.2.1".parse().unwrap()));

        // `/0` matches every address of its own family only.
        assert!(IpCidr::parse("0.0.0.0/0")
            .unwrap()
            .contains("203.0.113.9".parse().unwrap()));
        assert!(!IpCidr::parse("0.0.0.0/0")
            .unwrap()
            .contains("2001:db8::1".parse().unwrap()));
        assert!(IpCidr::parse("::/0")
            .unwrap()
            .contains("2001:db8::1".parse().unwrap()));

        // A dual-stack listener reports IPv4 peers as IPv4-mapped IPv6; those
        // are compared in their IPv4 form so IPv4 trusted ranges keep working.
        let mapped: IpAddr = "::ffff:10.1.2.3".parse().expect("mapped ipv4");
        assert!(IpCidr::parse("10.0.0.0/8").unwrap().contains(mapped));
        assert!(!IpCidr::parse("2001:db8::/32").unwrap().contains(mapped));
        assert!(!IpCidr::parse("::/0").unwrap().contains(mapped));

        assert!(IpCidr::parse("not-a-cidr").is_none());
        assert!(
            IpCidr::parse("10.0.0.0").is_none(),
            "a bare address is not a CIDR"
        );
        assert!(IpCidr::parse("10.0.0.0/33").is_none());
        assert!(IpCidr::parse("2001:db8::/129").is_none());
    }

    #[test]
    fn cidr_supports_ipv6() {
        let v6 = IpCidr::parse("2001:db8::/32").expect("valid v6 cidr");
        assert!(v6.contains("2001:db8::1".parse().unwrap()));
        assert!(!v6.contains("2001:db9::1".parse().unwrap()));
    }

    #[test]
    fn forwarded_for_skips_trusted_proxies_from_the_right() {
        let proxies = trusted(&["10.0.0.0/8"]);
        assert_eq!(
            client_from_forwarded_for("203.0.113.7, 10.0.0.2, 10.0.0.3", &proxies),
            Some("203.0.113.7".parse().unwrap()),
            "the right-most untrusted hop is the client"
        );
        assert_eq!(
            client_from_forwarded_for("1.1.1.1, 203.0.113.7, 10.0.0.3", &proxies),
            Some("203.0.113.7".parse().unwrap()),
            "a spoofed left-most entry must not win"
        );
        assert_eq!(
            client_from_forwarded_for("10.0.0.1, 10.0.0.3", &proxies),
            None,
            "a chain of trusted proxies only is incomplete or forged: fall back to the peer"
        );
        assert_eq!(
            client_from_forwarded_for("not-an-ip, 2001:db8::1", &proxies),
            Some("2001:db8::1".parse().unwrap()),
            "malformed hops are skipped"
        );
        assert_eq!(client_from_forwarded_for("", &proxies), None);
    }

    #[test]
    fn trusted_proxy_resolves_the_right_most_untrusted_hop() {
        let trusted = trusted(&["10.0.0.0/8"]);
        let info = resolve_client_info(
            Some(peer("10.0.0.5")),
            Some("203.0.113.7, 10.0.0.2, 10.0.0.3"),
            None,
            &trusted,
        );
        assert_eq!(info.peer_addr, Some(peer("10.0.0.5")));
        assert_eq!(info.client_ip, Some("203.0.113.7".parse().unwrap()));
        assert_eq!(
            info.forwarded_for,
            vec![
                "203.0.113.7".parse::<IpAddr>().unwrap(),
                "10.0.0.2".parse().unwrap(),
                "10.0.0.3".parse().unwrap(),
            ]
        );
    }

    #[test]
    fn spoofed_prefix_does_not_win() {
        let trusted = trusted(&["10.0.0.0/8"]);
        let info = resolve_client_info(
            Some(peer("10.0.0.5")),
            Some("1.1.1.1, 203.0.113.7, 10.0.0.3"),
            None,
            &trusted,
        );
        assert_eq!(info.client_ip, Some("203.0.113.7".parse().unwrap()));
    }

    #[test]
    fn all_trusted_chain_falls_back_to_the_peer() {
        let trusted = trusted(&["10.0.0.0/8"]);
        let info = resolve_client_info(
            Some(peer("10.0.0.5")),
            Some("10.0.0.1, 10.0.0.3"),
            None,
            &trusted,
        );
        assert_eq!(info.client_ip, Some("10.0.0.5".parse().unwrap()));
        assert_eq!(info.forwarded_for.len(), 2);
    }

    #[test]
    fn untrusted_peer_ignores_forwarded_headers() {
        let trusted = trusted(&["10.0.0.0/8"]);
        let info = resolve_client_info(
            Some(peer("203.0.113.9")),
            Some("1.1.1.1"),
            Some("2.2.2.2"),
            &trusted,
        );
        assert_eq!(info.client_ip, Some("203.0.113.9".parse().unwrap()));
        assert!(
            info.forwarded_for.is_empty(),
            "client-controlled headers must never surface as trusted"
        );
    }

    #[test]
    fn no_peer_returns_peer_only_semantics() {
        let trusted = trusted(&["10.0.0.0/8"]);
        let info = resolve_client_info(None, Some("203.0.113.7"), Some("203.0.113.8"), &trusted);
        assert_eq!(info.peer_addr, None);
        assert_eq!(info.client_ip, None);
        assert!(info.forwarded_for.is_empty());
    }

    #[test]
    fn real_ip_is_used_when_forwarded_for_yields_nothing() {
        let trusted = trusted(&["10.0.0.0/8"]);
        let info = resolve_client_info(
            Some(peer("10.0.0.5")),
            Some("10.0.0.1, 10.0.0.3"),
            Some("203.0.113.7"),
            &trusted,
        );
        assert_eq!(info.client_ip, Some("203.0.113.7".parse().unwrap()));
        assert_eq!(info.forwarded_for.len(), 2);

        let info = resolve_client_info(Some(peer("10.0.0.5")), None, Some("203.0.113.8"), &trusted);
        assert_eq!(info.client_ip, Some("203.0.113.8".parse().unwrap()));
        assert!(info.forwarded_for.is_empty());
    }

    #[test]
    fn empty_trusted_list_trusts_nobody() {
        let info = resolve_client_info(Some(peer("10.0.0.5")), Some("203.0.113.7"), None, &[]);
        assert_eq!(info.client_ip, Some("10.0.0.5".parse().unwrap()));
        assert!(info.forwarded_for.is_empty());
    }

    #[test]
    fn ipv6_trusted_proxy_chain_resolves() {
        let trusted = trusted(&["2001:db8:1::/48"]);
        let peer = SocketAddr::new("2001:db8:1::5".parse().expect("valid ipv6 peer"), 443);
        let info = resolve_client_info(
            Some(peer),
            Some("2400:cb00::1, 2001:db8:1::2"),
            None,
            &trusted,
        );
        assert_eq!(info.client_ip, Some("2400:cb00::1".parse().unwrap()));
        assert_eq!(info.forwarded_for.len(), 2);
    }
}

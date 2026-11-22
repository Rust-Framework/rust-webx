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
    /// The peer is the client; the forwarded chain is deliberately empty.
    pub fn from_peer(peer_addr: Option<SocketAddr>) -> Self {
        Self {
            peer_addr,
            client_ip: peer_addr.map(|peer| peer.ip()),
            forwarded_for: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

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
}

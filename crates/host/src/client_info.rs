//! Resolve trusted client information before the endpoint runs.
//!
//! Installed only when `ClientInfo.TrustForwardedHeaders` is enabled. It reads
//! the listener peer address and, **only when that peer belongs to a configured
//! trusted proxy**, the `X-Forwarded-For` / `X-Real-IP` headers:
//!
//! * `client_ip` becomes the right-most `X-Forwarded-For` hop that is not a
//!   trusted proxy (a client cannot prepend a spoofed address and win);
//! * `forwarded_for` exposes the parsed chain (left-to-right);
//! * when forwarding is not trusted the forwarded chain stays empty, so business
//!   code can never mistake attacker-controlled headers for the client address.
//!
//! Handlers read the result through a `#[fromclient] client: ClientInfo` request
//! field; the binder calls [`IHttpContext::client_info`].

use std::net::IpAddr;
use std::ops::ControlFlow;

use async_trait::async_trait;

use webx_core::client_info::ClientInfo;
use webx_core::config::ClientInfoSection;
use webx_core::error::Result;
use webx_core::http::IHttpContext;
use webx_core::middleware::IMiddleware;

use crate::rate_limit::{client_from_forwarded_for, IpCidr};

/// Client-info resolution middleware.
pub struct ClientInfoMiddleware {
    trusted_proxies: Vec<IpCidr>,
}

impl ClientInfoMiddleware {
    /// Build from the `ClientInfo` configuration section.
    ///
    /// Invalid CIDRs are ignored with a warning (the same policy as the rate
    /// limiter: a malformed entry must not disable the rest).
    pub fn from_section(config: &ClientInfoSection) -> Self {
        let trusted_proxies = config
            .trusted_proxies
            .iter()
            .filter_map(|value| match IpCidr::parse(value) {
                Some(cidr) => Some(cidr),
                None => {
                    tracing::warn!(
                        value,
                        "ClientInfo.TrustedProxies entry is not a valid CIDR; ignoring it"
                    );
                    None
                }
            })
            .collect();
        Self { trusted_proxies }
    }

    fn resolve(&self, ctx: &dyn IHttpContext) -> ClientInfo {
        let peer = ctx.request().remote_addr();
        let mut info = ClientInfo::from_peer(peer);

        let peer_trusted = peer.is_some_and(|peer| {
            self.trusted_proxies
                .iter()
                .any(|cidr| cidr.contains(peer.ip()))
        });
        if !peer_trusted {
            return info;
        }

        if let Some(forwarded) = ctx.request().header("x-forwarded-for") {
            info.forwarded_for = forwarded
                .split(',')
                .filter_map(|part| part.trim().parse::<IpAddr>().ok())
                .collect();
            if let Some(client) = client_from_forwarded_for(forwarded, &self.trusted_proxies) {
                info.client_ip = Some(client);
            }
        }
        if info.client_ip == peer.map(|peer| peer.ip()) {
            // No usable forwarded address: fall back to `X-Real-IP` when the
            // direct peer is a trusted proxy.
            if let Some(real) = ctx.request().header("x-real-ip") {
                if let Ok(client) = real.trim().parse::<IpAddr>() {
                    info.client_ip = Some(client);
                }
            }
        }
        info
    }
}

#[async_trait]
impl IMiddleware for ClientInfoMiddleware {
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<ControlFlow<()>> {
        let info = self.resolve(ctx);
        ctx.set_client_info(info);
        Ok(ControlFlow::Continue(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;

    #[test]
    fn trusted_proxies_are_parsed_and_invalid_entries_skipped() {
        let config = ClientInfoSection {
            trust_forwarded_headers: true,
            trusted_proxies: vec!["10.0.0.0/8".into(), "not-a-cidr".into()],
        };
        let middleware = ClientInfoMiddleware::from_section(&config);
        assert_eq!(middleware.trusted_proxies.len(), 1);
        assert!(
            middleware.trusted_proxies[0].contains(SocketAddr::from(([10, 1, 2, 3], 1234)).ip())
        );
    }
}

//! Resolve trusted client information before the endpoint runs.
//!
//! Installed only when `ClientInfo.TrustForwardedHeaders` is enabled. It reads
//! the listener peer address and the `X-Forwarded-For` / `X-Real-IP` headers,
//! then delegates to the core resolver ([`webx_core::client_info`]) — the
//! framework's single forwarded-header implementation:
//!
//! * `client_ip` becomes the right-most `X-Forwarded-For` hop that is not a
//!   trusted proxy (a client cannot prepend a spoofed address and win);
//! * `forwarded_for` exposes the parsed chain (left-to-right);
//! * when forwarding is not trusted (the peer is not a configured proxy, or no
//!   proxy is configured) the forwarded chain stays empty, so business code can
//!   never mistake attacker-controlled headers for the client address.
//!
//! Middleware that needs the client address (rate limiting included) reads the
//! resolved value through [`IHttpContext::client_info`] instead of parsing the
//! headers again. Handlers read it through a `#[fromclient] client: ClientInfo`
//! request field; the binder calls [`IHttpContext::client_info`].

use std::ops::ControlFlow;

use async_trait::async_trait;

use webx_core::client_info::{resolve_client_info, ClientInfo, IpCidr};
use webx_core::config::ClientInfoSection;
use webx_core::error::Result;
use webx_core::http::IHttpContext;
use webx_core::middleware::IMiddleware;

/// Client-info resolution middleware.
pub struct ClientInfoMiddleware {
    trusted_proxies: Vec<IpCidr>,
}

impl ClientInfoMiddleware {
    /// Build from the `ClientInfo` configuration section.
    ///
    /// Invalid CIDRs are ignored with a warning: a malformed entry must not
    /// disable the rest of the trusted proxy list.
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
        resolve_client_info(
            ctx.request().remote_addr(),
            ctx.request().header("x-forwarded-for"),
            ctx.request().header("x-real-ip"),
            &self.trusted_proxies,
        )
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

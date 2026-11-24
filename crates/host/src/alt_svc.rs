//! `Alt-Svc` advertisement for HTTP/3 endpoints.
//!
//! Applications that run an HTTP/3 transport alongside the hyper HTTP/1.1+2
//! listener use [`HostBuilder::use_alt_svc`](crate::HostBuilder::use_alt_svc)
//! to advertise the UDP endpoint on every HTTP/1.1+2 response:
//!
//! ```rust,ignore
//! let h3 = H3Transport::new(H3Config::new("0.0.0.0:443", "cert.pem", "key.pem"))?;
//! let host = Host::builder()
//!     .use_http_transport(Arc::new(h3))
//!     .use_alt_svc("h3=\":443\"; ma=86400")
//!     .build();
//! ```
//!
//! The header is only sent when the operator explicitly configures it: an
//! application without an HTTP/3 listener must not advertise one. UDP ports,
//! firewalls and L4 load balancers still have to be provisioned for QUIC; a
//! client that cannot reach the UDP endpoint falls back to the TCP listener.
//!
//! The middleware runs on the response path (`after`) so the header is also
//! present on error responses and SSE headers.

use std::ops::ControlFlow;
use std::sync::Arc;

use webx_core::error::Result;
use webx_core::http::IHttpContext;
use webx_core::middleware::IMiddleware;

/// Response middleware that sets the `Alt-Svc` header.
///
/// Prefer the host-builder helper [`crate::HostBuilder::use_alt_svc`];
/// construct this type directly only when assembling the pipeline without a
/// [`crate::HostBuilder`].
pub struct AltSvcMiddleware {
    value: String,
}

impl AltSvcMiddleware {
    /// Advertise `value` verbatim as the `Alt-Svc` header value.
    ///
    /// The value is operator configuration (for example
    /// `h3=":443"; ma=86400`); the middleware does not parse or validate it.
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
        }
    }

    /// Build the conventional HTTP/3 advertisement for `port`.
    ///
    /// Produces `h3=":<port>"; ma=86400`. The port is the **UDP** port of the
    /// `rust-webx-h3` endpoint.
    pub fn h3(port: u16) -> Self {
        Self::new(format!("h3=\":{port}\"; ma=86400"))
    }

    /// The advertised header value.
    pub fn value(&self) -> &str {
        &self.value
    }
}

impl std::fmt::Debug for AltSvcMiddleware {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AltSvcMiddleware")
            .field("value", &self.value)
            .finish()
    }
}

#[async_trait::async_trait]
impl IMiddleware for AltSvcMiddleware {
    async fn invoke(&self, _ctx: &mut dyn IHttpContext) -> Result<ControlFlow<()>> {
        Ok(ControlFlow::Continue(()))
    }

    async fn after(&self, ctx: &mut dyn IHttpContext) -> Result<()> {
        ctx.response_mut().set_header("alt-svc", &self.value);
        Ok(())
    }
}

/// Factory form used by the host builder (`Fn() -> Arc<dyn IMiddleware>`).
pub(crate) fn factory(value: String) -> Arc<dyn IMiddleware> {
    Arc::new(AltSvcMiddleware::new(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn h3_builds_the_standard_advertisement() {
        let middleware = AltSvcMiddleware::h3(4433);
        assert_eq!(middleware.value(), "h3=\":4433\"; ma=86400");
    }
}

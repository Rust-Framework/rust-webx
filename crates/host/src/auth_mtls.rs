//! mTLS authentication scheme.
//!
//! The TLS terminator verifies the client certificate chain against
//! `Tls.ClientCaPath` and injects the leaf certificate's SHA-256 fingerprint
//! into [`CLIENT_CERT_FINGERPRINT_HEADER`]. This scheme maps that fingerprint to
//! application identity; an unknown certificate is a 401, never anonymous.

use std::sync::Arc;

use async_trait::async_trait;
use webx_core::auth::{IAuthenticationScheme, IClaims};
use webx_core::error::{Error, Result};
use webx_core::http::{IHttpContext, CLIENT_CERT_FINGERPRINT_HEADER};

/// Maps a verified client-certificate fingerprint to identity claims.
#[async_trait]
pub trait IMtlsIdentityResolver: Send + Sync {
    /// Resolve the fingerprint (lowercase hex SHA-256 of the DER leaf).
    async fn resolve(&self, fingerprint: &str) -> Result<Option<Box<dyn IClaims>>>;
}

/// `X-WebX-Client-Cert-SHA256` → identity.
pub struct MtlsScheme {
    resolver: Arc<dyn IMtlsIdentityResolver>,
}

impl MtlsScheme {
    /// Build with an application resolver.
    pub fn new(resolver: Arc<dyn IMtlsIdentityResolver>) -> Self {
        Self { resolver }
    }
}

#[async_trait]
impl IAuthenticationScheme for MtlsScheme {
    fn name(&self) -> &'static str {
        "mtls"
    }

    fn matches(&self, ctx: &dyn IHttpContext) -> bool {
        ctx.request()
            .header(CLIENT_CERT_FINGERPRINT_HEADER)
            .is_some_and(|fingerprint| !fingerprint.trim().is_empty())
    }

    async fn authenticate(&self, ctx: &mut dyn IHttpContext) -> Result<Option<Box<dyn IClaims>>> {
        let Some(fingerprint) = ctx.request().header(CLIENT_CERT_FINGERPRINT_HEADER) else {
            return Ok(None);
        };
        match self.resolver.resolve(fingerprint.trim()).await? {
            Some(claims) => Ok(Some(claims)),
            None => Err(Error::unauthorized(
                "client certificate is not registered".to_string(),
            )),
        }
    }
}

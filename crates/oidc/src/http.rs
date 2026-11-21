//! Minimal HTTP surface used by the OIDC client.
//!
//! [`IOidcHttp`] exists so tests (and custom transports: proxies, mTLS, a
//! service mesh) can substitute the network without constructing a streaming
//! response. [`HttpOidcHttp`] is the production adapter over
//! `rust-webx-client`, keeping its SSRF guard and timeouts.

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use url::form_urlencoded::Serializer;
use webx_client::{HttpClient, HttpClientOptions, IHttpClient, OutboundRequest};
use webx_core::error::{Error, Result};

/// Maximum accepted provider JSON body (discovery/token/JWKS).
pub const OIDC_MAX_BODY_BYTES: usize = 512 * 1024;

/// JSON-over-HTTP transport for the OIDC client.
#[async_trait]
pub trait IOidcHttp: Send + Sync {
    /// GET `url` and parse a JSON body.
    async fn get_json(&self, url: &str) -> Result<Value>;
    /// POST an `application/x-www-form-urlencoded` body with extra headers.
    async fn post_form(
        &self,
        url: &str,
        form: &[(&str, String)],
        headers: &[(&str, String)],
    ) -> Result<Value>;
}

/// Production [`IOidcHttp`] backed by `rust-webx-client`.
pub struct HttpOidcHttp {
    client: HttpClient,
}

impl HttpOidcHttp {
    /// Build for `issuer_base` (absolute `https://` URL) with `timeout`.
    pub fn new(issuer_base: &str, timeout: Duration) -> Result<Self> {
        Self::build(issuer_base, timeout, None)
    }

    /// Build with an explicit CA bundle (enterprise/private IdP).
    ///
    /// The PEM file **replaces** the public roots for these requests; a
    /// missing/invalid bundle fails construction.
    pub fn with_root_certificates(
        issuer_base: &str,
        timeout: Duration,
        root_certificates: impl Into<PathBuf>,
    ) -> Result<Self> {
        Self::build(issuer_base, timeout, Some(root_certificates.into()))
    }

    fn build(
        issuer_base: &str,
        timeout: Duration,
        root_certificates: Option<PathBuf>,
    ) -> Result<Self> {
        if !issuer_base.starts_with("https://") {
            return Err(Error::Di(format!(
                "OIDC issuer must be https:// (got '{issuer_base}')"
            )));
        }
        let mut options = HttpClientOptions::new(issuer_base);
        options.connect_timeout = timeout.min(Duration::from_secs(3));
        options.request_timeout = Some(timeout);
        options.read_timeout = Some(timeout);
        options.pool_max_idle_per_host = 2;
        // OIDC discovery/token endpoints are plain JSON over HTTP/1.1; avoiding
        // h2 keeps private IdP proxies and test servers simple.
        options.http2 = false;
        // `http2_only` must stay false so an HTTP/1.1-only provider still works.
        options.http2_only = false;
        options.root_certificates = root_certificates;
        let client = HttpClient::new("oidc", options)
            .map_err(|error| Error::Di(format!("invalid OIDC issuer URL: {error}")))?;
        Ok(Self { client })
    }
}

#[async_trait]
impl IOidcHttp for HttpOidcHttp {
    async fn get_json(&self, url: &str) -> Result<Value> {
        let response = self
            .client
            .execute(OutboundRequest::get(url))
            .await
            .map_err(unavailable)?;
        if !response.is_success() {
            return Err(Error::Status(
                503,
                format!("OIDC endpoint returned HTTP {}", response.status),
            ));
        }
        let body = response
            .body
            .collect(OIDC_MAX_BODY_BYTES)
            .await
            .map_err(unavailable)?;
        serde_json::from_slice(&body)
            .map_err(|error| Error::Status(503, format!("invalid OIDC JSON: {error}")))
    }

    async fn post_form(
        &self,
        url: &str,
        form: &[(&str, String)],
        headers: &[(&str, String)],
    ) -> Result<Value> {
        let body = Serializer::new(String::new())
            .extend_pairs(form.iter().map(|(key, value)| (*key, value.as_str())))
            .finish();
        let mut request = OutboundRequest::post(url)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(body);
        for (name, value) in headers {
            request = request.header(*name, value.clone());
        }
        let response = self.client.execute(request).await.map_err(unavailable)?;
        if !response.is_success() {
            // A 4xx from the token endpoint is a rejected code/client
            // (invalid_grant, invalid_client), not a bad gateway. Only an
            // upstream 5xx is a transient availability problem.
            if response.status >= 500 {
                return Err(Error::Status(
                    503,
                    format!("OIDC token endpoint returned HTTP {}", response.status),
                ));
            }
            return Err(Error::Unauthorized(
                "OIDC token endpoint rejected the request".to_string(),
            ));
        }
        let body = response
            .body
            .collect(OIDC_MAX_BODY_BYTES)
            .await
            .map_err(unavailable)?;
        serde_json::from_slice(&body)
            .map_err(|error| Error::Status(502, format!("invalid token JSON: {error}")))
    }
}

fn unavailable(error: webx_client::HttpClientError) -> Error {
    Error::Status(503, format!("OIDC transport failed: {error}"))
}

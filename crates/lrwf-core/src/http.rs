//! HTTP abstraction traits: IHttpContext, IHttpRequest, IHttpResponse.

use crate::auth::IClaims;
use crate::error::Result;
use std::collections::HashMap;

/// Common HTTP status codes.
pub struct HttpStatus;

impl HttpStatus {
    pub const OK: u16 = 200;
    pub const CREATED: u16 = 201;
    pub const NO_CONTENT: u16 = 204;
    pub const BAD_REQUEST: u16 = 400;
    pub const UNAUTHORIZED: u16 = 401;
    pub const FORBIDDEN: u16 = 403;
    pub const NOT_FOUND: u16 = 404;
    pub const INTERNAL_SERVER_ERROR: u16 = 500;
}

// ---------------------------------------------------------------------------
// Claims extension for IHttpContext — MUST be defined before IHttpContext
// ---------------------------------------------------------------------------

/// Extension trait that adds claims storage to an `IHttpContext`.
///
/// Implemented by `HttpContext` in `lrwf-http`. This trait is a supertrait
/// of `IHttpContext` so that authentication/authorization middleware can
/// store and retrieve claims through `&mut dyn IHttpContext` directly.
pub trait IClaimsExt {
    /// Store authentication claims in the context.
    fn set_claims(&mut self, claims: Box<dyn IClaims>);

    /// Retrieve authentication claims from the context, if present.
    fn claims(&self) -> Option<&dyn IClaims>;
}

/// HTTP context encapsulating the request, response, and service provider
/// for the duration of a single HTTP request.
///
/// Extends `IClaimsExt` so middleware can store/retrieve auth claims.
///
/// Analogous to ASP.NET Core's HttpContext.
pub trait IHttpContext: IClaimsExt + Send {
    fn request(&self) -> &dyn IHttpRequest;
    fn request_mut(&mut self) -> &mut dyn IHttpRequest;
    fn response(&self) -> &dyn IHttpResponse;
    fn response_mut(&mut self) -> &mut dyn IHttpResponse;
}

/// HTTP request abstraction.
///
/// Analogous to ASP.NET Core's HttpRequest.
///
/// Methods are non-generic to maintain dyn-compatibility.
/// Use `serde_json::from_slice` on the raw body bytes for JSON deserialization.
#[async_trait::async_trait]
pub trait IHttpRequest: Send {
    fn method(&self) -> &str;
    fn path(&self) -> &str;
    fn header(&self, name: &str) -> Option<&str>;
    fn query(&self) -> &HashMap<String, String>;
    fn route_params(&self) -> &HashMap<String, String>;
    fn route_params_mut(&mut self) -> &mut HashMap<String, String>;

    /// The original route pattern that was matched (e.g., `"/api/users/{id}"`).
    /// Set by the router after successful route matching.
    fn route_pattern(&self) -> Option<&str>;
    fn route_pattern_mut(&mut self) -> &mut Option<String>;

    /// Return the raw request body bytes.
    async fn body_bytes(&self) -> Result<Vec<u8>>;

    /// Return the request body as a UTF-8 string.
    async fn body_text(&self) -> Result<String> {
        let bytes = self.body_bytes().await?;
        String::from_utf8(bytes).map_err(|e| crate::error::Error::Http(e.to_string()))
    }
}

/// HTTP response abstraction.
///
/// Analogous to ASP.NET Core's HttpResponse.
///
/// Methods are non-generic to maintain dyn-compatibility.
#[async_trait::async_trait]
pub trait IHttpResponse: Send {
    fn set_status(&mut self, code: u16);
    fn set_header(&mut self, key: &str, value: &str);

    /// Write raw bytes as the response body.
    async fn write_bytes(&mut self, data: Vec<u8>) -> Result<()>;

    /// Write a UTF-8 string as the response body.
    async fn write_text(&mut self, text: &str) -> Result<()> {
        self.write_bytes(text.as_bytes().to_vec()).await
    }
}

/// Helper: build a JSON response by serializing a value and writing it.
pub async fn write_json_response<T: serde::Serialize + Send>(
    resp: &mut dyn IHttpResponse,
    value: &T,
) -> Result<()> {
    let json = serde_json::to_vec(value)?;
    resp.set_header("content-type", "application/json");
    resp.write_bytes(json).await
}

/// Helper: read JSON from the request body.
pub async fn read_json_body<T: serde::de::DeserializeOwned>(
    req: &dyn IHttpRequest,
) -> Result<T> {
    let bytes = req.body_bytes().await?;
    serde_json::from_slice(&bytes).map_err(|e| crate::error::Error::Serialization(e))
}

/// Type alias for JSON responses in controller methods.
pub type Json<T> = T;

//! HTTP abstraction traits: IHttpContext, IHttpRequest, IHttpResponse.
//!
//! This module defines the core interfaces for HTTP request/response handling
//! in the rust-webx framework. All types are defined as traits to enable testability
//! and middleware composition.
//!
//! # Traits
//!
//! - [`IHttpContext`] — The central context for a single HTTP request, providing
//!   access to request, response, and authentication claims.
//! - [`IHttpRequest`] — Read-only access to the incoming HTTP request.
//! - [`IHttpResponse`] — Mutable access to the outgoing HTTP response.
//! - [`IClaimsExt`] — Extension for storing/retrieving authentication claims.
//! - [`FromHttpContext`] — Build a request struct from HTTP request data.
//!
//! # Helpers
//!
//! - [`read_json_body`] — Deserialize JSON from the request body.
//! - [`write_json_response`] — Serialize a value as JSON into the response.

use crate::auth::IClaims;
use crate::error::Result;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::io::{AsyncRead, AsyncSeek};

/// Common HTTP status codes.
pub struct HttpStatus;

impl HttpStatus {
    pub const OK: u16 = 200;
    pub const CREATED: u16 = 201;
    pub const NO_CONTENT: u16 = 204;
    pub const PARTIAL_CONTENT: u16 = 206;
    pub const NOT_MODIFIED: u16 = 304;
    pub const BAD_REQUEST: u16 = 400;
    pub const UNAUTHORIZED: u16 = 401;
    pub const FORBIDDEN: u16 = 403;
    pub const NOT_FOUND: u16 = 404;
    pub const RANGE_NOT_SATISFIABLE: u16 = 416;
    pub const PAYLOAD_TOO_LARGE: u16 = 413;
    pub const UNSUPPORTED_MEDIA_TYPE: u16 = 415;
    pub const INTERNAL_SERVER_ERROR: u16 = 500;
}

/// An inclusive byte range, as requested by a `Range` header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    /// First byte to send, inclusive.
    pub start: u64,
    /// Last byte to send, inclusive.
    pub end: u64,
}

impl ByteRange {
    /// Number of bytes covered by the range.
    pub fn len(&self) -> u64 {
        self.end.saturating_sub(self.start) + 1
    }

    /// A range is never empty: `start > end` is normalised away when parsing.
    pub fn is_empty(&self) -> bool {
        false
    }
}

/// How a file response should be presented.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Disposition {
    /// Render in place — the default, used for images, PDFs and media.
    #[default]
    Inline,
    /// Prompt the browser to save the file under [`FileBody::file_name`].
    Attachment,
}

/// Read/write buffer size used for streamed response bodies and spooled uploads.
///
/// A 4 KiB buffer turns a 1 GiB transfer into ~262k reads *and* ~262k socket
/// writes; 64 KiB cuts both by 16x while staying well inside cache. This is the
/// size nginx, hyper and tower-http settle on for bulk transfer.
pub const STREAM_BUF_SIZE: usize = 64 * 1024;

/// An async reader that can also seek.
///
/// Seeking is what lets a stream answer `Range` requests, so a
/// [`FileSource::SeekableStream`] needs both capabilities. Implemented
/// automatically for anything that is `AsyncRead + AsyncSeek + Send + Unpin`,
/// so you never implement it yourself.
pub trait SeekableReader: AsyncRead + AsyncSeek + Send + Unpin {}

impl<T: AsyncRead + AsyncSeek + Send + Unpin> SeekableReader for T {}

/// Where a file response's bytes come from.
///
/// This mirrors the sources ASP.NET Core accepts for `File(...)`:
/// a physical path, a stream, or an in-memory buffer. Pick the one you already
/// have — none of them require materialising the content first.
pub enum FileSource {
    /// A file on disk.
    ///
    /// Streamed in 64 KiB chunks; the host opens it once and reads length and
    /// validators from the same handle.
    Path(PathBuf),
    /// An async reader whose length is known and which can seek, so `Range`
    /// requests can be served.
    ///
    /// Use this for object-storage handles, database LOB readers, archive
    /// entries — anything that can jump to a byte offset.
    SeekableStream {
        /// The reader. Consumed when the response is sent.
        reader: Box<dyn SeekableReader>,
        /// Total length in bytes, so `Content-Length` and `Content-Range` are exact.
        len: u64,
    },
    /// An async reader of unknown length.
    ///
    /// Sent with chunked transfer encoding and no `Content-Length`; `Range`
    /// requests receive the full body. Use this for pipes, live producers and
    /// compressed/encrypted streams where the final size is not known up front.
    Stream(Box<dyn AsyncRead + Send + Unpin>),
}

impl std::fmt::Debug for FileSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FileSource::Path(path) => f.debug_tuple("Path").field(path).finish(),
            FileSource::SeekableStream { len, .. } => f
                .debug_struct("SeekableStream")
                .field("len", len)
                .finish_non_exhaustive(),
            FileSource::Stream(_) => f.debug_tuple("Stream").finish(),
        }
    }
}

impl std::fmt::Debug for FileBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileBody")
            .field("source", &self.source)
            .field("content_type", &self.content_type)
            .field("file_name", &self.file_name)
            .field("disposition", &self.disposition)
            .field("enable_range_processing", &self.enable_range_processing)
            .field("last_modified", &self.last_modified)
            .field("entity_tag", &self.entity_tag)
            .finish()
    }
}

/// A file response — the rust-webx counterpart of ASP.NET Core's
/// `File(...)` / `PhysicalFile(...)` / `Results.File(...)`.
///
/// The host streams the content and fills in the HTTP metadata:
///
/// * `Content-Length` — exact when it is known (path, seekable stream)
/// * `ETag` / `Last-Modified` — derived from the file, or supplied via
///   [`FileBody::entity_tag`] / [`FileBody::last_modified`] for streams
/// * `Accept-Ranges` and `Range` handling (`206` / `416`) — when the source can
///   seek and [`FileBody::enable_range_processing`] is left on
/// * `If-Range`, `If-None-Match`, `If-Modified-Since` (`304`)
///
/// Build one with [`FileBody::path`], [`FileBody::stream`] or
/// [`FileBody::seekable_stream`], then hand it to
/// [`crate::route::scan::ResponseData::with_file`].
pub struct FileBody {
    /// Where the bytes come from.
    pub source: FileSource,
    /// Media type. When `None` the host infers it from the path extension, or
    /// falls back to `application/octet-stream` for streams.
    pub content_type: Option<String>,
    /// Name used in `Content-Disposition`.
    pub file_name: Option<String>,
    /// Whether the content should be shown inline or downloaded.
    pub disposition: Disposition,
    /// Whether `Range` requests should be served (default `true`).
    ///
    /// Turn it off to force a single full response — useful when range
    /// requests would multiply backend reads.
    pub enable_range_processing: bool,
    /// `Last-Modified` to advertise.
    ///
    /// Derived from the file for [`FileSource::Path`]; required for a stream if
    /// you want `If-Modified-Since` to work.
    pub last_modified: Option<SystemTime>,
    /// `ETag` to advertise.
    ///
    /// Derived from size and mtime for [`FileSource::Path`]; required for a
    /// stream if you want `If-None-Match` to work.
    pub entity_tag: Option<String>,
}

impl FileBody {
    /// Send the file at `path` — ASP.NET Core's `PhysicalFile(...)`.
    pub fn path(path: impl Into<PathBuf>) -> Self {
        Self {
            source: FileSource::Path(path.into()),
            content_type: None,
            file_name: None,
            disposition: Disposition::Inline,
            enable_range_processing: true,
            last_modified: None,
            entity_tag: None,
        }
    }

    /// Send an async reader of unknown length — ASP.NET Core's `File(Stream, ...)`.
    ///
    /// The response uses chunked transfer encoding and does not serve `Range`.
    /// If you know the length and can seek, use [`FileBody::seekable_stream`] so
    /// clients can resume and cache.
    pub fn stream(reader: impl AsyncRead + Send + Unpin + 'static) -> Self {
        Self {
            source: FileSource::Stream(Box::new(reader)),
            content_type: None,
            file_name: None,
            disposition: Disposition::Inline,
            enable_range_processing: false,
            last_modified: None,
            entity_tag: None,
        }
    }

    /// Send an async reader of known length that supports seeking.
    ///
    /// This is the stream form that keeps every HTTP nicety: exact
    /// `Content-Length`, `Range`/`206`, and — with [`FileBody::entity_tag`] —
    /// conditional requests.
    pub fn seekable_stream(
        reader: impl AsyncRead + AsyncSeek + Send + Unpin + 'static,
        len: u64,
    ) -> Self {
        Self {
            source: FileSource::SeekableStream {
                reader: Box::new(reader),
                len,
            },
            content_type: None,
            file_name: None,
            disposition: Disposition::Inline,
            enable_range_processing: true,
            last_modified: None,
            entity_tag: None,
        }
    }

    /// Send bytes compiled into the executable — ASP.NET Core's embedded
    /// `ManifestEmbeddedFileProvider`.
    ///
    /// The slice is borrowed, never copied, and is served as a seekable source,
    /// so `Content-Length`, `Range`/`206` and conditional requests all work.
    /// Build one with `include_bytes!`-backed tables, such as the ones produced
    /// by `webx::builder().web_root(...).build()` and included with
    /// `webx::spa::embed_assets!()`.
    ///
    /// The extension-derived media type is not available for in-memory bytes,
    /// so set [`FileBody::content_type`] explicitly. There is no mtime either,
    /// so supply [`FileBody::entity_tag`] (a content hash is the usual choice)
    /// if clients should be able to revalidate.
    pub fn static_bytes(bytes: &'static [u8]) -> Self {
        Self::seekable_stream(std::io::Cursor::new(bytes), bytes.len() as u64)
    }

    /// Override the media type instead of deriving it from the extension.
    pub fn content_type(mut self, content_type: impl Into<String>) -> Self {
        self.content_type = Some(content_type.into());
        self
    }

    /// Send the file as a download named `file_name`.
    pub fn download_name(mut self, file_name: impl Into<String>) -> Self {
        self.file_name = Some(file_name.into());
        self.disposition = Disposition::Attachment;
        self
    }

    /// Send the file inline under `file_name`.
    pub fn inline_name(mut self, file_name: impl Into<String>) -> Self {
        self.file_name = Some(file_name.into());
        self.disposition = Disposition::Inline;
        self
    }

    /// Enable or disable `Range` processing.
    pub fn enable_range_processing(mut self, enabled: bool) -> Self {
        self.enable_range_processing = enabled;
        self
    }

    /// Advertise an explicit `Last-Modified`.
    pub fn last_modified(mut self, last_modified: SystemTime) -> Self {
        self.last_modified = Some(last_modified);
        self
    }

    /// Advertise an explicit `ETag`.
    ///
    /// Include the quotes: `"abc123"`. A strong tag lets clients resume with
    /// `If-Range`.
    pub fn entity_tag(mut self, entity_tag: impl Into<String>) -> Self {
        self.entity_tag = Some(entity_tag.into());
        self
    }

    /// Whether this source can seek, and therefore serve byte ranges.
    pub fn is_range_capable(&self) -> bool {
        self.enable_range_processing
            && matches!(
                self.source,
                FileSource::Path(_) | FileSource::SeekableStream { .. }
            )
    }

    /// The length in bytes, when it is known without touching the filesystem.
    pub fn known_len(&self) -> Option<u64> {
        match &self.source {
            FileSource::Path(_) | FileSource::Stream(_) => None,
            FileSource::SeekableStream { len, .. } => Some(*len),
        }
    }
}

/// The body of an HTTP response.
pub enum ResponseBody {
    /// Fully materialised bytes.
    Bytes(Vec<u8>),
    /// A file streamed by the host.
    File(FileBody),
}

impl std::fmt::Debug for ResponseBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResponseBody::Bytes(bytes) => f.debug_tuple("Bytes").field(&bytes.len()).finish(),
            ResponseBody::File(file) => f.debug_tuple("File").field(file).finish(),
        }
    }
}

impl ResponseBody {
    /// Length in bytes, when it is known without touching the filesystem.
    pub fn len(&self) -> Option<u64> {
        match self {
            ResponseBody::Bytes(bytes) => Some(bytes.len() as u64),
            ResponseBody::File(file) => file.known_len(),
        }
    }

    /// Whether this body is a known-empty byte buffer.
    pub fn is_empty(&self) -> bool {
        matches!(self, ResponseBody::Bytes(bytes) if bytes.is_empty())
    }

    /// The in-memory bytes, when this is a byte body.
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            ResponseBody::Bytes(bytes) => Some(bytes),
            ResponseBody::File(_) => None,
        }
    }

    /// The file descriptor, when this is a file body.
    pub fn as_file(&self) -> Option<&FileBody> {
        match self {
            ResponseBody::File(file) => Some(file),
            ResponseBody::Bytes(_) => None,
        }
    }
}

impl From<Vec<u8>> for ResponseBody {
    fn from(bytes: Vec<u8>) -> Self {
        ResponseBody::Bytes(bytes)
    }
}

impl From<String> for ResponseBody {
    fn from(text: String) -> Self {
        ResponseBody::Bytes(text.into_bytes())
    }
}

impl From<&str> for ResponseBody {
    fn from(text: &str) -> Self {
        ResponseBody::Bytes(text.as_bytes().to_vec())
    }
}

impl From<FileBody> for ResponseBody {
    fn from(file: FileBody) -> Self {
        ResponseBody::File(file)
    }
}

// ---------------------------------------------------------------------------
// Claims extension for IHttpContext — MUST be defined before IHttpContext
// ---------------------------------------------------------------------------

/// Extension trait that adds claims storage to an `IHttpContext`.
///
/// Implemented by `HttpContext` in `rust-webx-host`. This trait is a supertrait
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
    ///
    /// The body can only be read once; a second call reports an error rather
    /// than silently returning an empty body.
    async fn body_bytes(&mut self) -> Result<Vec<u8>>;

    /// Return the request body as a UTF-8 string.
    async fn body_text(&mut self) -> Result<String> {
        let bytes = self.body_bytes().await?;
        String::from_utf8(bytes).map_err(|e| crate::error::Error::Http(e.to_string()))
    }

    /// The request's media type, without parameters (e.g. `"multipart/form-data"`).
    ///
    /// Comparison is case-insensitive; see [`IHttpRequest::is_multipart`].
    fn content_type(&self) -> Option<&str> {
        self.header("content-type")
            .map(|value| value.split(';').next().unwrap_or("").trim())
            .filter(|value| !value.is_empty())
    }

    /// Whether the request declares `Content-Type: multipart/form-data`.
    fn is_multipart(&self) -> bool {
        self.content_type()
            .is_some_and(|ct| ct.eq_ignore_ascii_case("multipart/form-data"))
    }

    /// Parse the request body as `multipart/form-data`.
    ///
    /// The parsed form is cached on the request, so repeated calls are cheap and
    /// spooled uploads stay alive until the request is finished. Call
    /// [`crate::form::FormFile::save_as`] from your handler to keep a file.
    ///
    /// The default implementation reports the request as unsupported; the
    /// rust-webx HTTP host provides the real parser.
    async fn multipart(&mut self) -> Result<Arc<crate::form::MultipartForm>> {
        Err(crate::error::Error::UnsupportedMediaType(
            "multipart/form-data is not supported by this request implementation".to_string(),
        ))
    }
}

/// HTTP response abstraction.
///
/// Analogous to ASP.NET Core's HttpResponse.
///
/// Methods are non-generic to maintain dyn-compatibility.
#[async_trait::async_trait]
pub trait IHttpResponse: Send {
    /// Get the current HTTP status code.
    fn status(&self) -> u16;
    fn set_status(&mut self, code: u16);
    fn set_header(&mut self, key: &str, value: &str);

    /// Remove a response header by name. Default is a no-op.
    fn remove_header(&mut self, _key: &str) {}

    /// Whether a response body has been written.
    fn has_body(&self) -> bool {
        false
    }

    /// Write raw bytes as the response body.
    async fn write_bytes(&mut self, data: Vec<u8>) -> Result<()>;

    /// Replace the response body with a structured [`ResponseBody`].
    ///
    /// The default implementation handles in-memory bodies and rejects file
    /// bodies; the rust-webx HTTP host overrides it to stream files with
    /// `Range`, `ETag` and `Content-Disposition` support.
    async fn write_body(&mut self, body: ResponseBody) -> Result<()> {
        match body {
            ResponseBody::Bytes(bytes) => self.write_bytes(bytes).await,
            ResponseBody::File(_) => Err(crate::error::Error::Internal(
                "file responses require the rust-webx HTTP host".to_string(),
            )),
        }
    }

    /// Write a UTF-8 string as the response body.
    async fn write_text(&mut self, text: &str) -> Result<()> {
        self.write_bytes(text.as_bytes().to_vec()).await
    }

    /// Read the current response body bytes.
    ///
    /// Returns an empty Vec if no body has been written.
    /// Used by post-processing middleware (e.g. compression) in `after` hooks.
    fn body_bytes(&self) -> Vec<u8> {
        Vec::new()
    }

    /// Read a response header value by name.
    ///
    /// Returns None if the header is not set.
    /// Used by post-processing middleware (e.g. compression) to inspect content-type.
    fn header(&self, _key: &str) -> Option<&str> {
        None
    }
}

/// Build a `Content-Disposition` header value (RFC 6266).
///
/// A plain ASCII `filename` is always included for legacy clients, and a
/// `filename*` (RFC 5987) parameter carries the real name so non-ASCII
/// filenames survive the round trip.
pub fn content_disposition_value(disposition: Disposition, file_name: &str) -> String {
    let kind = match disposition {
        Disposition::Inline => "inline",
        Disposition::Attachment => "attachment",
    };

    let ascii: String = file_name
        .chars()
        .map(|ch| {
            if ch.is_ascii() && ch != '"' && ch != '\\' && !ch.is_control() {
                ch
            } else {
                '_'
            }
        })
        .collect();

    format!(
        "{kind}; filename=\"{ascii}\"; filename*=UTF-8''{}",
        percent_encode_ext_value(file_name)
    )
}

/// Percent-encode using the `attr-char` set from RFC 5987.
fn percent_encode_ext_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        let ch = *byte as char;
        let is_attr_char = ch.is_ascii_alphanumeric()
            || matches!(
                ch,
                '!' | '#' | '$' | '&' | '+' | '-' | '.' | '^' | '_' | '`' | '|' | '~'
            );
        if is_attr_char {
            out.push(ch);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
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
    req: &mut dyn IHttpRequest,
) -> Result<T> {
    let bytes = req.body_bytes().await?;
    serde_json::from_slice(&bytes).map_err(crate::error::Error::Serialization)
}

/// Type alias for JSON responses in controller methods.
pub type Json<T> = T;

/// Trait for constructing a request struct from the HTTP context.
///
/// Implemented automatically by the `#[get]`, `#[post]`, `#[put]`, `#[delete]`
/// proc macros for request types. Custom implementations are also supported.
///
/// # Automatic implementation
///
/// For POST/PUT/PATCH routes, the macro will attempt `serde_json::from_slice`
/// on the request body. For GET/DELETE routes with path parameters, the macro
/// will extract parameters from `ctx.request().route_params()` by field name.
///
/// # Manual implementation
///
/// ```ignore
/// #[async_trait::async_trait]
/// impl FromHttpContext for MyRequest {
///     async fn from_http_context(ctx: &dyn IHttpContext) -> Result<Self> {
///         // custom construction logic
///     }
/// }
/// ```
#[async_trait::async_trait]
pub trait FromHttpContext: Sized + Send + 'static {
    async fn from_http_context(ctx: &dyn IHttpContext) -> Result<Self>;
}

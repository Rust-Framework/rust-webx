//! HttpContext — runtime implementation of IHttpContext, IHttpRequest, IHttpResponse.
//!
//! # Request body
//!
//! The body is *not* read eagerly. It stays as the live hyper stream until
//! something asks for it:
//!
//! * [`IHttpRequest::body_bytes`] buffers the whole body, enforcing
//!   `App.MaxBodySize`.
//! * [`IHttpRequest::multipart`] streams the body through a multipart parser,
//!   enforcing `Form.MaxRequestSize`, `Form.MaxFileSize` and
//!   `Form.MaxFieldSize`, and spooling large file parts to disk.
//!
//! Either way the body can only be consumed once; asking for it twice reports a
//! clear error rather than silently returning an empty body.
//!
//! # Response body
//!
//! Responses carry either in-memory bytes or a [`FileBody`]. File bodies are
//! streamed by the host with `Content-Length`, `ETag`, `Last-Modified`,
//! `Accept-Ranges` and `Content-Disposition`, and satisfy `Range`,
//! `If-Range`, `If-None-Match` and `If-Modified-Since` requests, so downloads
//! can be cached and resumed.

use http_body_util::{combinators::UnsyncBoxBody, BodyExt, Full};
use hyper::body::{Bytes, Frame, Incoming};
use hyper::Request;
use rust_webx_core::auth::IClaims;
use rust_webx_core::config::AppOptions;
use rust_webx_core::error::{Error, Result};
use rust_webx_core::form::{FormFileBuilder, MultipartForm};
use rust_webx_core::http::{
    content_disposition_value, ByteRange, FileBody, FileSource, IClaimsExt, IHttpContext,
    IHttpRequest, IHttpResponse, ResponseBody, SeekableReader,
};
use futures_util::{StreamExt, TryStreamExt};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, SeekFrom};
use tokio::sync::Mutex;

use crate::problem_response::{build_problem, problem_to_bytes};

/// Response body produced by the HTTP host.
///
/// Deliberately the *unsync* box: a streamed body only ever needs to be `Send`,
/// and requiring `Sync` would rule out readers that are not shared between
/// threads (most streaming sources).
pub type RespBody = UnsyncBoxBody<Bytes, std::io::Error>;

/// Limits applied while reading a request body.
///
/// Built from `App` and `Form` configuration; see [`BodyLimits::from_options`].
#[derive(Debug, Clone)]
pub struct BodyLimits {
    /// Limit for non-multipart bodies (`App.MaxBodySize`).
    pub body: usize,
    /// Limit for the whole multipart body (`Form.MaxRequestSize`).
    pub multipart_body: u64,
    /// Limit for one uploaded file (`Form.MaxFileSize`).
    pub file: u64,
    /// Limit for one text field (`Form.MaxFieldSize`).
    pub field: usize,
    /// Bytes buffered in memory per file before spooling (`Form.MemoryThreshold`).
    pub memory_threshold: usize,
    /// Directory for spooled uploads (`Form.TempDir`).
    pub spool_dir: PathBuf,
}

impl Default for BodyLimits {
    fn default() -> Self {
        Self {
            body: 10 * 1024 * 1024,
            multipart_body: 256 * 1024 * 1024,
            file: 128 * 1024 * 1024,
            field: 1024 * 1024,
            memory_threshold: 1024 * 1024,
            spool_dir: rust_webx_core::form::spool_root(),
        }
    }
}

impl BodyLimits {
    /// Derive limits from the merged application configuration.
    ///
    /// A relative `Form.TempDir` is resolved against the application base
    /// directory, matching how `wwwroot` and `appsettings.json` are located.
    pub fn from_options(options: &AppOptions) -> Self {
        let spool_dir = match options.form.temp_dir.as_deref() {
            Some(dir) if !dir.trim().is_empty() => {
                let path = PathBuf::from(dir);
                if path.is_absolute() {
                    path
                } else {
                    rust_webx_core::paths::app_base().join(path)
                }
            }
            _ => rust_webx_core::form::spool_root(),
        };

        Self {
            body: options.app.max_body_size,
            multipart_body: options.form.max_request_size as u64,
            file: options.form.max_file_size,
            field: options.form.max_field_size,
            memory_threshold: options.form.memory_threshold,
            spool_dir,
        }
    }

    /// Publish the spool directory so `FormFile` values can be validated
    /// wherever they are bound.
    pub fn apply_spool_root(&self) {
        rust_webx_core::form::set_spool_root(self.spool_dir.clone());
    }
}

/// Concrete implementation of IHttpContext wrapping a hyper request and response.
pub struct HttpContext {
    req: HttpRequest,
    resp: HttpResponse,
    /// Authentication claims set by authentication middleware.
    claims: Option<Box<dyn IClaims>>,
}

impl HttpContext {
    /// Create a context around `req`.
    ///
    /// No I/O happens here: the body stays a live stream until the endpoint
    /// asks for it.
    pub fn new(req: Request<Incoming>, limits: BodyLimits) -> Self {
        let (parts, body) = req.into_parts();

        let mut headers = HashMap::new();
        for (name, value) in parts.headers.iter() {
            if let Ok(value) = value.to_str() {
                headers.insert(name.to_string(), value.to_string());
            }
        }

        let query_params = parts
            .uri
            .query()
            .map(parse_query_string)
            .unwrap_or_default();

        let mut resp = HttpResponse::new(200);

        // Refuse an oversized upload from its declared length, without reading
        // it. Chunked bodies have no `Content-Length` and are bounded while
        // streaming instead.
        let declared_limit = if is_multipart_content_type(headers.get("content-type")) {
            limits.multipart_body
        } else {
            limits.body as u64
        };
        if let Some(declared) = headers
            .get("content-length")
            .and_then(|value| value.parse::<u64>().ok())
        {
            if declared > declared_limit {
                let problem = build_problem(
                    413,
                    format!("request body exceeds the {declared_limit}-byte limit"),
                );
                resp.set_status(413);
                resp.set_header("content-type", "application/problem+json");
                resp.body = Some(ResponseBody::Bytes(problem_to_bytes(&problem)));
            }
        }

        let req = HttpRequest {
            method: parts.method.to_string(),
            path: parts.uri.path().to_string(),
            headers,
            query_params,
            route_params: HashMap::new(),
            route_pattern: None,
            body: Mutex::new(RequestBody::Pending(Some(body))),
            limits,
        };

        Self { req, resp, claims: None }
    }

    /// Finish the request and produce the hyper response.
    pub async fn into_response(self) -> hyper::Response<RespBody> {
        let HttpContext { req, resp, .. } = self;
        finalize_response(&req, resp).await
    }
}

impl IClaimsExt for HttpContext {
    fn set_claims(&mut self, claims: Box<dyn IClaims>) {
        self.claims = Some(claims);
    }

    fn claims(&self) -> Option<&dyn IClaims> {
        self.claims.as_deref()
    }
}

impl IHttpContext for HttpContext {
    fn request(&self) -> &dyn IHttpRequest {
        &self.req
    }

    fn request_mut(&mut self) -> &mut dyn IHttpRequest {
        &mut self.req
    }

    fn response(&self) -> &dyn IHttpResponse {
        &self.resp
    }

    fn response_mut(&mut self) -> &mut dyn IHttpResponse {
        &mut self.resp
    }
}

// ---------------------------------------------------------------------------
// Request
// ---------------------------------------------------------------------------

/// State of the request body.
enum RequestBody {
    /// Still streaming from the connection.
    Pending(Option<Incoming>),
    /// Fully buffered, replayable.
    Buffered(Vec<u8>),
    /// Parsed as a multipart form.
    Form(Arc<MultipartForm>),
}

/// Concrete implementation of IHttpRequest.
pub struct HttpRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    query_params: HashMap<String, String>,
    route_params: HashMap<String, String>,
    route_pattern: Option<String>,
    body: Mutex<RequestBody>,
    limits: BodyLimits,
}

#[async_trait::async_trait]
impl IHttpRequest for HttpRequest {
    fn method(&self) -> &str {
        &self.method
    }

    fn path(&self) -> &str {
        &self.path
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(|s| s.as_str())
    }

    fn query(&self) -> &HashMap<String, String> {
        &self.query_params
    }

    fn route_params(&self) -> &HashMap<String, String> {
        &self.route_params
    }

    fn route_params_mut(&mut self) -> &mut HashMap<String, String> {
        &mut self.route_params
    }

    fn route_pattern(&self) -> Option<&str> {
        self.route_pattern.as_deref()
    }

    fn route_pattern_mut(&mut self) -> &mut Option<String> {
        &mut self.route_pattern
    }

    async fn body_bytes(&mut self) -> Result<Vec<u8>> {
        let mut guard = self.body.lock().await;
        let incoming = match &mut *guard {
            RequestBody::Buffered(bytes) => return Ok(bytes.clone()),
            RequestBody::Form(_) => {
                return Err(Error::Http(
                    "request body was already read as multipart/form-data".to_string(),
                ))
            }
            RequestBody::Pending(slot) => slot.take(),
        };

        let Some(incoming) = incoming else {
            return Err(Error::Http(
                "request body has already been consumed".to_string(),
            ));
        };

        let bytes = read_body(incoming, self.limits.body).await?;
        *guard = RequestBody::Buffered(bytes.clone());
        Ok(bytes)
    }

    async fn body_text(&mut self) -> Result<String> {
        let bytes = self.body_bytes().await?;
        String::from_utf8(bytes).map_err(|e| Error::Http(e.to_string()))
    }

    async fn multipart(&mut self) -> Result<Arc<MultipartForm>> {
        let mut guard = self.body.lock().await;

        match &*guard {
            RequestBody::Form(form) => return Ok(Arc::clone(form)),
            RequestBody::Buffered(_) => {
                return Err(Error::UnsupportedMediaType(
                    "request body was already read as a non-multipart body".to_string(),
                ))
            }
            RequestBody::Pending(_) => {}
        }

        if !self.is_multipart() {
            return Err(Error::UnsupportedMediaType(
                "expected Content-Type: multipart/form-data".to_string(),
            ));
        }

        let raw_content_type = self.header("content-type").unwrap_or("");
        let boundary = multipart_boundary(raw_content_type).ok_or_else(|| {
            Error::Validation(
                "multipart/form-data request is missing its boundary parameter".to_string(),
            )
        })?;

        let incoming = match &mut *guard {
            RequestBody::Pending(slot) => slot.take(),
            _ => None,
        };

        let Some(body) = incoming else {
            return Err(Error::Http(
                "request body has already been consumed".to_string(),
            ));
        };

        let form = Arc::new(parse_multipart(body, &boundary, &self.limits).await?);
        *guard = RequestBody::Form(Arc::clone(&form));
        Ok(form)
    }
}

/// Drain a body stream into memory, enforcing `limit`.
async fn read_body(incoming: Incoming, limit: usize) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut total = 0usize;
    let mut stream = incoming.into_data_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|err| Error::Http(format!("failed to read request body: {err}")))?;
        total = total.saturating_add(chunk.len());
        if total > limit {
            return Err(Error::PayloadTooLarge(format!(
                "request body exceeds the {limit}-byte limit"
            )));
        }
        out.extend_from_slice(&chunk);
    }

    Ok(out)
}

/// Whether a raw `Content-Type` header value declares a multipart form.
fn is_multipart_content_type(content_type: Option<&String>) -> bool {
    content_type
        .map(|value| {
            value
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .eq_ignore_ascii_case("multipart/form-data")
        })
        .unwrap_or(false)
}

/// Extract the `boundary` parameter of a `multipart/form-data` content type.
fn multipart_boundary(content_type: &str) -> Option<String> {
    for part in content_type.split(';').skip(1) {
        let part = part.trim();
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        if key.trim().eq_ignore_ascii_case("boundary") {
            let value = value.trim().trim_matches('"');
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// Stream a multipart body into a [`MultipartForm`].
///
/// Text fields are buffered (bounded by `limits.field`); file parts stream into
/// a [`FormFileBuilder`], which keeps small files in memory and spools large
/// ones to disk. Nothing is copied twice, so peak memory stays bounded no
/// matter how large the upload is.
async fn parse_multipart(
    incoming: Incoming,
    boundary: &str,
    limits: &BodyLimits,
) -> Result<MultipartForm> {
    let stream = incoming.into_data_stream();
    let mut multipart = multer::Multipart::new(stream, boundary);
    let mut form = MultipartForm::new();
    let mut total: u64 = 0;

    while let Some(field) = multipart.next_field().await.map_err(multer_error)? {
        let field_name = field.name().unwrap_or_default().to_string();
        if field_name.is_empty() {
            continue;
        }
        let file_name = field.file_name().map(str::to_string);
        let content_type = field.content_type().map(|m| m.to_string());

        match file_name {
            Some(file_name) => {
                let mut builder = FormFileBuilder::new(field_name.clone(), file_name, content_type)
                    .memory_threshold(limits.memory_threshold)
                    .max_size(limits.file)
                    .spool_dir(limits.spool_dir.clone());

                let mut field = field;
                while let Some(chunk) = field.chunk().await.map_err(multer_error)? {
                    total = charge(&mut total, chunk.len(), limits.multipart_body)?;
                    builder.write(&chunk).await?;
                }
                form.push_file(builder.finish().await?);
            }
            None => {
                let mut value: Vec<u8> = Vec::new();
                let mut field = field;
                while let Some(chunk) = field.chunk().await.map_err(multer_error)? {
                    total = charge(&mut total, chunk.len(), limits.multipart_body)?;
                    if value.len() + chunk.len() > limits.field {
                        return Err(Error::PayloadTooLarge(format!(
                            "form field `{field_name}` exceeds the {}-byte limit",
                            limits.field
                        )));
                    }
                    value.extend_from_slice(&chunk);
                }
                let text = String::from_utf8(value).map_err(|_| {
                    Error::Validation(format!("form field `{field_name}` is not valid UTF-8"))
                })?;
                form.push_field(field_name, text);
            }
        }
    }

    Ok(form)
}

/// Add `len` to the running total, failing when the request limit is exceeded.
fn charge(total: &mut u64, len: usize, limit: u64) -> Result<u64> {
    *total = total.saturating_add(len as u64);
    if *total > limit {
        return Err(Error::PayloadTooLarge(format!(
            "multipart request exceeds the {limit}-byte limit"
        )));
    }
    Ok(*total)
}

fn multer_error(err: multer::Error) -> Error {
    match err {
        multer::Error::FieldSizeExceeded { limit, .. } => Error::PayloadTooLarge(format!(
            "multipart field exceeds the {limit}-byte limit"
        )),
        multer::Error::StreamSizeExceeded { limit } => Error::PayloadTooLarge(format!(
            "multipart request exceeds the {limit}-byte limit"
        )),
        multer::Error::IncompleteFieldData { .. } | multer::Error::IncompleteStream => {
            Error::Validation("multipart body ended unexpectedly".to_string())
        }
        other => Error::Validation(format!("invalid multipart body: {other}")),
    }
}

// ---------------------------------------------------------------------------
// Response
// ---------------------------------------------------------------------------

/// Concrete implementation of IHttpResponse.
pub struct HttpResponse {
    status: u16,
    headers: HashMap<String, String>,
    body: Option<ResponseBody>,
}

impl HttpResponse {
    pub fn new(status: u16) -> Self {
        Self {
            status,
            headers: HashMap::new(),
            body: None,
        }
    }
}

#[async_trait::async_trait]
impl IHttpResponse for HttpResponse {
    fn status(&self) -> u16 {
        self.status
    }

    fn has_body(&self) -> bool {
        self.body.is_some()
    }

    fn set_status(&mut self, code: u16) {
        self.status = code;
    }

    fn set_header(&mut self, key: &str, value: &str) {
        self.headers.insert(key.to_string(), value.to_string());
    }

    fn remove_header(&mut self, key: &str) {
        self.headers.remove(key);
    }

    async fn write_bytes(&mut self, data: Vec<u8>) -> Result<()> {
        self.body = Some(ResponseBody::Bytes(data));
        Ok(())
    }

    async fn write_text(&mut self, text: &str) -> Result<()> {
        self.body = Some(ResponseBody::Bytes(text.as_bytes().to_vec()));
        Ok(())
    }

    async fn write_body(&mut self, body: ResponseBody) -> Result<()> {
        self.body = Some(body);
        Ok(())
    }

    fn body_bytes(&self) -> Vec<u8> {
        self.body
            .as_ref()
            .and_then(ResponseBody::as_bytes)
            .map(<[u8]>::to_vec)
            .unwrap_or_default()
    }

    fn header(&self, key: &str) -> Option<&str> {
        self.headers.get(key).map(|s| s.as_str())
    }
}

// ---------------------------------------------------------------------------
// Response finalisation
// ---------------------------------------------------------------------------

async fn finalize_response(req: &HttpRequest, resp: HttpResponse) -> hyper::Response<RespBody> {
    let HttpResponse {
        status,
        mut headers,
        body,
    } = resp;

    match body.unwrap_or(ResponseBody::Bytes(Vec::new())) {
        ResponseBody::Bytes(bytes) => {
            default_cache_control(&mut headers, false);
            bytes_response(status, headers, bytes)
        }
        ResponseBody::File(file) => {
            default_cache_control(&mut headers, true);
            file_response(req, status, headers, file).await
        }
    }
}

/// Apply the framework's default `Cache-Control` when the response has none.
///
/// Ordinary API responses must not be stored; a file response with validators is
/// exactly the case where `no-store` would defeat the `ETag` we just sent, so it
/// revalidates instead. Anything the application set explicitly wins.
fn default_cache_control(headers: &mut HashMap<String, String>, is_file: bool) {
    if headers.contains_key("cache-control") {
        return;
    }
    headers.insert(
        "cache-control".to_string(),
        if is_file {
            "public, max-age=0, must-revalidate".to_string()
        } else {
            "no-store".to_string()
        },
    );
}

/// A file body whose source has been resolved to something streamable.
enum PreparedSource {
    /// An open file handle; seeks to serve ranges.
    Path(tokio::fs::File),
    /// A reader that can seek, so ranges are served by seeking.
    Seekable(Box<dyn SeekableReader>),
    /// A reader of unknown length; chunked, no ranges.
    Stream(Box<dyn AsyncRead + Send + Unpin>),
}

impl PreparedSource {
    fn is_seekable(&self) -> bool {
        matches!(self, PreparedSource::Path(_) | PreparedSource::Seekable(_))
    }
}

/// Serve a file response with validators, conditional handling and byte ranges.
///
/// Works the same way for a path, a seekable stream and a plain stream; the only
/// differences are which HTTP features the source can support.
async fn file_response(
    req: &HttpRequest,
    status: u16,
    mut headers: HashMap<String, String>,
    file: FileBody,
) -> hyper::Response<RespBody> {
    let FileBody {
        source,
        content_type,
        file_name,
        disposition,
        enable_range_processing,
        last_modified,
        entity_tag,
    } = file;

    // ── Resolve the source, its length and its validators ──
    let mut modified = last_modified;
    let mut etag = entity_tag;
    let mut source_path: Option<PathBuf> = None;
    let known_len;

    let prepared = match source {
        FileSource::Path(path) => {
            // Open once and take length/mtime from the same handle: one syscall
            // pair instead of `metadata()` followed by `open()`.
            let handle = match tokio::fs::File::open(&path).await {
                Ok(handle) => handle,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    return problem_bytes(404, headers, "Not Found")
                }
                Err(err) => {
                    return problem_bytes(
                        500,
                        headers,
                        &format!("cannot open `{}`: {err}", path.display()),
                    )
                }
            };
            let metadata = match handle.metadata().await {
                Ok(metadata) => metadata,
                Err(err) => {
                    return problem_bytes(
                        500,
                        headers,
                        &format!("cannot stat `{}`: {err}", path.display()),
                    )
                }
            };
            if !metadata.is_file() {
                return problem_bytes(404, headers, "Not Found");
            }

            let len = metadata.len();
            if modified.is_none() {
                modified = metadata.modified().ok();
            }
            if etag.is_none() {
                etag = Some(etag_for(len, modified));
            }
            known_len = Some(len);
            source_path = Some(path);
            PreparedSource::Path(handle)
        }
        FileSource::SeekableStream { reader, len } => {
            known_len = Some(len);
            PreparedSource::Seekable(reader)
        }
        FileSource::Stream(reader) => {
            known_len = None;
            PreparedSource::Stream(reader)
        }
    };

    // RFC 7232 §2.2.2: an origin server must not send a `Last-Modified` later
    // than the response generation time, and must not treat such a date as a
    // precondition. Restored archives and clock skew produce future mtimes, and
    // they would otherwise make `If-Modified-Since` and date-based `If-Range`
    // answer nonsense. The ETag above was already derived from the real mtime,
    // so content changes are still detected.
    let modified = modified.filter(|value| *value <= SystemTime::now());

    // ── Representation metadata ──
    if !headers.contains_key("content-type") {
        let resolved = content_type
            .or_else(|| source_path.as_deref().and_then(guess_content_type))
            .unwrap_or_else(|| "application/octet-stream".to_string());
        headers.insert("content-type".to_string(), resolved);
    }
    if !headers.contains_key("content-disposition") {
        if let Some(name) = &file_name {
            headers.insert(
                "content-disposition".to_string(),
                content_disposition_value(disposition, name),
            );
        }
    }
    if let Some(tag) = &etag {
        headers.insert("etag".to_string(), tag.clone());
    }
    if let Some(modified) = modified {
        headers.insert(
            "last-modified".to_string(),
            httpdate::fmt_http_date(modified),
        );
    }

    // `Accept-Ranges: none` tells download managers not to bother trying, which
    // is more useful than the header's absence (absence means "unknown").
    let range_capable = enable_range_processing && prepared.is_seekable() && known_len.is_some();
    headers.insert(
        "accept-ranges".to_string(),
        if range_capable { "bytes" } else { "none" }.to_string(),
    );

    // ── Conditional GET: the representation is unchanged ──
    if status == 200 && is_not_modified(req, etag.as_deref(), modified) {
        headers.remove("content-length");
        return bytes_response(304, headers, Vec::new());
    }

    // ── Byte ranges ──
    let mut ranges: Vec<ByteRange> = Vec::new();
    if status == 200 && range_capable {
        let len = known_len.expect("range-capable sources know their length");
        if let Some(raw) = req.header("range") {
            // `If-Range` gates the whole range request: if the client's copy is
            // stale, the correct answer is the *full* representation, not a
            // range from a different version.
            if if_range_matches(req, etag.as_deref(), modified) {
                match parse_ranges(raw, len) {
                    RangeOutcome::Ignored => {}
                    RangeOutcome::Unsatisfiable => {
                        headers.insert("content-range".to_string(), format!("bytes */{len}"));
                        return problem_bytes(416, headers, "Range Not Satisfiable");
                    }
                    RangeOutcome::Satisfiable(parsed) => ranges = parsed,
                }
            }
        }
    }

    // ── Body ──
    //
    // A multi-range request needs to re-read the same source from several
    // offsets. Only a file handle can do that; a range-capable *stream* cannot
    // be restarted, so those requests are answered with the full representation
    // (RFC 7233 §3.1 lets a server ignore `Range`).
    if ranges.len() > 1 && !matches!(prepared, PreparedSource::Path(_)) {
        ranges.clear();
    }
    if ranges.len() > 1 {
        return multipart_byteranges_response(prepared, headers, ranges, known_len);
    }

    let (status, start, count) = match ranges.first() {
        Some(range) => {
            let len = known_len.expect("range-capable sources know their length");
            headers.insert(
                "content-range".to_string(),
                format!("bytes {}-{}/{len}", range.start, range.end),
            );
            (206, range.start, range.len())
        }
        None => (status, 0, known_len.unwrap_or(0)),
    };

    match prepared {
        PreparedSource::Path(mut handle) => {
            headers.insert("content-length".to_string(), count.to_string());
            if count == 0 {
                return bytes_response(status, headers, Vec::new());
            }
            if start > 0 {
                if let Err(err) = handle.seek(SeekFrom::Start(start)).await {
                    return problem_bytes(500, headers, &format!("cannot seek: {err}"));
                }
            }
            build_response(status, headers, reader_body(handle.take(count)))
        }
        PreparedSource::Seekable(mut reader) => {
            headers.insert("content-length".to_string(), count.to_string());
            if count == 0 {
                return bytes_response(status, headers, Vec::new());
            }
            if start > 0 {
                if let Err(err) = reader.seek(SeekFrom::Start(start)).await {
                    return problem_bytes(500, headers, &format!("cannot seek: {err}"));
                }
            }
            build_response(status, headers, reader_body(reader.take(count)))
        }
        PreparedSource::Stream(reader) => {
            // Unknown length: chunked transfer encoding, no `Content-Length`.
            // Nothing is buffered.
            build_response(status, headers, reader_body(reader))
        }
    }
}

/// The media type of the parts inside a `multipart/byteranges` response.
///
/// The response-level `Content-Type` is about to be replaced, so the original
/// representation type has to be captured first.
fn representation_content_type(headers: &HashMap<String, String>) -> String {
    headers
        .get("content-type")
        .filter(|value| !value.starts_with("multipart/byteranges"))
        .cloned()
        .unwrap_or_else(|| "application/octet-stream".to_string())
}

/// Serve several disjoint ranges as one `multipart/byteranges` response.
///
/// Ranges are coalesced before this point, so the parts are disjoint and the
/// total payload never exceeds the file size — no traffic amplification. The
/// response length is computed exactly rather than falling back to chunked,
/// because every part's size is known up front.
fn multipart_byteranges_response(
    prepared: PreparedSource,
    mut headers: HashMap<String, String>,
    ranges: Vec<ByteRange>,
    known_len: Option<u64>,
) -> hyper::Response<RespBody> {
    let PreparedSource::Path(handle) = prepared else {
        unreachable!("multi-range is only routed here for path sources");
    };

    let total_len = known_len.unwrap_or(0);
    let media_type = representation_content_type(&headers);
    let boundary = format!("webx-{}", uuid::Uuid::new_v4().simple());

    // Pre-render each part's header block so the total length is exact by
    // construction and the stream only has to emit bytes.
    let parts: Vec<(Bytes, ByteRange)> = ranges
        .iter()
        .map(|range| {
            let header = format!(
                "--{boundary}\r\nContent-Type: {media_type}\r\nContent-Range: bytes {}-{}/{total_len}\r\n\r\n",
                range.start, range.end
            );
            (Bytes::from(header), *range)
        })
        .collect();
    let closing = Bytes::from(format!("--{boundary}--\r\n"));

    let content_length: u64 = parts
        .iter()
        .map(|(header, range)| header.len() as u64 + range.len() + 2)
        .sum::<u64>()
        + closing.len() as u64;

    headers.insert(
        "content-type".to_string(),
        format!("multipart/byteranges; boundary={boundary}"),
    );
    headers.insert("content-length".to_string(), content_length.to_string());
    // `Content-Range` is per part here, so the message must not carry one.
    headers.remove("content-range");

    let stream = multipart_stream(handle, parts, closing);
    build_response(
        206,
        headers,
        BodyExt::boxed_unsync(http_body_util::StreamBody::new(stream)),
    )
}

/// Chunk size for each `multipart/byteranges` part read.
const MULTIPART_CHUNK: usize = STREAM_BUF_SIZE;

/// State carried between polls of the `multipart/byteranges` stream.
struct MultipartState {
    handle: tokio::fs::File,
    parts: std::vec::IntoIter<(Bytes, ByteRange)>,
    closing: Bytes,
    /// Header bytes still to emit for the current part.
    pending_header: Bytes,
    phase: MultipartPhase,
    buffer: bytes::BytesMut,
}

enum MultipartPhase {
    /// Move to the next part's header, or close the body.
    NextPart,
    /// Stream the current part's payload.
    Payload(u64),
    /// Emit the `\r\n` that terminates a part.
    Terminator,
    /// Emit the closing delimiter.
    Closing,
    Done,
}

/// Build the `multipart/byteranges` body as a stream of frames.
///
/// Written as a `Stream` rather than a hand-rolled `AsyncRead`: the byte layout
/// is a sequence of independent chunks, so `unfold` expresses it directly and
/// there is no `poll_*` plumbing to get wrong.
fn multipart_stream(
    handle: tokio::fs::File,
    parts: Vec<(Bytes, ByteRange)>,
    closing: Bytes,
) -> impl futures_util::Stream<Item = std::io::Result<Frame<Bytes>>> + Send {
    let state = MultipartState {
        handle,
        parts: parts.into_iter(),
        closing,
        pending_header: Bytes::new(),
        phase: MultipartPhase::NextPart,
        buffer: bytes::BytesMut::new(),
    };

    futures_util::stream::unfold(state, |mut state| async move {
        loop {
            match std::mem::replace(&mut state.phase, MultipartPhase::Done) {
                MultipartPhase::NextPart => match state.parts.next() {
                    Some((header, range)) => {
                        if let Err(err) = state.handle.seek(SeekFrom::Start(range.start)).await {
                            return Some((Err(err), state));
                        }
                        state.pending_header = header;
                        state.phase = MultipartPhase::Payload(range.len());
                    }
                    None => state.phase = MultipartPhase::Closing,
                },
                MultipartPhase::Payload(remaining) => {
                    if remaining == 0 {
                        state.phase = MultipartPhase::Terminator;
                        continue;
                    }

                    // Read at most one chunk, never past the part boundary.
                    let limit = MULTIPART_CHUNK.min(remaining as usize);
                    state.buffer.clear();
                    state.buffer.reserve(limit);
                    let read = match state.handle.read_buf(&mut state.buffer).await {
                        Ok(read) => read,
                        Err(err) => return Some((Err(err), state)),
                    };
                    if read == 0 {
                        return Some((
                            Err(std::io::Error::new(
                                std::io::ErrorKind::UnexpectedEof,
                                "file changed size while serving byte ranges",
                            )),
                            state,
                        ));
                    }
                    let keep = read.min(limit);
                    state.buffer.truncate(keep);
                    let chunk = state.buffer.split().freeze();

                    // The part header is emitted immediately before its first
                    // payload byte, so a part is one contiguous run of frames.
                    let header = std::mem::take(&mut state.pending_header);
                    let frame = if header.is_empty() {
                        chunk
                    } else {
                        let mut combined = bytes::BytesMut::with_capacity(header.len() + chunk.len());
                        combined.extend_from_slice(&header);
                        combined.extend_from_slice(&chunk);
                        combined.freeze()
                    };

                    state.phase = MultipartPhase::Payload(remaining - keep as u64);
                    return Some((Ok(Frame::data(frame)), state));
                }
                MultipartPhase::Terminator => {
                    state.phase = MultipartPhase::NextPart;
                    return Some((Ok(Frame::data(Bytes::from_static(b"\r\n"))), state));
                }
                MultipartPhase::Closing => {
                    state.phase = MultipartPhase::Done;
                    let closing = std::mem::take(&mut state.closing);
                    return Some((Ok(Frame::data(closing)), state));
                }
                MultipartPhase::Done => return None,
            }
        }
    })
}

/// Read buffer used for every streamed response body.
///
/// Shared with the upload path so both sides of a transfer use the same size.
const STREAM_BUF_SIZE: usize = rust_webx_core::http::STREAM_BUF_SIZE;

/// Turn any async reader into a streaming response body.
fn reader_body<R>(reader: R) -> RespBody
where
    R: tokio::io::AsyncRead + Send + 'static,
{
    let stream = tokio_util::io::ReaderStream::with_capacity(reader, STREAM_BUF_SIZE);
    let body = http_body_util::StreamBody::new(stream.map_ok(Frame::data));
    BodyExt::boxed_unsync(body)
}

fn bytes_response(
    status: u16,
    headers: HashMap<String, String>,
    bytes: Vec<u8>,
) -> hyper::Response<RespBody> {
    build_response(status, headers, full_body(bytes))
}

fn full_body(bytes: Vec<u8>) -> RespBody {
    Full::new(Bytes::from(bytes))
        .map_err(|never: std::convert::Infallible| match never {})
        .boxed_unsync()
}

/// Build an RFC 7807 `application/problem+json` response for a file error.
fn problem_bytes(
    status: u16,
    mut headers: HashMap<String, String>,
    detail: &str,
) -> hyper::Response<RespBody> {
    headers.insert(
        "content-type".to_string(),
        "application/problem+json".to_string(),
    );
    bytes_response(status, headers, problem_to_bytes(&build_problem(status, detail)))
}

fn build_response(
    status: u16,
    headers: HashMap<String, String>,
    body: RespBody,
) -> hyper::Response<RespBody> {
    let mut builder = hyper::Response::builder().status(status);
    for (key, value) in &headers {
        builder = builder.header(key.as_str(), value.as_str());
    }
    builder.body(body).unwrap_or_else(|_| {
        hyper::Response::builder()
            .status(hyper::StatusCode::INTERNAL_SERVER_ERROR)
            .body(full_body(b"Internal Server Error".to_vec()))
            .expect("static error response is valid")
    })
}

/// Result of parsing a `Range` header.
#[derive(Debug, PartialEq, Eq)]
enum RangeOutcome {
    /// Syntactically invalid: the whole header must be ignored (RFC 7233 §2.1).
    Ignored,
    /// Valid but nothing overlaps the representation: answer `416`.
    Unsatisfiable,
    /// One or more disjoint, coalesced ranges, in ascending order.
    Satisfiable(Vec<ByteRange>),
}

/// Maximum number of parts to answer in one `multipart/byteranges` response.
///
/// Ranges are coalesced first, so the parts are disjoint and the total payload
/// can never exceed the file size; this cap only bounds the *framing* overhead
/// and the per-part seeks. A client asking for more gets the full
/// representation, which RFC 7233 §3.1 permits.
const MAX_RANGE_PARTS: usize = 8;

/// Parse a byte-range-set (RFC 7233 §2.1) and coalesce it.
///
/// Coalescing matters for more than tidiness: without it a client could ask for
/// the same range eight times and make the server send the file eight times
/// over. Merging overlapping and adjacent ranges guarantees the parts are
/// disjoint, so the response can never be larger than the file.
fn parse_ranges(value: &str, len: u64) -> RangeOutcome {
    let value = value.trim();
    let Some(spec) = strip_prefix_ignore_ascii_case(value, "bytes=") else {
        return RangeOutcome::Ignored;
    };

    let mut ranges: Vec<ByteRange> = Vec::new();
    let mut saw_valid_spec = false;

    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        match parse_single_range(part, len) {
            // One malformed member invalidates the whole header.
            SingleRange::Malformed => return RangeOutcome::Ignored,
            SingleRange::Unsatisfiable => saw_valid_spec = true,
            SingleRange::Satisfiable(range) => {
                saw_valid_spec = true;
                ranges.push(range);
            }
        }
    }

    if ranges.is_empty() {
        return if saw_valid_spec {
            RangeOutcome::Unsatisfiable
        } else {
            RangeOutcome::Ignored
        };
    }

    ranges.sort_by_key(|range| range.start);
    let mut merged: Vec<ByteRange> = Vec::with_capacity(ranges.len());
    for range in ranges {
        match merged.last_mut() {
            // `+ 1` merges adjacent ranges too (`0-9,10-19` is one read).
            Some(last) if range.start <= last.end.saturating_add(1) => {
                last.end = last.end.max(range.end);
            }
            _ => merged.push(range),
        }
    }

    if merged.len() > MAX_RANGE_PARTS {
        return RangeOutcome::Ignored;
    }

    RangeOutcome::Satisfiable(merged)
}

enum SingleRange {
    /// Not a valid `byte-range-spec`.
    Malformed,
    /// Valid syntax, but outside the representation.
    Unsatisfiable,
    Satisfiable(ByteRange),
}

/// Parse one `first-byte-pos "-" [last-byte-pos]` or `"-" suffix-length`.
fn parse_single_range(spec: &str, len: u64) -> SingleRange {
    let Some((start_text, end_text)) = spec.split_once('-') else {
        return SingleRange::Malformed;
    };
    let (start_text, end_text) = (start_text.trim(), end_text.trim());

    if start_text.is_empty() {
        // Suffix range: the last N bytes.
        let Ok(suffix) = end_text.parse::<u64>() else {
            return SingleRange::Malformed;
        };
        if suffix == 0 || len == 0 {
            return SingleRange::Unsatisfiable;
        }
        return SingleRange::Satisfiable(ByteRange {
            start: len.saturating_sub(suffix),
            end: len - 1,
        });
    }

    let Ok(start) = start_text.parse::<u64>() else {
        return SingleRange::Malformed;
    };
    if start >= len {
        return SingleRange::Unsatisfiable;
    }

    let end = if end_text.is_empty() {
        len - 1
    } else {
        match end_text.parse::<u64>() {
            Ok(end) => end.min(len - 1),
            Err(_) => return SingleRange::Malformed,
        }
    };

    if end < start {
        return SingleRange::Unsatisfiable;
    }

    SingleRange::Satisfiable(ByteRange { start, end })
}

fn strip_prefix_ignore_ascii_case<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    if value.len() >= prefix.len() && value[..prefix.len()].eq_ignore_ascii_case(prefix) {
        Some(&value[prefix.len()..])
    } else {
        None
    }
}

/// A weak validator derived from size and modification time.
///
/// This is the same scheme nginx and most static servers use: free to compute,
/// and it changes whenever the file does. Full `SystemTime` precision is used so
/// an in-place edit that keeps the size is still detected within the same
/// second — `Last-Modified` cannot express that, but an `ETag` can.
///
/// It is emitted as a **strong** tag because download resumption needs one:
/// RFC 7233 §3.2 requires a strong validator for `If-Range`, so a weak tag would
/// silently disable resumable downloads. Callers that need a byte-exact
/// validator should set one explicitly with `FileBody::entity_tag`.
fn etag_for(len: u64, modified: Option<SystemTime>) -> String {
    match modified {
        Some(modified) => format!("\"{:x}-{:x}\"", len, epoch_nanos(modified)),
        None => format!("\"{len:x}\""),
    }
}

fn epoch_secs(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn epoch_nanos(time: SystemTime) -> u128 {
    time.duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// Whether an entity-tag is weak (`W/"..."`).
fn is_weak_etag(tag: &str) -> bool {
    tag.trim_start().starts_with("W/")
}

/// Strip the weakness marker so two tags can be compared by value.
fn etag_value(tag: &str) -> &str {
    tag.trim().trim_start_matches("W/")
}

/// Whether the client's cached representation is still current.
///
/// Uses **weak** comparison, per RFC 7232 §3.2: `If-None-Match` only asks
/// whether the content is semantically the same, so a weak tag is enough. A
/// response without validators can never answer `304`.
fn is_not_modified(req: &HttpRequest, etag: Option<&str>, modified: Option<SystemTime>) -> bool {
    if let Some(if_none_match) = req.header("if-none-match") {
        let Some(etag) = etag else {
            return false;
        };
        let expected = etag_value(etag);
        return if_none_match.split(',').any(|candidate| {
            let candidate = candidate.trim();
            candidate == "*" || etag_value(candidate) == expected
        });
    }

    if let Some(if_modified_since) = req.header("if-modified-since") {
        if let Some(modified) = modified {
            if let Ok(since) = httpdate::parse_http_date(if_modified_since) {
                return epoch_secs(modified) <= epoch_secs(since);
            }
        }
    }

    false
}

/// Whether a `Range` header should be honoured, per RFC 7233 §3.2.
///
/// `If-Range` is the precondition that makes resumption safe, so it is strict:
///
/// * an entity-tag must match by **strong** comparison — both tags must be
///   strong and byte-equal. A weak tag (ours or the client's) can never match,
///   because a weak validator does not promise byte-for-byte identity;
/// * a date must be at least as new as `Last-Modified`.
///
/// Returning `false` means "ignore `Range` and send the full representation".
fn if_range_matches(req: &HttpRequest, etag: Option<&str>, modified: Option<SystemTime>) -> bool {
    let Some(value) = req.header("if-range") else {
        return true;
    };
    let value = value.trim();

    if value.starts_with('"') || value.starts_with("W/") {
        let Some(etag) = etag else {
            return false;
        };
        if is_weak_etag(value) || is_weak_etag(etag) {
            return false;
        }
        return value == etag.trim();
    }

    match (httpdate::parse_http_date(value), modified) {
        (Ok(instant), Some(modified)) => epoch_secs(instant) >= epoch_secs(modified),
        _ => false,
    }
}

fn guess_content_type(path: &Path) -> Option<String> {
    mime_guess::from_path(path)
        .first()
        .map(|mime| mime.essence_str().to_string())
}

// ---------------------------------------------------------------------------
// Query string helpers
// ---------------------------------------------------------------------------

/// Parse a query string like "key1=val1&key2=val2" into a HashMap.
fn parse_query_string(query: &str) -> HashMap<String, String> {
    let mut params = HashMap::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let mut parts = pair.splitn(2, '=');
        if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
            params.insert(percent_decode(key), percent_decode(value));
        }
    }
    params
}

fn percent_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                result.push(byte as char);
            }
        } else if c == '+' {
            result.push(' ');
        } else {
            result.push(c);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_boundary_parameter() {
        assert_eq!(
            multipart_boundary("multipart/form-data; boundary=----abc123").as_deref(),
            Some("----abc123")
        );
        assert_eq!(
            multipart_boundary("multipart/form-data; charset=utf-8; boundary=\"quoted\"").as_deref(),
            Some("quoted")
        );
        assert_eq!(multipart_boundary("multipart/form-data"), None);
    }

    fn ranges(start: u64, end: u64) -> RangeOutcome {
        RangeOutcome::Satisfiable(vec![ByteRange { start, end }])
    }

    #[test]
    fn parses_byte_ranges() {
        assert_eq!(parse_ranges("bytes=0-99", 1000), ranges(0, 99));
        assert_eq!(parse_ranges("bytes=500-", 1000), ranges(500, 999));
        assert_eq!(parse_ranges("bytes=-200", 1000), ranges(800, 999));
        // Clamped to the end of the representation.
        assert_eq!(parse_ranges("bytes=0-99999", 1000), ranges(0, 999));
        // Case-insensitive unit, and whitespace around members is tolerated.
        assert_eq!(parse_ranges("BYTES=0-9", 1000), ranges(0, 9));
        assert_eq!(parse_ranges("bytes= 0-9 ", 1000), ranges(0, 9));
    }

    #[test]
    fn rejects_unsatisfiable_and_malformed_ranges() {
        assert_eq!(parse_ranges("bytes=1000-", 1000), RangeOutcome::Unsatisfiable);
        assert_eq!(parse_ranges("bytes=-0", 1000), RangeOutcome::Unsatisfiable);
        assert_eq!(
            parse_ranges("bytes=500-100", 1000),
            RangeOutcome::Unsatisfiable
        );
        assert_eq!(parse_ranges("items=0-10", 1000), RangeOutcome::Ignored);
        assert_eq!(parse_ranges("bytes=", 1000), RangeOutcome::Ignored);
        // RFC 7233 §2.1: one malformed member invalidates the whole header.
        assert_eq!(parse_ranges("bytes=abc-def", 1000), RangeOutcome::Ignored);
        assert_eq!(parse_ranges("bytes=0-9,abc-def", 1000), RangeOutcome::Ignored);
    }

    #[test]
    fn suffix_range_on_empty_file_is_unsatisfiable() {
        assert_eq!(parse_ranges("bytes=-10", 0), RangeOutcome::Unsatisfiable);
    }

    #[test]
    fn multi_range_is_coalesced_into_disjoint_parts() {
        // Disjoint ranges stay separate, in ascending order.
        assert_eq!(
            parse_ranges("bytes=20-30,0-10", 1000),
            RangeOutcome::Satisfiable(vec![
                ByteRange { start: 0, end: 10 },
                ByteRange { start: 20, end: 30 },
            ])
        );

        // Overlapping ranges merge, so a client cannot ask for the same bytes
        // twice and make the server send them twice.
        assert_eq!(
            parse_ranges("bytes=0-99,50-149", 1000),
            ranges(0, 149)
        );

        // Adjacent ranges merge too — one read instead of two.
        assert_eq!(parse_ranges("bytes=0-9,10-19", 1000), ranges(0, 19));

        // Duplicates collapse completely: no traffic amplification.
        assert_eq!(
            parse_ranges("bytes=0-,0-,0-,0-,0-,0-,0-,0-", 1000),
            ranges(0, 999)
        );

        // An unsatisfiable member next to a satisfiable one is dropped, not fatal.
        assert_eq!(parse_ranges("bytes=0-9,5000-6000", 1000), ranges(0, 9));
    }

    #[test]
    fn too_many_parts_falls_back_to_the_full_representation() {
        let mut spec = String::from("bytes=");
        for index in 0..(MAX_RANGE_PARTS + 1) {
            if index > 0 {
                spec.push(',');
            }
            spec.push_str(&format!("{}-{}", index * 4, index * 4 + 1));
        }
        assert_eq!(parse_ranges(&spec, 1000), RangeOutcome::Ignored);
    }

    #[test]
    fn etag_changes_with_size_and_mtime() {
        let base = UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        assert_eq!(etag_for(10, Some(base)), etag_for(10, Some(base)));
        assert_ne!(etag_for(10, Some(base)), etag_for(11, Some(base)));
        assert_ne!(
            etag_for(10, Some(base)),
            etag_for(10, Some(base + std::time::Duration::from_secs(1)))
        );
        // Sub-second precision matters: an in-place edit that keeps the size is
        // invisible to `Last-Modified`, but must change the ETag. (1 µs rather
        // than 1 ns, because Windows filesystem timestamps are 100 ns ticks.)
        assert_ne!(
            etag_for(10, Some(base)),
            etag_for(10, Some(base + std::time::Duration::from_micros(1)))
        );
    }

    #[test]
    fn etag_strength_rules() {
        assert!(!is_weak_etag("\"abc\""));
        assert!(is_weak_etag("W/\"abc\""));
        assert_eq!(etag_value("W/\"abc\""), "\"abc\"");
        assert_eq!(etag_value("\"abc\""), "\"abc\"");
    }

    #[test]
    fn parses_query_strings() {
        let params = parse_query_string("a=1&b=hello+world&c=%2Ftmp");
        assert_eq!(params.get("a").map(String::as_str), Some("1"));
        assert_eq!(params.get("b").map(String::as_str), Some("hello world"));
        assert_eq!(params.get("c").map(String::as_str), Some("/tmp"));
    }
}

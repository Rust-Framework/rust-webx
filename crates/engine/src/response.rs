//! Response assembly for the 2.0 engine.
//!
//! Everything between an [`EndpointResult`] and hyper's `Response<RespBody>`:
//! status/header layout, byte bodies, file bodies (validators, conditional
//! requests, byte ranges, `multipart/byteranges`) and HEAD body suppression.
//!
//! The file/range logic is the proven 1.5 implementation moved out of
//! `webx_host::context`, including its range and validator tests. The one
//! behavioural difference is that the framework's default `Cache-Control` is
//! no longer applied here: cache policy is host middleware in 2.0, while this
//! module owns only content-type/body/status.

use bytes::Bytes;
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Full, StreamBody};
use hyper::body::Frame;
use hyper::header::{HeaderMap, HeaderName, HeaderValue};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, SeekFrom};
use webx_core::http::{
    content_disposition_value, ByteRange, FileBody, FileSource, IHttpRequest, SeekableReader,
    STREAM_BUF_SIZE,
};
use webx_core::problem::ProblemDetails;

/// Response body produced by the engine.
///
/// Deliberately the *unsync* box: a streamed body only ever needs to be `Send`,
/// and requiring `Sync` would rule out readers that are not shared between
/// threads (most streaming sources).
pub type RespBody = UnsyncBoxBody<Bytes, io::Error>;

/// The result of one HTTP handler dispatch, before it is written to the
/// response.
///
/// Re-exported from `webx_core` on purpose: direct dispatch functions, the
/// router and the endpoint adapter all construct this one type, so the engine
/// must not define a second enum with the same shape.
pub use webx_core::action::EndpointResult;

/// The payload a response carries before it becomes a hyper body.
#[derive(Debug)]
pub enum ResponseBodyState {
    /// Nothing was written: the status decides the semantics (204, 304, ...).
    None,
    /// In-memory bytes. Already `Bytes`, so a JSON DTO handed in through
    /// [`EndpointResult::Json`] is not copied again.
    Bytes(Bytes),
    /// A file or stream, served with validators and byte ranges.
    File(FileBody),
}

/// Status, headers and payload of a response, ready to become hyper's.
pub struct ResponseParts {
    /// HTTP status code.
    pub status: u16,
    /// Response headers; may hold duplicate values (`Set-Cookie`).
    pub headers: HeaderMap,
    /// The response payload.
    pub body: ResponseBodyState,
}

impl ResponseParts {
    /// An empty response with `status` and no headers.
    pub fn new(status: u16) -> Self {
        Self {
            status,
            headers: HeaderMap::new(),
            body: ResponseBodyState::None,
        }
    }

    /// Turn the parts into a hyper response.
    ///
    /// HEAD responses keep the representation headers (`Content-Length`,
    /// `ETag`, ...) and drop only the body. Byte bodies get an explicit
    /// `Content-Length` for HEAD, because hyper's HTTP/2 server does not derive
    /// it while suppressing the body; every other `Content-Length` is left for
    /// hyper to derive from the exact body size.
    pub async fn assemble(self, req: &mut dyn IHttpRequest) -> hyper::Response<RespBody> {
        let ResponseParts {
            status,
            mut headers,
            body,
        } = self;
        let is_head = req.method().eq_ignore_ascii_case("HEAD");
        apply_security_headers(&mut headers);

        let mut response = match body {
            ResponseBodyState::File(file) => file_response(req, status, headers, file).await,
            ResponseBodyState::Bytes(bytes) => {
                if is_head && !headers.contains_key("content-length") {
                    set_content_length(&mut headers, bytes.len() as u64);
                }
                // Framework default for API/byte responses; a handler or
                // middleware that set a policy keeps it.
                if !headers.contains_key("cache-control") {
                    insert_static_header(&mut headers, "cache-control", "no-store");
                }
                bytes_response(status, headers, bytes)
            }
            ResponseBodyState::None => {
                if is_head && !headers.contains_key("content-length") {
                    set_content_length(&mut headers, 0);
                }
                if !headers.contains_key("cache-control") {
                    insert_static_header(&mut headers, "cache-control", "no-store");
                }
                bytes_response(status, headers, Bytes::new())
            }
        };

        suppress_head_body(&mut response, is_head);
        response
    }
}

/// Drop the body of a HEAD response, keeping its representation headers.
///
/// hyper suppresses the body for HEAD on HTTP/1.1 but not on HTTP/2, so the
/// engine does it for both protocols and keeps the `Content-Length` RFC 9110
/// requires for the corresponding GET.
pub fn suppress_head_body(response: &mut hyper::Response<RespBody>, is_head: bool) {
    if is_head {
        *response.body_mut() = empty_body();
    }
}

// ---------------------------------------------------------------------------
// Header helpers
// ---------------------------------------------------------------------------

/// Insert/replace a header, ignoring names/values that are not valid HTTP.
pub(crate) fn set_header(headers: &mut HeaderMap, key: &str, value: &str) {
    let (Ok(name), Ok(value)) = (
        HeaderName::from_bytes(key.as_bytes()),
        HeaderValue::from_str(value),
    ) else {
        tracing::warn!(header = key, "ignoring invalid response header");
        return;
    };
    headers.insert(name, value);
}

/// Append a header without replacing existing values.
pub(crate) fn append_header(headers: &mut HeaderMap, key: &str, value: &str) {
    let (Ok(name), Ok(value)) = (
        HeaderName::from_bytes(key.as_bytes()),
        HeaderValue::from_str(value),
    ) else {
        tracing::warn!(header = key, "ignoring invalid response header");
        return;
    };
    headers.append(name, value);
}

/// Apply the framework's default security headers without parsing or
/// allocating. A value already set by the application/middleware wins.
fn apply_security_headers(headers: &mut HeaderMap) {
    for (name, value) in [
        ("x-content-type-options", "nosniff"),
        ("x-frame-options", "DENY"),
        (
            "strict-transport-security",
            "max-age=31536000; includeSubDomains",
        ),
        ("referrer-policy", "strict-origin-when-cross-origin"),
        (
            "permissions-policy",
            "camera=(), microphone=(), geolocation=()",
        ),
    ] {
        if !headers.contains_key(name) {
            insert_static_header(headers, name, value);
        }
    }
}

/// Set a header whose name and value are compile-time constants.
///
/// Used for the framework's own headers (`Content-Type: application/json`,
/// problem details, ...): no parsing, and no allocation for the value.
pub(crate) fn insert_static_header(
    headers: &mut HeaderMap,
    name: &'static str,
    value: &'static str,
) {
    headers.insert(
        HeaderName::from_static(name),
        HeaderValue::from_static(value),
    );
}

fn set_content_length(headers: &mut HeaderMap, len: u64) {
    set_header(headers, "content-length", &len.to_string());
}

// ---------------------------------------------------------------------------
// Response bodies
// ---------------------------------------------------------------------------

fn full_body(bytes: Bytes) -> RespBody {
    Full::new(bytes)
        .map_err(|never: std::convert::Infallible| match never {})
        .boxed_unsync()
}

fn empty_body() -> RespBody {
    full_body(Bytes::new())
}

/// Read buffer used for every streamed response body.
///
/// Shared with the upload path so both sides of a transfer use the same size.
const STREAM_BUFFER: usize = STREAM_BUF_SIZE;

/// Turn any async reader into a streaming response body.
fn reader_body<R>(reader: R) -> RespBody
where
    R: tokio::io::AsyncRead + Send + 'static,
{
    use futures_util::TryStreamExt;
    let stream = tokio_util::io::ReaderStream::with_capacity(reader, STREAM_BUFFER);
    let body = StreamBody::new(stream.map_ok(Frame::data));
    BodyExt::boxed_unsync(body)
}

fn bytes_response(status: u16, headers: HeaderMap, bytes: Bytes) -> hyper::Response<RespBody> {
    build_response(status, headers, full_body(bytes))
}

fn build_response(status: u16, headers: HeaderMap, body: RespBody) -> hyper::Response<RespBody> {
    let mut response = hyper::Response::new(body);
    *response.status_mut() =
        hyper::StatusCode::from_u16(status).unwrap_or(hyper::StatusCode::INTERNAL_SERVER_ERROR);
    // Move the HeaderMap instead of rebuilding it through the builder: this
    // keeps duplicate headers (e.g. two `Set-Cookie`s) and avoids a second
    // allocation/copy pass for every response.
    *response.headers_mut() = headers;
    response
}

// ---------------------------------------------------------------------------
// File responses
// ---------------------------------------------------------------------------

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
///
/// SSE needs no separate path: `SseResult` writes a `FileBody::stream` whose
/// frames are the encoded events, so it arrives here as an unknown-length
/// source and is sent chunked with backpressure from the client.
async fn file_response(
    req: &mut dyn IHttpRequest,
    status: u16,
    mut headers: HeaderMap,
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
                    return problem_response(404, headers, "Not Found")
                }
                Err(err) => {
                    tracing::error!(
                        path = %path.display(),
                        error = %err,
                        "cannot open file for download"
                    );
                    return problem_response(500, headers, "an unexpected error occurred");
                }
            };
            let metadata = match handle.metadata().await {
                Ok(metadata) => metadata,
                Err(err) => {
                    tracing::error!(
                        path = %path.display(),
                        error = %err,
                        "cannot stat file for download"
                    );
                    return problem_response(500, headers, "an unexpected error occurred");
                }
            };
            if !metadata.is_file() {
                return problem_response(404, headers, "Not Found");
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
        set_header(&mut headers, "content-type", &resolved);
    }
    if !headers.contains_key("content-disposition") {
        if let Some(name) = &file_name {
            set_header(
                &mut headers,
                "content-disposition",
                &content_disposition_value(disposition, name),
            );
        }
    }
    if let Some(tag) = &etag {
        set_header(&mut headers, "etag", tag);
    }
    if let Some(modified) = modified {
        set_header(
            &mut headers,
            "last-modified",
            &httpdate::fmt_http_date(modified),
        );
    }

    // `Accept-Ranges: none` tells download managers not to bother trying, which
    // is more useful than the header's absence (absence means "unknown").
    let range_capable = enable_range_processing && prepared.is_seekable() && known_len.is_some();
    insert_static_header(
        &mut headers,
        "accept-ranges",
        if range_capable { "bytes" } else { "none" },
    );

    // ── Conditional GET: the representation is unchanged ──
    if status == 200 && is_not_modified(req, etag.as_deref(), modified) {
        headers.remove("content-length");
        return bytes_response(304, headers, Bytes::new());
    }

    // ── Byte ranges ──
    let mut ranges: Vec<ByteRange> = Vec::new();
    if status == 200 && range_capable {
        if let Some(len) = known_len {
            if let Some(raw) = req.header("range") {
                // `If-Range` gates the whole range request: if the client's copy
                // is stale, the correct answer is the *full* representation, not
                // a range from a different version.
                if if_range_matches(req, etag.as_deref(), modified) {
                    match parse_ranges(raw, len) {
                        RangeOutcome::Ignored => {}
                        RangeOutcome::Unsatisfiable => {
                            set_header(&mut headers, "content-range", &format!("bytes */{len}"));
                            return problem_response(416, headers, "Range Not Satisfiable");
                        }
                        RangeOutcome::Satisfiable(parsed) => ranges = parsed,
                    }
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
            let known_len = known_len.unwrap_or(0);
            set_header(
                &mut headers,
                "content-range",
                &format!("bytes {}-{}/{known_len}", range.start, range.end),
            );
            (206, range.start, range.len())
        }
        None => (status, 0, known_len.unwrap_or(0)),
    };

    match prepared {
        PreparedSource::Path(mut handle) => {
            set_content_length(&mut headers, count);
            if count == 0 {
                return bytes_response(status, headers, Bytes::new());
            }
            if start > 0 {
                if let Err(err) = handle.seek(SeekFrom::Start(start)).await {
                    tracing::error!(error = %err, "cannot seek file for ranged response");
                    return problem_response(500, headers, "an unexpected error occurred");
                }
            }
            build_response(status, headers, reader_body(handle.take(count)))
        }
        PreparedSource::Seekable(mut reader) => {
            set_content_length(&mut headers, count);
            if count == 0 {
                return bytes_response(status, headers, Bytes::new());
            }
            if start > 0 {
                if let Err(err) = reader.seek(SeekFrom::Start(start)).await {
                    tracing::error!(error = %err, "cannot seek stream for ranged response");
                    return problem_response(500, headers, "an unexpected error occurred");
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
fn representation_content_type(headers: &HeaderMap) -> String {
    headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.starts_with("multipart/byteranges"))
        .unwrap_or("application/octet-stream")
        .to_string()
}

/// Serve several disjoint ranges as one `multipart/byteranges` response.
///
/// Ranges are coalesced before this point, so the parts are disjoint and the
/// total payload never exceeds the file size — no traffic amplification. The
/// response length is computed exactly rather than falling back to chunked,
/// because every part's size is known up front.
fn multipart_byteranges_response(
    prepared: PreparedSource,
    mut headers: HeaderMap,
    ranges: Vec<ByteRange>,
    known_len: Option<u64>,
) -> hyper::Response<RespBody> {
    let PreparedSource::Path(handle) = prepared else {
        // Multi-range is only routed here for path sources; a seekable stream
        // is answered with the full representation instead. Reaching this is an
        // engine invariant violation, not a client error.
        return problem_response(500, headers, "an unexpected error occurred");
    };

    let total_len = known_len.unwrap_or(0);
    let media_type = representation_content_type(&headers);
    let boundary = next_byteranges_boundary();

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

    set_header(
        &mut headers,
        "content-type",
        &format!("multipart/byteranges; boundary={boundary}"),
    );
    set_content_length(&mut headers, content_length);
    // `Content-Range` is per part here, so the message must not carry one.
    headers.remove("content-range");

    let stream = multipart_stream(handle, parts, closing);
    build_response(206, headers, BodyExt::boxed_unsync(StreamBody::new(stream)))
}

/// A per-response boundary for `multipart/byteranges`.
///
/// It only has to be absent from the payload; a process-wide counter plus the
/// clock guarantees two concurrent responses never share a value.
fn next_byteranges_boundary() -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    format!("webx-{nanos:x}-{sequence:x}")
}

/// Chunk size for each `multipart/byteranges` part read.
const MULTIPART_CHUNK: usize = STREAM_BUFFER;

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
                        let mut combined =
                            bytes::BytesMut::with_capacity(header.len() + chunk.len());
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

// ---------------------------------------------------------------------------
// File errors
// ---------------------------------------------------------------------------

/// Build an RFC 7807 `application/problem+json` response for a file error.
///
/// Only reached for filesystem/range failures; normal handler errors are written
/// by the host's error middleware.
fn problem_response(
    status: u16,
    mut headers: HeaderMap,
    detail: &str,
) -> hyper::Response<RespBody> {
    insert_static_header(&mut headers, "content-type", "application/problem+json");
    let problem = ProblemDetails {
        typ: Some(format!("https://httpstatuses.com/{status}")),
        title: problem_title(status).to_string(),
        status,
        detail: Some(detail.to_string()),
        instance: None,
        extensions: None,
    };
    let body = serde_json::to_vec(&problem).unwrap_or_default();
    bytes_response(status, headers, Bytes::from(body))
}

/// Standard HTTP status phrase used as the problem `title`.
fn problem_title(status: u16) -> &'static str {
    match status {
        404 => "Not Found",
        416 => "Range Not Satisfiable",
        _ => "Internal Server Error",
    }
}

// ---------------------------------------------------------------------------
// Byte-range parsing
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Validators and conditional requests
// ---------------------------------------------------------------------------

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
fn is_not_modified(
    req: &dyn IHttpRequest,
    etag: Option<&str>,
    modified: Option<SystemTime>,
) -> bool {
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
fn if_range_matches(
    req: &dyn IHttpRequest,
    etag: Option<&str>,
    modified: Option<SystemTime>,
) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(
            parse_ranges("bytes=1000-", 1000),
            RangeOutcome::Unsatisfiable
        );
        assert_eq!(parse_ranges("bytes=-0", 1000), RangeOutcome::Unsatisfiable);
        assert_eq!(
            parse_ranges("bytes=500-100", 1000),
            RangeOutcome::Unsatisfiable
        );
        assert_eq!(parse_ranges("items=0-10", 1000), RangeOutcome::Ignored);
        assert_eq!(parse_ranges("bytes=", 1000), RangeOutcome::Ignored);
        // RFC 7233 §2.1: one malformed member invalidates the whole header.
        assert_eq!(parse_ranges("bytes=abc-def", 1000), RangeOutcome::Ignored);
        assert_eq!(
            parse_ranges("bytes=0-9,abc-def", 1000),
            RangeOutcome::Ignored
        );
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
        assert_eq!(parse_ranges("bytes=0-99,50-149", 1000), ranges(0, 149));

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
    fn invalid_response_headers_are_ignored_instead_of_panicking() {
        let mut headers = HeaderMap::new();
        set_header(&mut headers, "bad\nname", "x");
        set_header(&mut headers, "x-test", "bad\nvalue");
        assert!(headers.is_empty());
    }

    #[test]
    fn byteranges_boundaries_are_distinct() {
        assert_ne!(next_byteranges_boundary(), next_byteranges_boundary());
    }

    // -----------------------------------------------------------------------
    // File assembly end-to-end (path source, ranges, validators, HEAD)
    // -----------------------------------------------------------------------

    use crate::body::BodyLimits;
    use crate::context::EngineContext;
    use std::sync::Arc;
    use webx_core::http::{IHttpContext, ResponseBody};

    /// A 10-byte file plus the directory that owns it.
    fn sample_file() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("note.txt");
        std::fs::write(&path, b"abcdefghij").expect("sample file");
        (dir, path)
    }

    /// A context whose response already carries a file body.
    async fn file_context(method: &str, headers: &[(&str, &str)], body: FileBody) -> EngineContext {
        let mut builder = http::Request::builder().method(method).uri("/file");
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let parts = builder
            .body(())
            .expect("test request builds")
            .into_parts()
            .0;
        let mut ctx = EngineContext::from_parts(
            parts,
            Bytes::new(),
            Arc::new(BodyLimits::default()),
            None,
            None,
        );
        ctx.response_mut()
            .write_body(ResponseBody::File(body))
            .await
            .expect("the file body is accepted");
        ctx
    }

    async fn collect_body(response: hyper::Response<RespBody>) -> Bytes {
        response
            .into_body()
            .collect()
            .await
            .expect("body collects")
            .to_bytes()
    }

    fn header_value(response: &hyper::Response<RespBody>, name: &str) -> String {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    }

    #[tokio::test]
    async fn file_response_streams_the_bytes_with_validators() {
        let (_dir, path) = sample_file();
        let response = file_context("GET", &[], FileBody::path(&path))
            .await
            .finalize()
            .await;

        assert_eq!(response.status(), 200);
        assert_eq!(header_value(&response, "content-length"), "10");
        assert_eq!(header_value(&response, "accept-ranges"), "bytes");
        assert!(header_value(&response, "etag").starts_with('"'));
        assert!(!header_value(&response, "last-modified").is_empty());
        assert!(header_value(&response, "content-type").starts_with("text/plain"));
        assert_eq!(&collect_body(response).await[..], b"abcdefghij");
    }

    #[tokio::test]
    async fn file_range_gets_206_with_content_range() {
        let (_dir, path) = sample_file();
        let response = file_context("GET", &[("range", "bytes=2-5")], FileBody::path(&path))
            .await
            .finalize()
            .await;

        assert_eq!(response.status(), 206);
        assert_eq!(header_value(&response, "content-range"), "bytes 2-5/10");
        assert_eq!(header_value(&response, "content-length"), "4");
        assert_eq!(&collect_body(response).await[..], b"cdef");
    }

    #[tokio::test]
    async fn file_conditional_get_answers_304() {
        let (_dir, path) = sample_file();
        let etag = {
            let response = file_context("GET", &[], FileBody::path(&path))
                .await
                .finalize()
                .await;
            header_value(&response, "etag")
        };
        assert!(!etag.is_empty());

        let response = file_context(
            "GET",
            &[("if-none-match", etag.as_str())],
            FileBody::path(&path),
        )
        .await
        .finalize()
        .await;

        assert_eq!(response.status(), 304);
        assert!(collect_body(response).await.is_empty());
    }

    #[tokio::test]
    async fn file_head_keeps_content_length_without_a_body() {
        let (_dir, path) = sample_file();
        let response = file_context("HEAD", &[], FileBody::path(&path))
            .await
            .finalize()
            .await;

        assert_eq!(response.status(), 200);
        assert_eq!(header_value(&response, "content-length"), "10");
        assert!(collect_body(response).await.is_empty());
    }

    #[tokio::test]
    async fn file_multi_range_answers_multipart_byteranges() {
        let (_dir, path) = sample_file();
        let response = file_context("GET", &[("range", "bytes=0-1,4-5")], FileBody::path(&path))
            .await
            .finalize()
            .await;

        assert_eq!(response.status(), 206);
        let content_type = header_value(&response, "content-type");
        assert!(
            content_type.starts_with("multipart/byteranges; boundary="),
            "got {content_type}"
        );
        let body = String::from_utf8(collect_body(response).await.to_vec()).expect("utf-8 body");
        assert!(body.contains("Content-Range: bytes 0-1/10"), "{body}");
        assert!(body.contains("Content-Range: bytes 4-5/10"), "{body}");
        assert!(body.contains("\r\n\r\nab\r\n"), "{body}");
        assert!(body.contains("\r\n\r\nef\r\n"), "{body}");
    }

    #[tokio::test]
    async fn missing_file_is_a_problem_response() {
        let dir = tempfile::tempdir().expect("temp dir");
        let missing = dir.path().join("missing.txt");
        let response = file_context("GET", &[], FileBody::path(&missing))
            .await
            .finalize()
            .await;

        assert_eq!(response.status(), 404);
        assert_eq!(
            header_value(&response, "content-type"),
            "application/problem+json"
        );
    }
}

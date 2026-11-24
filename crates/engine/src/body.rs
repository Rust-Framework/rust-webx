//! Request body limits and the shared body helpers of the 2.0 engine.
//!
//! The body stays the live hyper stream until something asks for it:
//!
//! * `read_body` buffers it (enforcing the configured cap) and hands out a
//!   reference-counted `Bytes`, returning the first frame as-is when the whole
//!   body arrived in one frame, so a small JSON body is never copied twice;
//! * `stream_body` exposes it as a stream for proxy/large-upload endpoints;
//! * `parse_multipart` streams it through `multer` into the core
//!   [`MultipartForm`], spooling large file parts to disk with the existing
//!   `FormFileBuilder`.
//!
//! [`BodyLimits`] moved here from the 1.5 host context; the host re-exports it,
//! so `webx_host::BodyLimits` keeps working.

use bytes::{Bytes, BytesMut};
use futures_util::StreamExt;
use std::path::PathBuf;
use webx_core::config::AppOptions;
use webx_core::error::{Error, Result};
use webx_core::form::{FormFileBuilder, MultipartForm};
use webx_core::http::RequestBodyStream;

use crate::transport::{TransportBodyFrame, TransportRequestBodyStream};

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
            spool_dir: webx_core::form::spool_root(),
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
                    webx_core::paths::app_base().join(path)
                }
            }
            _ => webx_core::form::spool_root(),
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
        webx_core::form::set_spool_root(self.spool_dir.clone());
    }
}

/// Buffer a body stream into `Bytes`, enforcing `limit`.
///
/// The common small-body case is one data frame: that frame's `Bytes` is
/// returned directly instead of being copied into a new buffer. Larger bodies
/// are extended into a `BytesMut` sized from what has been seen so far.
///
/// A body whose declared size already exceeds `limit` is rejected before any
/// frame is polled. Chunked bodies have no exact size hint and are bounded while
/// reading; the total is checked before a chunk is appended, so the buffer never
/// grows past the limit by more than the rejected chunk itself.
pub(crate) async fn read_body(
    mut frames: TransportRequestBodyStream,
    exact_len: Option<u64>,
    limit: usize,
) -> Result<Bytes> {
    if let Some(exact) = exact_len {
        if exact > limit as u64 {
            return Err(payload_too_large(limit));
        }
    }

    let mut total = 0usize;
    let mut first: Option<Bytes> = None;
    let mut buffer: Option<BytesMut> = None;

    while let Some(frame) = frames.next().await {
        let frame = frame.map_err(map_body_error)?;
        let chunk = match frame {
            TransportBodyFrame::Data(chunk) => chunk,
            // Trailer frames carry no payload; they are legal and ignored by a
            // buffered reader (the hyper path always behaved this way).
            TransportBodyFrame::Trailers(_) => continue,
        };

        total = total.saturating_add(chunk.len());
        if total > limit {
            return Err(payload_too_large(limit));
        }

        if let Some(buffer) = buffer.as_mut() {
            buffer.extend_from_slice(&chunk);
        } else if let Some(first_frame) = first.take() {
            // Second frame: promote to a growing buffer.
            let mut combined = BytesMut::with_capacity(first_frame.len() + chunk.len());
            combined.extend_from_slice(&first_frame);
            combined.extend_from_slice(&chunk);
            buffer = Some(combined);
        } else {
            first = Some(chunk);
        }
    }

    match (first, buffer) {
        (_, Some(buffer)) => Ok(buffer.freeze()),
        (Some(bytes), None) => Ok(bytes),
        (None, None) => Ok(Bytes::new()),
    }
}

/// Expose a live body as a stream of chunks, enforcing `limit` as it goes.
///
/// Trailer frames are rejected: a proxy endpoint forwards data, and silently
/// dropping trailers would lose information the client encoded.
pub(crate) fn stream_body(frames: TransportRequestBodyStream, limit: usize) -> RequestBodyStream {
    let limit = limit as u64;
    let mut seen = 0u64;
    let stream = frames.map(move |frame| match frame {
        Ok(TransportBodyFrame::Data(bytes)) => {
            seen = seen.saturating_add(bytes.len() as u64);
            if seen > limit {
                return Err(Error::status(413, "request body too large".to_string()));
            }
            Ok(bytes)
        }
        Ok(TransportBodyFrame::Trailers(_)) => Err(Error::bad_request(
            "unexpected trailer frame in request body".to_string(),
        )),
        Err(error) => {
            tracing::debug!(error = %error, "failed to read request body");
            Err(Error::bad_request(format!("request body stream: {error}")))
        }
    });
    Box::pin(stream)
}

fn payload_too_large(limit: usize) -> Error {
    Error::payload_too_large(format!("request body exceeds the {limit}-byte limit"))
}

fn map_body_error(err: impl std::fmt::Display) -> Error {
    // Transport failure while reading the body: the client went away or the
    // framing is broken. Log the details, never serve them.
    tracing::debug!(error = %err, "failed to read request body");
    Error::state("failed to read request body".to_string())
}

/// Whether a raw `Content-Type` header value declares a multipart form.
pub(crate) fn is_multipart_content_type(content_type: Option<&str>) -> bool {
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
pub(crate) fn multipart_boundary(content_type: &str) -> Option<String> {
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
pub(crate) async fn parse_multipart(
    frames: TransportRequestBodyStream,
    boundary: &str,
    limits: &BodyLimits,
) -> Result<MultipartForm> {
    // `multer` consumes a data-only stream; trailer frames are skipped, matching
    // hyper's old `into_data_stream()` behavior.
    let data = frames.filter_map(|frame| async move {
        match frame {
            Ok(TransportBodyFrame::Data(bytes)) => Some(Ok(bytes)),
            Ok(TransportBodyFrame::Trailers(_)) => None,
            Err(error) => Some(Err(error)),
        }
    });
    let mut multipart = multer::Multipart::new(data, boundary);
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
                        return Err(Error::payload_too_large(format!(
                            "form field `{field_name}` exceeds the {}-byte limit",
                            limits.field
                        )));
                    }
                    value.extend_from_slice(&chunk);
                }
                let text = String::from_utf8(value).map_err(|_| {
                    Error::validation(format!("form field `{field_name}` is not valid UTF-8"))
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
        return Err(Error::payload_too_large(format!(
            "multipart request exceeds the {limit}-byte limit"
        )));
    }
    Ok(*total)
}

fn multer_error(err: multer::Error) -> Error {
    match err {
        multer::Error::FieldSizeExceeded { limit, .. } => {
            Error::payload_too_large(format!("multipart field exceeds the {limit}-byte limit"))
        }
        multer::Error::StreamSizeExceeded { limit } => {
            Error::payload_too_large(format!("multipart request exceeds the {limit}-byte limit"))
        }
        multer::Error::IncompleteFieldData { .. } | multer::Error::IncompleteStream => {
            Error::validation("multipart body ended unexpectedly".to_string())
        }
        other => Error::validation(format!("invalid multipart body: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::TransportBodyError;
    use futures_util::stream;
    use http::HeaderMap;

    fn frames(items: Vec<TransportBodyFrame>) -> TransportRequestBodyStream {
        Box::pin(stream::iter(
            items
                .into_iter()
                .map(Ok::<_, TransportBodyError>)
                .collect::<Vec<_>>(),
        ))
    }

    fn data(bytes: &'static [u8]) -> TransportBodyFrame {
        TransportBodyFrame::Data(Bytes::from_static(bytes))
    }

    #[test]
    fn parses_the_boundary_parameter() {
        assert_eq!(
            multipart_boundary("multipart/form-data; boundary=----abc123").as_deref(),
            Some("----abc123")
        );
        assert_eq!(
            multipart_boundary("multipart/form-data; charset=utf-8; boundary=\"quoted\"")
                .as_deref(),
            Some("quoted")
        );
        assert_eq!(multipart_boundary("multipart/form-data"), None);
    }

    #[test]
    fn recognizes_multipart_content_types() {
        assert!(is_multipart_content_type(Some(
            "multipart/form-data; boundary=x"
        )));
        assert!(is_multipart_content_type(Some("Multipart/Form-Data")));
        assert!(!is_multipart_content_type(Some("application/json")));
        assert!(!is_multipart_content_type(None));
    }

    #[tokio::test]
    async fn read_body_returns_a_single_frame_without_copying() {
        let input = Bytes::from_static(b"{\"id\":1}");
        let out = read_body(
            frames(vec![TransportBodyFrame::Data(input.clone())]),
            None,
            1024,
        )
        .await
        .expect("body reads");
        assert_eq!(
            input.as_ptr(),
            out.as_ptr(),
            "a single-frame body must be handed out, not copied again"
        );
    }

    #[tokio::test]
    async fn read_body_joins_multiple_frames() {
        let out = read_body(frames(vec![data(b"ab"), data(b"cd")]), None, 1024)
            .await
            .expect("body reads");
        assert_eq!(&out[..], b"abcd");
    }

    #[tokio::test]
    async fn read_body_ignores_trailer_frames() {
        let out = read_body(
            frames(vec![
                data(b"ab"),
                TransportBodyFrame::Trailers(HeaderMap::new()),
            ]),
            None,
            1024,
        )
        .await
        .expect("body reads");
        assert_eq!(&out[..], b"ab");
    }

    #[tokio::test]
    async fn read_body_enforces_the_limit() {
        let err = read_body(frames(vec![data(b"0123456789")]), None, 4)
            .await
            .expect_err("the body exceeds the limit");
        assert!(err.code() == "payload_too_large");
        assert_eq!(err.status_code(), 413);
    }

    #[tokio::test]
    async fn read_body_accepts_a_body_at_the_limit() {
        let out = read_body(frames(vec![data(b"0123456789")]), None, 10)
            .await
            .expect("body at the limit is accepted");
        assert_eq!(&out[..], b"0123456789");
    }

    #[tokio::test]
    async fn read_body_rejects_a_declared_oversized_body() {
        // `exact_len` is the neutral equivalent of an exact `Content-Length`
        // size hint: the limit is enforced before any poll.
        let err = read_body(frames(vec![data(b"0123456789")]), Some(10), 4)
            .await
            .expect_err("declared size over the limit is rejected");
        assert_eq!(err.status_code(), 413);
    }
}

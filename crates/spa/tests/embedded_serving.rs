//! End-to-end serving of compiled-in assets.
//!
//! These tests drive [`SpaMiddleware`] through a mock context that captures the
//! `FileBody` the middleware hands to the host, so they assert on the bytes and
//! headers a client would actually receive — including the brotli negotiation
//! that keeps a compressed embedded asset from being served to a client that
//! cannot decode it.

use std::collections::HashMap;
use std::io::Write as _;
use std::ops::ControlFlow;

use webx_core::auth::IClaims;
use webx_core::error::Result;
use webx_core::http::{
    FileSource, IClaimsExt, IHttpContext, IHttpRequest, IHttpResponse, ResponseBody,
};
use webx_core::middleware::IMiddleware;
use webx_spa::{ContentEncoding, EmbeddedAsset, EmbeddedAssets, SpaMiddleware, SpaSource};

// ── Fixtures ──

/// A body large and repetitive enough that brotli always shrinks it.
fn compressible() -> String {
    "body { color: red; }\n".repeat(128)
}

fn brotli(source: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = brotli::CompressorWriter::new(&mut out, 4096, 11, 24);
    encoder.write_all(source.as_bytes()).unwrap();
    encoder.flush().unwrap();
    drop(encoder);
    assert!(
        out.len() < source.len(),
        "fixture must actually compress ({} vs {})",
        out.len(),
        source.len()
    );
    out
}

struct Fixture {
    compressed_source: String,
    plain_source: String,
    /// Held only so the directory outlives the test — dropping it deletes the
    /// directory the middleware is still configured to look in.
    #[allow(dead_code)]
    overlay: tempfile::TempDir,
}

/// A table with one brotli-stored asset and one stored verbatim, built the way
/// the build script would build it.
///
/// The disk overlay is an empty temp directory on purpose. The middleware
/// resolves a relative overlay against the process working directory and the
/// app base, which on some platforms reaches the repository's own `wwwroot`
/// (docbit ships an `app.css` there), and a disk hit would silently shadow the
/// embedded table these tests exist to check. An absolute path cannot resolve
/// anywhere else, so the lookup misses on every platform.
fn fixture() -> (Fixture, SpaMiddleware) {
    let compressed_source = compressible();
    let plain_source = "already-compressed-bytes".to_string();

    let payload: &'static [u8] = Box::leak(brotli(&compressed_source).into_boxed_slice());
    let plain: &'static [u8] = Box::leak(plain_source.clone().into_bytes().into_boxed_slice());

    let entries: &'static [EmbeddedAsset] = Box::leak(
        vec![
            EmbeddedAsset {
                path: "app.css",
                bytes: payload,
                raw_len: compressed_source.len(),
                encoding: ContentEncoding::Brotli,
                content_type: "text/css",
                etag: "\"sha256-compressed\"",
            },
            EmbeddedAsset {
                path: "logo.png",
                bytes: plain,
                raw_len: plain_source.len(),
                encoding: ContentEncoding::Identity,
                content_type: "image/png",
                etag: "\"sha256-plain\"",
            },
        ]
        .into_boxed_slice(),
    );

    let overlay = tempfile::tempdir().expect("temp dir for the disk overlay");
    let source = SpaSource::from(EmbeddedAssets::new("wwwroot", entries))
        .with_overlay_root(overlay.path().to_string_lossy().into_owned());
    (
        Fixture {
            compressed_source,
            plain_source,
            overlay,
        },
        SpaMiddleware::from_source(source),
    )
}

// ── Mock HTTP context ──

struct Ctx {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    query: HashMap<String, String>,
    route_params: HashMap<String, String>,
    route_pattern: Option<String>,
    response: Resp,
    claims: Option<Box<dyn IClaims>>,
}

impl Ctx {
    fn get(path: &str) -> Self {
        Self {
            method: "GET".into(),
            path: path.into(),
            headers: HashMap::new(),
            query: HashMap::new(),
            route_params: HashMap::new(),
            route_pattern: None,
            response: Resp::default(),
            claims: None,
        }
    }

    fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.insert(name.to_lowercase(), value.into());
        self
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.response
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn take_body(&mut self) -> ResponseBody {
        self.response.body.take().expect("a body was written")
    }
}

impl IClaimsExt for Ctx {
    fn set_claims(&mut self, claims: Box<dyn IClaims>) {
        self.claims = Some(claims);
    }
    fn claims(&self) -> Option<&dyn IClaims> {
        self.claims.as_deref()
    }
}

impl IHttpContext for Ctx {
    fn request(&self) -> &dyn IHttpRequest {
        self
    }
    fn request_mut(&mut self) -> &mut dyn IHttpRequest {
        self
    }
    fn response(&self) -> &dyn IHttpResponse {
        &self.response
    }
    fn response_mut(&mut self) -> &mut dyn IHttpResponse {
        &mut self.response
    }
}

#[async_trait::async_trait]
impl IHttpRequest for Ctx {
    fn method(&self) -> &str {
        &self.method
    }
    fn path(&self) -> &str {
        &self.path
    }
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(&name.to_lowercase()).map(String::as_str)
    }
    fn query(&self) -> &HashMap<String, String> {
        &self.query
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
        Ok(Vec::new())
    }
}

#[derive(Default)]
struct Resp {
    status: u16,
    headers: Vec<(String, String)>,
    body: Option<ResponseBody>,
}

#[async_trait::async_trait]
impl IHttpResponse for Resp {
    fn status(&self) -> u16 {
        self.status
    }
    fn set_status(&mut self, code: u16) {
        self.status = code;
    }
    fn set_header(&mut self, key: &str, value: &str) {
        self.headers.push((key.into(), value.into()));
    }
    async fn write_bytes(&mut self, data: Vec<u8>) -> Result<()> {
        self.body = Some(ResponseBody::Bytes(data));
        Ok(())
    }
    /// Capture the file body instead of rejecting it, so the test can read the
    /// exact bytes and metadata the real host would turn into a response.
    async fn write_body(&mut self, body: ResponseBody) -> Result<()> {
        self.body = Some(body);
        Ok(())
    }
}

/// Read a captured file body's bytes, the way the host streams them.
async fn drain(body: ResponseBody) -> Vec<u8> {
    match body {
        ResponseBody::Bytes(bytes) => bytes,
        ResponseBody::File(file) => match file.source {
            FileSource::Path(path) => std::fs::read(path).unwrap(),
            FileSource::SeekableStream { mut reader, len } => {
                use tokio::io::AsyncReadExt as _;
                let mut buf = Vec::with_capacity(len as usize);
                reader.read_to_end(&mut buf).await.unwrap();
                buf
            }
            FileSource::Stream(_) => panic!("embedded assets are always seekable"),
        },
    }
}

/// What a client would receive, assembled the way the host would assemble it.
struct Served {
    status: u16,
    content_encoding: Option<String>,
    vary: Option<String>,
    cache_control: Option<String>,
    content_type: Option<String>,
    etag: Option<String>,
    bytes: Vec<u8>,
}

impl Served {
    fn len(&self) -> usize {
        self.bytes.len()
    }
}

/// Serve one request, then read the response the way the host would.
///
/// `content-type` and `ETag` travel on the [`FileBody`] — the host turns them
/// into headers — so they are read from there rather than from the response.
async fn serve(mw: &SpaMiddleware, accept: Option<&str>, path: &str) -> Served {
    let mut ctx = Ctx::get(path);
    if let Some(accept) = accept {
        ctx = ctx.with_header("accept-encoding", accept);
    }
    let flow = mw.invoke(&mut ctx).await.expect("serving must succeed");
    assert!(
        matches!(flow, ControlFlow::Continue(())),
        "static responses continue the pipeline"
    );

    let status = ctx.response.status;
    let body = ctx.take_body();
    let (content_type, etag, length) = match &body {
        ResponseBody::Bytes(_) => (None, None, None),
        ResponseBody::File(file) => (
            file.content_type.clone(),
            file.entity_tag.clone(),
            match &file.source {
                FileSource::Path(p) => std::fs::metadata(p).ok().map(|m| m.len()),
                FileSource::SeekableStream { len, .. } => Some(*len),
                FileSource::Stream(_) => None,
            },
        ),
    };

    let bytes = drain(body).await;
    if let Some(length) = length {
        assert_eq!(
            length as usize,
            bytes.len(),
            "Content-Length must match the bytes sent"
        );
    }

    Served {
        status,
        content_encoding: ctx.header("content-encoding").map(str::to_string),
        vary: ctx.header("vary").map(str::to_string),
        cache_control: ctx.header("cache-control").map(str::to_string),
        content_type,
        etag,
        bytes,
    }
}

fn identity_etag() -> &'static str {
    "\"sha256-compressed\""
}

// ── Tests ──

#[tokio::test]
async fn a_client_that_accepts_brotli_gets_the_stored_stream_verbatim() {
    let (f, mw) = fixture();
    let served = serve(&mw, Some("gzip, br"), "/app.css").await;

    assert_eq!(served.status, 200);
    assert_eq!(served.content_encoding.as_deref(), Some("br"));
    assert_eq!(served.content_type.as_deref(), Some("text/css"));
    assert_eq!(served.vary.as_deref(), Some("accept-encoding"));
    assert!(served.cache_control.is_some(), "cache policy must be set");
    // A compressed representation needs its own validator.
    assert_eq!(served.etag.as_deref(), Some("\"sha256-compressed-br\""));

    assert!(
        served.len() < f.compressed_source.len(),
        "the compressed representation must be smaller"
    );
    // No decode happened on this path: the bytes are the stored stream.
    assert_eq!(served.bytes, brotli(&f.compressed_source));
}

#[tokio::test]
async fn a_client_without_brotli_gets_the_decoded_file() {
    let (f, mw) = fixture();
    let served = serve(&mw, Some("gzip, deflate"), "/app.css").await;

    assert_eq!(served.status, 200);
    assert_eq!(served.content_encoding, None);
    assert_eq!(served.vary.as_deref(), Some("accept-encoding"));
    assert_eq!(served.content_type.as_deref(), Some("text/css"));
    assert_eq!(served.etag.as_deref(), Some(identity_etag()));
    assert_eq!(
        served.bytes,
        f.compressed_source.as_bytes(),
        "the decoded bytes must be the original file"
    );
}

#[tokio::test]
async fn a_zero_quality_request_for_brotli_falls_back_to_the_decoded_file() {
    let (f, mw) = fixture();
    let served = serve(&mw, Some("br;q=0, gzip"), "/app.css").await;

    assert_eq!(served.content_encoding, None);
    assert_eq!(served.etag.as_deref(), Some(identity_etag()));
    assert_eq!(served.bytes, f.compressed_source.as_bytes());
}

#[tokio::test]
async fn a_wildcard_client_is_served_the_compressed_stream() {
    let (f, mw) = fixture();
    let served = serve(&mw, Some("*"), "/app.css").await;

    assert_eq!(served.content_encoding.as_deref(), Some("br"));
    assert_eq!(served.bytes, brotli(&f.compressed_source));
}

#[tokio::test]
async fn a_verbatim_asset_is_never_announced_as_encoded() {
    let (f, mw) = fixture();
    for accept in ["gzip, br", "identity", "*"] {
        let served = serve(&mw, Some(accept), "/logo.png").await;

        assert_eq!(served.status, 200, "accept-encoding: {accept}");
        assert_eq!(
            served.content_encoding, None,
            "an identity payload must not claim an encoding (accept-encoding: {accept})"
        );
        assert_eq!(served.content_type.as_deref(), Some("image/png"));
        assert_eq!(served.etag.as_deref(), Some("\"sha256-plain\""));
        // A verbatim payload is identical whichever encodings the client offers,
        // so it must not advertise `Vary`.
        assert_eq!(served.vary, None, "accept-encoding: {accept}");
        assert_eq!(served.bytes, f.plain_source.as_bytes());
    }
}

#[tokio::test]
async fn encoded_and_decoded_requests_agree_on_content_but_not_on_validators() {
    let (f, mw) = fixture();

    let encoded = serve(&mw, Some("br"), "/app.css").await;
    let decoded = serve(&mw, Some("gzip"), "/app.css").await;

    // Different representations, different validators — so a shared cache can
    // never hand one representation's bytes to a client expecting the other.
    assert_ne!(encoded.etag, decoded.etag);
    assert!(encoded.etag.as_deref().unwrap().ends_with("-br\""));
    // The compressed stream still decodes to exactly what the other path served.
    assert_eq!(encoded.bytes, brotli(&f.compressed_source));
    assert_eq!(decoded.bytes, f.compressed_source.as_bytes());
}

#[tokio::test]
async fn the_decoded_fallback_is_reused_across_requests() {
    let (_, mw) = fixture();
    let first = serve(&mw, Some("gzip"), "/app.css").await;
    let second = serve(&mw, Some("gzip"), "/app.css").await;
    assert_eq!(first.bytes, second.bytes);
}

#[tokio::test]
async fn a_missing_file_still_falls_back_to_the_embedded_index() {
    let (_, mw) = fixture();
    // No index.html in this fixture, and no disk root, so the router decides.
    let flow = mw
        .invoke(&mut Ctx::get("/nothing/here"))
        .await
        .expect("must not error");
    assert!(matches!(flow, ControlFlow::Continue(())));
}

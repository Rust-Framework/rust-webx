//! End-to-end tests for the file upload / download infrastructure.
//!
//! Covers, over real HTTP:
//!
//! * `multipart/form-data` binding into a typed request struct (`FormFile`),
//! * memory vs. disk spooling of uploaded parts,
//! * `ResponseData::file` downloads with `Content-Disposition`, `ETag`,
//!   `Last-Modified`, `Range` and conditional requests,
//! * `HEAD`,
//! * upload limits (`Form.MaxFileSize`, `Form.MaxRequestSize`, `Form.MaxFieldSize`).

use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use rust_webx::*;
use serde::{Deserialize, Serialize};

fn find_free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Build a host with `configure`, run it, and return its base URL.
///
/// Retries on a fresh port when another parallel test wins the race for the
/// ephemeral port between `find_free_port` and the server's `bind`, so the
/// suite stays parallel without flaking.
async fn spawn(configure: impl Fn(&mut HostAppBuilder) + Send + Sync + 'static) -> String {
    let configure = std::sync::Arc::new(configure);

    for _ in 0..3 {
        let port = find_free_port();
        let addr = format!("127.0.0.1:{port}");
        let base = format!("http://{addr}");

        let configure = std::sync::Arc::clone(&configure);
        let host = Host::builder()
            .mode(AppMode::Development)
            .no_spa()
            .configure(move |builder| configure(builder))
            .build();
        tokio::spawn(async move {
            let _ = host.run_at(&addr).await;
        });

        let client = reqwest::Client::new();
        for _ in 0..100 {
            if let Ok(resp) = client.get(format!("{base}/health/live")).send().await {
                if resp.status().is_success() {
                    return base;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    panic!("test host never became ready after retrying several ports");
}

// ---------------------------------------------------------------------------
// Routes under test
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, WebxRequestMeta)]
struct UploadRequest {
    title: String,
    confirm: bool,
    avatar: FormFile,
    #[serde(default)]
    gallery: Vec<FormFile>,
}

#[derive(Debug, Serialize, Deserialize)]
struct UploadSummary {
    title: String,
    confirm: bool,
    file_name: String,
    content_type: Option<String>,
    size: u64,
    spooled: bool,
    gallery_len: usize,
    content: String,
}

#[post("/test/upload")]
impl IRequest<UploadSummary> for UploadRequest {}

#[derive(Default)]
struct UploadHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<UploadRequest, UploadSummary> for UploadHandler {
    async fn handle(&mut self, req: UploadRequest) -> Result<UploadSummary> {
        let bytes = req.avatar.read_bytes().await?;
        Ok(UploadSummary {
            title: req.title,
            confirm: req.confirm,
            file_name: req.avatar.file_name().to_string(),
            content_type: req.avatar.content_type().map(str::to_string),
            size: req.avatar.size(),
            spooled: req.avatar.path().is_some(),
            gallery_len: req.gallery.len(),
            content: String::from_utf8_lossy(&bytes).into_owned(),
        })
    }
}

static DOWNLOAD_DIR: OnceLock<PathBuf> = OnceLock::new();

#[derive(Debug, Default, Deserialize)]
struct DownloadRequest {
    name: String,
}

#[get("/test/download/{name}")]
impl IRequest<ResponseData> for DownloadRequest {}

#[derive(Default)]
struct DownloadHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<DownloadRequest, ResponseData> for DownloadHandler {
    async fn handle(&mut self, req: DownloadRequest) -> Result<ResponseData> {
        let dir = DOWNLOAD_DIR.get().expect("download dir is initialised");
        Ok(ResponseData::file(dir.join(&req.name)).download_name("report.csv"))
    }
}

/// A download whose file does not exist, to check the error path.
#[derive(Debug, Default, Deserialize)]
struct MissingDownloadRequest;

#[get("/test/download-missing")]
impl IRequest<ResponseData> for MissingDownloadRequest {}

#[derive(Default)]
struct MissingDownloadHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<MissingDownloadRequest, ResponseData> for MissingDownloadHandler {
    async fn handle(&mut self, _req: MissingDownloadRequest) -> Result<ResponseData> {
        let dir = DOWNLOAD_DIR.get().expect("download dir is initialised");
        Ok(ResponseData::file(dir.join("does-not-exist.csv")))
    }
}

/// An in-memory body with a download name, to check `Content-Disposition`
/// on non-file responses.
#[derive(Debug, Default, Deserialize)]
struct InlineCsvRequest;

#[get("/test/inline-csv")]
impl IRequest<ResponseData> for InlineCsvRequest {}

#[derive(Default)]
struct InlineCsvHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<InlineCsvRequest, ResponseData> for InlineCsvHandler {
    async fn handle(&mut self, _req: InlineCsvRequest) -> Result<ResponseData> {
        Ok(ResponseData::text("a,b\n1,2\n")
            .content_type("text/csv; charset=utf-8")
            .download_name("导出.csv"))
    }
}

/// A response streamed from a reader of unknown length (`File(Stream, ...)`).
#[derive(Debug, Default, Deserialize)]
struct StreamDownloadRequest;

#[get("/test/download-stream")]
impl IRequest<ResponseData> for StreamDownloadRequest {}

#[derive(Default)]
struct StreamDownloadHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<StreamDownloadRequest, ResponseData> for StreamDownloadHandler {
    async fn handle(&mut self, _req: StreamDownloadRequest) -> Result<ResponseData> {
        // A live producer: nothing is buffered, so the total size is unknown.
        let (reader, mut writer) = tokio::io::duplex(1024);
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            for i in 0..4 {
                let _ = writer.write_all(format!("chunk-{i}\n").as_bytes()).await;
            }
        });

        Ok(ResponseData::file_stream(reader, "text/plain")
            .download_name("stream.txt")
            .header("x-produced-by", "test"))
    }
}

/// A response streamed from a reader that knows its length and can seek
/// (`File(Stream, ...)` with range processing enabled).
#[derive(Debug, Default, Deserialize)]
struct SeekableDownloadRequest;

#[get("/test/download-seekable")]
impl IRequest<ResponseData> for SeekableDownloadRequest {}

/// The same seekable source, but with `Range` processing switched off.
#[derive(Debug, Default, Deserialize)]
struct NonRangeDownloadRequest;

#[get("/test/download-nonrange")]
impl IRequest<ResponseData> for NonRangeDownloadRequest {}

const SEEKABLE_BODY: &[u8] = b"0123456789abcdef";

#[derive(Default)]
struct SeekableDownloadHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<SeekableDownloadRequest, ResponseData> for SeekableDownloadHandler {
    async fn handle(&mut self, _req: SeekableDownloadRequest) -> Result<ResponseData> {
        // `Cursor<Vec<u8>>` is the simplest seekable reader; object-storage and
        // database LOB handles behave the same way.
        let cursor = std::io::Cursor::new(SEEKABLE_BODY.to_vec());
        Ok(ResponseData::with_file(
            FileBody::seekable_stream(cursor, SEEKABLE_BODY.len() as u64)
                .content_type("application/octet-stream")
                .entity_tag("\"seekable-v1\"")
                .download_name("seekable.bin"),
        ))
    }
}

#[derive(Default)]
struct NonRangeDownloadHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<NonRangeDownloadRequest, ResponseData> for NonRangeDownloadHandler {
    async fn handle(&mut self, _req: NonRangeDownloadRequest) -> Result<ResponseData> {
        let cursor = std::io::Cursor::new(SEEKABLE_BODY.to_vec());
        Ok(ResponseData::with_file(
            FileBody::seekable_stream(cursor, SEEKABLE_BODY.len() as u64)
                .content_type("application/octet-stream")
                .enable_range_processing(false),
        ))
    }
}

/// Deterministic content for the large download used by the resume tests.
const BIG_LEN: usize = 64 * 1024 + 7;

fn big_body() -> Vec<u8> {
    (0..BIG_LEN).map(|index| (index % 251) as u8).collect()
}

fn download_dir() -> PathBuf {
    DOWNLOAD_DIR
        .get_or_init(|| {
            let dir = std::env::temp_dir().join(format!("webx-download-test-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("data.csv"), b"col_a,col_b\n1,2\n").unwrap();
            std::fs::write(dir.join("big.bin"), big_body()).unwrap();
            dir
        })
        .clone()
}

// ---------------------------------------------------------------------------
// Upload
// ---------------------------------------------------------------------------

const SMALL_CSV: &[u8] = b"col_a,col_b\n1,2\n";

#[tokio::test]
async fn multipart_upload_binds_text_and_file_fields() {
    let base = spawn(|_| {}).await;
    let client = reqwest::Client::new();

    let form = reqwest::multipart::Form::new()
        .text("title", "quarterly")
        .text("confirm", "true")
        .part(
            "avatar",
            reqwest::multipart::Part::bytes(SMALL_CSV.to_vec())
                .file_name("../unsafe/data.csv")
                .mime_str("text/csv")
                .unwrap(),
        );

    let resp = client
        .post(format!("{base}/test/upload"))
        .multipart(form)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200, "upload should succeed");
    let summary: UploadSummary = resp.json().await.unwrap();

    assert_eq!(summary.title, "quarterly");
    assert!(summary.confirm);
    // The client-supplied path is reduced to a safe basename.
    assert_eq!(summary.file_name, "data.csv");
    assert_eq!(summary.content_type.as_deref(), Some("text/csv"));
    assert_eq!(summary.size, SMALL_CSV.len() as u64);
    assert_eq!(summary.content, "col_a,col_b\n1,2\n");
    assert!(!summary.spooled, "a 14-byte part should stay in memory");
}

#[tokio::test]
async fn multipart_upload_binds_repeated_file_parts() {
    let base = spawn(|_| {}).await;
    let client = reqwest::Client::new();

    let form = reqwest::multipart::Form::new()
        .text("title", "gallery")
        .text("confirm", "false")
        .part(
            "avatar",
            reqwest::multipart::Part::bytes(b"primary".to_vec()).file_name("a.txt"),
        )
        .part(
            "gallery",
            reqwest::multipart::Part::bytes(b"one".to_vec()).file_name("g1.txt"),
        )
        .part(
            "gallery",
            reqwest::multipart::Part::bytes(b"two".to_vec()).file_name("g2.txt"),
        );

    let resp = client
        .post(format!("{base}/test/upload"))
        .multipart(form)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    let summary: UploadSummary = resp.json().await.unwrap();
    assert_eq!(summary.gallery_len, 2);
    assert!(!summary.confirm);
    assert_eq!(summary.content, "primary");
}

#[tokio::test]
async fn large_upload_is_spooled_to_disk_and_still_readable() {
    let spool = std::env::temp_dir().join(format!("webx-spool-test-{}", std::process::id()));
    let spool_for_config = spool.clone();
    let base = spawn(move |builder| {
        let dir = spool_for_config.to_string_lossy().into_owned();
        builder.useOptions(move |o| {
            o.form.memory_threshold = 16;
            o.form.temp_dir = Some(dir);
        });
    })
    .await;

    let payload = vec![b'z'; 4096];
    let form = reqwest::multipart::Form::new()
        .text("title", "big")
        .text("confirm", "true")
        .part(
            "avatar",
            reqwest::multipart::Part::bytes(payload.clone()).file_name("big.bin"),
        );

    let resp = reqwest::Client::new()
        .post(format!("{base}/test/upload"))
        .multipart(form)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    let summary: UploadSummary = resp.json().await.unwrap();
    assert_eq!(summary.size, 4096);
    assert!(summary.spooled, "a 4 KiB part with a 16-byte threshold must spool");
    assert_eq!(summary.content.len(), 4096);
}

#[tokio::test]
async fn json_body_cannot_bind_a_file_field() {
    let base = spawn(|_| {}).await;

    let resp = reqwest::Client::new()
        .post(format!("{base}/test/upload"))
        .json(&serde_json::json!({
            "title": "x",
            "confirm": true,
            "avatar": { "path": "/etc/passwd" }
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 400, "JSON must not satisfy a FormFile");
}

#[tokio::test]
async fn missing_required_form_field_reports_400() {
    let base = spawn(|_| {}).await;

    let form = reqwest::multipart::Form::new().text("title", "no confirm field");
    let resp = reqwest::Client::new()
        .post(format!("{base}/test/upload"))
        .multipart(form)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 400);
    let problem: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(problem["status"], 400);
}

// ---------------------------------------------------------------------------
// Upload limits
// ---------------------------------------------------------------------------

#[tokio::test]
async fn oversized_file_part_is_rejected_with_413() {
    let base = spawn(|builder| {
        builder.useOptions(|o| {
            o.form.max_file_size = 100;
            o.form.max_request_size = 1024 * 1024;
        });
    })
    .await;

    let form = reqwest::multipart::Form::new()
        .text("title", "too big")
        .text("confirm", "true")
        .part(
            "avatar",
            reqwest::multipart::Part::bytes(vec![b'x'; 4096]).file_name("big.bin"),
        );

    let resp = reqwest::Client::new()
        .post(format!("{base}/test/upload"))
        .multipart(form)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 413);
    let problem: serde_json::Value = resp.json().await.unwrap();
    let detail = problem["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("limit"), "unexpected detail: {detail}");
}

#[tokio::test]
async fn declared_content_length_above_the_limit_is_rejected_before_upload() {
    let base = spawn(|builder| {
        builder.useOptions(|o| {
            o.form.max_request_size = 256;
        });
    })
    .await;

    let form = reqwest::multipart::Form::new()
        .text("title", "huge")
        .text("confirm", "true")
        .part(
            "avatar",
            reqwest::multipart::Part::bytes(vec![b'x'; 4096]).file_name("big.bin"),
        );

    let resp = reqwest::Client::new()
        .post(format!("{base}/test/upload"))
        .multipart(form)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 413);
    let problem: serde_json::Value = resp.json().await.unwrap();
    let detail = problem["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains("request body"),
        "the declared length should be refused up front, got: {detail}"
    );
}

#[tokio::test]
async fn oversized_text_field_is_rejected_with_413() {
    let base = spawn(|builder| {
        builder.useOptions(|o| {
            o.form.max_field_size = 32;
            o.form.max_request_size = 1024 * 1024;
        });
    })
    .await;

    let form = reqwest::multipart::Form::new()
        .text("title", "t".repeat(512))
        .text("confirm", "true")
        .part(
            "avatar",
            reqwest::multipart::Part::bytes(SMALL_CSV.to_vec()).file_name("data.csv"),
        );

    let resp = reqwest::Client::new()
        .post(format!("{base}/test/upload"))
        .multipart(form)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 413);
}

// ---------------------------------------------------------------------------
// Download
// ---------------------------------------------------------------------------

#[tokio::test]
async fn file_download_sets_headers_and_streams_the_body() {
    let _ = download_dir();
    let base = spawn(|_| {}).await;

    let resp = reqwest::Client::new()
        .get(format!("{base}/test/download/data.csv"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    let headers = resp.headers().clone();
    assert!(
        headers
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap()
            .contains("csv"),
        "content type should be inferred from the extension"
    );
    let disposition = headers.get("content-disposition").unwrap().to_str().unwrap();
    assert!(disposition.starts_with("attachment"), "got {disposition}");
    assert!(disposition.contains("report.csv"), "got {disposition}");
    assert_eq!(
        headers.get("content-length").unwrap().to_str().unwrap(),
        "16"
    );
    assert_eq!(
        headers.get("accept-ranges").unwrap().to_str().unwrap(),
        "bytes"
    );
    assert!(headers.contains_key("etag"));
    assert!(headers.contains_key("last-modified"));

    assert_eq!(resp.text().await.unwrap(), "col_a,col_b\n1,2\n");
}

#[tokio::test]
async fn range_request_returns_partial_content() {
    let _ = download_dir();
    let base = spawn(|_| {}).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{base}/test/download/data.csv"))
        .header("range", "bytes=0-4")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 206);
    let headers = resp.headers().clone();
    assert_eq!(
        headers.get("content-range").unwrap().to_str().unwrap(),
        "bytes 0-4/16"
    );
    assert_eq!(headers.get("content-length").unwrap().to_str().unwrap(), "5");
    assert_eq!(resp.text().await.unwrap(), "col_a");

    // Suffix range: the last 4 bytes.
    let resp = client
        .get(format!("{base}/test/download/data.csv"))
        .header("range", "bytes=-4")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 206);
    assert_eq!(
        resp.headers()
            .get("content-range")
            .unwrap()
            .to_str()
            .unwrap(),
        "bytes 12-15/16"
    );
    assert_eq!(resp.text().await.unwrap(), "1,2\n");
}

#[tokio::test]
async fn unsatisfiable_range_returns_416_with_content_range() {
    let _ = download_dir();
    let base = spawn(|_| {}).await;

    let resp = reqwest::Client::new()
        .get(format!("{base}/test/download/data.csv"))
        .header("range", "bytes=500-600")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 416);
    assert_eq!(
        resp.headers()
            .get("content-range")
            .unwrap()
            .to_str()
            .unwrap(),
        "bytes */16"
    );
}

#[tokio::test]
async fn conditional_get_returns_304() {
    let _ = download_dir();
    let base = spawn(|_| {}).await;
    let client = reqwest::Client::new();
    let url = format!("{base}/test/download/data.csv");

    let first = client.get(&url).send().await.unwrap();
    let etag = first
        .headers()
        .get("etag")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    let second = client
        .get(&url)
        .header("if-none-match", &etag)
        .send()
        .await
        .unwrap();
    assert_eq!(second.status().as_u16(), 304);
    assert_eq!(second.text().await.unwrap(), "");

    // A stale validator must still produce the full body.
    let third = client
        .get(&url)
        .header("if-none-match", "\"stale\"")
        .send()
        .await
        .unwrap();
    assert_eq!(third.status().as_u16(), 200);
    assert_eq!(third.text().await.unwrap(), "col_a,col_b\n1,2\n");
}

#[tokio::test]
async fn head_request_reports_headers_without_a_body() {
    let _ = download_dir();
    let base = spawn(|_| {}).await;

    let resp = reqwest::Client::new()
        .head(format!("{base}/test/download/data.csv"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(
        resp.headers()
            .get("content-length")
            .unwrap()
            .to_str()
            .unwrap(),
        "16"
    );
    assert_eq!(resp.bytes().await.unwrap().len(), 0);
}

#[tokio::test]
async fn missing_download_target_returns_404() {
    let _ = download_dir();
    let base = spawn(|_| {}).await;

    let resp = reqwest::Client::new()
        .get(format!("{base}/test/download-missing"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 404);
}

#[tokio::test]
async fn in_memory_body_can_be_downloaded_with_a_unicode_name() {
    let base = spawn(|_| {}).await;

    let resp = reqwest::Client::new()
        .get(format!("{base}/test/inline-csv"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    let disposition = resp
        .headers()
        .get("content-disposition")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(disposition.starts_with("attachment"), "got {disposition}");
    // RFC 5987 form carries the real (non-ASCII) name.
    assert!(
        disposition.contains("filename*=UTF-8''"),
        "got {disposition}"
    );
    assert_eq!(resp.text().await.unwrap(), "a,b\n1,2\n");
}

// ---------------------------------------------------------------------------
// Resumable downloads (断点续传)
// ---------------------------------------------------------------------------

/// A download manager walks the file in chunks with `Range` + `If-Range` and
/// must end up with byte-identical content.
#[tokio::test]
async fn resumable_download_reassembles_the_file() {
    let body = big_body();
    let base = spawn(|_| {}).await;
    let client = reqwest::Client::new();
    let url = format!("{base}/test/download/big.bin");

    // 1. Probe with HEAD to learn the size and the validator.
    let probe = client.head(&url).send().await.unwrap();
    assert_eq!(probe.status().as_u16(), 200);
    assert_eq!(
        probe
            .headers()
            .get("content-length")
            .unwrap()
            .to_str()
            .unwrap(),
        BIG_LEN.to_string()
    );
    assert_eq!(
        probe
            .headers()
            .get("accept-ranges")
            .unwrap()
            .to_str()
            .unwrap(),
        "bytes"
    );
    let etag = probe
        .headers()
        .get("etag")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    // 2. Resume in 16 KiB steps.
    const STEP: usize = 16 * 1024;
    let mut received: Vec<u8> = Vec::new();
    while received.len() < body.len() {
        let start = received.len();
        let end = (start + STEP - 1).min(body.len() - 1);

        let resp = client
            .get(&url)
            .header("range", format!("bytes={start}-{end}"))
            .header("if-range", &etag)
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status().as_u16(), 206, "resume at {start} must be partial");
        assert_eq!(
            resp.headers()
                .get("content-range")
                .unwrap()
                .to_str()
                .unwrap(),
            format!("bytes {start}-{end}/{BIG_LEN}")
        );
        assert_eq!(
            resp.headers()
                .get("content-length")
                .unwrap()
                .to_str()
                .unwrap(),
            (end - start + 1).to_string()
        );
        received.extend_from_slice(&resp.bytes().await.unwrap());
    }

    assert_eq!(received.len(), body.len());
    assert!(received == body, "resumed download differs from the original");
}

#[tokio::test]
async fn if_range_with_a_stale_validator_restarts_from_the_whole_file() {
    let _ = download_dir();
    let base = spawn(|_| {}).await;
    let url = format!("{base}/test/download/data.csv");

    let resp = reqwest::Client::new()
        .get(&url)
        .header("range", "bytes=0-4")
        .header("if-range", "\"stale-validator\"")
        .send()
        .await
        .unwrap();

    // Resuming from a stale copy would splice two versions together, so the
    // correct answer is the full representation.
    assert_eq!(resp.status().as_u16(), 200);
    assert!(resp.headers().get("content-range").is_none());
    assert_eq!(resp.text().await.unwrap(), "col_a,col_b\n1,2\n");
}

#[tokio::test]
async fn if_range_rejects_weak_validators() {
    let _ = download_dir();
    let base = spawn(|_| {}).await;
    let url = format!("{base}/test/download/data.csv");

    let resp = reqwest::Client::new()
        .get(&url)
        .header("range", "bytes=0-4")
        // RFC 7233 §3.2 requires a *strong* comparison for If-Range.
        .header("if-range", "W/\"4eb-1\"")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(resp.text().await.unwrap(), "col_a,col_b\n1,2\n");
}

#[tokio::test]
async fn if_range_with_a_matching_validator_serves_the_range() {
    let _ = download_dir();
    let base = spawn(|_| {}).await;
    let client = reqwest::Client::new();
    let url = format!("{base}/test/download/data.csv");

    let etag = client
        .head(&url)
        .send()
        .await
        .unwrap()
        .headers()
        .get("etag")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    let resp = client
        .get(&url)
        .header("range", "bytes=12-15")
        .header("if-range", &etag)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 206);
    assert_eq!(resp.text().await.unwrap(), "1,2\n");

    // A date-based If-Range works too, at second granularity.
    let last_modified = client
        .head(&url)
        .send()
        .await
        .unwrap()
        .headers()
        .get("last-modified")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let resp = client
        .get(&url)
        .header("range", "bytes=12-15")
        .header("if-range", &last_modified)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 206);
}

#[tokio::test]
async fn multi_range_returns_multipart_byteranges() {
    let body = big_body();
    let _ = download_dir();
    let base = spawn(|_| {}).await;

    let resp = reqwest::Client::new()
        .get(format!("{base}/test/download/big.bin"))
        .header("range", "bytes=0-9,20-29")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 206);
    let headers = resp.headers().clone();
    let content_type = headers
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let boundary = content_type
        .strip_prefix("multipart/byteranges; boundary=")
        .unwrap_or_else(|| panic!("unexpected content type: {content_type}"))
        .to_string();
    // A message-level Content-Range would be meaningless for multiple parts.
    assert!(headers.get("content-range").is_none());

    // The declared length must match the bytes actually sent, or the client
    // will hang waiting for the rest.
    let declared: usize = headers
        .get("content-length")
        .unwrap()
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    let bytes = resp.bytes().await.unwrap();
    assert_eq!(declared, bytes.len());

    let expected = [
        format!("--{boundary}\r\nContent-Type: application/octet-stream\r\nContent-Range: bytes 0-9/{BIG_LEN}\r\n\r\n"),
        String::from_utf8_lossy(&body[0..10]).into_owned(),
        "\r\n".to_string(),
        format!("--{boundary}\r\nContent-Type: application/octet-stream\r\nContent-Range: bytes 20-29/{BIG_LEN}\r\n\r\n"),
        String::from_utf8_lossy(&body[20..30]).into_owned(),
        "\r\n".to_string(),
        format!("--{boundary}--\r\n"),
    ]
    .concat();

    assert_eq!(
        bytes.as_ref(),
        expected.as_bytes(),
        "multipart/byteranges layout is wrong"
    );
}

#[tokio::test]
async fn duplicate_ranges_are_coalesced_into_one_part() {
    let _ = download_dir();
    let base = spawn(|_| {}).await;

    let resp = reqwest::Client::new()
        .get(format!("{base}/test/download/data.csv"))
        .header("range", "bytes=0-4,0-4,0-4,0-4")
        .send()
        .await
        .unwrap();

    // Coalescing is what stops `Range` from amplifying traffic: four copies of
    // the same range must collapse into one.
    assert_eq!(resp.status().as_u16(), 206);
    assert_eq!(
        resp.headers()
            .get("content-range")
            .unwrap()
            .to_str()
            .unwrap(),
        "bytes 0-4/16"
    );
    assert_eq!(resp.text().await.unwrap(), "col_a");
}

#[tokio::test]
async fn excessive_range_count_falls_back_to_the_whole_file() {
    let body = big_body();
    let _ = download_dir();
    let base = spawn(|_| {}).await;

    // Nine disjoint parts is past the cap; all of them are satisfiable, so the
    // request is refused rather than served with framing overhead.
    let spec = (0..9)
        .map(|index| format!("{}-{}", index * 4, index * 4 + 1))
        .collect::<Vec<_>>()
        .join(",");

    let resp = reqwest::Client::new()
        .get(format!("{base}/test/download/big.bin"))
        .header("range", format!("bytes={spec}"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    assert!(resp.headers().get("content-range").is_none());
    assert_eq!(resp.bytes().await.unwrap().len(), body.len());
}

#[tokio::test]
async fn unsatisfiable_members_are_dropped_from_a_range_set() {
    let _ = download_dir();
    let base = spawn(|_| {}).await;

    // Only the first member overlaps the representation; the rest must not turn
    // the request into a 416.
    let resp = reqwest::Client::new()
        .get(format!("{base}/test/download/data.csv"))
        .header("range", "bytes=0-3,9000-9001")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 206);
    assert_eq!(
        resp.headers()
            .get("content-range")
            .unwrap()
            .to_str()
            .unwrap(),
        "bytes 0-3/16"
    );
    assert_eq!(resp.text().await.unwrap(), "col_");
}

#[tokio::test]
async fn multi_range_on_a_seekable_stream_serves_the_whole_body() {
    // A seekable *stream* cannot be restarted for several offsets, so the
    // ranges are ignored rather than answered with a truncated body.
    let base = spawn(|_| {}).await;

    let resp = reqwest::Client::new()
        .get(format!("{base}/test/download-seekable"))
        .header("range", "bytes=0-1,4-5")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(resp.text().await.unwrap(), "0123456789abcdef");
}

// ---------------------------------------------------------------------------
// OpenAPI
// ---------------------------------------------------------------------------

#[tokio::test]
async fn openapi_documents_the_upload_endpoint_as_multipart() {
    let base = spawn(|_| {}).await;

    let spec: serde_json::Value = reqwest::Client::new()
        .get(format!("{base}/api/openapi.json"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let request_body = &spec["paths"]["/test/upload"]["post"]["requestBody"];
    let schema = &request_body["content"]["multipart/form-data"]["schema"];
    assert_eq!(schema["type"], "object", "got {request_body}");

    // `FormFile` fields are binary parts; repeated files become an array.
    assert_eq!(schema["properties"]["avatar"]["type"], "string");
    assert_eq!(schema["properties"]["avatar"]["format"], "binary");
    assert_eq!(schema["properties"]["gallery"]["type"], "array");
    assert_eq!(
        schema["properties"]["gallery"]["items"]["format"],
        "binary"
    );

    // A multipart request documents its unmarked fields as form fields too.
    assert_eq!(schema["properties"]["title"]["type"], "string");
    assert_eq!(schema["properties"]["confirm"]["type"], "boolean");

    // The generic JSON body must not be advertised for an upload endpoint.
    assert!(request_body["content"]["application/json"].is_null());
}

// ---------------------------------------------------------------------------
// Streamed downloads
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stream_download_is_chunked_and_unbuffered() {
    let base = spawn(|_| {}).await;

    let resp = reqwest::Client::new()
        .get(format!("{base}/test/download-stream"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    let headers = resp.headers().clone();
    assert_eq!(
        headers.get("content-type").unwrap().to_str().unwrap(),
        "text/plain"
    );
    assert_eq!(
        headers.get("x-produced-by").unwrap().to_str().unwrap(),
        "test"
    );
    // An unknown length means chunked encoding — no Content-Length — and no
    // range support; the server says so explicitly so clients do not try.
    assert!(
        headers.get("content-length").is_none(),
        "a live stream must not claim a length"
    );
    assert_eq!(
        headers.get("accept-ranges").unwrap().to_str().unwrap(),
        "none"
    );
    assert!(headers.get("accept-ranges").is_none() || headers.get("accept-ranges").unwrap() == "none");
    assert!(headers
        .get("content-disposition")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("stream.txt"));

    assert_eq!(
        resp.text().await.unwrap(),
        "chunk-0\nchunk-1\nchunk-2\nchunk-3\n"
    );
}

#[tokio::test]
async fn seekable_stream_download_serves_ranges_and_validators() {
    let base = spawn(|_| {}).await;
    let client = reqwest::Client::new();
    let url = format!("{base}/test/download-seekable");

    let full = client.get(&url).send().await.unwrap();
    assert_eq!(full.status().as_u16(), 200);
    let headers = full.headers().clone();
    assert_eq!(
        headers.get("content-length").unwrap().to_str().unwrap(),
        "16"
    );
    assert_eq!(
        headers.get("accept-ranges").unwrap().to_str().unwrap(),
        "bytes"
    );
    assert_eq!(headers.get("etag").unwrap().to_str().unwrap(), "\"seekable-v1\"");
    assert_eq!(full.text().await.unwrap(), "0123456789abcdef");

    // A range on a seekable stream seeks instead of buffering.
    let partial = client
        .get(&url)
        .header("range", "bytes=4-7")
        .send()
        .await
        .unwrap();
    assert_eq!(partial.status().as_u16(), 206);
    assert_eq!(
        partial
            .headers()
            .get("content-range")
            .unwrap()
            .to_str()
            .unwrap(),
        "bytes 4-7/16"
    );
    assert_eq!(partial.text().await.unwrap(), "4567");

    // The explicit entity tag drives conditional requests.
    let not_modified = client
        .get(&url)
        .header("if-none-match", "\"seekable-v1\"")
        .send()
        .await
        .unwrap();
    assert_eq!(not_modified.status().as_u16(), 304);

    // Range processing can be switched off per file body; the response then
    // advertises that explicitly instead of leaving clients guessing.
    let no_range = client
        .get(format!("{base}/test/download-nonrange"))
        .header("range", "bytes=0-3")
        .send()
        .await
        .unwrap();
    assert_eq!(no_range.status().as_u16(), 200);
    assert_eq!(
        no_range
            .headers()
            .get("accept-ranges")
            .unwrap()
            .to_str()
            .unwrap(),
        "none"
    );
    assert_eq!(no_range.text().await.unwrap(), "0123456789abcdef");
}

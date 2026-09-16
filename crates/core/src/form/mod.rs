//! Multipart form binding — typed access to `multipart/form-data` uploads.
//!
//! This module provides the framework's file-upload primitives:
//!
//! - [`FormFile`] — one uploaded file, with metadata and streaming access.
//! - [`MultipartForm`] — the parsed form: text fields plus uploaded files.
//! - [`FormFileBuilder`] — incremental spooling used by the HTTP layer.
//!
//! # How files are stored
//!
//! A file part is buffered in memory until it exceeds `memory_threshold`, then
//! it is spooled to a temporary file under the configured spool directory.
//! Either way the uploaded bytes live in exactly one place and are never copied
//! into a `String`, so a 1 GB upload costs a bounded amount of RSS.
//!
//! Spooled files are owned by the [`FormFile`] handles that reference them and
//! are deleted when the last handle is dropped — that is, when the request
//! struct is dropped at the end of the request. Call [`FormFile::save_as`] (or
//! [`FormFile::copy_to`]) from your handler to keep the bytes.
//!
//! # Binding
//!
//! Request structs bind form data through ordinary `serde`:
//!
//! ```ignore
//! #[derive(Deserialize)]
//! struct UploadRequest {
//!     title: String,          // text field
//!     avatar: FormFile,       // file part
//!     gallery: Vec<FormFile>, // repeated file parts
//! }
//! ```
//!
//! `FormFile` deliberately only deserializes from the framework's own spool
//! directory, so a client cannot make the server read arbitrary paths by
//! posting JSON.

mod de;
mod spool;

pub use de::{FormDeserializer, FormError};
pub use spool::{sanitize_file_name, set_spool_root, spool_root, SPOOL_FILE_PREFIX};

use crate::error::{Error, Result};
use crate::http::STREAM_BUF_SIZE;
use bytes::Bytes;
use std::fmt;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};

tokio::task_local! {
    /// The form currently being bound, used to hand `FormFile` values to
    /// `serde` while preserving their reference-counted ownership.
    static ACTIVE_FORM: Arc<MultipartForm>;
}

/// Task-local access to the form being deserialized.
///
/// `serde` cannot move an `Arc` through a `Visitor`, so file parts are passed
/// as an index into the active form instead. Binding runs inside
/// [`ActiveForm::run`], which makes the lookup safe: a `FormFile` can only ever
/// be produced while the framework is deserializing a real parsed form.
pub(crate) struct ActiveForm;

impl ActiveForm {
    /// Run `f` with `form` available to `FormFile::deserialize`.
    pub(crate) fn run<R>(form: Arc<MultipartForm>, f: impl FnOnce() -> R) -> R {
        ACTIVE_FORM.sync_scope(form, f)
    }

    /// Resolve a file index produced by [`FormDeserializer`].
    pub(crate) fn resolve(position: usize) -> Option<FormFile> {
        ACTIVE_FORM
            .try_with(|form| form.files.get(position).cloned())
            .ok()
            .flatten()
    }
}

/// One file part of a `multipart/form-data` request.
///
/// Cloning a `FormFile` is cheap: all clones share the same underlying bytes or
/// spooled temporary file, and the temporary file is removed when the last
/// clone is dropped.
#[derive(Clone)]
pub struct FormFile {
    inner: Arc<FormFileInner>,
}

struct FormFileInner {
    field_name: String,
    original_file_name: String,
    file_name: String,
    content_type: Option<String>,
    size: u64,
    source: FileSource,
}

enum FileSource {
    /// Small parts stay in memory, behind a reference-counted buffer so opening
    /// them for reading does not copy the bytes.
    Memory(Bytes),
    /// Large parts are spooled; `_guard` deletes the file on drop.
    Disk {
        path: PathBuf,
        _guard: tempfile::TempPath,
    },
}

/// Zero-copy reader over a shared [`Bytes`] buffer.
struct SharedBytesReader {
    data: Bytes,
    position: usize,
}

impl AsyncRead for SharedBytesReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let position = self.position;
        let remaining = &self.data[position..];
        if remaining.is_empty() {
            return Poll::Ready(Ok(()));
        }
        let take = remaining.len().min(buf.remaining());
        buf.put_slice(&remaining[..take]);
        self.position = position + take;
        Poll::Ready(Ok(()))
    }
}

impl fmt::Debug for FormFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FormFile")
            .field("field_name", &self.inner.field_name)
            .field("file_name", &self.inner.file_name)
            .field("content_type", &self.inner.content_type)
            .field("size", &self.inner.size)
            .field(
                "spooled_to_disk",
                &matches!(self.inner.source, FileSource::Disk { .. }),
            )
            .finish()
    }
}

impl FormFile {
    /// The form field name this file arrived under (e.g. `"avatar"`).
    pub fn field_name(&self) -> &str {
        &self.inner.field_name
    }

    /// A filesystem-safe basename derived from the client-supplied filename.
    ///
    /// Path separators, control characters and Windows-reserved characters are
    /// removed, so this is safe to pass to [`Path::join`]. Use
    /// [`FormFile::original_file_name`] if you need the raw value for display.
    pub fn file_name(&self) -> &str {
        &self.inner.file_name
    }

    /// The filename exactly as sent by the client, without sanitisation.
    ///
    /// Never use this to build a filesystem path.
    pub fn original_file_name(&self) -> &str {
        &self.inner.original_file_name
    }

    /// The part's declared content type, if the client sent one.
    ///
    /// This is attacker-controlled metadata: treat it as a hint, not a
    /// guarantee.
    pub fn content_type(&self) -> Option<&str> {
        self.inner.content_type.as_deref()
    }

    /// Size of the uploaded content in bytes.
    pub fn size(&self) -> u64 {
        self.inner.size
    }

    /// Whether the uploaded content is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.size == 0
    }

    /// Lowercase file extension of [`FormFile::file_name`], without the dot.
    pub fn extension(&self) -> Option<String> {
        Path::new(&self.inner.file_name)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
    }

    /// The spooled temporary file path, when the upload was large enough to be
    /// written to disk.
    ///
    /// The path is only valid while this `FormFile` (or a clone) is alive.
    /// Prefer [`FormFile::save_as`] to hand a file to the rest of your
    /// application.
    pub fn path(&self) -> Option<&Path> {
        match &self.inner.source {
            FileSource::Disk { path, .. } => Some(path.as_path()),
            FileSource::Memory(_) => None,
        }
    }

    /// Open the uploaded content as an async reader.
    ///
    /// Memory-backed files read from a reference-counted buffer without copying
    /// it; spooled files are opened from disk. Either way the read is streamed.
    pub async fn open(&self) -> Result<Box<dyn AsyncRead + Unpin + Send>> {
        match &self.inner.source {
            FileSource::Memory(bytes) => Ok(Box::new(SharedBytesReader {
                data: Bytes::clone(bytes),
                position: 0,
            })),
            FileSource::Disk { path, .. } => {
                let file = tokio::fs::File::open(path).await.map_err(io_error)?;
                Ok(Box::new(file))
            }
        }
    }

    /// Read the whole upload into memory.
    ///
    /// Prefer [`FormFile::open`] or [`FormFile::copy_to`] for files that may be
    /// large.
    pub async fn read_bytes(&self) -> Result<Vec<u8>> {
        match &self.inner.source {
            FileSource::Memory(bytes) => Ok(bytes.to_vec()),
            FileSource::Disk { path, .. } => tokio::fs::read(path).await.map_err(io_error),
        }
    }

    /// Stream the upload into `writer`, returning the number of bytes written.
    ///
    /// Uses the framework's 64 KiB copy buffer rather than the 8 KiB default,
    /// which matters when a multi-gigabyte upload is being persisted.
    pub async fn copy_to<W>(&self, writer: &mut W) -> Result<u64>
    where
        W: AsyncWrite + Unpin + ?Sized,
    {
        let mut reader = self.open().await?;
        let mut buffer = vec![0u8; STREAM_BUF_SIZE];
        let mut total: u64 = 0;

        loop {
            let read = reader.read(&mut buffer).await.map_err(io_error)?;
            if read == 0 {
                return Ok(total);
            }
            writer.write_all(&buffer[..read]).await.map_err(io_error)?;
            total += read as u64;
        }
    }

    /// Stream the upload to `destination`, creating parent directories.
    ///
    /// The bytes are written to a sibling temporary file first and renamed into
    /// place, so `destination` never contains a partially written upload.
    pub async fn save_as(&self, destination: impl AsRef<Path>) -> Result<()> {
        let destination = destination.as_ref();
        let parent = destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        tokio::fs::create_dir_all(parent).await.map_err(io_error)?;

        let staging = staging_path(destination);
        let mut out = tokio::fs::File::create(&staging).await.map_err(io_error)?;

        if let Err(err) = self.copy_to(&mut out).await {
            drop(out);
            let _ = tokio::fs::remove_file(&staging).await;
            return Err(err);
        }
        out.flush().await.map_err(io_error)?;
        out.sync_all().await.map_err(io_error)?;
        drop(out);

        if let Err(err) = tokio::fs::rename(&staging, destination).await {
            let _ = tokio::fs::remove_file(&staging).await;
            return Err(io_error(err));
        }
        Ok(())
    }

    fn from_parts(
        field_name: String,
        original_file_name: String,
        content_type: Option<String>,
        size: u64,
        source: FileSource,
    ) -> Self {
        let file_name = sanitize_file_name(&original_file_name);
        Self {
            inner: Arc::new(FormFileInner {
                field_name,
                original_file_name,
                file_name,
                content_type,
                size,
                source,
            }),
        }
    }
}

/// A path in the same directory as `destination`, used for atomic writes.
///
/// Keeping the staging file next to the target guarantees the final
/// [`tokio::fs::rename`] stays on one filesystem.
fn staging_path(destination: &Path) -> PathBuf {
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let name = destination
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("upload");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    parent.join(format!(".{}.{}.{}.part", name, std::process::id(), nonce))
}

fn io_error(err: std::io::Error) -> Error {
    Error::Internal(format!("file I/O error: {err}"))
}

/// Incrementally builds a [`FormFile`] while a multipart part streams in.
///
/// The builder buffers in memory up to `memory_threshold` bytes and spills to a
/// temporary file beyond that, so peak memory per upload is bounded regardless
/// of file size.
pub struct FormFileBuilder {
    field_name: String,
    original_file_name: String,
    content_type: Option<String>,
    memory_threshold: usize,
    max_size: u64,
    spool_dir: PathBuf,
    buffer: Vec<u8>,
    spooled: Option<SpooledState>,
    written: u64,
}

struct SpooledState {
    /// Buffered on purpose: `tokio::fs::File` dispatches every write to the
    /// blocking pool, so coalescing transport-sized chunks into one write per
    /// buffer saves a thread hop per chunk.
    writer: tokio::io::BufWriter<tokio::fs::File>,
    guard: tempfile::TempPath,
    path: PathBuf,
}

impl FormFileBuilder {
    /// Start building a file for `field_name`.
    ///
    /// `raw_file_name` is the client-supplied name; it is sanitised by
    /// [`sanitize_file_name`] when the file is finished.
    pub fn new(
        field_name: impl Into<String>,
        raw_file_name: impl Into<String>,
        content_type: Option<String>,
    ) -> Self {
        Self {
            field_name: field_name.into(),
            original_file_name: raw_file_name.into(),
            content_type,
            memory_threshold: 1024 * 1024,
            max_size: 128 * 1024 * 1024,
            spool_dir: spool_root(),
            buffer: Vec::new(),
            spooled: None,
            written: 0,
        }
    }

    /// Bytes kept in memory before spilling to disk. Default: 1 MiB.
    pub fn memory_threshold(mut self, bytes: usize) -> Self {
        self.memory_threshold = bytes;
        self
    }

    /// Maximum accepted size for this file. Default: 128 MiB.
    ///
    /// Exceeding it fails the upload with [`Error::PayloadTooLarge`].
    pub fn max_size(mut self, bytes: u64) -> Self {
        self.max_size = bytes;
        self
    }

    /// Directory used for spooled files. Default: [`spool_root`].
    pub fn spool_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.spool_dir = dir.into();
        self
    }

    /// Append a chunk of the current part.
    ///
    /// Returns [`Error::PayloadTooLarge`] as soon as the file exceeds
    /// [`FormFileBuilder::max_size`], so an oversized upload is rejected
    /// without buffering the whole body.
    pub async fn write(&mut self, chunk: &[u8]) -> Result<()> {
        if chunk.is_empty() {
            return Ok(());
        }
        self.written = self.written.saturating_add(chunk.len() as u64);
        if self.written > self.max_size {
            return Err(Error::PayloadTooLarge(format!(
                "file field `{}` exceeds the {}-byte limit",
                self.field_name, self.max_size
            )));
        }

        if self.spooled.is_none() && self.buffer.len() + chunk.len() > self.memory_threshold {
            self.spill().await?;
        }

        match &mut self.spooled {
            Some(state) => {
                state.writer.write_all(chunk).await.map_err(io_error)?;
            }
            None => self.buffer.extend_from_slice(chunk),
        }
        Ok(())
    }

    /// Finish the part and produce the [`FormFile`].
    pub async fn finish(mut self) -> Result<FormFile> {
        if let Some(state) = self.spooled.take() {
            let SpooledState {
                mut writer,
                guard,
                path,
            } = state;
            // `BufWriter::into_inner` drops whatever is still buffered, so the
            // flush has to happen first.
            writer.flush().await.map_err(io_error)?;
            let file = writer.into_inner();
            file.sync_all().await.map_err(io_error)?;
            drop(file);
            Ok(FormFile::from_parts(
                self.field_name,
                self.original_file_name,
                self.content_type,
                self.written,
                FileSource::Disk {
                    path,
                    _guard: guard,
                },
            ))
        } else {
            Ok(FormFile::from_parts(
                self.field_name,
                self.original_file_name,
                self.content_type,
                self.written,
                FileSource::Memory(Bytes::from(std::mem::take(&mut self.buffer))),
            ))
        }
    }

    async fn spill(&mut self) -> Result<()> {
        tokio::fs::create_dir_all(&self.spool_dir)
            .await
            .map_err(io_error)?;
        let named = tempfile::Builder::new()
            .prefix(SPOOL_FILE_PREFIX)
            .tempfile_in(&self.spool_dir)
            .map_err(io_error)?;
        let (std_file, guard) = named.into_parts();
        let path = guard.to_path_buf();
        let mut writer = tokio::io::BufWriter::with_capacity(
            STREAM_BUF_SIZE,
            tokio::fs::File::from_std(std_file),
        );
        if !self.buffer.is_empty() {
            writer.write_all(&self.buffer).await.map_err(io_error)?;
        }
        self.buffer = Vec::new();
        self.buffer.shrink_to_fit();
        self.spooled = Some(SpooledState {
            writer,
            guard,
            path,
        });
        Ok(())
    }
}

/// One text part of a multipart form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormField {
    /// Field name as sent by the client.
    pub name: String,
    /// Field value decoded as UTF-8.
    pub value: String,
}

/// A parsed `multipart/form-data` body.
///
/// Cheap to clone: files are reference-counted handles.
#[derive(Debug, Clone, Default)]
pub struct MultipartForm {
    fields: Vec<FormField>,
    files: Vec<FormFile>,
}

impl MultipartForm {
    /// Create an empty form.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a text field. Empty names are ignored.
    pub fn push_field(&mut self, name: impl Into<String>, value: impl Into<String>) {
        let name = name.into();
        if name.is_empty() {
            return;
        }
        self.fields.push(FormField {
            name,
            value: value.into(),
        });
    }

    /// Append an uploaded file.
    pub fn push_file(&mut self, file: FormFile) {
        self.files.push(file);
    }

    /// All text fields, in the order they appeared.
    pub fn fields(&self) -> &[FormField] {
        &self.fields
    }

    /// All uploaded files, in the order they appeared.
    pub fn files(&self) -> &[FormFile] {
        &self.files
    }

    /// The first text value for `name`.
    pub fn text<'a>(&'a self, name: &str) -> Option<&'a str> {
        self.texts(name).next()
    }

    /// Every text value for `name`, in order.
    pub fn texts<'a, 'n>(&'a self, name: &'n str) -> impl Iterator<Item = &'a str> + 'n
    where
        'a: 'n,
    {
        self.fields
            .iter()
            .filter(move |f| f.name == name)
            .map(|f| f.value.as_str())
    }

    /// The first uploaded file for `name`.
    pub fn file<'a>(&'a self, name: &str) -> Option<&'a FormFile> {
        self.files_named(name).next()
    }

    /// Every uploaded file for `name`, in order.
    pub fn files_named<'a, 'n>(&'a self, name: &'n str) -> impl Iterator<Item = &'a FormFile> + 'n
    where
        'a: 'n,
    {
        self.files.iter().filter(move |f| f.field_name() == name)
    }

    /// Whether any field or file uses `name`.
    pub fn contains(&self, name: &str) -> bool {
        self.fields.iter().any(|f| f.name == name) || self.file(name).is_some()
    }

    /// Whether the form carries neither fields nor files.
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty() && self.files.is_empty()
    }

    /// Number of text fields.
    pub fn field_count(&self) -> usize {
        self.fields.len()
    }

    /// Number of uploaded files.
    pub fn file_count(&self) -> usize {
        self.files.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_path_traversal() {
        assert_eq!(sanitize_file_name("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_file_name(r"..\..\windows\win.ini"), "win.ini");
        assert_eq!(sanitize_file_name("/absolute/path/a.txt"), "a.txt");
    }

    #[test]
    fn sanitize_rejects_empty_and_dot_names() {
        assert_eq!(sanitize_file_name(""), "file");
        assert_eq!(sanitize_file_name("   "), "file");
        assert_eq!(sanitize_file_name(".."), "file");
        assert_eq!(sanitize_file_name("."), "file");
        assert_eq!(sanitize_file_name("trailing."), "trailing");
    }

    #[test]
    fn sanitize_replaces_reserved_characters() {
        assert_eq!(sanitize_file_name("a:b|c?.txt"), "a_b_c_.txt");
        assert_eq!(sanitize_file_name("nul\u{0}byte.txt"), "nulbyte.txt");
    }

    #[test]
    fn sanitize_keeps_leading_dot_names() {
        assert_eq!(sanitize_file_name(".env"), ".env");
    }

    #[test]
    fn sanitize_caps_length() {
        let long = "x".repeat(500) + ".txt";
        let out = sanitize_file_name(&long);
        assert!(out.len() <= 200, "got {} bytes", out.len());
        assert!(out.ends_with(".txt"));
    }

    #[tokio::test]
    async fn builder_keeps_small_files_in_memory() {
        let mut builder = FormFileBuilder::new("avatar", "a.png", Some("image/png".into()))
            .memory_threshold(1024);
        builder.write(b"hello").await.unwrap();
        let file = builder.finish().await.unwrap();
        assert_eq!(file.size(), 5);
        assert!(file.path().is_none());
        assert_eq!(file.read_bytes().await.unwrap(), b"hello");
        assert_eq!(file.content_type(), Some("image/png"));
        assert_eq!(file.extension().as_deref(), Some("png"));
    }

    #[tokio::test]
    async fn builder_spills_large_files_to_disk_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let mut builder = FormFileBuilder::new("blob", "big.bin", None)
            .memory_threshold(8)
            .spool_dir(dir.path().to_path_buf());
        builder.write(b"0123456789").await.unwrap();
        builder.write(b"abcdefghij").await.unwrap();
        let file = builder.finish().await.unwrap();

        assert_eq!(file.size(), 20);
        let path = file.path().expect("should be spooled").to_path_buf();
        assert!(path.exists());
        assert_eq!(file.read_bytes().await.unwrap(), b"0123456789abcdefghij");

        drop(file);
        assert!(!path.exists(), "spooled file must be deleted on drop");
    }

    #[tokio::test]
    async fn builder_enforces_max_size() {
        let mut builder = FormFileBuilder::new("f", "f.bin", None).max_size(4);
        let err = builder.write(b"12345").await.unwrap_err();
        assert!(matches!(err, Error::PayloadTooLarge(_)), "got {err:?}");
        assert_eq!(err.status_code(), 413);
    }

    #[tokio::test]
    async fn save_as_writes_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let mut builder = FormFileBuilder::new("f", "note.txt", None);
        builder.write(b"content").await.unwrap();
        let file = builder.finish().await.unwrap();

        let dest = dir.path().join("nested/deep/note.txt");
        file.save_as(&dest).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"content");

        // No staging files left behind.
        let leftovers: Vec<_> = std::fs::read_dir(dest.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".part"))
            .collect();
        assert!(leftovers.is_empty(), "staging file leaked: {leftovers:?}");
    }

    #[test]
    fn form_lookup_helpers() {
        let mut form = MultipartForm::new();
        form.push_field("title", "hello");
        form.push_field("tag", "a");
        form.push_field("tag", "b");
        assert_eq!(form.text("title"), Some("hello"));
        assert_eq!(form.texts("tag").collect::<Vec<_>>(), vec!["a", "b"]);
        assert!(form.contains("title"));
        assert!(!form.contains("missing"));
        assert_eq!(form.field_count(), 3);
    }
}

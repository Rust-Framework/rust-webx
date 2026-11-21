//! Object storage port and recording metadata (2.4-2).
//!
//! The framework owns the **port**, not a cloud SDK: applications register an
//! [`IObjectStore`] (in-memory for tests, filesystem/S3 for production) and the
//! recording control plane stores segments through it. Uploads are
//! **resumable**: `begin_upload` → `put_part` (retryable, idempotent per part)
//! → `complete_upload`, with `list_parts` for resume and `abort_upload` for
//! rollback. A cloud adapter can also expose pre-signed URLs via
//! [`IObjectStore::presign_put`].
//!
//! Two implementations ship in core so the port is executable in CI without a
//! broker: [`InMemoryObjectStore`] (fake) and [`FileSystemObjectStore`] (real
//! local persistence). An S3 adapter is a 2.4-7 starter.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha1::Digest;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::error::{Error, Result};

/// Stored object metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectMeta {
    /// Storage key.
    pub key: String,
    /// Object size in bytes.
    pub size: u64,
    /// Media type.
    pub content_type: String,
    /// Content hash (`sha1-hex`) used for integrity checks.
    pub etag: String,
}

/// One stored object with its bytes.
#[derive(Debug, Clone)]
pub struct StoredObject {
    /// Metadata.
    pub meta: ObjectMeta,
    /// Raw bytes.
    pub bytes: Vec<u8>,
}

/// Resumable upload session id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct UploadId(String);

impl UploadId {
    /// Session id as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn next(prefix: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(format!("{prefix}-{}", NEXT.fetch_add(1, Ordering::Relaxed)))
    }
}

/// Acknowledged part of a resumable upload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartEtag {
    /// 1-based part number.
    pub part: u32,
    /// Content hash of the part (`sha1-hex`).
    pub etag: String,
    /// Part size in bytes.
    pub size: u64,
}

/// Object storage port.
#[async_trait]
pub trait IObjectStore: Send + Sync {
    /// Store a whole object (small files, recording covers).
    async fn put(&self, key: &str, bytes: Vec<u8>, content_type: &str) -> Result<ObjectMeta>;

    /// Read a whole object.
    async fn get(&self, key: &str) -> Result<Option<StoredObject>>;

    /// Read object metadata without the body.
    async fn head(&self, key: &str) -> Result<Option<ObjectMeta>>;

    /// Delete an object (idempotent).
    async fn delete(&self, key: &str) -> Result<()>;

    /// Start a resumable upload for `key`.
    async fn begin_upload(&self, key: &str, content_type: &str) -> Result<UploadId>;

    /// Upload/replace one part (the same `part` may be retried safely).
    async fn put_part(&self, upload: &UploadId, part: u32, bytes: Vec<u8>) -> Result<PartEtag>;

    /// Parts uploaded so far (for resume).
    async fn list_parts(&self, upload: &UploadId) -> Result<Vec<PartEtag>>;

    /// Finish the upload: requires consecutive parts starting at 1.
    async fn complete_upload(&self, upload: UploadId) -> Result<ObjectMeta>;

    /// Drop the upload session and its parts.
    async fn abort_upload(&self, upload: UploadId) -> Result<()>;

    /// Optional pre-signed PUT URL (cloud adapters); the default means "upload
    /// through the application".
    fn presign_put(&self, _key: &str, _expires: Duration) -> Option<String> {
        None
    }
}

fn sha1_hex(bytes: &[u8]) -> String {
    data_encoding::HEXLOWER.encode(&sha1::Sha1::digest(bytes))
}

fn validate_key(key: &str) -> Result<()> {
    if key.is_empty() {
        return Err(Error::Validation("object key must not be empty".into()));
    }
    if key.starts_with('/') || key.contains("..") || key.contains('\\') {
        return Err(Error::Validation(
            "object key must be a relative path without '..' or backslashes".into(),
        ));
    }
    Ok(())
}

fn expected_parts(parts: &[PartEtag]) -> Result<u32> {
    if parts.is_empty() {
        return Err(Error::Validation("upload has no parts".into()));
    }
    let mut sorted: Vec<u32> = parts.iter().map(|part| part.part).collect();
    sorted.sort_unstable();
    if sorted.first() != Some(&1) || sorted.windows(2).any(|pair| pair[1] != pair[0] + 1) {
        return Err(Error::Validation(
            "upload parts must be consecutive starting at 1".into(),
        ));
    }
    Ok(*sorted.last().expect("non-empty"))
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// In-memory fake
// ---------------------------------------------------------------------------

#[derive(Default)]
struct MemoryState {
    objects: HashMap<String, StoredObject>,
    uploads: HashMap<UploadId, MemoryUpload>,
}

struct MemoryUpload {
    key: String,
    content_type: String,
    parts: HashMap<u32, Vec<u8>>,
}

/// In-memory object store (tests, single-process demos).
#[derive(Default)]
pub struct InMemoryObjectStore {
    state: Mutex<MemoryState>,
}

impl InMemoryObjectStore {
    /// Empty store.
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, MemoryState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[async_trait]
impl IObjectStore for InMemoryObjectStore {
    async fn put(&self, key: &str, bytes: Vec<u8>, content_type: &str) -> Result<ObjectMeta> {
        validate_key(key)?;
        let meta = ObjectMeta {
            key: key.to_string(),
            size: bytes.len() as u64,
            content_type: content_type.to_string(),
            etag: sha1_hex(&bytes),
        };
        self.lock().objects.insert(
            key.to_string(),
            StoredObject {
                meta: meta.clone(),
                bytes,
            },
        );
        Ok(meta)
    }

    async fn get(&self, key: &str) -> Result<Option<StoredObject>> {
        validate_key(key)?;
        Ok(self.lock().objects.get(key).cloned())
    }

    async fn head(&self, key: &str) -> Result<Option<ObjectMeta>> {
        validate_key(key)?;
        Ok(self
            .lock()
            .objects
            .get(key)
            .map(|object| object.meta.clone()))
    }

    async fn delete(&self, key: &str) -> Result<()> {
        validate_key(key)?;
        self.lock().objects.remove(key);
        Ok(())
    }

    async fn begin_upload(&self, key: &str, content_type: &str) -> Result<UploadId> {
        validate_key(key)?;
        let upload = UploadId::next("upload");
        self.lock().uploads.insert(
            upload.clone(),
            MemoryUpload {
                key: key.to_string(),
                content_type: content_type.to_string(),
                parts: HashMap::new(),
            },
        );
        Ok(upload)
    }

    async fn put_part(&self, upload: &UploadId, part: u32, bytes: Vec<u8>) -> Result<PartEtag> {
        if part == 0 {
            return Err(Error::Validation("part numbers start at 1".into()));
        }
        let mut state = self.lock();
        let session = state
            .uploads
            .get_mut(upload)
            .ok_or_else(|| Error::NotFound(format!("upload '{}' not found", upload.as_str())))?;
        let etag = sha1_hex(&bytes);
        let size = bytes.len() as u64;
        session.parts.insert(part, bytes);
        Ok(PartEtag { part, etag, size })
    }

    async fn list_parts(&self, upload: &UploadId) -> Result<Vec<PartEtag>> {
        let state = self.lock();
        let session = state
            .uploads
            .get(upload)
            .ok_or_else(|| Error::NotFound(format!("upload '{}' not found", upload.as_str())))?;
        let mut parts: Vec<PartEtag> = session
            .parts
            .iter()
            .map(|(part, bytes)| PartEtag {
                part: *part,
                etag: sha1_hex(bytes),
                size: bytes.len() as u64,
            })
            .collect();
        parts.sort_by_key(|part| part.part);
        Ok(parts)
    }

    async fn complete_upload(&self, upload: UploadId) -> Result<ObjectMeta> {
        let mut state = self.lock();
        let session = state
            .uploads
            .remove(&upload)
            .ok_or_else(|| Error::NotFound(format!("upload '{}' not found", upload.as_str())))?;
        let parts: Vec<PartEtag> = session
            .parts
            .iter()
            .map(|(part, bytes)| PartEtag {
                part: *part,
                etag: sha1_hex(bytes),
                size: bytes.len() as u64,
            })
            .collect();
        let last = expected_parts(&parts)?;
        let mut bytes = Vec::new();
        for part in 1..=last {
            bytes.extend_from_slice(&session.parts[&part]);
        }
        let meta = ObjectMeta {
            key: session.key.clone(),
            size: bytes.len() as u64,
            content_type: session.content_type,
            etag: sha1_hex(&bytes),
        };
        state.objects.insert(
            session.key,
            StoredObject {
                meta: meta.clone(),
                bytes,
            },
        );
        Ok(meta)
    }

    async fn abort_upload(&self, upload: UploadId) -> Result<()> {
        self.lock().uploads.remove(&upload);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// File-system implementation (real local persistence)
// ---------------------------------------------------------------------------

/// File-system object store: keys map to files under `root`, resumable uploads
/// live under `<root>/.uploads/<id>/`.
#[derive(Debug, Clone)]
pub struct FileSystemObjectStore {
    root: PathBuf,
}

impl FileSystemObjectStore {
    /// Create a store rooted at `root` (created on demand).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn object_path(&self, key: &str) -> Result<PathBuf> {
        validate_key(key)?;
        Ok(self.root.join(key))
    }

    fn upload_dir(&self, upload: &UploadId) -> PathBuf {
        self.root.join(".uploads").join(upload.as_str())
    }

    async fn read_bytes(path: &Path) -> Result<Option<Vec<u8>>> {
        match tokio::fs::read(path).await {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(Error::Internal(format!(
                "object store read failed: {error}"
            ))),
        }
    }

    async fn meta_of(&self, key: &str, path: &Path, content_type: &str) -> Result<ObjectMeta> {
        let bytes = tokio::fs::read(path).await.map_err(|error| {
            Error::Internal(format!("object store metadata read failed: {error}"))
        })?;
        Ok(ObjectMeta {
            key: key.to_string(),
            size: bytes.len() as u64,
            content_type: content_type.to_string(),
            etag: sha1_hex(&bytes),
        })
    }

    fn content_type_path(&self, upload: &UploadId) -> PathBuf {
        self.upload_dir(upload).join("content-type")
    }
}

#[async_trait]
impl IObjectStore for FileSystemObjectStore {
    async fn put(&self, key: &str, bytes: Vec<u8>, content_type: &str) -> Result<ObjectMeta> {
        let path = self.object_path(key)?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|error| {
                Error::Internal(format!("object store directory create failed: {error}"))
            })?;
        }
        tokio::fs::write(&path, &bytes)
            .await
            .map_err(|error| Error::Internal(format!("object store write failed: {error}")))?;
        Ok(ObjectMeta {
            key: key.to_string(),
            size: bytes.len() as u64,
            content_type: content_type.to_string(),
            etag: sha1_hex(&bytes),
        })
    }

    async fn get(&self, key: &str) -> Result<Option<StoredObject>> {
        let path = self.object_path(key)?;
        let Some(bytes) = Self::read_bytes(&path).await? else {
            return Ok(None);
        };
        let meta = ObjectMeta {
            key: key.to_string(),
            size: bytes.len() as u64,
            content_type: String::new(),
            etag: sha1_hex(&bytes),
        };
        Ok(Some(StoredObject { meta, bytes }))
    }

    async fn head(&self, key: &str) -> Result<Option<ObjectMeta>> {
        let path = self.object_path(key)?;
        match tokio::fs::metadata(&path).await {
            Ok(_) => Ok(Some(self.meta_of(key, &path, "").await?)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(Error::Internal(format!(
                "object store head failed: {error}"
            ))),
        }
    }

    async fn delete(&self, key: &str) -> Result<()> {
        let path = self.object_path(key)?;
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(Error::Internal(format!(
                "object store delete failed: {error}"
            ))),
        }
    }

    async fn begin_upload(&self, key: &str, content_type: &str) -> Result<UploadId> {
        validate_key(key)?;
        let upload = UploadId::next("upload");
        let dir = self.upload_dir(&upload);
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|error| Error::Internal(format!("upload directory create failed: {error}")))?;
        tokio::fs::write(self.content_type_path(&upload), content_type.as_bytes())
            .await
            .map_err(|error| Error::Internal(format!("upload metadata write failed: {error}")))?;
        tokio::fs::write(dir.join("target"), key)
            .await
            .map_err(|error| Error::Internal(format!("upload metadata write failed: {error}")))?;
        Ok(upload)
    }

    async fn put_part(&self, upload: &UploadId, part: u32, bytes: Vec<u8>) -> Result<PartEtag> {
        if part == 0 {
            return Err(Error::Validation("part numbers start at 1".into()));
        }
        let dir = self.upload_dir(upload);
        if !tokio::fs::try_exists(dir.join("target"))
            .await
            .unwrap_or(false)
        {
            return Err(Error::NotFound(format!(
                "upload '{}' not found",
                upload.as_str()
            )));
        }
        let path = dir.join(format!("part-{part:05}"));
        tokio::fs::write(&path, &bytes)
            .await
            .map_err(|error| Error::Internal(format!("upload part write failed: {error}")))?;
        Ok(PartEtag {
            part,
            etag: sha1_hex(&bytes),
            size: bytes.len() as u64,
        })
    }

    async fn list_parts(&self, upload: &UploadId) -> Result<Vec<PartEtag>> {
        let dir = self.upload_dir(upload);
        if !tokio::fs::try_exists(dir.join("target"))
            .await
            .unwrap_or(false)
        {
            return Err(Error::NotFound(format!(
                "upload '{}' not found",
                upload.as_str()
            )));
        }
        let mut parts = Vec::new();
        let mut entries = tokio::fs::read_dir(&dir)
            .await
            .map_err(|error| Error::Internal(format!("upload directory read failed: {error}")))?;
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|error| Error::Internal(format!("upload directory read failed: {error}")))?
        {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let Some(part) = name
                .strip_prefix("part-")
                .and_then(|value| value.parse::<u32>().ok())
            else {
                continue;
            };
            let bytes = tokio::fs::read(entry.path())
                .await
                .map_err(|error| Error::Internal(format!("upload part read failed: {error}")))?;
            parts.push(PartEtag {
                part,
                etag: sha1_hex(&bytes),
                size: bytes.len() as u64,
            });
        }
        parts.sort_by_key(|part| part.part);
        Ok(parts)
    }

    async fn complete_upload(&self, upload: UploadId) -> Result<ObjectMeta> {
        let dir = self.upload_dir(&upload);
        let key = tokio::fs::read_to_string(dir.join("target"))
            .await
            .map_err(|_| Error::NotFound(format!("upload '{}' not found", upload.as_str())))?;
        let content_type = tokio::fs::read_to_string(self.content_type_path(&upload))
            .await
            .unwrap_or_default();
        let parts = self.list_parts(&upload).await?;
        let last = expected_parts(&parts)?;

        let target = self.object_path(&key)?;
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|error| {
                Error::Internal(format!("object store directory create failed: {error}"))
            })?;
        }
        let mut output = tokio::fs::File::create(&target)
            .await
            .map_err(|error| Error::Internal(format!("upload target create failed: {error}")))?;
        let mut hasher = sha1::Sha1::new();
        let mut size = 0u64;
        for part in 1..=last {
            let path = dir.join(format!("part-{part:05}"));
            let mut input = tokio::fs::File::open(&path)
                .await
                .map_err(|error| Error::Internal(format!("upload part open failed: {error}")))?;
            let mut buffer = vec![0u8; 64 * 1024];
            loop {
                let read = input.read(&mut buffer).await.map_err(|error| {
                    Error::Internal(format!("upload part read failed: {error}"))
                })?;
                if read == 0 {
                    break;
                }
                hasher.update(&buffer[..read]);
                output.write_all(&buffer[..read]).await.map_err(|error| {
                    Error::Internal(format!("upload target write failed: {error}"))
                })?;
                size += read as u64;
            }
        }
        output
            .flush()
            .await
            .map_err(|error| Error::Internal(format!("upload target flush failed: {error}")))?;
        let meta = ObjectMeta {
            key,
            size,
            content_type,
            etag: data_encoding::HEXLOWER.encode(&hasher.finalize()),
        };
        let _ = tokio::fs::remove_dir_all(&dir).await;
        Ok(meta)
    }

    async fn abort_upload(&self, upload: UploadId) -> Result<()> {
        let _ = tokio::fs::remove_dir_all(self.upload_dir(&upload)).await;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Recording metadata
// ---------------------------------------------------------------------------

/// Recording lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingStatus {
    /// Segments are still being uploaded.
    Recording,
    /// All segments uploaded and finalized.
    Completed,
    /// Recorder failed; the recording is not playable.
    Failed,
}

/// One uploaded recording segment (typically a few seconds of media).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingSegment {
    /// 1-based segment/part number.
    pub part: u32,
    /// Object key in the store.
    pub key: String,
    /// Segment size in bytes.
    pub size: u64,
    /// Media duration in milliseconds.
    pub duration_ms: u64,
    /// Segment start (unix seconds).
    pub started_at_unix: u64,
}

/// Recording control-plane metadata (playback index).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingMeta {
    /// Recording id.
    pub id: String,
    /// Room the recording belongs to.
    pub room_id: String,
    /// User that started the recording.
    pub owner_id: String,
    /// Current state.
    pub status: RecordingStatus,
    /// Media content type (`audio/ogg`, `video/webm`, ...).
    pub content_type: String,
    /// Segments in playback order.
    pub segments: Vec<RecordingSegment>,
    /// Sum of segment sizes.
    pub total_size: u64,
    /// Sum of segment durations.
    pub total_duration_ms: u64,
    /// Creation time (unix seconds).
    pub created_at_unix: u64,
    /// Completion time (unix seconds).
    pub completed_at_unix: Option<u64>,
}

impl RecordingMeta {
    /// Start a new recording at the current wall-clock time.
    pub fn new(
        id: impl Into<String>,
        room_id: impl Into<String>,
        owner_id: impl Into<String>,
        content_type: impl Into<String>,
    ) -> Self {
        Self::new_at(id, room_id, owner_id, content_type, now_unix())
    }

    /// Start a recording at a fixed unix time (tests / injected clocks).
    pub fn new_at(
        id: impl Into<String>,
        room_id: impl Into<String>,
        owner_id: impl Into<String>,
        content_type: impl Into<String>,
        created_at_unix: u64,
    ) -> Self {
        Self {
            id: id.into(),
            room_id: room_id.into(),
            owner_id: owner_id.into(),
            status: RecordingStatus::Recording,
            content_type: content_type.into(),
            segments: Vec::new(),
            total_size: 0,
            total_duration_ms: 0,
            created_at_unix,
            completed_at_unix: None,
        }
    }

    /// Append a segment (keeps the playback index ordered by `part`).
    pub fn add_segment(&mut self, segment: RecordingSegment) {
        self.total_size = self.total_size.saturating_add(segment.size);
        self.total_duration_ms = self.total_duration_ms.saturating_add(segment.duration_ms);
        self.segments.push(segment);
        self.segments.sort_by_key(|segment| segment.part);
    }

    /// Insert or replace one segment (idempotent retry of the same part).
    ///
    /// Recomputes the totals from the index so a retried upload cannot inflate
    /// the recording size/duration.
    pub fn replace_segment(&mut self, segment: RecordingSegment) {
        self.segments
            .retain(|existing| existing.part != segment.part);
        self.segments.push(segment);
        self.segments.sort_by_key(|segment| segment.part);
        self.total_size = self.segments.iter().map(|segment| segment.size).sum();
        self.total_duration_ms = self
            .segments
            .iter()
            .map(|segment| segment.duration_ms)
            .sum();
    }

    /// Mark the recording complete.
    pub fn complete(&mut self) {
        self.status = RecordingStatus::Completed;
        self.completed_at_unix = Some(now_unix());
    }

    /// Mark the recording failed.
    pub fn fail(&mut self) {
        self.status = RecordingStatus::Failed;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn exercise_store(store: &dyn IObjectStore) {
        let meta = store
            .put("cover.json", b"{}".to_vec(), "application/json")
            .await
            .expect("put");
        assert_eq!(meta.size, 2);
        let object = store.get("cover.json").await.expect("get").expect("exists");
        assert_eq!(object.bytes, b"{}");
        assert!(store.head("cover.json").await.expect("head").is_some());

        let upload = store
            .begin_upload("rec/part-1", "audio/ogg")
            .await
            .expect("begin");
        let first = store
            .put_part(&upload, 1, b"abc".to_vec())
            .await
            .expect("p1");
        assert_eq!(first.size, 3);
        // Retrying part 2 is safe (last write wins) and parts can be listed.
        store
            .put_part(&upload, 2, b"de".to_vec())
            .await
            .expect("p2a");
        store
            .put_part(&upload, 2, b"def".to_vec())
            .await
            .expect("p2b");
        let parts = store.list_parts(&upload).await.expect("list");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[1].size, 3);
        let completed = store.complete_upload(upload).await.expect("complete");
        assert_eq!(completed.size, 6);
        assert_eq!(completed.content_type, "audio/ogg");
        let object = store
            .get("rec/part-1")
            .await
            .expect("get")
            .expect("completed object");
        assert_eq!(object.bytes, b"abcdef");

        // Gaps are rejected.
        let upload = store
            .begin_upload("rec/gap", "audio/ogg")
            .await
            .expect("begin");
        store
            .put_part(&upload, 2, b"x".to_vec())
            .await
            .expect("part");
        assert!(matches!(
            store.complete_upload(upload).await,
            Err(Error::Validation(_))
        ));

        // Abort removes the session.
        let upload = store
            .begin_upload("rec/aborted", "audio/ogg")
            .await
            .expect("begin");
        store.abort_upload(upload.clone()).await.expect("abort");
        assert!(matches!(
            store.list_parts(&upload).await,
            Err(Error::NotFound(_))
        ));

        // Key traversal is rejected.
        assert!(matches!(
            store.put("../escape", Vec::new(), "text/plain").await,
            Err(Error::Validation(_))
        ));

        store.delete("cover.json").await.expect("delete");
        assert!(store.get("cover.json").await.expect("get").is_none());
    }

    #[tokio::test]
    async fn in_memory_store_supports_resumable_uploads() {
        let store = InMemoryObjectStore::new();
        exercise_store(&store).await;
    }

    #[tokio::test]
    async fn file_system_store_supports_resumable_uploads() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = FileSystemObjectStore::new(dir.path());
        exercise_store(&store).await;
        // Real persistence: a new store instance sees the completed object.
        let store = FileSystemObjectStore::new(dir.path());
        let upload = store
            .begin_upload("persisted", "text/plain")
            .await
            .expect("begin");
        store
            .put_part(&upload, 1, b"hi".to_vec())
            .await
            .expect("part");
        store.complete_upload(upload).await.expect("complete");
        let reopened = FileSystemObjectStore::new(dir.path());
        let object = reopened
            .get("persisted")
            .await
            .expect("get")
            .expect("exists");
        assert_eq!(object.bytes, b"hi");
    }

    #[test]
    fn recording_metadata_keeps_a_sorted_index_and_totals() {
        let fixed = RecordingMeta::new_at("rec-fixed", "room-1", "user-1", "audio/ogg", 42);
        assert_eq!(fixed.created_at_unix, 42, "injected clock is honoured");
        let mut recording = RecordingMeta::new("rec-1", "room-1", "user-1", "audio/ogg");
        assert_eq!(recording.status, RecordingStatus::Recording);
        recording.add_segment(RecordingSegment {
            part: 2,
            key: "rec/2".into(),
            size: 10,
            duration_ms: 2_000,
            started_at_unix: 1,
        });
        recording.add_segment(RecordingSegment {
            part: 1,
            key: "rec/1".into(),
            size: 5,
            duration_ms: 1_000,
            started_at_unix: 0,
        });
        assert_eq!(recording.segments[0].part, 1, "playback index is ordered");
        assert_eq!(recording.total_size, 15);
        assert_eq!(recording.total_duration_ms, 3_000);
        // Retrying a part must not inflate the totals.
        recording.replace_segment(RecordingSegment {
            part: 2,
            key: "rec/2".into(),
            size: 12,
            duration_ms: 2_500,
            started_at_unix: 1,
        });
        assert_eq!(recording.segments.len(), 2);
        assert_eq!(recording.total_size, 17);
        assert_eq!(recording.total_duration_ms, 3_500);
        recording.complete();
        assert_eq!(recording.status, RecordingStatus::Completed);
        assert!(recording.completed_at_unix.is_some());
    }
}

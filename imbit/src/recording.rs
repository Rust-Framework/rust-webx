//! Recording control plane (2.4-2): resumable segment upload, playback index,
//! segment playback.
//!
//! Segments are stored through the [`IObjectStore`] port; the recording index
//! (status, ordered segments, totals) lives in [`RecordingRegistry`]. Uploading
//! the same `part` again is an idempotent retry (resume), completing marks the
//! recording playable.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use webx::*;

use crate::contracts::*;
use crate::store::ImStore;

/// Recording index (control-plane state).
#[derive(Default)]
pub struct RecordingRegistry {
    next_id: AtomicU64,
    recordings: Mutex<HashMap<String, RecordingMeta>>,
}

impl RecordingRegistry {
    /// Create an id for a new recording.
    fn next_recording_id(&self) -> String {
        format!("rec-{}", self.next_id.fetch_add(1, Ordering::Relaxed) + 1)
    }

    fn insert(&self, meta: RecordingMeta) {
        let mut recordings = self
            .recordings
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        recordings.insert(meta.id.clone(), meta);
    }

    fn get(&self, recording_id: &str) -> Option<RecordingMeta> {
        let recordings = self
            .recordings
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        recordings.get(recording_id).cloned()
    }

    fn update<F>(&self, recording_id: &str, update: F) -> Option<RecordingMeta>
    where
        F: FnOnce(&mut RecordingMeta),
    {
        let mut recordings = self
            .recordings
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let meta = recordings.get_mut(recording_id)?;
        update(meta);
        Some(meta.clone())
    }
}

fn status_label(status: RecordingStatus) -> &'static str {
    match status {
        RecordingStatus::Recording => "recording",
        RecordingStatus::Completed => "completed",
        RecordingStatus::Failed => "failed",
    }
}

fn view(meta: &RecordingMeta) -> RecordingView {
    RecordingView {
        id: meta.id.clone(),
        room_id: meta.room_id.clone(),
        owner_id: meta.owner_id.clone(),
        status: status_label(meta.status).to_string(),
        content_type: meta.content_type.clone(),
        total_size: meta.total_size,
        total_duration_ms: meta.total_duration_ms,
        created_at_unix: meta.created_at_unix,
        completed_at_unix: meta.completed_at_unix,
        segments: meta
            .segments
            .iter()
            .map(|segment| RecordingSegmentView {
                part: segment.part,
                size: segment.size,
                duration_ms: segment.duration_ms,
                started_at_unix: segment.started_at_unix,
            })
            .collect(),
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

fn require_member(store: &ImStore, room_id: &str, user_id: &str) -> Result<()> {
    if !store.is_member(room_id, user_id) {
        return Err(Error::forbidden(format!(
            "user is not a member of room '{room_id}'"
        )));
    }
    Ok(())
}

/// Storage key prefix of one recording.
pub fn recording_prefix(recording_id: &str) -> String {
    format!("recordings/{recording_id}")
}

/// Deterministic object key for one segment.
pub fn segment_key(recording_id: &str, part: u32) -> String {
    format!("{}/part-{part:05}", recording_prefix(recording_id))
}

#[derive(Inject)]
pub struct StartRecordingHandler {
    #[inject]
    store: Arc<ImStore>,
    #[inject]
    recordings: Arc<RecordingRegistry>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<StartRecordingRequest, RecordingView> for StartRecordingHandler {
    async fn handle(&mut self, req: StartRecordingRequest) -> ActionResult<RecordingView> {
        let room = self
            .store
            .room(&req.room_id)
            .ok_or_else(|| Error::not_found(format!("room '{}' not found", req.room_id)))?;
        require_member(&self.store, &room.id, &req.user.id)?;
        let content_type = req
            .content_type
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "audio/ogg".to_string());
        let id = self.recordings.next_recording_id();
        let meta = RecordingMeta::new(id, room.id, req.user.id, content_type);
        let view = view(&meta);
        self.recordings.insert(meta);
        Ok(view)
    }
}

#[derive(Inject)]
pub struct UploadSegmentHandler {
    #[inject]
    store: Arc<ImStore>,
    #[inject]
    recordings: Arc<RecordingRegistry>,
    #[inject]
    storage: Arc<dyn IObjectStore>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<UploadSegmentRequest, RecordingView> for UploadSegmentHandler {
    async fn handle(&mut self, req: UploadSegmentRequest) -> ActionResult<RecordingView> {
        let meta = self.recordings.get(&req.recording_id).ok_or_else(|| {
            Error::not_found(format!("recording '{}' not found", req.recording_id))
        })?;
        require_member(&self.store, &meta.room_id, &req.user.id)?;
        if meta.status != RecordingStatus::Recording {
            return Err(Error::conflict(format!(
                "recording '{}' is not accepting segments",
                req.recording_id
            )));
        }
        if req.part == 0 {
            return Err(Error::validation("part numbers start at 1"));
        }
        let bytes = data_encoding::BASE64
            .decode(req.data_base64.as_bytes())
            .map_err(|_| Error::validation("data_base64 is not valid base64"))?;
        let key = segment_key(&meta.id, req.part);
        let stored = self
            .storage
            .put(&key, bytes.clone(), &meta.content_type)
            .await?;
        let segment = RecordingSegment {
            part: req.part,
            key,
            size: stored.size,
            duration_ms: req.duration_ms,
            started_at_unix: req.started_at_unix.unwrap_or_else(now_unix),
        };
        let updated = self
            .recordings
            .update(&meta.id, |meta| {
                // Retrying a part replaces its previous entry (idempotent resume).
                meta.replace_segment(segment);
            })
            .ok_or_else(|| Error::not_found(format!("recording '{}' not found", meta.id)))?;
        Ok(view(&updated))
    }
}

#[derive(Inject)]
pub struct CompleteRecordingHandler {
    #[inject]
    store: Arc<ImStore>,
    #[inject]
    recordings: Arc<RecordingRegistry>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CompleteRecordingRequest, RecordingView> for CompleteRecordingHandler {
    async fn handle(&mut self, req: CompleteRecordingRequest) -> ActionResult<RecordingView> {
        let meta = self.recordings.get(&req.recording_id).ok_or_else(|| {
            Error::not_found(format!("recording '{}' not found", req.recording_id))
        })?;
        require_member(&self.store, &meta.room_id, &req.user.id)?;
        if meta.segments.is_empty() {
            return Err(Error::validation("recording has no uploaded segments"));
        }
        let updated = self
            .recordings
            .update(&req.recording_id, |meta| meta.complete())
            .ok_or_else(|| {
                Error::not_found(format!("recording '{}' not found", req.recording_id))
            })?;
        Ok(view(&updated))
    }
}

#[derive(Inject)]
pub struct GetRecordingHandler {
    #[inject]
    store: Arc<ImStore>,
    #[inject]
    recordings: Arc<RecordingRegistry>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<GetRecordingRequest, RecordingView> for GetRecordingHandler {
    async fn handle(&mut self, req: GetRecordingRequest) -> ActionResult<RecordingView> {
        let meta = self.recordings.get(&req.recording_id).ok_or_else(|| {
            Error::not_found(format!("recording '{}' not found", req.recording_id))
        })?;
        require_member(&self.store, &meta.room_id, &req.user.id)?;
        Ok(view(&meta))
    }
}

#[derive(Inject)]
pub struct GetSegmentHandler {
    #[inject]
    store: Arc<ImStore>,
    #[inject]
    recordings: Arc<RecordingRegistry>,
    #[inject]
    storage: Arc<dyn IObjectStore>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<GetSegmentRequest, Response> for GetSegmentHandler {
    async fn handle(&mut self, req: GetSegmentRequest) -> ActionResult<Response> {
        let meta = self.recordings.get(&req.recording_id).ok_or_else(|| {
            Error::not_found(format!("recording '{}' not found", req.recording_id))
        })?;
        require_member(&self.store, &meta.room_id, &req.user.id)?;
        let segment = meta
            .segments
            .iter()
            .find(|segment| segment.part == req.part)
            .ok_or_else(|| {
                Error::not_found(format!(
                    "recording '{}' has no segment {}",
                    req.recording_id, req.part
                ))
            })?;
        let object = self
            .storage
            .get(&segment.key)
            .await?
            .ok_or_else(|| Error::not_found("segment object is missing from storage"))?;
        Ok(Box::new(ContentResult::bytes(
            object.bytes,
            meta.content_type.clone(),
        )))
    }
}

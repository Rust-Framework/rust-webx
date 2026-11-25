//! In-process recording control plane.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

use webx_core::storage::RecordingSegment;

use crate::model::Recording;
use crate::StoreError;

/// Recording index (control-plane state) of the reference app.
///
/// Segment bytes live behind the `IObjectStore` port; this registry only keeps
/// the aggregates. The durable variant keeps the same four methods on top of a
/// `rust-ef` `DbContext`.
#[derive(Default)]
pub struct RecordingRegistry {
    next_id: AtomicU64,
    recordings: Mutex<HashMap<String, Recording>>,
}

impl RecordingRegistry {
    fn lock(&self) -> MutexGuard<'_, HashMap<String, Recording>> {
        self.recordings
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn next_recording_id(&self) -> String {
        format!("rec-{}", self.next_id.fetch_add(1, Ordering::Relaxed) + 1)
    }

    /// Start the control plane of a recording; the caller has already checked
    /// room membership.
    pub fn start(
        &self,
        room_id: &str,
        owner_id: &str,
        content_type: &str,
        created_at_unix: u64,
    ) -> Recording {
        let recording = Recording::start(
            self.next_recording_id(),
            room_id,
            owner_id,
            content_type,
            created_at_unix,
        );
        self.lock()
            .insert(recording.id().to_string(), recording.clone());
        recording
    }

    /// Load a recording or report a typed `404`.
    pub fn find(&self, recording_id: &str) -> Result<Recording, StoreError> {
        self.lock()
            .get(recording_id)
            .cloned()
            .ok_or_else(|| recording_not_found(recording_id))
    }

    /// Insert or replace one part; retrying a part resumes the upload.
    pub fn put_segment(
        &self,
        recording_id: &str,
        segment: RecordingSegment,
    ) -> Result<Recording, StoreError> {
        self.modify(recording_id, |recording| {
            recording.put_segment(segment);
            Ok(())
        })
    }

    /// Mark the recording playable (a segment-less recording is rejected by the
    /// aggregate).
    pub fn complete(&self, recording_id: &str, now_unix: u64) -> Result<Recording, StoreError> {
        self.modify(recording_id, |recording| recording.complete(now_unix))
    }

    fn modify<F>(&self, recording_id: &str, change: F) -> Result<Recording, StoreError>
    where
        F: FnOnce(&mut Recording) -> Result<(), StoreError>,
    {
        let mut recordings = self.lock();
        let recording = recordings
            .get_mut(recording_id)
            .ok_or_else(|| recording_not_found(recording_id))?;
        change(recording)?;
        Ok(recording.clone())
    }
}

fn recording_not_found(recording_id: &str) -> StoreError {
    StoreError::not_found(format!("recording '{recording_id}' not found"))
}

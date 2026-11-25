//! Recording aggregate: resumable segment upload and the playback index.

use webx_core::storage::{RecordingSegment, RecordingStatus};

use crate::StoreError;

/// Storage key prefix of one recording (shared by upload and playback).
pub fn recording_prefix(recording_id: &str) -> String {
    format!("recordings/{recording_id}")
}

/// Deterministic object key of one segment.
pub fn segment_key(recording_id: &str, part: u32) -> String {
    format!("{}/part-{part:05}", recording_prefix(recording_id))
}

/// A recording and its ordered playback index.
///
/// Totals are derived from the index instead of being stored, so a retried part
/// can never inflate the recording size or duration. The segment bytes
/// themselves live behind the `IObjectStore` port; this aggregate owns the
/// control plane only.
#[derive(Debug, Clone)]
pub struct Recording {
    id: String,
    room_id: String,
    owner_id: String,
    status: RecordingStatus,
    content_type: String,
    segments: Vec<RecordingSegment>,
    created_at_unix: u64,
    completed_at_unix: Option<u64>,
}

impl Recording {
    /// Start the control plane of a recording.
    pub fn start(
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
            created_at_unix,
            completed_at_unix: None,
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn room_id(&self) -> &str {
        &self.room_id
    }

    pub fn owner_id(&self) -> &str {
        &self.owner_id
    }

    pub fn status(&self) -> RecordingStatus {
        self.status
    }

    /// Media type of every segment of this recording.
    pub fn content_type(&self) -> &str {
        &self.content_type
    }

    /// The playback index, ordered by part.
    pub fn segments(&self) -> &[RecordingSegment] {
        &self.segments
    }

    pub fn created_at_unix(&self) -> u64 {
        self.created_at_unix
    }

    pub fn completed_at_unix(&self) -> Option<u64> {
        self.completed_at_unix
    }

    /// Whether segments may still be uploaded.
    pub fn is_accepting_segments(&self) -> bool {
        self.status == RecordingStatus::Recording
    }

    /// Insert or replace one part (a retried part resumes the upload).
    pub fn put_segment(&mut self, segment: RecordingSegment) {
        match self
            .segments
            .iter_mut()
            .find(|existing| existing.part == segment.part)
        {
            Some(existing) => *existing = segment,
            None => self.segments.push(segment),
        }
        self.segments.sort_by_key(|segment| segment.part);
    }

    pub fn segment(&self, part: u32) -> Option<&RecordingSegment> {
        self.segments.iter().find(|segment| segment.part == part)
    }

    /// Mark the recording playable.
    ///
    /// A recording without segments is not a recording: completing it is a
    /// domain violation (`400`), not a state change.
    pub fn complete(&mut self, now_unix: u64) -> Result<(), StoreError> {
        if self.segments.is_empty() {
            return Err(StoreError::validation("recording has no uploaded segments"));
        }
        self.status = RecordingStatus::Completed;
        self.completed_at_unix = Some(now_unix);
        Ok(())
    }

    pub fn total_size(&self) -> u64 {
        self.segments.iter().map(|segment| segment.size).sum()
    }

    pub fn total_duration_ms(&self) -> u64 {
        self.segments
            .iter()
            .map(|segment| segment.duration_ms)
            .sum()
    }
}

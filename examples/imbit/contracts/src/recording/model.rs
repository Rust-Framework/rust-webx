//! Read models of a recording and its playback index.

use serde::Serialize;

/// Control-plane state plus the ordered playback index of one recording.
#[derive(Debug, Clone, Serialize)]
pub struct RecordingModel {
    pub id: String,
    pub room_id: String,
    pub owner_id: String,
    /// `recording` | `completed` | `failed`.
    pub status: String,
    /// Media type of every segment (`audio/ogg`, `video/webm`, ...).
    pub content_type: String,
    pub total_size: u64,
    pub total_duration_ms: u64,
    pub created_at_unix: u64,
    pub completed_at_unix: Option<u64>,
    /// Segments in playback order.
    pub segments: Vec<RecordingSegmentModel>,
}

/// One segment of the playback index (the bytes live in the object store).
#[derive(Debug, Clone, Serialize)]
pub struct RecordingSegmentModel {
    /// 1-based part number.
    pub part: u32,
    pub size: u64,
    pub duration_ms: u64,
    pub started_at_unix: u64,
}

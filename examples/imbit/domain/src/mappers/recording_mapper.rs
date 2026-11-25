//! Recording → `RecordingModel`.

use imbit_contracts::recording::{RecordingModel, RecordingSegmentModel};
use webx_core::storage::{RecordingSegment, RecordingStatus};

use crate::mappers::ToModel;
use crate::model::Recording;

impl ToModel<RecordingModel> for &Recording {
    fn to_model(self) -> RecordingModel {
        RecordingModel {
            id: self.id().to_string(),
            room_id: self.room_id().to_string(),
            owner_id: self.owner_id().to_string(),
            status: status_label(self.status()).to_string(),
            content_type: self.content_type().to_string(),
            total_size: self.total_size(),
            total_duration_ms: self.total_duration_ms(),
            created_at_unix: self.created_at_unix(),
            completed_at_unix: self.completed_at_unix(),
            segments: self.segments().iter().map(ToModel::to_model).collect(),
        }
    }
}

impl ToModel<RecordingSegmentModel> for &RecordingSegment {
    fn to_model(self) -> RecordingSegmentModel {
        RecordingSegmentModel {
            part: self.part,
            size: self.size,
            duration_ms: self.duration_ms,
            started_at_unix: self.started_at_unix,
        }
    }
}

/// Wire label of the recording state (`recording` | `completed` | `failed`).
fn status_label(status: RecordingStatus) -> &'static str {
    match status {
        RecordingStatus::Recording => "recording",
        RecordingStatus::Completed => "completed",
        RecordingStatus::Failed => "failed",
    }
}

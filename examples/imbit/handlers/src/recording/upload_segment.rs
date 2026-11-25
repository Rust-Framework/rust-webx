//! `POST /api/recordings/{recording_id}/parts/{part}` — upload one segment.
//!
//! The bytes go through the `IObjectStore` port; the aggregate keeps only the
//! playback index. Re-uploading a part replaces its entry (resume path).

use std::sync::Arc;

use webx::*;

use imbit_contracts::recording::{RecordingModel, UploadSegmentRequest};
use imbit_domain::mappers::ToModel;
use imbit_domain::model::segment_key;
use imbit_domain::{ImStore, RecordingRegistry, StoreError};

#[derive(Inject)]
pub struct UploadSegmentHandler {
    #[inject]
    store: Arc<ImStore>,
    #[inject]
    recordings: Arc<RecordingRegistry>,
    #[inject]
    storage: Arc<dyn IObjectStore>,
    #[inject]
    clock: Arc<dyn IClock>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<UploadSegmentRequest, RecordingModel> for UploadSegmentHandler {
    async fn handle(&mut self, request: UploadSegmentRequest) -> ActionResult<RecordingModel> {
        let recording = self.recordings.find(&request.recording_id)?;
        self.store
            .require_member(recording.room_id(), &request.user.id)?;
        if !recording.is_accepting_segments() {
            return Err(StoreError::conflict(format!(
                "recording '{}' is not accepting segments",
                request.recording_id
            ))
            .into());
        }
        if request.part == 0 {
            return Err(StoreError::validation("part numbers start at 1").into());
        }

        let bytes = data_encoding::BASE64
            .decode(request.data_base64.as_bytes())
            .map_err(|_| StoreError::validation("data_base64 is not valid base64"))?;
        let key = segment_key(&request.recording_id, request.part);
        let stored = self
            .storage
            .put(&key, bytes, recording.content_type())
            .await?;

        let segment = RecordingSegment {
            part: request.part,
            key,
            size: stored.size,
            duration_ms: request.duration_ms,
            started_at_unix: request
                .started_at_unix
                .unwrap_or_else(|| self.clock.now_unix()),
        };
        let recording = self
            .recordings
            .put_segment(&request.recording_id, segment)?;

        Ok(recording.to_model())
    }
}

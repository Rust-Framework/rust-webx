//! `GET /api/recordings/{recording_id}/parts/{part}` — play one segment.
//!
//! Playback is a non-JSON body, so the endpoint declares `IRequest<Response>`
//! and the handler answers with a `ContentResult` carrying the recording's
//! media type.

use std::sync::Arc;

use webx::*;

use imbit_contracts::recording::GetSegmentRequest;
use imbit_domain::{ImStore, RecordingRegistry, StoreError};

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
    async fn handle(&mut self, request: GetSegmentRequest) -> ActionResult<Response> {
        let recording = self.recordings.find(&request.recording_id)?;
        self.store
            .require_member(recording.room_id(), &request.user.id)?;
        let key = recording
            .segment(request.part)
            .map(|segment| segment.key.clone())
            .ok_or_else(|| {
                StoreError::not_found(format!(
                    "recording '{}' has no segment {}",
                    request.recording_id, request.part
                ))
            })?;

        let object = self
            .storage
            .get(&key)
            .await?
            .ok_or_else(|| StoreError::not_found("segment object is missing from storage"))?;

        Ok(Box::new(ContentResult::bytes(
            object.bytes,
            recording.content_type().to_string(),
        )))
    }
}

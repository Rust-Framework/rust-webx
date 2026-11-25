//! `POST /api/recordings` — start the control plane for a room recording.

use std::sync::Arc;

use webx::*;

use imbit_contracts::recording::{RecordingModel, StartRecordingRequest};
use imbit_domain::mappers::ToModel;
use imbit_domain::{ImStore, RecordingRegistry};

/// Opus-in-Ogg is what the reference client records.
const DEFAULT_CONTENT_TYPE: &str = "audio/ogg";

#[derive(Inject)]
pub struct StartRecordingHandler {
    #[inject]
    store: Arc<ImStore>,
    #[inject]
    recordings: Arc<RecordingRegistry>,
    #[inject]
    clock: Arc<dyn IClock>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<StartRecordingRequest, RecordingModel> for StartRecordingHandler {
    async fn handle(&mut self, request: StartRecordingRequest) -> ActionResult<RecordingModel> {
        // Only a member of the room may record it (`404` room / `403` member).
        self.store
            .require_member(&request.room_id, &request.user.id)?;

        let content_type = request
            .content_type
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_CONTENT_TYPE.to_string());
        let recording = self.recordings.start(
            &request.room_id,
            &request.user.id,
            &content_type,
            self.clock.now_unix(),
        );

        Ok(recording.to_model())
    }
}

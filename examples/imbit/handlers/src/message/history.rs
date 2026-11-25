//! `GET /api/rooms/{room_id}/messages` — offline catch-up over HTTP.
//!
//! Same message log and read model as the WebSocket `history` frame, so a
//! client that cannot hold a connection still catches up.

use std::sync::Arc;

use webx::*;

use imbit_contracts::message::{MessageHistoryModel, MessageHistoryRequest, MessageModel};
use imbit_domain::mappers::ToModel;
use imbit_domain::ImStore;

use crate::common::CATCH_UP_LIMIT;

#[derive(Inject)]
pub struct MessageHistoryHandler {
    #[inject]
    store: Arc<ImStore>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<MessageHistoryRequest, MessageHistoryModel> for MessageHistoryHandler {
    async fn handle(
        &mut self,
        request: MessageHistoryRequest,
    ) -> ActionResult<MessageHistoryModel> {
        let messages: Vec<MessageModel> = self
            .store
            .history(
                &request.room_id,
                &request.user.id,
                request.after_seq.unwrap_or(0),
                CATCH_UP_LIMIT,
            )?
            .iter()
            .map(ToModel::to_model)
            .collect();

        Ok(MessageHistoryModel {
            room_id: request.room_id,
            messages,
        })
    }
}

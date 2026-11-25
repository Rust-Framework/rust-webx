//! `GET /api/rooms/{room_id}/messages` — offline catch-up over HTTP.
//!
//! Mirrors the WebSocket `history` frame from the same message log, so a client
//! that cannot hold a connection still catches up.

use serde::{Deserialize, Serialize};
use webx::*;

use super::model::MessageModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct MessageHistoryRequest {
    #[fromclaim]
    pub user: IdentityUser,
    #[frompath]
    pub room_id: String,
    /// Return messages with a greater `seq` (0 = from the start).
    #[fromquery]
    pub after_seq: Option<u64>,
}

/// One catch-up page, oldest first.
#[derive(Debug, Serialize)]
pub struct MessageHistoryModel {
    pub room_id: String,
    pub messages: Vec<MessageModel>,
}

#[get("/api/rooms/{room_id}/messages")]
impl IRequest<MessageHistoryModel> for MessageHistoryRequest {}

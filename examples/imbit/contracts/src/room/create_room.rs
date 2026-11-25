//! `POST /api/rooms` — create a room owned by the caller.

use serde::Deserialize;
use webx::*;

use super::model::RoomModel;

/// The owner comes from the verified token; the body only names the room.
#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct CreateRoomRequest {
    #[fromclaim]
    pub user: IdentityUser,
    /// Display name; an empty name falls back to `room-<id>`.
    pub name: String,
}

#[post("/api/rooms")]
impl IRequest<RoomModel> for CreateRoomRequest {}

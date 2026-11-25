//! `POST /api/rooms/{room_id}/join` — join an existing room.

use serde::Deserialize;
use webx::*;

use super::model::RoomModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct JoinRoomRequest {
    #[fromclaim]
    pub user: IdentityUser,
    #[frompath]
    pub room_id: String,
}

#[post("/api/rooms/{room_id}/join")]
impl IRequest<RoomModel> for JoinRoomRequest {}

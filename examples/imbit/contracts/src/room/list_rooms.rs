//! `GET /api/rooms` — the rooms the caller belongs to.

use serde::Deserialize;
use webx::*;

use super::model::RoomModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct ListRoomsRequest {
    #[fromclaim]
    pub user: IdentityUser,
}

#[get("/api/rooms")]
impl IRequest<Vec<RoomModel>> for ListRoomsRequest {}

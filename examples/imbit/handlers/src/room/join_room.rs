//! `POST /api/rooms/{room_id}/join` — join an existing room.

use std::sync::Arc;

use webx::*;

use imbit_contracts::room::{JoinRoomRequest, RoomModel};
use imbit_domain::mappers::ToModel;
use imbit_domain::ImStore;

#[derive(Inject)]
pub struct JoinRoomHandler {
    #[inject]
    store: Arc<ImStore>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<JoinRoomRequest, RoomModel> for JoinRoomHandler {
    async fn handle(&mut self, request: JoinRoomRequest) -> ActionResult<RoomModel> {
        let room = self.store.join_room(
            &request.room_id,
            &request.user.id,
            request.user.display_name(),
        )?;

        Ok(room.to_model())
    }
}

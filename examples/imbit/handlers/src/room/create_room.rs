//! `POST /api/rooms` — create a room owned by the authenticated caller.

use std::sync::Arc;

use webx::*;

use imbit_contracts::room::{CreateRoomRequest, RoomModel};
use imbit_domain::mappers::ToModel;
use imbit_domain::ImStore;

#[derive(Inject)]
pub struct CreateRoomHandler {
    #[inject]
    store: Arc<ImStore>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateRoomRequest, RoomModel> for CreateRoomHandler {
    async fn handle(&mut self, request: CreateRoomRequest) -> ActionResult<RoomModel> {
        let user = &request.user;
        let room = self
            .store
            .create_room(&user.id, user.display_name(), &request.name);

        Ok(room.to_model())
    }
}

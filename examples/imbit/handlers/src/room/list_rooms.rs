//! `GET /api/rooms` — the rooms the authenticated caller belongs to.

use std::sync::Arc;

use webx::*;

use imbit_contracts::room::{ListRoomsRequest, RoomModel};
use imbit_domain::mappers::ToModel;
use imbit_domain::ImStore;

#[derive(Inject)]
pub struct ListRoomsHandler {
    #[inject]
    store: Arc<ImStore>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListRoomsRequest, Vec<RoomModel>> for ListRoomsHandler {
    async fn handle(&mut self, request: ListRoomsRequest) -> ActionResult<Vec<RoomModel>> {
        let rooms = self
            .store
            .rooms_of(&request.user.id)
            .iter()
            .map(ToModel::to_model)
            .collect();

        Ok(rooms)
    }
}

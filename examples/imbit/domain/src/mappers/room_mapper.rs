//! Room → `RoomModel`.

use imbit_contracts::room::RoomModel;

use crate::mappers::ToModel;
use crate::model::Room;

impl ToModel<RoomModel> for &Room {
    fn to_model(self) -> RoomModel {
        RoomModel {
            id: self.id().to_string(),
            name: self.name().to_string(),
            owner_id: self.owner_id().to_string(),
            members: self.member_names(),
        }
    }
}

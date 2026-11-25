//! Message → `MessageModel`.

use imbit_contracts::message::MessageModel;

use crate::mappers::ToModel;
use crate::model::Message;

impl ToModel<MessageModel> for &Message {
    fn to_model(self) -> MessageModel {
        MessageModel {
            room_id: self.room_id.clone(),
            seq: self.seq,
            from: self.from_name.clone(),
            text: self.text.clone(),
            client_msg_id: self.client_msg_id.clone(),
            at_unix: self.at_unix,
        }
    }
}

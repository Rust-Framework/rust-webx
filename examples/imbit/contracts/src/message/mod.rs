//! Message context: the read model and the offline catch-up operation.

pub mod history;
pub mod model;

pub use history::{MessageHistoryModel, MessageHistoryRequest};
pub use model::MessageModel;

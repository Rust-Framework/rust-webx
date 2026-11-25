//! Read model of a room.

use serde::Serialize;

/// A room as clients see it: identity, owner and the member display names.
#[derive(Debug, Clone, Serialize)]
pub struct RoomModel {
    pub id: String,
    pub name: String,
    pub owner_id: String,
    /// Member display names, sorted for a stable payload.
    pub members: Vec<String>,
}

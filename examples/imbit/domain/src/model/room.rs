//! Room aggregate: ownership, membership and the per-room message sequence.

use std::collections::BTreeMap;

/// A room and its members.
///
/// Members are keyed by user id and keep the display name observed at join
/// time: the id is the stable identity (permissions, presence, signaling
/// targets) while the name is what clients render.
#[derive(Debug, Clone)]
pub struct Room {
    id: String,
    name: String,
    owner_id: String,
    members: BTreeMap<String, String>,
    next_seq: u64,
}

impl Room {
    /// Create a room owned by `owner_id`; an empty name falls back to
    /// `room-<id>` so a room always has a display name.
    pub fn create(id: impl Into<String>, name: &str, owner_id: &str, owner_name: &str) -> Self {
        let id = id.into();
        let name = match name.trim() {
            "" => format!("room-{id}"),
            trimmed => trimmed.to_string(),
        };
        Self {
            id,
            name,
            owner_id: owner_id.to_string(),
            members: BTreeMap::from([(owner_id.to_string(), owner_name.to_string())]),
            next_seq: 0,
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn owner_id(&self) -> &str {
        &self.owner_id
    }

    /// The display name of `user_id`, when that user is a member.
    pub fn username_of(&self, user_id: &str) -> Option<&str> {
        self.members.get(user_id).map(String::as_str)
    }

    pub fn is_member(&self, user_id: &str) -> bool {
        self.members.contains_key(user_id)
    }

    /// Add a member (idempotent) and report the member count.
    pub fn join(&mut self, user_id: &str, user_name: &str) -> usize {
        self.members
            .insert(user_id.to_string(), user_name.to_string());
        self.members.len()
    }

    pub fn member_count(&self) -> usize {
        self.members.len()
    }

    /// Member display names, sorted for a stable presence/room payload.
    pub fn member_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.members.values().cloned().collect();
        names.sort();
        names
    }

    /// Member user ids in stable (sorted) order.
    ///
    /// The id is the address presence and permissions use: a room's online
    /// members are computed by filtering these ids through the shared online
    /// presence store, never by matching display names.
    pub fn member_ids(&self) -> Vec<String> {
        self.members.keys().cloned().collect()
    }

    /// Allocate the next gapless sequence number of this room's log.
    pub fn allocate_seq(&mut self) -> u64 {
        self.next_seq += 1;
        self.next_seq
    }
}

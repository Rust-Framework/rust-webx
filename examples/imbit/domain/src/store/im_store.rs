//! In-process IM repository: users, rooms, message logs and deduplication.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, MutexGuard};

use crate::model::{Message, Room, User};
use crate::StoreError;

/// One posted message plus whether it answered a retry.
#[derive(Debug, Clone)]
pub struct PostedMessage {
    pub message: Message,
    /// `true` when the client message id was already stored (idempotent retry).
    pub duplicate: bool,
}

#[derive(Default)]
struct StoreState {
    next_user: u64,
    next_room: u64,
    /// username → user (login lookup).
    users: HashMap<String, User>,
    /// user id → username (display names, signaling targets).
    usernames: HashMap<String, String>,
    rooms: BTreeMap<String, Room>,
    /// room id → append-only message log.
    messages: HashMap<String, Vec<Message>>,
    /// (room, user, client_msg_id) → server seq (at-least-once retries).
    dedupe: HashMap<(String, String, String), u64>,
}

/// Process-local IM store.
#[derive(Default)]
pub struct ImStore {
    state: Mutex<StoreState>,
}

impl ImStore {
    fn lock(&self) -> MutexGuard<'_, StoreState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Login credential lookup (`None` = first login for this username).
    pub fn credentials(&self, username: &str) -> Option<(String, String)> {
        self.lock()
            .users
            .get(username)
            .map(|user| (user.id().to_string(), user.password_hash().to_string()))
    }

    /// Register a user and return the assigned id.
    pub fn register_user(&self, username: &str, password_hash: String) -> String {
        let mut state = self.lock();
        state.next_user += 1;
        let user = User::new(format!("u{}", state.next_user), username, password_hash);
        state
            .usernames
            .insert(user.id().to_string(), user.username().to_string());
        state
            .users
            .insert(user.username().to_string(), user.clone());
        user.id().to_string()
    }

    /// Display name of a user id.
    pub fn username_of(&self, user_id: &str) -> Option<String> {
        self.lock().usernames.get(user_id).cloned()
    }

    /// User id of a username (signaling targets are usernames on the wire).
    pub fn user_id_of(&self, username: &str) -> Option<String> {
        self.lock()
            .users
            .get(username)
            .map(|user| user.id().to_string())
    }

    /// Create a room owned by `owner_id`.
    pub fn create_room(&self, owner_id: &str, owner_name: &str, name: &str) -> Room {
        let mut state = self.lock();
        state.next_room += 1;
        let room = Room::create(format!("r{}", state.next_room), name, owner_id, owner_name);
        state.rooms.insert(room.id().to_string(), room.clone());
        room
    }

    /// Add a member (idempotent) and return the updated room.
    pub fn join_room(
        &self,
        room_id: &str,
        user_id: &str,
        user_name: &str,
    ) -> Result<Room, StoreError> {
        let mut state = self.lock();
        let room = state
            .rooms
            .get_mut(room_id)
            .ok_or_else(|| room_not_found(room_id))?;
        room.join(user_id, user_name);
        Ok(room.clone())
    }

    /// Rooms the user belongs to, ordered by id.
    pub fn rooms_of(&self, user_id: &str) -> Vec<Room> {
        self.lock()
            .rooms
            .values()
            .filter(|room| room.is_member(user_id))
            .cloned()
            .collect()
    }

    /// Load a room or report a typed `404`.
    pub fn find_room(&self, room_id: &str) -> Result<Room, StoreError> {
        self.lock()
            .rooms
            .get(room_id)
            .cloned()
            .ok_or_else(|| room_not_found(room_id))
    }

    /// Load a room and require `user_id` to be a member (`404` / `403`).
    ///
    /// The room rule lives here rather than in every handler: a room exists for
    /// its members only, and "not a member" must never be reported as "missing".
    pub fn require_member(&self, room_id: &str, user_id: &str) -> Result<Room, StoreError> {
        let room = self.find_room(room_id)?;
        if !room.is_member(user_id) {
            return Err(not_a_member(room_id));
        }
        Ok(room)
    }

    /// Append a message; a known `(room, user, client_msg_id)` answers the
    /// stored message instead of appending a second copy.
    pub fn post_message(
        &self,
        room_id: &str,
        user_id: &str,
        user_name: &str,
        client_msg_id: &str,
        text: &str,
        at_unix: u64,
    ) -> Result<PostedMessage, StoreError> {
        let mut state = self.lock();
        let dedupe_key = (
            room_id.to_string(),
            user_id.to_string(),
            client_msg_id.to_string(),
        );

        if let Some(seq) = state.dedupe.get(&dedupe_key).copied() {
            let message = state
                .messages
                .get(room_id)
                .and_then(|messages| messages.iter().find(|message| message.seq == seq))
                .cloned()
                .ok_or_else(|| {
                    StoreError::conflict("the deduplicated message is missing from the log")
                })?;
            return Ok(PostedMessage {
                message,
                duplicate: true,
            });
        }

        let room = state
            .rooms
            .get_mut(room_id)
            .ok_or_else(|| room_not_found(room_id))?;
        if !room.is_member(user_id) {
            return Err(not_a_member(room_id));
        }
        let seq = room.allocate_seq();

        let message = Message {
            room_id: room_id.to_string(),
            seq,
            from_user: user_id.to_string(),
            from_name: user_name.to_string(),
            client_msg_id: client_msg_id.to_string(),
            text: text.to_string(),
            at_unix,
        };
        state
            .messages
            .entry(room_id.to_string())
            .or_default()
            .push(message.clone());
        state.dedupe.insert(dedupe_key, seq);
        Ok(PostedMessage {
            message,
            duplicate: false,
        })
    }

    /// Messages after `after_seq`, oldest first (offline catch-up).
    pub fn history(
        &self,
        room_id: &str,
        user_id: &str,
        after_seq: u64,
        limit: usize,
    ) -> Result<Vec<Message>, StoreError> {
        let state = self.lock();
        let room = state
            .rooms
            .get(room_id)
            .ok_or_else(|| room_not_found(room_id))?;
        if !room.is_member(user_id) {
            return Err(not_a_member(room_id));
        }
        Ok(state
            .messages
            .get(room_id)
            .into_iter()
            .flatten()
            .filter(|message| message.seq > after_seq)
            .take(limit)
            .cloned()
            .collect())
    }
}

fn room_not_found(room_id: &str) -> StoreError {
    StoreError::not_found(format!("room '{room_id}' not found"))
}

fn not_a_member(room_id: &str) -> StoreError {
    StoreError::forbidden(format!("user is not a member of room '{room_id}'"))
}

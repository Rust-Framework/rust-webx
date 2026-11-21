//! In-memory IM domain state for the reference application.
//!
//! The app keeps users, rooms, per-room message logs and client-message-id
//! deduplication in process memory so the reference stays runnable without a
//! database. The durable variant swaps this module for a `rust-ef` DbContext;
//! the handler surface (login / rooms / messages / catch-up) does not change.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug)]
pub struct UserRecord {
    pub user_id: String,
    pub username: String,
    pub password_hash: String,
}

#[derive(Clone, Debug)]
pub struct RoomRecord {
    pub id: String,
    pub name: String,
    pub owner_id: String,
    pub members: HashSet<String>,
    pub next_seq: u64,
}

#[derive(Clone, Debug)]
pub struct StoredMessage {
    pub room_id: String,
    pub seq: u64,
    pub from_user: String,
    pub from_name: String,
    pub client_msg_id: String,
    pub text: String,
    pub at_unix: u64,
}

#[derive(Debug)]
pub struct PostedMessage {
    pub message: StoredMessage,
    /// `true` when the client message id was already stored (idempotent retry).
    pub duplicate: bool,
}

#[derive(Default)]
struct StoreState {
    next_user: u64,
    next_room: u64,
    users: HashMap<String, UserRecord>,
    user_ids: HashMap<String, String>,
    rooms: HashMap<String, RoomRecord>,
    messages: HashMap<String, Vec<StoredMessage>>,
    /// (room, user, client_msg_id) -> server seq, for at-least-once retries.
    dedupe: HashMap<(String, String, String), u64>,
}

/// Process-local IM store.
#[derive(Default)]
pub struct ImStore {
    state: Mutex<StoreState>,
}

impl ImStore {
    fn lock(&self) -> std::sync::MutexGuard<'_, StoreState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Login credential lookup (`None` = first login for this username).
    pub fn credentials(&self, username: &str) -> Option<(String, String)> {
        self.lock()
            .users
            .get(username)
            .map(|user| (user.user_id.clone(), user.password_hash.clone()))
    }

    /// Create a user and return its id.
    pub fn create_user(&self, username: &str, password_hash: String) -> String {
        let mut state = self.lock();
        state.next_user += 1;
        let user_id = format!("u{}", state.next_user);
        let record = UserRecord {
            user_id: user_id.clone(),
            username: username.to_string(),
            password_hash,
        };
        state.user_ids.insert(user_id.clone(), username.to_string());
        state.users.insert(username.to_string(), record);
        user_id
    }

    /// Display name for a user id.
    pub fn username_of(&self, user_id: &str) -> Option<String> {
        self.lock().user_ids.get(user_id).cloned()
    }

    /// User id for a username (signaling targets are usernames on the wire).
    pub fn user_id_of(&self, username: &str) -> Option<String> {
        self.lock()
            .users
            .get(username)
            .map(|user| user.user_id.clone())
    }

    /// Whether `user_id` is a member of `room_id`.
    pub fn is_member(&self, room_id: &str, user_id: &str) -> bool {
        self.lock()
            .rooms
            .get(room_id)
            .is_some_and(|room| room.members.contains(user_id))
    }

    /// Create a room owned by `owner_id`.
    pub fn create_room(&self, owner_id: &str, owner_name: &str, name: &str) -> RoomRecord {
        let mut state = self.lock();
        state.next_room += 1;
        let room = RoomRecord {
            id: format!("r{}", state.next_room),
            name: if name.trim().is_empty() {
                format!("room-{}", state.next_room)
            } else {
                name.trim().to_string()
            },
            owner_id: owner_id.to_string(),
            members: HashSet::from([owner_id.to_string()]),
            next_seq: 0,
        };
        let _ = owner_name;
        state.rooms.insert(room.id.clone(), room.clone());
        room
    }

    /// Add a user to a room; returns member count.
    pub fn join_room(&self, room_id: &str, user_id: &str) -> Result<usize, String> {
        let mut state = self.lock();
        let room = state
            .rooms
            .get_mut(room_id)
            .ok_or_else(|| format!("room '{room_id}' not found"))?;
        room.members.insert(user_id.to_string());
        Ok(room.members.len())
    }

    /// Rooms the user belongs to.
    pub fn rooms_of(&self, user_id: &str) -> Vec<RoomRecord> {
        let state = self.lock();
        let mut rooms: Vec<RoomRecord> = state
            .rooms
            .values()
            .filter(|room| room.members.contains(user_id))
            .cloned()
            .collect();
        rooms.sort_by(|left, right| left.id.cmp(&right.id));
        rooms
    }

    /// Room lookup.
    pub fn room(&self, room_id: &str) -> Option<RoomRecord> {
        self.lock().rooms.get(room_id).cloned()
    }

    /// Append a message (idempotent per `(room, user, client_msg_id)`).
    pub fn post_message(
        &self,
        room_id: &str,
        user_id: &str,
        user_name: &str,
        client_msg_id: &str,
        text: &str,
    ) -> Result<PostedMessage, String> {
        let mut state = self.lock();
        let room = state
            .rooms
            .get(room_id)
            .ok_or_else(|| format!("room '{room_id}' not found"))?;
        if !room.members.contains(user_id) {
            return Err(format!("user is not a member of room '{room_id}'"));
        }
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
                .ok_or_else(|| "deduplicated message is missing from the log".to_string())?;
            return Ok(PostedMessage {
                message,
                duplicate: true,
            });
        }
        let room = state.rooms.get_mut(room_id).expect("room checked above");
        room.next_seq += 1;
        let message = StoredMessage {
            room_id: room_id.to_string(),
            seq: room.next_seq,
            from_user: user_id.to_string(),
            from_name: user_name.to_string(),
            client_msg_id: client_msg_id.to_string(),
            text: text.to_string(),
            at_unix: now_unix(),
        };
        state
            .messages
            .entry(room_id.to_string())
            .or_default()
            .push(message.clone());
        state.dedupe.insert(dedupe_key, message.seq);
        Ok(PostedMessage {
            message,
            duplicate: false,
        })
    }

    /// Messages after `after_seq` (offline catch-up), oldest first.
    pub fn history(
        &self,
        room_id: &str,
        user_id: &str,
        after_seq: u64,
        limit: usize,
    ) -> Result<Vec<StoredMessage>, String> {
        let state = self.lock();
        let room = state
            .rooms
            .get(room_id)
            .ok_or_else(|| format!("room '{room_id}' not found"))?;
        if !room.members.contains(user_id) {
            return Err(format!("user is not a member of room '{room_id}'"));
        }
        let messages = state.messages.get(room_id).cloned().unwrap_or_default();
        Ok(messages
            .into_iter()
            .filter(|message| message.seq > after_seq)
            .take(limit)
            .collect())
    }

    /// Member display names of a room.
    pub fn member_names(&self, room_id: &str) -> Vec<String> {
        let state = self.lock();
        let mut names: Vec<String> = state
            .rooms
            .get(room_id)
            .map(|room| {
                room.members
                    .iter()
                    .map(|member| state.user_ids.get(member).cloned().unwrap_or_default())
                    .filter(|name| !name.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

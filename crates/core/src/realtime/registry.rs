//! Live-connection registry (2.3-4a).
//!
//! Process-local indexes over the connections the host accepted:
//!
//! * by connection id (`get`/`register`/`unregister`),
//! * by authenticated user (`user_connections`, presence),
//! * by room (`join`/`leave`/`room_connections`).
//!
//! Fan-out code (chat rooms, presence, notifications) resolves
//! [`IConnectionRegistry`] from DI and calls the
//! `*_connections` helpers; it never needs to own a socket. Cross-node fan-out
//! is layered on top with `RealtimeFanoutEvent` over the event transport
//! (2.3-4b: the event bus is the single cross-process transport).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::realtime::{ConnectionId, IConnection};

/// Presence state of a user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresenceStatus {
    /// At least one live connection and no away marker.
    Online,
    /// At least one live connection plus an explicit away marker (still valid).
    Away,
    /// No live connections (or the away marker expired and none reconnected).
    Offline,
}

impl PresenceStatus {
    /// Stable label for logs/metrics.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Online => "online",
            Self::Away => "away",
            Self::Offline => "offline",
        }
    }
}

/// Snapshot of a user's presence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Presence {
    /// Current status.
    pub status: PresenceStatus,
    /// Number of live connections for the user.
    pub connections: usize,
}

/// Snapshot of a user's presence.
impl Presence {
    /// Whether the user has at least one connection.
    pub fn is_online(&self) -> bool {
        self.connections > 0
    }
}

/// A per-user "away until" marker.
#[derive(Debug, Clone, Copy)]
struct AwayMarker {
    until: Instant,
}

/// Process-local connection registry.
pub trait IConnectionRegistry: Send + Sync {
    /// Add a live connection (called by the transport when a session starts).
    fn register(&self, connection: Arc<dyn IConnection>);

    /// Remove a connection (called when the session ends).
    fn unregister(&self, id: ConnectionId);

    /// Look up one live connection.
    fn get(&self, id: ConnectionId) -> Option<Arc<dyn IConnection>>;

    /// Number of live connections.
    fn len(&self) -> usize;

    /// Whether the registry is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// All live connections of an authenticated user.
    fn user_connections(&self, user_id: &str) -> Vec<Arc<dyn IConnection>>;

    /// Join a room (idempotent).
    fn join(&self, id: ConnectionId, room: &str);

    /// Leave a room (idempotent).
    fn leave(&self, id: ConnectionId, room: &str);

    /// All live connections in a room.
    fn room_connections(&self, room: &str) -> Vec<Arc<dyn IConnection>>;

    /// Set an explicit away marker for `user_id`; it expires after `ttl` (a
    /// reconnecting user is online again).
    fn set_presence(&self, user_id: &str, status: PresenceStatus, ttl: Duration);

    /// Current presence snapshot for `user_id`.
    fn presence(&self, user_id: &str) -> Presence;
}

#[derive(Default)]
struct RegistryState {
    connections: HashMap<u64, Arc<dyn IConnection>>,
    /// connection id -> authenticated user id.
    connection_users: HashMap<u64, String>,
    /// user id -> connection ids.
    users: HashMap<String, HashSet<u64>>,
    /// room -> connection ids.
    rooms: HashMap<String, HashSet<u64>>,
    /// user id -> away marker.
    away: HashMap<String, AwayMarker>,
}

/// In-memory [`IConnectionRegistry`].
///
/// A single mutex keeps the three indexes consistent; the critical section is
/// only index bookkeeping (no I/O and no awaits), and lookups return cloned
/// `Arc`s so fan-out happens outside the lock.
#[derive(Default)]
pub struct InMemoryConnectionRegistry {
    state: Mutex<RegistryState>,
    subscriber_seq: AtomicU64,
}

impl InMemoryConnectionRegistry {
    /// Empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, RegistryState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Number of rooms with at least one connection (diagnostics/tests).
    pub fn room_count(&self) -> usize {
        self.lock()
            .rooms
            .values()
            .filter(|ids| !ids.is_empty())
            .count()
    }

    /// Room membership of one connection (diagnostics/tests).
    pub fn rooms_of(&self, id: ConnectionId) -> Vec<String> {
        let state = self.lock();
        let mut rooms: Vec<String> = state
            .rooms
            .iter()
            .filter(|(_, ids)| ids.contains(&id.value()))
            .map(|(room, _)| room.clone())
            .collect();
        rooms.sort();
        rooms
    }

    /// Remove connections that are no longer live? Kept for API parity with
    /// remote adapters; the local registry unregisters explicitly.
    pub fn next_token(&self) -> u64 {
        self.subscriber_seq.fetch_add(1, Ordering::Relaxed)
    }
}

impl IConnectionRegistry for InMemoryConnectionRegistry {
    fn register(&self, connection: Arc<dyn IConnection>) {
        let id = connection.id().value();
        let identity = connection
            .context()
            .identity()
            .map(|identity| identity.id.clone());
        let mut state = self.lock();
        if let Some(user_id) = identity {
            state.users.entry(user_id.clone()).or_default().insert(id);
            state.connection_users.insert(id, user_id);
        }
        state.connections.insert(id, connection);
    }

    fn unregister(&self, id: ConnectionId) {
        let mut state = self.lock();
        state.connections.remove(&id.value());
        if let Some(user_id) = state.connection_users.remove(&id.value()) {
            if let Some(ids) = state.users.get_mut(&user_id) {
                ids.remove(&id.value());
                if ids.is_empty() {
                    state.users.remove(&user_id);
                }
            }
        }
        for ids in state.rooms.values_mut() {
            ids.remove(&id.value());
        }
        state.rooms.retain(|_, ids| !ids.is_empty());
    }

    fn get(&self, id: ConnectionId) -> Option<Arc<dyn IConnection>> {
        self.lock().connections.get(&id.value()).cloned()
    }

    fn len(&self) -> usize {
        self.lock().connections.len()
    }

    fn user_connections(&self, user_id: &str) -> Vec<Arc<dyn IConnection>> {
        let state = self.lock();
        state
            .users
            .get(user_id)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| state.connections.get(id).cloned())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn join(&self, id: ConnectionId, room: &str) {
        let mut state = self.lock();
        if state.connections.contains_key(&id.value()) {
            state
                .rooms
                .entry(room.to_string())
                .or_default()
                .insert(id.value());
        }
    }

    fn leave(&self, id: ConnectionId, room: &str) {
        let mut state = self.lock();
        let empty = match state.rooms.get_mut(room) {
            Some(ids) => {
                ids.remove(&id.value());
                ids.is_empty()
            }
            None => false,
        };
        if empty {
            state.rooms.remove(room);
        }
    }

    fn room_connections(&self, room: &str) -> Vec<Arc<dyn IConnection>> {
        let state = self.lock();
        state
            .rooms
            .get(room)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| state.connections.get(id).cloned())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn set_presence(&self, user_id: &str, status: PresenceStatus, ttl: Duration) {
        let mut state = self.lock();
        match status {
            PresenceStatus::Away => {
                state.away.insert(
                    user_id.to_string(),
                    AwayMarker {
                        until: Instant::now() + ttl,
                    },
                );
            }
            // Online/Offline are derived from live connections; clear the marker.
            PresenceStatus::Online | PresenceStatus::Offline => {
                state.away.remove(user_id);
            }
        }
    }

    fn presence(&self, user_id: &str) -> Presence {
        let mut state = self.lock();
        let connections = state.users.get(user_id).map_or(0, HashSet::len);
        let away = match state.away.get(user_id) {
            Some(marker) if marker.until > Instant::now() => true,
            Some(_) => {
                state.away.remove(user_id);
                false
            }
            None => false,
        };
        let status = if connections == 0 {
            PresenceStatus::Offline
        } else if away {
            PresenceStatus::Away
        } else {
            PresenceStatus::Online
        };
        Presence {
            status,
            connections,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;
    use crate::auth::IdentityUser;
    use crate::error::Result;
    use crate::realtime::{
        ConnectionContext, ConnectionMeta, ConnectionScope, IConnection, RealtimeMessage,
        TransportKind,
    };

    struct FakeConnection {
        context: Arc<ConnectionContext>,
        sent: Arc<AtomicUsize>,
    }

    impl FakeConnection {
        fn new(identity: Option<Arc<IdentityUser>>) -> Self {
            let provider = rust_dix::ServiceCollection::new()
                .build()
                .expect("provider");
            let scope = Arc::new(ConnectionScope::new(provider.scope()));
            let meta = ConnectionMeta::new(TransportKind::WebSocket);
            Self {
                context: Arc::new(ConnectionContext::new(meta, identity, scope)),
                sent: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    #[async_trait::async_trait]
    impl IConnection for FakeConnection {
        fn meta(&self) -> &ConnectionMeta {
            self.context.meta()
        }

        fn context(&self) -> &ConnectionContext {
            &self.context
        }

        async fn recv(&self) -> Option<Result<RealtimeMessage>> {
            None
        }

        async fn send_text(&self, _text: String) -> Result<()> {
            self.sent.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }

        async fn send_binary(&self, _bytes: Vec<u8>) -> Result<()> {
            Ok(())
        }

        async fn close(&self, _code: u16, _reason: &str) -> Result<()> {
            Ok(())
        }
    }

    fn user(id: &str) -> Arc<IdentityUser> {
        Arc::new(IdentityUser {
            id: id.to_string(),
            ..Default::default()
        })
    }

    #[test]
    fn register_indexes_by_user_and_rooms() {
        let registry = InMemoryConnectionRegistry::new();
        let first = Arc::new(FakeConnection::new(Some(user("alice"))));
        let second = Arc::new(FakeConnection::new(Some(user("bob"))));
        registry.register(Arc::clone(&first) as Arc<dyn IConnection>);
        registry.register(Arc::clone(&second) as Arc<dyn IConnection>);
        assert_eq!(registry.len(), 2);

        assert_eq!(registry.user_connections("alice").len(), 1);
        assert_eq!(registry.presence("alice").status, PresenceStatus::Online);
        assert_eq!(registry.presence("nobody").status, PresenceStatus::Offline);

        registry.join(first.id(), "room-1");
        registry.join(second.id(), "room-1");
        registry.join(first.id(), "room-1");
        assert_eq!(registry.room_connections("room-1").len(), 2);
        assert_eq!(registry.rooms_of(first.id()), vec!["room-1".to_string()]);

        registry.leave(first.id(), "room-1");
        assert_eq!(registry.room_connections("room-1").len(), 1);
        assert_eq!(registry.room_count(), 1);

        registry.unregister(first.id());
        registry.unregister(second.id());
        assert!(registry.is_empty());
        assert!(registry.room_connections("room-1").is_empty());
        assert_eq!(registry.presence("alice").status, PresenceStatus::Offline);
    }

    #[test]
    fn away_marker_expires_and_clears() {
        let registry = InMemoryConnectionRegistry::new();
        let connection = Arc::new(FakeConnection::new(Some(user("carol"))));
        registry.register(Arc::clone(&connection) as Arc<dyn IConnection>);

        registry.set_presence("carol", PresenceStatus::Away, Duration::from_millis(500));
        assert_eq!(registry.presence("carol").status, PresenceStatus::Away);

        std::thread::sleep(Duration::from_millis(600));
        assert_eq!(registry.presence("carol").status, PresenceStatus::Online);

        // Explicitly going online clears any marker without touching connections.
        registry.set_presence("carol", PresenceStatus::Away, Duration::from_secs(5));
        registry.set_presence("carol", PresenceStatus::Online, Duration::from_secs(5));
        assert_eq!(registry.presence("carol").status, PresenceStatus::Online);
    }
}

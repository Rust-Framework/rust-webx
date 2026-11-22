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

/// Direction of a user-level connection-count change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresenceDirection {
    /// A connection for the user was added.
    Connected,
    /// A connection for the user was removed.
    Disconnected,
}

/// One atomic user-level presence change.
///
/// Computed inside the registry lock, so it is the race-free way to answer
/// "is this the user's first/last connection": business code never counts
/// `user_connections` across two calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresenceTransition {
    /// Authenticated user whose connection count changed.
    pub user_id: String,
    /// Tenant claim of the connection (`None` when the token has none).
    pub tenant_id: Option<String>,
    /// Live connection count of the user *after* the change.
    pub connections: usize,
    /// Whether a connection was added or removed.
    pub direction: PresenceDirection,
}

impl PresenceTransition {
    /// This was the user's first live connection (became online).
    pub fn is_first(&self) -> bool {
        self.direction == PresenceDirection::Connected && self.connections == 1
    }

    /// This was the user's last live connection (became offline).
    pub fn is_last(&self) -> bool {
        self.direction == PresenceDirection::Disconnected && self.connections == 0
    }
}

/// Observer notified for every authenticated connection's presence transition.
///
/// Register it with `HostBuilder::use_presence_observer` (or as a DI singleton
/// for `dyn IPresenceObserver`). Observers run on the session task before
/// `IWebSocketHandler::on_connect` and after the connection is unregistered, so
/// "announce online/offline exactly once per transition" needs no app-side
/// counting. Never block the session: do fan-out/event publishing, not I/O
/// waits that outlive the transition.
#[async_trait::async_trait]
pub trait IPresenceObserver: Send + Sync {
    /// Called after the registry applied the change.
    async fn on_presence_transition(&self, transition: PresenceTransition);
}

/// Process-local connection registry.
pub trait IConnectionRegistry: Send + Sync {
    /// Add a live connection (called by the transport when a session starts).
    fn register(&self, connection: Arc<dyn IConnection>);

    /// Remove a connection (called when the session ends).
    fn unregister(&self, id: ConnectionId);

    /// Add a connection and atomically report the user-level transition.
    ///
    /// Default: register and report nothing (a remote registry may not track
    /// presence locally). The in-memory registry reports every authenticated
    /// registration; anonymous connections report `None`.
    fn register_observed(&self, connection: Arc<dyn IConnection>) -> Option<PresenceTransition> {
        self.register(connection);
        None
    }

    /// Remove a connection and atomically report the user-level transition.
    ///
    /// Default: unregister and report nothing.
    fn unregister_observed(&self, id: ConnectionId) -> Option<PresenceTransition> {
        self.unregister(id);
        None
    }

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

    /// Number of live connections for `user_id` (`0` when offline).
    fn user_connection_count(&self, user_id: &str) -> usize {
        self.user_connections(user_id).len()
    }

    /// Number of live connections in `room`.
    fn room_connection_count(&self, room: &str) -> usize {
        self.room_connections(room).len()
    }

    /// Number of live connections in `tenant_id`.
    ///
    /// Connections whose identity carries no tenant claim are not counted. The
    /// default returns `0`; a remote/shared registry must override it when it
    /// knows the tenant index.
    fn tenant_connection_count(&self, _tenant_id: &str) -> usize {
        0
    }

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
    /// tenant id -> connection ids (identities without a claim are not indexed).
    tenants: HashMap<String, HashSet<u64>>,
    /// connection id -> tenant id.
    connection_tenants: HashMap<u64, String>,
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

    /// Shared registration body: inserts the connection and, for an
    /// authenticated identity, computes the user-level transition while the
    /// indexes are still locked.
    fn register_inner(&self, connection: Arc<dyn IConnection>) -> Option<PresenceTransition> {
        let id = connection.id().value();
        let identity = connection.context().identity().cloned();
        let mut state = self.lock();
        let transition = identity.map(|identity| {
            state
                .users
                .entry(identity.id.clone())
                .or_default()
                .insert(id);
            state.connection_users.insert(id, identity.id.clone());
            let tenant_id = identity.tenant().map(str::to_string);
            if let Some(tenant_id) = &tenant_id {
                state
                    .tenants
                    .entry(tenant_id.clone())
                    .or_default()
                    .insert(id);
                state.connection_tenants.insert(id, tenant_id.clone());
            }
            PresenceTransition {
                user_id: identity.id.clone(),
                tenant_id,
                connections: state.users.get(&identity.id).map_or(0, HashSet::len),
                direction: PresenceDirection::Connected,
            }
        });
        state.connections.insert(id, connection);
        transition
    }

    /// Shared unregistration body: removes the connection and, for an
    /// authenticated identity, computes the user-level transition atomically.
    fn unregister_inner(&self, id: ConnectionId) -> Option<PresenceTransition> {
        let mut state = self.lock();
        state.connections.remove(&id.value());
        let user_id = state.connection_users.remove(&id.value());
        let tenant_id = state.connection_tenants.remove(&id.value());
        let transition = user_id.map(|user_id| {
            let mut connections = 0;
            if let Some(ids) = state.users.get_mut(&user_id) {
                ids.remove(&id.value());
                connections = ids.len();
                if ids.is_empty() {
                    state.users.remove(&user_id);
                }
            }
            if let Some(tenant_id) = &tenant_id {
                if let Some(ids) = state.tenants.get_mut(tenant_id) {
                    ids.remove(&id.value());
                    if ids.is_empty() {
                        state.tenants.remove(tenant_id);
                    }
                }
            }
            PresenceTransition {
                user_id: user_id.clone(),
                tenant_id: tenant_id.clone(),
                connections,
                direction: PresenceDirection::Disconnected,
            }
        });
        for ids in state.rooms.values_mut() {
            ids.remove(&id.value());
        }
        state.rooms.retain(|_, ids| !ids.is_empty());
        transition
    }
}

impl IConnectionRegistry for InMemoryConnectionRegistry {
    fn register(&self, connection: Arc<dyn IConnection>) {
        let _ = self.register_inner(connection);
    }

    fn unregister(&self, id: ConnectionId) {
        let _ = self.unregister_inner(id);
    }

    fn register_observed(&self, connection: Arc<dyn IConnection>) -> Option<PresenceTransition> {
        self.register_inner(connection)
    }

    fn unregister_observed(&self, id: ConnectionId) -> Option<PresenceTransition> {
        self.unregister_inner(id)
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

    fn user_connection_count(&self, user_id: &str) -> usize {
        self.lock().users.get(user_id).map_or(0, HashSet::len)
    }

    fn room_connection_count(&self, room: &str) -> usize {
        self.lock().rooms.get(room).map_or(0, HashSet::len)
    }

    fn tenant_connection_count(&self, tenant_id: &str) -> usize {
        self.lock().tenants.get(tenant_id).map_or(0, HashSet::len)
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

    fn tenant_user(id: &str, tenant_id: &str) -> Arc<IdentityUser> {
        Arc::new(IdentityUser {
            id: id.to_string(),
            tenant_id: tenant_id.to_string(),
            ..Default::default()
        })
    }

    #[test]
    fn observed_registration_reports_first_and_last_transitions() {
        let registry = InMemoryConnectionRegistry::new();
        let first_connection = Arc::new(FakeConnection::new(Some(tenant_user("alice", "acme"))));
        let second_connection = Arc::new(FakeConnection::new(Some(tenant_user("alice", "acme"))));
        let anonymous = Arc::new(FakeConnection::new(None));

        let first = registry
            .register_observed(Arc::clone(&first_connection) as Arc<dyn IConnection>)
            .expect("an authenticated registration reports a transition");
        assert_eq!(first.direction, PresenceDirection::Connected);
        assert_eq!(first.user_id, "alice");
        assert_eq!(first.tenant_id.as_deref(), Some("acme"));
        assert_eq!(first.connections, 1);
        assert!(first.is_first());
        assert!(!first.is_last());

        let second = registry
            .register_observed(Arc::clone(&second_connection) as Arc<dyn IConnection>)
            .expect("the second registration still reports the count");
        assert_eq!(second.connections, 2);
        assert!(!second.is_first());

        assert!(
            registry
                .register_observed(Arc::clone(&anonymous) as Arc<dyn IConnection>)
                .is_none(),
            "anonymous connections have no presence"
        );

        let after_second = registry
            .unregister_observed(second_connection.id())
            .expect("authenticated unregistration reports a transition");
        assert_eq!(after_second.direction, PresenceDirection::Disconnected);
        assert_eq!(after_second.connections, 1);
        assert!(!after_second.is_last());

        let last = registry
            .unregister_observed(first_connection.id())
            .expect("the last unregistration reports");
        assert!(last.is_last());
        assert_eq!(last.connections, 0);
        assert_eq!(last.tenant_id.as_deref(), Some("acme"));

        assert!(
            registry
                .unregister_observed(first_connection.id())
                .is_none(),
            "unknown connections report nothing"
        );
    }

    #[test]
    fn connection_counts_are_indexed_by_user_room_and_tenant() {
        let registry = InMemoryConnectionRegistry::new();
        let alice = Arc::new(FakeConnection::new(Some(tenant_user("alice", "acme"))));
        let bob = Arc::new(FakeConnection::new(Some(tenant_user("bob", "acme"))));
        let anonymous = Arc::new(FakeConnection::new(None));
        for connection in [&alice, &bob, &anonymous] {
            registry.register(Arc::clone(connection) as Arc<dyn IConnection>);
        }
        registry.join(alice.id(), "room-1");
        registry.join(bob.id(), "room-1");

        assert_eq!(registry.user_connection_count("alice"), 1);
        assert_eq!(registry.user_connection_count("nobody"), 0);
        assert_eq!(registry.room_connection_count("room-1"), 2);
        assert_eq!(registry.room_connection_count("room-2"), 0);
        assert_eq!(registry.tenant_connection_count("acme"), 2);
        assert_eq!(registry.tenant_connection_count("other"), 0);

        registry.unregister(alice.id());
        assert_eq!(registry.tenant_connection_count("acme"), 1);
        assert_eq!(registry.room_connection_count("room-1"), 1);
        registry.unregister(bob.id());
        assert_eq!(registry.tenant_connection_count("acme"), 0);
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

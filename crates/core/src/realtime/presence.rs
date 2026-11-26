//! Cross-node online presence: the [`IOnlinePresence`] port and its
//! dependency-free implementation.
//!
//! The process-local [`IConnectionRegistry`](crate::realtime::IConnectionRegistry)
//! answers "who is connected **to this node**". Multi-instance IM also needs an
//! authoritative answer for "who is online **at all**", and a room's fan-out
//! needs to reach every node that holds a member. This module owns the port; a
//! shared implementation (Redis ZSET with TTL scores) ships with
//! `rust-webx-redis`, and the host can bridge presence transitions + heartbeats
//! with [`HostBuilder::use_online_presence`](crate::realtime::IOnlinePresence).
//!
//! # Model
//!
//! A **scope** is a namespaced set of live members (`user:<id>`,
//! `tenant:<id>`, `room:<id>`, or an application namespace). A **member** is one
//! live connection: the same user on two devices is two members, which makes
//! connection counts exact. Members carry a **TTL**: a node that crashes cannot
//! leave permanent online ghost entries, and the owning node refreshes its
//! members with [`IOnlinePresence::touch`].
//!
//! # Fail-closed rules
//!
//! Empty scope/member fields and a zero TTL are validation errors: a store that
//! accepts them would report "online" for input that cannot be addressed or
//! expires instantly. A scope is bounded by
//! [`OnlinePresenceLimits::max_members_per_scope`]; exceeding it is a retryable
//! `503` instead of unbounded memory.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;

use crate::error::{Error, Result};
use crate::time::{IClock, SystemClock};

/// Namespaced online-presence scope.
///
/// The framework uses `user:` / `tenant:` namespaces when it bridges presence
/// transitions; applications may use any other namespace (for example `room:`)
/// through [`PresenceScope::custom`]. The rendered string is the store key, so
/// it must not be empty and should stay stable across releases.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PresenceScope(String);

impl PresenceScope {
    /// `user:<user_id>`: every live connection of one user.
    pub fn user(user_id: impl AsRef<str>) -> Self {
        Self(format!("user:{}", user_id.as_ref()))
    }

    /// `tenant:<tenant_id>`: every live connection in one tenant.
    pub fn tenant(tenant_id: impl AsRef<str>) -> Self {
        Self(format!("tenant:{}", tenant_id.as_ref()))
    }

    /// `room:<room_id>`: application-managed room presence.
    pub fn room(room_id: impl AsRef<str>) -> Self {
        Self(format!("room:{}", room_id.as_ref()))
    }

    /// An application namespace (`<namespace>:<id>`).
    pub fn custom(namespace: impl AsRef<str>, id: impl AsRef<str>) -> Self {
        Self(format!("{}:{}", namespace.as_ref(), id.as_ref()))
    }

    /// The rendered store key.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn validate(&self) -> Result<()> {
        let (namespace, id) = self.0.split_once(':').unwrap_or(("", ""));
        if namespace.trim().is_empty() || id.trim().is_empty() {
            return Err(Error::validation(
                "a presence scope must be `<namespace>:<id>` with both parts non-empty",
            ));
        }
        validate_scope_key(&self.0)?;
        Ok(())
    }
}

impl fmt::Display for PresenceScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// One live connection inside a presence scope.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PresenceMember {
    /// Authenticated user id.
    pub user_id: String,
    /// Membership slot id. The host bridge uses the node id (one member per
    /// user per node); an application-managed scope can use a connection id.
    pub connection_id: String,
    /// Instance that owns the connection. Refreshed by that instance.
    pub node_id: String,
    /// When the connection joined (Unix seconds; diagnostics).
    pub joined_at_unix_secs: u64,
}

impl PresenceMember {
    /// A member for `user_id`/`connection_id` with no node stamp yet.
    pub fn new(user_id: impl Into<String>, connection_id: impl Into<String>) -> Self {
        Self {
            user_id: user_id.into(),
            connection_id: connection_id.into(),
            node_id: String::new(),
            joined_at_unix_secs: 0,
        }
    }

    /// Stamp the owning node.
    pub fn with_node_id(mut self, node_id: impl Into<String>) -> Self {
        self.node_id = node_id.into();
        self
    }

    /// Stamp the join time.
    pub fn with_joined_at_unix_secs(mut self, joined_at_unix_secs: u64) -> Self {
        self.joined_at_unix_secs = joined_at_unix_secs;
        self
    }

    /// Stable identity of this member (`user_id`, `connection_id`, `node_id`).
    ///
    /// The rendered form is **length-prefixed**, not a bare delimiter join: a
    /// `|` (or any other separator) inside an id can never make two different
    /// triples collide. It is an internal identity key, not a display format;
    /// the field order is the only stable part.
    pub fn key(&self) -> String {
        format!(
            "{}:{}:{}:{}:{}:{}",
            self.user_id.len(),
            self.user_id,
            self.connection_id.len(),
            self.connection_id,
            self.node_id.len(),
            self.node_id
        )
    }

    fn validate(&self) -> Result<()> {
        if self.user_id.trim().is_empty() {
            return Err(Error::validation("a presence member needs a user id"));
        }
        if self.connection_id.trim().is_empty() {
            return Err(Error::validation("a presence member needs a connection id"));
        }
        validate_component("member.user_id", &self.user_id)?;
        validate_component("member.connection_id", &self.connection_id)?;
        // The node stamp is optional (a member may be created before it is
        // stamped by the host bridge), but when present it must be safe to use
        // as part of a shared key/identity.
        if !self.node_id.is_empty() {
            validate_component("member.node_id", &self.node_id)?;
        }
        Ok(())
    }
}

/// A rendered presence key component must stay bounded and printable: both the
/// in-memory and Redis adapters embed it in a shared key/identity.
fn validate_component(name: &str, value: &str) -> Result<()> {
    if value.len() > 512 {
        return Err(Error::validation(format!(
            "{name} must be at most 512 bytes"
        )));
    }
    if value.chars().any(char::is_control) {
        return Err(Error::validation(format!(
            "{name} must not contain control characters"
        )));
    }
    Ok(())
}

fn validate_scope_key(scope: &str) -> Result<()> {
    validate_component("presence scope", scope)
}

/// Bounds for [`InMemoryOnlinePresence`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnlinePresenceLimits {
    /// Maximum members retained per scope; a scope that is full rejects joins
    /// with a retryable `503` instead of growing without limit.
    pub max_members_per_scope: usize,
}

impl Default for OnlinePresenceLimits {
    fn default() -> Self {
        Self {
            max_members_per_scope: 100_000,
        }
    }
}

/// Shared online-presence store.
///
/// Implementations are registered as a **singleton** (`HostBuilder::use_online_presence`
/// does this, together with the presence-transition bridge and its heartbeat).
/// All methods are async so a network-backed store fits without changing call
/// sites; the in-memory implementation is `O(1)` per operation.
#[async_trait]
pub trait IOnlinePresence: Send + Sync {
    /// Add or refresh `member` in `scope` with `ttl`; returns the scope count.
    async fn join(
        &self,
        scope: &PresenceScope,
        member: PresenceMember,
        ttl: Duration,
    ) -> Result<u64>;

    /// Remove `member` from `scope`; returns the remaining scope count.
    ///
    /// Removing an unknown member is not an error (disconnect is idempotent).
    async fn leave(&self, scope: &PresenceScope, member: &PresenceMember) -> Result<u64>;

    /// Refresh the TTL of an existing member (heartbeat).
    ///
    /// Unknown/expired members are not resurrected: a heartbeat after a TTL
    /// expiry cannot make a dead connection look online again.
    async fn touch(
        &self,
        scope: &PresenceScope,
        member: &PresenceMember,
        ttl: Duration,
    ) -> Result<()>;

    /// Live members of `scope`, ordered by user id then connection id.
    async fn members(&self, scope: &PresenceScope) -> Result<Vec<PresenceMember>>;

    /// Number of live members in `scope`.
    async fn count(&self, scope: &PresenceScope) -> Result<u64>;
}

#[derive(Default)]
struct ScopeState {
    /// member key -> (member, expires_at_unix_secs).
    members: HashMap<String, (PresenceMember, u64)>,
}

/// Process-local [`IOnlinePresence`].
///
/// One mutex over all scopes; the critical section is pure bookkeeping (no I/O,
/// no awaits). Expired members are pruned lazily on every read and on `join`, so
/// memory stays bounded without a sweeper.
pub struct InMemoryOnlinePresence {
    limits: OnlinePresenceLimits,
    clock: Arc<dyn IClock>,
    state: Mutex<HashMap<String, ScopeState>>,
}

impl Default for InMemoryOnlinePresence {
    fn default() -> Self {
        Self::new(OnlinePresenceLimits::default())
    }
}

impl InMemoryOnlinePresence {
    /// New store with `limits` and the system clock.
    pub fn new(limits: OnlinePresenceLimits) -> Self {
        Self::with_clock(limits, Arc::new(SystemClock))
    }

    /// New store with an injected clock (deterministic tests).
    pub fn with_clock(limits: OnlinePresenceLimits, clock: Arc<dyn IClock>) -> Self {
        Self {
            limits,
            clock,
            state: Mutex::new(HashMap::new()),
        }
    }

    /// Retention bounds.
    pub fn limits(&self) -> &OnlinePresenceLimits {
        &self.limits
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, ScopeState>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn now_unix(&self) -> u64 {
        self.clock.now_unix()
    }

    fn prune(state: &mut ScopeState, now_unix: u64) -> usize {
        let before = state.members.len();
        state
            .members
            .retain(|_, (_, expires_at)| *expires_at > now_unix);
        before - state.members.len()
    }

    fn ttl_secs(ttl: Duration) -> Result<u64> {
        let seconds = ttl.as_secs();
        if seconds == 0 {
            return Err(Error::validation(
                "presence TTL must be at least one second",
            ));
        }
        Ok(seconds)
    }
}

#[async_trait]
impl IOnlinePresence for InMemoryOnlinePresence {
    async fn join(
        &self,
        scope: &PresenceScope,
        member: PresenceMember,
        ttl: Duration,
    ) -> Result<u64> {
        scope.validate()?;
        member.validate()?;
        let ttl_secs = Self::ttl_secs(ttl)?;
        let now = self.now_unix();
        let expires_at = now.saturating_add(ttl_secs);
        let mut state = self.lock();
        let scope_state = state.entry(scope.as_str().to_string()).or_default();
        Self::prune(scope_state, now);
        let key = member.key();
        if !scope_state.members.contains_key(&key)
            && scope_state.members.len() >= self.limits.max_members_per_scope
        {
            return Err(Error::status(
                503,
                format!(
                    "presence scope '{}' is full ({} members)",
                    scope, self.limits.max_members_per_scope
                ),
            ));
        }
        scope_state.members.insert(key, (member, expires_at));
        Ok(scope_state.members.len() as u64)
    }

    async fn leave(&self, scope: &PresenceScope, member: &PresenceMember) -> Result<u64> {
        scope.validate()?;
        member.validate()?;
        let now = self.now_unix();
        let mut state = self.lock();
        let scope_state = state.entry(scope.as_str().to_string()).or_default();
        Self::prune(scope_state, now);
        scope_state.members.remove(&member.key());
        Ok(scope_state.members.len() as u64)
    }

    async fn touch(
        &self,
        scope: &PresenceScope,
        member: &PresenceMember,
        ttl: Duration,
    ) -> Result<()> {
        scope.validate()?;
        member.validate()?;
        let ttl_secs = Self::ttl_secs(ttl)?;
        let now = self.now_unix();
        let mut state = self.lock();
        let Some(scope_state) = state.get_mut(scope.as_str()) else {
            return Ok(());
        };
        Self::prune(scope_state, now);
        if let Some((_, expires_at)) = scope_state.members.get_mut(&member.key()) {
            *expires_at = now.saturating_add(ttl_secs);
        }
        Ok(())
    }

    async fn members(&self, scope: &PresenceScope) -> Result<Vec<PresenceMember>> {
        scope.validate()?;
        let now = self.now_unix();
        let mut state = self.lock();
        let scope_state = state.entry(scope.as_str().to_string()).or_default();
        Self::prune(scope_state, now);
        let mut members: Vec<PresenceMember> = scope_state
            .members
            .values()
            .map(|(member, _)| member.clone())
            .collect();
        members.sort_by(|left, right| {
            left.user_id
                .cmp(&right.user_id)
                .then_with(|| left.connection_id.cmp(&right.connection_id))
                .then_with(|| left.node_id.cmp(&right.node_id))
        });
        Ok(members)
    }

    async fn count(&self, scope: &PresenceScope) -> Result<u64> {
        scope.validate()?;
        let now = self.now_unix();
        let mut state = self.lock();
        let scope_state = state.entry(scope.as_str().to_string()).or_default();
        Self::prune(scope_state, now);
        Ok(scope_state.members.len() as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::TestClock;

    #[test]
    fn member_identity_encoding_cannot_be_ambiguous() {
        // A delimiter inside an id must not let two different triples collide.
        let first = PresenceMember::new("a|b", "c");
        let second = PresenceMember::new("a", "b|c");
        assert_ne!(first.key(), second.key());
        // The join stamp is diagnostics, not identity.
        assert_eq!(
            PresenceMember::new("u1", "c1")
                .with_node_id("n1")
                .with_joined_at_unix_secs(1)
                .key(),
            PresenceMember::new("u1", "c1")
                .with_node_id("n1")
                .with_joined_at_unix_secs(99)
                .key()
        );
    }

    #[test]
    fn oversized_or_control_key_components_are_rejected() {
        let long = "x".repeat(513);
        assert!(PresenceScope::user(long.clone()).validate().is_err());
        assert!(PresenceMember::new(long, "c1").validate().is_err());
        assert!(PresenceMember::new("u1\n", "c1").validate().is_err());
        assert!(PresenceMember::new("u1", "c1")
            .with_node_id("node\u{1}")
            .validate()
            .is_err());
        assert!(PresenceMember::new("u1", "c1")
            .with_node_id("node-a")
            .validate()
            .is_ok());
    }

    fn store(clock: Arc<TestClock>) -> InMemoryOnlinePresence {
        InMemoryOnlinePresence::with_clock(OnlinePresenceLimits::default(), clock)
    }

    #[tokio::test]
    async fn join_leave_and_count_are_namespaced() {
        let clock = Arc::new(TestClock::from_unix(1_700_000_000));
        let store = store(Arc::clone(&clock));
        let scope = PresenceScope::user("u1");
        let member = PresenceMember::new("u1", "c1").with_node_id("node-a");
        assert_eq!(
            store
                .join(&scope, member.clone(), Duration::from_secs(60))
                .await
                .expect("join"),
            1
        );
        assert_eq!(
            store
                .count(&PresenceScope::tenant("t1"))
                .await
                .expect("count"),
            0
        );
        assert_eq!(store.count(&scope).await.expect("count"), 1);

        assert_eq!(store.leave(&scope, &member).await.expect("leave"), 0);
        assert_eq!(store.count(&scope).await.expect("count"), 0);
    }

    #[tokio::test]
    async fn members_expire_after_ttl_and_heartbeat_extends() {
        let clock = Arc::new(TestClock::from_unix(1_700_000_000));
        let store = store(Arc::clone(&clock));
        let scope = PresenceScope::room("r1");
        let member = PresenceMember::new("u1", "c1").with_node_id("node-a");
        store
            .join(&scope, member.clone(), Duration::from_secs(10))
            .await
            .expect("join");

        clock.advance(Duration::from_secs(9));
        store
            .touch(&scope, &member, Duration::from_secs(10))
            .await
            .expect("touch");
        clock.advance(Duration::from_secs(9));
        assert_eq!(
            store.count(&scope).await.expect("count"),
            1,
            "heartbeat extended the ttl"
        );

        clock.advance(Duration::from_secs(2));
        assert_eq!(
            store.count(&scope).await.expect("count"),
            0,
            "no heartbeat -> expired"
        );
        store
            .touch(&scope, &member, Duration::from_secs(10))
            .await
            .expect("touch");
        assert_eq!(
            store.count(&scope).await.expect("count"),
            0,
            "touch cannot resurrect"
        );
    }

    #[tokio::test]
    async fn same_user_on_two_nodes_is_two_members() {
        let clock = Arc::new(TestClock::from_unix(1_700_000_000));
        let store = store(clock);
        let scope = PresenceScope::user("u1");
        store
            .join(
                &scope,
                PresenceMember::new("u1", "c1").with_node_id("node-a"),
                Duration::from_secs(60),
            )
            .await
            .expect("join a");
        store
            .join(
                &scope,
                PresenceMember::new("u1", "c2").with_node_id("node-b"),
                Duration::from_secs(60),
            )
            .await
            .expect("join b");
        let members = store.members(&scope).await.expect("members");
        assert_eq!(members.len(), 2);
        assert_eq!(members[0].node_id, "node-a");
    }

    #[tokio::test]
    async fn invalid_inputs_are_typed_validation_errors() {
        let clock = Arc::new(TestClock::from_unix(1_700_000_000));
        let store = store(clock);
        let scope = PresenceScope::user("u1");
        let member = PresenceMember::new("u1", "c1");
        assert!(store
            .join(&scope, member.clone(), Duration::ZERO)
            .await
            .is_err());
        assert!(store
            .join(
                &PresenceScope("nonsense".to_string()),
                member,
                Duration::from_secs(1)
            )
            .await
            .is_err());
        assert!(store
            .join(
                &scope,
                PresenceMember::new("", "c1"),
                Duration::from_secs(1)
            )
            .await
            .is_err());
    }

    #[tokio::test]
    async fn a_full_scope_rejects_joins_instead_of_growing() {
        let clock = Arc::new(TestClock::from_unix(1_700_000_000));
        let store = InMemoryOnlinePresence::with_clock(
            OnlinePresenceLimits {
                max_members_per_scope: 1,
            },
            clock,
        );
        let scope = PresenceScope::user("u1");
        store
            .join(
                &scope,
                PresenceMember::new("u1", "c1"),
                Duration::from_secs(60),
            )
            .await
            .expect("first join");
        let error = store
            .join(
                &scope,
                PresenceMember::new("u1", "c2"),
                Duration::from_secs(60),
            )
            .await
            .expect_err("second join must not grow the scope");
        assert_eq!(error.status_code(), 503);
    }
}

//! Redis sorted-set-backed shared online presence ([`IOnlinePresence`]).
//!
//! One sorted set per scope:
//!
//! ```text
//! {prefix}:scope:{scope}   member = JSON(PresenceMember), score = expiry unix seconds
//! ```
//!
//! `join`/`touch` store `now + ttl` as the score and every operation first runs
//! `ZREMRANGEBYSCORE -inf now`, so expired members are never observed.
//! `members`/`count` are therefore authoritative across nodes: a node that
//! crashes stops refreshing its members and they disappear when the TTL passes.
//! `touch` uses `ZADD XX`: a heartbeat cannot resurrect an entry whose TTL
//! already elapsed.
//!
//! # Member identity
//!
//! The ZSET member is the JSON encoding of the whole [`PresenceMember`] (so the
//! join timestamp round-trips and no delimiter inside a user id can split a
//! member). Mutating operations resolve the stored encoding through an
//! in-process identity index keyed by `user|connection|node` — the same identity
//! the in-memory store uses — so a heartbeat or leave built from a fresh
//! `PresenceMember` (a different join stamp) still addresses the entry this
//! process joined. Each node owns its own members, so the index does not need to
//! be shared; cross-node reads go through the ZSET.
//!
//! ```no_run
//! use std::sync::Arc;
//! use std::time::Duration;
//! use webx_core::realtime::{IOnlinePresence, PresenceMember, PresenceScope};
//! use webx_redis::{RedisConnection, RedisOnlinePresence};
//!
//! # async fn demo() -> webx_core::error::Result<()> {
//! let connection = Arc::new(RedisConnection::new(
//!     "redis://127.0.0.1:6379",
//!     Duration::from_secs(3),
//!     Duration::from_secs(2),
//! ));
//! let presence = RedisOnlinePresence::new(connection);
//! let scope = PresenceScope::user("u1");
//! presence
//!     .join(
//!         &scope,
//!         PresenceMember::new("u1", "c1").with_node_id("node-a"),
//!         Duration::from_secs(60),
//!     )
//!     .await?;
//! # Ok(())
//! # }
//! ```

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use webx_core::error::{Error, Result};
use webx_core::realtime::{IOnlinePresence, PresenceMember, PresenceScope};
use webx_core::time::{IClock, SystemClock};

use crate::connection::{transport_error, RedisConnection};

/// Default key prefix for presence scopes.
pub(crate) const DEFAULT_PRESENCE_PREFIX: &str = "webx:presence";

/// Default maximum live members per scope.
pub(crate) const DEFAULT_MAX_MEMBERS_PER_SCOPE: usize = 100_000;

/// Options for [`RedisOnlinePresence`].
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct RedisOnlinePresenceOptions {
    /// Key prefix: each scope uses `{prefix}:scope:{scope}`. Use a distinct
    /// prefix per application so several services can share one Redis.
    pub prefix: String,
    /// Maximum live members per scope. A scope that is full rejects `join` with
    /// a retryable `503` instead of growing without limit.
    pub max_members_per_scope: usize,
}

impl Default for RedisOnlinePresenceOptions {
    fn default() -> Self {
        Self {
            prefix: DEFAULT_PRESENCE_PREFIX.to_string(),
            max_members_per_scope: DEFAULT_MAX_MEMBERS_PER_SCOPE,
        }
    }
}

impl RedisOnlinePresenceOptions {
    /// Default options (`webx:presence` prefix, 100k members per scope).
    pub fn new() -> Self {
        Self::default()
    }

    /// Override the key prefix.
    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    /// Override the maximum live members per scope (clamped to at least one).
    pub fn with_max_members_per_scope(mut self, max_members_per_scope: usize) -> Self {
        self.max_members_per_scope = max_members_per_scope.max(1);
        self
    }
}

/// JSON form of a [`PresenceMember`] stored in the sorted set.
///
/// A local mirror is needed because the core type is not (and should not be)
/// serde-bound; it keeps the ZSET member self-describing and delimiter-safe.
#[derive(Serialize, Deserialize)]
struct StoredMember {
    user_id: String,
    connection_id: String,
    node_id: String,
    joined_at_unix_secs: u64,
}

impl From<&PresenceMember> for StoredMember {
    fn from(member: &PresenceMember) -> Self {
        Self {
            user_id: member.user_id.clone(),
            connection_id: member.connection_id.clone(),
            node_id: member.node_id.clone(),
            joined_at_unix_secs: member.joined_at_unix_secs,
        }
    }
}

impl From<StoredMember> for PresenceMember {
    fn from(member: StoredMember) -> Self {
        PresenceMember::new(member.user_id, member.connection_id)
            .with_node_id(member.node_id)
            .with_joined_at_unix_secs(member.joined_at_unix_secs)
    }
}

/// Redis sorted-set-backed [`IOnlinePresence`].
///
/// Safe to share across nodes: every operation reads the shared ZSET, so
/// `members`/`count` include connections owned by other instances. The TTL is
/// refreshed by the application or the host presence heartbeat.
pub struct RedisOnlinePresence {
    connection: Arc<RedisConnection>,
    options: RedisOnlinePresenceOptions,
    clock: Arc<dyn IClock>,
    /// `(scope, member identity)` -> `(exact ZSET member, expiry)`.
    ///
    /// Mutating operations use it to address a member by the same identity the
    /// in-memory store uses, even when the caller passes a fresh join stamp.
    /// Entries are pruned by expiry on every access, so a process that joins
    /// members which later expire without an explicit `leave` does not grow this
    /// index without bound.
    joined: Mutex<HashMap<(String, String), (String, u64)>>,
}

impl RedisOnlinePresence {
    /// Create a store with default options and the system clock.
    pub fn new(connection: Arc<RedisConnection>) -> Self {
        Self::with_options(connection, RedisOnlinePresenceOptions::default())
    }

    /// Create a store with explicit options and the system clock.
    pub fn with_options(
        connection: Arc<RedisConnection>,
        options: RedisOnlinePresenceOptions,
    ) -> Self {
        Self::with_clock(connection, options, Arc::new(SystemClock))
    }

    /// Create a store with an injected clock (deterministic tests).
    pub(crate) fn with_clock(
        connection: Arc<RedisConnection>,
        options: RedisOnlinePresenceOptions,
        clock: Arc<dyn IClock>,
    ) -> Self {
        Self {
            connection,
            options,
            clock,
            joined: Mutex::new(HashMap::new()),
        }
    }

    /// Retention/prefix options (tests).
    #[cfg(test)]
    pub(crate) fn options(&self) -> &RedisOnlinePresenceOptions {
        &self.options
    }

    /// Sorted-set key for `scope` (crate-internal).
    pub(crate) fn scope_key(&self, scope: &PresenceScope) -> String {
        scope_key(&self.options.prefix, scope.as_str())
    }

    fn max_members(&self) -> usize {
        self.options.max_members_per_scope.max(1)
    }

    fn index(
        &self,
        now: u64,
    ) -> std::sync::MutexGuard<'_, HashMap<(String, String), (String, u64)>> {
        let mut index = self.joined.lock().unwrap_or_else(PoisonError::into_inner);
        index.retain(|_, (_, expires_at)| *expires_at > now);
        index
    }

    fn remember(
        &self,
        scope: &PresenceScope,
        member: &PresenceMember,
        encoded: &str,
        expires_at: u64,
    ) {
        let now = self.clock.now_unix();
        self.index(now).insert(
            (scope.as_str().to_string(), member.key()),
            (encoded.to_string(), expires_at),
        );
    }

    fn recall(&self, scope: &PresenceScope, member: &PresenceMember, now: u64) -> Option<String> {
        self.index(now)
            .get(&(scope.as_str().to_string(), member.key()))
            .map(|(encoded, _)| encoded.clone())
    }

    fn forget(&self, scope: &PresenceScope, member: &PresenceMember, now: u64) {
        self.index(now)
            .remove(&(scope.as_str().to_string(), member.key()));
    }

    fn touch_index(&self, scope: &PresenceScope, member: &PresenceMember, expires_at: u64) {
        let now = self.clock.now_unix();
        let mut index = self.index(now);
        if let Some(entry) = index.get_mut(&(scope.as_str().to_string(), member.key())) {
            entry.1 = expires_at;
        }
    }

    async fn manager(&self) -> Result<redis::aio::ConnectionManager> {
        self.connection
            .manager()
            .await
            .map_err(|error| Error::status(503, format!("redis unavailable: {error}")))
    }
}

#[async_trait]
impl IOnlinePresence for RedisOnlinePresence {
    async fn join(
        &self,
        scope: &PresenceScope,
        member: PresenceMember,
        ttl: Duration,
    ) -> Result<u64> {
        validate_scope(scope)?;
        validate_member(&member)?;
        let ttl_secs = ttl_secs(ttl)?;
        let now = self.clock.now_unix();
        let score = expires_at(now, ttl_secs);
        let key = self.scope_key(scope);
        let encoded = encode_member(&member)?;
        let mut connection = self.manager().await?;
        purge_expired(&mut connection, &key, now).await?;

        // Core semantics: an existing member is refreshed; a new member is
        // rejected when the scope is full.
        let known = self.recall(scope, &member, now);
        let canonical = known.clone().unwrap_or_else(|| encoded.clone());
        if !member_exists(&mut connection, &key, &canonical).await? {
            let count = scope_count(&mut connection, &key).await?;
            let max_members = self.max_members() as u64;
            if count >= max_members {
                self.forget(scope, &member, now);
                return Err(Error::status(
                    503,
                    format!("presence scope '{}' is full ({max_members} members)", scope),
                ));
            }
        }
        if let Some(previous) = known {
            if previous != encoded {
                remove_member(&mut connection, &key, &previous).await?;
            }
        }
        add_member(&mut connection, &key, score, &encoded).await?;
        self.remember(scope, &member, &encoded, score);
        scope_count(&mut connection, &key).await
    }

    async fn leave(&self, scope: &PresenceScope, member: &PresenceMember) -> Result<u64> {
        validate_scope(scope)?;
        validate_member(member)?;
        let now = self.clock.now_unix();
        let key = self.scope_key(scope);
        let encoded = encode_member(member)?;
        let known = self.recall(scope, member, now);
        let canonical = known.clone().unwrap_or(encoded);
        let mut connection = self.manager().await?;
        purge_expired(&mut connection, &key, now).await?;
        let removed = remove_member(&mut connection, &key, &canonical).await?;
        if removed > 0 || known.is_some() {
            self.forget(scope, member, now);
        }
        scope_count(&mut connection, &key).await
    }

    async fn touch(
        &self,
        scope: &PresenceScope,
        member: &PresenceMember,
        ttl: Duration,
    ) -> Result<()> {
        validate_scope(scope)?;
        validate_member(member)?;
        let ttl_secs = ttl_secs(ttl)?;
        let now = self.clock.now_unix();
        let score = expires_at(now, ttl_secs);
        let key = self.scope_key(scope);
        let encoded = encode_member(member)?;
        let canonical = self.recall(scope, member, now).unwrap_or(encoded);
        let mut connection = self.manager().await?;
        purge_expired(&mut connection, &key, now).await?;
        // `XX` only updates an existing entry: an unknown or already expired
        // member is not resurrected by a late heartbeat.
        add_existing_member(&mut connection, &key, score, &canonical).await?;
        self.touch_index(scope, member, score);
        Ok(())
    }

    async fn members(&self, scope: &PresenceScope) -> Result<Vec<PresenceMember>> {
        validate_scope(scope)?;
        let now = self.clock.now_unix();
        let key = self.scope_key(scope);
        let mut connection = self.manager().await?;
        purge_expired(&mut connection, &key, now).await?;
        let encoded: Vec<String> = redis::cmd("ZRANGE")
            .arg(&key)
            .arg(0)
            .arg(-1)
            .query_async(&mut connection)
            .await
            .map_err(|error| transport_error("redis presence ZRANGE failed", error))?;
        let mut members: Vec<PresenceMember> = encoded
            .iter()
            .filter_map(|raw| match decode_member(raw) {
                Some(member) => Some(member),
                None => {
                    tracing::warn!(
                        scope = %scope,
                        "skipping malformed redis presence member"
                    );
                    None
                }
            })
            .collect();
        sort_members(&mut members);
        Ok(members)
    }

    async fn count(&self, scope: &PresenceScope) -> Result<u64> {
        validate_scope(scope)?;
        let now = self.clock.now_unix();
        let key = self.scope_key(scope);
        let mut connection = self.manager().await?;
        purge_expired(&mut connection, &key, now).await?;
        scope_count(&mut connection, &key).await
    }
}

/// `{prefix}:scope:{scope}` (or `scope:{scope}` without a prefix).
fn scope_key(prefix: &str, scope: &str) -> String {
    if prefix.is_empty() {
        format!("scope:{scope}")
    } else {
        format!("{prefix}:scope:{scope}")
    }
}

/// Core-equivalent scope validation (`<namespace>:<id>`, both non-empty).
fn validate_scope(scope: &PresenceScope) -> Result<()> {
    let (namespace, id) = scope.as_str().split_once(':').unwrap_or(("", ""));
    if namespace.trim().is_empty() || id.trim().is_empty() {
        return Err(Error::validation(
            "a presence scope must be `<namespace>:<id>` with both parts non-empty",
        ));
    }
    Ok(())
}

/// Core-equivalent member validation.
fn validate_member(member: &PresenceMember) -> Result<()> {
    if member.user_id.trim().is_empty() {
        return Err(Error::validation("a presence member needs a user id"));
    }
    if member.connection_id.trim().is_empty() {
        return Err(Error::validation("a presence member needs a connection id"));
    }
    Ok(())
}

/// Core-equivalent TTL validation (`0` would expire instantly).
fn ttl_secs(ttl: Duration) -> Result<u64> {
    let seconds = ttl.as_secs();
    if seconds == 0 {
        return Err(Error::validation(
            "presence TTL must be at least one second",
        ));
    }
    Ok(seconds)
}

/// Expiry score (Unix seconds); saturating so a huge TTL cannot wrap.
fn expires_at(now_unix: u64, ttl_secs: u64) -> u64 {
    now_unix.saturating_add(ttl_secs)
}

fn encode_member(member: &PresenceMember) -> Result<String> {
    serde_json::to_string(&StoredMember::from(member)).map_err(|error| {
        Error::internal(format!(
            "failed to encode a presence member as JSON: {error}"
        ))
    })
}

fn decode_member(raw: &str) -> Option<PresenceMember> {
    serde_json::from_str::<StoredMember>(raw)
        .ok()
        .map(PresenceMember::from)
}

/// Same ordering as the in-memory store: user id, then connection, then node.
fn sort_members(members: &mut [PresenceMember]) {
    members.sort_by(|left, right| {
        left.user_id
            .cmp(&right.user_id)
            .then_with(|| left.connection_id.cmp(&right.connection_id))
            .then_with(|| left.node_id.cmp(&right.node_id))
    });
}

async fn purge_expired(
    connection: &mut redis::aio::ConnectionManager,
    key: &str,
    now_unix: u64,
) -> Result<()> {
    let now = i64::try_from(now_unix).unwrap_or(i64::MAX);
    redis::cmd("ZREMRANGEBYSCORE")
        .arg(key)
        .arg("-inf")
        .arg(now)
        .query_async::<i64>(connection)
        .await
        .map(|_| ())
        .map_err(|error| transport_error("redis presence ZREMRANGEBYSCORE failed", error))
}

async fn scope_count(connection: &mut redis::aio::ConnectionManager, key: &str) -> Result<u64> {
    redis::cmd("ZCARD")
        .arg(key)
        .query_async(connection)
        .await
        .map_err(|error| transport_error("redis presence ZCARD failed", error))
}

async fn member_exists(
    connection: &mut redis::aio::ConnectionManager,
    key: &str,
    encoded: &str,
) -> Result<bool> {
    let score: Option<f64> = redis::cmd("ZSCORE")
        .arg(key)
        .arg(encoded)
        .query_async(connection)
        .await
        .map_err(|error| transport_error("redis presence ZSCORE failed", error))?;
    Ok(score.is_some())
}

async fn add_member(
    connection: &mut redis::aio::ConnectionManager,
    key: &str,
    score: u64,
    encoded: &str,
) -> Result<()> {
    redis::cmd("ZADD")
        .arg(key)
        .arg(score)
        .arg(encoded)
        .query_async::<i64>(connection)
        .await
        .map(|_| ())
        .map_err(|error| transport_error("redis presence ZADD failed", error))
}

/// Update the score of an existing member only (`ZADD XX`).
async fn add_existing_member(
    connection: &mut redis::aio::ConnectionManager,
    key: &str,
    score: u64,
    encoded: &str,
) -> Result<()> {
    redis::cmd("ZADD")
        .arg(key)
        .arg("XX")
        .arg(score)
        .arg(encoded)
        .query_async::<i64>(connection)
        .await
        .map(|_| ())
        .map_err(|error| transport_error("redis presence ZADD XX failed", error))
}

async fn remove_member(
    connection: &mut redis::aio::ConnectionManager,
    key: &str,
    encoded: &str,
) -> Result<u64> {
    redis::cmd("ZREM")
        .arg(key)
        .arg(encoded)
        .query_async(connection)
        .await
        .map_err(|error| transport_error("redis presence ZREM failed", error))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_connection() -> Arc<RedisConnection> {
        Arc::new(RedisConnection::new(
            "redis://127.0.0.1:1",
            std::time::Duration::from_millis(200),
            std::time::Duration::from_millis(200),
        ))
    }

    fn store() -> RedisOnlinePresence {
        RedisOnlinePresence::new(test_connection())
    }

    fn member(connection_id: &str) -> PresenceMember {
        PresenceMember::new("u1", connection_id)
            .with_node_id("node-a")
            .with_joined_at_unix_secs(1_700_000_000)
    }

    #[test]
    fn scope_keys_are_namespaced() {
        let presence = store();
        assert_eq!(presence.options().prefix, "webx:presence");
        assert_eq!(
            presence.scope_key(&PresenceScope::user("u1")),
            "webx:presence:scope:user:u1"
        );

        let presence = RedisOnlinePresence::with_options(
            test_connection(),
            RedisOnlinePresenceOptions::default().with_prefix("app"),
        );
        assert_eq!(
            presence.scope_key(&PresenceScope::room("r1")),
            "app:scope:room:r1"
        );

        let presence = RedisOnlinePresence::with_options(
            test_connection(),
            RedisOnlinePresenceOptions::default().with_prefix(""),
        );
        assert_eq!(
            presence.scope_key(&PresenceScope::tenant("t1")),
            "scope:tenant:t1"
        );
    }

    #[test]
    fn options_have_bounded_defaults_and_config_defaults() {
        let options = RedisOnlinePresenceOptions::default();
        assert_eq!(options.prefix, DEFAULT_PRESENCE_PREFIX);
        assert_eq!(options.max_members_per_scope, DEFAULT_MAX_MEMBERS_PER_SCOPE);

        let decoded: RedisOnlinePresenceOptions =
            serde_json::from_str("{}").expect("options deserialize");
        assert_eq!(decoded.prefix, DEFAULT_PRESENCE_PREFIX);
        assert_eq!(decoded.max_members_per_scope, DEFAULT_MAX_MEMBERS_PER_SCOPE);

        let decoded: RedisOnlinePresenceOptions =
            serde_json::from_str(r#"{"prefix":"app","max_members_per_scope":4}"#)
                .expect("options deserialize");
        assert_eq!(decoded.prefix, "app");
        assert_eq!(decoded.max_members_per_scope, 4);

        // Zero would reject every join; clamp to one instead.
        let options = RedisOnlinePresenceOptions::default().with_max_members_per_scope(0);
        assert_eq!(options.max_members_per_scope, 1);
    }

    #[test]
    fn members_round_trip_through_json_including_delimiter_characters() {
        let member = PresenceMember::new("user|with|pipes", "c1")
            .with_node_id("node\"a\"")
            .with_joined_at_unix_secs(42);
        let encoded = encode_member(&member).expect("encode member");
        assert_eq!(decode_member(&encoded), Some(member.clone()));

        // A join stamp makes two encodings different, but the identity the
        // store resolves is the same.
        let refreshed = PresenceMember::new("user|with|pipes", "c1")
            .with_node_id("node\"a\"")
            .with_joined_at_unix_secs(99);
        assert_ne!(
            encode_member(&refreshed).expect("encode refreshed"),
            encoded
        );
        assert_eq!(refreshed.key(), member.key());

        assert_eq!(decode_member("not json"), None);
    }

    #[test]
    fn the_identity_index_ignores_the_join_stamp() {
        let presence = store();
        let scope = PresenceScope::user("u1");
        let joined = member("c1");
        let encoded = encode_member(&joined).expect("encode joined");
        presence.remember(&scope, &joined, &encoded, 10);

        let refreshed = PresenceMember::new("u1", "c1")
            .with_node_id("node-a")
            .with_joined_at_unix_secs(9_999);
        assert_eq!(presence.recall(&scope, &refreshed, 0), Some(encoded));
        assert_eq!(
            presence.recall(&PresenceScope::user("u2"), &refreshed, 0),
            None,
            "the index is scoped"
        );

        presence.forget(&scope, &refreshed, 0);
        assert_eq!(presence.recall(&scope, &joined, 0), None);
    }

    #[test]
    fn members_are_sorted_like_the_memory_store() {
        let mut members = vec![
            PresenceMember::new("u2", "c1").with_node_id("n1"),
            PresenceMember::new("u1", "c2").with_node_id("n1"),
            PresenceMember::new("u1", "c1").with_node_id("n2"),
            PresenceMember::new("u1", "c1").with_node_id("n1"),
        ];
        sort_members(&mut members);
        let ordered: Vec<(String, String, String)> = members
            .into_iter()
            .map(|member| (member.user_id, member.connection_id, member.node_id))
            .collect();
        assert_eq!(
            ordered,
            vec![
                ("u1".into(), "c1".into(), "n1".into()),
                ("u1".into(), "c1".into(), "n2".into()),
                ("u1".into(), "c2".into(), "n1".into()),
                ("u2".into(), "c1".into(), "n1".into()),
            ]
        );
    }

    #[test]
    fn expiry_scores_saturate_instead_of_wrapping() {
        assert_eq!(expires_at(1_700_000_000, 60), 1_700_000_060);
        assert_eq!(expires_at(u64::MAX - 1, 60), u64::MAX);
    }

    #[test]
    fn invalid_inputs_are_typed_validation_errors() {
        let scope = PresenceScope::user("u1");
        let member = member("c1");
        assert!(validate_scope(&scope).is_ok());
        assert!(validate_scope(&PresenceScope::custom("", "x")).is_err());
        assert!(validate_scope(&PresenceScope::custom("ns", "  ")).is_err());
        assert!(validate_scope(&PresenceScope::custom("nonsense", "")).is_err());
        assert!(validate_member(&member).is_ok());
        assert!(validate_member(&PresenceMember::new("", "c1")).is_err());
        assert!(validate_member(&PresenceMember::new("u1", "")).is_err());
        assert_eq!(ttl_secs(Duration::from_secs(30)).expect("ttl"), 30);
        assert!(ttl_secs(Duration::ZERO).is_err());
        assert!(ttl_secs(Duration::from_millis(999)).is_err());
    }

    #[tokio::test]
    async fn mutating_inputs_are_rejected_before_any_connection_attempt() {
        let presence = store();
        let scope = PresenceScope::user("u1");
        let error = presence
            .join(&scope, member("c1"), Duration::ZERO)
            .await
            .expect_err("a zero TTL must be rejected");
        assert_eq!(error.status_code(), 400);

        let error = presence
            .join(
                &PresenceScope::custom("invalid", ""),
                member("c1"),
                Duration::from_secs(30),
            )
            .await
            .expect_err("an invalid scope must be rejected");
        assert_eq!(error.status_code(), 400);

        let error = presence
            .touch(
                &scope,
                &PresenceMember::new("", "c1"),
                Duration::from_secs(30),
            )
            .await
            .expect_err("an empty user id must be rejected");
        assert_eq!(error.status_code(), 400);
    }
}

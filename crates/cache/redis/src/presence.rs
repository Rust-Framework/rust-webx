//! Redis-backed shared online presence ([`IOnlinePresence`]).
//!
//! Two keys per scope:
//!
//! ```text
//! {prefix}:scope:{scope}     ZSET: member = stable identity, score = expiry unix seconds
//! {prefix}:members:{scope}   HASH: field = stable identity, value = PresenceMember JSON
//! ```
//!
//! The HASH lives under a different suffix segment (`members:` instead of
//! `scope:`) than the sorted set on purpose: a second key type inside the
//! `scope:` namespace could collide with the sorted set of a scope whose
//! rendered id merely ends with the HASH suffix (`user:u1:member` vs
//! `user:u1`), and a cross-type collision is a hard `WRONGTYPE` failure.
//!
//! # Stable identity
//!
//! The ZSET member is the JSON encoding of `[user_id, connection_id, node_id]`.
//! JSON arrays are delimiter-safe (`user|with|pipes` cannot collide with
//! another triple) and stable: the join stamp is **not** part of the identity, so
//! a `leave`/`touch` built from a fresh [`PresenceMember`] addresses a member
//! joined by another node when the `(user_id, connection_id, node_id)` triple
//! matches. The host's node id is per-process, so after a restart the old entry
//! is no longer addressable and expires by TTL. `joined_at_unix_secs` is stored
//! only in the HASH payload, which is why `members` can round-trip it.
//!
//! # Atomicity and expiry
//!
//! Every operation is one Lua script. The shared preamble removes expired ZSET
//! members in bounded `ZRANGEBYSCORE` batches and deletes the same fields from
//! the HASH, so expiry cleanup never needs to parse a member and the HASH cannot
//! accumulate stale fields (`unpack` has an argument-count limit, hence
//! batching). Because the cleanup and the mutation/read share one script, no
//! node can observe an expired member, and `join` checks `ZCARD` against
//! `max_members_per_scope` in the same atomic step that writes: concurrent joins
//! cannot overshoot the bound. A heartbeat (`touch`) uses `ZADD XX` after the
//! purge, so it neither resurrects an expired member nor rewrites the stored
//! join stamp.
//!
//! # Compatibility with 2.9.2 data and rolling upgrades
//!
//! 2.9.2 stored the whole `PresenceMember` JSON as the ZSET member (no HASH) and
//! resolved mutations through a process-local identity index. This version reads
//! those legacy members: [`IOnlinePresence::members`] falls back to decoding a
//! JSON member when the raw value is not an identity, and the shared purge
//! removes legacy members by score (`HDEL` on a JSON field is a no-op). The
//! migration converges within one TTL:
//!
//! * A legacy member whose owning process already upgraded is no longer touched
//!   (the new `touch` addresses the identity, not the JSON member), so it simply
//!   expires and is purged.
//! * `leave`/`join` never scan the whole scope, so a legacy JSON member of the
//!   same logical identity is not removed by an identity `leave`; it disappears
//!   when its TTL elapses. Until then it can be counted/listed in addition to
//!   the new identity entry.
//! * A 2.9.2 node reading the new identity members skips them as malformed
//!   (its `count` still includes them), and a 2.9.2 `join` writes a second,
//!   JSON-encoded member for the same identity. During a mixed-version window,
//!   treat the new nodes as authoritative; after all nodes upgrade, the old
//!   entries are gone within one TTL.
//!
//! The scripts address two keys per call, which targets a single logical Redis
//! (standalone or Sentinel, as before); Redis Cluster needs a dedicated adapter
//! (documented in `docs/design/upstream-gaps.md`).
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

use std::sync::Arc;
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

/// Wrap a script body with the shared expiry-cleanup preamble.
///
/// One definition of "purge expired members and their HASH payloads" so the
/// five operations cannot drift apart. The batch literal (256) keeps every
/// `ZREM`/`HDEL` well below the Lua `unpack` argument limit while making a
/// 100k-member purge take a bounded number of script iterations.
macro_rules! presence_script {
    ($($body:literal)+) => {
        concat!(
            r#"
-- Shared preamble: remove expired ZSET members and their HASH payloads.
-- Batched because `unpack` (Lua 5.1, used by Redis) has an argument limit.
local function purge_expired(key_zset, key_members, now)
  while true do
    local expired = redis.call('ZRANGEBYSCORE', key_zset, '-inf', now, 'LIMIT', 0, 256)
    if #expired == 0 then
      break
    end
    redis.call('ZREM', key_zset, unpack(expired))
    redis.call('HDEL', key_members, unpack(expired))
  end
end
"#,
            $($body)+
        )
    };
}

/// Atomic join: purge, refresh-or-capacity-check, then write both keys.
///
/// KEYS[1] = scope ZSET, KEYS[2] = member HASH.
/// ARGV[1] = identity, ARGV[2] = expiry unix seconds, ARGV[3] = max members,
/// ARGV[4] = `PresenceMember` JSON, ARGV[5] = now unix seconds.
/// Returns `{1, live count}` on success and `{0, live count}` when the scope is
/// full; a rejected join writes no member state.
const JOIN_SCRIPT: &str = presence_script!(
    r#"
local identity = ARGV[1]
local expires_at = ARGV[2]
local max_members = tonumber(ARGV[3])
purge_expired(KEYS[1], KEYS[2], ARGV[5])
if redis.call('ZSCORE', KEYS[1], identity) then
  -- Same identity: refresh the TTL and the stored member (join replaces the
  -- payload, including its join stamp, like the in-memory store), no new slot.
  redis.call('ZADD', KEYS[1], expires_at, identity)
  redis.call('HSET', KEYS[2], identity, ARGV[4])
  return {1, redis.call('ZCARD', KEYS[1])}
end
local count = redis.call('ZCARD', KEYS[1])
if count >= max_members then
  return {0, count}
end
redis.call('ZADD', KEYS[1], expires_at, identity)
redis.call('HSET', KEYS[2], identity, ARGV[4])
return {1, count + 1}
"#
);

/// Atomic leave by identity: purge, then remove the ZSET member and its payload.
///
/// KEYS[1] = scope ZSET, KEYS[2] = member HASH.
/// ARGV[1] = identity, ARGV[2] = now unix seconds.
/// Returns the remaining live scope count.
const LEAVE_SCRIPT: &str = presence_script!(
    r#"
purge_expired(KEYS[1], KEYS[2], ARGV[2])
redis.call('ZREM', KEYS[1], ARGV[1])
redis.call('HDEL', KEYS[2], ARGV[1])
return redis.call('ZCARD', KEYS[1])
"#
);

/// Atomic heartbeat: purge, then extend the TTL of a **live** member only.
///
/// KEYS[1] = scope ZSET, KEYS[2] = member HASH.
/// ARGV[1] = identity, ARGV[2] = expiry unix seconds, ARGV[3] = now unix seconds.
/// `ZADD XX` after the purge: an expired member is already gone (no
/// resurrection) and the HASH payload is untouched (the join stamp is stable).
const TOUCH_SCRIPT: &str = presence_script!(
    r#"
purge_expired(KEYS[1], KEYS[2], ARGV[3])
return redis.call('ZADD', KEYS[1], 'XX', ARGV[2], ARGV[1])
"#
);

/// Atomic read of the live ZSET members (expiry cleanup included).
///
/// KEYS[1] = scope ZSET, KEYS[2] = member HASH.
/// ARGV[1] = now unix seconds.
/// Returns `{identity-or-legacy-JSON, ...}`; the caller reads the payloads back
/// with one `HMGET` in the same order.
const MEMBERS_SCRIPT: &str = presence_script!(
    r#"
purge_expired(KEYS[1], KEYS[2], ARGV[1])
return redis.call('ZRANGE', KEYS[1], 0, -1)
"#
);

/// Atomic count of the live members (expiry cleanup included).
///
/// KEYS[1] = scope ZSET, KEYS[2] = member HASH.
/// ARGV[1] = now unix seconds.
const COUNT_SCRIPT: &str = presence_script!(
    r#"
purge_expired(KEYS[1], KEYS[2], ARGV[1])
return redis.call('ZCARD', KEYS[1])
"#
);

/// Options for [`RedisOnlinePresence`].
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct RedisOnlinePresenceOptions {
    /// Key prefix: each scope uses `{prefix}:scope:{scope}` (expiry sorted set)
    /// and `{prefix}:members:{scope}` (member payloads). Use a distinct prefix
    /// per application so several services can share one Redis.
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

/// JSON form of a [`PresenceMember`], stored as the HASH payload (and read as a
/// fallback for members written by 2.9.2).
///
/// A local mirror is needed because the core type is not (and should not be)
/// serde-bound.
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
/// Safe to share across nodes: the stable identity and the shared ZSET/HASH
/// are the only state, so there is no process-local index to lose. Mutations
/// address the exact `(user_id, connection_id, node_id)` triple; the host's node
/// id is per-process, so entries left by a crashed/restarted process expire by
/// TTL. The TTL is refreshed by the application or the host presence heartbeat.
pub struct RedisOnlinePresence {
    connection: Arc<RedisConnection>,
    options: RedisOnlinePresenceOptions,
    clock: Arc<dyn IClock>,
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

    /// Hash key holding the member payloads of `scope` (crate-internal).
    pub(crate) fn members_key(&self, scope: &PresenceScope) -> String {
        members_key(&self.options.prefix, scope.as_str())
    }

    fn max_members(&self) -> usize {
        self.options.max_members_per_scope.max(1)
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
        let expires_at = expires_at(now, ttl_secs);
        let identity = identity_key(&member)?;
        let payload = encode_member(&member)?;
        let max_members = self.max_members();
        let mut connection = self.manager().await?;
        let (joined, count): (i64, u64) = redis::cmd("EVAL")
            .arg(JOIN_SCRIPT)
            .arg(2)
            .arg(self.scope_key(scope))
            .arg(self.members_key(scope))
            .arg(&identity)
            .arg(expires_at)
            .arg(max_members as u64)
            .arg(&payload)
            .arg(now)
            .query_async(&mut connection)
            .await
            .map_err(|error| transport_error("redis presence join failed", error))?;
        if joined == 0 {
            return Err(Error::status(
                503,
                format!("presence scope '{}' is full ({max_members} members)", scope),
            ));
        }
        Ok(count)
    }

    async fn leave(&self, scope: &PresenceScope, member: &PresenceMember) -> Result<u64> {
        validate_scope(scope)?;
        validate_member(member)?;
        let now = self.clock.now_unix();
        let identity = identity_key(member)?;
        let mut connection = self.manager().await?;
        redis::cmd("EVAL")
            .arg(LEAVE_SCRIPT)
            .arg(2)
            .arg(self.scope_key(scope))
            .arg(self.members_key(scope))
            .arg(&identity)
            .arg(now)
            .query_async(&mut connection)
            .await
            .map_err(|error| transport_error("redis presence leave failed", error))
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
        let expires_at = expires_at(now, ttl_secs);
        let identity = identity_key(member)?;
        let mut connection = self.manager().await?;
        let _updated: i64 = redis::cmd("EVAL")
            .arg(TOUCH_SCRIPT)
            .arg(2)
            .arg(self.scope_key(scope))
            .arg(self.members_key(scope))
            .arg(&identity)
            .arg(expires_at)
            .arg(now)
            .query_async(&mut connection)
            .await
            .map_err(|error| transport_error("redis presence touch failed", error))?;
        Ok(())
    }

    async fn members(&self, scope: &PresenceScope) -> Result<Vec<PresenceMember>> {
        validate_scope(scope)?;
        let now = self.clock.now_unix();
        let mut connection = self.manager().await?;
        let raw: Vec<String> = redis::cmd("EVAL")
            .arg(MEMBERS_SCRIPT)
            .arg(2)
            .arg(self.scope_key(scope))
            .arg(self.members_key(scope))
            .arg(now)
            .query_async(&mut connection)
            .await
            .map_err(|error| transport_error("redis presence members failed", error))?;
        let payloads = member_payloads(&mut connection, &self.members_key(scope), &raw).await?;
        let mut members = decode_members(&raw, &payloads);
        sort_members(&mut members);
        Ok(members)
    }

    async fn count(&self, scope: &PresenceScope) -> Result<u64> {
        validate_scope(scope)?;
        let now = self.clock.now_unix();
        let mut connection = self.manager().await?;
        redis::cmd("EVAL")
            .arg(COUNT_SCRIPT)
            .arg(2)
            .arg(self.scope_key(scope))
            .arg(self.members_key(scope))
            .arg(now)
            .query_async(&mut connection)
            .await
            .map_err(|error| transport_error("redis presence count failed", error))
    }
}

/// `{prefix}:scope:{scope}` (or `scope:{scope}` without a prefix): expiry ZSET.
fn scope_key(prefix: &str, scope: &str) -> String {
    if prefix.is_empty() {
        format!("scope:{scope}")
    } else {
        format!("{prefix}:scope:{scope}")
    }
}

/// `{prefix}:members:{scope}` (or `members:{scope}` without a prefix): payload
/// HASH. The suffix segment differs from the ZSET's `scope:` segment so no two
/// key roles can ever be the same Redis key.
fn members_key(prefix: &str, scope: &str) -> String {
    if prefix.is_empty() {
        format!("members:{scope}")
    } else {
        format!("{prefix}:members:{scope}")
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
    validate_component("presence scope", scope.as_str())?;
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
    validate_component("member.user_id", &member.user_id)?;
    validate_component("member.connection_id", &member.connection_id)?;
    if !member.node_id.is_empty() {
        validate_component("member.node_id", &member.node_id)?;
    }
    Ok(())
}

/// Bounded, printable key component (same rule as the core in-memory adapter).
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

/// Stable ZSET member: JSON array `[user_id, connection_id, node_id]`.
///
/// Delimiter-safe by construction (unlike `PresenceMember::key`, whose `|`
/// separator can be forged inside a user id) and join-stamp independent.
fn identity_key(member: &PresenceMember) -> Result<String> {
    serde_json::to_string(&[
        member.user_id.as_str(),
        member.connection_id.as_str(),
        member.node_id.as_str(),
    ])
    .map_err(|error| {
        Error::internal(format!(
            "failed to encode a presence member identity as JSON: {error}"
        ))
    })
}

/// Decode a stable identity written by this version (`[str; 3]`).
///
/// A 2.9.2 member is a JSON **object**, so it never decodes as an identity and
/// falls through to [`decode_member`].
fn decode_identity(raw: &str) -> Option<[String; 3]> {
    serde_json::from_str(raw).ok()
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

/// Pair the raw live ZSET members with their `HMGET` payloads.
///
/// `ids` and `payloads` are in the same order. An identity entry takes its
/// `PresenceMember` from the HASH payload (a member that left between the two
/// reads has `None` and is skipped); a raw value that is not an identity is a
/// 2.9.2 JSON member and is decoded directly; anything undecodable is malformed
/// and skipped, like the previous implementation did.
fn decode_members(ids: &[String], payloads: &[Option<String>]) -> Vec<PresenceMember> {
    let mut members = Vec::with_capacity(ids.len());
    for (index, raw) in ids.iter().enumerate() {
        if decode_identity(raw).is_some() {
            match payloads.get(index).and_then(|payload| payload.as_deref()) {
                Some(payload) => match decode_member(payload) {
                    Some(member) => members.push(member),
                    None => tracing::warn!("skipping malformed redis presence payload"),
                },
                // A member that left between the ZRANGE and the HMGET, or a
                // HASH that lost its field independently. Not fatal: the ZSET
                // entry is authoritative and it is gone/cleaned next script.
                None => tracing::debug!("skipping redis presence identity without a payload"),
            }
        } else if let Some(legacy) = decode_member(raw) {
            // 2.9.2 wrote the whole member JSON as the sorted-set member.
            members.push(legacy);
        } else {
            tracing::warn!("skipping malformed redis presence member");
        }
    }
    members
}

/// Read the HASH payloads for the raw ZSET members in one `HMGET`.
async fn member_payloads(
    connection: &mut redis::aio::ConnectionManager,
    key: &str,
    ids: &[String],
) -> Result<Vec<Option<String>>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    // `HMGET` rejects an empty field list, hence the early return above.
    redis::cmd("HMGET")
        .arg(key)
        .arg(ids)
        .query_async(connection)
        .await
        .map_err(|error| transport_error("redis presence HMGET failed", error))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_or_control_key_components_are_rejected() {
        let long = "x".repeat(513);
        assert!(validate_scope(&PresenceScope::user(long.clone())).is_err());
        assert!(validate_member(&PresenceMember::new(long, "c1")).is_err());
        assert!(validate_member(&PresenceMember::new("u1", "c1\n")).is_err());
        assert!(validate_member(&PresenceMember::new("u1", "c1").with_node_id("node-a")).is_ok());
    }

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
    fn scope_and_member_keys_are_namespaced() {
        let presence = store();
        assert_eq!(presence.options().prefix, "webx:presence");
        assert_eq!(
            presence.scope_key(&PresenceScope::user("u1")),
            "webx:presence:scope:user:u1"
        );
        assert_eq!(
            presence.members_key(&PresenceScope::user("u1")),
            "webx:presence:members:user:u1"
        );

        let presence = RedisOnlinePresence::with_options(
            test_connection(),
            RedisOnlinePresenceOptions::default().with_prefix("app"),
        );
        assert_eq!(
            presence.scope_key(&PresenceScope::room("r1")),
            "app:scope:room:r1"
        );
        assert_eq!(
            presence.members_key(&PresenceScope::room("r1")),
            "app:members:room:r1"
        );

        let presence = RedisOnlinePresence::with_options(
            test_connection(),
            RedisOnlinePresenceOptions::default().with_prefix(""),
        );
        assert_eq!(
            presence.scope_key(&PresenceScope::tenant("t1")),
            "scope:tenant:t1"
        );
        assert_eq!(
            presence.members_key(&PresenceScope::tenant("t1")),
            "members:tenant:t1"
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
    fn stored_member_round_trips_through_json_including_delimiter_characters() {
        let member = PresenceMember::new("user|with|pipes", "c1")
            .with_node_id("node\"a\"")
            .with_joined_at_unix_secs(42);
        let encoded = encode_member(&member).expect("encode member");
        assert_eq!(decode_member(&encoded), Some(member.clone()));

        assert_eq!(decode_member("not json"), None);
    }

    #[test]
    fn identity_encoding_is_delimiter_safe_and_ignores_the_join_stamp() {
        let joined = member("c1");
        let refreshed = PresenceMember::new("u1", "c1")
            .with_node_id("node-a")
            .with_joined_at_unix_secs(9_999);
        assert_eq!(
            identity_key(&joined).expect("identity"),
            identity_key(&refreshed).expect("identity"),
            "the join stamp is not part of the identity"
        );

        // Both `PresenceMember::key()` and the Redis identity encoding are
        // delimiter-safe, so an id containing `|` cannot collide with another
        // triple. (The core key is length-prefixed since this change.)
        let pipes_in_user = PresenceMember::new("a|b", "c").with_node_id("n");
        let pipes_in_connection = PresenceMember::new("a", "b|c").with_node_id("n");
        assert_ne!(pipes_in_user.key(), pipes_in_connection.key());
        assert_ne!(
            identity_key(&pipes_in_user).expect("identity"),
            identity_key(&pipes_in_connection).expect("identity"),
            "delimiter characters must not collide identities"
        );

        // Same identity, different scopes: the scope is part of the key layout,
        // not of the encoding.
        assert_eq!(
            identity_key(&joined).expect("identity"),
            identity_key(&PresenceMember::new("u1", "c1").with_node_id("node-a"))
                .expect("identity")
        );
    }

    #[test]
    fn identity_decoding_only_accepts_identity_arrays() {
        let identity = identity_key(&member("c1")).expect("identity");
        assert_eq!(
            decode_identity(&identity),
            Some(["u1".to_string(), "c1".to_string(), "node-a".to_string()])
        );
        assert_eq!(
            decode_identity(&encode_member(&member("c1")).expect("payload")),
            None,
            "a 2.9.2 member JSON object is not an identity"
        );
        assert_eq!(decode_identity("not json"), None);
        assert_eq!(decode_identity(r#"["u1","c1"]"#), None);
        assert_eq!(decode_identity(r#"["u1","c1","n",1]"#), None);
    }

    #[test]
    fn members_decode_identities_legacy_json_and_skip_malformed() {
        let joined = member("c1");
        let identity = identity_key(&joined).expect("identity");
        let payload = encode_member(&joined).expect("payload");

        let legacy = PresenceMember::new("u1", "legacy")
            .with_node_id("node-b")
            .with_joined_at_unix_secs(500);
        let legacy_json = encode_member(&legacy).expect("legacy payload");

        let orphan_identity = identity_key(&member("c2")).expect("identity");
        let raw = vec![
            identity,
            legacy_json,
            "garbage".to_string(),
            orphan_identity,
            "u1|c1|node-a".to_string(),
        ];
        let payloads = vec![
            Some(payload),
            None, // legacy JSON has no HASH field
            None,
            None, // identity with a missing payload (concurrent leave)
            None,
        ];
        let decoded = decode_members(&raw, &payloads);
        assert_eq!(decoded, vec![joined, legacy]);

        // A malformed HASH payload is skipped too, not panicked on.
        let raw = vec![identity_key(&member("c1")).expect("identity")];
        let payloads = vec![Some("not json".to_string())];
        assert!(decode_members(&raw, &payloads).is_empty());
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
    fn scripts_purge_expired_members_and_keep_the_hash_in_sync() {
        let scripts = [
            JOIN_SCRIPT,
            LEAVE_SCRIPT,
            TOUCH_SCRIPT,
            MEMBERS_SCRIPT,
            COUNT_SCRIPT,
        ];
        for script in scripts {
            assert_eq!(
                script.matches("local function purge_expired").count(),
                1,
                "every script uses the shared purge preamble exactly once"
            );
            assert!(script.contains("redis.call('ZRANGEBYSCORE'"));
            assert!(
                script.contains("redis.call('HDEL'"),
                "purge must delete the HASH payload together with the ZSET member"
            );
        }
        assert!(JOIN_SCRIPT.contains("'ZCARD'") && JOIN_SCRIPT.contains("max_members"));
        assert!(JOIN_SCRIPT.contains("'HSET'") && JOIN_SCRIPT.contains("'ZADD'"));
        assert!(LEAVE_SCRIPT.contains("'ZREM'") && LEAVE_SCRIPT.contains("'HDEL'"));
        assert!(TOUCH_SCRIPT.contains("'ZADD', KEYS[1], 'XX'"));
        assert!(
            !TOUCH_SCRIPT.contains("HSET"),
            "a heartbeat must not rewrite the stored join stamp"
        );
        assert!(MEMBERS_SCRIPT.contains("'ZRANGE'"));
        assert!(COUNT_SCRIPT.contains("'ZCARD'"));
        assert!(
            JOIN_SCRIPT.contains("'LIMIT', 0, 256"),
            "keep the documented purge batch literal in sync"
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

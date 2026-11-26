//! Live-Redis tests for the shared-presence identity and capacity semantics.
//!
//! Set `WEBX_REDIS_URL` (for example `redis://127.0.0.1:6379`) to run them;
//! without a broker every test returns early and passes. Each test uses its own
//! key prefix, so tests never see each other's presence state.
//!
//! These tests pin the external key layout on purpose:
//!
//! ```text
//! {prefix}:scope:{scope}     ZSET: member = stable identity, score = expiry
//! {prefix}:members:{scope}   HASH: field = identity, value = PresenceMember JSON
//! ```

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use webx_core::realtime::{IOnlinePresence, PresenceMember, PresenceScope};
use webx_redis::{RedisConnection, RedisOnlinePresence, RedisOnlinePresenceOptions};

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_test_writer()
        .try_init();
}

fn unique_suffix() -> String {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    format!(
        "{nanos}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

fn zset_key(prefix: &str, scope: &PresenceScope) -> String {
    format!("{prefix}:scope:{}", scope.as_str())
}

fn members_key(prefix: &str, scope: &PresenceScope) -> String {
    format!("{prefix}:members:{}", scope.as_str())
}

fn connection(url: &str) -> Arc<RedisConnection> {
    Arc::new(RedisConnection::new(
        url,
        Duration::from_secs(15),
        Duration::from_secs(15),
    ))
}

fn store(
    connection: &Arc<RedisConnection>,
    prefix: &str,
    max_members_per_scope: usize,
) -> Arc<RedisOnlinePresence> {
    Arc::new(RedisOnlinePresence::with_options(
        Arc::clone(connection),
        RedisOnlinePresenceOptions {
            prefix: prefix.to_string(),
            max_members_per_scope,
        },
    ))
}

/// Raw `DEL` of both key roles so a test leaves no state behind.
async fn cleanup(connection: &RedisConnection, prefix: &str, scope: &PresenceScope) {
    let Ok(mut raw) = connection.manager().await else {
        return;
    };
    let _: i64 = redis::cmd("DEL")
        .arg(zset_key(prefix, scope))
        .arg(members_key(prefix, scope))
        .query_async(&mut raw)
        .await
        .expect("cleanup DEL");
}

async fn raw_zcard(connection: &RedisConnection, prefix: &str, scope: &PresenceScope) -> i64 {
    let mut raw = connection.manager().await.expect("raw redis connection");
    redis::cmd("ZCARD")
        .arg(zset_key(prefix, scope))
        .query_async(&mut raw)
        .await
        .expect("ZCARD")
}

async fn raw_hlen(connection: &RedisConnection, prefix: &str, scope: &PresenceScope) -> i64 {
    let mut raw = connection.manager().await.expect("raw redis connection");
    redis::cmd("HLEN")
        .arg(members_key(prefix, scope))
        .query_async(&mut raw)
        .await
        .expect("HLEN")
}

/// A cross-node `leave`/`touch` is built from a different join stamp (and would
/// arrive on another node or after a process restart): the stable identity still
/// addresses the member, and a heartbeat keeps the original join stamp.
#[tokio::test]
async fn cross_node_leave_and_touch_use_the_stable_identity() {
    init_tracing();
    let Ok(url) = std::env::var("WEBX_REDIS_URL") else {
        return;
    };
    let suffix = unique_suffix();
    let prefix = format!("webx:test:presence:identity:{suffix}");
    let node_a = connection(&url);
    let node_b = connection(&url);
    let presence_a = store(&node_a, &prefix, 10);
    let presence_b = store(&node_b, &prefix, 10);
    let scope = PresenceScope::user(format!("u-{suffix}"));
    let user_id = format!("u-{suffix}");

    let joined = PresenceMember::new(&user_id, "c1")
        .with_node_id("node-a")
        .with_joined_at_unix_secs(1_000);
    assert_eq!(
        presence_a
            .join(&scope, joined.clone(), Duration::from_secs(60))
            .await
            .expect("join"),
        1
    );

    // The other instance sees the member and its stored join stamp.
    assert_eq!(presence_b.count(&scope).await.expect("count"), 1);
    assert_eq!(
        presence_b.members(&scope).await.expect("members"),
        vec![joined.clone()]
    );

    // A heartbeat from another node with a fresh stamp extends the TTL without
    // rewriting the join stamp.
    let heartbeat = PresenceMember::new(&user_id, "c1")
        .with_node_id("node-a")
        .with_joined_at_unix_secs(9_999);
    presence_b
        .touch(&scope, &heartbeat, Duration::from_secs(60))
        .await
        .expect("touch");
    assert_eq!(
        presence_a.members(&scope).await.expect("members"),
        vec![joined.clone()],
        "a heartbeat must not rewrite the join stamp"
    );

    // A leave from the other node (or a restarted process) with yet another
    // stamp removes the member.
    let leaving = PresenceMember::new(&user_id, "c1")
        .with_node_id("node-a")
        .with_joined_at_unix_secs(12_345);
    assert_eq!(
        presence_b.leave(&scope, &leaving).await.expect("leave"),
        0,
        "leave from another node must remove the member joined with a different stamp"
    );
    assert_eq!(presence_a.count(&scope).await.expect("count"), 0);
    assert!(presence_a
        .members(&scope)
        .await
        .expect("members after leave")
        .is_empty());
    assert_eq!(raw_zcard(&node_a, &prefix, &scope).await, 0);
    assert_eq!(raw_hlen(&node_a, &prefix, &scope).await, 0);

    // A heartbeat for a member that is not there is a no-op, not an error.
    presence_a
        .touch(&scope, &heartbeat, Duration::from_secs(60))
        .await
        .expect("touch of an unknown member is a no-op");
    assert_eq!(presence_a.count(&scope).await.expect("count"), 0);

    cleanup(&node_a, &prefix, &scope).await;
}

/// `max_members_per_scope = 1` and eight concurrent joins from two independent
/// instances: the capacity check and the write are one atomic step, so exactly
/// one join wins and the scope never overshoots.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_joins_cannot_overshoot_capacity() {
    init_tracing();
    let Ok(url) = std::env::var("WEBX_REDIS_URL") else {
        return;
    };
    let suffix = unique_suffix();
    let prefix = format!("webx:test:presence:capacity:{suffix}");
    let node_a = connection(&url);
    let node_b = connection(&url);
    let presence_a = store(&node_a, &prefix, 1);
    let presence_b = store(&node_b, &prefix, 1);
    let scope = PresenceScope::user(format!("u-{suffix}"));
    let user_id = format!("u-{suffix}");

    let mut tasks = Vec::new();
    for index in 0..8u32 {
        let presence = if index % 2 == 0 {
            Arc::clone(&presence_a)
        } else {
            Arc::clone(&presence_b)
        };
        let scope = scope.clone();
        let user_id = user_id.clone();
        tasks.push(tokio::spawn(async move {
            let member = PresenceMember::new(user_id, format!("c{index}"))
                .with_node_id(format!("node-{index}"));
            presence.join(&scope, member, Duration::from_secs(60)).await
        }));
    }

    let mut accepted = 0usize;
    let mut rejected = 0usize;
    for task in tasks {
        match task.await.expect("a concurrent join must not panic") {
            Ok(count) => {
                accepted += 1;
                assert_eq!(count, 1, "the winner sees the single live member");
            }
            Err(error) => {
                rejected += 1;
                assert_eq!(error.status_code(), 503);
                assert!(error.retryable(), "a full scope must be retryable");
            }
        }
    }
    assert_eq!(
        (accepted, rejected),
        (1, 7),
        "exactly one of eight concurrent joins may win a max=1 scope"
    );
    assert_eq!(presence_a.count(&scope).await.expect("count"), 1);
    assert_eq!(presence_b.count(&scope).await.expect("count"), 1);
    assert_eq!(raw_zcard(&node_a, &prefix, &scope).await, 1);
    assert_eq!(
        raw_hlen(&node_a, &prefix, &scope).await,
        1,
        "the rejected joins wrote no payload"
    );

    // Refreshing the identity that already holds the single slot is not a new
    // member: it must succeed even while the scope is full, without adding one.
    let winner = presence_a
        .members(&scope)
        .await
        .expect("members")
        .into_iter()
        .next()
        .expect("the single live member");
    let refreshed = PresenceMember::new(winner.user_id.clone(), winner.connection_id.clone())
        .with_node_id(winner.node_id.clone())
        .with_joined_at_unix_secs(winner.joined_at_unix_secs + 1);
    assert_eq!(
        presence_b
            .join(&scope, refreshed, Duration::from_secs(60))
            .await
            .expect("refreshing the existing identity must not be rejected"),
        1,
        "a refresh does not consume a second slot"
    );
    assert_eq!(raw_zcard(&node_a, &prefix, &scope).await, 1);
    assert_eq!(raw_hlen(&node_a, &prefix, &scope).await, 1);

    cleanup(&node_a, &prefix, &scope).await;
}

/// Expiry cleanup runs in bounded batches inside the scripts; a burst larger
/// than one `unpack` batch is purged completely, HASH included.
#[tokio::test]
async fn purge_removes_expired_members_in_batches() {
    init_tracing();
    let Ok(url) = std::env::var("WEBX_REDIS_URL") else {
        return;
    };
    let suffix = unique_suffix();
    let prefix = format!("webx:test:presence:purge-batch:{suffix}");
    let node = connection(&url);
    let presence = store(&node, &prefix, 10);
    let scope = PresenceScope::user(format!("u-{suffix}"));
    let user_id = format!("u-{suffix}");
    let expired_at = now_unix().saturating_sub(10);

    // More expired entries than one purge batch (256).
    let mut zadd = redis::cmd("ZADD");
    zadd.arg(zset_key(&prefix, &scope));
    let mut hset = redis::cmd("HSET");
    hset.arg(members_key(&prefix, &scope));
    for index in 0..300 {
        let member = PresenceMember::new(&user_id, format!("expired-{index}"))
            .with_node_id("node-a")
            .with_joined_at_unix_secs(1);
        let identity = serde_json::to_string(&[
            member.user_id.as_str(),
            member.connection_id.as_str(),
            member.node_id.as_str(),
        ])
        .expect("identity");
        let payload = serde_json::json!({
            "user_id": member.user_id,
            "connection_id": member.connection_id,
            "node_id": member.node_id,
            "joined_at_unix_secs": member.joined_at_unix_secs,
        })
        .to_string();
        zadd.arg(expired_at).arg(&identity);
        hset.arg(&identity).arg(&payload);
    }
    let mut raw = node.manager().await.expect("raw redis connection");
    let _: i64 = zadd.query_async(&mut raw).await.expect("seed expired ZSET");
    let _: i64 = hset.query_async(&mut raw).await.expect("seed expired HASH");
    assert_eq!(raw_zcard(&node, &prefix, &scope).await, 300);
    assert_eq!(raw_hlen(&node, &prefix, &scope).await, 300);

    // Joining a live member purges every expired batch first and only sees the
    // live count.
    assert_eq!(
        presence
            .join(
                &scope,
                PresenceMember::new(&user_id, "live").with_node_id("node-a"),
                Duration::from_secs(60),
            )
            .await
            .expect("join over expired members"),
        1
    );
    assert_eq!(raw_zcard(&node, &prefix, &scope).await, 1);
    assert_eq!(
        raw_hlen(&node, &prefix, &scope).await,
        1,
        "every expired batch is deleted from the HASH as well"
    );
    assert_eq!(presence.count(&scope).await.expect("count"), 1);

    cleanup(&node, &prefix, &scope).await;
}

/// The purge that runs first in every script removes the HASH payload together
/// with the expired ZSET member, and a late heartbeat never resurrects it.
#[tokio::test]
async fn expired_member_is_not_resurrected_and_purge_clears_the_hash() {
    init_tracing();
    let Ok(url) = std::env::var("WEBX_REDIS_URL") else {
        return;
    };
    let suffix = unique_suffix();
    let prefix = format!("webx:test:presence:expiry:{suffix}");
    let node = connection(&url);
    let presence = store(&node, &prefix, 10);
    let scope = PresenceScope::room(format!("r-{suffix}"));
    let member = PresenceMember::new(format!("u-{suffix}"), "c1")
        .with_node_id("node-a")
        .with_joined_at_unix_secs(1_000);

    presence
        .join(&scope, member.clone(), Duration::from_secs(1))
        .await
        .expect("join");
    assert_eq!(raw_zcard(&node, &prefix, &scope).await, 1);
    assert_eq!(raw_hlen(&node, &prefix, &scope).await, 1);

    tokio::time::sleep(Duration::from_millis(1_200)).await;

    // A heartbeat after the TTL expired is a no-op (`ZADD XX` after the purge)
    // instead of resurrecting the member.
    presence
        .touch(&scope, &member, Duration::from_secs(60))
        .await
        .expect("touch");
    assert_eq!(
        presence.count(&scope).await.expect("count"),
        0,
        "touch cannot resurrect an expired member"
    );
    assert!(presence.members(&scope).await.expect("members").is_empty());
    assert_eq!(raw_zcard(&node, &prefix, &scope).await, 0);
    assert_eq!(
        raw_hlen(&node, &prefix, &scope).await,
        0,
        "expiry cleanup must delete the HASH payload, not only the ZSET member"
    );

    cleanup(&node, &prefix, &scope).await;
}

/// Both instances read the same live set; a join of an already-known identity
/// from another node refreshes it instead of adding a duplicate.
#[tokio::test]
async fn two_instances_see_the_same_members_and_refresh_without_duplicates() {
    init_tracing();
    let Ok(url) = std::env::var("WEBX_REDIS_URL") else {
        return;
    };
    let suffix = unique_suffix();
    let prefix = format!("webx:test:presence:shared:{suffix}");
    let node_a = connection(&url);
    let node_b = connection(&url);
    let presence_a = store(&node_a, &prefix, 10);
    let presence_b = store(&node_b, &prefix, 10);
    let scope = PresenceScope::user(format!("u-{suffix}"));
    let user_id = format!("u-{suffix}");

    presence_a
        .join(
            &scope,
            PresenceMember::new(&user_id, "c1")
                .with_node_id("node-a")
                .with_joined_at_unix_secs(1_000),
            Duration::from_secs(60),
        )
        .await
        .expect("join c1");
    presence_a
        .join(
            &scope,
            PresenceMember::new(&user_id, "c2")
                .with_node_id("node-a")
                .with_joined_at_unix_secs(1_000),
            Duration::from_secs(60),
        )
        .await
        .expect("join c2");
    presence_b
        .join(
            &scope,
            PresenceMember::new(&user_id, "c3")
                .with_node_id("node-b")
                .with_joined_at_unix_secs(1_000),
            Duration::from_secs(60),
        )
        .await
        .expect("join c3");

    // Same identity from another node (a restart or a duplicate event): refresh,
    // do not add.
    assert_eq!(
        presence_b
            .join(
                &scope,
                PresenceMember::new(&user_id, "c1")
                    .with_node_id("node-a")
                    .with_joined_at_unix_secs(2_000),
                Duration::from_secs(60),
            )
            .await
            .expect("re-join c1"),
        3,
        "a known identity must not consume a new slot"
    );

    let members_a = presence_a.members(&scope).await.expect("members a");
    let members_b = presence_b.members(&scope).await.expect("members b");
    assert_eq!(
        members_a, members_b,
        "both instances read the same live set"
    );
    assert_eq!(
        members_a
            .iter()
            .map(|member| member.connection_id.as_str())
            .collect::<Vec<_>>(),
        vec!["c1", "c2", "c3"],
        "members are sorted by connection id"
    );
    assert_eq!(
        members_a
            .iter()
            .find(|member| member.connection_id == "c1")
            .map(|member| member.joined_at_unix_secs),
        Some(2_000),
        "a duplicate join refreshes the stored member"
    );
    assert_eq!(presence_a.count(&scope).await.expect("count"), 3);
    assert_eq!(raw_zcard(&node_a, &prefix, &scope).await, 3);
    assert_eq!(raw_hlen(&node_a, &prefix, &scope).await, 3);

    cleanup(&node_a, &prefix, &scope).await;
}

/// 2.9.2 producers wrote the whole member JSON as the ZSET member: those
/// entries stay readable and are purged by score once expired.
#[tokio::test]
async fn legacy_json_members_are_still_readable() {
    init_tracing();
    let Ok(url) = std::env::var("WEBX_REDIS_URL") else {
        return;
    };
    let suffix = unique_suffix();
    let prefix = format!("webx:test:presence:legacy:{suffix}");
    let node = connection(&url);
    let presence = store(&node, &prefix, 10);
    let scope = PresenceScope::user(format!("u-{suffix}"));
    let user_id = format!("u-{suffix}");
    let now = now_unix();

    let live_legacy = format!(
        r#"{{"user_id":"{user_id}","connection_id":"legacy-live","node_id":"node-a","joined_at_unix_secs":500}}"#
    );
    let expired_legacy = format!(
        r#"{{"user_id":"{user_id}","connection_id":"legacy-expired","node_id":"node-a","joined_at_unix_secs":400}}"#
    );
    let mut raw = node.manager().await.expect("raw redis connection");
    let _: i64 = redis::cmd("ZADD")
        .arg(zset_key(&prefix, &scope))
        .arg(now + 60)
        .arg(&live_legacy)
        .arg(now.saturating_sub(10))
        .arg(&expired_legacy)
        .arg(now.saturating_sub(20))
        .arg("not a member")
        .query_async(&mut raw)
        .await
        .expect("seed legacy members");

    // The expired/malformed entries are cleaned; the live JSON member decodes
    // through the 2.9.2 fallback with its original join stamp.
    let members = presence.members(&scope).await.expect("members");
    assert_eq!(members.len(), 1, "only the live legacy member is visible");
    assert_eq!(members[0].connection_id, "legacy-live");
    assert_eq!(members[0].joined_at_unix_secs, 500);
    assert_eq!(presence.count(&scope).await.expect("count"), 1);
    assert_eq!(
        raw_zcard(&node, &prefix, &scope).await,
        1,
        "expired and malformed legacy members are purged"
    );
    assert_eq!(
        raw_hlen(&node, &prefix, &scope).await,
        0,
        "purging a legacy JSON member must not create HASH state"
    );

    cleanup(&node, &prefix, &scope).await;
}

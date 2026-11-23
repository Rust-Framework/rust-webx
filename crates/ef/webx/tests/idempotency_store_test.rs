//! End-to-end idempotency store test over in-memory SQLite: the three-state
//! protocol (`Started` → `InProgress` → `Replay`), TTL reclamation, release,
//! expired-row cleanup and binary-body round-trips.

use std::sync::Arc;
use std::time::Duration;

use rust_ef::db_context::{DbContext, DbContextOptions, DbContextOptionsBuilder};
use rust_ef::prelude::*;
use rust_ef_sqlite::DbContextOptionsBuilderExt as _;
use webx_core::hosted::PeriodicBackgroundService;
use webx_core::idempotency::{IIdempotencyStore, IdempotencyBegin, IdempotentResponse};
use webx_ef::idempotency::{
    key_hash, register_idempotency_model, EfIdempotencyStore, IdempotencyHousekeeping,
    IdempotencyRecord, STATE_COMPLETED, STATE_IN_PROGRESS,
};

const TTL: Duration = Duration::from_secs(60);

async fn in_memory_options() -> Arc<DbContextOptions> {
    let mut builder = DbContextOptionsBuilder::new();
    builder.use_sqlite_in_memory();
    Arc::new(builder.build())
}

/// Register the table entity and create the schema (tests only; production uses
/// the official rust-ef migration tooling).
async fn prepare(options: &Arc<DbContextOptions>) {
    let mut ctx = DbContext::from_options(options).expect("context");
    register_idempotency_model(&mut ctx);
    ctx.bootstrap().await.expect("bootstrap schema");
}

fn store(options: &Arc<DbContextOptions>) -> EfIdempotencyStore {
    EfIdempotencyStore::new(Arc::clone(options))
}

/// Read every row through a context that never called
/// `register_idempotency_model` — query paths resolve the entity lazily.
async fn rows(options: &Arc<DbContextOptions>) -> Vec<IdempotencyRecord> {
    let mut ctx = DbContext::from_options(options).expect("context");
    linq!(ctx.set::<IdempotencyRecord>();)
        .to_list()
        .await
        .expect("rows")
}

async fn row(options: &Arc<DbContextOptions>, key: &str) -> Option<IdempotencyRecord> {
    let digest = key_hash(key);
    rows(options)
        .await
        .into_iter()
        .find(|record| record.key_hash == digest)
}

async fn seed_rows(options: &Arc<DbContextOptions>, records: Vec<IdempotencyRecord>) {
    let mut ctx = DbContext::from_options(options).expect("context");
    for record in records {
        ctx.add(record);
    }
    ctx.save_changes().await.expect("seed rows");
}

/// Move a key's lease into the past (simulates the TTL running out).
async fn expire_row(options: &Arc<DbContextOptions>, key: &str) {
    let mut ctx = DbContext::from_options(options).expect("context");
    let target = key_hash(key);
    let affected = linq!(
        ctx.set::<IdempotencyRecord>(),
        |record: IdempotencyRecord| record.key_hash == target
    )
    .execute_update()
    .set_column(IdempotencyRecord::COLUMN_EXPIRES_AT, 0i64)
    .execute()
    .await
    .expect("expire row");
    assert_eq!(affected, 1, "the seeded row must exist");
}

fn response(status: u16, content_type: Option<&str>, body: &[u8]) -> IdempotentResponse {
    IdempotentResponse::new(
        status,
        content_type.map(str::to_string),
        None,
        body.to_vec(),
    )
}

fn seed_record(key: &str, state: &str, created_at: i64, expires_at: i64) -> IdempotencyRecord {
    IdempotencyRecord {
        key_hash: key_hash(key),
        key: key.to_string(),
        state: state.to_string(),
        status: None,
        content_type: None,
        content_encoding: None,
        attempt: None,
        headers: None,
        body: None,
        created_at,
        expires_at,
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after the Unix epoch")
        .as_secs() as i64
}

#[tokio::test]
async fn started_then_completed_replays_the_stored_response() {
    let options = in_memory_options().await;
    prepare(&options).await;
    let store = store(&options);

    assert_eq!(
        store.begin("order-1", TTL).await.expect("begin"),
        IdempotencyBegin::Started
    );

    store
        .complete(
            "order-1",
            response(201, Some("application/json"), br#"{"id":1}"#),
            TTL,
        )
        .await
        .expect("complete");

    match store.begin("order-1", TTL).await.expect("replay") {
        IdempotencyBegin::Replay(stored) => {
            assert_eq!(stored.status, 201);
            assert_eq!(stored.content_type.as_deref(), Some("application/json"));
            assert!(stored.content_encoding.is_none());
            assert_eq!(stored.body, br#"{"id":1}"#.to_vec());
        }
        other => panic!("expected a replay, got {other:?}"),
    }
}

#[tokio::test]
async fn a_second_begin_while_in_progress_reports_in_progress() {
    let options = in_memory_options().await;
    prepare(&options).await;
    let store = store(&options);

    assert_eq!(
        store.begin("order-2", TTL).await.expect("first begin"),
        IdempotencyBegin::Started
    );
    assert_eq!(
        store.begin("order-2", TTL).await.expect("second begin"),
        IdempotencyBegin::InProgress
    );
}

#[tokio::test]
async fn an_expired_key_is_reclaimed_and_the_old_response_is_dropped() {
    let options = in_memory_options().await;
    prepare(&options).await;
    let store = store(&options);

    assert_eq!(
        store.begin("order-3", TTL).await.expect("begin"),
        IdempotencyBegin::Started
    );
    store
        .complete("order-3", response(201, None, b"old"), TTL)
        .await
        .expect("complete old");
    assert!(matches!(
        store.begin("order-3", TTL).await.expect("replay old"),
        IdempotencyBegin::Replay(_)
    ));

    expire_row(&options, "order-3").await;

    // The expired completed row is claimed atomically and its old response is
    // cleared, so the retry's handler runs.
    assert_eq!(
        store.begin("order-3", TTL).await.expect("reclaim"),
        IdempotencyBegin::Started
    );
    let reclaimed = row(&options, "order-3").await.expect("row");
    assert_eq!(reclaimed.state, STATE_IN_PROGRESS);
    assert!(reclaimed.status.is_none());
    assert!(reclaimed.content_type.is_none());
    assert!(reclaimed.body.is_none());

    store
        .complete("order-3", response(202, None, b"new"), TTL)
        .await
        .expect("complete new");
    match store.begin("order-3", TTL).await.expect("replay new") {
        IdempotencyBegin::Replay(stored) => {
            assert_eq!(stored.status, 202);
            assert_eq!(stored.body, b"new".to_vec());
        }
        other => panic!("expected a replay, got {other:?}"),
    }
}

#[tokio::test]
async fn an_expired_in_progress_reservation_is_reclaimable() {
    let options = in_memory_options().await;
    prepare(&options).await;
    let store = store(&options);

    assert_eq!(
        store.begin("order-4", TTL).await.expect("begin"),
        IdempotencyBegin::Started
    );
    expire_row(&options, "order-4").await;

    // A killed instance's lease expires: the next retry owns the key again.
    assert_eq!(
        store.begin("order-4", TTL).await.expect("reclaim"),
        IdempotencyBegin::Started
    );
}

#[tokio::test]
async fn release_frees_the_key_but_keeps_a_completed_response() {
    let options = in_memory_options().await;
    prepare(&options).await;
    let store = store(&options);

    assert_eq!(
        store.begin("order-5", TTL).await.expect("begin"),
        IdempotencyBegin::Started
    );
    store.release("order-5").await.expect("release");
    assert_eq!(
        store.begin("order-5", TTL).await.expect("begin again"),
        IdempotencyBegin::Started
    );

    store
        .complete("order-5", response(200, None, b"ok"), TTL)
        .await
        .expect("complete");
    store
        .release("order-5")
        .await
        .expect("releasing a completed key is a no-op");
    assert!(matches!(
        store.begin("order-5", TTL).await.expect("still stored"),
        IdempotencyBegin::Replay(_)
    ));
}

#[tokio::test]
async fn complete_is_silent_when_the_reservation_is_gone_or_completed() {
    let options = in_memory_options().await;
    prepare(&options).await;
    let store = store(&options);

    // No row at all: the middleware must not fail the request.
    store
        .complete("missing", response(200, None, b"x"), TTL)
        .await
        .expect("completing a missing key is a no-op");

    assert_eq!(
        store.begin("order-6", TTL).await.expect("begin"),
        IdempotencyBegin::Started
    );
    store
        .complete("order-6", response(201, None, b"first"), TTL)
        .await
        .expect("first completion");
    // A late second completion must not overwrite the stored response.
    store
        .complete("order-6", response(202, None, b"second"), TTL)
        .await
        .expect("second completion is a no-op");
    match store.begin("order-6", TTL).await.expect("replay") {
        IdempotencyBegin::Replay(stored) => {
            assert_eq!(stored.status, 201);
            assert_eq!(stored.body, b"first".to_vec());
        }
        other => panic!("expected the first response, got {other:?}"),
    }
}

#[tokio::test]
async fn a_binary_body_round_trips_byte_for_byte() {
    let options = in_memory_options().await;
    prepare(&options).await;
    let store = store(&options);
    let body: Vec<u8> = vec![0x00, 0xff, 0x80, 0x01, b'{', b'}', 0x0a, 0x00, 0xc3, 0x28];
    let mut stored = response(200, Some("application/octet-stream"), &body);
    stored.content_encoding = Some("gzip".to_string());

    assert_eq!(
        store.begin("blob-1", TTL).await.expect("begin"),
        IdempotencyBegin::Started
    );
    store
        .complete("blob-1", stored.clone(), TTL)
        .await
        .expect("complete");

    match store.begin("blob-1", TTL).await.expect("replay") {
        IdempotencyBegin::Replay(replayed) => {
            assert_eq!(replayed.status, 200);
            assert_eq!(replayed.content_encoding.as_deref(), Some("gzip"));
            assert_eq!(replayed.body, body);
        }
        other => panic!("expected a replay, got {other:?}"),
    }
}

#[tokio::test]
async fn purge_expired_only_deletes_rows_past_the_grace_period() {
    let options = in_memory_options().await;
    prepare(&options).await;
    let now = now_secs();
    seed_rows(
        &options,
        vec![
            // Expired more than a day ago: purgeable.
            seed_record("purged", STATE_COMPLETED, now - 200_000, now - 90_000),
            // Expired one minute ago: still inside the grace period.
            seed_record("recent", STATE_IN_PROGRESS, now - 200_000, now - 60),
            // Live reservation: never touched by cleanup.
            seed_record("live", STATE_IN_PROGRESS, now, now + 3_600),
        ],
    )
    .await;

    let purged = store(&options)
        .purge_expired(now, 24 * 3_600)
        .await
        .expect("purge");
    assert_eq!(purged, 1);

    let mut keys: Vec<String> = rows(&options).await.into_iter().map(|r| r.key).collect();
    keys.sort();
    assert_eq!(keys, ["live", "recent"]);
}

#[tokio::test]
async fn housekeeping_tick_purges_rows_past_its_grace_period() {
    let options = in_memory_options().await;
    prepare(&options).await;
    let now = now_secs();
    seed_rows(
        &options,
        vec![
            // Expired two hours ago: removed by a one-hour grace.
            seed_record("old", STATE_COMPLETED, now - 100_000, now - 7_200),
            // Expired one minute ago: kept.
            seed_record("recent", STATE_IN_PROGRESS, now - 100_000, now - 60),
            seed_record("live", STATE_IN_PROGRESS, now, now + 3_600),
        ],
    )
    .await;

    let housekeeping = IdempotencyHousekeeping::new(Arc::new(store(&options)))
        .with_grace(Duration::from_secs(3_600));
    assert_eq!(housekeeping.interval(), Duration::from_secs(3_600));
    housekeeping.tick().await.expect("tick");

    let mut keys: Vec<String> = rows(&options).await.into_iter().map(|r| r.key).collect();
    keys.sort();
    assert_eq!(keys, ["live", "recent"]);
}

#[tokio::test]
async fn a_failed_insert_leaves_the_context_usable_for_the_collision_fallback() {
    // `begin` recovers from the "another instance inserted the same key" race by
    // clearing tracking and re-reading on the same context. Verify rust-ef leaves
    // that context usable after the failed INSERT (the statement is rolled back).
    let options = in_memory_options().await;
    prepare(&options).await;
    let now = now_secs();

    let mut ctx = DbContext::from_options(&options).expect("context");
    ctx.add(seed_record("dup", STATE_IN_PROGRESS, now, now + 60));
    ctx.save_changes().await.expect("first insert");

    ctx.add(seed_record("dup", STATE_IN_PROGRESS, now, now + 60));
    let error = ctx.save_changes().await.expect_err("duplicate key");
    assert!(
        matches!(error, EFError::UniqueViolation { .. }),
        "expected a unique violation, got {error}"
    );

    ctx.clear_tracking();
    let target = "dup".to_string();
    let found = linq!(
        ctx.set::<IdempotencyRecord>(),
        |record: IdempotencyRecord| record.key == target
    )
    .first_or_default()
    .await
    .expect("query after a failed save");
    assert!(found.is_some());
}

#[test]
fn the_store_is_usable_as_a_shared_trait_object() {
    // `HostBuilder::use_idempotency` takes `Arc<dyn IIdempotencyStore>`, which
    // requires the concrete store to be `Send + Sync + 'static`.
    fn assert_shared_store<T: IIdempotencyStore + Send + Sync + 'static>() {}
    assert_shared_store::<EfIdempotencyStore>();
}

#[tokio::test]
async fn a_long_protocol_key_is_stored_by_digest_and_never_truncated() {
    // The middleware's protocol key embeds the principal, method, path and the
    // client `Idempotency-Key` (up to 255 chars), so it can exceed any fixed
    // VARCHAR width. The indexed primary key is its SHA-256 digest (64 hex
    // chars); the full key is kept for operators.
    let options = in_memory_options().await;
    prepare(&options).await;
    let store = store(&options);

    let key = format!(
        "idem:user:{}:POST:/api/v1/reports/very/deep/path/{}/{}",
        "principal",
        "segment/".repeat(40),
        "k".repeat(255)
    );
    assert!(
        key.len() > 244,
        "the regression key exceeds a 244-byte column"
    );

    assert_eq!(
        store.begin(&key, TTL).await.expect("begin"),
        IdempotencyBegin::Started
    );
    store
        .complete(&key, response(200, None, b"long"), TTL)
        .await
        .expect("complete");

    let stored = row(&options, &key).await.expect("row");
    assert_eq!(stored.key, key);
    assert_eq!(stored.key_hash, key_hash(&key));
    assert_eq!(
        stored.key_hash.len(),
        64,
        "digest primary key is fixed size"
    );

    match store.begin(&key, TTL).await.expect("replay") {
        IdempotencyBegin::Replay(replayed) => assert_eq!(replayed.body, b"long".to_vec()),
        other => panic!("expected a replay, got {other:?}"),
    }
}

#[tokio::test]
async fn a_late_completion_for_an_expired_reservation_is_discarded() {
    let options = in_memory_options().await;
    prepare(&options).await;
    let store = store(&options);

    assert_eq!(
        store.begin("late-1", TTL).await.expect("begin"),
        IdempotencyBegin::Started
    );
    expire_row(&options, "late-1").await;

    // The handler finished after its lease expired: the completion must not
    // resurrect the reservation (the row stays invisible to retries).
    store
        .complete("late-1", response(201, None, b"late"), TTL)
        .await
        .expect("late completion is silent");
    let after = row(&options, "late-1").await.expect("row");
    assert_eq!(
        after.state, STATE_IN_PROGRESS,
        "an expired reservation must not become completed"
    );
    assert!(after.body.is_none(), "the late response must not be stored");

    // The next retry reclaims the key and runs the handler again.
    assert_eq!(
        store.begin("late-1", TTL).await.expect("reclaim"),
        IdempotencyBegin::Started
    );
}

#[tokio::test]
async fn a_late_release_for_an_expired_reservation_keeps_the_row() {
    let options = in_memory_options().await;
    prepare(&options).await;
    let store = store(&options);

    assert_eq!(
        store.begin("late-2", TTL).await.expect("begin"),
        IdempotencyBegin::Started
    );
    expire_row(&options, "late-2").await;
    store
        .release("late-2")
        .await
        .expect("late release is silent");

    assert!(
        row(&options, "late-2").await.is_some(),
        "releasing an expired reservation must not delete it"
    );
    assert_eq!(
        store.begin("late-2", TTL).await.expect("reclaim"),
        IdempotencyBegin::Started
    );
}

#[tokio::test]
async fn an_oversized_body_releases_the_reservation_instead_of_failing() {
    let options = in_memory_options().await;
    prepare(&options).await;
    // MySQL BLOB cannot hold more than ~60 KiB; simulate a tiny column budget.
    let store = store(&options).with_max_body_bytes(8);

    assert_eq!(
        store.begin("big-body", TTL).await.expect("begin"),
        IdempotencyBegin::Started
    );
    store
        .complete("big-body", response(200, None, b"0123456789"), TTL)
        .await
        .expect("an oversized body must not fail the request");

    assert!(
        row(&options, "big-body").await.is_none(),
        "the reservation must be released instead of staying in_progress"
    );
    assert_eq!(
        store.begin("big-body", TTL).await.expect("retry"),
        IdempotencyBegin::Started,
        "the client can retry immediately"
    );
}

#[tokio::test]
async fn a_long_protocol_key_is_truncated_only_in_the_debug_column() {
    let options = in_memory_options().await;
    prepare(&options).await;
    let store = store(&options);

    let key = format!("idem:user:u1:POST:/api/x:{}", "k".repeat(2000));
    assert_eq!(
        store.begin(&key, TTL).await.expect("begin"),
        IdempotencyBegin::Started
    );
    store
        .complete(&key, response(200, None, b"ok"), TTL)
        .await
        .expect("complete");

    let stored = row(&options, &key).await.expect("row");
    assert_eq!(stored.key.chars().count(), 1024, "debug key is truncated");
    assert_eq!(
        stored.key_hash,
        key_hash(&key),
        "the digest is the identity"
    );
    assert!(matches!(
        store.begin(&key, TTL).await.expect("replay"),
        IdempotencyBegin::Replay(_)
    ));
}

#[tokio::test]
async fn the_reservation_lease_is_shorter_than_the_replay_retention() {
    let options = in_memory_options().await;
    prepare(&options).await;
    let store = store(&options);
    let retention = Duration::from_secs(3_600);

    let (begin, _lease) = store
        .begin_lease("lease-1", retention, Duration::from_secs(2))
        .await
        .expect("begin");
    assert_eq!(begin, IdempotencyBegin::Started);
    let reserved = row(&options, "lease-1").await.expect("row");
    assert!(
        reserved.expires_at - now_secs() <= 3,
        "begin_lease must use the explicit lease, not the 1h retention TTL: {}",
        reserved.expires_at
    );

    store
        .complete_lease("lease-1", None, response(200, None, b"ok"), retention)
        .await
        .expect("complete");
    let completed = row(&options, "lease-1").await.expect("row");
    assert!(
        completed.expires_at - now_secs() >= 3_000,
        "complete_lease must use the retention TTL: {}",
        completed.expires_at
    );
}

/// The store honours the lease it is given, even when it is longer than the
/// retention: there is no hidden `min(lease, ttl)` correction inside the store
/// (the middleware is the component that caps the lease).
#[tokio::test]
async fn the_store_honours_the_explicit_lease_without_a_hidden_correction() {
    let options = in_memory_options().await;
    prepare(&options).await;
    let store = store(&options);
    let retention = Duration::from_secs(10);
    let lease = Duration::from_secs(3_600);

    let (begin, _lease) = store
        .begin_lease("lease-long", retention, lease)
        .await
        .expect("begin");
    assert_eq!(begin, IdempotencyBegin::Started);
    let reserved = row(&options, "lease-long").await.expect("row");
    assert!(
        reserved.expires_at - now_secs() >= 3_000,
        "the reservation must live for the passed lease, not be clamped to the \
         10s retention: {}",
        reserved.expires_at
    );

    store
        .complete_lease("lease-long", None, response(200, None, b"ok"), retention)
        .await
        .expect("complete");
    let completed = row(&options, "lease-long").await.expect("row");
    assert!(
        completed.expires_at - now_secs() <= 11,
        "a completed response uses the retention: {}",
        completed.expires_at
    );
}

/// The legacy token-less `begin` keeps its single-duration contract: the
/// reservation (and the stored response) are bounded by the given TTL.
#[tokio::test]
async fn the_legacy_begin_uses_the_ttl_for_the_reservation() {
    let options = in_memory_options().await;
    prepare(&options).await;
    let store = store(&options);
    let ttl = Duration::from_secs(120);

    assert_eq!(
        store.begin("legacy-begin", ttl).await.expect("begin"),
        IdempotencyBegin::Started
    );
    let reserved = row(&options, "legacy-begin").await.expect("row");
    let remaining = reserved.expires_at - now_secs();
    assert!(
        (115..=120).contains(&remaining),
        "legacy begin reserves for the TTL, got {remaining}s"
    );

    store
        .complete("legacy-begin", response(200, None, b"ok"), ttl)
        .await
        .expect("complete");
    let completed = row(&options, "legacy-begin").await.expect("row");
    assert!((115..=120).contains(&(completed.expires_at - now_secs())));
}

#[tokio::test]
async fn a_reclaimed_lease_fences_out_the_stale_attempt() {
    let options = in_memory_options().await;
    prepare(&options).await;
    let store = store(&options);

    let (begin, lease) = store.begin_lease("fence-1", TTL, TTL).await.expect("begin");
    assert_eq!(begin, IdempotencyBegin::Started);
    let old_lease = lease.expect("a fencing token");
    let reserved = row(&options, "fence-1").await.expect("row");
    assert_eq!(reserved.attempt.as_deref(), Some(old_lease.token.as_str()));

    // The handler outlived its lease: the row expires and a retry claims it
    // with a fresh token.
    expire_row(&options, "fence-1").await;
    let (begin, lease) = store
        .begin_lease("fence-1", TTL, TTL)
        .await
        .expect("reclaim");
    assert_eq!(begin, IdempotencyBegin::Started);
    let new_lease = lease.expect("a fencing token");
    assert_ne!(old_lease.token, new_lease.token);
    let reclaimed = row(&options, "fence-1").await.expect("row");
    assert_eq!(reclaimed.state, STATE_IN_PROGRESS);
    assert_eq!(reclaimed.attempt.as_deref(), Some(new_lease.token.as_str()));

    // The stale attempt's late completion must not overwrite the live lease...
    store
        .complete_lease(
            "fence-1",
            Some(&old_lease),
            response(201, None, b"stale"),
            TTL,
        )
        .await
        .expect("stale completion is silent");
    let after = row(&options, "fence-1").await.expect("row");
    assert_eq!(after.state, STATE_IN_PROGRESS);
    assert!(after.body.is_none(), "the stale body must not be stored");

    // ...and its late release must not delete the new owner's row.
    store
        .release_lease("fence-1", Some(&old_lease))
        .await
        .expect("stale release is silent");
    assert!(
        row(&options, "fence-1").await.is_some(),
        "a stale release must not delete the new lease"
    );

    // The current attempt still completes normally and clears its token.
    store
        .complete_lease(
            "fence-1",
            Some(&new_lease),
            response(202, None, b"fresh"),
            TTL,
        )
        .await
        .expect("complete");
    let completed = row(&options, "fence-1").await.expect("row");
    assert_eq!(completed.state, STATE_COMPLETED);
    assert!(
        completed.attempt.is_none(),
        "the fencing token is cleared when the response is stored"
    );
    match store.begin("fence-1", TTL).await.expect("replay") {
        IdempotencyBegin::Replay(stored) => {
            assert_eq!(stored.status, 202);
            assert_eq!(stored.body, b"fresh".to_vec());
        }
        other => panic!("expected a replay, got {other:?}"),
    }
}

#[tokio::test]
async fn replayable_headers_round_trip_in_order() {
    let options = in_memory_options().await;
    prepare(&options).await;
    let store = store(&options);
    let headers = vec![
        ("location".to_string(), "/api/orders/7".to_string()),
        ("etag".to_string(), "\"v1\"".to_string()),
        (
            "last-modified".to_string(),
            "Wed, 21 Oct 2015 07:28:00 GMT".to_string(),
        ),
        (
            "cache-control".to_string(),
            "private, max-age=60".to_string(),
        ),
    ];
    let mut stored = response(201, Some("application/json"), b"{}");
    stored.headers = headers.clone();

    let (begin, lease) = store
        .begin_lease("headers-1", TTL, TTL)
        .await
        .expect("begin");
    assert_eq!(begin, IdempotencyBegin::Started);
    store
        .complete_lease("headers-1", lease.as_ref(), stored, TTL)
        .await
        .expect("complete");

    let stored_row = row(&options, "headers-1").await.expect("row");
    assert!(
        stored_row
            .headers
            .as_deref()
            .is_some_and(|json| json.contains("\"location\"")),
        "the replayable headers are persisted as JSON: {:?}",
        stored_row.headers
    );

    match store.begin("headers-1", TTL).await.expect("replay") {
        IdempotencyBegin::Replay(replayed) => assert_eq!(replayed.headers, headers),
        other => panic!("expected a replay, got {other:?}"),
    }
}

#[tokio::test]
async fn only_whitelisted_headers_survive_a_replay() {
    let options = in_memory_options().await;
    prepare(&options).await;
    let store = store(&options);
    let now = now_secs();
    let mut record = seed_record("filtered-headers", STATE_COMPLETED, now - 10, now + 3_600);
    record.status = Some(201);
    record.body = Some(b"ok".to_vec());
    // A hand-edited row (or future store version) may contain anything; only the
    // replay whitelist may reach a replayed response. Names are canonicalized.
    record.headers = Some(
        r#"[["set-cookie","session=secret"],["ETag","\"v2\""],["location","/api/x"]]"#.to_string(),
    );
    seed_rows(&options, vec![record]).await;

    match store.begin("filtered-headers", TTL).await.expect("replay") {
        IdempotencyBegin::Replay(replayed) => assert_eq!(
            replayed.headers,
            vec![
                ("etag".to_string(), "\"v2\"".to_string()),
                ("location".to_string(), "/api/x".to_string()),
            ],
            "Set-Cookie must never be replayed and names are lower-cased"
        ),
        other => panic!("expected a replay, got {other:?}"),
    }
}

#[tokio::test]
async fn a_reclaimed_row_written_before_2_8_gets_a_fresh_attempt_token() {
    let options = in_memory_options().await;
    prepare(&options).await;
    let store = store(&options);
    let now = now_secs();
    // A pre-2.8 expired reservation: NULL attempt, NULL headers.
    seed_rows(
        &options,
        vec![seed_record("legacy", STATE_IN_PROGRESS, now - 300, now - 1)],
    )
    .await;

    let (begin, lease) = store
        .begin_lease("legacy", TTL, TTL)
        .await
        .expect("claim legacy row");
    assert_eq!(begin, IdempotencyBegin::Started);
    let lease = lease.expect("a fencing token");
    let claimed = row(&options, "legacy").await.expect("row");
    assert_eq!(claimed.attempt.as_deref(), Some(lease.token.as_str()));
    assert!(claimed.headers.is_none());
}

#[tokio::test]
async fn a_pre_2_8_completed_row_replays_an_empty_response() {
    let options = in_memory_options().await;
    prepare(&options).await;
    let store = store(&options);
    let now = now_secs();
    seed_rows(
        &options,
        vec![seed_record(
            "legacy-done",
            STATE_COMPLETED,
            now - 10,
            now + 3_600,
        )],
    )
    .await;

    match store.begin("legacy-done", TTL).await.expect("replay") {
        IdempotencyBegin::Replay(stored) => {
            assert_eq!(stored.status, 200, "defensive status fallback");
            assert!(stored.headers.is_empty());
            assert!(stored.body.is_empty());
        }
        other => panic!("expected a replay, got {other:?}"),
    }
}

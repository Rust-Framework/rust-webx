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
    rows(options)
        .await
        .into_iter()
        .find(|record| record.key == key)
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
    let target = key.to_string();
    let affected = linq!(
        ctx.set::<IdempotencyRecord>(),
        |record: IdempotencyRecord| record.key == target
    )
    .execute_update()
    .set_column(IdempotencyRecord::COLUMN_EXPIRES_AT, 0i64)
    .execute()
    .await
    .expect("expire row");
    assert_eq!(affected, 1, "the seeded row must exist");
}

fn response(status: u16, content_type: Option<&str>, body: &[u8]) -> IdempotentResponse {
    IdempotentResponse {
        status,
        content_type: content_type.map(str::to_string),
        content_encoding: None,
        body: body.to_vec(),
    }
}

fn seed_record(key: &str, state: &str, created_at: i64, expires_at: i64) -> IdempotencyRecord {
    IdempotencyRecord {
        key_hash: key_hash(key),
        key: key.to_string(),
        state: state.to_string(),
        status: None,
        content_type: None,
        content_encoding: None,
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

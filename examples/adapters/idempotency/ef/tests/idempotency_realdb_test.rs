//! Real-database (PostgreSQL / MySQL) tests for
//! [`EfIdempotencyStore`](rust_webx_idempotency_ef::EfIdempotencyStore).
//!
//! Ignored by default: they need a live server and a dedicated empty database,
//! and are opt-in per provider through an environment variable:
//!
//! ```text
//! RUST_EF_PG_URL=postgres://test:test@127.0.0.1:5432/rust_webx_test \
//! RUST_EF_MYSQL_URL=mysql://root:test@127.0.0.1:3306/rust_webx_test \
//!   cargo test -p rust-webx-idempotency-ef --features postgres,mysql --tests -- --ignored
//! ```
//!
//! CI runs them in the `realdb` job (postgres:16 + mysql:8 services). The table
//! is dropped and re-created with `ctx.bootstrap()` (tests only — production uses
//! the official rust-ef migration tooling), so a run is repeatable against a
//! shared database.
//!
//! Why a real database: SQLite exercises the protocol logic, but the provider
//! specifics only show up on a server — the unique-key INSERT race fallback, a
//! `TEXT`/`BLOB` `webx_idempotency` schema, Unix-seconds `expires_at` columns,
//! conditional `UPDATE`/`DELETE` affected-row counts and binary body bytes.

#![cfg(any(feature = "postgres", feature = "mysql"))]

use std::sync::Arc;
use std::time::Duration;

use rust_ef::db_context::{DbContext, DbContextOptions, DbContextOptionsBuilder};
use rust_ef::prelude::*;
use rust_webx_idempotency_ef::{
    key_hash, register_idempotency_model, EfIdempotencyStore, IdempotencyRecord, STATE_COMPLETED,
    STATE_IN_PROGRESS,
};
use webx_core::idempotency::{IIdempotencyStore, IdempotencyBegin, IdempotentResponse};

/// Replay retention used by the tests (`Idempotency.TtlSeconds`).
const TTL: Duration = Duration::from_secs(60);
/// Reservation lease used by the tests (`Idempotency.LeaseSeconds`).
const LEASE: Duration = Duration::from_secs(30);

/// Read the database URL from `name`; `None` (with a printed skip line) when it
/// is not configured, so `--ignored` can run both providers on one machine.
fn database_url(name: &str) -> Option<String> {
    match std::env::var(name) {
        Ok(url) if !url.trim().is_empty() => Some(url),
        _ => {
            eprintln!("skip: {name} is not set (real-database test)");
            None
        }
    }
}

/// Drop and re-create the `webx_idempotency` table for a repeatable run.
async fn prepare(options: &Arc<DbContextOptions>) {
    let mut ctx = DbContext::from_options(options).expect("context");
    register_idempotency_model(&mut ctx);
    ctx.ensure_deleted().await.expect("drop the store table");
    ctx.bootstrap().await.expect("create the store table");
}

/// Read one stored row by protocol key (the digest is the primary key).
async fn row(options: &Arc<DbContextOptions>, key: &str) -> Option<IdempotencyRecord> {
    let mut ctx = DbContext::from_options(options).expect("context");
    let target = key_hash(key);
    linq!(
        ctx.set::<IdempotencyRecord>(),
        |record: IdempotencyRecord| record.key_hash == target
    )
    .first_or_default()
    .await
    .expect("query the stored row")
}

/// Move a key's lease deadline into the past (simulates the lease running out).
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
    .expect("expire the reservation");
    assert_eq!(affected, 1, "the reservation row must exist");
}

fn response(status: u16, body: &[u8]) -> IdempotentResponse {
    IdempotentResponse::new(
        status,
        Some("application/json".to_string()),
        None,
        body.to_vec(),
    )
}

/// The provider-independent protocol walk-through; each provider wires its own
/// options and calls this once.
async fn exercise_store(options: Arc<DbContextOptions>) {
    prepare(&options).await;
    let store = EfIdempotencyStore::new(Arc::clone(&options));

    // 1. begin → Started; a concurrent/duplicate begin while in flight 409s.
    let (begin, lease) = store
        .begin_lease("realdb-1", TTL, LEASE)
        .await
        .expect("begin");
    assert_eq!(begin, IdempotencyBegin::Started);
    let lease = lease.expect("the store must issue a fencing token");
    assert_eq!(
        store.begin("realdb-1", TTL).await.expect("duplicate begin"),
        IdempotencyBegin::InProgress
    );

    // 2. complete → replay: status, content-type, body and replayable headers.
    let mut stored = response(201, br#"{"id":1}"#);
    stored.headers = vec![
        ("location".to_string(), "/api/orders/1".to_string()),
        ("etag".to_string(), "\"v1\"".to_string()),
    ];
    store
        .complete_lease("realdb-1", Some(&lease), stored, TTL)
        .await
        .expect("complete");
    let completed = row(&options, "realdb-1").await.expect("row");
    assert_eq!(completed.state, STATE_COMPLETED);
    let remaining = completed.expires_at - unix_now_secs();
    assert!(
        (TTL.as_secs() as i64 - 2..=TTL.as_secs() as i64).contains(&remaining),
        "a completed row replays for the retention TTL, got {remaining}s"
    );
    match store.begin("realdb-1", TTL).await.expect("replay") {
        IdempotencyBegin::Replay(replayed) => {
            assert_eq!(replayed.status, 201);
            assert_eq!(replayed.content_type.as_deref(), Some("application/json"));
            assert_eq!(replayed.body, br#"{"id":1}"#.to_vec());
            assert_eq!(
                replayed.headers,
                vec![
                    ("location".to_string(), "/api/orders/1".to_string()),
                    ("etag".to_string(), "\"v1\"".to_string()),
                ],
                "the replayable headers round-trip through the real database"
            );
        }
        other => panic!("expected a replay, got {other:?}"),
    }

    // 3. The unique-key INSERT race: two instances starting at once → exactly one
    //    `Started`, the loser resolves the row and reports `InProgress`.
    let (first, second) = tokio::join!(
        store.begin_lease("realdb-race", TTL, LEASE),
        store.begin_lease("realdb-race", TTL, LEASE),
    );
    let outcomes = [first.expect("first racer"), second.expect("second racer")];
    let started = outcomes
        .iter()
        .filter(|(begin, _)| matches!(begin, IdempotencyBegin::Started))
        .count();
    let in_progress = outcomes
        .iter()
        .filter(|(begin, _)| matches!(begin, IdempotencyBegin::InProgress))
        .count();
    assert_eq!(
        (started, in_progress),
        (1, 1),
        "exactly one instance must win the unique-key INSERT"
    );
    store
        .release("realdb-race")
        .await
        .expect("release the race winner");

    // 4. Lease expiry reclamation and fencing: a retry claims the expired row
    //    with a new token; the stale attempt's late complete/release is a no-op.
    let (begin, stale_lease) = store
        .begin_lease("realdb-fence", TTL, LEASE)
        .await
        .expect("begin");
    assert_eq!(begin, IdempotencyBegin::Started);
    let stale_lease = stale_lease.expect("token");
    expire_row(&options, "realdb-fence").await;

    let (begin, live_lease) = store
        .begin_lease("realdb-fence", TTL, LEASE)
        .await
        .expect("reclaim");
    assert_eq!(begin, IdempotencyBegin::Started);
    let live_lease = live_lease.expect("token");
    assert_ne!(stale_lease.token, live_lease.token);
    let reclaimed = row(&options, "realdb-fence").await.expect("row");
    assert_eq!(reclaimed.state, STATE_IN_PROGRESS);
    assert_eq!(
        reclaimed.attempt.as_deref(),
        Some(live_lease.token.as_str())
    );

    store
        .complete_lease(
            "realdb-fence",
            Some(&stale_lease),
            response(201, b"stale"),
            TTL,
        )
        .await
        .expect("a stale completion is silent");
    let after_stale = row(&options, "realdb-fence").await.expect("row");
    assert_eq!(after_stale.state, STATE_IN_PROGRESS);
    assert!(after_stale.body.is_none(), "the stale body is not stored");
    store
        .release_lease("realdb-fence", Some(&stale_lease))
        .await
        .expect("a stale release is silent");
    assert_eq!(
        store.begin("realdb-fence", TTL).await.expect("still live"),
        IdempotencyBegin::InProgress
    );

    // 5. The live attempt still completes normally.
    store
        .complete_lease(
            "realdb-fence",
            Some(&live_lease),
            response(202, b"live"),
            TTL,
        )
        .await
        .expect("complete the live attempt");
    match store.begin("realdb-fence", TTL).await.expect("replay") {
        IdempotencyBegin::Replay(replayed) => {
            assert_eq!(replayed.status, 202);
            assert_eq!(replayed.body, b"live".to_vec());
        }
        other => panic!("expected the live response, got {other:?}"),
    }

    // 6. A killed instance's reservation is reclaimable after the lease (the
    //    `expires_at <= now` conditional UPDATE is provider SQL).
    store
        .begin_lease("realdb-killed", TTL, LEASE)
        .await
        .expect("begin");
    expire_row(&options, "realdb-killed").await;
    assert_eq!(
        store
            .begin("realdb-killed", TTL)
            .await
            .expect("reclaim a killed reservation"),
        IdempotencyBegin::Started
    );
}

fn unix_now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after the Unix epoch")
        .as_secs() as i64
}

#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore = "requires a PostgreSQL server (RUST_EF_PG_URL); run with --ignored"]
async fn postgres_store_lifecycle() {
    use rust_ef_postgres::DbContextOptionsBuilderExt as _;

    let Some(url) = database_url("RUST_EF_PG_URL") else {
        return;
    };
    let mut builder = DbContextOptionsBuilder::new();
    builder.use_postgres_with(&url, |options| {
        options.insecure();
    });
    exercise_store(Arc::new(builder.build())).await;
}

#[cfg(feature = "mysql")]
#[tokio::test]
#[ignore = "requires a MySQL server (RUST_EF_MYSQL_URL); run with --ignored"]
async fn mysql_store_lifecycle() {
    use rust_ef_mysql::DbContextOptionsBuilderExt as _;

    let Some(url) = database_url("RUST_EF_MYSQL_URL") else {
        return;
    };
    let mut builder = DbContextOptionsBuilder::new();
    builder.use_mysql_with(&url, |options| {
        options.insecure();
    });
    exercise_store(Arc::new(builder.build())).await;
}

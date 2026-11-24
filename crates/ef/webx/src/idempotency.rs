//! Durable [`IIdempotencyStore`] over `rust-ef`.
//!
//! The framework's `InMemoryIdempotencyStore` only sees the keys of its own
//! process, so a retry that lands on another instance of a multi-instance
//! deployment executes the handler a second time. [`EfIdempotencyStore`] keeps
//! the reservation and the cached response in one database table
//! (`webx_idempotency`), so every instance agrees on the state:
//!
//! ```text
//! begin(key) ──► Started      INSERT (unique key) or CAS-claim an expired row
//!            ├─► InProgress   another request owns the key → 409 + Retry-After
//!            └─► Replay(rsp)  stored response → replay bytes, no handler
//! ```
//!
//! The protocol lives in [`webx_core::idempotency::IIdempotencyStore`]; this
//! module only replaces the storage. The middleware, the key format and the
//! three-state contract are unchanged.
//!
//! # Deployment semantics
//!
//! * **Multi-instance**: the primary key of `webx_idempotency` is the
//!   idempotency key, so two nodes racing on the same key cannot both start the
//!   handler: one INSERT wins and the other resolves the row (usually
//!   `InProgress`). No shared cache / Redis lock is required.
//! * **Leases and retention**: the middleware passes two durations —
//!   `retention` (how long a completed response replays, `Idempotency.TtlSeconds`)
//!   and `lease` (how long an in-flight reservation blocks retries,
//!   `Idempotency.LeaseSeconds`, capped at the retention). A reservation row
//!   expires after the lease, so a killed node's reservation becomes claimable
//!   after ~60s and the client's next retry claims it with a conditional
//!   `UPDATE ... WHERE key_hash = ? AND expires_at <= ?`; a completed row keeps
//!   the retention `expires_at`. The store uses the lease it is given — it does
//!   not impose its own correction. `complete`/`release` are conditional on
//!   `expires_at > now`, so a late completion for an already-expired reservation
//!   is discarded instead of resurrecting it.
//! * **Fencing tokens**: every claim writes a fresh `attempt` token into the row
//!   and returns it as an [`IdempotencyLease`] ([`IIdempotencyStore::begin_lease`]).
//!   `complete_lease`/`release_lease` add `attempt = ?` to the compare-and-set and
//!   clear the token when they win, so an attempt whose lease expired (and was
//!   reclaimed) cannot overwrite the new owner's response or delete its row —
//!   even while the new lease is live. Rows written before 2.8 have a `NULL`
//!   `attempt`; they are reclaimed by inserting a token on the next claim, and
//!   the token-less `complete`/`release` keep working for old callers.
//! * **Replayable headers**: the stored response keeps the whitelisted response
//!   headers (`Location`, `ETag`, `Last-Modified`, `Cache-Control`) as a JSON
//!   column, so a replay is header-identical to the original response. Header
//!   sets larger than 8 KiB are not stored (the body is still cached).
//! * **Cleanup**: rows stay readable until `expires_at`; a row is deleted only
//!   after it has been expired for the grace period. `IdempotencyHousekeeping`
//!   does that on a periodic tick (default: hourly, grace 24h) and
//!   [`EfIdempotencyStore::purge_expired`] deletes in bounded batches
//!   ([`PURGE_BATCH_SIZE`] rows per statement) so a large backlog never becomes
//!   one long write lock on the hot table.
//! * **Not a unique constraint**: a business invariant still needs a unique
//!   index. The idempotency key only makes "the same key at most once" hold;
//!   after `expires_at` the same key executes again.
//!
//! # Wiring
//!
//! The table schema is declared as an entity and materialized by the official
//! rust-ef migration tooling — never hand-written DDL. Register the entity where
//! the application model is registered, and hand the **same**
//! `Arc<DbContextOptions>` to the store and to the host's `DbContext`
//! registration (the provider/connection pool and metadata cache live in the
//! options and are `Arc`-shared):
//!
//! ```ignore
//! use std::sync::Arc;
//! use std::time::Duration;
//! use rust_ef::db_context::{DbContext, DbContextOptionsBuilder};
//! use webx::*;
//! use webx_ef::idempotency::{
//!     register_idempotency_model, EfIdempotencyStore, IdempotencyHousekeeping,
//! };
//!
//! // 1. One options instance for the whole process.
//! let mut builder = DbContextOptionsBuilder::new();
//! builder.use_postgres(&database_url);
//! let options = Arc::new(builder.build());
//! options.create_provider().expect("database provider"); // fail fast at startup
//!
//! // 2. Model registration + schema migration (or `ctx.bootstrap()` for a fresh DB).
//! let mut ctx = DbContext::from_options(&options)?;
//! ctx.set::<Order>(); // the application's own entities
//! register_idempotency_model(&mut ctx);
//! ctx.migrate("Migrations").await?;
//!
//! // 3. Store + host wiring.
//! let store = Arc::new(EfIdempotencyStore::new(Arc::clone(&options)));
//! let housekeeping = IdempotencyHousekeeping::new(Arc::clone(&store));
//!
//! Host::builder()
//!     .register({
//!         let options = Arc::clone(&options);
//!         move |svc| {
//!             svc.scoped::<DbContext>(move |_| {
//!                 Arc::new(DbContext::from_options(&options).expect("DbContext"))
//!             })
//!         }
//!     })
//!     .use_idempotency(Arc::clone(&store), Duration::from_secs(24 * 3600))
//!     .register(move |svc| {
//!         svc.add_background_service_with(move |_| {
//!             Arc::new(IdempotencyHousekeeping::new(Arc::clone(&store)))
//!         })
//!     })
//!     .build();
//! ```
//!
//! `use_idempotency` runs before the DI provider exists, which is why the store
//! is built from [`DbContextOptions`] rather than from a `ScopeFactory`: the
//! options are the one database dependency the application owns at startup.
//! As an alternative to `use_idempotency`, register the store as a DI instance
//! and set `Idempotency.Enabled = true`; the host uses the registered
//! `dyn IIdempotencyStore` when no explicit store is passed.
//!
//! # Schema changes
//!
//! 2.8 adds two **nullable** columns to `webx_idempotency`: `attempt`
//! (`VARCHAR(64)`, the fencing token) and `headers` (`TEXT`, JSON replayable
//! headers). Existing rows stay valid (`NULL` = written before the upgrade) and
//! are claimed normally; upgrade an existing database with the official rust-ef
//! migration tooling (snapshot diff → generated migration → `ctx.migrate`), never
//! with hand-written `ALTER TABLE`.
//!
//! # Race semantics
//!
//! * `begin`/`begin_lease`: a missing row is INSERTed with a fresh `attempt`
//!   token; a unique-key violation means another node inserted it first, so the
//!   tracking state is dropped and the existing row is resolved. A completed,
//!   unexpired row replays its stored response; an expired row is claimed with
//!   one conditional UPDATE (exactly one winner) that writes the new `attempt`
//!   token and clears the previous response; a live row is `InProgress`.
//! * `complete`/`complete_lease`: a conditional UPDATE (`WHERE state =
//!   'in_progress' AND expires_at > now` plus `attempt = ?` for the lease
//!   variant) stores the response and clears `attempt`. An affected-row count of
//!   `0` is not an error — the reservation expired, was reclaimed or was
//!   released, so a retry may run the handler again (a late completion must never
//!   resurrect a stale reservation or overwrite a newer attempt).
//! * `release`/`release_lease`: deletes the row only while it is still
//!   `in_progress` (and still owned by `attempt` for the lease variant), so
//!   releasing a completed key does not drop its cached response and a stale
//!   attempt cannot delete the new owner's row.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use rust_ef::db_context::{DbContext, DbContextOptions};
use rust_ef::error::EFError;
use rust_ef::prelude::*;
use sha2::{Digest, Sha256};
use webx_core::error::{Error, Result};
use webx_core::hosted::PeriodicBackgroundService;
use webx_core::idempotency::{
    IIdempotencyStore, IdempotencyBegin, IdempotencyLease, IdempotentResponse,
    REPLAYABLE_RESPONSE_HEADERS,
};

use crate::map_ef_error;

/// Row state: a request owns the key and is still running.
pub const STATE_IN_PROGRESS: &str = "in_progress";

/// Row state: the response is stored and replays until `expires_at`.
pub const STATE_COMPLETED: &str = "completed";

/// Default delay between housekeeping ticks (1 hour).
pub const DEFAULT_CLEANUP_INTERVAL: Duration = Duration::from_secs(3_600);

/// Default grace after `expires_at` before a row is deleted (24 hours).
///
/// Keeping expired rows for a while makes debugging a retry trivial; the
/// reservation itself already expired, so the grace does not affect the
/// protocol.
pub const DEFAULT_CLEANUP_GRACE: Duration = Duration::from_secs(24 * 3_600);

/// Rows deleted per statement by [`EfIdempotencyStore::purge_expired`].
///
/// Cleanup never sends one unbounded `DELETE`: a large backlog is removed in
/// bounded statements so a single tick cannot hold a long write lock on a table
/// that is also the hot path of every state-changing request. The batch is small
/// enough to stay under every provider's bind-parameter limit with the
/// `IN (...)` list used to target it (SQLite 999, PostgreSQL/MySQL 65,535).
pub const PURGE_BATCH_SIZE: usize = 500;

/// One stored idempotency key.
///
/// `key_hash` is the primary key (unique across all instances), so a duplicate
/// INSERT is a unique-constraint violation. `expires_at` is a Unix-seconds lease
/// deadline: after it the key can be claimed again, which is also what makes a
/// killed instance recoverable. `body` is the raw response body (compressed
/// bytes are stored as-is together with `content_encoding`). `attempt` is the
/// fencing token of the live attempt and `headers` the JSON-encoded replayable
/// response headers (see [`REPLAYABLE_RESPONSE_HEADERS`]).
#[derive(Debug, Clone, EntityType)]
#[table("webx_idempotency")]
pub struct IdempotencyRecord {
    /// SHA-256 (hex) of the full protocol key — the primary key.
    ///
    /// The middleware's protocol key embeds the principal, method, request path
    /// and the client's `Idempotency-Key`, so it is unbounded in practice. The
    /// unique/indexed column therefore stores a fixed-size digest
    /// ([`key_hash`]) while the full key is kept in [`Self::key`] for
    /// operators; hashing also keeps the primary-key index small.
    #[primary_key]
    #[max_length(64)]
    pub key_hash: String,
    /// The full protocol key (principal + method + path + header), kept for
    /// debugging; not indexed because the digest is the lookup path.
    #[required]
    pub key: String,
    /// [`STATE_IN_PROGRESS`] or [`STATE_COMPLETED`].
    #[required]
    #[max_length(16)]
    pub state: String,
    /// HTTP status of the stored response (`None` while in progress).
    pub status: Option<i32>,
    /// `content-type` of the stored response.
    pub content_type: Option<String>,
    /// `content-encoding` of the stored response (bytes are replayed as-is).
    pub content_encoding: Option<String>,
    /// Fencing token of the attempt that owns this reservation.
    ///
    /// `None` once the response is stored and on rows written before 2.8; a
    /// claim always writes a fresh token (`begin_lease` returns it to the
    /// middleware). `complete_lease`/`release_lease` add `attempt = ?` to their
    /// compare-and-set, so a stale attempt cannot overwrite or delete a newer
    /// owner's row.
    #[max_length(64)]
    pub attempt: Option<String>,
    /// Replayable response headers as a JSON array of `[name, value]` pairs.
    ///
    /// `None` when the response carried none or the row predates 2.8. Only the
    /// allowed names survive a replay; anything else is ignored.
    pub headers: Option<String>,
    /// Raw body of the stored response.
    pub body: Option<Vec<u8>>,
    /// Unix-seconds time this reservation was created (or last claimed).
    #[required]
    pub created_at: i64,
    /// Unix-seconds lease deadline; indexed because cleanup and reclaim scan it.
    #[required]
    #[index]
    pub expires_at: i64,
}

/// Register [`IdempotencyRecord`] in the application `DbContext` model.
///
/// Call it from the same place the application registers its own entities
/// (`ctx.set::<T>()`), before `bootstrap`/`migrate`; the `webx_idempotency`
/// table is then created and migrated by the official rust-ef tooling. Query
/// paths resolve the entity lazily, so only schema creation/upgrade needs this
/// call.
pub fn register_idempotency_model(ctx: &mut DbContext) {
    ctx.set::<IdempotencyRecord>();
}

/// Durable [`IIdempotencyStore`] backed by rust-ef.
///
/// Holds the application's [`DbContextOptions`] (the same instance the host's
/// `DbContext` registration uses) and opens a short-lived context per operation,
/// so a request never shares a context across `.await` points.
pub struct EfIdempotencyStore {
    options: Arc<DbContextOptions>,
    max_body_bytes: usize,
}

impl EfIdempotencyStore {
    /// Build a store over the application's database options.
    ///
    /// The options must be the ones the host resolves `DbContext` from, so both
    /// use the same provider/connection pool and metadata cache.
    pub fn new(options: Arc<DbContextOptions>) -> Self {
        let max_body_bytes = default_body_budget(&options);
        Self {
            options,
            max_body_bytes,
        }
    }

    /// Largest response body stored in the table (default: provider-dependent).
    ///
    /// MySQL maps `Vec<u8>` to `BLOB` (65,535 bytes), so the default budget is
    /// ~60 KiB there and effectively unlimited on PostgreSQL (`BYTEA`) / SQLite
    /// (`BLOB`). Larger bodies are not cached: the reservation is released so a
    /// retry re-executes instead of receiving 409 until the TTL expires. Keep
    /// the middleware's `MaxBodyBytes` at or below this value for the same
    /// semantics end-to-end.
    pub fn with_max_body_bytes(mut self, max_body_bytes: usize) -> Self {
        self.max_body_bytes = max_body_bytes.max(1);
        self
    }

    /// Delete rows that expired before `now - older_than` (Unix seconds).
    ///
    /// Rows are kept for `older_than` seconds after `expires_at` so an operator
    /// can still inspect a recently expired key. `older_than` is clamped to `0`,
    /// so `purge_expired(now, 0)` deletes every expired row. Returns the number
    /// of deleted rows.
    ///
    /// Deletion is batched ([`PURGE_BATCH_SIZE`] rows per statement): a large
    /// backlog becomes several bounded statements instead of one long `DELETE`
    /// over the whole table. Each batch repeats the expiry predicate inside the
    /// statement that deletes, so a row another instance reclaimed between the
    /// batch read and the delete keeps its live reservation.
    pub async fn purge_expired(&self, now: i64, older_than: i64) -> Result<u64> {
        let cutoff = now.saturating_sub(older_than.max(0));
        let mut ctx = self.context()?;
        let mut deleted = 0u64;
        loop {
            let batch = delete_expired_batch(&mut ctx, cutoff).await?;
            deleted = deleted.saturating_add(batch);
            // A short batch means the table has no more expired rows to remove.
            if batch < PURGE_BATCH_SIZE as u64 {
                break;
            }
        }
        Ok(deleted)
    }

    /// Open a context for one operation.
    ///
    /// The options are built once at startup, so a failure here is a
    /// configuration problem (missing provider / invalid connection string),
    /// not a per-request database error; per-request failures surface from the
    /// query itself as mapped framework errors.
    fn context(&self) -> Result<DbContext> {
        DbContext::from_options(&self.options).map_err(|error| {
            Error::internal(format!(
                "the idempotency store could not create a DbContext from the configured \
                 options: {error}"
            ))
        })
    }
}

/// Delete one bounded batch of expired rows; returns how many rows were removed.
///
/// The candidate primary keys are read first (at most [`PURGE_BATCH_SIZE`] of
/// them, so a backlog never materialises in memory) and the `DELETE` repeats the
/// expiry predicate. That repetition is the correctness boundary of batching: a
/// row that another instance reclaimed between the read and the delete now has a
/// future `expires_at`, so it no longer matches and its live reservation
/// survives.
async fn delete_expired_batch(ctx: &mut DbContext, cutoff: i64) -> Result<u64> {
    let hashes: Vec<String> = linq!(
        ctx.set::<IdempotencyRecord>(),
        |record: IdempotencyRecord| record.expires_at < cutoff;
        take PURGE_BATCH_SIZE;
        select (record.key_hash)
    )
    .to_list_typed_1::<String>()
    .await
    .map_err(map_ef_error)?;

    if hashes.is_empty() {
        return Ok(0);
    }

    linq!(
        ctx.set::<IdempotencyRecord>(),
        |record: IdempotencyRecord| hashes.contains(record.key_hash) && record.expires_at < cutoff
    )
    .execute_delete()
    .await
    .map_err(map_ef_error)
}

/// Find the row for a protocol key (looked up by its digest), if any.
async fn find_record(
    ctx: &mut DbContext,
    key: &str,
) -> rust_ef::error::EFResult<Option<IdempotencyRecord>> {
    let target = key_hash(key);
    linq!(
        ctx.set::<IdempotencyRecord>(),
        |record: IdempotencyRecord| record.key_hash == target
    )
    .first_or_default()
    .await
}

/// Apply the three-state protocol to an existing row.
///
/// * completed and unexpired → replay the stored response;
/// * expired (either state) → claim it with one conditional UPDATE so exactly one
///   caller becomes the owner; the update writes `attempt` and clears the
///   previous response;
/// * otherwise → another request is still running.
async fn resolve_existing(
    ctx: &mut DbContext,
    record: IdempotencyRecord,
    now: i64,
    expires_at: i64,
    attempt: String,
) -> Result<(IdempotencyBegin, Option<IdempotencyLease>)> {
    if record.state == STATE_COMPLETED && record.expires_at > now {
        return Ok((IdempotencyBegin::Replay(stored_response(&record)), None));
    }

    if record.expires_at <= now {
        let target = record.key_hash;
        let affected = linq!(ctx.set::<IdempotencyRecord>(), |row: IdempotencyRecord| row
            .key_hash
            == target
            && row.expires_at <= now)
        .execute_update()
        .set_column(IdempotencyRecord::COLUMN_STATE, STATE_IN_PROGRESS)
        .set_column(IdempotencyRecord::COLUMN_ATTEMPT, Some(attempt.clone()))
        .set_column(IdempotencyRecord::COLUMN_STATUS, DbValue::Null)
        .set_column(IdempotencyRecord::COLUMN_CONTENT_TYPE, DbValue::Null)
        .set_column(IdempotencyRecord::COLUMN_CONTENT_ENCODING, DbValue::Null)
        .set_column(IdempotencyRecord::COLUMN_HEADERS, DbValue::Null)
        .set_column(IdempotencyRecord::COLUMN_BODY, DbValue::Null)
        .set_column(IdempotencyRecord::COLUMN_CREATED_AT, now)
        .set_column(IdempotencyRecord::COLUMN_EXPIRES_AT, expires_at)
        .execute()
        .await
        .map_err(map_ef_error)?;
        // The CAS re-checks the expiry: a concurrent owner that completed the
        // key between our SELECT and UPDATE wins and we report InProgress.
        return Ok(if affected == 1 {
            (
                IdempotencyBegin::Started,
                Some(IdempotencyLease::new(attempt)),
            )
        } else {
            (IdempotencyBegin::InProgress, None)
        });
    }

    Ok((IdempotencyBegin::InProgress, None))
}

/// Rebuild the stored response of a completed row.
fn stored_response(record: &IdempotencyRecord) -> IdempotentResponse {
    IdempotentResponse::new(
        // A completed row is written by `complete`, which always stores the
        // status; 200 is only a defensive fallback for a hand-edited row.
        record
            .status
            .and_then(|status| u16::try_from(status).ok())
            .unwrap_or(200),
        record.content_type.clone(),
        record.content_encoding.clone(),
        record.body.clone().unwrap_or_default(),
    )
    .with_headers(decode_headers(record))
}

#[async_trait]
impl IIdempotencyStore for EfIdempotencyStore {
    async fn begin(&self, key: &str, ttl: Duration) -> Result<IdempotencyBegin> {
        // Legacy token-less entry point: no separate lease, so the reservation
        // and the stored response are both bounded by `ttl`.
        let (begin, _lease) = self.begin_lease(key, ttl, ttl).await?;
        Ok(begin)
    }

    async fn begin_lease(
        &self,
        key: &str,
        _retention: Duration,
        lease: Duration,
    ) -> Result<(IdempotencyBegin, Option<IdempotencyLease>)> {
        let now = unix_now_secs();
        // The reservation row lives for the caller's lease, not for the replay
        // retention: a crashed handler must be retryable after ~`lease`, not
        // after the full retention period. The caller (the middleware) already
        // caps `lease` at the retention; the store honours the value as given.
        let expires_at = now.saturating_add(lease_seconds(lease));
        let attempt = new_attempt_token();
        let mut ctx = self.context()?;

        if let Some(record) = find_record(&mut ctx, key).await.map_err(map_ef_error)? {
            return resolve_existing(&mut ctx, record, now, expires_at, attempt).await;
        }

        ctx.add(IdempotencyRecord {
            key_hash: key_hash(key),
            key: debug_key(key),
            state: STATE_IN_PROGRESS.to_string(),
            status: None,
            content_type: None,
            content_encoding: None,
            attempt: Some(attempt.clone()),
            headers: None,
            body: None,
            created_at: now,
            expires_at,
        });
        match ctx.save_changes().await {
            Ok(_) => Ok((
                IdempotencyBegin::Started,
                Some(IdempotencyLease::new(attempt)),
            )),
            Err(EFError::UniqueViolation { .. }) => {
                // Another instance inserted the same key between our SELECT and
                // INSERT. The failed INSERT is rolled back; drop its tracking
                // entry and resolve the row the other node created.
                ctx.clear_tracking();
                let Some(record) = find_record(&mut ctx, key).await.map_err(map_ef_error)? else {
                    // The winning row was purged before we could read it. Do not
                    // claim the key twice: report InProgress and let the client's
                    // next retry take the normal path.
                    return Ok((IdempotencyBegin::InProgress, None));
                };
                resolve_existing(&mut ctx, record, now, expires_at, attempt).await
            }
            Err(error) => Err(map_ef_error(error)),
        }
    }

    async fn complete(&self, key: &str, response: IdempotentResponse, ttl: Duration) -> Result<()> {
        self.complete_attempt(key, None, response, ttl).await
    }

    async fn complete_lease(
        &self,
        key: &str,
        lease: Option<&IdempotencyLease>,
        response: IdempotentResponse,
        retention: Duration,
    ) -> Result<()> {
        self.complete_attempt(
            key,
            lease.map(|lease| lease.token.as_str()),
            response,
            retention,
        )
        .await
    }

    async fn release(&self, key: &str) -> Result<()> {
        self.release_attempt(key, None).await
    }

    async fn release_lease(&self, key: &str, lease: Option<&IdempotencyLease>) -> Result<()> {
        self.release_attempt(key, lease.map(|lease| lease.token.as_str()))
            .await
    }
}

impl EfIdempotencyStore {
    /// Store the response of one attempt.
    ///
    /// `attempt` is the fencing token of the lease variant (`None` for the
    /// token-less `complete`, which keeps the pre-2.8 behavior). When set, the
    /// compare-and-set also requires the row to still carry that token, so a
    /// reclaimed reservation's late completion is a no-op. `retention` is how
    /// long the stored response replays.
    async fn complete_attempt(
        &self,
        key: &str,
        attempt: Option<&str>,
        response: IdempotentResponse,
        retention: Duration,
    ) -> Result<()> {
        if response.body.len() > self.max_body_bytes {
            // The table column cannot hold this body (MySQL BLOB is 64 KiB).
            // Free the reservation so the client's retry re-executes instead of
            // receiving 409 until the retention TTL; do not fail the request.
            tracing::warn!(
                body_bytes = response.body.len(),
                max_bytes = self.max_body_bytes,
                "idempotency: response body exceeds the store column limit; re-execution allowed"
            );
            return self.release_attempt(key, attempt).await;
        }
        let now = unix_now_secs();
        let expires_at = now.saturating_add(lease_seconds(retention));
        let target = key_hash(key);
        let target_for_log = target.clone();
        let attempt_for_log = attempt.map(str::to_string);
        let mut ctx = self.context()?;

        // `expires_at > now` is part of the compare-and-set: a completion that
        // arrives after the reservation expired must be discarded instead of
        // resurrecting a lease that a retry may already have reclaimed. With a
        // lease token the CAS also requires the row to still be owned by this
        // attempt, which fences out a late completion from a reclaimed attempt.
        let mut query = linq!(
            ctx.set::<IdempotencyRecord>(),
            |record: IdempotencyRecord| record.key_hash == target
                && record.state == STATE_IN_PROGRESS
                && record.expires_at > now
        );
        if let Some(attempt) = attempt {
            query = query.filter(linq!(
                |record: IdempotencyRecord| record.attempt == Some(attempt.to_string())
            ));
        }
        let affected = query
            .execute_update()
            .set_column(
                IdempotencyRecord::COLUMN_STATUS,
                Some(i32::from(response.status)),
            )
            .set_column(
                IdempotencyRecord::COLUMN_CONTENT_TYPE,
                response.content_type,
            )
            .set_column(
                IdempotencyRecord::COLUMN_CONTENT_ENCODING,
                response.content_encoding,
            )
            .set_column(
                IdempotencyRecord::COLUMN_HEADERS,
                encode_headers(&response.headers),
            )
            .set_column(IdempotencyRecord::COLUMN_BODY, Some(response.body))
            .set_column(IdempotencyRecord::COLUMN_STATE, STATE_COMPLETED)
            .set_column(IdempotencyRecord::COLUMN_ATTEMPT, DbValue::Null)
            .set_column(IdempotencyRecord::COLUMN_EXPIRES_AT, expires_at)
            .execute()
            .await
            .map_err(map_ef_error)?;
        if affected == 0 {
            // The reservation expired (outlived its lease), was released or was
            // reclaimed by another attempt while the handler ran; a retry may
            // already be executing again, so a late completion must not
            // resurrect the old response or overwrite the new attempt. Not an
            // error: the middleware still returns the handler's own response to
            // this caller. Logged at `warn` because a pattern of these means the
            // handler's worst case exceeds Idempotency.LeaseSeconds (the handler
            // may now run twice for one key) or a clock skew is shortening the
            // lease.
            tracing::warn!(
                key_hash = %target_for_log,
                lease = ?attempt_for_log,
                "idempotency: a completed response was not stored because the reservation is \
                 no longer owned by this attempt (its lease expired, it was released, or a \
                 retry reclaimed the key); a retry may execute the handler again — raise \
                 Idempotency.LeaseSeconds above the handler's worst-case duration"
            );
        }
        Ok(())
    }

    /// Delete the live reservation of one attempt.
    ///
    /// `attempt` is the fencing token of the lease variant; when set, the row is
    /// only deleted while it still carries that token.
    async fn release_attempt(&self, key: &str, attempt: Option<&str>) -> Result<()> {
        let now = unix_now_secs();
        let target = key_hash(key);
        let mut ctx = self.context()?;
        // Same expiry guard as `complete`: releasing an already-expired (and
        // possibly reclaimed) reservation must not delete another attempt's row.
        let mut query = linq!(
            ctx.set::<IdempotencyRecord>(),
            |record: IdempotencyRecord| record.key_hash == target
                && record.state == STATE_IN_PROGRESS
                && record.expires_at > now
        );
        if let Some(attempt) = attempt {
            query = query.filter(linq!(
                |record: IdempotencyRecord| record.attempt == Some(attempt.to_string())
            ));
        }
        let affected = query.execute_delete().await.map_err(map_ef_error)?;
        if affected > 0 {
            tracing::debug!("idempotency: released an in-progress reservation");
        }
        Ok(())
    }
}

/// Periodic maintenance for [`EfIdempotencyStore`]: deletes expired rows.
///
/// Register it with `add_background_service_with` (it implements
/// [`PeriodicBackgroundService`]); the defaults are one tick per hour and a
/// 24-hour grace after `expires_at`. Cleanup is one indexed scan per
/// [`PURGE_BATCH_SIZE`] rows and is safe to run on every instance — deleting the
/// same expired row twice is a no-op.
#[derive(Clone)]
pub struct IdempotencyHousekeeping {
    store: Arc<EfIdempotencyStore>,
    interval: Duration,
    grace: Duration,
}

impl IdempotencyHousekeeping {
    /// A housekeeping worker over `store` with the default interval and grace.
    pub fn new(store: Arc<EfIdempotencyStore>) -> Self {
        Self {
            store,
            interval: DEFAULT_CLEANUP_INTERVAL,
            grace: DEFAULT_CLEANUP_GRACE,
        }
    }

    /// Change the tick interval (must be greater than zero).
    pub fn with_interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }

    /// Change how long an expired row is kept before deletion.
    pub fn with_grace(mut self, grace: Duration) -> Self {
        self.grace = grace;
        self
    }
}

#[async_trait]
impl PeriodicBackgroundService for IdempotencyHousekeeping {
    fn interval(&self) -> Duration {
        self.interval
    }

    async fn tick(&self) -> Result<()> {
        let grace = i64::try_from(self.grace.as_secs()).unwrap_or(i64::MAX);
        let purged = self.store.purge_expired(unix_now_secs(), grace).await?;
        if purged > 0 {
            tracing::debug!(purged, "idempotency: purged expired keys");
        }
        Ok(())
    }
}

/// The stored primary key for a protocol key: the SHA-256 digest as lowercase
/// hex.
///
/// The middleware's protocol key embeds the principal, method, request path and
/// the client's `Idempotency-Key` header; the path and header are not bounded by
/// the framework, so the database primary key is a fixed-size digest instead of
/// the raw string. The digest is stable across instances (same input, same
/// primary key), which is what makes the uniqueness/CAS protocol work.
pub fn key_hash(key: &str) -> String {
    let digest = Sha256::digest(key.as_bytes());
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// Largest body the provider's `Vec<u8>` column can hold without truncation.
fn default_body_budget(options: &DbContextOptions) -> usize {
    // MySQL maps Vec<u8> to BLOB (65,535 bytes); PostgreSQL BYTEA and SQLite
    // BLOB are effectively unbounded, so the middleware's MaxBodyBytes caps
    // them. A provider that cannot be constructed falls back to the safe value.
    const MYSQL_BLOB_LIMIT: usize = 60_000;
    match options.create_provider() {
        Ok(provider) if provider.name().eq_ignore_ascii_case("MySQL") => MYSQL_BLOB_LIMIT,
        _ => usize::MAX,
    }
}

/// Whole seconds of a lease `ttl`, clamped to at least one second.
///
/// The column is Unix seconds, so a sub-second TTL would create a row that is
/// already expired; one second is the shortest meaningful lease.
fn lease_seconds(ttl: Duration) -> i64 {
    i64::try_from(ttl.as_secs()).unwrap_or(i64::MAX).max(1)
}

/// Truncated protocol key stored for operators (the lookup uses the digest).
///
/// The full protocol key includes the client header and the request path; only
/// the first 1024 characters are kept so a pathological path can never overflow
/// the MySQL `TEXT` column. The digest remains the identity.
fn debug_key(key: &str) -> String {
    const MAX_DEBUG_KEY_CHARS: usize = 1024;
    key.chars().take(MAX_DEBUG_KEY_CHARS).collect()
}

/// Largest JSON header set stored in the `headers` column (8 KiB).
///
/// The whitelist means at most four headers; the cap only guards against a
/// pathological header value. A larger set is dropped (with a warning) instead
/// of failing the response or overflowing a MySQL `TEXT` column; the body is
/// cached normally.
const MAX_HEADERS_BYTES: usize = 8 * 1024;

/// A fresh opaque fencing token for one reservation attempt.
fn new_attempt_token() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// Serialize the replayable response headers for storage.
///
/// An empty set is stored as `NULL` (same as rows written before 2.8), and a set
/// that exceeds [`MAX_HEADERS_BYTES`] is dropped with a warning.
fn encode_headers(headers: &[(String, String)]) -> Option<String> {
    if headers.is_empty() {
        return None;
    }
    match serde_json::to_string(headers) {
        Ok(json) if json.len() <= MAX_HEADERS_BYTES => Some(json),
        Ok(json) => {
            tracing::warn!(
                headers_bytes = json.len(),
                max_bytes = MAX_HEADERS_BYTES,
                "idempotency: replayable response headers exceed the store limit; not stored"
            );
            None
        }
        Err(error) => {
            tracing::warn!(
                %error,
                "idempotency: replayable response headers could not be encoded; not stored"
            );
            None
        }
    }
}

/// Rebuild the replayable response headers of a completed row.
///
/// Only [`REPLAYABLE_RESPONSE_HEADERS`] are replayed (case-insensitively); a
/// malformed JSON column is treated as "no headers" instead of failing the
/// replay.
fn decode_headers(record: &IdempotencyRecord) -> Vec<(String, String)> {
    let Some(json) = record.headers.as_deref() else {
        return Vec::new();
    };
    let decoded: Vec<(String, String)> = match serde_json::from_str(json) {
        Ok(headers) => headers,
        Err(error) => {
            tracing::warn!(
                %error,
                key_hash = %record.key_hash,
                "idempotency: stored replayable headers are not valid JSON; replaying none"
            );
            return Vec::new();
        }
    };
    decoded
        .into_iter()
        .filter(|(name, _)| {
            REPLAYABLE_RESPONSE_HEADERS
                .iter()
                .any(|allowed| name.eq_ignore_ascii_case(allowed))
        })
        .map(|(name, value)| (name.to_ascii_lowercase(), value))
        .collect()
}

/// Current Unix time in whole seconds.
fn unix_now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sub_second_ttl_still_produces_a_one_second_lease() {
        assert_eq!(lease_seconds(Duration::ZERO), 1);
        assert_eq!(lease_seconds(Duration::from_millis(500)), 1);
        assert_eq!(lease_seconds(Duration::from_secs(90)), 90);
    }

    #[test]
    fn a_record_can_be_constructed_without_a_response() {
        let record = IdempotencyRecord {
            key_hash: key_hash("idem:user:post:/orders:3f9c"),
            key: "idem:user:post:/orders:3f9c".to_string(),
            state: STATE_IN_PROGRESS.to_string(),
            status: None,
            content_type: None,
            content_encoding: None,
            attempt: None,
            headers: None,
            body: None,
            created_at: 1,
            expires_at: 2,
        };
        assert_eq!(record.state, STATE_IN_PROGRESS);
        assert!(record.body.is_none());
        assert!(record.attempt.is_none(), "pre-2.8 rows carry no token");
    }
}

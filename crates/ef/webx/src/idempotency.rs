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
//! * **TTL and leases**: a reservation is a lease. A killed node's reservation
//!   expires after the configured TTL and the client's next retry claims it with
//!   a conditional `UPDATE ... WHERE key_hash = ? AND expires_at <= ?`.
//!   `complete`/`release` are conditional on `expires_at > now`, so a late
//!   completion for an already-expired reservation is discarded instead of
//!   resurrecting it. The port carries no per-attempt token, so a completion
//!   that races a reclaim can still overwrite the new lease; keep the TTL above
//!   the handler's worst-case duration (the only execution guarantee outside a
//!   lease is at-most-once *while* the key stays reserved).
//! * **Cleanup**: rows stay readable until `expires_at`; a row is deleted only
//!   after it has been expired for the grace period. `IdempotencyHousekeeping`
//!   does that on a periodic tick (default: hourly, grace 24h).
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
//!
//! # Race semantics
//!
//! * `begin`: a missing row is INSERTed; a unique-key violation means another
//!   node inserted it first, so the tracking state is dropped and the existing
//!   row is resolved. A completed, unexpired row replays its stored response;
//!   an expired row is claimed with one conditional UPDATE (exactly one winner),
//!   which also clears the previous response; a live row is `InProgress`.
//! * `complete`: a conditional UPDATE (`WHERE state = 'in_progress'`) stores the
//!   response. An affected-row count of `0` is not an error — the reservation
//!   expired or was released, so a retry may run the handler again (a late
//!   completion must never resurrect a stale reservation).
//! * `release`: deletes the row only while it is still `in_progress`, so
//!   releasing a completed key does not drop its cached response.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use rust_ef::db_context::{DbContext, DbContextOptions};
use rust_ef::error::EFError;
use rust_ef::prelude::*;
use sha2::{Digest, Sha256};
use webx_core::error::{Error, Result};
use webx_core::hosted::PeriodicBackgroundService;
use webx_core::idempotency::{IIdempotencyStore, IdempotencyBegin, IdempotentResponse};

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

/// One stored idempotency key.
///
/// `key` is the primary key (unique across all instances), so a duplicate INSERT
/// is a unique-constraint violation. `expires_at` is a Unix-seconds lease
/// deadline: after it the key can be claimed again, which is also what makes a
/// killed instance recoverable. `body` is the raw response body (compressed
/// bytes are stored as-is together with `content_encoding`).
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
}

impl EfIdempotencyStore {
    /// Build a store over the application's database options.
    ///
    /// The options must be the ones the host resolves `DbContext` from, so both
    /// use the same provider/connection pool and metadata cache.
    pub fn new(options: Arc<DbContextOptions>) -> Self {
        Self { options }
    }

    /// Delete rows that expired before `now - older_than` (Unix seconds).
    ///
    /// Rows are kept for `older_than` seconds after `expires_at` so an operator
    /// can still inspect a recently expired key. `older_than` is clamped to `0`,
    /// so `purge_expired(now, 0)` deletes every expired row. Returns the number
    /// of deleted rows (bounded by the database driver, one statement).
    pub async fn purge_expired(&self, now: i64, older_than: i64) -> Result<u64> {
        let mut ctx = self.context()?;
        let cutoff = now.saturating_sub(older_than.max(0));
        linq!(
            ctx.set::<IdempotencyRecord>(),
            |record: IdempotencyRecord| record.expires_at < cutoff
        )
        .execute_delete()
        .await
        .map_err(map_ef_error)
    }

    /// Open a context for one operation.
    ///
    /// The options are built once at startup, so a failure here is a
    /// configuration problem (missing provider / invalid connection string),
    /// not a per-request database error; per-request failures surface from the
    /// query itself as mapped framework errors.
    fn context(&self) -> Result<DbContext> {
        DbContext::from_options(&self.options).map_err(|error| {
            Error::Internal(format!(
                "the idempotency store could not create a DbContext from the configured \
                 options: {error}"
            ))
        })
    }
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
///   caller becomes the owner and the previous response is cleared;
/// * otherwise → another request is still running.
async fn resolve_existing(
    ctx: &mut DbContext,
    record: IdempotencyRecord,
    now: i64,
    expires_at: i64,
) -> Result<IdempotencyBegin> {
    if record.state == STATE_COMPLETED && record.expires_at > now {
        return Ok(IdempotencyBegin::Replay(stored_response(&record)));
    }

    if record.expires_at <= now {
        let target = record.key_hash;
        let affected = linq!(ctx.set::<IdempotencyRecord>(), |row: IdempotencyRecord| row
            .key_hash
            == target
            && row.expires_at <= now)
        .execute_update()
        .set_column(IdempotencyRecord::COLUMN_STATE, STATE_IN_PROGRESS)
        .set_column(IdempotencyRecord::COLUMN_STATUS, DbValue::Null)
        .set_column(IdempotencyRecord::COLUMN_CONTENT_TYPE, DbValue::Null)
        .set_column(IdempotencyRecord::COLUMN_CONTENT_ENCODING, DbValue::Null)
        .set_column(IdempotencyRecord::COLUMN_BODY, DbValue::Null)
        .set_column(IdempotencyRecord::COLUMN_CREATED_AT, now)
        .set_column(IdempotencyRecord::COLUMN_EXPIRES_AT, expires_at)
        .execute()
        .await
        .map_err(map_ef_error)?;
        // The CAS re-checks the expiry: a concurrent owner that completed the
        // key between our SELECT and UPDATE wins and we report InProgress.
        return Ok(if affected == 1 {
            IdempotencyBegin::Started
        } else {
            IdempotencyBegin::InProgress
        });
    }

    Ok(IdempotencyBegin::InProgress)
}

/// Rebuild the stored response of a completed row.
fn stored_response(record: &IdempotencyRecord) -> IdempotentResponse {
    IdempotentResponse {
        // A completed row is written by `complete`, which always stores the
        // status; 200 is only a defensive fallback for a hand-edited row.
        status: record
            .status
            .and_then(|status| u16::try_from(status).ok())
            .unwrap_or(200),
        content_type: record.content_type.clone(),
        content_encoding: record.content_encoding.clone(),
        body: record.body.clone().unwrap_or_default(),
    }
}

#[async_trait]
impl IIdempotencyStore for EfIdempotencyStore {
    async fn begin(&self, key: &str, ttl: Duration) -> Result<IdempotencyBegin> {
        let now = unix_now_secs();
        let expires_at = now.saturating_add(lease_seconds(ttl));
        let mut ctx = self.context()?;

        if let Some(record) = find_record(&mut ctx, key).await.map_err(map_ef_error)? {
            return resolve_existing(&mut ctx, record, now, expires_at).await;
        }

        ctx.add(IdempotencyRecord {
            key_hash: key_hash(key),
            key: key.to_string(),
            state: STATE_IN_PROGRESS.to_string(),
            status: None,
            content_type: None,
            content_encoding: None,
            body: None,
            created_at: now,
            expires_at,
        });
        match ctx.save_changes().await {
            Ok(_) => Ok(IdempotencyBegin::Started),
            Err(EFError::UniqueViolation { .. }) => {
                // Another instance inserted the same key between our SELECT and
                // INSERT. The failed INSERT is rolled back; drop its tracking
                // entry and resolve the row the other node created.
                ctx.clear_tracking();
                let Some(record) = find_record(&mut ctx, key).await.map_err(map_ef_error)? else {
                    // The winning row was purged before we could read it. Do not
                    // claim the key twice: report InProgress and let the client's
                    // next retry take the normal path.
                    return Ok(IdempotencyBegin::InProgress);
                };
                resolve_existing(&mut ctx, record, now, expires_at).await
            }
            Err(error) => Err(map_ef_error(error)),
        }
    }

    async fn complete(&self, key: &str, response: IdempotentResponse, ttl: Duration) -> Result<()> {
        let now = unix_now_secs();
        let expires_at = now.saturating_add(lease_seconds(ttl));
        let target = key_hash(key);
        let mut ctx = self.context()?;

        // `expires_at > now` is part of the compare-and-set: a completion that
        // arrives after the reservation expired must be discarded instead of
        // resurrecting a lease that a retry may already have reclaimed. (The
        // port carries no per-attempt token, so a completion that races a
        // reclaim can still land on the new lease - keep the TTL above the
        // handler's worst-case duration; see the deployment notes.)
        let affected = linq!(
            ctx.set::<IdempotencyRecord>(),
            |record: IdempotencyRecord| record.key_hash == target
                && record.state == STATE_IN_PROGRESS
                && record.expires_at > now
        )
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
        .set_column(IdempotencyRecord::COLUMN_BODY, Some(response.body))
        .set_column(IdempotencyRecord::COLUMN_STATE, STATE_COMPLETED)
        .set_column(IdempotencyRecord::COLUMN_EXPIRES_AT, expires_at)
        .execute()
        .await
        .map_err(map_ef_error)?;
        if affected == 0 {
            // The reservation expired or was released while the handler ran; a
            // retry may already be executing again, so a late completion must
            // not resurrect the old response. Not an error: the middleware
            // still returns the handler's own response to this caller.
            tracing::debug!(
                "idempotency: the reservation is no longer in progress (expired or released); \
                 response not stored"
            );
        }
        Ok(())
    }

    async fn release(&self, key: &str) -> Result<()> {
        let now = unix_now_secs();
        let target = key_hash(key);
        let mut ctx = self.context()?;
        // Same expiry guard as `complete`: releasing an already-expired (and
        // possibly reclaimed) reservation must not delete another attempt's row.
        let affected = linq!(
            ctx.set::<IdempotencyRecord>(),
            |record: IdempotencyRecord| record.key_hash == target
                && record.state == STATE_IN_PROGRESS
                && record.expires_at > now
        )
        .execute_delete()
        .await
        .map_err(map_ef_error)?;
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
/// 24-hour grace after `expires_at`. Cleanup is a single indexed `DELETE` per
/// tick and is safe to run on every instance — deleting the same expired row
/// twice is a no-op.
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

/// Whole seconds of a lease `ttl`, clamped to at least one second.
///
/// The column is Unix seconds, so a sub-second TTL would create a row that is
/// already expired; one second is the shortest meaningful lease.
fn lease_seconds(ttl: Duration) -> i64 {
    i64::try_from(ttl.as_secs()).unwrap_or(i64::MAX).max(1)
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
            body: None,
            created_at: 1,
            expires_at: 2,
        };
        assert_eq!(record.state, STATE_IN_PROGRESS);
        assert!(record.body.is_none());
    }
}

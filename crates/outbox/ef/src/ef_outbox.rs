//! rust-ef implementation of the outbox: producer-side enqueue + worker storage.
//!
//! Producers enqueue through [`OutboxDbContextExt`] on **their own `DbContext`**,
//! so the outbox row commits (or rolls back) with the business data. The worker
//! (same crate) resolves an owned `DbContext` from a fresh DI scope per storage
//! operation, so the context never crosses an await point.
//!
//! The application registers the context with rust-ef's `add_dbcontext`
//! (SQLite/PostgreSQL/MySQL); this crate never builds a second connection stack.

use std::sync::Arc;

use async_trait::async_trait;
use rust_dix::ScopeFactory;
use rust_ef::db_context::DbContext;
use rust_ef::error::{EFError, EFResult};
use rust_ef::prelude::*;
use webx_core::error::{Error, Result};
use webx_core::mediator::IEventRequest;

use crate::entity::OutboxMessage;
use crate::port::{store_error, IOutbox, OutboxEntry};

/// Longest `last_error` text persisted on a row.
const MAX_ERROR_CHARS: usize = 500;

/// Maximum backoff exponent (delay is capped at `base * 2^10`).
const MAX_BACKOFF_EXPONENT: u32 = 10;

/// Enqueue helpers for the application's `DbContext` (producer side).
///
/// All methods only **attach** the row to the current unit of work; the caller
/// persists it with `ctx.save_changes().await`, normally inside
/// `ctx.use_transaction(...)` together with the business changes. A failed
/// transaction therefore leaves no outbox row behind.
pub trait OutboxDbContextExt {
    /// Queue `event` with a generated id; returns the id.
    fn enqueue_outbox<T>(&mut self, event: &T) -> EFResult<String>
    where
        T: IEventRequest + serde::Serialize;

    /// Queue `event` under a caller-chosen id.
    ///
    /// A deterministic id makes producer retries idempotent: the duplicate insert
    /// fails with [`EFError::UniqueViolation`], which the caller can treat as
    /// "already queued".
    fn enqueue_outbox_with_id<T>(&mut self, id: &str, event: &T) -> EFResult<()>
    where
        T: IEventRequest + serde::Serialize;

    /// Queue an already-encoded wire event (JSON payload).
    fn enqueue_outbox_wire(
        &mut self,
        id: &str,
        event_name: &str,
        payload_json: &str,
    ) -> EFResult<()>;
}

impl OutboxDbContextExt for DbContext {
    fn enqueue_outbox<T>(&mut self, event: &T) -> EFResult<String>
    where
        T: IEventRequest + serde::Serialize,
    {
        let id = uuid::Uuid::new_v4().to_string();
        self.enqueue_outbox_with_id(&id, event)?;
        Ok(id)
    }

    fn enqueue_outbox_with_id<T>(&mut self, id: &str, event: &T) -> EFResult<()>
    where
        T: IEventRequest + serde::Serialize,
    {
        let payload = serde_json::to_string(event).map_err(|error| {
            EFError::Other(
                format!("failed to encode the outbox event as JSON: {error}"),
                None,
            )
        })?;
        self.enqueue_outbox_wire(id, T::event_name(), &payload)
    }

    fn enqueue_outbox_wire(
        &mut self,
        id: &str,
        event_name: &str,
        payload_json: &str,
    ) -> EFResult<()> {
        if id.trim().is_empty() || id.len() > 36 {
            return Err(EFError::model_validation(
                "outbox id must be 1-36 characters",
            ));
        }
        if event_name.trim().is_empty() || event_name.len() > 128 {
            return Err(EFError::model_validation(
                "outbox event name must be 1-128 characters",
            ));
        }
        let now = unix_now_secs();
        self.add(OutboxMessage {
            id: id.to_string(),
            event_name: event_name.to_string(),
            payload: payload_json.to_string(),
            attempts: 0,
            next_attempt_at: now,
            locked_until: 0,
            delivered_at: None,
            dead_lettered_at: None,
            last_error: None,
            created_at: now,
        });
        Ok(())
    }
}

/// Durable [`IOutbox`] backed by rust-ef.
pub struct EfOutbox {
    services: Arc<dyn ScopeFactory>,
    max_attempts: i32,
}

impl EfOutbox {
    /// Build a store over the container's scopes.
    ///
    /// The `DbContext` must be registered as a scoped rust-ef service (`add_dbcontext`);
    /// the outbox fails with a readable error when it is not.
    pub fn new(services: Arc<dyn ScopeFactory>, max_attempts: i32) -> Self {
        Self {
            services,
            max_attempts: max_attempts.max(1),
        }
    }

    /// Open a scope and resolve the owned `DbContext` for one operation.
    fn context(&self) -> Result<DbContext> {
        let scope = self.services.create_scope();
        scope.get_owned::<DbContext>().map_err(|error| {
            Error::Internal(format!(
                "outbox needs a scoped DbContext registered with rust-ef `add_dbcontext`; \
                 resolving it from a fresh DI scope failed: {error}"
            ))
        })
    }
}

#[async_trait]
impl IOutbox for EfOutbox {
    async fn claim(&self, now: i64, limit: usize, lease_secs: i64) -> Result<Vec<OutboxEntry>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let lease_until = now.saturating_add(lease_secs.max(1));
        let mut ctx = self.context()?;

        // Candidate selection is only a hint: the per-row CAS below re-checks
        // every guard, so a row delivered/claimed concurrently is skipped.
        let candidates = linq!(
            ctx.set::<OutboxMessage>(),
            |row: OutboxMessage| row.delivered_at.is_null()
                && row.dead_lettered_at.is_null()
                && row.next_attempt_at <= now
                && row.locked_until <= now;
            order_by row.next_attempt_at
        )
        .take(limit)
        .to_list()
        .await
        .map_err(store_error)?;

        let mut claimed = Vec::with_capacity(candidates.len());
        for row in candidates {
            let id = row.id.clone();
            let affected = linq!(ctx.set::<OutboxMessage>(), |guard: OutboxMessage| guard.id
                == id
                && guard.delivered_at.is_null()
                && guard.dead_lettered_at.is_null()
                && guard.next_attempt_at <= now
                && guard.locked_until <= now)
            .execute_update()
            .set_column(OutboxMessage::COLUMN_LOCKED_UNTIL, lease_until)
            .execute()
            .await
            .map_err(store_error)?;
            if affected == 1 {
                claimed.push(OutboxEntry {
                    id: row.id,
                    event_name: row.event_name,
                    payload: row.payload,
                    attempts: row.attempts,
                });
            }
        }
        Ok(claimed)
    }

    async fn complete(&self, id: &str) -> Result<()> {
        let mut ctx = self.context()?;
        let target = id.to_string();
        let now = unix_now_secs();
        linq!(ctx.set::<OutboxMessage>(), |row: OutboxMessage| row.id
            == target
            && row.delivered_at.is_null()
            && row.dead_lettered_at.is_null())
        .execute_update()
        .set_column(OutboxMessage::COLUMN_DELIVERED_AT, Some(now))
        .set_column(OutboxMessage::COLUMN_LOCKED_UNTIL, 0i64)
        .execute()
        .await
        .map_err(store_error)?;
        Ok(())
    }

    async fn fail(&self, id: &str, error: &str, retry_base_secs: i64) -> Result<()> {
        let mut ctx = self.context()?;
        let target = id.to_string();
        let query_id = target.clone();
        let Some(row) = linq!(ctx.set::<OutboxMessage>(), |row: OutboxMessage| row.id
            == query_id)
        .first_or_default()
        .await
        .map_err(store_error)?
        else {
            return Ok(());
        };
        if row.delivered_at.is_some() || row.dead_lettered_at.is_some() {
            // A late failure report for an already-finished row is a no-op.
            return Ok(());
        }

        let now = unix_now_secs();
        let message = truncate(error, MAX_ERROR_CHARS);
        let attempts = row.attempts.saturating_add(1);
        let dead_lettered = attempts >= self.max_attempts;
        let next_attempt_at = if dead_lettered {
            row.next_attempt_at
        } else {
            // 2^(attempts - 1) base delay: base, 2x, 4x, ... capped.
            let exponent =
                u32::try_from(attempts.saturating_sub(1).min(MAX_BACKOFF_EXPONENT as i32))
                    .unwrap_or(0);
            let delay = retry_base_secs
                .max(1)
                .saturating_mul(2i64.saturating_pow(exponent));
            now.saturating_add(delay)
        };

        ctx.attach(row);
        let Some(tracked) = ctx
            .set::<OutboxMessage>()
            .tracked_entries_mut()
            .find(|row| row.id == target)
        else {
            return Ok(());
        };
        tracked.attempts = attempts;
        tracked.locked_until = 0;
        tracked.last_error = Some(message.clone());
        tracked.next_attempt_at = next_attempt_at;
        if dead_lettered {
            tracked.dead_lettered_at = Some(now);
        }
        ctx.detect_changes();
        ctx.save_changes().await.map_err(store_error)?;

        if dead_lettered {
            tracing::warn!(
                outbox_id = %target,
                attempts,
                error = %message,
                "outbox row dead-lettered"
            );
        }
        Ok(())
    }

    async fn dead_letter(&self, id: &str, error: &str) -> Result<()> {
        let mut ctx = self.context()?;
        let target = id.to_string();
        let now = unix_now_secs();
        linq!(ctx.set::<OutboxMessage>(), |row: OutboxMessage| row.id
            == target
            && row.delivered_at.is_null()
            && row.dead_lettered_at.is_null())
        .execute_update()
        .set_column(OutboxMessage::COLUMN_DEAD_LETTERED_AT, Some(now))
        .set_column(OutboxMessage::COLUMN_LOCKED_UNTIL, 0i64)
        .set_column(
            OutboxMessage::COLUMN_LAST_ERROR,
            Some(truncate(error, MAX_ERROR_CHARS)),
        )
        .execute()
        .await
        .map_err(store_error)?;
        Ok(())
    }

    async fn purge(&self, before: i64) -> Result<u64> {
        let mut ctx = self.context()?;
        linq!(ctx.set::<OutboxMessage>(), |row: OutboxMessage| (row
            .delivered_at
            .is_not_null()
            && row.delivered_at < before)
            || (row.dead_lettered_at.is_not_null()
                && row.dead_lettered_at < before))
        .execute_delete()
        .await
        .map_err(store_error)
    }
}

/// Current Unix time in whole seconds.
pub(crate) fn unix_now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// Cap `text` at `max` characters on a character boundary.
fn truncate(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

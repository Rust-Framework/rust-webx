//! Transactional outbox for `rust-webx` events, over `rust-ef`.
//!
//! The contract, the dispatcher and the in-memory store live in the framework
//! crate [`webx_outbox`]; this crate adds the two pieces that must be bound to
//! the application's database:
//!
//! * the **producer** helper [`OutboxDbContextExt`], which writes the outbox row
//!   **in the same transaction as the business data** — an event is published if
//!   and only if the business change commits;
//! * the durable **store** [`EfOutbox`] behind the framework [`IOutbox`] port.
//!
//! ```ignore
//! use webx_outbox_ef::{register_outbox_model, OutboxDbContextExt, OutboxEfHostBuilderExt};
//!
//! // once, where the DbContext model is registered
//! register_outbox_model(&mut ctx);
//!
//! // in the handler: one transaction, one commit
//! ctx.use_transaction(|ctx| async move {
//!     ctx.add(order);
//!     ctx.enqueue_outbox(&OrderPlaced { order_id })?;
//!     ctx.save_changes().await
//! })
//! .await?;
//!
//! // in the host
//! Host::builder().add_outbox_ef(OutboxOptions::default());
//! ```
//!
//! [`OutboxDispatcher`] is a
//! [`PeriodicBackgroundService`](webx_core::hosted::PeriodicBackgroundService):
//! each tick claims due rows with a lease + compare-and-set (so two instances
//! never dispatch the same row), sends each row through the process's
//! `#[handler] IEventHandler<T>` registrations, and marks it delivered or retries
//! with exponential backoff until the dead-letter threshold.
//!
//! # Semantics
//!
//! * **At-least-once delivery**: the handler runs before `delivered_at` is set, so
//!   a crash in between re-delivers the event — handlers must be idempotent.
//! * **Same transaction, no second connection**: producers enqueue on their own
//!   `DbContext`; the worker resolves an owned context per storage operation.
//! * **Bounded work**: `BatchSize` rows per tick, `LeaseSeconds` reclaim for a
//!   crashed worker, `MaxAttempts` before dead-letter, `RetentionDays` cleanup.
//! * **No schema at runtime**: the table is declared as an entity and materialized
//!   by the official rust-ef migration tooling; `ensure_created` is for tests.

mod ef_outbox;
mod entity;
mod register;

pub use ef_outbox::{EfOutbox, OutboxDbContextExt};
pub use entity::{register_outbox_model, OutboxMessage};
pub use register::{OutboxEfHostBuilderExt, OutboxEfServiceCollectionExt};
pub use webx_outbox::{IOutbox, OutboxDispatcher, OutboxEntry, OutboxOptions};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_surface_is_reexported() {
        let _ = std::any::type_name::<EfOutbox>();
        let _ = std::any::type_name::<OutboxDispatcher>();
        let _ = std::any::type_name::<OutboxOptions>();
        let _ = std::any::type_name::<OutboxEntry>();
        let _ = std::any::type_name::<OutboxMessage>();
        fn assert_trait<T: ?Sized>() {}
        assert_trait::<dyn IOutbox>();
        fn assert_host_ext<T: OutboxEfHostBuilderExt>() {}
        assert_host_ext::<webx_host::server::HostBuilder>();
        fn assert_collection_ext<T: OutboxEfServiceCollectionExt>() {}
        assert_collection_ext::<rust_dix::ServiceCollection>();
    }
}

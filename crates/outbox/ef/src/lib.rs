//! Transactional outbox for `rust-webx` events, over `rust-ef`.
//!
//! The producer writes the outbox row **in the same transaction as the business
//! data**, so an event is published if and only if the business change commits:
//!
//! ```ignore
//! use webx_outbox_ef::{register_outbox_model, OutboxDbContextExt};
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
//! ```
//!
//! [`OutboxDispatcher`] is a [`PeriodicBackgroundService`](webx_core::hosted::PeriodicBackgroundService):
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
//!
//! ```ignore
//! Host::builder()
//!     .add_outbox(OutboxOptions::default())
//!     .build();
//! ```

mod ef_outbox;
mod entity;
mod options;
mod port;
mod register;
mod worker;

pub use ef_outbox::{EfOutbox, OutboxDbContextExt};
pub use entity::{register_outbox_model, OutboxMessage};
pub use options::OutboxOptions;
pub use port::{IOutbox, OutboxEntry};
pub use register::{OutboxHostBuilderExt, OutboxServiceCollectionExt};
pub use worker::OutboxDispatcher;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_are_normalised_to_positive_values() {
        let options = OutboxOptions {
            interval_seconds: 0,
            batch_size: 0,
            lease_seconds: 0,
            max_attempts: 0,
            retry_base_seconds: 0,
            ..OutboxOptions::default()
        };
        assert_eq!(options.interval().as_secs(), 1);
        assert_eq!(options.batch_size(), 1);
        assert_eq!(options.lease_seconds(), 1);
        assert_eq!(options.max_attempts(), 1);
        assert_eq!(options.retry_base_seconds(), 1);
    }

    #[test]
    fn retention_zero_disables_cleanup() {
        let options = OutboxOptions {
            retention_days: 0,
            ..OutboxOptions::default()
        };
        assert_eq!(options.retention_seconds(), 0);
    }

    #[test]
    fn public_surface_is_reexported() {
        let _ = std::any::type_name::<EfOutbox>();
        let _ = std::any::type_name::<OutboxDispatcher>();
        let _ = std::any::type_name::<OutboxOptions>();
        let _ = std::any::type_name::<OutboxEntry>();
        let _ = std::any::type_name::<OutboxMessage>();
        fn assert_trait<T: ?Sized>() {}
        assert_trait::<dyn IOutbox>();
        fn assert_host_ext<T: OutboxHostBuilderExt>() {}
        assert_host_ext::<webx_host::server::HostBuilder>();
        fn assert_collection_ext<T: OutboxServiceCollectionExt>() {}
        assert_collection_ext::<rust_dix::ServiceCollection>();
    }
}

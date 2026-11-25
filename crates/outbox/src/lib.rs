//! Transactional outbox: the worker-facing contract, the lease-based dispatcher
//! and an in-memory store.
//!
//! # Boundary: dispatch semantics here, durability in a storage adapter
//!
//! An event is only really "outboxed" when the outbox row commits **in the same
//! transaction as the business change**. That atomicity belongs next to the
//! connection/`DbContext` that owns the transaction, so the producer-side
//! enqueue helper ships with the storage adapter (for example
//! [`rust-webx-outbox-ef`](https://docs.rs/rust-webx-outbox-ef)) and not here.
//!
//! This crate owns everything that is independent of a database:
//!
//! * [`IOutbox`] — the store contract the dispatcher drives: atomic claim with a
//!   lease, `complete`, `fail` with exponential backoff, `dead_letter`, `purge`;
//! * [`OutboxDispatcher`] — the
//!   [`PeriodicBackgroundService`](webx_core::hosted::PeriodicBackgroundService)
//!   that claims due rows and dispatches each one through the process's
//!   `#[handler] IEventHandler<T>` registrations;
//! * [`InMemoryOutbox`] — a `Mutex`-backed store for development and tests.
//!
//! A storage adapter registers its store through
//! [`add_outbox`](OutboxServiceCollectionExt::add_outbox) (or
//! [`add_outbox_with`](OutboxServiceCollectionExt::add_outbox_with) when the
//! store needs the container to build itself) and adds the producer-side
//! enqueue helper next to its own transaction.
//!
//! # Semantics guaranteed by the dispatcher
//!
//! * **At-least-once delivery**: the handlers run before the row is completed, so
//!   a crash in between re-delivers the event — handlers must be idempotent.
//! * **Single-winner claims**: claiming is a lease + compare-and-set, so two
//!   dispatchers never dispatch the same row, and a crashed worker's rows are
//!   reclaimed after `lease_seconds`.
//! * **Bounded work per tick**: `batch_size` rows, `max_attempts` failures before
//!   a dead letter, `retention_days` cleanup for finished rows.
//! * **Unroutable events fail fast**: a row whose event name has no registered
//!   handler is dead-lettered immediately instead of retrying forever.
//!
//! ```ignore
//! use std::sync::Arc;
//! use webx_outbox::{InMemoryOutbox, OutboxOptions, OutboxServiceCollectionExt};
//!
//! Host::builder().add_outbox(Arc::new(InMemoryOutbox::new(3)), OutboxOptions::default());
//! ```

mod memory;
mod options;
mod port;
mod register;
pub mod testing;
mod time;
mod worker;

pub use memory::{InMemoryOutbox, OutboxRecord};
pub use options::OutboxOptions;
pub use port::{IOutbox, OutboxEntry};
pub use register::{OutboxHostBuilderExt, OutboxServiceCollectionExt};
pub use testing::{run_contract, OutboxContractSeeder};
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
        let _ = std::any::type_name::<InMemoryOutbox>();
        let _ = std::any::type_name::<OutboxDispatcher>();
        let _ = std::any::type_name::<OutboxOptions>();
        let _ = std::any::type_name::<OutboxEntry>();
        let _ = std::any::type_name::<OutboxRecord>();
        fn assert_trait<T: ?Sized>() {}
        assert_trait::<dyn IOutbox>();
        fn assert_host_ext<T: OutboxHostBuilderExt>() {}
        assert_host_ext::<webx_host::server::HostBuilder>();
        fn assert_collection_ext<T: OutboxServiceCollectionExt>() {}
        assert_collection_ext::<rust_dix::ServiceCollection>();
    }
}

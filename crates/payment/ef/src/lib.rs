//! Durable payment webhook inbox for `rust-webx-pay`, backed by rust-ef.
//!
//! Payment callbacks are delivered **at least once** and may arrive out of
//! order. The reliable shape is "persist first, acknowledge, process later":
//!
//! 1. the webhook handler verifies the provider signature (core crate), then
//!    calls [`IPaymentInbox::record`] — the unique `(provider, dedupe_key)`
//!    index makes duplicate deliveries harmless;
//! 2. the provider callback is acknowledged as soon as the event is durable;
//! 3. [`PaymentInboxWorker`] (a framework
//!    [`PeriodicBackgroundService`](webx_core::hosted::PeriodicBackgroundService))
//!    leases due rows, decodes the [`PaymentEvent`](webx_pay::PaymentEvent) and
//!    calls the application's
//!    [`IPaymentEventHandler`]; failures back off exponentially and are
//!    dead-lettered after the configured attempt count.
//!
//! # Wiring
//!
//! ```ignore
//! use std::sync::Arc;
//! use rust_ef::di::DbContextServiceCollectionExt as _;
//! use rust_ef_sqlite::DbContextOptionsBuilderExt as _;
//! use webx_host::server::Host;
//! use webx_pay::PaymentEvent;
//! use webx_pay_ef::{
//!     IPaymentEventHandler, PaymentInboxHostBuilderExt, PaymentInboxOptions,
//! };
//!
//! struct OrderHandler; // application-owned
//!
//! #[async_trait::async_trait]
//! impl IPaymentEventHandler for OrderHandler {
//!     async fn handle(&self, event: PaymentEvent) -> Result<(), webx_pay::PaymentError> {
//!         // idempotent business processing keyed by event.dedupe_key
//!         Ok(())
//!     }
//! }
//!
//! let host = Host::builder()
//!     .register(|services| {
//!         services.add_dbcontext(|options| {
//!             options.use_sqlite("app.db");
//!         })
//!     })
//!     .add_payment_inbox_with_handler(
//!         PaymentInboxOptions::default(),
//!         Arc::new(OrderHandler),
//!     )
//!     .build();
//! ```
//!
//! The schema is owned by the official rust-ef migrations (`rust-ef migration
//! add` / `apply`), not by runtime code. In tests only, create the schema with
//! [`DbContext::bootstrap`](rust_ef::db_context::DbContext::bootstrap); note the
//! unique index is created by `bootstrap`/migrations, not by `ensure_created`.
//!
//! # Scope rules
//!
//! * The application registers the rust-ef `DbContext`; this crate never opens a
//!   second connection stack and never runs DDL.
//! * No routes, no second web/DI/ORM stack: the webhook route and the
//!   provider-specific acknowledgement stay in the application/core crate.
//! * Identity/tenant is not part of the inbox: events are system callbacks, and
//!   business ownership is the handler's concern.

mod ef_inbox;
mod entity;
mod options;
mod port;
mod register;
mod worker;

#[cfg(test)]
mod test_support;

pub use ef_inbox::EfPaymentInbox;
pub use entity::{PaymentWebhookInbox, PaymentWebhookInboxConfiguration};
pub use options::PaymentInboxOptions;
pub use port::{IPaymentEventHandler, IPaymentInbox, InboxEntry, RecordOutcome};
pub use register::{PaymentInboxHostBuilderExt, PaymentInboxServiceCollectionExt};
pub use worker::PaymentInboxWorker;

/// Current Unix time in whole seconds.
pub(crate) fn unix_now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_is_a_recent_unix_timestamp() {
        // 2024-01-01T00:00:00Z; catches a broken clock conversion.
        assert!(unix_now_secs() > 1_704_067_200);
    }

    #[test]
    fn public_types_are_reexported() {
        // Compile-time surface check: these are the application-facing names.
        let _ = std::any::type_name::<EfPaymentInbox>();
        let _ = std::any::type_name::<PaymentInboxOptions>();
        let _ = std::any::type_name::<PaymentInboxWorker>();
        let _ = std::any::type_name::<PaymentWebhookInbox>();
        let _ = std::any::type_name::<PaymentWebhookInboxConfiguration>();
        fn assert_trait<T: ?Sized>() {}
        assert_trait::<dyn IPaymentInbox>();
        assert_trait::<dyn IPaymentEventHandler>();
    }
}

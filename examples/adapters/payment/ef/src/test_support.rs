//! Shared fixtures for the crate's unit tests (rust-ef SQLite in-memory).

#![cfg(test)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rust_dix::{ScopeFactory, ServiceCollection, ServiceProvider};
use rust_ef::prelude::*;
use rust_ef_sqlite::DbContextOptionsBuilderExt as _;
use webx_pay::{AccountId, Money, PaymentError, PaymentEvent, PaymentEventType, ProviderId};

use crate::ef_inbox::EfPaymentInbox;
use crate::entity::PaymentWebhookInbox;
use crate::options::PaymentInboxOptions;
use crate::port::{IPaymentEventHandler, IPaymentInbox};
use crate::register::PaymentInboxServiceCollectionExt as _;

/// A payment event with the given de-duplication key.
pub(crate) fn event(dedupe_key: &str) -> PaymentEvent {
    let mut event = PaymentEvent::new(ProviderId::Stripe, PaymentEventType::Succeeded, dedupe_key);
    event.account = Some(AccountId::new("acct_1").expect("account"));
    event.order_id = Some("ORDER-1".to_string());
    event.amount = Some(Money::usd(1234));
    event.occurred_at = 1_700_000_000;
    event
}

/// A fresh SQLite in-memory `DbContextOptions` (one shared connection per
/// options instance, so the schema is visible to every context created from it).
pub(crate) fn in_memory_options() -> Arc<DbContextOptions> {
    let mut builder = DbContextOptionsBuilder::new();
    builder.use_sqlite_in_memory();
    Arc::new(builder.build())
}

/// Create the schema through the official bootstrap path (tests only; the
/// unique de-duplication index is created by `bootstrap`/migrations, not by
/// `ensure_created`).
pub(crate) async fn prepare_schema(options: &Arc<DbContextOptions>) {
    let ctx = DbContext::from_options(options).expect("context");
    ctx.bootstrap().await.expect("bootstrap schema");
}

/// An inbox over an app-style scoped `DbContext` registration, without the
/// worker/handler registrations.
pub(crate) async fn direct_env(
    max_attempts: i32,
) -> (Arc<ServiceProvider>, Arc<dyn IPaymentInbox>) {
    let options = in_memory_options();
    prepare_schema(&options).await;

    let scoped_options = Arc::clone(&options);
    let provider = ServiceCollection::new()
        .scoped::<DbContext>(move |_| {
            Arc::new(DbContext::from_options(&scoped_options).expect("context"))
        })
        .build()
        .expect("provider");
    let services = provider
        .get::<dyn ScopeFactory>()
        .expect("built-in ScopeFactory");
    let inbox: Arc<dyn IPaymentInbox> = Arc::new(EfPaymentInbox::new(services, max_attempts));
    (provider, inbox)
}

/// A provider wired like an application: rust-ef `add_dbcontext` +
/// [`crate::add_payment_inbox`](crate::PaymentInboxServiceCollectionExt::add_payment_inbox).
///
/// The schema is bootstrapped through a scope created from the same provider,
/// so the in-memory database is shared with everything the inbox resolves.
pub(crate) async fn registered_env(
    options: PaymentInboxOptions,
    handler: Option<Arc<dyn IPaymentEventHandler>>,
) -> Arc<ServiceProvider> {
    let services = ServiceCollection::new().add_dbcontext(|builder| {
        builder.use_sqlite_in_memory();
    });
    let services = match handler {
        Some(handler) => services.add_payment_inbox_with_handler(options, handler),
        None => services.add_payment_inbox(options),
    };
    build_registered(services).await
}

/// Build a host-like provider from `services` and bootstraps the inbox schema
/// through a scope (the in-memory database lives on the provider options, so it
/// is shared by every later scope).
pub(crate) async fn build_registered(services: ServiceCollection) -> Arc<ServiceProvider> {
    let provider = services.build().expect("provider");
    {
        let scope = provider.scope();
        let ctx: DbContext = scope.get_owned().expect("scoped DbContext");
        ctx.bootstrap().await.expect("bootstrap schema");
    }
    provider
}

/// The DI-registration helper without a handler; the test registers the handler
/// itself so the worker resolves it from DI on every tick.
pub(crate) async fn registered_env_with_di_handler(
    options: PaymentInboxOptions,
    handler: Arc<dyn IPaymentEventHandler>,
) -> Arc<ServiceProvider> {
    let services = ServiceCollection::new()
        .add_dbcontext(|builder| {
            builder.use_sqlite_in_memory();
        })
        .add_payment_inbox(options)
        .instance::<dyn IPaymentEventHandler>(handler);
    build_registered(services).await
}

/// Read every inbox row (assertions on persisted state; the test suite stores
/// only a handful of rows, so a bounded read is enough).
pub(crate) async fn rows(provider: &Arc<ServiceProvider>) -> Vec<PaymentWebhookInbox> {
    let scope = provider.scope();
    let mut ctx: DbContext = scope.get_owned().expect("context");
    linq!(
        ctx.set::<PaymentWebhookInbox>(),
        |row: PaymentWebhookInbox| row.created_at >= 0;
        order_by row.created_at
    )
    .take(1_000)
    .to_list()
    .await
    .expect("query rows")
}

/// Read one inbox row by id.
pub(crate) async fn row(provider: &Arc<ServiceProvider>, id: &str) -> PaymentWebhookInbox {
    let scope = provider.scope();
    let mut ctx: DbContext = scope.get_owned().expect("context");
    let id = id.to_string();
    linq!(
        ctx.set::<PaymentWebhookInbox>(),
        |row: PaymentWebhookInbox| row.id == id
    )
    .first_or_default()
    .await
    .expect("query row")
    .expect("row exists")
}

/// Number of persisted inbox rows.
pub(crate) async fn row_count(provider: &Arc<ServiceProvider>) -> i64 {
    let scope = provider.scope();
    let mut ctx: DbContext = scope.get_owned().expect("context");
    linq!(ctx.set::<PaymentWebhookInbox>(); count)
        .await
        .expect("count rows")
}

/// Application handler test double: records events and can be switched to fail.
pub(crate) struct TestHandler {
    seen: Mutex<Vec<PaymentEvent>>,
    fail: AtomicBool,
}

impl TestHandler {
    /// A handler that succeeds.
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            seen: Mutex::new(Vec::new()),
            fail: AtomicBool::new(false),
        })
    }

    /// Make every subsequent `handle` call fail.
    pub(crate) fn fail(&self, fail: bool) {
        self.fail.store(fail, Ordering::Relaxed);
    }

    /// De-duplication keys of the successfully handled events, in order.
    pub(crate) fn handled_keys(&self) -> Vec<String> {
        self.seen
            .lock()
            .expect("handler lock")
            .iter()
            .map(|event| event.dedupe_key.clone())
            .collect()
    }
}

#[async_trait]
impl IPaymentEventHandler for TestHandler {
    async fn handle(&self, event: PaymentEvent) -> Result<(), PaymentError> {
        if self.fail.load(Ordering::Relaxed) {
            return Err(PaymentError::internal("test handler failure"));
        }
        self.seen.lock().expect("handler lock").push(event);
        Ok(())
    }
}

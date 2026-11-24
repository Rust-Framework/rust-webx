//! End-to-end outbox test over in-memory SQLite: same-transaction enqueue,
//! lease-based dispatch, delivered/dead-letter state and duplicate-id handling.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use rust_dix::{ScopeFactory, ServiceCollection};
use rust_ef::db_context::{DbContext, DbContextOptionsBuilder};
use rust_ef::error::EFError;
use rust_ef::prelude::*;
use rust_ef_sqlite::DbContextOptionsBuilderExt as _;
use webx::*;
use webx_core::mediator::IEventRequest;
use webx_outbox_ef::{
    register_outbox_model, OutboxDbContextExt, OutboxDispatcher, OutboxMessage, OutboxOptions,
    OutboxServiceCollectionExt,
};

static DELIVERED: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct OrderPlaced {
    order_id: String,
}

impl IEventRequest for OrderPlaced {
    fn event_name() -> &'static str {
        "order.placed"
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct OrderFailed {
    order_id: String,
}

impl IEventRequest for OrderFailed {
    fn event_name() -> &'static str {
        "order.failed"
    }
}

#[derive(Default)]
struct OrderHandler;

#[handler]
#[async_trait::async_trait]
impl IEventHandler<OrderPlaced> for OrderHandler {
    async fn handle(&self, event: OrderPlaced) -> Result<()> {
        assert_eq!(event.order_id, "order-1");
        DELIVERED.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[derive(Default)]
struct FailingHandler;

#[handler]
#[async_trait::async_trait]
impl IEventHandler<OrderFailed> for FailingHandler {
    async fn handle(&self, _event: OrderFailed) -> Result<()> {
        Err(Error::internal("handler boom".to_string()))
    }
}

async fn in_memory_options() -> Arc<rust_ef::db_context::DbContextOptions> {
    let mut builder = DbContextOptionsBuilder::new();
    builder.use_sqlite_in_memory();
    Arc::new(builder.build())
}

async fn prepare(options: &Arc<rust_ef::db_context::DbContextOptions>) {
    let mut ctx = DbContext::from_options(options).expect("context");
    register_outbox_model(&mut ctx);
    ctx.bootstrap().await.expect("bootstrap schema");
}

async fn provider(
    options: Arc<rust_ef::db_context::DbContextOptions>,
) -> Arc<rust_dix::ServiceProvider> {
    let scoped = Arc::clone(&options);
    ServiceCollection::new()
        .scoped::<DbContext>(move |_| {
            let mut ctx = DbContext::from_options(&scoped).expect("context");
            register_outbox_model(&mut ctx);
            Arc::new(ctx)
        })
        .add_outbox(OutboxOptions::default().disabled())
        .build()
        .expect("provider")
}

async fn own_context(provider: &Arc<rust_dix::ServiceProvider>) -> DbContext {
    provider
        .create_scope()
        .get_owned::<DbContext>()
        .expect("scoped DbContext")
}

async fn rows(provider: &Arc<rust_dix::ServiceProvider>) -> Vec<OutboxMessage> {
    let mut ctx = own_context(provider).await;
    linq!(ctx.set::<OutboxMessage>();)
        .to_list()
        .await
        .expect("rows")
}

async fn tick(provider: &Arc<rust_dix::ServiceProvider>) {
    let services: Arc<dyn ScopeFactory> = provider.clone();
    let dispatcher = OutboxDispatcher::new(
        services,
        Arc::clone(provider),
        OutboxOptions::default().disabled(),
    );
    dispatcher.tick().await.expect("tick");
}

#[tokio::test]
async fn enqueue_in_a_transaction_is_delivered_once() {
    DELIVERED.store(0, Ordering::SeqCst);
    let options = in_memory_options().await;
    prepare(&options).await;
    let provider = provider(options).await;

    let mut ctx = own_context(&provider).await;
    ctx.use_transaction(|ctx| {
        Box::pin(async move {
            ctx.enqueue_outbox(&OrderPlaced {
                order_id: "order-1".to_string(),
            })?;
            ctx.save_changes().await
        })
    })
    .await
    .expect("transaction commits");

    tick(&provider).await;
    assert_eq!(DELIVERED.load(Ordering::SeqCst), 1);

    let rows = rows(&provider).await;
    assert_eq!(rows.len(), 1);
    assert!(rows[0].delivered_at.is_some());
    assert!(rows[0].dead_lettered_at.is_none());

    // A second tick re-claims nothing: delivery is exactly once absent failures.
    tick(&provider).await;
    assert_eq!(DELIVERED.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_failing_handler_is_dead_lettered_after_max_attempts() {
    let options = in_memory_options().await;
    prepare(&options).await;
    let provider = provider(options).await;

    let mut ctx = own_context(&provider).await;
    ctx.use_transaction(|ctx| {
        Box::pin(async move {
            ctx.enqueue_outbox(&OrderFailed {
                order_id: "order-2".to_string(),
            })?;
            ctx.save_changes().await
        })
    })
    .await
    .expect("transaction");

    // `max_attempts` is 6 by default; run enough ticks to reach it.
    for _ in 0..6 {
        tick(&provider).await;
        let rows = rows(&provider).await;
        let row = &rows[0];
        if row.dead_lettered_at.is_some() {
            assert_eq!(row.attempts, 6);
            assert!(row
                .last_error
                .as_deref()
                .is_some_and(|error| error.contains("handler boom")));
            return;
        }
        // Move the retry time forward so the next tick picks the row again.
        let mut ctx = own_context(&provider).await;
        linq!(ctx.set::<OutboxMessage>(), |row: OutboxMessage| row
            .attempts
            > 0)
        .execute_update()
        .set_column(OutboxMessage::COLUMN_NEXT_ATTEMPT_AT, 0i64)
        .set_column(OutboxMessage::COLUMN_LOCKED_UNTIL, 0i64)
        .execute()
        .await
        .expect("make row due");
    }
    panic!("the failing row was never dead-lettered");
}

#[tokio::test]
async fn an_event_without_a_handler_is_dead_lettered_immediately() {
    let options = in_memory_options().await;
    prepare(&options).await;
    let provider = provider(options).await;

    let mut ctx = own_context(&provider).await;
    ctx.use_transaction(|ctx| {
        Box::pin(async move {
            ctx.enqueue_outbox_wire("unknown-1", "unknown.event", "{}")?;
            ctx.save_changes().await
        })
    })
    .await
    .expect("transaction");

    tick(&provider).await;
    let rows = rows(&provider).await;
    assert!(rows[0].dead_lettered_at.is_some());
    assert!(rows[0]
        .last_error
        .as_deref()
        .is_some_and(|error| error.contains("no #[handler] registration")));
}

#[tokio::test]
async fn a_duplicate_id_is_a_unique_violation() {
    let options = in_memory_options().await;
    prepare(&options).await;
    let provider = provider(options).await;

    let mut ctx = own_context(&provider).await;
    ctx.enqueue_outbox_wire("dup-1", "order.placed", "{\"order_id\":\"a\"}")
        .expect("enqueue");
    ctx.save_changes().await.expect("first insert");

    ctx.enqueue_outbox_wire("dup-1", "order.placed", "{\"order_id\":\"b\"}")
        .expect("enqueue");
    let error = ctx.save_changes().await.expect_err("duplicate id");
    assert!(
        matches!(error, EFError::UniqueViolation { .. }),
        "expected a unique violation, got {error}"
    );
}

#[tokio::test]
async fn an_oversized_payload_is_rejected_before_the_insert() {
    let options = in_memory_options().await;
    prepare(&options).await;
    let provider = provider(options).await;

    let mut ctx = own_context(&provider).await;
    let payload = format!("{{\"blob\":\"{}\"}}", "x".repeat(1 << 20));
    let error = ctx
        .enqueue_outbox_wire("big-1", "order.placed", &payload)
        .expect_err("payloads above 1 MiB are rejected");
    assert!(
        matches!(error, EFError::ModelValidation(ref message, _) if message.contains("at most")),
        "expected a model validation error, got {error}"
    );
}

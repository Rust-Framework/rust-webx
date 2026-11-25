//! Dispatcher behaviour over the in-memory store: at-least-once delivery,
//! immediate dead-lettering of unroutable events, and failure/backoff accounting.
//!
//! The event registrations are submitted manually instead of through
//! `#[handler]`: that macro expands to `::webx` (the umbrella crate), and this
//! crate's tests must not depend on it — the umbrella re-exports this crate, so a
//! dev-dependency edge would make the publish order cyclic. `EventRegistration`
//! is the same public registration the macro emits.

use std::any::TypeId;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use rust_dix::{ScopeFactory, ServiceCollection, ServiceProvider};
use webx_core::error::Error;
use webx_core::hosted::PeriodicBackgroundService;
use webx_core::mediator::{BoxedEventDispatch, EventRegistration};
use webx_outbox::{IOutbox, InMemoryOutbox, OutboxDispatcher, OutboxOptions, OutboxRecord};

static DELIVERED: AtomicUsize = AtomicUsize::new(0);

#[derive(serde::Deserialize)]
struct OrderPlaced {
    order_id: String,
}

struct OrderFailed;

fn deliver_order_placed(_provider: Arc<ServiceProvider>, payload: Vec<u8>) -> BoxedEventDispatch {
    Box::pin(async move {
        let event: OrderPlaced =
            serde_json::from_slice(&payload).map_err(|error| Error::internal(error.to_string()))?;
        assert_eq!(event.order_id, "order-1");
        DELIVERED.fetch_add(1, Ordering::SeqCst);
        Ok(())
    })
}

fn fail_order_failed(_provider: Arc<ServiceProvider>, _payload: Vec<u8>) -> BoxedEventDispatch {
    Box::pin(async move { Err(Error::internal("handler boom".to_string())) })
}

inventory::submit! {
    EventRegistration {
        event_name: || "order.placed",
        event_type_id: TypeId::of::<OrderPlaced>(),
        dispatch: deliver_order_placed,
    }
}

inventory::submit! {
    EventRegistration {
        event_name: || "order.failed",
        event_type_id: TypeId::of::<OrderFailed>(),
        dispatch: fail_order_failed,
    }
}

fn dispatcher(store: &Arc<InMemoryOutbox>) -> OutboxDispatcher {
    let provider = ServiceCollection::new()
        .instance::<dyn IOutbox>(Arc::clone(store) as Arc<dyn IOutbox>)
        .build()
        .expect("provider");
    let services: Arc<dyn ScopeFactory> = provider.clone();
    OutboxDispatcher::new(services, provider, OutboxOptions::default().disabled())
}

fn record(store: &Arc<InMemoryOutbox>, id: &str) -> OutboxRecord {
    store.record(id).expect("seeded row")
}

#[tokio::test]
async fn a_registered_handler_delivers_a_row_once() {
    DELIVERED.store(0, Ordering::SeqCst);
    let store = Arc::new(InMemoryOutbox::new(3));
    assert!(store.enqueue("order-1", "order.placed", r#"{"order_id":"order-1"}"#));
    let dispatcher = dispatcher(&store);

    dispatcher.tick().await.expect("tick");
    assert_eq!(DELIVERED.load(Ordering::SeqCst), 1);
    let row = record(&store, "order-1");
    assert!(row.delivered_at.is_some());
    assert!(row.dead_lettered_at.is_none());

    // A second tick re-claims nothing: delivery is exactly once absent failures.
    dispatcher.tick().await.expect("tick");
    assert_eq!(DELIVERED.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn an_event_without_a_handler_is_dead_lettered_immediately() {
    let store = Arc::new(InMemoryOutbox::new(3));
    assert!(store.enqueue("unknown-1", "unknown.event", "{}"));
    let dispatcher = dispatcher(&store);

    dispatcher.tick().await.expect("tick");
    let row = record(&store, "unknown-1");
    assert!(row.dead_lettered_at.is_some());
    assert!(row.delivered_at.is_none());
    assert!(row
        .last_error
        .as_deref()
        .is_some_and(|error| error.contains("no #[handler] registration")));
}

#[tokio::test]
async fn a_failing_handler_records_attempts_and_backs_off() {
    let store = Arc::new(InMemoryOutbox::new(3));
    assert!(store.enqueue("failed-1", "order.failed", "{}"));
    let dispatcher = dispatcher(&store);

    dispatcher.tick().await.expect("tick");
    let row = record(&store, "failed-1");
    assert_eq!(row.attempts, 1);
    assert!(row.delivered_at.is_none());
    assert!(row.dead_lettered_at.is_none());
    assert_eq!(row.locked_until, 0, "a failed row releases its lease");
    assert!(row.next_attempt_at > 0);
    assert!(row
        .last_error
        .as_deref()
        .is_some_and(|error| error.contains("handler boom")));
}

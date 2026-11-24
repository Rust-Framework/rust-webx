//! Event transport: the [`IEventPublisher`] port and the default in-memory
//! transport.
//!
//! `IMediator::publish` does not run event handlers. It encodes the event into
//! an [`EventMessage`] and hands it to the registered [`IEventPublisher`], which
//! returns as soon as it accepted the event. Handler execution therefore cannot
//! delay the publisher or fail its request; a handler error is logged by the
//! transport (or retried / dead-lettered by a durable adapter).
//!
//! The default transport is a bounded in-process queue:
//!
//! ```text
//! publish ──► InMemoryEventPublisher ──► bounded mpsc ──► InMemoryEventWorker
//!                                                        └─► EventDispatcher
//!                                                            └─► IEventHandler<T> (DI + inventory)
//! ```
//!
//! A Redis / NATS / SQS adapter implements [`IEventPublisher`] and forwards the
//! same two fields (`event_name`, `payload`) to the broker. Its consumer loop
//! reconstructs an [`EventMessage`] and calls [`EventDispatcher::dispatch`],
//! which routes the payload to the same `#[handler]`-registered event handlers
//! (the registrations emitted by `#[handler]` / `#[handler(inject)]`). No
//! framework internals are involved.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use futures_util::FutureExt;
use rust_dix::{IServiceResolver, Scope, ServiceProvider};
use tokio::sync::mpsc;

use crate::error::{Error, Result};
use crate::handler::IEventHandler;
use crate::hosted::{BackgroundService, CancellationToken};
use crate::mediator::IEventRequest;

/// Default capacity of the in-memory event queue.
///
/// The queue is bounded so a stalled consumer cannot grow memory without limit;
/// `publish` fails with a retryable 503 when it is full instead of blocking the
/// publisher.
pub const DEFAULT_EVENT_QUEUE_CAPACITY: usize = 1024;

/// A fully encoded event handed to an [`IEventPublisher`].
///
/// `event_name` is [`IEventRequest::event_name`] and `payload` is JSON. Both
/// fields are transport-agnostic: an in-memory queue passes the value through,
/// a broker stores the two fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventMessage {
    /// Stable routing name of the event (see [`IEventRequest::event_name`]).
    pub event_name: String,
    /// JSON encoded event body.
    pub payload: Vec<u8>,
}

impl EventMessage {
    /// Build a message from an already encoded payload (adapter/tests).
    pub fn new(event_name: impl Into<String>, payload: impl Into<Vec<u8>>) -> Self {
        Self {
            event_name: event_name.into(),
            payload: payload.into(),
        }
    }

    /// Encode `event` into its wire form.
    pub fn from_event<T: IEventRequest + serde::Serialize>(event: &T) -> Result<Self> {
        let event_name = T::event_name();
        let payload = serde_json::to_vec(event).map_err(|error| {
            Error::internal(format!("failed to serialize event '{event_name}': {error}"))
        })?;
        Ok(Self {
            event_name: event_name.to_string(),
            payload,
        })
    }

    /// Decode the payload into `T`.
    pub fn decode<T: IEventRequest + serde::de::DeserializeOwned>(&self) -> Result<T> {
        serde_json::from_slice(&self.payload).map_err(|error| {
            Error::internal(format!(
                "event '{}' payload is not valid JSON for {}: {error}",
                self.event_name,
                std::any::type_name::<T>()
            ))
        })
    }

    /// Encode the message for a transport that carries **one binary body**
    /// (NATS, RabbitMQ, Kafka).
    ///
    /// Format: a version byte (`1`), a `u32` big-endian UTF-8 name length, the
    /// name, then the payload bytes. The version byte makes future envelope
    /// fields (trace context, delivery count, ...) an additive wire change.
    /// Redis Streams and the in-memory queue keep the two fields separate, so
    /// they do not need this framing.
    pub fn to_wire(&self) -> Result<Vec<u8>> {
        let name = self.event_name.as_bytes();
        if name.len() > MAX_EVENT_NAME_BYTES {
            return Err(Error::internal(format!(
                "event name is {} bytes; the wire limit is {MAX_EVENT_NAME_BYTES}",
                name.len()
            )));
        }
        let name_len = u32::try_from(name.len())
            .map_err(|_| Error::internal("event name is too long".to_string()))?;
        let mut out = Vec::with_capacity(5 + name.len() + self.payload.len());
        out.push(EVENT_WIRE_VERSION);
        out.extend_from_slice(&name_len.to_be_bytes());
        out.extend_from_slice(name);
        out.extend_from_slice(&self.payload);
        Ok(out)
    }

    /// Decode a message produced by [`EventMessage::to_wire`].
    pub fn from_wire(bytes: &[u8]) -> Result<Self> {
        const HEADER_LEN: usize = 5;
        if bytes.len() < HEADER_LEN {
            return Err(Error::internal(
                "event frame is truncated: missing version/name length".to_string(),
            ));
        }
        if bytes[0] != EVENT_WIRE_VERSION {
            return Err(Error::internal(format!(
                "unsupported event wire version {}",
                bytes[0]
            )));
        }
        let name_len = u32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]) as usize;
        if name_len > MAX_EVENT_NAME_BYTES {
            return Err(Error::internal(format!(
                "event frame names {name_len} bytes; the wire limit is {MAX_EVENT_NAME_BYTES}"
            )));
        }
        let name_end = HEADER_LEN + name_len;
        if bytes.len() < name_end {
            return Err(Error::internal(
                "event frame is truncated: incomplete event name".to_string(),
            ));
        }
        let event_name = std::str::from_utf8(&bytes[HEADER_LEN..name_end])
            .map_err(|error| Error::internal(format!("event name is not UTF-8: {error}")))?
            .to_string();
        Ok(Self {
            event_name,
            payload: bytes[name_end..].to_vec(),
        })
    }
}

/// Version byte of the [`EventMessage::to_wire`] envelope.
pub const EVENT_WIRE_VERSION: u8 = 1;

/// Upper bound for an event name in [`EventMessage::to_wire`].
pub const MAX_EVENT_NAME_BYTES: usize = 64 * 1024;

/// Process-local event counters exported at `/metrics`.
///
/// Registered by `add_mediator()` as a singleton; `None` only when a mediator
/// is constructed manually over a provider that did not call `add_mediator`.
#[derive(Debug, Default)]
pub struct EventMetrics {
    published: AtomicU64,
    publish_failed: AtomicU64,
    dispatch_failed: AtomicU64,
    dropped_no_handler: AtomicU64,
    dead_lettered: AtomicU64,
    consumer_reconnects: AtomicU64,
}

/// Immutable view of [`EventMetrics`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EventMetricsSnapshot {
    /// Events accepted by the transport.
    pub published: u64,
    /// Events rejected before or by the transport.
    pub publish_failed: u64,
    /// Dispatches where at least one handler failed or panicked.
    pub dispatch_failed: u64,
    /// Events with no registered handler (logged no-op).
    pub dropped_no_handler: u64,
    /// Events written to a dead-letter destination by a durable adapter.
    pub dead_lettered: u64,
    /// Broker consumer reconnect attempts.
    pub consumer_reconnects: u64,
}

impl EventMetrics {
    /// Create empty counters.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one event accepted by the transport.
    pub fn record_published(&self) {
        self.published.fetch_add(1, Ordering::Relaxed);
    }

    /// Record one event rejected by encoding or the transport.
    pub fn record_publish_failed(&self) {
        self.publish_failed.fetch_add(1, Ordering::Relaxed);
    }

    /// Record one dispatch with at least one handler failure.
    pub fn record_dispatch_failed(&self) {
        self.dispatch_failed.fetch_add(1, Ordering::Relaxed);
    }

    /// Record one event dropped because it had no handler.
    pub fn record_dropped_no_handler(&self) {
        self.dropped_no_handler.fetch_add(1, Ordering::Relaxed);
    }

    /// Record one event written to a dead-letter destination.
    pub fn record_dead_lettered(&self) {
        self.dead_lettered.fetch_add(1, Ordering::Relaxed);
    }

    /// Record one broker consumer reconnect attempt.
    pub fn record_consumer_reconnect(&self) {
        self.consumer_reconnects.fetch_add(1, Ordering::Relaxed);
    }

    /// Read the current counters.
    pub fn snapshot(&self) -> EventMetricsSnapshot {
        EventMetricsSnapshot {
            published: self.published.load(Ordering::Relaxed),
            publish_failed: self.publish_failed.load(Ordering::Relaxed),
            dispatch_failed: self.dispatch_failed.load(Ordering::Relaxed),
            dropped_no_handler: self.dropped_no_handler.load(Ordering::Relaxed),
            dead_lettered: self.dead_lettered.load(Ordering::Relaxed),
            consumer_reconnects: self.consumer_reconnects.load(Ordering::Relaxed),
        }
    }
}

/// Boxed future returned by an [`EventDispatchFn`].
pub type BoxedEventDispatch = Pin<Box<dyn Future<Output = Result<()>> + Send>>;

/// Decodes one event payload and runs every registered handler for it.
///
/// The provider is passed by value so the returned future owns everything it
/// needs; the function creates its own scope, which is disposed when the future
/// completes.
pub type EventDispatchFn = fn(Arc<ServiceProvider>, Vec<u8>) -> BoxedEventDispatch;

/// Compile-time registration emitted by `#[handler]` on an
/// `impl IEventHandler<T> for Handler` block.
///
/// The registration is what makes an event reachable from a broker consumer:
/// the consumer only has the event name and the JSON payload, so the dispatcher
/// needs a per-event function that turns those two into `IEventHandler<T>`
/// calls.
pub struct EventRegistration {
    /// Stable event name (matches [`IEventRequest::event_name`]).
    ///
    /// Stored as a function so an application override of `event_name` is
    /// honoured even though `inventory` entries are statics.
    pub event_name: fn() -> &'static str,
    /// Concrete event type, for diagnostics and duplicate detection.
    pub event_type_id: TypeId,
    /// Decode + fan-out function generated for this event.
    pub dispatch: EventDispatchFn,
}

inventory::collect!(EventRegistration);

/// Boxed future returned by an [`EventHandlerCall`].
pub type BoxedEventHandlerCall = Pin<Box<dyn Future<Output = Result<()>> + Send>>;

/// Constructs one event handler from a DI resolver.
pub type EventHandlerFactory = fn(&dyn IServiceResolver) -> Result<Arc<dyn Any + Send + Sync>>;

/// Calls one type-erased event handler with one type-erased event.
pub type EventHandlerCall =
    fn(Arc<dyn Any + Send + Sync>, Box<dyn Any + Send>) -> BoxedEventHandlerCall;

/// Compile-time registration emitted by `#[handler]` /
/// `#[handler(inject)]` on an `impl IEventHandler<T>` block.
///
/// This complements [`EventRegistration`] (which routes an event name to a
/// dispatcher): the registration constructs a handler per event, using the
/// same `Default` / `Inject` modes as request handlers. Event handlers are not
/// DI services; they are constructed per event.
pub struct EventHandlerRegistration {
    /// Stable event name (matches [`IEventRequest::event_name`]).
    pub event_name: fn() -> &'static str,
    /// Concrete event type.
    pub event_type_id: TypeId,
    /// Human-readable handler type, for diagnostics.
    pub handler_name: &'static str,
    /// Constructs the handler; may use DI (`#[handler(inject)]`) or `Default`.
    pub factory: EventHandlerFactory,
    /// Invokes the handler after downcasting the type-erased values.
    pub call: EventHandlerCall,
}

inventory::collect!(EventHandlerRegistration);

/// Index of the compile-time [`EventRegistration`] entries, keyed by event name.
#[derive(Clone)]
pub struct EventHandlerCache {
    handlers: HashMap<&'static str, EventDispatchFn>,
}

impl EventHandlerCache {
    /// Build the index from the `#[handler]` registrations in this binary.
    ///
    /// Several handlers for the same event produce several identical
    /// registrations; the first one wins. Two different event types that claim
    /// the same name are a configuration error: the first is kept and the
    /// collision is logged.
    pub fn build() -> Self {
        let mut handlers = HashMap::new();
        let mut types: HashMap<&'static str, TypeId> = HashMap::new();

        for registration in inventory::iter::<EventRegistration> {
            let event_name = (registration.event_name)();
            if let Some(existing) = types.get(event_name) {
                if *existing != registration.event_type_id {
                    tracing::warn!(
                        event = event_name,
                        existing = ?existing,
                        duplicate = ?registration.event_type_id,
                        "two event types share an event_name; keeping the first registration"
                    );
                }
                continue;
            }
            types.insert(event_name, registration.event_type_id);
            handlers.insert(event_name, registration.dispatch);
        }

        Self { handlers }
    }

    /// Whether a local handler exists for `event_name`.
    pub fn contains(&self, event_name: &str) -> bool {
        self.handlers.contains_key(event_name)
    }

    /// Number of distinct events with at least one local registration.
    pub fn len(&self) -> usize {
        self.handlers.len()
    }

    /// Whether no event registrations exist.
    pub fn is_empty(&self) -> bool {
        self.handlers.is_empty()
    }

    fn get(&self, event_name: &str) -> Option<EventDispatchFn> {
        self.handlers.get(event_name).copied()
    }
}

/// Decodes an [`EventMessage`] and fans it out to its handlers.
///
/// Used by the default in-memory worker and by broker consumers. Each event gets
/// a fresh DI scope, so Scoped dependencies (e.g. a DbContext) are isolated and
/// released after the event.
#[derive(Clone)]
pub struct EventDispatcher {
    provider: Arc<ServiceProvider>,
    handlers: Arc<EventHandlerCache>,
}

impl EventDispatcher {
    /// Build a dispatcher from this process's compile-time event registrations.
    pub fn new(provider: Arc<ServiceProvider>) -> Self {
        Self::with_cache(provider, Arc::new(EventHandlerCache::build()))
    }

    /// Build a dispatcher with an explicit event index.
    pub fn with_cache(provider: Arc<ServiceProvider>, handlers: Arc<EventHandlerCache>) -> Self {
        Self { provider, handlers }
    }

    /// Process-level event counters, when `add_mediator()` registered them.
    pub fn metrics(&self) -> Option<Arc<EventMetrics>> {
        self.provider.get::<EventMetrics>().ok()
    }

    /// Whether a local handler is registered for `event_name`.
    pub fn has_event(&self, event_name: &str) -> bool {
        self.handlers.contains(event_name)
    }

    /// Deliver one event to all registered handlers.
    ///
    /// Runs every remaining handler even when one fails, then returns an error
    /// summarizing the failures so the transport can log or retry it; the
    /// publisher never sees this result.
    ///
    /// A **permanent** condition — no `#[handler]` registration for the name,
    /// or no `IEventHandler<T>` in DI — is a logged no-op (`Ok(())`), not an
    /// error: an at-least-once transport can ack/advance instead of requeueing
    /// a message that can never succeed.
    pub async fn dispatch(&self, message: &EventMessage) -> Result<()> {
        let Some(dispatch) = self.handlers.get(&message.event_name) else {
            // No registration is permanent: retrying it can never succeed.
            // Treat it as a logged no-op so a broker consumer can ack/advance
            // instead of requeueing the message forever.
            tracing::warn!(
                event = %message.event_name,
                "event has no #[handler] registration; dropping it"
            );
            return Ok(());
        };

        (dispatch)(Arc::clone(&self.provider), message.payload.clone()).await
    }
}

/// Publish-side port for events.
///
/// Implement this trait to replace the default in-memory queue with Redis,
/// NATS, RabbitMQ, SQS, ... A publisher must return once the transport accepted
/// the event. It **must not** wait for event handlers and **must not** surface
/// their errors: the caller already committed its own work, and a slow or
/// failing subscriber is the transport's concern.
///
/// Register the implementation as a **singleton** (the host resolves the port
/// from the root provider). `HostBuilder::use_event_publisher(...)` does this
/// for you.
#[async_trait::async_trait]
pub trait IEventPublisher: Send + Sync {
    /// Hand `message` to the transport.
    async fn publish(&self, message: EventMessage) -> Result<()>;
}

/// Default in-memory transport: a bounded queue drained by
/// [`InMemoryEventWorker`].
///
/// `publish` only enqueues (`try_send`), so a slow handler never delays the
/// publisher. When the queue is full it returns a retryable 503 instead of
/// blocking or growing memory. This transport is at-most-once: a process crash
/// loses queued events. Use an outbox or a durable broker when delivery must
/// survive a crash.
pub struct InMemoryEventPublisher {
    sender: mpsc::Sender<EventMessage>,
    local_handlers: Arc<EventHandlerCache>,
}

impl InMemoryEventPublisher {
    /// Create the publisher and its worker over `provider`.
    ///
    /// The worker is not started here; register it as a
    /// [`BackgroundService`] (which `add_mediator()` does for the host) or run
    /// it with [`BackgroundService::execute`].
    pub fn new(
        provider: Arc<ServiceProvider>,
        capacity: usize,
    ) -> (Arc<Self>, InMemoryEventWorker) {
        let (sender, receiver) = mpsc::channel(capacity.max(1));
        let local_handlers = Arc::new(EventHandlerCache::build());
        let publisher = Arc::new(Self {
            sender,
            local_handlers,
        });
        let worker = InMemoryEventWorker::new(receiver, provider);
        (publisher, worker)
    }

    pub(crate) fn from_parts(
        sender: mpsc::Sender<EventMessage>,
        local_handlers: Arc<EventHandlerCache>,
    ) -> Self {
        Self {
            sender,
            local_handlers,
        }
    }

    /// Queue capacity.
    pub fn capacity(&self) -> usize {
        self.sender.max_capacity()
    }
}

#[async_trait::async_trait]
impl IEventPublisher for InMemoryEventPublisher {
    async fn publish(&self, message: EventMessage) -> Result<()> {
        // The default transport is local: an event with no local registration
        // would be silently dropped, which is exactly the failure mode this
        // design is meant to make visible.
        if !self.local_handlers.contains(&message.event_name) {
            return Err(Error::di(format!(
                "no #[handler] event registration for '{}': the in-memory transport \
                 has no local handler. Add #[handler] to the event-handler impl or \
                 register another IEventPublisher",
                message.event_name
            )));
        }

        self.sender.try_send(message).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => Error::status(
                503,
                "event queue is full; the event was not accepted".to_string(),
            ),
            mpsc::error::TrySendError::Closed(_) => {
                Error::state("event queue is closed".to_string())
            }
        })
    }
}

/// Drains the default in-memory queue and dispatches each event.
///
/// Registered as a [`BackgroundService`] by `add_mediator()`. One event is
/// processed at a time (bounded memory, predictable ordering); a slow handler
/// delays later events but never the publisher. Handler failures and panics are
/// caught per handler (see [`dispatch_event_handlers`]) and logged here, so one
/// bad event cannot kill the worker.
pub struct InMemoryEventWorker {
    receiver: tokio::sync::Mutex<mpsc::Receiver<EventMessage>>,
    dispatcher: EventDispatcher,
}

impl InMemoryEventWorker {
    /// Create a worker over an existing queue receiver.
    pub fn new(receiver: mpsc::Receiver<EventMessage>, provider: Arc<ServiceProvider>) -> Self {
        Self::with_dispatcher(receiver, EventDispatcher::new(provider))
    }

    /// Create a worker with an explicit dispatcher (tests/adapters).
    pub fn with_dispatcher(
        receiver: mpsc::Receiver<EventMessage>,
        dispatcher: EventDispatcher,
    ) -> Self {
        Self {
            receiver: tokio::sync::Mutex::new(receiver),
            dispatcher,
        }
    }
}

#[async_trait::async_trait]
impl BackgroundService for InMemoryEventWorker {
    async fn execute(&self, ct: CancellationToken) -> Result<()> {
        let mut receiver = self.receiver.lock().await;
        loop {
            tokio::select! {
                _ = ct.cancelled() => break,
                message = receiver.recv() => {
                    let Some(message) = message else { break };
                    if let Err(error) = self.dispatcher.dispatch(&message).await {
                        tracing::error!(
                            event = %message.event_name,
                            error = %error,
                            "event delivery failed"
                        );
                    }
                }
            }
        }
        Ok(())
    }
}

/// Run every registered `IEventHandler<T>` for `event` inside `scope`.
///
/// This is the generic body behind the per-event dispatch function emitted by
/// `#[handler]`. Handlers are isolated from each other: a failing handler does
/// not stop the remaining handlers, and a panicking handler is caught and
/// reported as an error instead of unwinding the worker.
pub async fn dispatch_event_handlers<T: IEventRequest>(scope: &Scope, event: T) -> Result<()> {
    let metrics = scope.get::<EventMetrics>().ok();
    let inventory_handlers: Vec<&'static EventHandlerRegistration> =
        inventory::iter::<EventHandlerRegistration>
            .into_iter()
            .filter(|registration| registration.event_type_id == TypeId::of::<T>())
            .collect();

    let mut failures = Vec::new();
    if inventory_handlers.is_empty() {
        // Compatibility path for framework handlers whose routing registration
        // is written by hand (for example the realtime fan-out sink) and for
        // tests built before `#[handler]` emitted a constructor. The model is
        // still singular: if an event has inventory handlers, DI is not
        // consulted at all, so auto and manual registrations cannot double-run.
        let handlers: Vec<Arc<dyn IEventHandler<T>>> = scope.get_all::<dyn IEventHandler<T>>();
        if handlers.is_empty() {
            // An event with no subscriber is a no-op, not a handler failure.
            // Returning an error here would make at-least-once transports
            // requeue it forever.
            if let Some(metrics) = &metrics {
                metrics.record_dropped_no_handler();
            }
            tracing::warn!(
                event = T::event_name(),
                "event has no registered IEventHandler; dropping it"
            );
            return Ok(());
        }
        for handler in handlers {
            match AssertUnwindSafe(handler.handle(event.clone()))
                .catch_unwind()
                .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => failures.push(error.to_string()),
                Err(payload) => failures.push(format!(
                    "handler panicked: {}",
                    panic_message(payload.as_ref())
                )),
            }
        }
    } else {
        let resolver: &dyn IServiceResolver = scope;
        for registration in inventory_handlers {
            // Construction is user code too (`Default` / `Inject`): isolate a
            // panic instead of letting it unwind the transport worker.
            let handler = match std::panic::catch_unwind(AssertUnwindSafe(|| {
                (registration.factory)(resolver)
            })) {
                Ok(Ok(handler)) => handler,
                Ok(Err(error)) => {
                    failures.push(format!(
                        "{} construction failed: {error}",
                        registration.handler_name
                    ));
                    continue;
                }
                Err(payload) => {
                    failures.push(format!(
                        "{} construction panicked: {}",
                        registration.handler_name,
                        panic_message(payload.as_ref())
                    ));
                    continue;
                }
            };
            let event: Box<dyn Any + Send> = Box::new(event.clone());
            match AssertUnwindSafe((registration.call)(handler, event))
                .catch_unwind()
                .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => failures.push(format!("{}: {error}", registration.handler_name)),
                Err(payload) => failures.push(format!(
                    "{} panicked: {}",
                    registration.handler_name,
                    panic_message(payload.as_ref())
                )),
            }
        }
    }

    if !failures.is_empty() {
        if let Some(metrics) = &metrics {
            metrics.record_dispatch_failed();
        }
    }

    match failures.len() {
        0 => Ok(()),
        1 => Err(Error::internal(format!(
            "event '{}' handler failed: {}",
            T::event_name(),
            failures[0]
        ))),
        count => Err(Error::internal(format!(
            "event '{}': {count} handlers failed: {}",
            T::event_name(),
            failures.join("; ")
        ))),
    }
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::time::{Duration, Instant};

    use rust_dix::{ScopeFactory, ServiceCollection};

    use crate::mediator::IMediator;

    #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct TestEvent {
        value: u32,
    }

    impl IEventRequest for TestEvent {}

    fn dispatch_test_event(provider: Arc<ServiceProvider>, payload: Vec<u8>) -> BoxedEventDispatch {
        Box::pin(async move {
            let event: TestEvent = serde_json::from_slice(&payload).map_err(|error| {
                Error::internal(format!("test event payload is invalid: {error}"))
            })?;
            let scope = provider.create_scope();
            dispatch_event_handlers(&scope, event).await
        })
    }

    inventory::submit! {
        EventRegistration {
            event_name: <TestEvent as IEventRequest>::event_name,
            event_type_id: TypeId::of::<TestEvent>(),
            dispatch: dispatch_test_event,
        }
    }

    struct Counting(Arc<AtomicU32>);

    #[async_trait::async_trait]
    impl IEventHandler<TestEvent> for Counting {
        async fn handle(&self, event: TestEvent) -> Result<()> {
            self.0.fetch_add(event.value, Ordering::SeqCst);
            Ok(())
        }
    }

    struct Failing;

    #[async_trait::async_trait]
    impl IEventHandler<TestEvent> for Failing {
        async fn handle(&self, _event: TestEvent) -> Result<()> {
            Err(Error::internal("handler failure".to_string()))
        }
    }

    struct Panicking;

    #[async_trait::async_trait]
    impl IEventHandler<TestEvent> for Panicking {
        async fn handle(&self, _event: TestEvent) -> Result<()> {
            panic!("handler exploded");
        }
    }

    struct Slow {
        started: Arc<tokio::sync::Notify>,
        finished: Arc<AtomicBool>,
    }

    #[async_trait::async_trait]
    impl IEventHandler<TestEvent> for Slow {
        async fn handle(&self, _event: TestEvent) -> Result<()> {
            self.started.notify_waiters();
            tokio::time::sleep(Duration::from_millis(200)).await;
            self.finished.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    fn provider_with(
        handlers: impl FnOnce(ServiceCollection) -> ServiceCollection,
    ) -> Arc<ServiceProvider> {
        handlers(ServiceCollection::new())
            .build()
            .expect("test provider must build")
    }

    #[test]
    fn event_message_round_trips_json() {
        let event = TestEvent { value: 42 };
        let message = EventMessage::from_event(&event).expect("encode");
        assert_eq!(
            message.event_name,
            <TestEvent as IEventRequest>::event_name()
        );
        assert_eq!(message.decode::<TestEvent>().expect("decode"), event);
    }

    #[test]
    fn wire_framing_round_trips_binary_payloads() {
        let message = EventMessage::new("user.created", vec![0, 1, 2, 255]);
        let wire = message.to_wire().expect("encode wire");
        let decoded = EventMessage::from_wire(&wire).expect("decode wire");
        assert_eq!(decoded, message);
    }

    #[test]
    fn wire_framing_rejects_truncated_or_oversized_frames() {
        assert!(EventMessage::from_wire(&[]).is_err());
        assert!(EventMessage::from_wire(&[EVENT_WIRE_VERSION, 0, 0, 0]).is_err());
        assert!(EventMessage::from_wire(&[EVENT_WIRE_VERSION, 0, 0, 1, 0, b'x']).is_err());

        let too_long = (MAX_EVENT_NAME_BYTES + 1) as u32;
        let mut frame = vec![EVENT_WIRE_VERSION];
        frame.extend_from_slice(&too_long.to_be_bytes());
        frame.resize(5 + too_long as usize, b'x');
        assert!(EventMessage::from_wire(&frame).is_err());
    }

    #[test]
    fn an_unknown_wire_version_is_rejected_instead_of_misparsed() {
        let message = EventMessage::new("user.created", Vec::new());
        let mut wire = message.to_wire().expect("encode wire");
        wire[0] = EVENT_WIRE_VERSION + 1;
        assert!(EventMessage::from_wire(&wire).is_err());
    }

    #[tokio::test]
    async fn dispatch_without_a_registration_is_a_logged_no_op() {
        let dispatcher = EventDispatcher::new(provider_with(|services| services));
        let message = EventMessage::new("unregistered::event", b"{}".to_vec());
        assert!(
            dispatcher.dispatch(&message).await.is_ok(),
            "a permanently unknown event must not look like a transient handler failure"
        );
    }

    #[tokio::test]
    async fn dispatch_without_a_di_handler_is_a_logged_no_op() {
        // The event registration exists, but no IEventHandler was registered.
        let dispatcher = EventDispatcher::new(provider_with(|services| services));
        let message = EventMessage::from_event(&TestEvent { value: 1 }).expect("encode");
        assert!(
            dispatcher.dispatch(&message).await.is_ok(),
            "an event with no subscriber is a no-op, not a handler failure"
        );
    }

    #[tokio::test]
    async fn dispatch_runs_every_handler_and_isolates_failures() {
        let counted = Arc::new(AtomicU32::new(0));
        let counted_for_factory = Arc::clone(&counted);
        let provider = provider_with(move |svc| {
            svc.singleton::<dyn IEventHandler<TestEvent>>(move |_| {
                Arc::new(Counting(Arc::clone(&counted_for_factory)))
            })
            .singleton::<dyn IEventHandler<TestEvent>>(|_| Arc::new(Failing))
        });

        let dispatcher = EventDispatcher::new(provider);
        let message = EventMessage::from_event(&TestEvent { value: 7 }).expect("encode");
        let error = dispatcher
            .dispatch(&message)
            .await
            .expect_err("the failing handler must be reported");

        assert!(error.to_string().contains("handler failure"), "{error}");
        assert_eq!(
            counted.load(Ordering::SeqCst),
            7,
            "the remaining handler must still run"
        );
    }

    #[tokio::test]
    async fn a_panicking_handler_becomes_an_error_instead_of_unwinding() {
        let counted = Arc::new(AtomicU32::new(0));
        let counted_for_factory = Arc::clone(&counted);
        let provider = provider_with(move |svc| {
            svc.singleton::<dyn IEventHandler<TestEvent>>(|_| Arc::new(Panicking))
                .singleton::<dyn IEventHandler<TestEvent>>(move |_| {
                    Arc::new(Counting(Arc::clone(&counted_for_factory)))
                })
        });

        let dispatcher = EventDispatcher::new(provider);
        let message = EventMessage::from_event(&TestEvent { value: 3 }).expect("encode");
        let error = dispatcher
            .dispatch(&message)
            .await
            .expect_err("a panicking handler must be reported");
        assert!(error.to_string().contains("handler panicked"), "{error}");
        assert_eq!(counted.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn in_memory_publish_returns_before_the_handler_finishes() {
        let started = Arc::new(tokio::sync::Notify::new());
        let finished = Arc::new(AtomicBool::new(false));
        let started_for_factory = Arc::clone(&started);
        let finished_for_factory = Arc::clone(&finished);
        let provider = provider_with(move |svc| {
            svc.singleton::<dyn IEventHandler<TestEvent>>(move |_| {
                Arc::new(Slow {
                    started: Arc::clone(&started_for_factory),
                    finished: Arc::clone(&finished_for_factory),
                })
            })
        });

        let (publisher, worker) = InMemoryEventPublisher::new(Arc::clone(&provider), 8);
        let ct = CancellationToken::new();
        let worker_ct = ct.clone();
        let task = tokio::spawn(async move { worker.execute(worker_ct).await });

        let started_at = Instant::now();
        publisher
            .publish(EventMessage::from_event(&TestEvent { value: 1 }).expect("encode"))
            .await
            .expect("publish must be accepted");
        assert!(
            started_at.elapsed() < Duration::from_millis(100),
            "publish waited for the handler: {:?}",
            started_at.elapsed()
        );
        assert!(
            !finished.load(Ordering::SeqCst),
            "publish must not run the handler inline"
        );

        tokio::time::timeout(Duration::from_secs(2), async {
            while !finished.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the worker must eventually run the handler");

        ct.cancel();
        task.await.expect("worker task").expect("worker stop");
    }

    struct AcceptingPublisher;

    #[async_trait::async_trait]
    impl IEventPublisher for AcceptingPublisher {
        async fn publish(&self, _message: EventMessage) -> Result<()> {
            Ok(())
        }
    }

    struct RejectingPublisher;

    #[async_trait::async_trait]
    impl IEventPublisher for RejectingPublisher {
        async fn publish(&self, _message: EventMessage) -> Result<()> {
            Err(Error::status(503, "broker unavailable".to_string()))
        }
    }

    #[tokio::test]
    async fn event_metrics_are_recorded_for_publish_and_dispatch() {
        let metrics = Arc::new(EventMetrics::new());
        let provider = ServiceCollection::new()
            .instance::<EventMetrics>(Arc::clone(&metrics))
            .instance::<dyn IEventPublisher>(Arc::new(AcceptingPublisher))
            .build()
            .expect("provider");
        let mediator = crate::mediator::Mediator::new(
            Arc::clone(&provider),
            Arc::new(crate::route::scan::HandlerCache::build()),
        );
        mediator
            .publish(TestEvent { value: 1 })
            .await
            .expect("publish");
        assert_eq!(metrics.snapshot().published, 1);

        let failing = ServiceCollection::new()
            .instance::<EventMetrics>(Arc::clone(&metrics))
            .singleton::<dyn IEventHandler<TestEvent>>(|_| Arc::new(Failing))
            .build()
            .expect("provider");
        let scope = failing.create_scope();
        assert!(dispatch_event_handlers(&scope, TestEvent { value: 2 })
            .await
            .is_err());
        assert_eq!(metrics.snapshot().dispatch_failed, 1);

        let empty = ServiceCollection::new()
            .instance::<EventMetrics>(Arc::clone(&metrics))
            .build()
            .expect("provider");
        let scope = empty.create_scope();
        dispatch_event_handlers(&scope, TestEvent { value: 3 })
            .await
            .expect("no handler is a no-op");
        assert_eq!(metrics.snapshot().dropped_no_handler, 1);

        let rejecting = ServiceCollection::new()
            .instance::<EventMetrics>(Arc::clone(&metrics))
            .instance::<dyn IEventPublisher>(Arc::new(RejectingPublisher))
            .build()
            .expect("provider");
        let mediator = crate::mediator::Mediator::new(
            Arc::clone(&rejecting),
            Arc::new(crate::route::scan::HandlerCache::build()),
        );
        assert!(mediator.publish(TestEvent { value: 4 }).await.is_err());
        assert_eq!(metrics.snapshot().publish_failed, 1);
    }

    #[tokio::test]
    async fn a_full_queue_reports_a_retryable_error_instead_of_blocking() {
        let provider = provider_with(|svc| {
            svc.singleton::<dyn IEventHandler<TestEvent>>(|_| {
                Arc::new(Counting(Arc::new(AtomicU32::new(0))))
            })
        });
        // Keep the worker (and therefore the receiver) alive but do not run it.
        let (publisher, _worker) = InMemoryEventPublisher::new(Arc::clone(&provider), 1);

        publisher
            .publish(EventMessage::from_event(&TestEvent { value: 1 }).expect("encode"))
            .await
            .expect("first publish");
        let error = publisher
            .publish(EventMessage::from_event(&TestEvent { value: 2 }).expect("encode"))
            .await
            .expect_err("the bounded queue must reject instead of blocking");

        assert_eq!(error.status_code(), 503);
        assert!(error.retryable());
    }
}

//! Live Redis integration tests for the distributed cache and event transport.
//!
//! Set `WEBX_REDIS_URL` (for example `redis://127.0.0.1:6379`) to run them.
//! Without a broker the tests return early and pass.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use std::time::Instant;

use rust_dix::{ScopeFactory, ServiceCollection, ServiceProvider};
use webx_core::cache::{CacheError, DistributedCacheEntryOptions, IDistributedCache};
use webx_core::error::Error;
use webx_core::handler::IEventHandler;
use webx_core::hosted::{BackgroundService, CancellationToken};
use webx_core::mediator::{
    dispatch_event_handlers, BoxedEventDispatch, EventDispatcher, EventMessage, EventRegistration,
    IEventPublisher, IEventRequest,
};
use webx_core::realtime::{
    IOnlinePresence, IRealtimeEventStore, PresenceMember, PresenceScope, ResumeRequest,
};
use webx_redis::{
    RedisAdvisoryLock, RedisCache, RedisConnection, RedisEventConsumer, RedisEventPublisher,
    RedisEventsOptions, RedisOnlinePresence, RedisOnlinePresenceOptions, RedisOptions,
    RedisRealtimeEventStore, RedisRealtimeEventStoreOptions,
};

use webx_contract_tests::event_transport::EventTransportContract;
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct TestEvent {
    value: u32,
}

impl IEventRequest for TestEvent {}

/// Manual equivalent of the `#[handler]`-generated registration: adapters do
/// not depend on the `webx` umbrella crate, so the inventory entry is written
/// by hand here.
fn dispatch_test_event(provider: Arc<ServiceProvider>, payload: Vec<u8>) -> BoxedEventDispatch {
    Box::pin(async move {
        let message = EventMessage::new(<TestEvent as IEventRequest>::event_name(), payload);
        let event = message.decode::<TestEvent>()?;
        let scope = <ServiceProvider as ScopeFactory>::create_scope(provider.as_ref());
        dispatch_event_handlers(&scope, event).await
    })
}

inventory::submit! {
    EventRegistration {
        event_name: <TestEvent as IEventRequest>::event_name,
        event_type_id: std::any::TypeId::of::<TestEvent>(),
        dispatch: dispatch_test_event,
    }
}

struct CountingHandler {
    counted: Arc<AtomicU32>,
}

#[async_trait::async_trait]
impl IEventHandler<TestEvent> for CountingHandler {
    async fn handle(&self, event: TestEvent) -> webx_core::error::Result<()> {
        self.counted.fetch_add(event.value, Ordering::SeqCst);
        Ok(())
    }
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_test_writer()
        .try_init();
}

fn unique_suffix() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    format!("{}-{nanos}", std::process::id())
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct FailingEvent {
    value: u32,
}

impl IEventRequest for FailingEvent {}

fn dispatch_failing_event(provider: Arc<ServiceProvider>, payload: Vec<u8>) -> BoxedEventDispatch {
    Box::pin(async move {
        let message = EventMessage::new(<FailingEvent as IEventRequest>::event_name(), payload);
        let event = message.decode::<FailingEvent>()?;
        let scope = <ServiceProvider as ScopeFactory>::create_scope(provider.as_ref());
        dispatch_event_handlers(&scope, event).await
    })
}

inventory::submit! {
    EventRegistration {
        event_name: <FailingEvent as IEventRequest>::event_name,
        event_type_id: std::any::TypeId::of::<FailingEvent>(),
        dispatch: dispatch_failing_event,
    }
}

struct AlwaysFailingHandler;

#[async_trait::async_trait]
impl IEventHandler<FailingEvent> for AlwaysFailingHandler {
    async fn handle(&self, _event: FailingEvent) -> webx_core::error::Result<()> {
        Err(Error::internal("always fails"))
    }
}

/// Broadcast-class event used to prove the Pub/Sub fan-out topology.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct BroadcastEvent {
    value: u32,
}

impl IEventRequest for BroadcastEvent {}

fn dispatch_broadcast_event(
    provider: Arc<ServiceProvider>,
    payload: Vec<u8>,
) -> BoxedEventDispatch {
    Box::pin(async move {
        let message = EventMessage::new(<BroadcastEvent as IEventRequest>::event_name(), payload);
        let event = message.decode::<BroadcastEvent>()?;
        let scope = <ServiceProvider as ScopeFactory>::create_scope(provider.as_ref());
        dispatch_event_handlers(&scope, event).await
    })
}

inventory::submit! {
    EventRegistration {
        event_name: <BroadcastEvent as IEventRequest>::event_name,
        event_type_id: std::any::TypeId::of::<BroadcastEvent>(),
        dispatch: dispatch_broadcast_event,
    }
}

struct BroadcastCountingHandler {
    counted: Arc<AtomicU32>,
}

#[async_trait::async_trait]
impl IEventHandler<BroadcastEvent> for BroadcastCountingHandler {
    async fn handle(&self, event: BroadcastEvent) -> webx_core::error::Result<()> {
        self.counted.fetch_add(event.value, Ordering::SeqCst);
        Ok(())
    }
}

/// Provider that can count both delivery classes independently per consumer.
fn provider_with_delivery_counters(
    competing: Arc<AtomicU32>,
    broadcast: Arc<AtomicU32>,
) -> Arc<ServiceProvider> {
    ServiceCollection::new()
        .singleton::<dyn IEventHandler<TestEvent>>(move |_| {
            Arc::new(CountingHandler {
                counted: Arc::clone(&competing),
            })
        })
        .singleton::<dyn IEventHandler<BroadcastEvent>>(move |_| {
            Arc::new(BroadcastCountingHandler {
                counted: Arc::clone(&broadcast),
            })
        })
        .build()
        .expect("test service provider must build")
}

/// Create the consumer group before any consumer starts, so an event published
/// right after the consumers start is never lost to the "group did not exist
/// yet" race (mirrors `XGROUP CREATE ... 0 MKSTREAM`).
async fn create_consumer_group(connection: &RedisConnection, stream: &str, group: &str) {
    let mut raw = connection
        .manager()
        .await
        .expect("raw connection for XGROUP");
    let result: std::result::Result<String, redis::RedisError> = redis::cmd("XGROUP")
        .arg("CREATE")
        .arg(stream)
        .arg(group)
        .arg("0")
        .arg("MKSTREAM")
        .query_async(&mut raw)
        .await;
    match result {
        Ok(_) => {}
        Err(error) if error.to_string().contains("BUSYGROUP") => {}
        Err(error) => panic!("XGROUP CREATE failed: {error}"),
    }
}

async fn pending_count(
    connection: &mut redis::aio::ConnectionManager,
    stream: &str,
    group: &str,
) -> Option<i64> {
    let value: redis::Value = redis::cmd("XPENDING")
        .arg(stream)
        .arg(group)
        .query_async(connection)
        .await
        .ok()?;
    match value {
        redis::Value::Array(parts) => match parts.first() {
            Some(redis::Value::Int(count)) => Some(*count),
            _ => None,
        },
        _ => None,
    }
}

async fn xlen(connection: &mut redis::aio::ConnectionManager, stream: &str) -> i64 {
    redis::cmd("XLEN")
        .arg(stream)
        .query_async::<i64>(connection)
        .await
        .unwrap_or(0)
}

async fn delete_streams(connection: &mut redis::aio::ConnectionManager, streams: &[&str]) {
    if streams.is_empty() {
        return;
    }
    let _ = redis::cmd("DEL")
        .arg(streams)
        .query_async::<i64>(connection)
        .await;
}

async fn stop_consumer(
    ct: CancellationToken,
    mut task: tokio::task::JoinHandle<webx_core::error::Result<()>>,
) {
    ct.cancel();
    match tokio::time::timeout(Duration::from_secs(5), &mut task).await {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(error))) => panic!("redis consumer failed: {error}"),
        Ok(Err(error)) => panic!("redis consumer task panicked: {error}"),
        Err(_) => {
            task.abort();
            let _ = task.await;
            panic!("redis consumer did not stop within 5s of cancellation");
        }
    }
}

fn provider_with_counting_handler(counted: Arc<AtomicU32>) -> Arc<ServiceProvider> {
    ServiceCollection::new()
        .singleton::<dyn IEventHandler<TestEvent>>(move |_| {
            Arc::new(CountingHandler {
                counted: Arc::clone(&counted),
            })
        })
        .build()
        .expect("test service provider must build")
}

#[tokio::test]
async fn cache_roundtrip() {
    let Ok(url) = std::env::var("WEBX_REDIS_URL") else {
        return;
    };
    let suffix = unique_suffix();
    let options = RedisOptions::new(url.clone()).with_prefix(format!("webx:test:{suffix}:"));
    let connection = Arc::new(RedisConnection::new(
        url,
        Duration::from_secs(15),
        Duration::from_secs(15),
    ));
    let cache = RedisCache::new(Arc::clone(&connection), &options);
    connection.ping().await.expect("health ping");
    let key = "roundtrip";

    cache
        .set(key, b"hello".to_vec(), None)
        .await
        .expect("cache set");
    assert_eq!(
        cache.get(key).await.expect("cache get"),
        Some(b"hello".to_vec())
    );
    assert!(cache.exists(key).await.expect("cache exists"));

    cache.remove(key).await.expect("cache remove");
    assert_eq!(cache.get(key).await.expect("cache get after remove"), None);
    assert!(!cache.exists(key).await.expect("cache exists after remove"));
}

#[tokio::test]
async fn cache_ttl_sliding_size_legacy_and_clear() {
    let Ok(url) = std::env::var("WEBX_REDIS_URL") else {
        return;
    };
    let suffix = unique_suffix();
    let prefix = format!("webx:test:ttl:{suffix}:");
    let options = RedisOptions::new(url.clone())
        .with_prefix(&prefix)
        .with_default_ttl(Duration::from_secs(2))
        .with_sliding_ttl(Duration::from_secs(1));
    let connection = Arc::new(RedisConnection::new(
        url,
        Duration::from_secs(15),
        Duration::from_secs(15),
    ));
    let cache = RedisCache::new(Arc::clone(&connection), &options);

    // Generous margins: the live suite runs in parallel over a WAN link, so a
    // short TTL can expire between SET and the first GET without a code bug.
    let absolute = DistributedCacheEntryOptions::new()
        .set_absolute_expiration_relative_to_now(Duration::from_secs(10));
    cache
        .set("abs", b"v".to_vec(), Some(&absolute))
        .await
        .expect("absolute set");
    assert_eq!(cache.get("abs").await.expect("get"), Some(b"v".to_vec()));
    tokio::time::sleep(Duration::from_millis(10_200)).await;
    assert_eq!(
        cache.get("abs").await.expect("expired get"),
        None,
        "absolute TTL must expire"
    );
    assert!(
        !cache.exists("abs").await.expect("expired exists"),
        "exists must agree with the expired absolute TTL"
    );

    let sliding =
        DistributedCacheEntryOptions::new().set_sliding_expiration(Duration::from_secs(5));
    cache
        .set("slide", b"v".to_vec(), Some(&sliding))
        .await
        .expect("sliding set");
    tokio::time::sleep(Duration::from_millis(2_000)).await;
    assert!(
        cache
            .get("slide")
            .await
            .expect("first sliding get")
            .is_some(),
        "sliding entry must be alive before the first refresh"
    );
    tokio::time::sleep(Duration::from_millis(2_000)).await;
    assert!(
        cache.get("slide").await.expect("refreshed get").is_some(),
        "a read must refresh the sliding window"
    );
    tokio::time::sleep(Duration::from_millis(5_200)).await;
    assert_eq!(
        cache.get("slide").await.expect("expired sliding get"),
        None,
        "sliding entry must expire after the refreshed window"
    );

    let refresh =
        DistributedCacheEntryOptions::new().set_sliding_expiration(Duration::from_secs(5));
    cache
        .set("refresh", b"v".to_vec(), Some(&refresh))
        .await
        .expect("refresh set");
    tokio::time::sleep(Duration::from_millis(2_000)).await;
    cache.refresh("refresh").await.expect("refresh");
    tokio::time::sleep(Duration::from_millis(3_000)).await;
    assert!(
        cache.get("refresh").await.expect("refresh get").is_some(),
        "refresh() must extend the sliding window"
    );

    let limited = DistributedCacheEntryOptions::new().set_size_limit(5);
    let error = cache
        .set("big", vec![0; 10], Some(&limited))
        .await
        .expect_err("size limit must reject the value");
    assert!(matches!(error, CacheError::Message(_)), "{error}");

    // A value written by another Redis client (no 13-byte envelope) is still
    // readable, and `clear()` only touches this prefix.
    let mut raw = connection.manager().await.expect("raw connection");
    let legacy_key = format!("{prefix}legacy");
    redis::cmd("SET")
        .arg(&legacy_key)
        .arg(b"raw".to_vec())
        .query_async::<()>(&mut raw)
        .await
        .expect("raw set");
    assert_eq!(
        cache.get("legacy").await.expect("legacy get"),
        Some(b"raw".to_vec())
    );
    cache.clear().await.expect("clear");
    assert_eq!(cache.get("legacy").await.expect("legacy after clear"), None);
    assert!(!cache.exists("legacy").await.expect("exists after clear"));
}

#[tokio::test]
async fn failing_event_reaches_dead_letter_and_acks() {
    init_tracing();
    let Ok(url) = std::env::var("WEBX_REDIS_URL") else {
        return;
    };
    let suffix = unique_suffix();
    let source = format!("webx:test:fail:{suffix}");
    let dead_letter = format!("{source}:dead");
    let provider = ServiceCollection::new()
        .singleton::<dyn IEventHandler<FailingEvent>>(|_| Arc::new(AlwaysFailingHandler))
        .build()
        .expect("failing provider");
    let dispatcher = Arc::new(EventDispatcher::new(provider));
    let connection = Arc::new(RedisConnection::new(
        url,
        Duration::from_secs(15),
        Duration::from_secs(15),
    ));
    let group = format!("webx-fail-{suffix}");
    let options = RedisEventsOptions {
        stream: source.clone(),
        group: group.clone(),
        consumer: format!("webx-fail-consumer-{suffix}"),
        start_id: "0".to_string(),
        dead_letter_stream: dead_letter.clone(),
        max_deliveries: 2,
        claim_idle_ms: 300,
        block_ms: 100,
        ..RedisEventsOptions::default()
    };
    let publisher = RedisEventPublisher::new(Arc::clone(&connection), options.clone());
    let consumer = Arc::new(RedisEventConsumer::new(
        Arc::clone(&connection),
        options,
        Arc::clone(&dispatcher),
    ));
    let ct = CancellationToken::new();
    let consumer_ct = ct.clone();
    let task_consumer = Arc::clone(&consumer);
    let task = tokio::spawn(async move { task_consumer.execute(consumer_ct).await });

    publisher
        .publish(EventMessage::from_event(&FailingEvent { value: 1 }).expect("encode"))
        .await
        .expect("publish failing event");

    let mut raw = connection.manager().await.expect("raw connection");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if xlen(&mut raw, &dead_letter).await > 0 {
            break;
        }
        if Instant::now() >= deadline {
            let groups: redis::Value = redis::cmd("XINFO")
                .arg("GROUPS")
                .arg(&source)
                .query_async(&mut raw)
                .await
                .unwrap_or(redis::Value::Nil);
            panic!(
                "the failing event did not reach the dead-letter stream: source_xlen={} dlq_xlen={} pending={:?} groups={groups:?}",
                xlen(&mut raw, &source).await,
                xlen(&mut raw, &dead_letter).await,
                pending_count(&mut raw, &source, &group).await
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let pending_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if pending_count(&mut raw, &source, &group).await == Some(0) {
            break;
        }
        assert!(
            Instant::now() < pending_deadline,
            "the dead-lettered entry was not XACKed"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    stop_consumer(ct, task).await;
    let replayed = consumer
        .replay_dead_letter(10)
        .await
        .expect("dead-letter replay");
    assert_eq!(replayed, 1, "the DLQ entry must be replayed");
    assert_eq!(
        xlen(&mut raw, &dead_letter).await,
        0,
        "the replayed DLQ entry must be removed"
    );
    delete_streams(&mut raw, &[&source, &dead_letter]).await;
    assert!(
        xlen(&mut raw, &dead_letter).await == 0,
        "cleanup must remove the dead-letter stream"
    );
}

#[tokio::test]
async fn unregistered_event_is_acked_not_left_pending() {
    init_tracing();
    let Ok(url) = std::env::var("WEBX_REDIS_URL") else {
        return;
    };
    let suffix = unique_suffix();
    let source = format!("webx:test:unregistered:{suffix}");
    let group = format!("webx-unregistered-{suffix}");
    let provider = ServiceCollection::new().build().expect("provider");
    let dispatcher = Arc::new(EventDispatcher::new(provider));
    let connection = Arc::new(RedisConnection::new(
        url,
        Duration::from_secs(15),
        Duration::from_secs(15),
    ));
    let options = RedisEventsOptions {
        stream: source.clone(),
        group: group.clone(),
        consumer: format!("webx-unregistered-consumer-{suffix}"),
        start_id: "0".to_string(),
        block_ms: 100,
        ..RedisEventsOptions::default()
    };
    let publisher = RedisEventPublisher::new(Arc::clone(&connection), options.clone());
    let consumer =
        RedisEventConsumer::new(Arc::clone(&connection), options, Arc::clone(&dispatcher));
    let ct = CancellationToken::new();
    let consumer_ct = ct.clone();
    let task = tokio::spawn(async move { consumer.execute(consumer_ct).await });

    publisher
        .publish(EventMessage::new("webx.test.unregistered", b"{}".to_vec()))
        .await
        .expect("publish unregistered");

    let mut raw = connection.manager().await.expect("raw connection");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if pending_count(&mut raw, &source, &group).await == Some(0) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "an unregistered event was left pending and would be redelivered forever"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    stop_consumer(ct, task).await;
    delete_streams(&mut raw, &[&source]).await;
}

#[tokio::test]
async fn default_start_id_replays_an_event_published_before_consumption() {
    init_tracing();
    let Ok(url) = std::env::var("WEBX_REDIS_URL") else {
        return;
    };
    let suffix = unique_suffix();
    let source = format!("webx:test:replay:{suffix}");
    let counted = Arc::new(AtomicU32::new(0));
    let provider = provider_with_counting_handler(Arc::clone(&counted));
    let dispatcher = Arc::new(EventDispatcher::new(provider));
    let connection = Arc::new(RedisConnection::new(
        url,
        Duration::from_secs(15),
        Duration::from_secs(15),
    ));
    let group = format!("webx-replay-{suffix}");
    let options = RedisEventsOptions {
        stream: source.clone(),
        group: group.clone(),
        consumer: format!("webx-replay-consumer-{suffix}"),
        block_ms: 100,
        ..RedisEventsOptions::default()
    };
    let publisher = RedisEventPublisher::new(Arc::clone(&connection), options.clone());
    publisher
        .publish(EventMessage::from_event(&TestEvent { value: 3 }).expect("encode"))
        .await
        .expect("publish before consumer");
    let consumer =
        RedisEventConsumer::new(Arc::clone(&connection), options, Arc::clone(&dispatcher));
    let ct = CancellationToken::new();
    let consumer_ct = ct.clone();
    let task = tokio::spawn(async move { consumer.execute(consumer_ct).await });

    let deadline = Instant::now() + Duration::from_secs(5);
    while counted.load(Ordering::SeqCst) == 0 {
        if Instant::now() >= deadline {
            let mut raw = connection.manager().await.expect("raw connection");
            let groups: redis::Value = redis::cmd("XINFO")
                .arg("GROUPS")
                .arg(&source)
                .query_async(&mut raw)
                .await
                .unwrap_or(redis::Value::Nil);
            panic!(
                "the default start_id did not replay the pre-consumer event: xlen={} pending={:?} groups={groups:?}",
                xlen(&mut raw, &source).await,
                pending_count(&mut raw, &source, &group).await
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    stop_consumer(ct, task).await;
    let mut raw = connection.manager().await.expect("raw connection");
    delete_streams(&mut raw, &[&source]).await;
    assert!(counted.load(Ordering::SeqCst) > 0);
}

#[tokio::test]
async fn event_roundtrip() {
    init_tracing();
    let Ok(url) = std::env::var("WEBX_REDIS_URL") else {
        return;
    };
    let suffix = unique_suffix();
    let counted = Arc::new(AtomicU32::new(0));
    let provider = provider_with_counting_handler(Arc::clone(&counted));
    let dispatcher = Arc::new(EventDispatcher::new(provider));

    let connection = Arc::new(RedisConnection::new(
        url,
        Duration::from_secs(15),
        Duration::from_secs(15),
    ));
    let stream = format!("webx:test:events:{suffix}");
    let events = RedisEventsOptions {
        stream: stream.clone(),
        group: format!("webx-test-{suffix}"),
        consumer: format!("webx-test-consumer-{suffix}"),
        consumer_enabled: true,
        block_ms: 250,
        claim_idle_ms: 1_000,
        ..RedisEventsOptions::default()
    };
    let publisher = RedisEventPublisher::new(Arc::clone(&connection), events.clone());
    let consumer =
        RedisEventConsumer::new(Arc::clone(&connection), events, Arc::clone(&dispatcher));

    let ct = CancellationToken::new();
    let consumer_ct = ct.clone();
    let mut task = tokio::spawn(async move { consumer.execute(consumer_ct).await });

    // Re-publish until the handler observes an event: the consumer group is
    // created on its first `XREADGROUP`, so a single early publish may be
    // outside the group's start position.
    let message = EventMessage::from_event(&TestEvent { value: 1 }).expect("encode event");
    let mut publish_error = None;
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        while counted.load(Ordering::SeqCst) == 0 {
            if let Err(error) = publisher.publish(message.clone()).await {
                publish_error = Some(error);
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;

    ct.cancel();
    let consumer_result = tokio::time::timeout(Duration::from_secs(5), &mut task).await;
    // Best-effort cleanup so the live test does not leave a stream behind on a
    // shared Redis instance.
    if let Ok(mut connection) = connection.manager().await {
        let _ = redis::cmd("DEL")
            .arg(&stream)
            .query_async::<i64>(&mut connection)
            .await;
    }
    match consumer_result {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(error))) => panic!("redis consumer failed: {error}"),
        Ok(Err(error)) => panic!("redis consumer task panicked: {error}"),
        Err(_) => {
            task.abort();
            let _ = task.await;
            panic!("redis consumer did not stop within 5s of cancellation");
        }
    }

    if let Some(error) = publish_error {
        panic!("publishing the test event failed: {error}");
    }
    assert!(
        counted.load(Ordering::SeqCst) > 0,
        "the event handler did not receive the published event"
    );
}

#[tokio::test]
async fn event_transport_satisfies_the_shared_contract() {
    init_tracing();
    let Ok(url) = std::env::var("WEBX_REDIS_URL") else {
        return;
    };
    let suffix = unique_suffix();
    let stream = format!("webx:contract:{suffix}");
    let contract = EventTransportContract::new();
    let connection = Arc::new(RedisConnection::new(
        url,
        Duration::from_secs(15),
        Duration::from_secs(15),
    ));
    let events = RedisEventsOptions {
        stream: stream.clone(),
        group: format!("webx-contract-{suffix}"),
        consumer: format!("webx-contract-consumer-{suffix}"),
        consumer_enabled: true,
        block_ms: 250,
        claim_idle_ms: 1_000,
        ..RedisEventsOptions::default()
    };
    let publisher = RedisEventPublisher::new(Arc::clone(&connection), events.clone());
    let consumer: Arc<dyn BackgroundService> = Arc::new(RedisEventConsumer::new(
        Arc::clone(&connection),
        events,
        contract.dispatcher(),
    ));
    let report = contract
        .run(&publisher, consumer, Duration::from_secs(10))
        .await
        .expect("redis event transport contract");
    assert!(report.delivered_count() >= 1);

    // Best-effort cleanup: the live test must not leave a stream behind.
    if let Ok(mut raw) = connection.manager().await {
        let _ = redis::cmd("DEL")
            .arg(&stream)
            .query_async::<i64>(&mut raw)
            .await;
    }
}
// ---------------------------------------------------------------------------
// Delivery classes: Streams (competing) + Pub/Sub (broadcast)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn broadcast_reaches_every_consumer_while_competing_stays_single_copy() {
    init_tracing();
    let Ok(url) = std::env::var("WEBX_REDIS_URL") else {
        return;
    };
    let suffix = unique_suffix();
    let stream = format!("webx:test:broadcast:{suffix}");
    let group = format!("webx-broadcast-{suffix}");
    let connection = Arc::new(RedisConnection::new(
        url,
        Duration::from_secs(15),
        Duration::from_secs(15),
    ));
    create_consumer_group(&connection, &stream, &group).await;

    let competing_a = Arc::new(AtomicU32::new(0));
    let competing_b = Arc::new(AtomicU32::new(0));
    let broadcast_a = Arc::new(AtomicU32::new(0));
    let broadcast_b = Arc::new(AtomicU32::new(0));

    let options = RedisEventsOptions {
        stream: stream.clone(),
        group: group.clone(),
        consumer: format!("consumer-a-{suffix}"),
        start_id: "0".to_string(),
        block_ms: 100,
        claim_idle_ms: 1_000,
        ..RedisEventsOptions::default()
    };
    let publisher = RedisEventPublisher::new(Arc::clone(&connection), options.clone());
    let consumer_a = Arc::new(RedisEventConsumer::new(
        Arc::clone(&connection),
        options.clone(),
        Arc::new(EventDispatcher::new(provider_with_delivery_counters(
            Arc::clone(&competing_a),
            Arc::clone(&broadcast_a),
        ))),
    ));
    let mut options_b = options;
    options_b.consumer = format!("consumer-b-{suffix}");
    let consumer_b = Arc::new(RedisEventConsumer::new(
        Arc::clone(&connection),
        options_b,
        Arc::new(EventDispatcher::new(provider_with_delivery_counters(
            Arc::clone(&competing_b),
            Arc::clone(&broadcast_b),
        ))),
    ));

    let ct = CancellationToken::new();
    let worker_ct = ct.clone();
    let task_a = tokio::spawn(async move { consumer_a.execute(worker_ct).await });
    let worker_ct = ct.clone();
    let task_b = tokio::spawn(async move { consumer_b.execute(worker_ct).await });

    // Broadcast: every consumer instance gets its own copy. PUBLISH to a node
    // that has not subscribed yet is lost, so re-publish until both are ready.
    let broadcast_message =
        EventMessage::from_event(&BroadcastEvent { value: 1 }).expect("encode broadcast event");
    let deadline = Instant::now() + Duration::from_secs(10);
    while broadcast_a.load(Ordering::SeqCst) == 0 || broadcast_b.load(Ordering::SeqCst) == 0 {
        assert!(
            Instant::now() < deadline,
            "every consumer must receive the broadcast event"
        );
        publisher
            .broadcast(broadcast_message.clone())
            .await
            .expect("broadcast publish");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Competing: the consumer group delivers exactly one copy across both
    // instances.
    let competing_message =
        EventMessage::from_event(&TestEvent { value: 1 }).expect("encode competing event");
    publisher
        .publish(competing_message)
        .await
        .expect("competing publish");
    let deadline = Instant::now() + Duration::from_secs(10);
    while competing_a.load(Ordering::SeqCst) + competing_b.load(Ordering::SeqCst) == 0 {
        assert!(
            Instant::now() < deadline,
            "the competing event must be delivered to one consumer"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // Give a duplicate delivery a chance to show up before asserting.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        competing_a.load(Ordering::SeqCst) + competing_b.load(Ordering::SeqCst),
        1,
        "a consumer group must deliver exactly one copy of a competing event"
    );

    stop_consumer(ct.clone(), task_a).await;
    stop_consumer(ct, task_b).await;

    if let Ok(mut raw) = connection.manager().await {
        delete_streams(&mut raw, &[&stream]).await;
    }
}

// ---------------------------------------------------------------------------
// Shared real-time state: replay log + online presence
// ---------------------------------------------------------------------------

#[tokio::test]
async fn realtime_event_store_resumes_with_retention_gaps() {
    init_tracing();
    let Ok(url) = std::env::var("WEBX_REDIS_URL") else {
        return;
    };
    let suffix = unique_suffix();
    let prefix = format!("webx:test:rt:{suffix}");
    let subscription = format!("room-{suffix}");
    let connection = Arc::new(RedisConnection::new(
        url,
        Duration::from_secs(15),
        Duration::from_secs(15),
    ));
    let store = RedisRealtimeEventStore::with_options(
        Arc::clone(&connection),
        RedisRealtimeEventStoreOptions {
            prefix: prefix.clone(),
            max_events: 50,
        },
    );

    // Enough appends to make the approximate `MAXLEN ~` trim evict the prefix
    // even with the default 100-entry stream macro node.
    let mut appended = Vec::new();
    for value in 0..250u32 {
        let event = store
            .append(&subscription, format!("{{\"n\":{value}}}"))
            .await
            .expect("append");
        appended.push(event.event_id);
    }
    assert!(
        appended.windows(2).all(|pair| pair[1] > pair[0]),
        "event ids must be monotonically increasing"
    );
    let last = *appended.last().expect("appended events");

    let all = store
        .resume(ResumeRequest::new(&subscription, None))
        .await
        .expect("resume from scratch");
    assert!(
        all.truncated,
        "a fresh cursor must report the evicted prefix"
    );
    let first_retained = all.events.first().expect("retained events").event_id;
    assert!(first_retained > 1);
    assert_eq!(all.next_cursor, last);
    assert_eq!(all.events.last().expect("last event").event_id, last);
    assert!(
        all.events.len() >= 50,
        "count retention must keep at least max_events: {}",
        all.events.len()
    );

    // A cursor immediately before the oldest retained event is not a gap.
    let boundary = store
        .resume(ResumeRequest::new(&subscription, Some(first_retained - 1)))
        .await
        .expect("resume at the retention boundary");
    assert!(!boundary.truncated);
    assert_eq!(
        boundary.events.first().map(|event| event.event_id),
        Some(first_retained)
    );

    // A cursor before it has a real gap.
    let gap = store
        .resume(ResumeRequest::new(&subscription, Some(first_retained - 2)))
        .await
        .expect("resume before the boundary");
    assert!(gap.truncated, "an evicted cursor must be reported as a gap");

    // Caught up: no events and the cursor does not move backwards.
    let caught_up = store
        .resume(ResumeRequest::new(&subscription, Some(last)))
        .await
        .expect("resume caught up");
    assert!(caught_up.events.is_empty());
    assert_eq!(caught_up.next_cursor, last);
    assert!(!caught_up.truncated);

    assert_eq!(store.latest_cursor(&subscription).await, last);
    let other = format!("{subscription}-other");
    assert_eq!(store.latest_cursor(&other).await, 0);
    let other_frame = store
        .resume(ResumeRequest::new(&other, None))
        .await
        .expect("resume empty subscription");
    assert!(other_frame.events.is_empty());
    assert_eq!(other_frame.next_cursor, 0);
    assert!(!other_frame.truncated);

    // A blank subscription fails closed with a validation error.
    let error = store
        .append("", "{}".to_string())
        .await
        .expect_err("a blank subscription cannot address a log");
    assert_eq!(error.status_code(), 400);

    if let Ok(mut raw) = connection.manager().await {
        let _ = redis::cmd("DEL")
            .arg(format!("{prefix}:{subscription}:log"))
            .arg(format!("{prefix}:{subscription}:seq"))
            .arg(format!("{prefix}:{other}:log"))
            .arg(format!("{prefix}:{other}:seq"))
            .query_async::<i64>(&mut raw)
            .await;
    }
}

#[tokio::test]
async fn presence_joins_touches_expires_and_leaves_by_identity() {
    init_tracing();
    let Ok(url) = std::env::var("WEBX_REDIS_URL") else {
        return;
    };
    let suffix = unique_suffix();
    let prefix = format!("webx:test:presence:{suffix}");
    let connection = Arc::new(RedisConnection::new(
        url,
        Duration::from_secs(15),
        Duration::from_secs(15),
    ));
    let presence = RedisOnlinePresence::with_options(
        Arc::clone(&connection),
        RedisOnlinePresenceOptions {
            prefix: prefix.clone(),
            max_members_per_scope: 1,
        },
    );
    let scope = PresenceScope::user(format!("u-{suffix}"));
    let member = PresenceMember::new(format!("u-{suffix}"), "c1")
        .with_node_id("node-a")
        .with_joined_at_unix_secs(1_000);

    assert_eq!(
        presence
            .join(&scope, member.clone(), Duration::from_secs(2))
            .await
            .expect("join"),
        1
    );
    assert_eq!(presence.count(&scope).await.expect("count"), 1);
    assert_eq!(
        presence.members(&scope).await.expect("members"),
        vec![member.clone()]
    );

    // A heartbeat built from a fresh join stamp still addresses the same
    // member (identity is user/connection/node) and extends its TTL.
    let refreshed = PresenceMember::new(format!("u-{suffix}"), "c1")
        .with_node_id("node-a")
        .with_joined_at_unix_secs(9_999);
    presence
        .touch(&scope, &refreshed, Duration::from_secs(5))
        .await
        .expect("touch");
    tokio::time::sleep(Duration::from_millis(2_200)).await;
    assert_eq!(
        presence.count(&scope).await.expect("count after touch"),
        1,
        "the heartbeat must extend the TTL past the original join window"
    );
    assert_eq!(
        presence
            .members(&scope)
            .await
            .expect("members after touch")
            .first()
            .map(|member| member.joined_at_unix_secs),
        Some(1_000),
        "a heartbeat does not rewrite the join stamp"
    );

    // Leave by identity even with a different join stamp.
    let leaving = PresenceMember::new(format!("u-{suffix}"), "c1")
        .with_node_id("node-a")
        .with_joined_at_unix_secs(12_345);
    assert_eq!(
        presence.leave(&scope, &leaving).await.expect("leave"),
        0,
        "leave must remove the member joined under a different stamp"
    );

    // Without a heartbeat the TTL expires the member.
    presence
        .join(&scope, member.clone(), Duration::from_secs(1))
        .await
        .expect("rejoin");
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    assert_eq!(presence.count(&scope).await.expect("expired count"), 0);
    assert!(presence
        .members(&scope)
        .await
        .expect("expired members")
        .is_empty());

    // A bounded scope rejects a second member with a retryable 503.
    presence
        .join(&scope, member.clone(), Duration::from_secs(30))
        .await
        .expect("join after expiry");
    let error = presence
        .join(
            &scope,
            PresenceMember::new(format!("u-{suffix}"), "c2").with_node_id("node-b"),
            Duration::from_secs(30),
        )
        .await
        .expect_err("a full scope must reject the join");
    assert_eq!(error.status_code(), 503);
    assert!(error.retryable());

    // Invalid inputs fail closed without touching Redis.
    let error = presence
        .join(&scope, member.clone(), Duration::ZERO)
        .await
        .expect_err("a zero TTL must be rejected");
    assert_eq!(error.status_code(), 400);
    let error = presence
        .join(
            &PresenceScope::custom("invalid", ""),
            member,
            Duration::from_secs(1),
        )
        .await
        .expect_err("an invalid scope must be rejected");
    assert_eq!(error.status_code(), 400);

    let _ = presence.leave(&scope, &leaving).await;
    if let Ok(mut raw) = connection.manager().await {
        let _ = redis::cmd("DEL")
            .arg(format!("{prefix}:scope:{scope}"))
            .query_async::<i64>(&mut raw)
            .await;
    }
}

// ---------------------------------------------------------------------------
// Advisory lock
// ---------------------------------------------------------------------------

#[tokio::test]
async fn redis_advisory_lock_serializes_and_recovers_from_expiry() {
    use webx_core::locking::IAdvisoryLock;

    let Ok(url) = std::env::var("WEBX_REDIS_URL") else {
        return;
    };
    let suffix = unique_suffix();
    let prefix = format!("webx:test:lock:{suffix}:");
    let connection = Arc::new(RedisConnection::new(
        url,
        Duration::from_secs(3),
        Duration::from_secs(2),
    ));
    let lock = RedisAdvisoryLock::new(Arc::clone(&connection), prefix.clone());
    let key = "quota:t1";

    let held = lock
        .try_acquire(key, Duration::from_secs(5))
        .await
        .expect("acquire")
        .expect("free key");
    assert!(lock
        .try_acquire(key, Duration::from_secs(5))
        .await
        .expect("contention is not an error")
        .is_none());
    assert!(lock.renew(&held).await.expect("renew"));
    assert!(lock.release(&held).await.expect("release"));
    assert!(!lock.release(&held).await.expect("double release is false"));

    // A crashed owner's lease expires and can be reclaimed; the stale token
    // cannot release the new lease.
    let short = lock
        .try_acquire(key, Duration::from_millis(50))
        .await
        .expect("acquire")
        .expect("free");
    tokio::time::sleep(Duration::from_millis(150)).await;
    let fresh = lock
        .try_acquire(key, Duration::from_secs(5))
        .await
        .expect("acquire")
        .expect("expired lease is reclaimable");
    assert_ne!(fresh.token, short.token);
    assert!(!lock.release(&short).await.expect("stale release is false"));
    assert!(lock.release(&fresh).await.expect("release fresh"));

    // Best-effort cleanup so the live test leaves no lock key behind.
    if let Ok(mut raw) = connection.manager().await {
        let _ = redis::cmd("DEL")
            .arg(format!("{prefix}lock:{key}"))
            .query_async::<i64>(&mut raw)
            .await;
    }
}

// ---------------------------------------------------------------------------
// Transport hardening regressions
// ---------------------------------------------------------------------------

/// Default options (5s block) must not sit on the shared multiplexed connection:
/// a blocking read on it would stall every cache/publish/health command for the
/// whole block window (and hit the default 5s command timeout).
#[tokio::test]
async fn default_block_window_never_stalls_the_shared_connection() {
    init_tracing();
    let Ok(url) = std::env::var("WEBX_REDIS_URL") else {
        return;
    };
    let suffix = unique_suffix();
    let counted = Arc::new(AtomicU32::new(0));
    let dispatcher = Arc::new(EventDispatcher::new(provider_with_counting_handler(
        Arc::clone(&counted),
    )));
    // Same 5s/5s as `add_redis` defaults.
    let connection = Arc::new(RedisConnection::new(
        url,
        Duration::from_secs(5),
        Duration::from_secs(5),
    ));
    let stream = format!("webx:test:defaults:{suffix}");
    let group = format!("webx-test-defaults-{suffix}");
    create_consumer_group(&connection, &stream, &group).await;
    let events = RedisEventsOptions {
        stream: stream.clone(),
        group: group.clone(),
        consumer: format!("webx-test-defaults-consumer-{suffix}"),
        ..RedisEventsOptions::default()
    };
    let publisher = RedisEventPublisher::new(Arc::clone(&connection), events.clone());
    let consumer =
        RedisEventConsumer::new(Arc::clone(&connection), events, Arc::clone(&dispatcher));

    let ct = CancellationToken::new();
    let consumer_ct = ct.clone();
    let task = tokio::spawn(async move { consumer.execute(consumer_ct).await });
    // Let the consumer enter its 5s blocking read.
    tokio::time::sleep(Duration::from_millis(300)).await;

    let ping_started = Instant::now();
    connection
        .ping()
        .await
        .expect("shared connection stays usable");
    let ping_elapsed = ping_started.elapsed();

    let message = EventMessage::from_event(&TestEvent { value: 1 }).expect("encode event");
    let publish_started = Instant::now();
    publisher
        .publish(message)
        .await
        .expect("publish on shared connection");
    let publish_elapsed = publish_started.elapsed();
    let deadline = Instant::now() + Duration::from_secs(3);
    while counted.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let delivered = counted.load(Ordering::SeqCst) > 0;

    stop_consumer(ct, task).await;
    if let Ok(mut raw) = connection.manager().await {
        delete_streams(&mut raw, &[&stream]).await;
    }

    assert!(
        ping_elapsed < Duration::from_secs(2),
        "a blocking XREADGROUP stalled the shared connection for {ping_elapsed:?}"
    );
    assert!(
        publish_elapsed < Duration::from_secs(2),
        "a blocking XREADGROUP stalled XADD for {publish_elapsed:?}"
    );
    assert!(delivered, "the default-config event was not delivered");
}

/// `consumer_enabled` only controls registration: a manually executed consumer
/// still runs both delivery loops, like the NATS/RabbitMQ/Kafka adapters.
#[tokio::test]
async fn manually_started_consumer_runs_both_loops_even_when_not_auto_registered() {
    init_tracing();
    let Ok(url) = std::env::var("WEBX_REDIS_URL") else {
        return;
    };
    let suffix = unique_suffix();
    let competing = Arc::new(AtomicU32::new(0));
    let broadcast = Arc::new(AtomicU32::new(0));
    let dispatcher = Arc::new(EventDispatcher::new(provider_with_delivery_counters(
        Arc::clone(&competing),
        Arc::clone(&broadcast),
    )));
    let connection = Arc::new(RedisConnection::new(
        url,
        Duration::from_secs(5),
        Duration::from_secs(5),
    ));
    let stream = format!("webx:test:manual:{suffix}");
    let group = format!("webx-test-manual-{suffix}");
    create_consumer_group(&connection, &stream, &group).await;
    let events = RedisEventsOptions {
        stream: stream.clone(),
        group: group.clone(),
        consumer: format!("webx-test-manual-consumer-{suffix}"),
        consumer_enabled: false,
        block_ms: 250,
        claim_idle_ms: 1_000,
        ..RedisEventsOptions::default()
    };
    let publisher = RedisEventPublisher::new(Arc::clone(&connection), events.clone());
    let consumer = RedisEventConsumer::new(Arc::clone(&connection), events, dispatcher);

    let ct = CancellationToken::new();
    let consumer_ct = ct.clone();
    let task = tokio::spawn(async move { consumer.execute(consumer_ct).await });
    tokio::time::sleep(Duration::from_millis(300)).await;

    let broadcast_message =
        EventMessage::from_event(&BroadcastEvent { value: 1 }).expect("encode broadcast");
    publisher
        .broadcast(broadcast_message)
        .await
        .expect("publish broadcast");
    let deadline = Instant::now() + Duration::from_secs(3);
    while broadcast.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let broadcast_delivered = broadcast.load(Ordering::SeqCst) > 0;

    stop_consumer(ct, task).await;
    if let Ok(mut raw) = connection.manager().await {
        delete_streams(&mut raw, &[&stream]).await;
    }

    assert!(
        broadcast_delivered,
        "a consumer whose options say consumer_enabled=false must still consume broadcast events when executed"
    );
}

/// A lost/reset sequence key is repaired from the Stream top; the next append
/// continues without a permanent id collision loop.
#[tokio::test]
async fn realtime_append_repairs_a_reset_sequence_key() {
    init_tracing();
    let Ok(url) = std::env::var("WEBX_REDIS_URL") else {
        return;
    };
    let suffix = unique_suffix();
    let prefix = format!("webx:test:rt-repair:{suffix}");
    let subscription = format!("room-{suffix}");
    let connection = Arc::new(RedisConnection::new(
        url,
        Duration::from_secs(5),
        Duration::from_secs(5),
    ));
    let store = RedisRealtimeEventStore::with_options(
        Arc::clone(&connection),
        RedisRealtimeEventStoreOptions {
            prefix: prefix.clone(),
            max_events: 50,
        },
    );

    let first = store
        .append(&subscription, "one".into())
        .await
        .expect("append");
    let second = store
        .append(&subscription, "two".into())
        .await
        .expect("append");
    let log_key = format!("{prefix}:{subscription}:log");
    let seq_key = format!("{prefix}:{subscription}:seq");
    {
        let mut raw = connection.manager().await.expect("raw connection");
        redis::cmd("DEL")
            .arg(&seq_key)
            .query_async::<i64>(&mut raw)
            .await
            .expect("delete seq");
    }

    // The sequence restarts at 1, so the first append collides with the stream
    // top and repairs the sequence before returning the error.
    let error = store
        .append(&subscription, "three".into())
        .await
        .expect_err("a reset sequence makes the first append fail closed");
    assert_eq!(error.status_code(), 503, "{error}");

    let repaired = store
        .append(&subscription, "four".into())
        .await
        .expect("the next append recovers");
    assert!(
        repaired.event_id > second.event_id,
        "repaired id {} must exceed the stream top {} (first was {})",
        repaired.event_id,
        second.event_id,
        first.event_id
    );

    {
        let mut raw = connection.manager().await.expect("raw connection");
        let _ = redis::cmd("DEL")
            .arg(&log_key)
            .arg(&seq_key)
            .query_async::<i64>(&mut raw)
            .await;
    }
}

/// A cursor from another store epoch (ahead of everything here) is reported as a
/// gap and reset to the latest known id, otherwise the client would never see a
/// new event again.
#[tokio::test]
async fn realtime_resume_resets_a_cursor_ahead_of_the_store() {
    init_tracing();
    let Ok(url) = std::env::var("WEBX_REDIS_URL") else {
        return;
    };
    let suffix = unique_suffix();
    let prefix = format!("webx:test:rt-ahead:{suffix}");
    let subscription = format!("room-{suffix}");
    let connection = Arc::new(RedisConnection::new(
        url,
        Duration::from_secs(5),
        Duration::from_secs(5),
    ));
    let store = RedisRealtimeEventStore::with_options(
        Arc::clone(&connection),
        RedisRealtimeEventStoreOptions {
            prefix: prefix.clone(),
            max_events: 50,
        },
    );

    let first = store
        .append(&subscription, "one".into())
        .await
        .expect("append");
    let ahead = store
        .resume(ResumeRequest::new(
            &subscription,
            Some(first.event_id + 100),
        ))
        .await
        .expect("resume");
    assert!(ahead.truncated, "an ahead cursor must report a gap");
    assert!(ahead.events.is_empty());
    assert_eq!(ahead.next_cursor, first.event_id);

    let second = store
        .append(&subscription, "two".into())
        .await
        .expect("append");
    let resumed = store
        .resume(ResumeRequest::new(&subscription, Some(ahead.next_cursor)))
        .await
        .expect("resume after reset");
    assert_eq!(resumed.events.len(), 1);
    assert_eq!(resumed.events[0].event_id, second.event_id);

    if let Ok(mut raw) = connection.manager().await {
        let _ = redis::cmd("DEL")
            .arg(format!("{prefix}:{subscription}:log"))
            .arg(format!("{prefix}:{subscription}:seq"))
            .query_async::<i64>(&mut raw)
            .await;
    }
}

/// Concurrent appends must get unique, monotonic ids from the atomic script.
#[tokio::test]
async fn realtime_concurrent_appends_get_unique_ids() {
    init_tracing();
    let Ok(url) = std::env::var("WEBX_REDIS_URL") else {
        return;
    };
    let suffix = unique_suffix();
    let prefix = format!("webx:test:rt-concurrent:{suffix}");
    let subscription = format!("room-{suffix}");
    let connection = Arc::new(RedisConnection::new(
        url,
        Duration::from_secs(15),
        Duration::from_secs(15),
    ));
    let store = Arc::new(RedisRealtimeEventStore::with_options(
        Arc::clone(&connection),
        RedisRealtimeEventStoreOptions {
            prefix: prefix.clone(),
            max_events: 256,
        },
    ));

    let mut tasks = Vec::new();
    for index in 0..16 {
        let store = Arc::clone(&store);
        let subscription = subscription.clone();
        tasks.push(tokio::spawn(async move {
            store
                .append(&subscription, format!("event-{index}"))
                .await
                .map(|event| event.event_id)
                .expect("concurrent append")
        }));
    }
    let mut ids = Vec::new();
    for task in tasks {
        ids.push(task.await.expect("append task"));
    }
    ids.sort_unstable();
    let expected: Vec<u64> = (1..=16).collect();
    assert_eq!(ids, expected);
    assert_eq!(store.latest_cursor(&subscription).await, 16);

    if let Ok(mut raw) = connection.manager().await {
        let _ = redis::cmd("DEL")
            .arg(format!("{prefix}:{subscription}:log"))
            .arg(format!("{prefix}:{subscription}:seq"))
            .query_async::<i64>(&mut raw)
            .await;
    }
}

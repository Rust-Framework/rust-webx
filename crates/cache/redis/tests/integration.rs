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
use webx_redis::{
    RedisAdvisoryLock, RedisCache, RedisConnection, RedisEventConsumer, RedisEventPublisher,
    RedisEventsOptions, RedisOptions,
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
        Err(Error::Internal("always fails".into()))
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
async fn start_id_zero_replays_an_event_published_before_consumption() {
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
        start_id: "0".to_string(),
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
                "start_id=0 did not replay the pre-consumer event: xlen={} pending={:?} groups={groups:?}",
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

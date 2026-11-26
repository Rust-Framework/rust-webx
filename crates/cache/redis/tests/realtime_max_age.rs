//! Live Redis test for the `max_age` replay window of
//! [`RedisRealtimeEventStore`](webx_redis::RedisRealtimeEventStore).
//!
//! Set `WEBX_REDIS_URL` (for example `redis://127.0.0.1:6379`) to run it.
//! Without a broker the test returns early and passes.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use webx_core::realtime::{IRealtimeEventStore, ResumeRequest};
use webx_redis::{RedisConnection, RedisRealtimeEventStore, RedisRealtimeEventStoreOptions};

fn unique_suffix() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    format!("{}-{nanos}", std::process::id())
}

/// Wait until just after a wall-clock second boundary.
///
/// The window compares whole seconds, so a zero-second window only keeps events
/// appended and resumed within the same second. Aligning the fresh append closes
/// the (otherwise tiny) flake where the second ticks between the append and the
/// resume.
async fn wait_for_second_boundary() {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| u64::from(elapsed.subsec_millis()))
        .unwrap_or(0);
    tokio::time::sleep(Duration::from_millis((1000 - millis) % 1000 + 10)).await;
}

#[tokio::test]
async fn max_age_window_filters_old_events_and_advances_the_cursor() {
    let Ok(url) = std::env::var("WEBX_REDIS_URL") else {
        return;
    };
    let suffix = unique_suffix();
    let prefix = format!("webx:test:rt-max-age:{suffix}");
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
            max_events: 64,
        },
    )
    .with_max_age(Duration::from_secs(0));

    let first = store
        .append(&subscription, "one".into())
        .await
        .expect("append first");
    let second = store
        .append(&subscription, "two".into())
        .await
        .expect("append second");
    assert!(second.event_id > first.event_id);

    // Let both events complete a whole second, then append a fresh one in a new
    // wall-clock second: a zero-second window keeps only the fresh event.
    tokio::time::sleep(Duration::from_millis(1_100)).await;
    wait_for_second_boundary().await;
    let third = store
        .append(&subscription, "three".into())
        .await
        .expect("append third");

    let frame = store
        .resume(ResumeRequest::new(&subscription, None))
        .await
        .expect("resume from scratch");
    assert!(frame.truncated, "the expired prefix must report a gap");
    assert_eq!(
        frame
            .events
            .iter()
            .map(|event| (event.event_id, event.payload.as_str()))
            .collect::<Vec<_>>(),
        vec![(third.event_id, "three")],
        "only the fresh event is inside the window"
    );
    assert_eq!(frame.next_cursor, third.event_id);

    // The advanced cursor is usable: the next append replays normally.
    wait_for_second_boundary().await;
    let fourth = store
        .append(&subscription, "four".into())
        .await
        .expect("append fourth");
    let next = store
        .resume(ResumeRequest::new(&subscription, Some(frame.next_cursor)))
        .await
        .expect("resume from next_cursor");
    assert_eq!(next.events.len(), 1);
    assert_eq!(next.events[0].event_id, fourth.event_id);
    assert_eq!(next.next_cursor, fourth.event_id);
    assert!(
        !next.truncated,
        "the cursor already covers the expired events"
    );

    // Best-effort cleanup so the live test leaves nothing behind.
    if let Ok(mut raw) = connection.manager().await {
        let _ = redis::cmd("DEL")
            .arg(format!("{prefix}:{subscription}:log"))
            .arg(format!("{prefix}:{subscription}:seq"))
            .query_async::<i64>(&mut raw)
            .await;
    }
}

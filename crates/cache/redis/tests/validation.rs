//! `add_redis` validates local options at host startup: a misconfigured
//! stream/group/tuning value fails before the server accepts requests instead
//! of silently running a consumer that can never make progress.

use webx_core::mode::AppMode;
use webx_host::server::Host;
use webx_redis::{RedisEventsOptions, RedisHostBuilderExt, RedisOptions};

#[tokio::test]
async fn invalid_event_options_fail_host_startup() {
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .add_redis(
            RedisOptions::new("redis://127.0.0.1:1").with_events(RedisEventsOptions {
                stream: String::new(),
                ..RedisEventsOptions::default()
            }),
        )
        .build();

    let error = host
        .run_at("127.0.0.1:0")
        .await
        .expect_err("a blank event stream must fail host startup");
    let message = error.to_string();
    assert!(
        message.contains("invalid Redis configuration"),
        "the startup error must identify the Redis configuration, got: {message}"
    );
    assert!(
        message.contains("stream"),
        "the startup error must name the invalid field, got: {message}"
    );
}

#[tokio::test]
async fn invalid_start_id_fails_host_startup() {
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .add_redis(
            RedisOptions::new("redis://127.0.0.1:1").with_events(RedisEventsOptions {
                start_id: "latest".to_string(),
                ..RedisEventsOptions::default()
            }),
        )
        .build();

    let error = host
        .run_at("127.0.0.1:0")
        .await
        .expect_err("an invalid start id must fail host startup");
    assert!(error.to_string().contains("start_id"), "got: {error}");
}

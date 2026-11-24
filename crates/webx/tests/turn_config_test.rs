//! 2.4 media config closure: the TURN section drives `TurnRestConfig` DI.
//!
//! The host owns the single source of truth (`Turn.Secret`, `Turn.Urls`,
//! `Turn.TtlSeconds`, `Turn.Realm` / `APP__Turn__*`); an invalid or half-filled
//! section fails startup instead of serving unusable credentials, and
//! `use_turn_config` is the explicit programmatic override.

use std::sync::Arc;

use webx::*;

#[tokio::test]
async fn turn_section_registers_a_validated_config_for_injection() {
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(|app| {
            app.useOptions(|options| {
                options.turn.secret = "turn-test-secret-32-bytes-minimum-ok".to_string();
                options.turn.urls = vec!["turn:turn.example:3478".to_string()];
                options.turn.ttl_seconds = 120;
                options.turn.realm = Some("example".to_string());
            });
        })
        .try_build()
        .expect("a valid Turn section must build");

    let config = host
        .provider()
        .get::<TurnRestConfig>()
        .expect("the host registers the Turn config for injection");
    assert_eq!(config.ttl_seconds, 120);
    assert_eq!(config.urls, vec!["turn:turn.example:3478"]);
    assert_eq!(config.realm.as_deref(), Some("example"));

    let credentials = config.credentials_for("user-1").expect("credentials");
    assert!(credentials.username.ends_with(":user-1"));
    assert_eq!(credentials.ttl_seconds, 120);
    assert!(!credentials.credential.is_empty());
}

#[tokio::test]
async fn partial_turn_section_fails_host_startup() {
    let error = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(|app| {
            app.useOptions(|options| {
                // URLs without a secret: configured, but unusable.
                options.turn.urls = vec!["turn:turn.example:3478".to_string()];
            });
        })
        .try_build()
        .err()
        .expect("a partial Turn section must not start");
    let message = error.operator().unwrap_or_default();
    assert!(message.contains("Turn"), "unexpected error: {message}");
}

#[tokio::test]
async fn use_turn_config_overrides_the_section() {
    let override_config = TurnRestConfig::new(
        "override-secret-32-bytes-minimum-value",
        vec!["turn:override.example:3478".to_string()],
    )
    .with_ttl(60);
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .use_turn_config(override_config)
        .try_build()
        .expect("builder override must build");
    let config = host
        .provider()
        .get::<TurnRestConfig>()
        .expect("registered config");
    assert_eq!(config.urls, vec!["turn:override.example:3478"]);
    assert_eq!(config.ttl_seconds, 60);
}

#[tokio::test]
async fn unconfigured_turn_section_registers_nothing() {
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .try_build()
        .expect("an empty Turn section is valid and means 'no TURN'");
    assert!(
        host.provider().get::<TurnRestConfig>().is_err(),
        "no TURN config should be registered when the section is empty"
    );
    let _ = Arc::new(host);
}

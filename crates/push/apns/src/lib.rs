//! Apple Push Notification service (APNs) adapter for the rust-webx
//! [`IPushSender`](webx_core::push::IPushSender) port.
//!
//! The framework owns the push port and its
//! [`PushOutcome`](webx_core::push::PushOutcome) verdict model; this crate owns
//! the APNs wire protocol: ES256 provider-token authentication over HTTP/2, the
//! `/3/device/{token}` request shape, and the translation of APNs status codes
//! into accepted / dead-token / retry-later verdicts.
//!
//! # Wiring
//!
//! ```ignore
//! use std::sync::Arc;
//! use std::time::Duration;
//! use webx_client::{HttpClient, HttpClientOptions};
//! use webx_core::push::IPushSender;
//! use webx_apns::{ApnsConfig, ApnsEnvironment, ApnsPushSender};
//!
//! // Credentials come from the environment or a secret manager, never from
//! // appsettings.json, source control or a database column.
//! let config = ApnsConfig {
//!     team_id: std::env::var("APNS_TEAM_ID")?,
//!     key_id: std::env::var("APNS_KEY_ID")?,
//!     private_key_pem: std::env::var("APNS_PRIVATE_KEY")?, // AuthKey_<key_id>.p8 contents
//!     topic: std::env::var("APNS_TOPIC")?,                // the app bundle id
//!     environment: ApnsEnvironment::Production,
//! };
//! let client = HttpClient::new(
//!     "apns",
//!     HttpClientOptions {
//!         // APNs speaks HTTP/2 only; this also advertises h2 via ALPN.
//!         http2_only: true,
//!         request_timeout: Some(Duration::from_secs(10)),
//!         ..HttpClientOptions::new(config.environment.base_url())
//!     },
//! )?;
//! let push: Arc<dyn IPushSender> = Arc::new(ApnsPushSender::new(config, Arc::new(client))?);
//! host.use_push(push);
//! ```
//!
//! The sender is a singleton: it caches the provider token and should be built
//! once in the composition root.
//!
//! # Verdicts
//!
//! | APNs response | [`PushOutcome`](webx_core::push::PushOutcome) |
//! |---|---|
//! | `200` | `Accepted` |
//! | `400` `BadDeviceToken` / `DeviceTokenNotForTopic`, `410` `Unregistered` | `TokenInvalid` |
//! | `400`/`403` `ExpiredProviderToken` | clears the token cache, `Retryable` |
//! | `429`, `5xx` | `Retryable` (with `Retry-After` when sent) |
//! | rejected credentials (`401`/`403`) | `Err` (configuration defect, fixed text) |
//! | other `4xx`/unknown reasons | `Err` (request defect, fixed text) |
//!
//! # Security boundary
//!
//! * The `.p8` signing key is only read from the caller (environment variable or
//!   secret store) and parsed into a [`jsonwebtoken::EncodingKey`];
//!   [`ApnsConfig`]'s `Debug` output redacts it.
//! * The provider token is cached for 45 minutes and refreshed under a
//!   `tokio::sync::Mutex` (one in-flight refresh, single-flight). It is never
//!   logged.
//! * Errors use the framework [`Error`](webx_core::error::Error) contract and
//!   carry fixed, token-free text: a device token never appears in an error
//!   message or a log line.

mod config;
mod sender;

pub use config::{ApnsConfig, ApnsEnvironment};
pub use sender::ApnsPushSender;

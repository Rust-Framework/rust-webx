//! Firebase Cloud Messaging (FCM HTTP v1) adapter for the rust-webx
//! [`IPushSender`](webx_core::push::IPushSender) port.
//!
//! The framework owns the push port and its
//! [`PushOutcome`](webx_core::push::PushOutcome) verdict model; this crate owns
//! the FCM HTTP v1 wire protocol: a Google service account signs an RS256 OAuth2
//! assertion, the resulting access token is cached, and messages are posted to
//! `projects/{project_id}/messages:send`. FCM error responses are translated
//! into accepted / dead-token / retry-later verdicts.
//!
//! # Wiring
//!
//! ```ignore
//! use std::sync::Arc;
//! use webx_client::{HttpClient, HttpClientOptions};
//! use webx_core::push::IPushSender;
//! use webx_fcm::{FcmPushSender, FcmServiceAccount};
//!
//! // Read the service-account JSON from the environment or a secret manager,
//! // never from appsettings.json or source control.
//! let json = std::env::var("FCM_SERVICE_ACCOUNT_JSON")?;
//! let account = FcmServiceAccount::from_json(&json)?;
//! // `messages:send` is posted relative to the client's base URL.
//! let client = HttpClient::new("fcm", HttpClientOptions::new("https://fcm.googleapis.com"))?;
//! let push: Arc<dyn IPushSender> = Arc::new(FcmPushSender::new(account, Arc::new(client))?);
//! host.use_push(push);
//! ```
//!
//! The `token_uri` from the service-account JSON is used as the OAuth2 token
//! endpoint (absolute URL), so a proxy or test double can be substituted by
//! changing that field only.
//!
//! # Verdicts
//!
//! | FCM response | [`PushOutcome`](webx_core::push::PushOutcome) |
//! |---|---|
//! | `200` | `Accepted` |
//! | `400`/`404` with `INVALID_ARGUMENT`/`NOT_FOUND` or `UNREGISTERED` | `TokenInvalid` |
//! | `429`, `5xx` | `Retryable` (`Retry-After` or RetryInfo `retryDelay`) |
//! | `401`/`403` | clears the access-token cache, `Err` (fixed text) |
//! | other `4xx` | `Err` (request defect, fixed text) |
//!
//! # Security boundary
//!
//! * The service-account private key is only read from the caller (environment
//!   variable or secret store) and parsed into a
//!   [`jsonwebtoken::EncodingKey`]; [`FcmServiceAccount`]'s `Debug` output
//!   redacts it.
//! * The OAuth2 access token is cached until 60 seconds before it expires and
//!   refreshed under a `tokio::sync::Mutex` (single-flight); it is never logged.
//! * Errors use the framework [`Error`](webx_core::error::Error) contract and
//!   carry fixed, token-free text: neither a device token nor an access token
//!   appears in an error message or a log line.

mod account;
mod sender;

pub use account::FcmServiceAccount;
pub use sender::FcmPushSender;

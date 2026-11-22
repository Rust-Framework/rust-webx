//! Square adapter for `rust-webx-pay`.
//!
//! Implements the Square Payments API (create/query/complete/cancel/refund),
//! Checkout API payment links for hosted redirect flows, and
//! `x-square-hmacsha256-signature` webhook verification. Amounts are integer
//! minor units with an explicit ISO 4217 currency; Square statuses are mapped to
//! the provider-neutral model while the raw status is preserved.
//!
//! ```ignore
//! use webx::Host;
//! use webx_pay_square::SquareHostBuilderExt;
//!
//! // Runtime configuration from the "Square" section.
//! let host = Host::builder().add_square().build();
//!
//! // Or a prebuilt static configuration:
//! // Host::builder().add_square_with(options).build();
//! ```
//!
//! # PCI boundary: client-side tokens only
//!
//! The server never sees card data. The browser must tokenize the card with the
//! Square Web Payments SDK (cards, Apple Pay, Google Pay) and the resulting
//! token is passed as `source_id`:
//!
//! ```ignore
//! let payment = CreatePayment::new(order_id, amount, PaymentMethod::Card, flow)
//!     .with_metadata("square_source_id", web_payments_sdk_token);
//! ```
//!
//! Without that metadata entry `create_payment` returns a
//! [`PaymentError::Validation`](webx_pay::PaymentError::Validation) explaining
//! the requirement: Square has no server-side card entry path.
//!
//! # Flows
//!
//! * [`PaymentFlow::ClientSdk`](webx_pay::PaymentFlow::ClientSdk) → `POST /v2/payments`;
//!   the returned [`ClientAction::ClientSdk`](webx_pay::ClientAction::ClientSdk)
//!   payload carries the Square payment id/status.
//! * [`PaymentFlow::Redirect`](webx_pay::PaymentFlow::Redirect) →
//!   `POST /v2/online-checkout/payment-links`; the returned
//!   [`ClientAction::Redirect`](webx_pay::ClientAction::Redirect) URL is the
//!   hosted checkout page. Payment links always capture immediately, so manual
//!   capture is rejected.
//! * QR / app flows are unsupported (Square has no server-rendered QR channel).
//!
//! # Identifiers
//!
//! Square has no lookup by merchant reference. `get_payment`, `capture_payment`,
//! `cancel_payment` and [`RefundCommand::order_id`](webx_pay::RefundCommand::order_id)
//! take the Square **payment id** (`PaymentIntent::provider_payment_id`);
//! `get_refund` takes the Square **refund id** (`Refund::provider_refund_id`).
//! Values that cannot be Square ids produce a validation error instead of an
//! opaque 404.
//!
//! # Webhooks
//!
//! The application declares the route and calls
//! [`Payments::verify_webhook`](webx_pay::Payments::verify_webhook) plus
//! [`Payments::ack`](webx_pay::Payments::ack); verification is
//! `base64(HMAC-SHA256(signature_key, notification_url + raw_body))` compared in
//! constant time. The event id is the de-duplication key. A successful
//! notification is acknowledged with an empty `200 OK` (Square retries anything
//! else).

mod mapping;
mod options;
mod provider;
mod transport;
mod webhook;

pub use mapping::{
    map_payment_event_type, map_payment_status, map_refund_event_type, map_refund_status,
    money_from, money_json,
};
pub use options::{
    SquareAccountOptions, SquareEnvironment, SquareOptions, DEFAULT_API_VERSION, LIVE_API_BASE,
    SANDBOX_API_BASE,
};
pub use provider::{
    SquareHostBuilderExt, SquareProvider, SquareProviderBuilder, SquareProviderFactory,
    SOURCE_ID_METADATA,
};
pub use transport::{derive_idempotency_key, IDEMPOTENCY_KEY_MAX};
pub use webhook::{verify_signature, SquareWebhookVerifier, SIGNATURE_HEADER};

/// Provider id owned by this adapter.
pub const PROVIDER: webx_pay::ProviderId = webx_pay::ProviderId::Square;

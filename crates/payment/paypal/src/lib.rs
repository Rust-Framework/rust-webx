//! PayPal adapter for rust-webx-pay.
//!
//! Implements Orders v2 (create/get/capture/cancel/refund) and Payments v2
//! (authorization capture, void, capture refund, refund lookup) over OAuth2
//! client-credentials with a per-account token cache, plus Webhooks v1
//! signature verification through
//! `POST /v1/notifications/verify-webhook-signature`.
//!
//! Amounts are rendered as PayPal decimal strings (`"99.00"`, `"1000"` for JPY,
//! `"1.234"` for KWD) from the shared [`webx_pay::Money`] minor-unit model.
//!
//! # Frontend catalog
//!
//! [`webx_pay::IPaymentProvider::payment_options`] returns one option per
//! configured account for a checkout page: [`webx_pay::PaymentMethod::PayPal`]
//! with the `client_sdk` and `redirect` flows and the public client
//! configuration `{"client_id": ..., "environment": "sandbox" | "live"}`. The
//! environment follows the account's configured
//! [`PayPalEnvironment`]; an `api_base` proxy override does not relabel it.
//! Only public identifiers are included: the client secret and webhook id never
//! leave the adapter. Applications reach the same catalog through
//! `webx_pay::Payments::payment_options` / `payment_options_for`, so both
//! `add_paypal()` (runtime configuration) and `add_paypal_with(options)`
//! (static configuration) are covered.
//!
//! [PayPalEnvironment]: crate::PayPalEnvironment
//!
//! # Provider order ids
//!
//! Orders v2 has no lookup by merchant reference, so the
//! [`webx_pay::IPaymentProvider`] methods that take an `order_id` (`get`,
//! `capture`, `cancel`, `refund`) expect the **PayPal order id** returned in
//! `PaymentIntent::provider_payment_id` (alphanumeric, for example
//! `5O190127TN364715T`). The merchant order id travels in
//! `purchase_units[].custom_id` and comes back on capture/refund webhooks.
//!
//! ```no_run
//! use webx::Host;
//! use webx_pay_paypal::PayPalHostBuilderExt;
//!
//! // Runtime configuration from the "PayPal" section.
//! let host = Host::builder().add_paypal().build();
//!
//! // Or a prebuilt static configuration:
//! // Host::builder()
//! //     .add_paypal_with(PayPalOptions::new("client-id", "client-secret", "webhook-id"))
//! //     .build();
//! ```

mod auth;
mod mapping;
mod model;
mod options;
mod provider;
mod transport;
mod webhook;

pub use auth::{PayPalClock, SystemClock};
pub use mapping::{map_capture_status, map_order_status, map_refund_status};
pub use options::{
    PayPalAccountOptions, PayPalEnvironment, PayPalOptions, LIVE_BASE_URL, SANDBOX_BASE_URL,
};
pub use provider::{
    PayPalHostBuilderExt, PayPalProvider, PayPalProviderBuilder, PayPalProviderFactory,
};
pub use webhook::{parse_event, PayPalWebhookVerifier};

/// Provider id owned by this adapter.
pub const PROVIDER: webx_pay::ProviderId = webx_pay::ProviderId::PayPal;

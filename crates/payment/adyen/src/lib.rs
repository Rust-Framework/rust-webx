//! Adyen Checkout adapter for `rust-webx-pay`.
//!
//! Implements the Adyen Checkout API v71: `/payments`, `/payments/details`,
//! `/payments/{pspReference}/captures`, `/cancels` and `/refunds`, plus standard
//! webhook HMAC-SHA256 verification (multi-item deliveries, constant-time
//! comparison, literal `[accepted]` acknowledgement). Request bodies are JSON
//! with `X-API-Key`; mutating calls carry an `Idempotency-Key`. Amounts are
//! always integer minor units.
//!
//! ```no_run
//! use webx_host::server::HostBuilder;
//! use webx_pay_adyen::{AdyenHostBuilderExt, AdyenOptions};
//!
//! // Runtime configuration from the "Adyen" configuration section.
//! let host = HostBuilder::new().add_adyen().build();
//!
//! // Or a prebuilt static configuration:
//! let host = HostBuilder::new()
//!     .add_adyen_with(AdyenOptions::new("api_key", "merchant_account", "hmac_key"))
//!     .build();
//! ```
//!
//! # Frontend payment catalog
//!
//! [`IPaymentProvider::payment_options`](webx_pay::IPaymentProvider::payment_options)
//! returns the public checkout catalog: for every configured account the
//! `card`, `apple_pay`, `google_pay` and `paypal` methods, each usable through
//! the client SDK (`client_sdk`) and hosted redirect (`redirect`) flows. The
//! attached `client_config` carries only the Adyen Drop-in/Components client key
//! (when configured), `environment` (`live`/`test`) and `merchant_account`; the
//! Checkout API key and the webhook HMAC key never appear in it.
//!
//! # Protocol notes and honest limitations
//!
//! * **Payment method details are not modeled.** The shared
//!   [`PaymentMethod`](webx_pay::PaymentMethod) only carries a type. The Adyen
//!   `paymentMethod` object is built as
//!   `{"type": <mapped type>}`; any additional fields (encrypted card data,
//!   Apple Pay/Google Pay tokens, issuer details) come from the
//!   [`METADATA_PAYMENT_METHOD`] metadata JSON object. Card data never reaches
//!   this server otherwise (PCI scope stays with Adyen/your client SDK).
//! * **No generic payment-status query.** The Checkout API has no documented
//!   `GET` payment endpoint, so [`IPaymentProvider::get_payment`](webx_pay::IPaymentProvider::get_payment)
//!   returns `Unsupported` instead of inventing one. Use
//!   [`AdyenProvider::get_payment_with_details`] (or
//!   [`AdyenProvider::get_payment_with_metadata`]) with the `redirectResult` /
//!   `threeDSResult` details returned from the redirect, or rely on the
//!   `AUTHORISATION` webhook.
//! * **Capture/cancel/refund need a pspReference.** The port has no separate
//!   provider-payment-id parameter and Adyen has no lookup-by-merchant-reference
//!   endpoint, so pass [`PaymentIntent::provider_payment_id`](webx_pay::PaymentIntent::provider_payment_id)
//!   (16 characters, optionally `psp:`-prefixed) in the `order_id` slot.
//! * **Adyen requires explicit capture and refund amounts.** `capture_payment`
//!   with `amount = None` and `refund` with `amount = None` are rejected: Adyen
//!   operates on a specified amount and this adapter cannot query the authorized
//!   or captured amount. A `CANCELLATION` acknowledgement has no amount in the
//!   Adyen response, so the returned intent uses ISO `XXX` with zero minor
//!   units; the real amount stays with the stored payment/webhook.
//! * **`get_refund` is unsupported.** Refunds are asynchronous; their final
//!   state arrives in `REFUND`/`REFUND_FAILED` webhooks and there is no
//!   documented refund-status endpoint.
//! * **`channel` cannot be derived for app flows.** `PaymentFlow::App` defaults
//!   to `Android`; set [`METADATA_CHANNEL`] to `iOS`/`Android`/`Web` to override.
//! * **Batched webhooks.** One delivery can contain several notification items.
//!   The shared [`IPaymentWebhookVerifier`](webx_pay::IPaymentWebhookVerifier)
//!   port returns a single event, so it returns the first item after verifying
//!   the whole delivery. Applications that expect batches must call
//!   [`AdyenWebhookVerifier::verify_all`].
//! * **Hosted Checkout Sessions are not implemented** (`hosted_checkout = false`).

mod hmac;
mod mapping;
mod options;
mod provider;
mod transport;
mod webhook;

use webx_pay::ProviderId;

pub use hmac::{
    decode_hmac_key, escape_field, sign_notification, sign_notification_hex, signing_string,
    verify_notification, NotificationHmacFields,
};
pub use mapping::map_result_code;
pub use options::{AdyenAccountOptions, AdyenOptions, DEFAULT_API_VERSION, TEST_API_BASE};
pub use provider::{
    AdyenHostBuilderExt, AdyenProvider, AdyenProviderBuilder, AdyenProviderFactory,
};
pub use webhook::{
    normalize_event, parse_notification, AdyenWebhookVerifier, NotificationAdditionalData,
    NotificationAmount, NotificationItem, ParsedNotification, ParsedNotificationItem, ACK_BODY,
};

/// Provider id owned by this adapter.
pub const PROVIDER: ProviderId = ProviderId::Adyen;

/// Pinned Adyen Checkout API version path segment.
pub const API_VERSION: &str = DEFAULT_API_VERSION;

/// Metadata key carrying the JSON object merged into Adyen's `paymentMethod`.
pub const METADATA_PAYMENT_METHOD: &str = "adyen_payment_method";

/// Metadata key carrying the JSON `details` object for `POST /payments/details`.
pub const METADATA_DETAILS: &str = "adyen_details";

/// Metadata key carrying the `paymentData` value for `POST /payments/details`.
pub const METADATA_PAYMENT_DATA: &str = "adyen_payment_data";

/// Metadata key overriding the Adyen `channel` (`Web`, `iOS`, `Android`).
pub const METADATA_CHANNEL: &str = "adyen_channel";

/// Current Unix time in seconds.
pub(crate) fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

//! Alipay adapter for rust-webx-pay.
//!
//! Implements Alipay Open Platform payments over the one outbound HTTP stack
//! (`rust-webx-client`): page/WAP redirects, app SDK order strings, QR
//! (`precreate`), query, close, refund/refund-query and asynchronous
//! notification verification (public-key **and** public-key-certificate mode).
//!
//! All amounts are CNY and converted to Alipay's two-decimal yuan string at the
//! adapter boundary; the shared model keeps integer minor units.
//!
//! ```no_run
//! use webx::Host;
//! use webx_pay_alipay::AlipayHostBuilderExt;
//!
//! // Runtime configuration from the "Alipay" section.
//! let host = Host::builder().add_alipay().build();
//!
//! // Or a prebuilt static configuration:
//! // Host::builder()
//! //     .add_alipay_with(AlipayOptions::new(
//! //         "2021000000000001",
//! //         "<application-private-key-pem>",
//! //         "<alipay-public-key-pem>",
//! //     ))
//! //     .build();
//! ```
//!
//! Certificate mode and multi-account setups use [`AlipayKeyMode`] /
//! [`AlipayOptions::with_account`]. Configuration binding uses the `"Alipay"`
//! section and [`AlipayHostBuilderExt::add_alipay`].
//!
//! # Signature behaviour (official SDK parity)
//!
//! * Request signature content: sorted, non-empty `k=v` joined by `&`, `sign`
//!   excluded (`sign_type` included).
//! * Notification signature content: `sign` **and** `sign_type` excluded.
//! * Gateway responses: RSA2 over the raw JSON substring of the
//!   `<method>_response` value (never a re-serialization); `error_response`
//!   handled as a typed provider error.
//! * Certificate SN: official Alipay value, i.e. lowercase zero-padded MD5 of
//!   `issuer name + decimal serial number` (see [`AlipayOptions`] docs and
//!   `sign.rs`).
//!
//! # Client actions
//!
//! * page/WAP → [`ClientAction::Redirect`](webx_pay::ClientAction) with the
//!   signed `gateway.do?...` URL.
//! * app → [`ClientAction::AppOrder`](webx_pay::ClientAction) with
//!   `payload = { "order_str": "<signed app order string>" }`; hand that string
//!   to the Alipay app SDK (`PayTask.payV2(orderStr, ...)`).
//! * QR → [`ClientAction::QrCode`](webx_pay::ClientAction) with the
//!   `qr_code` value returned by `alipay.trade.precreate`.
//!
//! Refund lookup (`get_refund`) takes `out_trade_no:out_request_no`, because
//! `alipay.trade.fastpay.refund.query` identifies a refund by both values.
//!
//! # Frontend payment catalog
//!
//! [`IPaymentProvider::payment_options`](webx_pay::IPaymentProvider::payment_options)
//! reports the selectable channels of every configured account for a checkout
//! page. Each account contributes four entries:
//!
//! | method | flow | `client_config` |
//! |---|---|---|
//! | `AlipayPage` / `AlipayWap` | redirect | `{ "app_id": "<app id>" }` |
//! | `AlipayApp` | app SDK | `{ "app_id": "<app id>" }` |
//! | `AlipayQr` | QR code | `{ "app_id": "<app id>" }` |
//!
//! Only the public application id is exposed. The application private key,
//! Alipay public key and certificates never appear in the catalog.
//!
//! # Callbacks
//!
//! The crate registers no routes. Applications bind
//! [`webx_pay::PaymentWebhook`], call `Payments::verify_webhook`, then answer
//! with `Payments::ack` — the adapter's acknowledgement is the literal text
//! body `success`.

mod mapping;
mod options;
mod provider;
mod sign;
#[cfg(test)]
mod test_support;
mod time;
mod transport;
mod webhook;

pub use mapping::{map_refund_status, map_trade_status};
pub use options::{AlipayAccountOptions, AlipayKeyMode, AlipayOptions};
pub use provider::{
    AlipayHostBuilderExt, AlipayProvider, AlipayProviderBuilder, AlipayProviderFactory,
};
pub use webhook::{verify_notification_signature, AlipayWebhookVerifier};

/// Provider id owned by this adapter.
pub const PROVIDER: webx_pay::ProviderId = webx_pay::ProviderId::Alipay;

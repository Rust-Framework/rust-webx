//! Payment domain kit for rust-webx: provider-neutral ports, money/status models,
//! runtime service factories, webhook ingress and multi-account routing.
//!
//! The crate is the *contract* half of the payment domain group. Protocol
//! adapters (`rust-webx-pay-stripe`, `-alipay`, `-wechatpay`, `-paypal`,
//! `-adyen`, `-square`) implement [`IPaymentProvider`] on top of the single
//! outbound HTTP stack (`rust-webx-client`). Applications register adapters on
//! the host and inject [`Payments`]:
//!
//! ```no_run
//! use webx::Host;
//!
//! // Adapter crates add `.add_stripe()`, `.add_alipay()`, ... here; the
//! // payment runtime is created lazily by the first adapter registration.
//! let host = Host::builder().build();
//! ```
//!
//! # Configuration model
//!
//! * `add_stripe()` (adapter crates) registers a runtime provider factory that
//!   reads the `Stripe` configuration section/monitor.
//! * `add_stripe_with(options)` registers a prebuilt provider instead.
//! * An application can replace the runtime source with
//!   [`IPaymentServiceFactory`] (DB/KMS) without depending on adapter crates;
//!   [`PaymentProviderBuilders`] lets it reuse the official protocol code.
//!
//! # Scope rules
//!
//! * No provider SDK and no second HTTP stack in this crate.
//! * Provider-specific fields stay in `provider_status`, `ClientAction::payload`
//!   and `PaymentEvent::raw`; the shared model only carries what is genuinely
//!   common.
//! * Tenant/account selection lives in [`PaymentScope`]; tenant ids come from
//!   the trusted identity, never from request bodies.
//! * Callback routes are declared by the application through
//!   [`payment_webhook!`]; this crate provides the raw-body request type,
//!   signature verification and the provider-specific acknowledgement, never a
//!   hidden route.
//! * Money is always integer minor units with an explicit ISO 4217 currency;
//!   there is no floating-point amount anywhere in the API.

mod account;
mod ack;
mod capabilities;
mod catalog;
mod error;
mod events;
mod factory;
mod ids;
mod model;
mod money;
mod payments;
mod provider;
mod register;
mod scope;
mod status;
pub mod testing;
mod webhook;

pub use account::AccountSet;
pub use ack::{empty_ok_ack, json_success_ack, text_success_ack};
pub use capabilities::{CurrencyScope, ProviderCapabilities, WebhookCapability};
pub use catalog::{PaymentFlowKind, PaymentOption};
pub use error::PaymentError;
pub use events::{PaymentEvent, PaymentEventType, PAYMENT_EVENT_PREFIX};
pub use factory::{
    DefaultPaymentServiceFactory, IPaymentProviderBuilder, IPaymentProviderFactory,
    IPaymentServiceFactory, PaymentProviderBuilders, PaymentProviderConfig,
    StaticPaymentProviderFactory,
};
pub use ids::{AccountId, ProviderId, TenantId};
pub use model::{
    CaptureMode, ClientAction, CreatePayment, PaymentAccount, PaymentFlow, PaymentIntent,
    PaymentMethod, Refund, RefundCommand,
};
pub use money::{Currency, Money, MoneyError};
pub use payments::{PaymentRuntimeOptions, Payments};
pub use provider::{IPaymentProvider, IPaymentWebhookVerifier, WebhookPayload};
pub use register::PaymentsServiceCollectionExt;
pub use scope::PaymentScope;
pub use status::{PaymentStatus, RefundStatus};
pub use webhook::PaymentWebhook;

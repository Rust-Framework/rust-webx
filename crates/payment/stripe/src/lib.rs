//! Stripe adapter for rust-webx-pay.
//!
//! Implements PaymentIntents (client-SDK flow), Checkout Sessions (hosted
//! redirect), refunds, capture/cancel and `Stripe-Signature` webhook
//! verification. Request bodies follow Stripe's form-encoded v1 API; amounts
//! are integer minor units.
//!
//! ```no_run
//! use webx::Host;
//! use webx_pay_stripe::StripeHostBuilderExt;
//!
//! // Runtime configuration from the "Stripe" section.
//! let host = Host::builder().add_stripe().build();
//!
//! // Or a prebuilt static configuration:
//! // Host::builder().add_stripe_with(StripeOptions::new("sk_test_...", "whsec_...")).build();
//! ```

mod form;
mod mapping;
mod options;
mod provider;
mod webhook;

pub use form::encode_form_pairs;
pub use mapping::map_status;
pub use options::{StripeAccountOptions, StripeOptions};
pub use provider::{
    StripeHostBuilderExt, StripeProvider, StripeProviderBuilder, StripeProviderFactory,
};
pub use webhook::{verify_signature, StripeWebhookVerifier, SIGNATURE_TOLERANCE_SECONDS};

/// Provider id owned by this adapter.
pub const PROVIDER: webx_pay::ProviderId = webx_pay::ProviderId::Stripe;

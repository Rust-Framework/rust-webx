//! Payment-provider contract suite.
//!
//! Behavioral assertions every [`IPaymentProvider`] adapter must satisfy
//! without network access: account/registry shape, capability consistency,
//! startup validation, and the "unsupported capability returns `Unsupported`"
//! rule. Protocol signatures and status mapping are adapter-specific and live in
//! each adapter's own tests.

use std::collections::HashSet;

use crate::{IPaymentProvider, PaymentError, PaymentScope};

/// Run the shared payment-provider contract.
///
/// `provider` must be configured with at least one account and valid static
/// configuration; the suite never performs a network call.
pub async fn payment_provider_contract(provider: &dyn IPaymentProvider) {
    let id = provider.id();
    let accounts = provider.accounts();
    assert!(
        !accounts.is_empty(),
        "{id} must expose at least one account"
    );

    let mut ids = HashSet::new();
    for account in accounts {
        assert_eq!(
            account.provider, id,
            "{id} returned an account owned by another provider"
        );
        assert!(
            ids.insert(account.id.clone()),
            "{id} returned a duplicate account id `{}`",
            account.id
        );
    }

    let capabilities = provider.capabilities();
    assert!(
        capabilities.create || capabilities.query,
        "{id} must support create or query"
    );
    assert!(
        !capabilities.partial_refund || capabilities.refund,
        "{id} declares partial refunds without refund support"
    );
    assert!(
        capabilities.webhook != crate::WebhookCapability::None,
        "{id} must declare a webhook signature capability"
    );
    assert_eq!(
        provider.webhook_verifier().provider(),
        id,
        "{id} webhook verifier must belong to the same provider"
    );

    provider
        .validate()
        .unwrap_or_else(|error| panic!("{id} configuration must validate: {error}"));

    let scope = PaymentScope::new(id.clone());
    if !capabilities.capture {
        let error = provider
            .capture_payment(&scope, "contract-order", None)
            .await
            .expect_err("capture must be unsupported");
        assert!(
            matches!(error, PaymentError::Unsupported { .. }),
            "{id} must return Unsupported for capture, got {error:?}"
        );
    }
    if !capabilities.cancel {
        let error = provider
            .cancel_payment(&scope, "contract-order")
            .await
            .expect_err("cancel must be unsupported");
        assert!(
            matches!(error, PaymentError::Unsupported { .. }),
            "{id} must return Unsupported for cancel, got {error:?}"
        );
    }
    if !capabilities.refund {
        let request = crate::RefundCommand::new("contract-order", "contract-refund");
        let error = provider
            .refund(&scope, request)
            .await
            .expect_err("refund must be unsupported");
        assert!(
            matches!(error, PaymentError::Unsupported { .. }),
            "{id} must return Unsupported for refund, got {error:?}"
        );
    }
}

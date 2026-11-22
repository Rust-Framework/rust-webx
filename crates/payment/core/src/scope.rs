//! Runtime resolution scope: provider + tenant + merchant account.

use std::fmt;

use crate::error::PaymentError;
use crate::ids::{AccountId, ProviderId, TenantId};

/// Everything the runtime needs to select a payment provider instance.
///
/// `tenant` comes from the trusted identity (or server configuration), never
/// from a request body/query. `account` selects a merchant account inside the
/// resolved provider; `None` means the provider's default account.
///
/// A bare [`ProviderId`] converts into a scope with no tenant/account, which is
/// the single-tenant/default-account shorthand:
///
/// ```ignore
/// payments.create(ProviderId::Stripe, request).await?;
/// payments.create(PaymentScope::new(ProviderId::Stripe).tenant(tenant), request).await?;
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PaymentScope {
    provider: ProviderId,
    tenant: Option<TenantId>,
    account: Option<AccountId>,
}

impl PaymentScope {
    /// A scope for `provider` with no tenant/account selection.
    pub fn new(provider: ProviderId) -> Self {
        Self {
            provider,
            tenant: None,
            account: None,
        }
    }

    /// Select the tenant (trusted identity only).
    pub fn tenant(mut self, tenant: impl Into<TenantId>) -> Self {
        self.tenant = Some(tenant.into());
        self
    }

    /// Select a merchant account.
    pub fn account(mut self, account: impl Into<AccountId>) -> Self {
        self.account = Some(account.into());
        self
    }

    /// Build a scope from the authenticated identity.
    ///
    /// The tenant comes from `IdentityUser::tenant_id`; an empty value means
    /// "no tenant claim". The identity is the only trusted source for tenant;
    /// never pass a request-body/query value here.
    ///
    /// ```ignore
    /// let scope = PaymentScope::from_identity(ProviderId::Stripe, &req.user)?;
    /// let intent = payments.create(scope, request).await?;
    /// ```
    pub fn from_identity(
        provider: ProviderId,
        user: &webx_core::auth::IdentityUser,
    ) -> Result<Self, PaymentError> {
        let mut scope = Self::new(provider);
        scope.tenant = Self::tenant_from_identity(user)?;
        Ok(scope)
    }

    /// Extract the trusted tenant from an identity, if the claim is present.
    ///
    /// This is the shared helper for [`Self::from_identity`] and the payment
    /// catalog (`Payments::payment_options_for`).
    pub fn tenant_from_identity(
        user: &webx_core::auth::IdentityUser,
    ) -> Result<Option<TenantId>, PaymentError> {
        let tenant = user.tenant_id.trim();
        if tenant.is_empty() {
            return Ok(None);
        }
        TenantId::new(tenant.to_string())
            .map(Some)
            .map_err(PaymentError::validation)
    }

    /// Provider being resolved.
    pub fn provider(&self) -> &ProviderId {
        &self.provider
    }

    /// Trusted tenant, when the caller supplied one.
    pub fn tenant_ref(&self) -> Option<&TenantId> {
        self.tenant.as_ref()
    }

    /// Merchant account, when the caller selected one.
    pub fn account_ref(&self) -> Option<&AccountId> {
        self.account.as_ref()
    }

    /// Render the scope for logs/metrics; tenant/account are hashed by callers
    /// that need redaction.
    pub fn label(&self) -> String {
        let tenant = self
            .tenant
            .as_ref()
            .map(|tenant| tenant.as_str())
            .unwrap_or("-");
        let account = self
            .account
            .as_ref()
            .map(|account| account.as_str())
            .unwrap_or("-");
        format!("{}/{tenant}/{account}", self.provider)
    }
}

impl From<ProviderId> for PaymentScope {
    fn from(provider: ProviderId) -> Self {
        Self::new(provider)
    }
}

impl fmt::Display for PaymentScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_provider_becomes_default_scope() {
        let scope = PaymentScope::from(ProviderId::Stripe);
        assert_eq!(scope.provider(), &ProviderId::Stripe);
        assert!(scope.tenant_ref().is_none());
        assert!(scope.account_ref().is_none());
    }

    #[test]
    fn builder_sets_trusted_scope_fields() {
        let scope = PaymentScope::new(ProviderId::Alipay)
            .tenant("org/acme")
            .account("acct_1");
        assert_eq!(scope.tenant_ref().unwrap().as_str(), "org/acme");
        assert_eq!(scope.account_ref().unwrap().as_str(), "acct_1");
        assert_eq!(scope.label(), "alipay/org/acme/acct_1");
    }

    #[test]
    fn identity_builds_the_tenant_scope_from_the_trusted_claim() {
        let user = webx_core::auth::IdentityUser {
            tenant_id: "org/acme".to_string(),
            ..Default::default()
        };
        let scope = PaymentScope::from_identity(ProviderId::Stripe, &user).unwrap();
        assert_eq!(scope.tenant_ref().unwrap().as_str(), "org/acme");

        let single_tenant = webx_core::auth::IdentityUser::default();
        let scope = PaymentScope::from_identity(ProviderId::Stripe, &single_tenant).unwrap();
        assert!(scope.tenant_ref().is_none());
    }

    #[test]
    fn scope_is_hashable_for_cache_keys() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(PaymentScope::new(ProviderId::Stripe).tenant("a"));
        set.insert(PaymentScope::new(ProviderId::Stripe).tenant("b"));
        assert_eq!(set.len(), 2);
    }
}

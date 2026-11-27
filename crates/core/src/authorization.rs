//! External policy decision point (PDP) authorization port.
//!
//! Route-level authorization in rust-webx is declarative (`#[authorize]` rules)
//! or in-process ([`crate::auth::IDynamicAuthorizer`]).
//! A deployment that centralizes roles and permissions in an external policy
//! service plugs it in through [`IAuthorizationClient`]: business code depends on
//! this port, and the host supplies an adapter (for example the HTTP PDP client
//! in `rust-webx-host`).
//!
//! The decision contract has two distinct failure modes, and callers must not
//! conflate them:
//!
//! * **`Ok(Deny)` is a normal decision.** The PDP answered, and the answer is
//!   "no". Map it to `403 Forbidden` exactly like a failed authorization rule.
//! * **`Err(..)` means the PDP could not decide.** The backend is unreachable,
//!   timed out, answered non-2xx or returned a malformed body. Treat it as
//!   **fail-closed**: answer `503 Service Unavailable` and never fall back to
//!   `Allow`. Every native adapter maps this to a retryable `503`.
//!
//! Nothing here defines a permission model, roles or resource hierarchies: what
//! `permission` and `resource` mean is the PDP's contract with the application.
//!
//! ```ignore
//! #[derive(Inject)]
//! struct PayInvoiceHandler {
//!     #[inject]
//!     authz: Arc<dyn IAuthorizationClient>,
//! }
//!
//! let decision = self.authz.check(&AccessCheck {
//!     subject: user.id().to_string(),
//!     tenant_id: user.tenant_id().map(ToString::to_string),
//!     permission: "invoice.pay".to_string(),
//!     resource: Some(invoice.id.to_string()),
//!     context: BTreeMap::new(),
//! })
//! .await?; // `?` = 503 fail-closed
//! if decision != AccessDecision::Allow {
//!     return Err(Error::forbidden("not allowed to pay this invoice"));
//! }
//! ```

use std::collections::BTreeMap;

use async_trait::async_trait;

use crate::error::Result;

/// One authorization question.
///
/// Pure data: no business model, no identity extraction. The caller fills it
/// from the request's trusted identity and the operation it is about to run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccessCheck {
    /// Subject id (user or service account) the decision is about.
    pub subject: String,
    /// Optional tenant scope. `None` means "not tenant-scoped", which is a
    /// different question from an empty tenant.
    pub tenant_id: Option<String>,
    /// Permission code the subject needs, in the PDP's vocabulary.
    pub permission: String,
    /// Optional resource identifier (instance or type) the permission applies
    /// to.
    pub resource: Option<String>,
    /// Additional attributes for attribute-based decisions. Ordered, so the
    /// HTTP adapter can build a stable cache key.
    pub context: BTreeMap<String, String>,
}

/// The PDP's answer to one [`AccessCheck`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessDecision {
    /// The subject may perform the operation.
    Allow,
    /// The subject may not perform the operation. This is a normal `403`, not an
    /// outage.
    Deny,
}

/// Port for an external policy decision point.
///
/// Implementations must return `Err` when the PDP is unavailable or its answer
/// cannot be trusted; see the module docs for the fail-closed rule. They must
/// never turn a failed lookup into [`AccessDecision::Allow`].
#[async_trait]
pub trait IAuthorizationClient: Send + Sync {
    /// Ask the PDP about one check.
    ///
    /// `Err` means the PDP could not answer (backend failure), not "denied".
    async fn check(&self, check: &AccessCheck) -> Result<AccessDecision>;

    /// Ask about several checks in one call.
    ///
    /// The returned vector has the same length and order as `checks`. The
    /// default implementation issues one [`check`](Self::check) per entry, in
    /// order; adapters with a native batch endpoint override it.
    async fn check_batch(&self, checks: &[AccessCheck]) -> Result<Vec<AccessDecision>> {
        let mut decisions = Vec::with_capacity(checks.len());
        for check in checks {
            decisions.push(self.check(check).await?);
        }
        Ok(decisions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed(AccessDecision);

    #[async_trait]
    impl IAuthorizationClient for Fixed {
        async fn check(&self, _check: &AccessCheck) -> Result<AccessDecision> {
            Ok(self.0)
        }
    }

    struct Unavailable;

    #[async_trait]
    impl IAuthorizationClient for Unavailable {
        async fn check(&self, _check: &AccessCheck) -> Result<AccessDecision> {
            Err(crate::error::Error::unavailable("policy service down"))
        }
    }

    fn check(permission: &str) -> AccessCheck {
        AccessCheck {
            subject: "user-1".to_string(),
            permission: permission.to_string(),
            ..AccessCheck::default()
        }
    }

    #[tokio::test]
    async fn access_check_defaults_are_empty_and_ordered() {
        let default = AccessCheck::default();
        assert_eq!(default.subject, "");
        assert_eq!(default.tenant_id, None);
        assert_eq!(default.resource, None);
        assert!(default.context.is_empty());

        let mut context = BTreeMap::new();
        context.insert("region".to_string(), "eu".to_string());
        context.insert("amount".to_string(), "100".to_string());
        let populated = AccessCheck {
            context,
            ..AccessCheck::default()
        };
        assert_eq!(
            populated.context.keys().cloned().collect::<Vec<_>>(),
            vec!["amount".to_string(), "region".to_string()],
            "BTreeMap keeps the context in a stable order for cache keys"
        );
    }

    #[tokio::test]
    async fn default_batch_preserves_order_and_deny_is_ok() {
        let allow = Fixed(AccessDecision::Allow);
        assert_eq!(
            allow
                .check_batch(&[check("a"), check("b"), check("c")])
                .await
                .expect("infallible"),
            vec![
                AccessDecision::Allow,
                AccessDecision::Allow,
                AccessDecision::Allow
            ]
        );

        let deny = Fixed(AccessDecision::Deny);
        assert_eq!(
            deny.check_batch(&[]).await.expect("empty batch"),
            Vec::<AccessDecision>::new()
        );
        assert_eq!(
            deny.check(&check("a"))
                .await
                .expect("deny is a normal answer"),
            AccessDecision::Deny
        );
    }

    #[tokio::test]
    async fn backend_failures_are_errors_and_retryable() {
        let error = Unavailable
            .check_batch(&[check("a")])
            .await
            .expect_err("a backend failure must never become a decision");
        assert_eq!(error.status_code(), 503);
        assert!(error.retryable());
        assert_eq!(
            crate::error::Error::unavailable("x").status_code(),
            503,
            "the adapter contract maps outages to 503"
        );
    }
}

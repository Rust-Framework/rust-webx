//! Dynamic authorization: the set of `IDynamicAuthorizer` instances the host
//! runs on protected routes.
//!
//! `AuthorizerSet` collects all `IDynamicAuthorizer` instances from DI and runs
//! them in sequence on protected routes.
//!
//! # Dynamic authorizer
//!
//! ```ignore
//! use webx_core::auth::{IClaims, IDynamicAuthorizer};
//!
//! #[derive(Default)]
//! struct RoleAuthorizer;
//!
//! #[async_trait::async_trait]
//! impl IDynamicAuthorizer for RoleAuthorizer {
//!     async fn authorize(&self, claims: &dyn IClaims, pattern: &str, method: &str) -> Result<()> {
//!         // your custom logic
//!         Ok(())
//!     }
//! }
//! ```
//!
//! Then register in DI:
//! ```ignore
//! svc.singleton::<dyn IDynamicAuthorizer>(|_| Arc::new(RoleAuthorizer::default()))
//! ```

use std::sync::Arc;
use webx_core::auth::{IClaims, IDynamicAuthorizer};
use webx_core::error::Result;

// ---------------------------------------------------------------------------
// AuthorizerSet —dynamic authorizer collection
// ---------------------------------------------------------------------------

/// A set of `IDynamicAuthorizer` instances collected from the DI container.
///
/// Created automatically in `HostBuilder::build()` when one or more
/// `IDynamicAuthorizer` implementations are registered. If none are registered,
/// this is `None` and no dynamic authorization checks run.
#[derive(Clone)]
pub struct AuthorizerSet {
    authorizers: Vec<Arc<dyn IDynamicAuthorizer>>,
}

impl AuthorizerSet {
    /// Create a new set from a vector of authorizers.
    pub fn new(authorizers: Vec<Arc<dyn IDynamicAuthorizer>>) -> Self {
        Self { authorizers }
    }

    /// Run all registered authorizers in sequence through the rule-less
    /// [`IDynamicAuthorizer::authorize`] entry.
    ///
    /// Use [`AuthorizerSet::authorize_with_declared_rules`] when the matched
    /// endpoint declared `#[authorize]` rules.
    ///
    /// Returns `Ok(())` if ALL authorizers pass. Returns the first `Err` if any denies.
    pub async fn authorize(
        &self,
        claims: &dyn IClaims,
        route_pattern: &str,
        method: &str,
    ) -> Result<()> {
        for authorizer in &self.authorizers {
            authorizer.authorize(claims, route_pattern, method).await?;
        }
        Ok(())
    }

    /// Run all registered authorizers in sequence, handing them the matched
    /// endpoint's `#[authorize]` declaration as the raw slice the route
    /// metadata owns (`RouteEntry.authorization`).
    ///
    /// Zero allocation, and the declaration is the same
    /// `&'static [AuthorizeRule]` a dynamic endpoint delegates in full
    /// (`#[authorize(dynamic)]`). Implementations that override only the
    /// rule-less `authorize` keep working through the trait's default
    /// forwarding.
    ///
    /// Returns `Ok(())` if ALL authorizers pass. Returns the first `Err` if any denies.
    pub async fn authorize_with_declared_rules(
        &self,
        claims: &dyn IClaims,
        route_pattern: &str,
        method: &str,
        declared: &'static [webx_core::route::scan::AuthorizeRule],
    ) -> Result<()> {
        for authorizer in &self.authorizers {
            authorizer
                .authorize_with_declared_rules(claims, route_pattern, method, declared)
                .await?;
        }
        Ok(())
    }

    /// Returns `true` if there are no authorizers registered.
    pub fn is_empty(&self) -> bool {
        self.authorizers.is_empty()
    }
}

/// Result from collecting `IDynamicAuthorizer` instances from DI.
/// `None` means no authorizers are registered (pass-through mode).
pub fn collect_authorizers(provider: &rust_dix::ServiceProvider) -> Option<Arc<AuthorizerSet>> {
    let authorizers: Vec<Arc<dyn IDynamicAuthorizer>> = provider.get_all();
    if authorizers.is_empty() {
        None
    } else {
        Some(Arc::new(AuthorizerSet::new(authorizers)))
    }
}

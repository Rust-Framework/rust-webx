//! Per-request ambient context for handler and persistence layers.

use std::future::Future;
use std::sync::Arc;

use crate::auth::IdentityUser;

tokio::task_local! {
    /// The full authenticated identity of the current request, if any.
    static IDENTITY: Option<Arc<IdentityUser>>;
}

/// Scoped identity of the current request, shared by every layer.
///
/// This is the single source of truth for "who is acting". The HTTP adapter
/// establishes the context **before** binding the request (`RequestModel::bind`),
/// so the binder, the handler, middleware and the persistence layer all read the
/// very same `Arc<IdentityUser>` — there is no second copy of `sub` and no way
/// for audit and handler code to disagree.
///
/// Background work (`IHostedService`, spawned tasks, timers) has **no** ambient
/// identity by default: `identity()` and `operator_id()` return `None` there,
/// which audit code must treat as the System principal. A background job that
/// acts on behalf of a user scopes that identity explicitly:
///
/// ```ignore
/// let identity = Arc::new(IdentityUser { id: "user-1".into(), ..Default::default() });
/// RequestContext::run(Some(identity), async move {
///     // audit here sees user-1
/// })
/// .await;
/// ```
pub struct RequestContext;

impl RequestContext {
    /// Run an async block with `identity` in scope.
    ///
    /// Pass `None` for an anonymous request (or a system-initiated task); inside
    /// `f`, `identity()` and `operator_id()` then return `None`.
    pub async fn run<F, R>(identity: Option<Arc<IdentityUser>>, f: F) -> R
    where
        F: Future<Output = R>,
    {
        IDENTITY.scope(identity, f).await
    }

    /// The authenticated identity of the current request, if any.
    ///
    /// Returns the very `Arc` the HTTP adapter created, so resolving the
    /// identity twice never allocates a second copy.
    pub fn identity() -> Option<Arc<IdentityUser>> {
        IDENTITY
            .try_with(|identity| identity.as_ref().map(Arc::clone))
            .ok()
            .flatten()
    }

    /// The current operator id (`IdentityUser::id`), if authenticated.
    ///
    /// `None` for anonymous requests and for background work that did not scope
    /// an identity — audit code should record those as System.
    pub fn operator_id() -> Option<String> {
        Self::identity().map(|identity| identity.id.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(id: &str, tenant: &str) -> Arc<IdentityUser> {
        Arc::new(IdentityUser {
            id: id.to_string(),
            tenant_id: tenant.to_string(),
            ..Default::default()
        })
    }

    #[tokio::test]
    async fn identity_and_operator_id_are_visible_inside_run() {
        let user = identity("user-1", "acme");

        RequestContext::run(Some(Arc::clone(&user)), async {
            let current = RequestContext::identity().expect("identity must be in scope");
            assert!(
                Arc::ptr_eq(&current, &user),
                "identity() must hand back the same Arc, not a second copy"
            );
            assert_eq!(current.tenant(), Some("acme"));
            assert_eq!(RequestContext::operator_id().as_deref(), Some("user-1"));
        })
        .await;
    }

    #[tokio::test]
    async fn anonymous_scope_has_no_identity() {
        RequestContext::run(None, async {
            assert!(RequestContext::identity().is_none());
            assert!(RequestContext::operator_id().is_none());
        })
        .await;
    }

    #[test]
    fn outside_run_work_is_system() {
        // No `run` on this task: background paths default to no identity.
        assert!(RequestContext::identity().is_none());
        assert!(RequestContext::operator_id().is_none());
    }

    #[tokio::test]
    async fn nested_scope_restores_the_outer_identity() {
        let outer = identity("outer-user", "");

        RequestContext::run(Some(Arc::clone(&outer)), async {
            RequestContext::run(None, async {
                assert!(RequestContext::identity().is_none());
            })
            .await;

            assert_eq!(
                RequestContext::operator_id().as_deref(),
                Some("outer-user"),
                "inner scope must not leak into the outer request"
            );
        })
        .await;
    }
}

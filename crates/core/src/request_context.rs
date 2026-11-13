//! Per-request ambient context for handler and persistence layers.

use std::future::Future;
use std::sync::Arc;

use crate::auth::IdentityUser;

tokio::task_local! {
    /// The full authenticated identity of the current request, if any.
    static IDENTITY: Option<Arc<IdentityUser>>;
    /// The trace id of the current request, if the host assigned one.
    static TRACE_ID: Option<Arc<str>>;
}

/// Scoped identity **and trace id** of the current request, shared by every
/// layer.
///
/// This is the single source of truth for "who is acting" and "which request is
/// this". The HTTP adapter establishes the context **before** binding the
/// request (`RequestModel::bind`), so the binder, the handler, middleware and
/// the persistence layer all read the very same `Arc<IdentityUser>` and the very
/// same trace id — there is no second copy of `sub` and no way for audit and
/// handler code to disagree.
///
/// The trace id is the one the tracing middleware echoes in the `x-trace-id`
/// response header, so a handler that stamps it onto an audit row can be joined
/// with the access log line for the same request:
///
/// ```ignore
/// let trace_id = RequestContext::trace_id().map(|id| id.to_string());
/// audit.trace_id = trace_id;
/// ```
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
    ///
    /// An enclosing trace id is preserved: nesting a narrower identity scope
    /// (audit helpers do this) must not make the trace id disappear.
    pub async fn run<F, R>(identity: Option<Arc<IdentityUser>>, f: F) -> R
    where
        F: Future<Output = R>,
    {
        let trace_id = Self::trace_id();
        Self::run_with_trace(identity, trace_id, f).await
    }

    /// Run an async block with `identity` and `trace_id` in scope.
    ///
    /// The HTTP adapter uses this: it passes the context's
    /// [`IHttpContext::trace_id`](crate::http::IHttpContext::trace_id) so the
    /// handler reads exactly the id emitted in the `x-trace-id` response header.
    /// Pass `None` for a request that has no trace id.
    pub async fn run_with_trace<F, R>(
        identity: Option<Arc<IdentityUser>>,
        trace_id: Option<Arc<str>>,
        f: F,
    ) -> R
    where
        F: Future<Output = R>,
    {
        IDENTITY.scope(identity, TRACE_ID.scope(trace_id, f)).await
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

    /// The trace id of the current request, if the host assigned one.
    ///
    /// Identical to the `x-trace-id` response header. `None` outside a request
    /// scope (background work) and for hosts that do not run the tracing
    /// middleware.
    pub fn trace_id() -> Option<Arc<str>> {
        TRACE_ID.try_with(|trace| trace.clone()).ok().flatten()
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

    #[tokio::test]
    async fn trace_id_is_visible_and_matches_the_scoped_value() {
        RequestContext::run_with_trace(None, Some(Arc::from("trace-42")), async {
            assert_eq!(RequestContext::trace_id().as_deref(), Some("trace-42"));
        })
        .await;

        assert!(
            RequestContext::trace_id().is_none(),
            "the trace id must not outlive its scope"
        );
    }

    #[tokio::test]
    async fn nested_run_keeps_the_enclosing_trace_id() {
        RequestContext::run_with_trace(Some(identity("user-1", "acme")), Some(Arc::from("t-1")), async {
            // A narrower identity scope (an audit helper, a background step)
            // must not lose the request's trace id.
            RequestContext::run(None, async {
                assert_eq!(RequestContext::trace_id().as_deref(), Some("t-1"));
            })
            .await;

            assert_eq!(RequestContext::trace_id().as_deref(), Some("t-1"));
        })
        .await;
    }

    #[tokio::test]
    async fn anonymous_request_without_tracing_has_no_trace_id() {
        RequestContext::run(
            None,
            async {
                assert!(RequestContext::trace_id().is_none());
            },
        )
        .await;
    }
}

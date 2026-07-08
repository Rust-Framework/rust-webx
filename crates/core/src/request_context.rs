//! Per-request ambient context for handler and persistence layers.

use std::future::Future;

tokio::task_local! {
    static OPERATOR_ID: Option<String>;
}

/// Scoped operator identity from JWT `sub`, set by the HTTP dispatch pipeline.
pub struct RequestContext;

impl RequestContext {
    /// Run an async block with the given operator id in scope.
    pub async fn run<F, R>(operator_id: Option<String>, f: F) -> R
    where
        F: Future<Output = R>,
    {
        OPERATOR_ID.scope(operator_id, f).await
    }

    /// Current request operator id, if authenticated.
    pub fn operator_id() -> Option<String> {
        OPERATOR_ID.try_with(|id| id.clone()).ok().flatten()
    }
}

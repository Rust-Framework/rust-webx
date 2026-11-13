//! Typed pipeline chain construction.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::error::Result;
use crate::pipeline::{IPipelineBehavior, Next};

/// Build the typed chain around a terminal handler.
///
/// The first behavior in `behaviors` is the outermost. The returned [`Next`]
/// runs the whole chain when [`Next::run`] is called.
pub fn build_typed_chain<Req, Rsp>(
    behaviors: Vec<Arc<dyn IPipelineBehavior<Req, Rsp>>>,
    terminal: impl FnOnce(Req) -> Pin<Box<dyn Future<Output = Result<Rsp>> + Send>> + Send + 'static,
) -> Next<Req, Rsp>
where
    Req: Send + 'static,
    Rsp: Send + 'static,
{
    let mut next = Next::new(terminal);
    for behavior in behaviors.into_iter().rev() {
        let inner = next;
        next = Next::new(move |req: Req| {
            let behavior = Arc::clone(&behavior);
            async move { behavior.handle(req, inner).await }
        });
    }
    next
}

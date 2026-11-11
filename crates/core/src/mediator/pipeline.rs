//! Pipeline behavior chain construction (typed and legacy erased).

// The legacy type-erased pipeline is deprecated but exercised on purpose here.
#![allow(deprecated)]

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::error::Result;
use crate::pipeline::{BoxedNextFn, IErasedPipelineBehavior, IPipelineBehavior, Next};

/// Build the legacy type-erased chain around a terminal handler.
///
/// Behaviors are applied in Vec order: the first element is the outermost
/// (runs first on request, last on response). Reverse-fold constructs the
/// nested closure chain.
#[allow(deprecated)]
pub fn build_chain(
    behaviors: Vec<Arc<dyn IErasedPipelineBehavior>>,
    terminal: BoxedNextFn,
) -> BoxedNextFn {
    let mut next = terminal;
    for behavior in behaviors.into_iter().rev() {
        let inner_next = next;
        let b = Arc::clone(&behavior);
        next = Box::new(
            move |req: Box<dyn std::any::Any + Send>| -> crate::pipeline::BoxedPipelineFuture {
                Box::pin(async move { b.handle(req, inner_next).await })
            },
        );
    }
    next
}

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

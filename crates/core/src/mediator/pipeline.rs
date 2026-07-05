//! Pipeline behavior chain construction.

use std::sync::Arc;

use crate::pipeline::{BoxedNextFn, IPipelineBehavior};

/// Build a chain of pipeline behaviors wrapping a terminal handler.
///
/// Behaviors are applied in Vec order: the first element is the outermost
/// (runs first on request, last on response). Reverse-fold constructs the
/// nested closure chain.
pub(crate) fn build_chain(
    behaviors: Vec<Arc<dyn IPipelineBehavior>>,
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

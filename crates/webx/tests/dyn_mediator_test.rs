//! `Arc<dyn IMediator>` + `send_boxed` (0.10 object-safe mediator).
//!
//! The typed `send` needs `T`/`R` at the call site; `send_boxed` lets code that
//! only holds a `dyn IMediator` dispatch a request and downcast the response.
//! It must run the same pipeline as `send`: this test registers a typed
//! behavior and checks it applies to the boxed path too.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use webx::*;

#[derive(Debug, Clone)]
struct DynRequest {
    n: u32,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct DynResponse {
    n: u32,
}

impl IRequest<DynResponse> for DynRequest {}

#[derive(Default)]
struct DynHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<DynRequest, DynResponse> for DynHandler {
    async fn handle(&mut self, req: DynRequest) -> ActionResult<DynResponse> {
        Ok(DynResponse { n: req.n * 2 })
    }
}

#[derive(Default)]
struct AddFive;

#[async_trait::async_trait]
impl IPipelineBehavior<DynRequest, DynResponse> for AddFive {
    async fn handle(
        &self,
        req: DynRequest,
        next: Next<DynRequest, DynResponse>,
    ) -> ActionResult<DynResponse> {
        next.run(DynRequest { n: req.n + 5 }).await
    }
}

fn provider() -> Arc<rust_dix::ServiceProvider> {
    rust_dix::ServiceCollection::new()
        .add_typed_pipeline::<AddFive, DynRequest, DynResponse>()
        .build()
        .expect("provider must build")
}

async fn send_boxed_with(
    provider: Arc<rust_dix::ServiceProvider>,
    req: DynRequest,
) -> Result<DynResponse> {
    let mediator: Arc<dyn IMediator> = Arc::new(Mediator::new(
        Arc::clone(&provider),
        Arc::new(HandlerCache::build()),
    ));

    mediator
        .send_boxed(Box::new(req))
        .await
        .and_then(|response| {
            response
                .downcast::<DynResponse>()
                .map(|boxed| *boxed)
                .map_err(|_| Error::Internal("send_boxed returned the wrong response type".into()))
        })
}

#[tokio::test]
async fn dyn_mediator_send_boxed_dispatches_through_the_typed_pipeline() {
    let response = send_boxed_with(provider(), DynRequest { n: 21 })
        .await
        .expect("send_boxed must dispatch");
    assert_eq!(
        response,
        DynResponse { n: 52 },
        "(21+5)*2: the typed behavior must apply to the boxed path"
    );
}

#[tokio::test]
async fn dyn_mediator_send_boxed_reports_unregistered_requests() {
    #[derive(Debug, Clone)]
    struct UnknownRequest;

    let provider = provider();
    let mediator: Arc<dyn IMediator> = Arc::new(Mediator::new(
        Arc::clone(&provider),
        Arc::new(HandlerCache::build()),
    ));

    let result = mediator.send_boxed(Box::new(UnknownRequest)).await;
    let message = format!("{}", result.expect_err("unregistered request must fail"));
    assert!(
        message.contains("boxed dispatch"),
        "error must name the missing boxed dispatch bridge: {message}"
    );
}

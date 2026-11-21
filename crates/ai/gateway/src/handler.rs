//! `/v1/chat/completions` handler.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use webx::{
    async_trait, get, handler, post, ActionResult, IMediator, IRequest, IRequestHandler,
    IdentityUser, Inject, JsonResult, Response, SseResult, WebxRequestMeta,
};
use webx_ai::{CanonicalRequest, GatewayContext, GatewayError, GatewayExecutor};

use crate::request::OpenAiChatRequest;
use crate::response::{openai_response, OpenAiErrorResult};
use crate::sse::OpenAiSseEncoder;

/// Executor-injected chat-completions handler.
#[derive(Inject)]
pub struct OpenAiChatHandler {
    /// Data-plane executor built by the application.
    #[inject]
    pub executor: Arc<GatewayExecutor>,
}

#[post("/v1/chat/completions")]
impl IRequest<Response> for OpenAiChatRequest {}

/// `GET /v1/models` request; the claim field makes the endpoint protected.
#[derive(Debug, Default, WebxRequestMeta)]
pub struct OpenAiModelsRequest {
    /// Authenticated principal; not otherwise used.
    #[fromclaim]
    pub user: IdentityUser,
}

#[get("/v1/models")]
impl IRequest<serde_json::Value> for OpenAiModelsRequest {}

/// Lists the public aliases currently in the catalog.
#[derive(Inject)]
pub struct OpenAiModelsHandler {
    /// Data-plane executor.
    #[inject]
    pub executor: Arc<GatewayExecutor>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<OpenAiModelsRequest, serde_json::Value> for OpenAiModelsHandler {
    async fn handle(&mut self, _request: OpenAiModelsRequest) -> ActionResult<serde_json::Value> {
        let catalog = self.executor.catalog_snapshot();
        let mut ids = catalog.aliases.keys().cloned().collect::<Vec<_>>();
        ids.sort();
        let data = ids
            .into_iter()
            .map(|id| {
                json!({
                    "id": id,
                    "object": "model",
                    "created": 0,
                    "owned_by": "gateway"
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({ "object": "list", "data": data }))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<OpenAiChatRequest, Response> for OpenAiChatHandler {
    async fn handle(&mut self, request: OpenAiChatRequest) -> ActionResult<Response> {
        // The billing/idempotency id is server-generated. A client-supplied
        // `x-request-id` is correlation metadata only: using it as the ledger's
        // unique key let a client replay one value and suppress every later
        // usage row (free usage), and colliding ids could hide another tenant's.
        let client_request_id = request.request_id.clone();
        let request_id = uuid::Uuid::new_v4().to_string();
        if let Some(correlation) = client_request_id {
            tracing::debug!(
                %correlation,
                request_id = %request_id,
                "client request id is correlation metadata only"
            );
        }
        let tenant = request.user.require_tenant()?.to_string();
        let key_id = request.user.id.clone();
        let alias = request.model.clone();
        let stream = request.stream.unwrap_or(false);
        let dry_run = request.dry_run.unwrap_or(false);

        let canonical = match CanonicalRequest::try_from(request) {
            Ok(canonical) => canonical,
            Err(message) => return Ok(invalid_response(message)),
        };
        let context = GatewayContext::new(request_id, tenant, key_id, alias.clone());

        if dry_run {
            let estimate = self.executor.estimate(&context, &canonical);
            return Ok(Box::new(JsonResult::new(json!({
                "object": "gateway.estimate",
                "model": estimate.model,
                "prompt_tokens": estimate.prompt_tokens,
                "max_completion_tokens": estimate.max_completion_tokens,
                "estimated_cost_micros": estimate.estimated_cost_micros,
            }))));
        }

        if stream {
            match self.executor.stream(&context, canonical).await {
                Ok(stream) => {
                    let encoder = OpenAiSseEncoder::new(stream, alias);
                    Ok(Box::new(
                        SseResult::new(encoder).keep_alive(Duration::from_secs(15)),
                    ))
                }
                Err(error) => Ok(error_response(error)),
            }
        } else {
            match self.executor.complete(&context, canonical).await {
                Ok(response) => Ok(Box::new(JsonResult::new(openai_response(
                    &alias, &response,
                )))),
                Err(error) => Ok(error_response(error)),
            }
        }
    }
}

fn error_response(error: GatewayError) -> Response {
    Box::new(OpenAiErrorResult::from_gateway(&error))
}

fn invalid_response(message: String) -> Response {
    Box::new(OpenAiErrorResult::invalid(message))
}

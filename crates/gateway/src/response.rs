//! OpenAI-compatible response envelopes and the gateway error result.

use async_trait::async_trait;
use serde_json::{json, Map, Value};
use std::time::{SystemTime, UNIX_EPOCH};
use webx::{IActionResult, IHttpContext};
use webx_ai::{CanonicalResponse, ContentPart, FinishReason, GatewayError, OpenAiError, Usage};

/// Build an OpenAI `chat.completion` body.
pub fn openai_response(alias: &str, response: &CanonicalResponse) -> Value {
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    for part in &response.message.content {
        match part {
            ContentPart::Text(content) => text.push_str(content),
            ContentPart::ToolCall(call) => tool_calls.push(json!({
                "id": call.id,
                "type": "function",
                "function": { "name": call.name, "arguments": call.arguments }
            })),
            // Assistant responses are text + tool calls; other parts are input-only.
            _ => {}
        }
    }

    let mut message = Map::new();
    message.insert("role".into(), json!("assistant"));
    if text.is_empty() && !tool_calls.is_empty() {
        message.insert("content".into(), Value::Null);
    } else {
        message.insert("content".into(), json!(text));
    }
    if !tool_calls.is_empty() {
        message.insert("tool_calls".into(), Value::Array(tool_calls));
    }

    let created = unix_secs();
    json!({
        "id": if response.id.is_empty() {
            format!("chatcmpl-{created}")
        } else {
            response.id.clone()
        },
        "object": "chat.completion",
        "created": created,
        "model": alias,
        "choices": [{
            "index": 0,
            "message": Value::Object(message),
            "finish_reason": finish_reason(&response.finish_reason),
        }],
        "usage": usage_json(response.usage),
    })
}

/// OpenAI `usage` object.
pub(crate) fn usage_json(usage: Usage) -> Value {
    json!({
        "prompt_tokens": usage.prompt_tokens,
        "completion_tokens": usage.completion_tokens,
        "total_tokens": usage.total_tokens(),
        "prompt_tokens_details": { "cached_tokens": usage.cached_tokens },
        "completion_tokens_details": { "reasoning_tokens": usage.reasoning_tokens },
    })
}

/// OpenAI `finish_reason` string.
pub(crate) fn finish_reason(reason: &FinishReason) -> &str {
    match reason {
        FinishReason::Stop => "stop",
        FinishReason::Length => "length",
        FinishReason::ToolCalls => "tool_calls",
        FinishReason::ContentFilter => "content_filter",
        FinishReason::Other(other) => other,
    }
}

/// An error action result with the OpenAI envelope.
pub struct OpenAiErrorResult {
    status: u16,
    body: Value,
    retry_after_secs: Option<u64>,
}

impl OpenAiErrorResult {
    /// Convert a gateway failure into an OpenAI error response.
    pub fn from_gateway(error: &GatewayError) -> Self {
        let status = error.http_status();
        let message = if status >= 500 {
            "an unexpected error occurred".to_string()
        } else {
            error.to_string()
        };
        let mut result = Self::new(status, message, error.retry_after());
        if matches!(
            error,
            GatewayError::Quota(quota) if quota.is_exceeded()
        ) {
            result.body = serde_json::to_value(OpenAiError::quota(error.to_string()))
                .unwrap_or_else(|_| json!({"error": {"message": "quota exceeded"}}));
        }
        result
    }

    /// A 400 from request conversion.
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(400, message, None)
    }

    /// Build an error result directly.
    pub fn new(
        status: u16,
        message: impl Into<String>,
        retry_after: Option<std::time::Duration>,
    ) -> Self {
        let body = serde_json::to_value(OpenAiError::from_status(status, message))
            .unwrap_or_else(|_| json!({"error": {"message": "request failed"}}));
        Self {
            status,
            body,
            retry_after_secs: retry_after.map(|duration| duration.as_secs().max(1)),
        }
    }
}

#[async_trait]
impl IActionResult for OpenAiErrorResult {
    async fn execute(self: Box<Self>, ctx: &mut dyn IHttpContext) -> webx::Result<()> {
        ctx.response_mut().set_status(self.status);
        ctx.response_mut()
            .set_header("content-type", "application/json");
        if let Some(retry_after) = self.retry_after_secs {
            let value = retry_after.to_string();
            ctx.response_mut().set_header("retry-after", &value);
        }
        let bytes = serde_json::to_vec(&self.body).unwrap_or_default();
        ctx.response_mut().write_bytes(bytes).await
    }
}

fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use webx_ai::{CanonicalResponse, Message, Role, ToolCall};

    fn response() -> CanonicalResponse {
        CanonicalResponse {
            id: "chatcmpl-1".into(),
            model: "upstream-model".into(),
            message: Message::new(
                Role::Assistant,
                vec![
                    ContentPart::Text("hello".into()),
                    ContentPart::ToolCall(ToolCall {
                        id: "call-1".into(),
                        name: "lookup".into(),
                        arguments: "{\"q\":\"x\"}".into(),
                    }),
                ],
            ),
            usage: Usage {
                prompt_tokens: 10,
                completion_tokens: 5,
                cached_tokens: 2,
                reasoning_tokens: 1,
            },
            finish_reason: FinishReason::ToolCalls,
        }
    }

    #[test]
    fn response_uses_the_public_alias_and_openai_shape() {
        let value = openai_response("fast", &response());
        assert_eq!(value["id"], "chatcmpl-1");
        assert_eq!(value["object"], "chat.completion");
        assert_eq!(value["model"], "fast");
        assert_eq!(value["choices"][0]["message"]["content"], "hello");
        assert_eq!(
            value["choices"][0]["message"]["tool_calls"][0]["id"],
            "call-1"
        );
        assert_eq!(value["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(value["usage"]["total_tokens"], 18);
        assert_eq!(value["usage"]["prompt_tokens_details"]["cached_tokens"], 2);
    }

    #[test]
    fn response_with_only_tool_calls_has_null_content() {
        let mut response = response();
        response.message.content = vec![ContentPart::ToolCall(ToolCall {
            id: "call-1".into(),
            name: "lookup".into(),
            arguments: "{}".into(),
        })];
        let value = openai_response("fast", &response);
        assert!(value["choices"][0]["message"]["content"].is_null());
    }

    #[test]
    fn error_result_hides_5xx_detail_and_sets_retry_after() {
        let error = GatewayError::Provider(webx_ai::ProviderError::Unavailable(
            "secret upstream detail".into(),
        ));
        let result = OpenAiErrorResult::from_gateway(&error);
        let body = serde_json::to_value(&result.body).expect("body");
        assert_eq!(result.status, 503);
        assert_eq!(body["error"]["message"], "an unexpected error occurred");
        assert!(result.retry_after_secs.is_none());

        let rate =
            OpenAiErrorResult::new(429, "slow down", Some(std::time::Duration::from_secs(3)));
        assert_eq!(rate.retry_after_secs, Some(3));
    }

    #[test]
    fn finish_reasons_map_to_openai_strings() {
        assert_eq!(finish_reason(&FinishReason::Stop), "stop");
        assert_eq!(finish_reason(&FinishReason::Length), "length");
        assert_eq!(finish_reason(&FinishReason::ToolCalls), "tool_calls");
        assert_eq!(
            finish_reason(&FinishReason::ContentFilter),
            "content_filter"
        );
        assert_eq!(
            finish_reason(&FinishReason::Other("custom".into())),
            "custom"
        );
    }

    #[test]
    fn quota_exceeded_uses_insufficient_quota() {
        let error = GatewayError::Quota(webx_ai::QuotaError::Exceeded {
            dimension: webx_ai::QuotaDimension::TokensPerMinute,
            limit: 10,
            retry_after: Some(std::time::Duration::from_secs(2)),
        });
        let result = OpenAiErrorResult::from_gateway(&error);
        assert_eq!(result.status, 429);
        assert_eq!(result.body["error"]["type"], "insufficient_quota");
        assert_eq!(result.body["error"]["code"], "insufficient_quota");
        assert_eq!(result.retry_after_secs, Some(2));
    }
}

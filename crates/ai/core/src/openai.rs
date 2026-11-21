//! OpenAI-compatible provider adapter.
//!
//! This is the reference adapter and the "passthrough lane" foundation: it
//! speaks the canonical IR internally and the OpenAI `/v1/chat/completions`
//! wire format on the outside. Non-stream responses are decoded into
//! [`CanonicalResponse`]; SSE chunks are decoded into [`StreamEvent`]s, so a
//! caller can forward text deltas while still accounting usage.
//!
//! It deliberately does **not** know about aliases or routing: the executor sets
//! `request.model` to the upstream model from the selected `ModelKey` before
//! calling the provider.

use crate::ir::{
    CanonicalRequest, CanonicalResponse, ContentPart, Delta, FinishReason, Message, Role,
    StreamEvent, ToolCall, ToolCallDelta,
};
use crate::provider::{CanonicalStream, ChatProvider, ProviderCapabilities, ProviderError};
use crate::usage::Usage;
use async_trait::async_trait;
use futures_util::stream::{Stream, StreamExt};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use webx_client::{HttpClientError, IHttpClient, OutboundRequest, OutboundResponse};
use webx_core::secrets::{ISecretStore, SecretError, SecretValue};

const DEFAULT_CHAT_PATH: &str = "/v1/chat/completions";
const MAX_JSON_BODY: usize = 8 * 1024 * 1024;
const MAX_ERROR_BODY: usize = 64 * 1024;

/// OpenAI-compatible provider.
pub struct OpenAiProvider {
    id: String,
    client: Arc<dyn IHttpClient>,
    api_key: SecretValue,
    chat_path: String,
    capabilities: ProviderCapabilities,
}

impl OpenAiProvider {
    /// Build an adapter for `id` using `client` and a bearer `api_key`.
    ///
    /// An empty key omits the authorization header (local vLLM/Ollama). The key
    /// is held in a [`SecretValue`] (redacted `Debug`, zeroized on drop); prefer
    /// [`OpenAiProvider::from_secret_store`] so the plaintext never appears in
    /// application code or config.
    pub fn new(
        id: impl Into<String>,
        client: Arc<dyn IHttpClient>,
        api_key: impl Into<SecretValue>,
    ) -> Self {
        Self {
            id: id.into(),
            client,
            api_key: api_key.into(),
            chat_path: DEFAULT_CHAT_PATH.to_string(),
            capabilities: ProviderCapabilities::chat_stream()
                .tools()
                .vision()
                .json_mode(),
        }
    }

    /// Resolve `name` from `store` and build the adapter in one step.
    ///
    /// ```ignore
    /// let provider = OpenAiProvider::from_secret_store(
    ///     "openai",
    ///     client,
    ///     &EnvSecretStore::new(),
    ///     "OPENAI_API_KEY",
    /// )
    /// .await?;
    /// ```
    pub async fn from_secret_store(
        id: impl Into<String>,
        client: Arc<dyn IHttpClient>,
        store: &dyn ISecretStore,
        name: impl AsRef<str>,
    ) -> Result<Self, SecretError> {
        let secret = store.get(name.as_ref()).await?;
        Ok(Self::new(id, client, secret))
    }

    /// Override the chat-completions path (or give a full URL).
    pub fn with_chat_path(mut self, path: impl Into<String>) -> Self {
        self.chat_path = path.into();
        self
    }

    /// Declare the provider's capabilities.
    pub fn with_capabilities(mut self, capabilities: ProviderCapabilities) -> Self {
        self.capabilities = capabilities;
        self
    }

    async fn send(&self, body: Value) -> Result<OutboundResponse, ProviderError> {
        let mut request = OutboundRequest::post(self.chat_path.clone())
            .json(&body)
            .map_err(|error| ProviderError::InvalidRequest(error.to_string()))?;
        if !self.api_key.is_empty() {
            request = request.header("authorization", format!("Bearer {}", self.api_key.expose()));
        }
        self.client.execute(request).await.map_err(client_error)
    }
}

#[async_trait]
impl ChatProvider for OpenAiProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn capabilities(&self) -> ProviderCapabilities {
        self.capabilities
    }

    async fn complete(
        &self,
        request: CanonicalRequest,
    ) -> Result<CanonicalResponse, ProviderError> {
        let body = encode_request(&request, false)?;
        let response = self.send(body).await?;
        if !(200..300).contains(&response.status) {
            return Err(error_from_response(response).await);
        }
        let bytes = response
            .body
            .collect(MAX_JSON_BODY)
            .await
            .map_err(client_error)?;
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|error| ProviderError::Protocol(format!("invalid JSON response: {error}")))?;
        decode_completion(&value, &request.model)
    }

    async fn stream(&self, request: CanonicalRequest) -> Result<CanonicalStream, ProviderError> {
        let body = encode_request(&request, true)?;
        let response = self.send(body).await?;
        if !(200..300).contains(&response.status) {
            return Err(error_from_response(response).await);
        }
        Ok(OpenAiStream::new(response.body.sse()).boxed())
    }
}

/// Encode a canonical request as an OpenAI chat-completions body.
fn encode_request(request: &CanonicalRequest, stream: bool) -> Result<Value, ProviderError> {
    if request.model.trim().is_empty() {
        return Err(ProviderError::InvalidRequest("model is required".into()));
    }
    if request.messages.is_empty() {
        return Err(ProviderError::InvalidRequest(
            "at least one message is required".into(),
        ));
    }

    let mut object = Map::new();
    object.insert("model".to_string(), json!(request.model));

    let messages = request
        .messages
        .iter()
        .map(encode_message)
        .collect::<Result<Vec<_>, _>>()?;
    object.insert("messages".to_string(), Value::Array(messages));

    if !request.tools.is_empty() {
        let tools = request
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "function": {
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.parameters,
                    }
                })
            })
            .collect::<Vec<_>>();
        object.insert("tools".to_string(), Value::Array(tools));
    }

    if let Some(temperature) = request.sampling.temperature {
        object.insert("temperature".to_string(), json!(temperature));
    }
    if let Some(top_p) = request.sampling.top_p {
        object.insert("top_p".to_string(), json!(top_p));
    }
    if let Some(max_tokens) = request.sampling.max_tokens {
        object.insert("max_tokens".to_string(), json!(max_tokens));
    }
    if !request.sampling.stop.is_empty() {
        object.insert("stop".to_string(), json!(request.sampling.stop));
    }
    if let Some(seed) = request.sampling.seed {
        object.insert("seed".to_string(), json!(seed));
    }

    if stream {
        object.insert("stream".to_string(), json!(true));
        object.insert(
            "stream_options".to_string(),
            json!({ "include_usage": true }),
        );
    }

    // Vendor extensions must never override a field the gateway set.
    for (key, value) in &request.extensions {
        if !object.contains_key(key) {
            object.insert(key.clone(), value.clone());
        }
    }

    Ok(Value::Object(object))
}

fn encode_message(message: &Message) -> Result<Value, ProviderError> {
    let mut object = Map::new();
    object.insert("role".to_string(), json!(role_name(message.role)));
    if let Some(name) = &message.name {
        object.insert("name".to_string(), json!(name));
    }

    let mut texts: Vec<String> = Vec::new();
    let mut images: Vec<Value> = Vec::new();
    let mut tool_calls: Vec<Value> = Vec::new();
    let mut tool_result: Option<(String, String)> = None;

    for part in &message.content {
        match part {
            ContentPart::Text(text) => texts.push(text.clone()),
            ContentPart::Image { url, detail } => {
                let mut image = json!({ "url": url });
                if let Some(detail) = detail {
                    image["detail"] = json!(detail);
                }
                images.push(json!({ "type": "image_url", "image_url": image }));
            }
            ContentPart::ToolCall(call) => tool_calls.push(json!({
                "id": call.id,
                "type": "function",
                "function": { "name": call.name, "arguments": call.arguments }
            })),
            ContentPart::ToolResult {
                tool_call_id,
                content,
            } => {
                if tool_result.is_some() {
                    return Err(ProviderError::InvalidRequest(
                        "a tool message can carry only one tool result".into(),
                    ));
                }
                tool_result = Some((tool_call_id.clone(), content.clone()));
            }
        }
    }

    match message.role {
        Role::Tool => {
            let Some((tool_call_id, content)) = tool_result else {
                return Err(ProviderError::InvalidRequest(
                    "a tool message requires a tool result".into(),
                ));
            };
            if !tool_calls.is_empty() || !images.is_empty() {
                return Err(ProviderError::InvalidRequest(
                    "a tool message cannot carry tool calls or images".into(),
                ));
            }
            object.insert("tool_call_id".to_string(), json!(tool_call_id));
            object.insert("content".to_string(), json!(content));
        }
        Role::Assistant => {
            if tool_result.is_some() || !images.is_empty() {
                return Err(ProviderError::InvalidRequest(
                    "an assistant message cannot carry tool results or images".into(),
                ));
            }
            if texts.is_empty() {
                object.insert(
                    "content".to_string(),
                    if tool_calls.is_empty() {
                        json!("")
                    } else {
                        Value::Null
                    },
                );
            } else {
                object.insert("content".to_string(), json!(texts.join("\n")));
            }
            if !tool_calls.is_empty() {
                object.insert("tool_calls".to_string(), Value::Array(tool_calls));
            }
        }
        Role::System | Role::User => {
            if tool_result.is_some() {
                return Err(ProviderError::InvalidRequest(
                    "tool results must use the tool role".into(),
                ));
            }
            if !tool_calls.is_empty() {
                return Err(ProviderError::InvalidRequest(
                    "tool calls must use the assistant role".into(),
                ));
            }
            if images.is_empty() {
                object.insert("content".to_string(), json!(texts.join("\n")));
            } else {
                let mut content: Vec<Value> = texts
                    .into_iter()
                    .map(|text| json!({ "type": "text", "text": text }))
                    .collect();
                content.extend(images);
                object.insert("content".to_string(), Value::Array(content));
            }
        }
    }

    Ok(Value::Object(object))
}

const fn role_name(role: Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

// ---------------------------------------------------------------------------
// Non-stream decoding
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct OpenAiResponse {
    id: Option<String>,
    model: Option<String>,
    #[serde(default)]
    choices: Vec<OpenAiChoice>,
    usage: Option<OpenAiUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoice {
    message: Option<OpenAiMessage>,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiMessage {
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<OpenAiToolCall>,
}

#[derive(Debug, Deserialize)]
struct OpenAiToolCall {
    id: Option<String>,
    function: Option<OpenAiFunction>,
}

#[derive(Debug, Deserialize, Default)]
struct OpenAiFunction {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct OpenAiUsage {
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
    prompt_tokens_details: Option<PromptDetails>,
    completion_tokens_details: Option<CompletionDetails>,
}

#[derive(Debug, Deserialize, Default)]
struct PromptDetails {
    cached_tokens: Option<u64>,
}

#[derive(Debug, Deserialize, Default)]
struct CompletionDetails {
    reasoning_tokens: Option<u64>,
}

fn decode_completion(
    value: &Value,
    fallback_model: &str,
) -> Result<CanonicalResponse, ProviderError> {
    let response: OpenAiResponse = serde_json::from_value(value.clone())
        .map_err(|error| ProviderError::Protocol(format!("invalid completion JSON: {error}")))?;
    let choice = response
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| ProviderError::Protocol("completion has no choices".into()))?;

    let mut parts = Vec::new();
    if let Some(message) = choice.message {
        if let Some(content) = message.content {
            if !content.is_empty() {
                parts.push(ContentPart::Text(content));
            }
        }
        for call in message.tool_calls {
            let function = call.function.unwrap_or_default();
            parts.push(ContentPart::ToolCall(ToolCall {
                id: call.id.unwrap_or_default(),
                name: function.name.unwrap_or_default(),
                arguments: function.arguments.unwrap_or_else(|| "{}".to_string()),
            }));
        }
    }
    if parts.is_empty() {
        parts.push(ContentPart::Text(String::new()));
    }

    Ok(CanonicalResponse {
        id: response.id.unwrap_or_default(),
        model: response.model.unwrap_or_else(|| fallback_model.to_string()),
        message: Message::new(Role::Assistant, parts),
        usage: decode_usage(response.usage),
        finish_reason: finish_reason(choice.finish_reason.as_deref()),
    })
}

fn decode_usage(usage: Option<OpenAiUsage>) -> Usage {
    let Some(usage) = usage else {
        return Usage::ZERO;
    };
    Usage {
        prompt_tokens: usage.prompt_tokens.unwrap_or(0),
        completion_tokens: usage.completion_tokens.unwrap_or(0),
        cached_tokens: usage
            .prompt_tokens_details
            .and_then(|details| details.cached_tokens)
            .unwrap_or(0),
        reasoning_tokens: usage
            .completion_tokens_details
            .and_then(|details| details.reasoning_tokens)
            .unwrap_or(0),
    }
}

fn finish_reason(reason: Option<&str>) -> FinishReason {
    match reason {
        Some("stop") | None => FinishReason::Stop,
        Some("length") => FinishReason::Length,
        Some("tool_calls") | Some("function_call") => FinishReason::ToolCalls,
        Some("content_filter") => FinishReason::ContentFilter,
        Some(other) => FinishReason::Other(other.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Stream decoding
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
struct OpenAiChunk {
    id: Option<String>,
    model: Option<String>,
    #[serde(default)]
    choices: Vec<OpenAiDeltaChoice>,
    usage: Option<OpenAiUsage>,
}

#[derive(Debug, Deserialize, Default)]
struct OpenAiDeltaChoice {
    #[serde(default)]
    delta: OpenAiDelta,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct OpenAiDelta {
    content: Option<String>,
    reasoning_content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<OpenAiToolCallDelta>,
}

#[derive(Debug, Deserialize, Default)]
struct OpenAiToolCallDelta {
    #[serde(default)]
    index: u32,
    id: Option<String>,
    function: Option<OpenAiFunction>,
}

fn decode_chunk(
    value: &Value,
    started: &mut bool,
) -> Result<(Vec<StreamEvent>, bool), ProviderError> {
    let chunk: OpenAiChunk = serde_json::from_value(value.clone())
        .map_err(|error| ProviderError::Protocol(format!("invalid stream chunk: {error}")))?;

    let mut events = Vec::new();
    if !*started && (chunk.id.is_some() || chunk.model.is_some()) {
        events.push(StreamEvent::Start {
            id: chunk.id.clone().unwrap_or_default(),
            model: chunk.model.clone().unwrap_or_default(),
        });
        *started = true;
    }

    let mut finished = false;
    for choice in chunk.choices {
        let tool_calls = choice
            .delta
            .tool_calls
            .into_iter()
            .map(|call| {
                let function = call.function.unwrap_or_default();
                ToolCallDelta {
                    index: call.index,
                    id: call.id,
                    name: function.name,
                    arguments_delta: function.arguments,
                }
            })
            .collect::<Vec<_>>();
        let delta = Delta {
            content: choice.delta.content,
            reasoning: choice.delta.reasoning_content,
            tool_calls,
        };
        if !delta.is_empty() {
            events.push(StreamEvent::Delta(delta));
        }
        if let Some(reason) = choice.finish_reason {
            events.push(StreamEvent::Done(finish_reason(Some(&reason))));
            finished = true;
        }
    }

    if let Some(usage) = chunk.usage {
        events.push(StreamEvent::Usage(decode_usage(Some(usage))));
    }

    Ok((events, finished))
}

/// Decodes one upstream SSE stream into canonical events.
struct OpenAiStream {
    source: webx_client::SseStream,
    pending: VecDeque<StreamEvent>,
    started: bool,
    saw_finish: bool,
    terminated: bool,
}

impl OpenAiStream {
    fn new(source: webx_client::SseStream) -> Self {
        Self {
            source,
            pending: VecDeque::new(),
            started: false,
            saw_finish: false,
            terminated: false,
        }
    }
}

impl Stream for OpenAiStream {
    type Item = Result<StreamEvent, ProviderError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            if let Some(event) = this.pending.pop_front() {
                return Poll::Ready(Some(Ok(event)));
            }
            if this.terminated {
                return Poll::Ready(None);
            }

            match Pin::new(&mut this.source).poll_next(cx) {
                Poll::Ready(Some(Ok(frame))) => {
                    if frame.data.as_ref() == b"[DONE]".as_slice() {
                        this.terminated = true;
                        if !this.saw_finish {
                            this.pending
                                .push_back(StreamEvent::Done(FinishReason::Stop));
                        }
                        continue;
                    }

                    let value: Value = match serde_json::from_slice(&frame.data) {
                        Ok(value) => value,
                        Err(error) => {
                            this.terminated = true;
                            return Poll::Ready(Some(Err(ProviderError::Protocol(format!(
                                "invalid stream JSON: {error}"
                            )))));
                        }
                    };
                    let (events, finished) = match decode_chunk(&value, &mut this.started) {
                        Ok(decoded) => decoded,
                        Err(error) => {
                            this.terminated = true;
                            return Poll::Ready(Some(Err(error)));
                        }
                    };
                    this.saw_finish |= finished;
                    this.pending.extend(events);
                }
                Poll::Ready(Some(Err(error))) => {
                    this.terminated = true;
                    return Poll::Ready(Some(Err(client_error(error))));
                }
                Poll::Ready(None) => {
                    this.terminated = true;
                    if !this.saw_finish {
                        this.pending
                            .push_back(StreamEvent::Done(FinishReason::Stop));
                    }
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Error mapping
// ---------------------------------------------------------------------------

async fn error_from_response(response: OutboundResponse) -> ProviderError {
    let status = response.status;
    let retry_after = response
        .headers
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs);
    let body = response
        .body
        .collect(MAX_ERROR_BODY)
        .await
        .unwrap_or_default();
    let message = extract_error_message(&body);
    classify_status(status, retry_after, message)
}

fn classify_status(status: u16, retry_after: Option<Duration>, message: String) -> ProviderError {
    match status {
        401 | 403 => ProviderError::Unauthorized,
        408 | 504 => ProviderError::Timeout,
        429 => ProviderError::RateLimited { retry_after },
        400..=499 => ProviderError::InvalidRequest(format!("{status}: {message}")),
        500..=599 => ProviderError::Unavailable(format!("{status}: {message}")),
        _ => ProviderError::Protocol(format!("unexpected status {status}: {message}")),
    }
}

fn extract_error_message(body: &[u8]) -> String {
    if let Ok(value) = serde_json::from_slice::<Value>(body) {
        if let Some(message) = value
            .get("error")
            .and_then(|error| error.get("message"))
            .and_then(|message| message.as_str())
        {
            return message.to_string();
        }
    }
    String::from_utf8_lossy(body).chars().take(256).collect()
}

fn client_error(error: HttpClientError) -> ProviderError {
    match error {
        HttpClientError::Timeout(_) => ProviderError::Timeout,
        HttpClientError::Connect(message)
        | HttpClientError::Protocol(message)
        | HttpClientError::Body(message)
        | HttpClientError::UnknownClient(message) => ProviderError::Unavailable(message),
        HttpClientError::InvalidRequest(message)
        | HttpClientError::InvalidUrl(message)
        | HttpClientError::BlockedAddress(message) => ProviderError::InvalidRequest(message),
        HttpClientError::BodyTooLarge { limit } => {
            ProviderError::Protocol(format!("response body exceeded {limit} bytes"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request() -> CanonicalRequest {
        CanonicalRequest::new("gpt-4o-mini").with_messages(vec![
            Message::text(Role::System, "be brief"),
            Message::text(Role::User, "hello"),
        ])
    }

    #[test]
    fn encodes_text_request() {
        let body = encode_request(&request(), false).expect("encodes");
        assert_eq!(body["model"], "gpt-4o-mini");
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["content"], "hello");
        assert!(body.get("stream").is_none());
    }

    struct FixedSecret(Option<&'static str>);

    #[async_trait::async_trait]
    impl ISecretStore for FixedSecret {
        async fn get(&self, name: &str) -> Result<SecretValue, SecretError> {
            match self.0 {
                Some(value) => Ok(SecretValue::new(value)),
                None => Err(SecretError::NotFound(name.to_string())),
            }
        }
    }

    #[tokio::test]
    async fn resolves_the_api_key_from_a_secret_store() {
        let client: Arc<dyn IHttpClient> = Arc::new(
            webx_client::HttpClient::new(
                "test",
                webx_client::HttpClientOptions::new("http://127.0.0.1:9"),
            )
            .expect("client"),
        );
        let provider = OpenAiProvider::from_secret_store(
            "openai",
            client,
            &FixedSecret(Some("sk-from-store")),
            "OPENAI_API_KEY",
        )
        .await
        .expect("secret resolves");
        assert_eq!(provider.api_key.expose(), "sk-from-store");
    }

    #[tokio::test]
    async fn a_missing_secret_is_a_typed_error() {
        let client: Arc<dyn IHttpClient> = Arc::new(
            webx_client::HttpClient::new(
                "test",
                webx_client::HttpClientOptions::new("http://127.0.0.1:9"),
            )
            .expect("client"),
        );
        let error = OpenAiProvider::from_secret_store(
            "openai",
            client,
            &FixedSecret(None),
            "OPENAI_API_KEY",
        )
        .await
        .err()
        .expect("missing secret");
        assert!(matches!(error, SecretError::NotFound(name) if name == "OPENAI_API_KEY"));
    }

    #[test]
    fn encodes_streaming_usage_request() {
        let body = encode_request(&request(), true).expect("encodes");
        assert_eq!(body["stream"], true);
        assert_eq!(body["stream_options"]["include_usage"], true);
    }

    #[test]
    fn encodes_multimodal_and_tool_messages() {
        let tool = crate::ir::ToolDef::new(
            "lookup",
            "look up",
            json!({"type": "object", "properties": {}}),
        );
        let request = CanonicalRequest::new("vision-model")
            .with_tools(vec![tool])
            .with_messages(vec![
                Message::new(
                    Role::User,
                    vec![
                        ContentPart::text("what is this?"),
                        ContentPart::image_with_detail("https://example.test/a.png", "high"),
                    ],
                ),
                Message::new(
                    Role::Assistant,
                    vec![
                        ContentPart::text("calling"),
                        ContentPart::ToolCall(ToolCall {
                            id: "call-1".into(),
                            name: "lookup".into(),
                            arguments: "{\"q\":\"x\"}".into(),
                        }),
                    ],
                ),
                Message::new(
                    Role::Tool,
                    vec![ContentPart::ToolResult {
                        tool_call_id: "call-1".into(),
                        content: "result".into(),
                    }],
                ),
            ]);

        let body = encode_request(&request, false).expect("encodes");
        assert_eq!(body["tools"][0]["function"]["name"], "lookup");
        assert_eq!(body["messages"][0]["content"][1]["type"], "image_url");
        assert_eq!(body["messages"][1]["tool_calls"][0]["id"], "call-1");
        assert_eq!(body["messages"][2]["role"], "tool");
        assert_eq!(body["messages"][2]["tool_call_id"], "call-1");
    }

    #[test]
    fn extensions_do_not_override_gateway_fields() {
        let request = request()
            .with_extension("model", json!("hijack"))
            .with_extension("custom", json!(1));
        let body = encode_request(&request, false).expect("encodes");
        assert_eq!(body["model"], "gpt-4o-mini");
        assert_eq!(body["custom"], 1);
    }

    #[test]
    fn decodes_completion_with_tool_calls_and_usage() {
        let value = json!({
            "id": "chatcmpl-1",
            "model": "gpt-4o-mini-2024",
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "call-1",
                        "type": "function",
                        "function": { "name": "lookup", "arguments": "{\"q\":\"x\"}" }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "prompt_tokens_details": { "cached_tokens": 2 },
                "completion_tokens_details": { "reasoning_tokens": 1 }
            }
        });

        let response = decode_completion(&value, "fallback").expect("decodes");
        assert_eq!(response.id, "chatcmpl-1");
        assert_eq!(response.model, "gpt-4o-mini-2024");
        assert_eq!(response.finish_reason, FinishReason::ToolCalls);
        assert_eq!(response.usage.prompt_tokens, 10);
        assert_eq!(response.usage.cached_tokens, 2);
        assert_eq!(response.usage.reasoning_tokens, 1);
        match &response.message.content[0] {
            ContentPart::ToolCall(call) => {
                assert_eq!(call.id, "call-1");
                assert_eq!(call.name, "lookup");
                assert_eq!(call.arguments, "{\"q\":\"x\"}");
            }
            other => panic!("expected tool call, got {other:?}"),
        }
    }

    #[test]
    fn decodes_stream_chunks_into_start_delta_usage_done() {
        let mut started = false;
        let (events, finished) = decode_chunk(
            &json!({
                "id": "chatcmpl-1",
                "model": "gpt-4o-mini",
                "choices": [{ "delta": { "content": "hel" }, "finish_reason": null }]
            }),
            &mut started,
        )
        .expect("decodes");
        assert!(started);
        assert!(!finished);
        assert!(matches!(events[0], StreamEvent::Start { .. }));
        match &events[1] {
            StreamEvent::Delta(delta) => assert_eq!(delta.content.as_deref(), Some("hel")),
            other => panic!("expected delta, got {other:?}"),
        }

        let (events, finished) = decode_chunk(
            &json!({
                "choices": [{ "delta": {}, "finish_reason": "stop" }],
                "usage": { "prompt_tokens": 3, "completion_tokens": 2 }
            }),
            &mut started,
        )
        .expect("decodes");
        assert!(finished);
        assert!(matches!(events[0], StreamEvent::Done(FinishReason::Stop)));
        match &events[1] {
            StreamEvent::Usage(usage) => assert_eq!(usage.total_tokens(), 5),
            other => panic!("expected usage, got {other:?}"),
        }
    }

    #[test]
    fn decodes_tool_call_delta() {
        let mut started = true;
        let (events, _) = decode_chunk(
            &json!({
                "choices": [{
                    "delta": {
                        "tool_calls": [{
                            "index": 0,
                            "id": "call-1",
                            "function": { "name": "lookup", "arguments": "{\"q\"" }
                        }]
                    },
                    "finish_reason": null
                }]
            }),
            &mut started,
        )
        .expect("decodes");

        match &events[0] {
            StreamEvent::Delta(delta) => {
                let call = &delta.tool_calls[0];
                assert_eq!(call.index, 0);
                assert_eq!(call.id.as_deref(), Some("call-1"));
                assert_eq!(call.name.as_deref(), Some("lookup"));
                assert_eq!(call.arguments_delta.as_deref(), Some("{\"q\""));
            }
            other => panic!("expected tool delta, got {other:?}"),
        }
    }

    #[test]
    fn maps_http_statuses_to_provider_errors() {
        assert!(matches!(
            classify_status(401, None, String::new()),
            ProviderError::Unauthorized
        ));
        assert!(matches!(
            classify_status(429, Some(Duration::from_secs(3)), String::new()),
            ProviderError::RateLimited {
                retry_after: Some(_)
            }
        ));
        assert!(matches!(
            classify_status(504, None, String::new()),
            ProviderError::Timeout
        ));
        assert!(matches!(
            classify_status(503, None, "down".into()),
            ProviderError::Unavailable(_)
        ));
        assert!(matches!(
            classify_status(400, None, "bad".into()),
            ProviderError::InvalidRequest(_)
        ));
    }

    #[test]
    fn extracts_openai_error_message() {
        let body = br#"{"error":{"message":"model not found","type":"invalid_request_error"}}"#;
        assert_eq!(extract_error_message(body), "model not found");
        assert_eq!(extract_error_message(b"plain text"), "plain text");
    }
}

//! OpenAI `/v1/chat/completions` request DTO and conversion to the canonical IR.

use serde::Deserialize;
use serde_json::{Map, Value};
use webx::IdentityUser;
use webx::WebxRequestMeta;
use webx_ai::{CanonicalRequest, ContentPart, Message, Role, SamplingParams, ToolCall, ToolDef};

/// Request-shape ceilings. These are defense-in-depth before a request reaches
/// tokenization/quota: a hostile client cannot force unbounded allocation or a
/// multi-million-token upstream bill with one JSON body.
const MAX_MESSAGES: usize = 512;
const MAX_TOOLS: usize = 128;
const MAX_STOP_SEQUENCES: usize = 16;
const MAX_MAX_TOKENS: u32 = 200_000;
const MAX_TEXT_LEN: usize = 2_000_000;

/// Stop sequence: a single string or an array of strings.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum StopValue {
    /// One stop sequence.
    Single(String),
    /// Multiple stop sequences.
    Multiple(Vec<String>),
}

/// One message in an OpenAI request.
#[derive(Debug, Clone, Deserialize)]
pub struct OpenAiMessage {
    /// `system` / `user` / `assistant` / `tool`.
    pub role: String,
    /// String, multimodal content array, or null.
    #[serde(default)]
    pub content: Option<Value>,
    /// Optional participant name.
    #[serde(default)]
    pub name: Option<String>,
    /// Assistant tool calls.
    #[serde(default)]
    pub tool_calls: Vec<OpenAiToolCall>,
    /// Tool-result correlation id.
    #[serde(default)]
    pub tool_call_id: Option<String>,
}

/// A tool call emitted by the model.
#[derive(Debug, Clone, Deserialize)]
pub struct OpenAiToolCall {
    /// Tool-call id.
    #[serde(default)]
    pub id: String,
    /// Function name + JSON arguments.
    #[serde(default)]
    pub function: OpenAiFunction,
}

/// Function name and arguments.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct OpenAiFunction {
    /// Function name.
    #[serde(default)]
    pub name: String,
    /// JSON-encoded arguments string.
    #[serde(default)]
    pub arguments: String,
}

/// A tool definition sent by the client.
#[derive(Debug, Clone, Deserialize)]
pub struct OpenAiTool {
    /// Usually `"function"`.
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    /// Function schema.
    pub function: OpenAiToolDefinition,
}

/// Function tool schema.
#[derive(Debug, Clone, Deserialize)]
pub struct OpenAiToolDefinition {
    /// Function name.
    pub name: String,
    /// Description.
    #[serde(default)]
    pub description: String,
    /// JSON schema.
    #[serde(default)]
    pub parameters: Value,
}

/// OpenAI-compatible chat completion request.
#[derive(Debug, Clone, Default, Deserialize, WebxRequestMeta)]
pub struct OpenAiChatRequest {
    /// Public model/alias.
    #[frombody]
    pub model: String,
    /// Conversation.
    #[frombody]
    pub messages: Vec<OpenAiMessage>,
    /// Whether to stream the response.
    #[frombody]
    pub stream: Option<bool>,
    /// Sampling temperature.
    #[frombody]
    pub temperature: Option<f32>,
    /// Nucleus sampling.
    #[frombody]
    pub top_p: Option<f32>,
    /// Maximum output tokens.
    #[frombody]
    pub max_tokens: Option<u32>,
    /// Stop sequence(s).
    #[frombody]
    pub stop: Option<StopValue>,
    /// Available tools.
    #[frombody]
    pub tools: Option<Vec<OpenAiTool>>,
    /// Tool-choice policy, forwarded as an extension.
    #[frombody]
    pub tool_choice: Option<Value>,
    /// Response-format policy, forwarded as an extension.
    #[frombody]
    pub response_format: Option<Value>,
    /// Deterministic seed.
    #[frombody]
    pub seed: Option<u64>,
    /// Return a token/cost estimate without calling the provider.
    #[frombody]
    pub dry_run: Option<bool>,
    /// Unknown fields preserved for provider adapters.
    #[serde(flatten)]
    pub extensions: Map<String, Value>,
    /// Authenticated principal (API key/JWT).
    #[fromclaim]
    #[serde(skip)]
    pub user: IdentityUser,
    /// Optional gateway request id.
    #[fromheader("x-request-id")]
    #[serde(skip)]
    pub request_id: Option<String>,
}

impl TryFrom<OpenAiChatRequest> for CanonicalRequest {
    type Error = String;

    fn try_from(request: OpenAiChatRequest) -> Result<Self, Self::Error> {
        let OpenAiChatRequest {
            model,
            messages,
            stream,
            temperature,
            top_p,
            max_tokens,
            stop,
            tools,
            tool_choice,
            response_format,
            seed,
            extensions,
            ..
        } = request;

        if model.trim().is_empty() {
            return Err("model is required".into());
        }
        if model.len() > 512 {
            return Err("model name is too long".into());
        }
        if messages.is_empty() {
            return Err("messages must not be empty".into());
        }
        if messages.len() > MAX_MESSAGES {
            return Err(format!("too many messages (max {MAX_MESSAGES})"));
        }
        if let Some(limit) = max_tokens {
            if limit == 0 || limit > MAX_MAX_TOKENS {
                return Err(format!("max_tokens out of range (1..={MAX_MAX_TOKENS})"));
            }
        }
        if tools.as_ref().is_some_and(|t| t.len() > MAX_TOOLS) {
            return Err(format!("too many tools (max {MAX_TOOLS})"));
        }

        let messages = messages
            .into_iter()
            .map(convert_message)
            .collect::<Result<Vec<_>, _>>()?;
        // Apply the text ceiling to the canonical messages, not only to the
        // tool-message path: user/assistant/system content, image URLs, tool
        // call arguments and names all reach tokenization/quota and must be
        // bounded. One check here covers every conversion path uniformly.
        for message in &messages {
            validate_message_limits(message)?;
        }
        let tools = tools
            .unwrap_or_default()
            .into_iter()
            .map(convert_tool)
            .collect::<Result<Vec<_>, _>>()?;
        let stop = stop
            .map(|stop| match stop {
                StopValue::Single(one) => vec![one],
                StopValue::Multiple(many) => many,
            })
            .unwrap_or_default();
        if stop.len() > MAX_STOP_SEQUENCES {
            return Err(format!("too many stop sequences (max {MAX_STOP_SEQUENCES})"));
        }
        if stop.iter().any(|s| s.len() > MAX_TEXT_LEN) {
            return Err("stop sequence is too long".into());
        }

        let mut canonical = CanonicalRequest {
            model,
            messages,
            tools,
            sampling: SamplingParams {
                temperature,
                top_p,
                max_tokens,
                stop,
                seed,
            },
            stream: stream.unwrap_or(false),
            extensions,
        };
        if let Some(tool_choice) = tool_choice {
            canonical
                .extensions
                .insert("tool_choice".into(), tool_choice);
        }
        if let Some(response_format) = response_format {
            canonical
                .extensions
                .insert("response_format".into(), response_format);
        }
        Ok(canonical)
    }
}

fn convert_tool(tool: OpenAiTool) -> Result<ToolDef, String> {
    if let Some(kind) = &tool.kind {
        if kind != "function" {
            return Err(format!("unsupported tool type: {kind}"));
        }
    }
    if tool.function.name.is_empty() {
        return Err("tool function name is required".into());
    }
    Ok(ToolDef::new(
        tool.function.name,
        tool.function.description,
        tool.function.parameters,
    ))
}

fn convert_message(message: OpenAiMessage) -> Result<Message, String> {
    let role = match message.role.as_str() {
        "system" => Role::System,
        "user" => Role::User,
        "assistant" => Role::Assistant,
        "tool" => Role::Tool,
        other => return Err(format!("unsupported role: {other}")),
    };

    match role {
        Role::Tool => {
            let tool_call_id = message
                .tool_call_id
                .ok_or_else(|| "tool message requires tool_call_id".to_string())?;
            if !message.tool_calls.is_empty() {
                return Err("tool message cannot carry tool_calls".into());
            }
            let content = content_as_text(message.content)?;
            let mut converted = Message::new(
                Role::Tool,
                vec![ContentPart::ToolResult {
                    tool_call_id,
                    content,
                }],
            );
            if let Some(name) = message.name {
                converted = converted.with_name(name);
            }
            Ok(converted)
        }
        Role::Assistant => {
            let mut parts = content_parts(message.content)?;
            for call in message.tool_calls {
                parts.push(ContentPart::ToolCall(ToolCall {
                    id: call.id,
                    name: call.function.name,
                    arguments: call.function.arguments,
                }));
            }
            let mut converted = Message::new(Role::Assistant, parts);
            if let Some(name) = message.name {
                converted = converted.with_name(name);
            }
            Ok(converted)
        }
        Role::System | Role::User => {
            if !message.tool_calls.is_empty() {
                return Err("tool_calls are only valid on assistant messages".into());
            }
            let mut converted = Message::new(role, content_parts(message.content)?);
            if let Some(name) = message.name {
                converted = converted.with_name(name);
            }
            Ok(converted)
        }
    }
}

/// Enforce [`MAX_TEXT_LEN`] on every caller-supplied string in a converted
/// message. Called on the canonical form so no conversion path (string content,
/// multimodal parts, tool calls, names) can bypass the ceiling.
fn validate_message_limits(message: &Message) -> Result<(), String> {
    let too_long = |text: &str| text.len() > MAX_TEXT_LEN;
    if message.name.as_ref().is_some_and(|name| too_long(name)) {
        return Err("message name is too long".into());
    }
    for part in &message.content {
        match part {
            ContentPart::Text(text) => {
                if too_long(text) {
                    return Err("message content is too long".into());
                }
            }
            ContentPart::Image { url, detail } => {
                if too_long(url) {
                    return Err("image url is too long".into());
                }
                if detail.as_ref().is_some_and(|value| too_long(value)) {
                    return Err("image detail is too long".into());
                }
            }
            ContentPart::ToolCall(call) => {
                if too_long(&call.id) || too_long(&call.name) || too_long(&call.arguments) {
                    return Err("tool call field is too long".into());
                }
            }
            ContentPart::ToolResult {
                tool_call_id,
                content,
            } => {
                if too_long(tool_call_id) || too_long(content) {
                    return Err("tool result field is too long".into());
                }
            }
        }
    }
    Ok(())
}

fn content_parts(content: Option<Value>) -> Result<Vec<ContentPart>, String> {
    match content {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::String(text)) => Ok(vec![ContentPart::Text(text)]),
        Some(Value::Array(parts)) => {
            let mut converted = Vec::with_capacity(parts.len());
            for part in parts {
                converted.push(convert_content_part(part)?);
            }
            Ok(converted)
        }
        Some(other) => Err(format!("content must be a string or an array, got {other}")),
    }
}

fn convert_content_part(part: Value) -> Result<ContentPart, String> {
    let kind = part
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| "content part requires a type".to_string())?;
    match kind {
        "text" => {
            let text = part
                .get("text")
                .and_then(Value::as_str)
                .ok_or_else(|| "text content part requires text".to_string())?;
            Ok(ContentPart::Text(text.to_string()))
        }
        "image_url" => {
            let image = part
                .get("image_url")
                .ok_or_else(|| "image_url content part requires image_url".to_string())?;
            let url = image
                .get("url")
                .and_then(Value::as_str)
                .ok_or_else(|| "image_url.url is required".to_string())?;
            let detail = image
                .get("detail")
                .and_then(Value::as_str)
                .map(str::to_string);
            Ok(ContentPart::Image {
                url: url.to_string(),
                detail,
            })
        }
        other => Err(format!("unsupported content part type: {other}")),
    }
}

fn content_as_text(content: Option<Value>) -> Result<String, String> {
    let check = |text: String| {
        if text.len() > MAX_TEXT_LEN {
            Err("message content is too long".to_string())
        } else {
            Ok(text)
        }
    };
    match content {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(text)) => check(text),
        Some(Value::Array(parts)) => {
            let mut text = String::new();
            for part in parts {
                match convert_content_part(part)? {
                    ContentPart::Text(part) => {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str(&part);
                    }
                    _ => return Err("tool content must be text".into()),
                }
            }
            check(text)
        }
        Some(other) => Err(format!("tool content must be text, got {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use webx::IdentityUser;

    fn base_message() -> OpenAiMessage {
        OpenAiMessage {
            role: "user".into(),
            content: Some(json!("hello")),
            name: None,
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    fn base_request() -> OpenAiChatRequest {
        OpenAiChatRequest {
            model: "fast".into(),
            messages: vec![base_message()],
            user: IdentityUser {
                id: "key-1".into(),
                tenant_id: "tenant-1".into(),
                ..IdentityUser::default()
            },
            ..OpenAiChatRequest::default()
        }
    }

    #[test]
    fn converts_text_messages_and_sampling() {
        let request = OpenAiChatRequest {
            temperature: Some(0.2),
            max_tokens: Some(64),
            stop: Some(StopValue::Single("\n".into())),
            ..base_request()
        };
        let canonical = CanonicalRequest::try_from(request).expect("converts");
        assert_eq!(canonical.model, "fast");
        assert_eq!(canonical.messages.len(), 1);
        assert_eq!(
            canonical.messages[0].content[0],
            ContentPart::Text("hello".into())
        );
        assert_eq!(canonical.sampling.temperature, Some(0.2));
        assert_eq!(canonical.sampling.max_tokens, Some(64));
        assert_eq!(canonical.sampling.stop, vec!["\n"]);
    }

    #[test]
    fn converts_multimodal_tools_and_tool_results() {
        let request = OpenAiChatRequest {
            tools: Some(vec![OpenAiTool {
                kind: Some("function".into()),
                function: OpenAiToolDefinition {
                    name: "lookup".into(),
                    description: "look up".into(),
                    parameters: json!({"type": "object"}),
                },
            }]),
            messages: vec![
                OpenAiMessage {
                    role: "user".into(),
                    content: Some(json!([
                        { "type": "text", "text": "what is this?" },
                        { "type": "image_url", "image_url": { "url": "https://example.test/a.png", "detail": "high" } }
                    ])),
                    name: None,
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                },
                OpenAiMessage {
                    role: "assistant".into(),
                    content: Some(json!("calling")),
                    name: None,
                    tool_calls: vec![OpenAiToolCall {
                        id: "call-1".into(),
                        function: OpenAiFunction {
                            name: "lookup".into(),
                            arguments: "{\"q\":\"x\"}".into(),
                        },
                    }],
                    tool_call_id: None,
                },
                OpenAiMessage {
                    role: "tool".into(),
                    content: Some(json!("result")),
                    name: None,
                    tool_calls: Vec::new(),
                    tool_call_id: Some("call-1".into()),
                },
            ],
            ..base_request()
        };

        let canonical = CanonicalRequest::try_from(request).expect("converts");
        assert_eq!(canonical.tools[0].name, "lookup");
        assert!(matches!(
            canonical.messages[0].content[1],
            ContentPart::Image { .. }
        ));
        assert!(matches!(
            canonical.messages[1].content[1],
            ContentPart::ToolCall(_)
        ));
        assert!(matches!(
            canonical.messages[2].content[0],
            ContentPart::ToolResult { .. }
        ));
    }

    #[test]
    fn rejects_unknown_roles_and_empty_models() {
        let mut request = base_request();
        request.messages[0].role = "robot".into();
        assert!(CanonicalRequest::try_from(request).is_err());

        let mut request = base_request();
        request.model.clear();
        assert!(CanonicalRequest::try_from(request).is_err());
    }

    #[test]
    fn forwards_tool_choice_and_unknown_fields() {
        let mut request = base_request();
        request.tool_choice = Some(json!("auto"));
        request.extensions.insert("custom".into(), json!(1));
        let canonical = CanonicalRequest::try_from(request).expect("converts");
        assert_eq!(canonical.extensions["tool_choice"], json!("auto"));
        assert_eq!(canonical.extensions["custom"], json!(1));
    }

    #[test]
    fn rejects_oversized_requests_before_conversion() {
        let request = OpenAiChatRequest {
            max_tokens: Some(u32::MAX),
            ..base_request()
        };
        assert!(CanonicalRequest::try_from(request).is_err(), "max_tokens must be bounded");

        let request = OpenAiChatRequest {
            messages: vec![
                OpenAiMessage {
                    role: "user".into(),
                    content: Some(json!("hi")),
                    name: None,
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                };
                MAX_MESSAGES + 1
            ],
            ..base_request()
        };
        assert!(CanonicalRequest::try_from(request).is_err(), "message count must be bounded");
    }

    #[test]
    fn text_ceiling_covers_every_message_path() {
        let oversized = "x".repeat(MAX_TEXT_LEN + 1);

        // Plain user content (the path that previously bypassed the ceiling).
        let request = OpenAiChatRequest {
            messages: vec![OpenAiMessage {
                role: "user".into(),
                content: Some(json!(oversized.clone())),
                ..base_message()
            }],
            ..base_request()
        };
        assert!(CanonicalRequest::try_from(request).is_err(), "user text must be bounded");

        // Assistant tool-call arguments.
        let request = OpenAiChatRequest {
            messages: vec![OpenAiMessage {
                role: "assistant".into(),
                content: Some(json!("ok")),
                tool_calls: vec![OpenAiToolCall {
                    id: "call-1".into(),
                    function: OpenAiFunction {
                        name: "f".into(),
                        arguments: oversized.clone(),
                    },
                }],
                ..base_message()
            }],
            ..base_request()
        };
        assert!(CanonicalRequest::try_from(request).is_err(), "tool arguments must be bounded");

        // Multimodal image URL.
        let image = json!([{
            "type": "image_url",
            "image_url": { "url": oversized }
        }]);
        let request = OpenAiChatRequest {
            messages: vec![OpenAiMessage {
                role: "user".into(),
                content: Some(image),
                ..base_message()
            }],
            ..base_request()
        };
        assert!(CanonicalRequest::try_from(request).is_err(), "image url must be bounded");
    }
}

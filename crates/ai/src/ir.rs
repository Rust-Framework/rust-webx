//! Provider-neutral request/response/stream types.
//!
//! The gateway speaks one canonical dialect internally: OpenAI-compatible input
//! is decoded into these types, a provider adapter translates them to its vendor
//! wire format, and the result is translated back. Keeping this layer neutral is
//! what lets one route fall back across providers without the data plane
//! caring which vendor served the request.
//!
//! Unknown vendor fields travel in `extensions`; adapters should preserve and
//! re-emit them when the upstream understands them.

use crate::provider::ProviderError;
use crate::usage::Usage;
use serde::Serialize;

/// Conversation role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Role {
    /// System/instructions message.
    System,
    /// End-user message.
    User,
    /// Model output (including tool calls).
    Assistant,
    /// Tool result fed back to the model.
    Tool,
}

/// One content block.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum ContentPart {
    /// Plain text.
    Text(String),
    /// Image input by URL (or a data URI).
    Image {
        /// Image URL or `data:` URI.
        url: String,
        /// Optional detail hint (`"low"`/`"high"`/`"auto"`).
        detail: Option<String>,
    },
    /// A tool call emitted by the model.
    ToolCall(ToolCall),
    /// A tool result supplied by the caller.
    ToolResult {
        /// Correlates with [`ToolCall::id`].
        tool_call_id: String,
        /// Serialized result content.
        content: String,
    },
}

impl ContentPart {
    /// A text block.
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text(text.into())
    }

    /// An image block without a detail hint.
    pub fn image(url: impl Into<String>) -> Self {
        Self::Image {
            url: url.into(),
            detail: None,
        }
    }

    /// An image block with a detail hint.
    pub fn image_with_detail(url: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::Image {
            url: url.into(),
            detail: Some(detail.into()),
        }
    }
}

/// A model-emitted tool call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolCall {
    /// Provider/tool-call id, echoed by the tool result.
    pub id: String,
    /// Function name.
    pub name: String,
    /// JSON-encoded arguments (a string, not a parsed object).
    pub arguments: String,
}

/// One conversation message.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Message {
    /// Who produced the message.
    pub role: Role,
    /// Ordered content blocks.
    pub content: Vec<ContentPart>,
    /// Optional participant name.
    pub name: Option<String>,
}

impl Message {
    /// A single-text-block message.
    pub fn text(role: Role, text: impl Into<String>) -> Self {
        Self {
            role,
            content: vec![ContentPart::Text(text.into())],
            name: None,
        }
    }

    /// A message with explicit content blocks.
    pub fn new(role: Role, content: Vec<ContentPart>) -> Self {
        Self {
            role,
            content,
            name: None,
        }
    }

    /// Set the participant name.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }
}

/// A tool/function definition.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ToolDef {
    /// Function name.
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// JSON Schema for the arguments object.
    pub parameters: serde_json::Value,
}

impl ToolDef {
    /// Build a tool definition.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: serde_json::Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
        }
    }
}

/// Sampling parameters common to chat providers.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SamplingParams {
    /// Sampling temperature.
    pub temperature: Option<f32>,
    /// Nucleus sampling probability.
    pub top_p: Option<f32>,
    /// Maximum output tokens.
    pub max_tokens: Option<u32>,
    /// Stop sequences.
    pub stop: Vec<String>,
    /// Deterministic seed, when supported.
    pub seed: Option<u64>,
}

/// One canonical chat request.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CanonicalRequest {
    /// Public model/alias the caller asked for.
    pub model: String,
    /// Conversation history.
    pub messages: Vec<Message>,
    /// Available tools.
    pub tools: Vec<ToolDef>,
    /// Sampling parameters.
    pub sampling: SamplingParams,
    /// Whether the caller wants a streamed response.
    pub stream: bool,
    /// Vendor-specific fields preserved by adapters.
    pub extensions: serde_json::Map<String, serde_json::Value>,
}

impl CanonicalRequest {
    /// A request with an empty conversation.
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            messages: Vec::new(),
            tools: Vec::new(),
            sampling: SamplingParams::default(),
            stream: false,
            extensions: serde_json::Map::new(),
        }
    }

    /// Set the conversation.
    pub fn with_messages(mut self, messages: Vec<Message>) -> Self {
        self.messages = messages;
        self
    }

    /// Set the available tools.
    pub fn with_tools(mut self, tools: Vec<ToolDef>) -> Self {
        self.tools = tools;
        self
    }

    /// Set sampling parameters.
    pub fn with_sampling(mut self, sampling: SamplingParams) -> Self {
        self.sampling = sampling;
        self
    }

    /// Mark the request as streamed.
    pub fn stream(mut self) -> Self {
        self.stream = true;
        self
    }

    /// Preserve a vendor extension field.
    pub fn with_extension(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.extensions.insert(key.into(), value);
        self
    }
}

/// Why generation stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FinishReason {
    /// Natural end of turn.
    Stop,
    /// Output token limit reached.
    Length,
    /// The model requested tool calls.
    ToolCalls,
    /// A provider content filter stopped generation.
    ContentFilter,
    /// Vendor-specific reason.
    Other(String),
}

/// Incremental tool-call fragment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolCallDelta {
    /// Index of the tool call in the assistant message.
    pub index: u32,
    /// Tool-call id, present on the first fragment.
    pub id: Option<String>,
    /// Function name, present on the first fragment.
    pub name: Option<String>,
    /// JSON argument fragment.
    pub arguments_delta: Option<String>,
}

/// Incremental assistant output.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Delta {
    /// Text fragment.
    pub content: Option<String>,
    /// Reasoning fragment, when the vendor exposes it.
    pub reasoning: Option<String>,
    /// Tool-call fragments.
    pub tool_calls: Vec<ToolCallDelta>,
}

impl Delta {
    /// A text delta.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: Some(text.into()),
            ..Self::default()
        }
    }

    /// Whether this delta carries nothing.
    pub fn is_empty(&self) -> bool {
        self.content.is_none() && self.reasoning.is_none() && self.tool_calls.is_empty()
    }
}

/// One event in a canonical stream.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    /// Stream metadata.
    Start {
        /// Upstream response id.
        id: String,
        /// Upstream model name.
        model: String,
    },
    /// Incremental output.
    Delta(Delta),
    /// Final (or interim) token usage.
    Usage(Usage),
    /// Generation finished.
    Done(FinishReason),
    /// The provider failed mid-stream.
    Error(ProviderError),
}

/// A completed non-streamed response.
#[derive(Debug, Clone, PartialEq)]
pub struct CanonicalResponse {
    /// Upstream response id.
    pub id: String,
    /// Upstream model name.
    pub model: String,
    /// Assistant message (text and/or tool calls).
    pub message: Message,
    /// Token usage.
    pub usage: Usage,
    /// Why generation stopped.
    pub finish_reason: FinishReason,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_message_has_one_text_part() {
        let message = Message::text(Role::User, "hello").with_name("alice");
        assert_eq!(message.role, Role::User);
        assert_eq!(message.name.as_deref(), Some("alice"));
        assert_eq!(message.content, vec![ContentPart::Text("hello".into())]);
    }

    #[test]
    fn request_builders_preserve_messages_tools_and_extensions() {
        let tool = ToolDef::new(
            "lookup",
            "look up a record",
            serde_json::json!({"type": "object"}),
        );
        let request = CanonicalRequest::new("fast")
            .with_messages(vec![Message::text(Role::System, "be brief")])
            .with_tools(vec![tool.clone()])
            .with_sampling(SamplingParams {
                temperature: Some(0.2),
                max_tokens: Some(64),
                ..SamplingParams::default()
            })
            .with_extension("vendor_flag", serde_json::json!(true))
            .stream();

        assert_eq!(request.model, "fast");
        assert!(request.stream);
        assert_eq!(request.messages.len(), 1);
        assert_eq!(request.tools, vec![tool]);
        assert_eq!(request.sampling.max_tokens, Some(64));
        assert_eq!(request.extensions["vendor_flag"], serde_json::json!(true));
    }

    #[test]
    fn content_part_helpers_build_expected_blocks() {
        assert_eq!(ContentPart::text("x"), ContentPart::Text("x".into()));
        assert_eq!(
            ContentPart::image("https://example.test/a.png"),
            ContentPart::Image {
                url: "https://example.test/a.png".into(),
                detail: None
            }
        );
        assert_eq!(
            ContentPart::image_with_detail("https://example.test/a.png", "high"),
            ContentPart::Image {
                url: "https://example.test/a.png".into(),
                detail: Some("high".into())
            }
        );
    }

    #[test]
    fn delta_emptiness_tracks_every_field() {
        assert!(Delta::default().is_empty());
        assert!(!Delta::text("x").is_empty());
        assert!(!Delta {
            reasoning: Some("thinking".into()),
            ..Delta::default()
        }
        .is_empty());
        assert!(!Delta {
            tool_calls: vec![ToolCallDelta {
                index: 0,
                id: Some("call-1".into()),
                name: Some("lookup".into()),
                arguments_delta: Some("{".into()),
            }],
            ..Delta::default()
        }
        .is_empty());
    }

    #[test]
    fn stream_events_carry_provider_errors() {
        let event = StreamEvent::Error(ProviderError::RateLimited {
            retry_after: Some(std::time::Duration::from_secs(2)),
        });
        match event {
            StreamEvent::Error(error) => {
                assert!(error.is_retryable());
                assert_eq!(error.http_status(), 429);
            }
            _ => panic!("expected an error event"),
        }
    }
}

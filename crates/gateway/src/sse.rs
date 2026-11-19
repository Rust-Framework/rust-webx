//! Canonical stream ->OpenAI SSE chunks.
//!
//! [`OpenAiSseEncoder`] adapts a [`GatewayStream`](webx_ai::GatewayStream) (or
//! any stream of canonical events) to the SSE frames an OpenAI SDK expects:
//! `chat.completion.chunk` events, a final `finish_reason`, optional usage, and
//! the `[DONE]` sentinel.

use futures_util::Stream;
use serde_json::{json, Map, Value};
use std::collections::VecDeque;
use std::pin::Pin;
use std::task::{Context, Poll};
use webx::SseEvent;
use webx_ai::{FinishReason, GatewayError, OpenAiError, StreamEvent};

use crate::response::{finish_reason, usage_json};

/// Adapts canonical stream events to OpenAI SSE frames.
pub struct OpenAiSseEncoder<S> {
    inner: S,
    model: String,
    id: String,
    created: u64,
    pending: VecDeque<SseEvent>,
    terminated: bool,
}

impl<S> OpenAiSseEncoder<S>
where
    S: Stream<Item = Result<StreamEvent, GatewayError>> + Unpin,
{
    /// Wrap a canonical stream for the public `model` alias.
    pub fn new(inner: S, model: impl Into<String>) -> Self {
        Self {
            inner,
            model: model.into(),
            id: "chatcmpl".to_string(),
            created: unix_secs(),
            pending: VecDeque::new(),
            terminated: false,
        }
    }

    fn chunk(&self, choices: Value, usage: Option<Value>) -> SseEvent {
        let mut value = json!({
            "id": self.id,
            "object": "chat.completion.chunk",
            "created": self.created,
            "model": self.model,
            "choices": choices,
        });
        if let Some(usage) = usage {
            value["usage"] = usage;
        }
        SseEvent::data(serde_json::to_string(&value).unwrap_or_default())
    }

    fn delta_chunk(&self, delta: Value) -> SseEvent {
        self.chunk(
            json!([{ "index": 0, "delta": delta, "finish_reason": null }]),
            None,
        )
    }

    fn finish_chunk(&self, reason: &FinishReason) -> SseEvent {
        self.chunk(
            json!([{ "index": 0, "delta": {}, "finish_reason": finish_reason(reason) }]),
            None,
        )
    }

    fn usage_chunk(&self, usage: webx_ai::Usage) -> SseEvent {
        self.chunk(json!([]), Some(usage_json(usage)))
    }

    fn error_chunk(error: &GatewayError) -> SseEvent {
        let status = error.http_status();
        let message = if status >= 500 {
            "an unexpected error occurred".to_string()
        } else {
            error.to_string()
        };
        let body = OpenAiError::from_status(status, message);
        SseEvent::data(serde_json::to_string(&body).unwrap_or_default())
    }

    fn done() -> SseEvent {
        SseEvent::data("[DONE]")
    }
}

impl<S> Stream for OpenAiSseEncoder<S>
where
    S: Stream<Item = Result<StreamEvent, GatewayError>> + Unpin,
{
    type Item = Result<SseEvent, webx::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            if let Some(event) = this.pending.pop_front() {
                return Poll::Ready(Some(Ok(event)));
            }
            if this.terminated {
                return Poll::Ready(None);
            }

            match Pin::new(&mut this.inner).poll_next(cx) {
                Poll::Ready(Some(Ok(StreamEvent::Start { id, .. }))) => {
                    if !id.is_empty() {
                        this.id = id;
                    }
                    let role = json!({ "index": 0, "delta": { "role": "assistant" }, "finish_reason": null });
                    let chunk = this.chunk(Value::Array(vec![role]), None);
                    this.pending.push_back(chunk);
                }
                Poll::Ready(Some(Ok(StreamEvent::Delta(delta)))) => {
                    let mut value = Map::new();
                    if let Some(content) = delta.content {
                        value.insert("content".into(), json!(content));
                    }
                    if let Some(reasoning) = delta.reasoning {
                        value.insert("reasoning_content".into(), json!(reasoning));
                    }
                    if !delta.tool_calls.is_empty() {
                        let calls = delta
                            .tool_calls
                            .into_iter()
                            .map(|call| {
                                let mut function = Map::new();
                                if let Some(name) = call.name {
                                    function.insert("name".into(), json!(name));
                                }
                                if let Some(arguments) = call.arguments_delta {
                                    function.insert("arguments".into(), json!(arguments));
                                }
                                let mut tool = Map::new();
                                tool.insert("index".into(), json!(call.index));
                                if let Some(id) = call.id {
                                    tool.insert("id".into(), json!(id));
                                }
                                tool.insert("type".into(), json!("function"));
                                tool.insert("function".into(), Value::Object(function));
                                Value::Object(tool)
                            })
                            .collect::<Vec<_>>();
                        value.insert("tool_calls".into(), Value::Array(calls));
                    }
                    if !value.is_empty() {
                        let chunk = this.delta_chunk(Value::Object(value));
                        this.pending.push_back(chunk);
                    }
                }
                Poll::Ready(Some(Ok(StreamEvent::Usage(usage)))) => {
                    let chunk = this.usage_chunk(usage);
                    this.pending.push_back(chunk);
                }
                Poll::Ready(Some(Ok(StreamEvent::Done(reason)))) => {
                    let chunk = this.finish_chunk(&reason);
                    this.pending.push_back(chunk);
                }
                Poll::Ready(Some(Ok(StreamEvent::Error(error)))) => {
                    this.pending
                        .push_back(Self::error_chunk(&GatewayError::Provider(error)));
                    this.pending.push_back(Self::done());
                    this.terminated = true;
                }
                Poll::Ready(Some(Err(error))) => {
                    this.pending.push_back(Self::error_chunk(&error));
                    this.pending.push_back(Self::done());
                    this.terminated = true;
                }
                Poll::Ready(None) => {
                    this.pending.push_back(Self::done());
                    this.terminated = true;
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

fn unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::stream;
    use futures_util::StreamExt;
    use webx_ai::{Delta, Usage};

    fn events() -> Vec<Result<StreamEvent, GatewayError>> {
        vec![
            Ok(StreamEvent::Start {
                id: "chatcmpl-1".into(),
                model: "upstream".into(),
            }),
            Ok(StreamEvent::Delta(Delta::text("hel"))),
            Ok(StreamEvent::Delta(Delta::text("lo"))),
            Ok(StreamEvent::Usage(Usage::new(2, 3))),
            Ok(StreamEvent::Done(FinishReason::Stop)),
        ]
    }

    async fn collect(events: Vec<Result<StreamEvent, GatewayError>>) -> Vec<SseEvent> {
        OpenAiSseEncoder::new(stream::iter(events), "fast")
            .map(|event| event.expect("event"))
            .collect()
            .await
    }

    fn data(event: &SseEvent) -> String {
        String::from_utf8(event.data.to_vec()).expect("utf8")
    }

    #[tokio::test]
    async fn encodes_role_deltas_finish_and_done() {
        let frames = collect(events()).await;
        assert_eq!(frames.len(), 6, "role + 2 deltas + usage + finish + done");
        assert!(data(&frames[0]).contains("\"role\":\"assistant\""));
        assert!(data(&frames[1]).contains("\"content\":\"hel\""));
        assert!(data(&frames[2]).contains("\"content\":\"lo\""));
        assert!(data(&frames[3]).contains("\"usage\""));
        assert!(data(&frames[4]).contains("\"finish_reason\":\"stop\""));
        assert_eq!(data(&frames[5]), "[DONE]");
    }

    #[tokio::test]
    async fn error_is_encoded_as_an_error_frame_then_done() {
        let frames = collect(vec![Err(GatewayError::Provider(
            webx_ai::ProviderError::Unavailable("down".into()),
        ))])
        .await;
        assert_eq!(frames.len(), 2);
        assert!(data(&frames[0]).contains("server_error"));
        assert_eq!(data(&frames[1]), "[DONE]");
    }
}

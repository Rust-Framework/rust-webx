//! End-to-end OpenAI adapter tests against the scripted mock upstream.

use std::sync::Arc;

use futures_util::StreamExt;
use serde_json::json;
use webx_ai::{
    CanonicalRequest, ChatProvider, FinishReason, Message, OpenAiProvider, Role, StreamEvent,
};
use webx_client::testing::MockUpstream;
use webx_client::{HttpClient, HttpClientOptions};

fn provider(mock: &MockUpstream) -> OpenAiProvider {
    OpenAiProvider::new(
        "mock",
        Arc::new(
            HttpClient::new("mock", HttpClientOptions::new(mock.base_url()))
                .expect("client builds"),
        ),
        "sk-test",
    )
}

fn request() -> CanonicalRequest {
    CanonicalRequest::new("gpt-test").with_messages(vec![Message::text(Role::User, "hi")])
}

#[tokio::test]
async fn completes_against_a_mock_openai_upstream() {
    let mock = MockUpstream::start().await.expect("mock starts");
    mock.json(
        200,
        &json!({
            "id": "chatcmpl-1",
            "model": "gpt-test",
            "choices": [{
                "message": { "role": "assistant", "content": "hello" },
                "finish_reason": "stop"
            }],
            "usage": { "prompt_tokens": 2, "completion_tokens": 1 }
        }),
    );

    let response = provider(&mock)
        .complete(request())
        .await
        .expect("completion succeeds");
    assert_eq!(response.id, "chatcmpl-1");
    assert_eq!(response.usage.total_tokens(), 3);
    assert!(matches!(
        &response.message.content[0],
        webx_ai::ContentPart::Text(text) if text == "hello"
    ));

    let requests = mock.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, "/v1/chat/completions");
    assert!(requests[0]
        .headers
        .iter()
        .any(|(name, value)| name == "authorization" && value == "Bearer sk-test"));
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).expect("request JSON");
    assert_eq!(body["model"], "gpt-test");
}

#[tokio::test]
async fn streams_against_a_mock_openai_upstream() {
    let mock = MockUpstream::start().await.expect("mock starts");
    mock.sse([
        r#"data: {"id":"chatcmpl-1","model":"gpt-test","choices":[{"delta":{"content":"hel"},"finish_reason":null}]}"#,
        r#"data: {"choices":[{"delta":{"content":"lo"},"finish_reason":"stop"}]}"#,
        "data: [DONE]",
    ]);

    let mut stream = provider(&mock)
        .stream(request().stream())
        .await
        .expect("stream starts");
    let mut text = String::new();
    let mut saw_done = false;
    while let Some(event) = stream.next().await {
        match event.expect("stream event") {
            StreamEvent::Delta(delta) => {
                if let Some(content) = delta.content {
                    text.push_str(&content);
                }
            }
            StreamEvent::Done(FinishReason::Stop) => saw_done = true,
            _ => {}
        }
    }
    assert_eq!(text, "hello");
    assert!(saw_done);
}

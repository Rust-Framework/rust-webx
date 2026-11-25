//! End-to-end tests for the OpenAI-compatible `/v1/chat/completions` surface.

use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use futures_util::stream::StreamExt;
use jsonwebtoken::{EncodingKey, Header};
use serde::Serialize;
use serde_json::json;
use webx::*;
use webx_ai::{
    hash_secret, Alias, ApiKeyRecord, CanonicalRequest, CanonicalResponse, CanonicalStream,
    ChatProvider, Delta, FinishReason, GatewayExecutor, MemoryApiKeyStore, Message, ModelCatalog,
    ModelKey, ModelSpec, OpenAiErrorWriter, ProviderCapabilities, ProviderError, Role,
    RouteStrategy, RouteTarget, StreamEvent, Usage,
};

const JWT_SECRET: &str = "gateway-surface-jwt-secret-at-least-32-bytes";

struct MockProvider;

#[async_trait::async_trait]
impl ChatProvider for MockProvider {
    fn id(&self) -> &str {
        "mock"
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::chat_stream().tools()
    }

    async fn complete(
        &self,
        _request: CanonicalRequest,
    ) -> std::result::Result<CanonicalResponse, ProviderError> {
        Ok(CanonicalResponse {
            id: "chatcmpl-1".into(),
            model: "mock-model".into(),
            message: Message::text(Role::Assistant, "hello"),
            usage: Usage::new(3, 2),
            finish_reason: FinishReason::Stop,
        })
    }

    async fn stream(
        &self,
        _request: CanonicalRequest,
    ) -> std::result::Result<CanonicalStream, ProviderError> {
        Ok(futures_util::stream::iter(vec![
            Ok(StreamEvent::Start {
                id: "chatcmpl-1".into(),
                model: "mock-model".into(),
            }),
            Ok(StreamEvent::Delta(Delta::text("hello"))),
            Ok(StreamEvent::Usage(Usage::new(3, 2))),
            Ok(StreamEvent::Done(FinishReason::Stop)),
        ])
        .boxed())
    }
}

fn executor() -> GatewayExecutor {
    let mut catalog = ModelCatalog::new(1);
    catalog.add_provider(Arc::new(MockProvider));
    let key = ModelKey::new("mock", "mock-model");
    catalog.add_model(
        ModelSpec::new(key.clone()).with_capabilities(ProviderCapabilities::chat_stream().tools()),
    );
    catalog.add_alias(
        "fast",
        Alias::new(RouteStrategy::Fallback, vec![RouteTarget::new(key)]),
    );
    let catalog = Arc::new(catalog);
    let router = Arc::new(webx_ai::CatalogRouter::new(Arc::clone(&catalog)));
    GatewayExecutor::new(catalog, router)
}

fn token() -> String {
    #[derive(Serialize)]
    struct Claims {
        sub: String,
        tenant_id: String,
        exp: usize,
    }

    let exp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs() as usize
        + 3600;
    jsonwebtoken::encode(
        &Header::default(),
        &Claims {
            sub: "key-1".into(),
            tenant_id: "tenant-1".into(),
            exp,
        },
        &EncodingKey::from_secret(JWT_SECRET.as_bytes()),
    )
    .expect("token encodes")
}

fn find_free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local addr")
        .port()
}

async fn spawn_host() -> String {
    // Force the inventory from the library crate into this binary.
    webx_ai_gateway::install();
    let _ = std::any::type_name::<webx_ai_gateway::OpenAiChatHandler>();

    let port = find_free_port();
    let addr = format!("127.0.0.1:{port}");
    let base = format!("http://{addr}");
    let executor = executor();

    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .use_authentication()
        .configure(|app| {
            app.options(|options| {
                options.jwt.secret = JWT_SECRET.to_string();
            });
        })
        .add_error_writer(Arc::new(OpenAiErrorWriter::new()))
        .register(move |svc| svc.instance(Arc::new(executor)))
        .build();

    tokio::spawn(async move {
        let _ = host.run_at(&addr).await;
    });

    for _ in 0..100 {
        if reqwest::get(format!("{base}/health/live")).await.is_ok() {
            return base;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("host did not start");
}

async fn spawn_host_with_api_keys(store: Arc<MemoryApiKeyStore>) -> String {
    webx_ai_gateway::install();
    let _ = std::any::type_name::<webx_ai_gateway::OpenAiChatHandler>();

    let port = find_free_port();
    let addr = format!("127.0.0.1:{port}");
    let base = format!("http://{addr}");
    let executor = executor();
    let validator = webx_ai_gateway::StoreApiKeyValidator::new(store, "v1", b"pepper-v1".to_vec());

    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .add_authentication_scheme(Arc::new(ApiKeyScheme::new(Arc::new(validator))))
        .add_error_writer(Arc::new(OpenAiErrorWriter::new()))
        .register(move |svc| svc.instance(Arc::new(executor)))
        .build();

    tokio::spawn(async move {
        let _ = host.run_at(&addr).await;
    });

    for _ in 0..100 {
        if reqwest::get(format!("{base}/health/live")).await.is_ok() {
            return base;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("host did not start");
}

#[tokio::test]
async fn non_stream_chat_completion_is_openai_shaped() {
    let base = spawn_host().await;
    let response = reqwest::Client::new()
        .post(format!("{base}/v1/chat/completions"))
        .bearer_auth(token())
        .json(&json!({
            "model": "fast",
            "messages": [{ "role": "user", "content": "hi" }]
        }))
        .send()
        .await
        .expect("request succeeds");

    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.expect("JSON body");
    assert_eq!(body["object"], "chat.completion");
    assert_eq!(body["model"], "fast");
    assert_eq!(body["choices"][0]["message"]["content"], "hello");
    assert_eq!(body["choices"][0]["finish_reason"], "stop");
    assert_eq!(body["usage"]["total_tokens"], 5);
}

#[tokio::test]
async fn stream_chat_completion_is_sse() {
    let base = spawn_host().await;
    let response = reqwest::Client::new()
        .post(format!("{base}/v1/chat/completions"))
        .bearer_auth(token())
        .json(&json!({
            "model": "fast",
            "messages": [{ "role": "user", "content": "hi" }],
            "stream": true
        }))
        .send()
        .await
        .expect("request succeeds");

    assert_eq!(response.status(), 200);
    assert_eq!(
        response.headers()["content-type"].to_str().unwrap(),
        "text/event-stream; charset=utf-8"
    );
    let body = response.text().await.expect("SSE body");
    assert!(body.contains("chat.completion.chunk"), "{body}");
    assert!(body.contains("hello"), "{body}");
    assert!(body.contains("[DONE]"), "{body}");
}

#[tokio::test]
async fn unknown_model_uses_the_openai_error_envelope() {
    let base = spawn_host().await;
    let response = reqwest::Client::new()
        .post(format!("{base}/v1/chat/completions"))
        .bearer_auth(token())
        .json(&json!({
            "model": "missing",
            "messages": [{ "role": "user", "content": "hi" }]
        }))
        .send()
        .await
        .expect("request succeeds");

    assert_eq!(response.status(), 404);
    let body: serde_json::Value = response.json().await.expect("JSON body");
    assert_eq!(body["error"]["type"], "not_found_error");
    assert_eq!(body["error"]["code"], "not_found");
}

#[tokio::test]
async fn lists_public_models() {
    let base = spawn_host().await;
    let response = reqwest::Client::new()
        .get(format!("{base}/v1/models"))
        .bearer_auth(token())
        .send()
        .await
        .expect("request succeeds");

    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.expect("JSON body");
    assert_eq!(body["object"], "list");
    let ids = body["data"]
        .as_array()
        .expect("data array")
        .iter()
        .filter_map(|model| model["id"].as_str())
        .collect::<Vec<_>>();
    assert!(ids.contains(&"fast"), "{ids:?}");
}

#[tokio::test]
async fn dry_run_returns_a_cost_estimate_without_generating() {
    let base = spawn_host().await;
    let response = reqwest::Client::new()
        .post(format!("{base}/v1/chat/completions"))
        .bearer_auth(token())
        .json(&json!({
            "model": "fast",
            "messages": [{ "role": "user", "content": "hi" }],
            "max_tokens": 10,
            "temperature": 0,
            "dry_run": true
        }))
        .send()
        .await
        .expect("request succeeds");

    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.expect("JSON body");
    assert_eq!(body["object"], "gateway.estimate");
    assert_eq!(body["model"], "fast");
    assert_eq!(body["max_completion_tokens"], 10);
    assert_eq!(body["estimated_cost_micros"], 0);
    assert!(body["prompt_tokens"].as_u64().unwrap_or_default() >= 1);
}

#[tokio::test]
async fn missing_auth_is_an_openai_401() {
    let base = spawn_host().await;
    let response = reqwest::Client::new()
        .post(format!("{base}/v1/chat/completions"))
        .json(&json!({
            "model": "fast",
            "messages": [{ "role": "user", "content": "hi" }]
        }))
        .send()
        .await
        .expect("request succeeds");

    assert_eq!(response.status(), 401);
    let body: serde_json::Value = response.json().await.expect("JSON body");
    assert_eq!(body["error"]["type"], "authentication_error");
}

#[tokio::test]
async fn api_key_auth_and_revocation_take_effect_immediately() {
    const SECRET: &str = "0123456789abcdef";
    let store = Arc::new(MemoryApiKeyStore::new());
    store.insert(
        ApiKeyRecord::new(
            "key-1",
            "tenant-1",
            hash_secret(b"pepper-v1", SECRET).to_vec(),
            "v1",
        )
        .with_roles(["admin"]),
    );
    let base = spawn_host_with_api_keys(Arc::clone(&store)).await;
    let client = reqwest::Client::new();
    let body = json!({
        "model": "fast",
        "messages": [{ "role": "user", "content": "hi" }]
    });

    let response = client
        .post(format!("{base}/v1/chat/completions"))
        .bearer_auth(format!("sk-key-1-{SECRET}"))
        .json(&body)
        .send()
        .await
        .expect("request succeeds");
    assert_eq!(response.status(), 200);

    store.revoke("key-1");
    let response = client
        .post(format!("{base}/v1/chat/completions"))
        .bearer_auth(format!("sk-key-1-{SECRET}"))
        .json(&body)
        .send()
        .await
        .expect("request succeeds");
    assert_eq!(response.status(), 401);
    let body: serde_json::Value = response.json().await.expect("JSON body");
    assert_eq!(body["error"]["type"], "authentication_error");
}

# rust-webx-client

Outbound HTTP client for [`rust-webx`](https://crates.io/crates/rust-webx): one
`hyper` stack, pooled connections, explicit timeouts, streamed request bodies,
streaming response bodies and incremental SSE decoding.

```toml
[dependencies]
rust-webx-client = "1.3"
```

```rust
use webx_client::{HttpClient, HttpClientOptions, IHttpClient, OutboundRequest};

let client = HttpClient::new("openai", HttpClientOptions::new("https://api.openai.com"))?;
let response = client
    .execute(OutboundRequest::post("/v1/chat/completions").json(&payload)?)
    .await?;

let mut events = response.body.sse();
while let Some(event) = events.next().await {
    forward(event?);
}
```

Design rules:

- one HTTP stack (`hyper` + `hyper-util` + `hyper-rustls`); no second client
  implementation;
- W3C `traceparent` is injected from `RequestContext::trace_id()` unless the
  request sets it explicitly;
- `testing::MockUpstream` provides a scripted local HTTP/SSE upstream for
  adapter and gateway integration tests, with fixture record/replay;
- a non-2xx response is a normal response, not an error — the gateway decides  whether to pass it through or fail over;
- request bodies are bytes for now; streaming uploads are a follow-up;
- retries/circuit breaking are a follow-up; `HttpClientError::is_connect()`
  exposes the typed signal they need;
- versions track the workspace, like `rust-webx-ai`.

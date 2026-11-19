# Mock 上游（MockUpstream）

`rust-webx-client` 内置一个脚本化本地上游，用于适配器/网关集成测试，不需要外网或第三方 mock 服务。

```rust
use std::time::Duration;
use webx_client::testing::MockUpstream;
use webx_client::{HttpClient, HttpClientOptions, IHttpClient, OutboundRequest};

let mock = MockUpstream::start().await?;
mock.json(200, &serde_json::json!({ "ok": true }));
mock.sse([
    r#"data: {"choices":[{"delta":{"content":"hel"}}]}"#,
    r#"data: {"choices":[{"delta":{"content":"lo"}}]}"#,
    "data: [DONE]",
]);
mock.status(429);
mock.delay_before_next(Duration::from_millis(50));

let client = HttpClient::new("mock", HttpClientOptions::new(mock.base_url()))?;
let response = client.execute(OutboundRequest::get("/anything")).await?;
let requests = mock.requests();
```

## 能力

- `json(status, &value)`：JSON 响应；
- `sse(chunks)`：`text/event-stream`，自动补齐 `\n\n`；
- `status(code)`：空 body 状态响应；
- `delay_before_next(d)`：给下一个脚本加延迟（超时/慢上游）；
- `requests()`：录制 method/path/headers/body；
- `shutdown()` / Drop：停止监听并 abort 任务。

## 典型用途

- OpenAI/Anthropic 适配器的 HTTP + SSE conformance；
- 首字节、429/5xx、慢上游、断连回归；
- 网关端到端测试（配合 `HttpClient` 与 `GatewayExecutor`）；
- 脱敏后录制请求/响应，做 replay fixture。

## Record/replay fixture

```rust
let fixture = mock.fixture();          // requests + responses（原始值）
mock.save_fixture("fixtures/openai.json")?;      // 默认脱敏后保存
mock.save_fixture_raw("local/raw.json")?;        // 仅本地使用，含明文凭据

let fixture = MockUpstream::load_fixture("fixtures/openai.json")?;
let replay = MockUpstream::start().await?;
replay.replay(&fixture);               // 按顺序入队响应
```

- `save_fixture` **默认脱敏**：`authorization`、`proxy-authorization`、`x-api-key`、`api-key`、
  `cookie`、`set-cookie` 的值替换为 `<redacted>`；请求体/响应体原样保留（如有 prompt 敏感内容，
  录制后自行处理）。
- `ReplayFixture::redacted()` 可在内存里生成脱敏副本；`save_fixture_raw` 只用于不提交的本地抓包。
- fixture 带 `version`（当前 `2`）。没有该字段的旧文件按版本 1 读取；比当前版本新的文件在
  `load_fixture` 时返回 `InvalidData`，不会按错误格式静默播放。
- fixture 是 pretty JSON，包含 request（method/path/headers/body）与 response
  （status/headers/body）。body 为 UTF-8，二进制会 lossy。

SSE 解码器另有“任意字节切分点 + 100 组伪随机多 chunk 切分”回归，确保上游分片方式不影响事件序列。

## 边界

- 只绑定 `127.0.0.1:0`，不是生产服务器；
- 请求按入队顺序消费；没有脚本时返回 503；
- 不模拟 HTTP/2、TLS、真实网络分区；
- fixture body 为 UTF-8（二进制 lossy）；
- 默认保存会脱敏凭据头，但**不**自动识别 body 内的密钥/prompt；
- 官方 SDK conformance、SSE fuzz/压测仍在后续版本。

示例测试见 `crates/ai/tests/openai_provider_test.rs`（complete/stream 真实 HTTP）。

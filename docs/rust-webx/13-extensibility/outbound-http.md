# 出站 HTTP（rust-webx-client）

服务端调用第三方 HTTP 服务（AI provider、支付、消息队列、内部微服务）统一使用
`rust-webx-client`。它复用 `hyper` + `hyper-rustls`，提供连接池、rustls TLS、明确的超时分层、
流式响应体和上游 SSE 解码；不要在业务里另起一套 `reqwest`/裸 `hyper` 客户端。

```toml
[dependencies]
rust-webx-client = "2.7"
```

## 命名客户端

每个上游一个客户端（独立 base URL、连接池、超时）：

```rust
use std::sync::Arc;
use webx::*;

let openai = HttpClient::new("openai", HttpClientOptions {
    base_url: "https://api.openai.com".into(),
    connect_timeout: Duration::from_secs(3),
    request_timeout: Some(Duration::from_secs(120)), // 只约束响应头
    read_timeout: Some(Duration::from_secs(120)),    // 帧间空闲
    pool_max_idle_per_host: 64,
    ..HttpClientOptions::default()
})?;

Host::builder()
    .add_http_client("openai", openai)
    .build();
```

`add_http_client` 把所有命名客户端装进一个 `HttpClientRegistry`，并注册两个可注入形式：
`Arc<HttpClientRegistry>` 与 `Arc<dyn IHttpClientFactory>`。手工 `.register(...)` 的 registry
优先（`try_add` 只在该端口为空时补默认）。Handler 依赖端口，不依赖具体客户端：

```rust
#[derive(Inject)]
pub struct SummarizeHandler {
    #[inject]
    clients: Arc<dyn IHttpClientFactory>,
}

let client = self.clients.create("openai")?; // Arc<dyn IHttpClient>
```

`rust-webx` 已重导出 `HttpClient` / `HttpClientOptions` / `HttpClientRegistry` /
`IHttpClientFactory`，应用通常不需要直接依赖 `rust-webx-client`。

## 请求

```rust
let response = client.execute(
    OutboundRequest::post("/v1/chat/completions")
        .header("authorization", format!("Bearer {key}"))
        .json(&payload)?          // 自动设置 content-type: application/json
).await?;
```

- 路径相对 `base_url`；也可以传绝对 `http(s)://` URL。
- `header` 同名后者覆盖前者；默认头（`default_headers`）会被请求头覆盖。
- 请求体支持字节或流式：`OutboundRequest::body_stream(chunks)` 逐 chunk 发送，不整段缓冲。
- **hop-by-hop 头不会转发**：`connection`、`keep-alive`、`proxy-*`、`te`、`trailer`、
  `transfer-encoding`、`upgrade` 以及 `Connection` 头里列出的字段名都会被剥离（RFC 9110）。
  `authorization` 等端到端头照常发送；上游凭据由调用方显式设置。
- 未显式设置 `traceparent` 时，客户端读取 `RequestContext::trace_id()` 注入 W3C
  `00-{trace_id}-{span}-01`；显式 header 优先。

## 响应

```rust
// 非流：有上限地收集，防止恶意上游打爆内存
let body = response.body.collect(1024 * 1024).await?;

// 流：原始字节
let mut chunks = response.body.bytes();

// SSE：逐事件解码，背压传导到上游
let mut events = response.body.sse();
while let Some(event) = events.next().await {
    let event = event?;
    // event.data 可直接转成下游 SseEvent
}
```

**非 2xx 不是错误**：`OutboundResponse` 带 `status`，由调用方决定透传、切候选还是映射成网关错误。
只有连接/超时/协议/读体失败才返回 `HttpClientError`。

## 超时与错误

| 选项 | 约束 |
|------|------|
| `connect_timeout` | TCP/TLS 建连 |
| `request_timeout` | 收到响应头之前（**不**限制流式 body） |
| `read_timeout` | 响应体帧之间的空闲 |
| `HttpClientError::Timeout` | 上述任一超时 |
| `HttpClientError::is_connect()` | 连接类/超时；重试策略据此判定 |

**首字节规则**：流式响应一旦向客户端发出第一个字节，就不能再重试/切候选（会重复输出与重复计费）。
重试只允许发生在 `execute` 返回之前（连 `OutboundResponse` 都还没拿到）。

## 代理与企业 CA

```rust
let client = HttpClient::new("openai", HttpClientOptions {
    base_url: "https://api.openai.com".into(),
    proxy: Some("http://proxy.internal:8080".into()),
    proxy_authorization: Some("Basic dXNlcjpwYXNz".into()), // 可选
    root_certificates: Some("certs/company-ca.pem".into()), // 可选
    ..HttpClientOptions::default()
})?;
```

- 代理走 HTTP `CONNECT` 隧道（明文 HTTP 与 HTTPS 目标都适用）；不读 `HTTP(S)_PROXY` 环境变量，
  显式配置才启用，避免生产环境被环境变量悄悄改写。
- `proxy_authorization` 是完整的 `Proxy-Authorization` 值，只用于 CONNECT，不转发给上游。
- `root_certificates` 指向 PEM 包；设置后**替换**内置 webpki 根（不是追加）。文件缺失、无证书或
  解析失败会让 `HttpClient::new` 直接返回 `InvalidRequest`，不会静默回退到公共根。
- 代理 URL/CA 校验都在构造期完成；运行期不会因配置错误改连直连。

## SSRF 防护

默认拒绝私网/链路本地/云元数据目标（`ssrf_protection = true`）：

```rust
let client = HttpClient::new("public", HttpClientOptions {
    base_url: "https://api.openai.com".into(),
    // 默认即可；需要访问内网 vLLM/Ollama 时显式放开：
    allow_private_networks: false,
    ..HttpClientOptions::default()
})?;
```

- 字面私有 IPv4（10/8、172.16/12、192.168/16、100.64/10 等）、链路本地
  （169.254/16，含 `169.254.169.254`）、文档/基准地址、IPv6 ULA/链路本地/组播一律拒绝，
  返回 `HttpClientError::BlockedAddress`。
- **IPv4-mapped IPv6**（`::ffff:a.b.c.d`）会先映射回 IPv4 再套用同一策略，堵住 `::ffff:169.254.169.254`
  这类前缀绕过；同时覆盖 IPv6 文档段 `2001:db8::/32`、站点本地 `fec0::/10`、
  6to4 `2002::/16` 与 NAT64 `64:ff9b::/96` 中嵌入的私有地址。
- 内部主机后缀 `.local`、`.internal`、`.home.arpa` 同样拒绝。
- 回环（`127.0.0.0/8`、`::1`、`localhost`）允许，便于本地开发与 mock。
- 内网/本地模型显式设置 `allow_private_networks: true` 才放行。
- **DNS rebinding 在连接期复查**：公网域名由带守卫的解析器解析，每个 DNS 应答 IP 都用同一套
  `is_blocked_ip` 过滤；若全部被封堵则连接失败，不再出现「URI 检查通过、连接时却指向私网」的窗口。
  字面 IP 不走解析器（hyper 直接连接），已在 URI 校验阶段覆盖。客户端不自动跟随重定向。
- 代理地址由运维配置，不经过该守卫；上游目标仍会校验。

## HTTP/2

出站与入站均支持 HTTP/2：

- **出站**：默认通过 TLS ALPN 协商 h2（`http2 = true`，HTTP/1.1 作为回退）。设置
  `http2_only = true` 可强制 HTTP/2——TLS 目标只 advertise `h2`，明文目标使用 h2c
  prior-knowledge。响应版本可从 `OutboundResponse::version` 读取。
- **入站**：服务端连接构建器版本自适应，同时接受 HTTP/1.1 与 HTTP/2；明文监听支持 h2c
  prior-knowledge，TLS 监听通过 ALPN（`h2` 优先、`http/1.1` 回退）协商。
- hop-by-hop 头过滤、超时与 SSE 解码在 h1/h2 下语义一致。

## 限制（当前版本）

- 客户端不实现自动重试/熔断；这些属于 `rust-webx-ai` 执行器的策略层（G9），本 crate 只提供
  `HttpClientError::is_connect()` 判定信号。
- 没有系统证书库（`rustls-native-certs`）；企业根证书用 `root_certificates` 显式提供。
- 没有连接级代理自动探测/`NO_PROXY` 例外；需要多个出口时注册多个命名客户端。

下游 SSE 响应的写法见[流式响应（SSE）](../05-request-pattern/streaming-sse.md)。

## 测试

`crates/client/tests/http_client_test.rs` 覆盖：JSON/头/流式请求体、SSE 解码、请求/帧间超时、
体积上限、非 2xx、`traceparent` 注入、hop-by-hop 剥离、`CONNECT` 代理隧道与自定义根证书；
`crates/webx/tests/http_client_wiring_test.rs` 覆盖 `HostBuilder::add_http_client` 的 DI 装配
（`IHttpClientFactory` 按名解析）。

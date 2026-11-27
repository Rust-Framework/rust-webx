# OAuth2 client credentials（服务账号取 token）

服务到服务调用用**机器身份**取 token，不要借用户 token，也不要在每个适配器里手写 token 端点
请求。框架把「取机器令牌」做成端口 `IClientCredentials`，`rust-webx-oauth2` 提供
`ClientCredentialsClient` 实现（RFC 6749 §4.4），缓存、提前刷新、并发单飞与失败分类都在实现里。

```toml
[dependencies]
rust-webx-oauth2 = "2.9"
```

| 项 | 位置 |
|----|------|
| 端口 | `webx::{IClientCredentials, ServiceToken}` |
| 实现 | `rust-webx-oauth2` 的 `ClientCredentialsClient`（`token()` / `token_for()` / `invalidate()`） |
| 缓存 | 按 `(scope 集合, audience)` 分桶，提前刷新 + 并发单飞 |
| 底层传输 | `IHttpClient`（`rust-webx-client`）；测试时用 `IHttpClient` 的 fake 实现（`OutboundResponse::from_bytes`/`ok_response` + `OutboundRequest` 读访问器） |

`ServiceToken { access_token, expires_at_unix_secs, scope }`：token 本身是 bearer 凭据，
`Debug` 打 `<redacted>`。

## 1. 装配

secret 由调用方传入（推荐 `ISecretStore`），不要写进 `appsettings.json`，也不读环境变量：

```rust
use std::sync::Arc;
use std::time::Duration;
use webx::{Host, IClientCredentials, ISecretStore};
use webx_oauth2::{ClientCredentialsClient, ClientCredentialsConfig, ClientSecretAuth};

let secret = secrets.get("BILLING_CLIENT_SECRET").await?;
let config = ClientCredentialsConfig::new(
    "https://id.example.com/oauth2/token", // 必须 https://
    "billing-service",
    secret.expose(),   // 只在装配期读一次
    ["billing.write"], // 默认 scope；需要别的 scope 时用 token_for(&[...])
)
.with_client_auth(ClientSecretAuth::Basic)     // Basic（默认）或 Post
.with_audience("https://billing.example.com")  // RFC 8707，可选
.with_refresh_ahead(Duration::from_secs(60))   // 上限为寿命的一半
.with_timeout(Duration::from_secs(10));        // 区间 1s–60s

// http: Arc<dyn IHttpClient>（命名客户端），clock: Arc<dyn IClock>（Host 已注册 SystemClock）
let credentials: Arc<dyn IClientCredentials> =
    Arc::new(ClientCredentialsClient::new(config, http, clock)?);

Host::builder()
    .register(move |svc| svc.instance::<dyn IClientCredentials>(Arc::clone(&credentials)))
    .build();
```

- **注册必须落到 `dyn IClientCredentials`**：Handler 注入的是 `Arc<dyn IClientCredentials>`，
  注册具体类型不会被解析。
- 构造期就校验：`client_id`/`client_secret` 非空、token 端点是绝对 URL 且必须 `https://`。
  `allow_private_networks(true)` 只放行**回环**地址（`localhost`/`127.0.0.0/8`/`::1`）的明文 `http`，
  用于本地 IdP 与测试；内网域名仍然必须 `https://`。配置错误在启动期以 `Error::di` 失败，不会在
  首个请求才暴露。
- 注入的 `IHttpClient` 保留自己的 SSRF 策略：内网端点还要用
  `HttpClientOptions::allow_private_networks` 构建客户端。
- 一个 client 一个实例；多 IdP / 多服务账号各建一个，分别注入。

## 2. 取 token 并发请求

```rust
use webx::*;

#[derive(Inject)]
pub struct ChargeHandler {
    #[inject]
    credentials: Arc<dyn IClientCredentials>,
    #[inject]
    clients: Arc<dyn IHttpClientFactory>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ChargeRequest, ChargeDto> for ChargeHandler {
    async fn handle(&mut self, req: ChargeRequest) -> ActionResult<ChargeDto> {
        // 缓存命中不发请求；并发调用共享同一个在途请求
        let token = self.credentials.token(&["billing.write"]).await?;
        let response = self
            .clients
            .create("billing")?
            .execute(
                OutboundRequest::post("/charges")
                    // 框架不自动注入 Bearer：调用方显式加头
                    .header("authorization", format!("Bearer {}", token.access_token))
                    .json(&req)?,
            )
            .await?;
        // ...
    }
}
```

- `token(&[...])` 传入的 scope **覆盖**实现里配置的 scope；传空切片表示请求里**不发** `scope`
  参数（用 IdP 的默认 scope），这是与「用配置的 scope」不同的缓存桶。
- 直接持有具体类型时还有 `token()`（用配置的 scope）与 `token_for(&[...])` 两个入口，
  语义与端口方法一致。
- `expires_at_unix_secs` 是 Unix 秒；自己比较时钟时留安全边距，不要用最后一秒。

## 3. 缓存与失败语义

| 场景 | 行为 |
|------|------|
| 缓存命中（`now < exp - refresh_ahead`） | 直接返回，不发请求 |
| 同一 `(scope, audience)` 并发 | 单飞：一个在途请求，其余等待同一结果 |
| 不同 scope / audience | 互不阻塞（每个桶一把锁，不是全局锁） |
| 缓存条目数 | 每实例上限 **256**，满了按插入顺序 FIFO 淘汰；`cached_entries()` 供诊断 |
| 单次 token 请求 | 端到端 deadline（`execute` + 读 body + 解析整体超时），不依赖注入 client 遵守提示 |
| token 端点 4xx | `Error::di`（凭据/配置错误，500，不重试） |
| 429 / 5xx / 超时 / 连不上 | 可重试的 503 |
| 任何失败 | 不缓存；**绝不**降级成无令牌请求 |

- token 端点省略 `expires_in` 时按 300s 记寿命（保守假设，避免缓存一个长期失效的 token）。
- `token_type` 不是 `Bearer`、响应不是 JSON、`access_token` 为空 → 503。
- 凭据轮换，或下游对本服务 token 回 `401` 时调用 `invalidate()`（清空所有桶；在途请求不取消，
  结果也不再被采纳）。
- 传输失败的错误文案固定，不回显请求 URL / 头，避免把凭据写进日志。

## 4. 边界

- **不做**自动注入 `Authorization: Bearer`（隐式注入会把"用谁的令牌"变成 ambient 状态）：
  调用方显式取 token、显式加头。
- **不做** refresh token：client credentials 没有用户上下文，过期前重取即可。
- **不读**环境变量取 secret：secret 由组合根从 `ISecretStore` 解析后一次性传入。
- 机器 token 不带用户、租户或角色：下游按 scope 判定；不要把它塞进面向用户的请求。
- 该端口只负责"取令牌"，不做签名校验、不感知资源服务器；验签见 [JWT Bearer 认证](jwt-authentication.md)。

判定与边界见 [federated-auth.md](../../design/federated-auth.md)。

相关：

- [密钥存储](secret-store.md)：`ISecretStore` 取 secret 的推荐来源。
- [出站 HTTP](../13-extensibility/outbound-http.md)：命名客户端与代理、超时、SSRF 规则。
- [服务端资源授权](resource-authorization.md)：资源服务器侧如何消费 scope 与 PDP 决策。

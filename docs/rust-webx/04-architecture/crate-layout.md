# Crate 分层结构

## 依赖方向规则

**单向依赖，不可逆向**：

```
应用代码 → rust-webx (伞) → host / macros / spa / openapi → core
                                                              ↑
                                                         零实现依赖
```

`core` 是金字塔顶端（最抽象），`host` 是运行时实现层，应用代码在最上层。

### 包名与导入名

Cargo **包名**带连字符，Rust **导入名**用下划线库名。两者不是一回事：

| 包名（`Cargo.toml`） | 导入名（`use`） | 职责 |
|----------------------|-----------------|------|
| `rust-webx` | `webx` | 伞 crate，统一 re-export |
| `rust-webx-core` | `webx_core` | 契约与配置 |
| `rust-webx-host` | `webx_host` | Host / 路由 / 中间件装配 |
| `rust-webx-engine` | `webx_engine` | 执行引擎（worker/路由/管道/传输端口） |
| `rust-webx-macros` | `webx_macros` | 过程宏（`#[get]`、`#[handler]`…） |
| `rust-webx-spa` | `webx_spa` | 静态文件 / SPA |
| `rust-webx-openapi` | `webx_openapi` | OpenAPI 生成 + UI |
| `rust-webx-ai` | `webx_ai` | AI 网关领域包（用量/记账/端口；不进入伞 crate） |
| `rust-webx-ai-gateway` | `webx_ai_gateway` | OpenAI 兼容 `/v1` HTTP 表面（依赖 webx + webx-ai） |
| `rust-webx-client` | `webx_client` | 出站 HTTP（连接池/流式/SSE；伞 crate 只重导出传输类型） |
| `rust-webx-outbox` | `webx_outbox` | 事务性 outbox 的端口（`IOutbox`/`OutboxEntry`）、派发器、`OutboxOptions` 与内存实现（不链接 rust-ef） |
| `rust-webx-mfa` | `webx_mfa` | TOTP / 恢复码 MFA（可选，见 auth 节） |
| `rust-webx-oauth2` | `webx_oauth2` | OAuth 2.0 社交登录客户端（可选） |
| `rust-webx-oidc` | `webx_oidc` | OIDC 客户端（discovery/PKCE/id_token，可选） |
| `rust-webx-webauthn` | `webx_webauthn` | WebAuthn / Passkey relying-party 校验（可选） |
| `rust-webx-redis` | `webx_redis` | 分布式缓存 + 事件传输 + 共享续传/在线态（可选） |
| `rust-webx-nats` / `rust-webx-rabbitmq` / `rust-webx-kafka` | `webx_nats` / `webx_rabbitmq` / `webx_kafka` | 事件传输适配器（可选；Kafka 仅竞争投递） |
| `rust-webx-pay` + 六个 provider crate | `webx_pay*` | 支付领域组（可选，见 payment 节） |
| `rust-webx-apns` / `rust-webx-fcm` | `webx_apns` / `webx_fcm` | 移动推送适配器（可选，见 push 节） |
| `rust-webx-h3` | `webx_h3` | 可选 HTTP/3 监听传输（QUIC + rustls + h3；不进伞 crate，默认构建不链接） |
| `rust-webx-build` | `webx` | 构建期（`build-dependencies`） |

> 共享契约 crate `rust-core`（`rust_core`）不在 `crates/` 下：`AppError`/`Result`、
> `PagedRequest`/`PagedResult`、`KeysetRequest`/`CursorPage`、`OperationResult`、`IPatchable`
> 等由它定义；`webx_core::error` / `::pagination` / `::keyset` / `::operation` / `::persistence`
> 只是**历史模块路径的重导出**（自 2.9.1 起），不要在 webx 侧找第二份定义。

> 与 ORM/数据库耦合的持久化参考适配器（`examples/adapters/{ai,auth,idempotency,outbox,payment}/ef`）
> **不在上表**：它们 `publish = false`，不是可依赖的包，只供复制。

`crates/` 只包含**可发布** crate（`publish = false` 会被布局门禁拒绝）；非发布的测试工具在
`testing/`（`alloc-probe`、`contract-tests`、`load-probe`、`upgrade-smoke`），`fuzz/` 是独立的 cargo-fuzz
workspace。应用代码只依赖 `crates/` 下的包。

对照 `rust-dix` / `rust-ef`：包名与导入名同样分别是 `rust-dix`→`rust_dix`、`rust-ef`→`rust_ef`。

## core — 契约层

`rust-webx-core` 定义框架公开 trait 与绑定/运行时辅助类型，**不依赖** hyper、tokio 或任何
HTTP 实现；其中 `error` / `error_codes` / `pagination` / `keyset` / `operation` /
`persistence` 是 `rust-core` 共享契约的重导出（定义在 `rust-core`，不是本 crate）。

设计意图：
- 第三方可基于 core trait 编写可插拔组件
- 单元测试可 Mock 全部接口
- 可替换 HTTP 引擎（如从 hyper 迁移）

契约域与代表模块（模块全集以 `crates/core/src/lib.rs` 的 `pub use` 与 rustdoc 为准）：

| 契约域 | 代表模块 | 代表类型 |
|---|---|---|
| 请求/处理 | `mediator`, `handler`, `route`, `http`, `middleware`, `action`, `pipeline` | `IRequest`, `IEventRequest`, `IRequestHandler`, `IEventHandler`, `IHostedService`, `IHttpContext`, `IActionResult` |
| 认证/安全 | `auth`, `security`, `idempotency`, `locking`, `secrets` | `IClaims`, `IAuthenticationScheme`, `IDynamicAuthorizer`, `IdentityUser` / `RequestUser`, `ISessionStore`, `IPasswordHasher`, `IIdempotencyStore`, `IAdvisoryLock` |
| 实时/通知 | `realtime`, `push`, `notifications`, `stream` | `IRealtimeEventStore`, `IOnlinePresence`, `IConnectionRegistry`, `WebSocketLimits`, `IPushSender`, `IEmailSender` / `ISmsSender`, `SseResult` |
| 配置/可观测 | `config`, `options_monitor`, `metrics`, `localization`, `time`, `error` | `AppOptions`, `IOptionsMonitor`, `IMeter`, `IStringLocalizer`, `IClock`, `Error` |
| 数据/分页 | `cache`（端口） + `rust_core::{pagination, keyset, operation, persistence}` 的重导出 | `IDistributedCache`；`PagedRequest`/`PagedResult`、`KeysetRequest`/`CursorPage`、`OperationResult`/`IPatchable` 定义在 **rust-core**，`webx_core` 重导出历史路径 |

## host — 运行时层

`rust-webx-host` 实现 core 定义的 HTTP 相关 trait：

| 组件 | 实现 |
|------|------|
| HTTP 服务 | hyper + tokio |
| 路由 | `webx_engine::Router`（matchit radix 树，生产路径）；`webx_host::router::Router` 仅为 1.5 兼容嵌入器，不服务请求 |
| 上下文 | `webx_engine::EngineContext`（应用契约是 `IHttpContext`） |
| 管道 | `webx_engine::Pipeline`（生产路径）；`MiddlewarePipeline::execute` 仅为 1.5 兼容执行器 |
| 认证 | `JwtAuth`, `jwt_middleware`, `AuthenticationMiddleware` + `JwtScheme` / `ApiKeyScheme` 链 |
| 授权 | 端点执行 `#[authorize]` 静态规则；`#[authorize(dynamic)]` 把声明交给 `IDynamicAuthorizer::authorize_with_declared_rules`；实例归属在 Handler |
| 端点 | `StubEndpoint`（路由宏端点）, `RequestEndpoint`, `ControllerEndpoint`, `StaticJsonEndpoint`, `StaticHtmlEndpoint` |
| 宿主 | `Host`, `HostBuilder`, `Server` |

## engine — 执行引擎

`rust-webx-engine` 是 HTTP **执行层**：acceptor + N 个 worker runtime、零拷贝上下文
（`EngineContext`）、matchit 路由（`Router`）、onion 中间件管道（`Pipeline`）、TLS/监听与
传输端口（`IHttpTransport` / `TransportRuntime` / `TransportRequest` / `TransportResponse`）。
`host` 通过 engine 的生产路径服务请求；应用侧只依赖 `core` 契约，不直接绑定 engine 类型。

## macros — 编译时层

`rust-webx-macros` 在编译期生成：

- 路由注册代码（写入 `inventory`）
- Handler 注册代码（向 `inventory` 提交 `HandlerRegistration`）
- 授权规则与端点文本元数据（`#[authorize]`、`#[display]`、`#[description]`、`#[category]`）

应用开发者通过 `#[get]`、`#[handler]` 等使用，无需直接接触宏展开代码。

## spa / openapi — 能力扩展层

| Crate | 职责 |
|-------|------|
| `rust-webx-spa` | `SpaMiddleware` 静态文件 + History fallback |
| `rust-webx-openapi` | OpenAPI 3.0 规范生成 + 内置 API 文档 UI（`/api/openapi.html`） |

二者均依赖 `core`，被 `host` 在 `HostBuilder` 中集成。

## ai — 可选领域包

`rust-webx-ai`（导入名 `webx_ai`）承载 AI 聚合网关的领域能力：统一 IR（`CanonicalRequest`/
`StreamEvent`）、`ChatProvider` 端口、OpenAI 兼容适配器、`ModelCatalog`/`CatalogRouter` 与健康表、
`CatalogHandle` 热替换、`GatewayExecutor` 执行器、精确响应缓存与单飞、成本计算、
`RetryPolicy`/`CircuitBreaker`/`Bulkhead` 韧性原语、
`Usage`/`UsageEvent` 与 `IUsageSink` 账本、`rollup_events`、`ITokenizer`、
API key 解析/HMAC、`IApiKeyStore`/`MemoryApiKeyStore`、配额预扣-结算、`OpenAiErrorWriter`。
HTTP 数据面端点、非 OpenAI 适配器与租户级路由后续加入。

- **不进入伞 crate**：应用显式依赖 `rust-webx-ai`，不需要 AI 能力的项目不引入。
- **不依赖 host/hyper**：领域类型与传输解耦；出站 HTTP 由 `rust-webx-client` 提供。
- **版本跟随 workspace**：`version.workspace = true`，与 `rust-webx` 同一个版本号，没有独立 0.x 线。

## gateway — OpenAI 兼容表面

`rust-webx-ai-gateway`（导入名 `webx_ai_gateway`）是 `rust-webx-ai` 的 HTTP 适配层：
`POST /v1/chat/completions` 的请求 DTO、IR 转换、JSON/SSE 输出与 OpenAI 错误信封。
它依赖伞 crate 与 `rust-webx-ai`，但不反向进入 `rust-webx-ai` 的领域依赖；应用注册
`Arc<GatewayExecutor>`、鉴权 scheme 与 `OpenAiErrorWriter` 后即可使用。

## payment — 可选支付领域组

`crates/payment/` 是与 ai/auth/cache/queue 同级的封闭领域组；回调 inbox 的参考实现在
`examples/adapters/payment/ef`（`publish = false`，复制使用）：

| crate | 导入名 | 职责 |
|---|---|---|
| `rust-webx-pay` | `webx_pay` | 统一模型与端口：`Money`/`PaymentStatus`/`PaymentEvent`、`IPaymentProvider`、`IPaymentWebhookVerifier`、`PaymentWebhook`、`Payments` 运行时（缓存/单飞/失效）、`PaymentScope`、`IPaymentServiceFactory`/`IPaymentProviderFactory`/`IPaymentProviderBuilder` |
| `rust-webx-pay-stripe` | `webx_pay_stripe` | Stripe PaymentIntents/Checkout、退款、`Stripe-Signature` |
| `rust-webx-pay-alipay` | `webx_pay_alipay` | 支付宝 page/wap/app/precreate、RSA2 签名与异步通知 |
| `rust-webx-pay-wechatpay` | `webx_pay_wechatpay` | 微信支付 APIv3、请求签名、平台证书/公钥、AEAD 解密 |
| `rust-webx-pay-paypal` | `webx_pay_paypal` | Orders v2、OAuth2、capture/void/refund、验签 API |
| `rust-webx-pay-adyen` | `webx_pay_adyen` | Checkout payments/captures/refunds、HMAC 回调 |
| `rust-webx-pay-square` | `webx_pay_square` | Payments/Refunds、Payment Links、HMAC 回调 |
| `rust-webx-pay-ef` | `webx_pay_ef` | 回调 inbox 的**参考适配器**（`examples/adapters/payment/ef`）：唯一去重、重试、死信；非发布，复制使用 |

- **不进入伞 crate**：应用显式依赖所需适配器；不需要支付的项目不引入。
- **唯一 HTTP 栈**：适配器只依赖 `rust-webx-client`，不引入服务商 SDK（官方 SDK 无 Rust 版本）。
- **回调路由由应用声明**：crate 只提供原始 body 请求类型、验签与 ACK，不注入隐式路由。
- **多账户/服务商**：一个适配器实例可持有多个商户账户；请求可选账户，回调可用
  `/webhooks/payments/{provider}/{account}`。
- **版本跟随 workspace**：`version.workspace = true`，与 `rust-webx` 同一版本号。

## outbox — 事务性 outbox（框架 crate）

`rust-webx-outbox`（导入名 `webx_outbox`）是可发布的框架 crate，只做契约与派发，**不链接 rust-ef**：

| 项 | 职责 |
|---|---|
| `IOutbox` / `OutboxEntry` | 端口与值类型：入队、领取、完成/失败 |
| `OutboxOptions` | interval / batch / lease / attempts / retry base / retention |
| `OutboxDispatcher` | `PeriodicBackgroundService` + `EventDispatcher`：租约领取、指数退避、死信、保留期清理 |
| `add_outbox` / `add_outbox_with` | 注册 `Arc<dyn IOutbox>` + 派发器：`add_outbox(store, OutboxOptions)`，需要容器构建存储时用 `add_outbox_with` |
| `InMemoryOutbox` | 单进程/测试用存储（作为 `store` 传给 `add_outbox`） |

- **存储归下游**：数据库表、同事务入队、租户字段属于数据适配；参考实现是
  `examples/adapters/outbox/ef`（`publish = false`，复制后 `add_outbox_ef`）；
- **表由应用实体声明**：`webx_outbox` 由官方 rust-ef 迁移生成，`bootstrap` 仅测试；
- **与事件传输分工**：outbox 保证本地事务与事件同生共死；跨进程投递交给 `IEventPublisher`。

## examples/adapters — 参考适配器（非发布）

`examples/adapters/` 是独立 workspace（自带 lockfile），五个与 ORM/数据库耦合的持久化适配器都在这里，
全部 `publish = false`：

| 适配器 | 实现的端口 | 说明 |
|---|---|---|
| `ai/ef` | `IUsageSink` | 用量账本（`ai_usage_events`）+ outbox/rollup workers |
| `auth/ef` | `ISessionRepository` / `IRefreshTokenStore` | 会话表与 refresh token 表 |
| `idempotency/ef` | `IIdempotencyStore` | `webx_idempotency` 表、租约围栏、清理服务 |
| `outbox/ef` | `IOutbox`（EF 存储） | 同事务 `enqueue_outbox` + `add_outbox_ef` |
| `payment/ef` | 回调 inbox | 唯一去重、重试、死信 |

- **定位是参考实现**：可复制不可依赖——复制进业务仓后包名、实体、迁移都归下游；
- **不发布**：不在 crates.io，不参与 semver 检查，也不在发布顺序里；
- **接入方式**（用默认 / 复制参考适配器 / 自己实现端口 + 契约测试）见
  [端口与适配器（下游接入）](../13-extensibility/integrations.md)。

## auth — 可选认证组

`crates/auth/` 下的 crate 不进伞 crate，按需显式依赖；认证 scheme/端口装配仍在 host：

| crate | 导入名 | 职责 |
|---|---|---|
| `rust-webx-mfa` | `webx_mfa` | TOTP 与恢复码多因素认证 |
| `rust-webx-oauth2` | `webx_oauth2` | OAuth 2.0 社交登录客户端（GitHub/Google/Microsoft/GitLab/Gitee/DingTalk/Feishu 预设） |
| `rust-webx-oidc` | `webx_oidc` | OIDC 客户端：discovery、PKCE、id_token 校验与会话桥接 |
| `rust-webx-webauthn` | `webx_webauthn` | WebAuthn / Passkey relying-party 校验 |

## cache — 可选缓存与实时共享

`rust-webx-redis`（导入名 `webx_redis`）同时提供 `IDistributedCache`、事件传输
（Streams 竞争 + Pub/Sub 广播）、`IRealtimeEventStore`（共享续传）与 `IOnlinePresence`
（跨节点在线态）。装配入口仍是 `add_redis` + 宿主已有的 `use_realtime_event_store` /
`use_online_presence`，没有额外注册动词；单节点客户端，Cluster/Sentinel 与
`max_age` 物理保留等边界见 [known-boundaries.md](../../design/known-boundaries.md)。

## queue — 可选事件传输

`rust-webx-nats` / `rust-webx-rabbitmq` / `rust-webx-kafka` 分别提供 NATS、RabbitMQ、
Kafka 的 `IEventPublisher` / 消费者实现；投递语义（竞争/广播、at-least-once/at-most-once、
死信、Kafka 广播不支持）以
[broker 能力矩阵](../../design/broker-capability-matrix.md) 为唯一权威。装配入口：
`add_nats` / `add_rabbitmq` / `add_kafka`。

## push — 可选移动推送

`rust-webx-apns` / `rust-webx-fcm` 实现 `IPushSender` 端口，分别对接 APNs token auth 与
FCM HTTP v1；宿主用 `HostBuilder::use_push` 装配，端口只做三态结果映射、不做自动重试。
`PushPlatform::WebPush` 目前**只有端口、没有适配器**（C2，见
[known-boundaries.md](../../design/known-boundaries.md)）。

## transport — 可选监听传输组

`crates/transport/` 存放非 hyper 的监听后端。engine 提供传输中立端口
（`IHttpTransport` / `TransportRuntime` / `TransportRequest` / `TransportResponse`，见
[传输端口设计](../../design/transport-port.md)），后端负责 accept、协议握手与连接级排空：

| crate | 导入名 | 职责 |
|---|---|---|
| `rust-webx-h3` | `webx_h3` | HTTP/3（preview）：QUIC + TLS 1.3 + h3，`H3Transport` 实现 `IHttpTransport`，提供 `H3Config`、RFC 9220 WebSocket over h3 与 adapter-level WebTransport |

- **应用按需依赖**：`rust-webx-h3 = "2.9"`；core/伞不链接 QUIC/h3，默认构建不受影响。
- **复用同一条 pipeline**：路由、中间件、绑定、错误写入、鉴权、DI 与指标都走 engine 生产路径；
  适配器只做帧编解码与连接管理。
- **配置**：TLS 证书、mTLS、QUIC keep-alive/idle/stream limits 与请求体上限通过 `H3Config`
  显式配置；`HostBuilder::use_http_transport` 接入 Host 生命周期。

## client — 出站 HTTP 层

`rust-webx-client`（导入名 `webx_client`）是服务端调用外部 HTTP 服务的唯一入口：

| 类型 | 职责 |
|------|------|
| `HttpClient` | 池化客户端；每个 provider 一个实例（base URL、超时、池大小、代理、CA） |
| `IHttpClient` | 端口；Handler 依赖它，测试可替换 |
| `OutboundRequest` / `OutboundResponse` | 请求构造（字节/流式 body）与流式响应 |
| `ResponseStream::{collect, bytes, sse}` | 有上限收集 / 原始字节流 / SSE 解码 |
| `HttpClientRegistry` / `IHttpClientFactory` | 按名字解析客户端 |
| `HttpClientError` | 连接/超时/协议/响应体分类；`is_connect()` 供重试判定 |

它依赖 `core`（复用 `SseEvent` 类型），不依赖 `host`，因此也能用于后台任务或 CLI。版本跟随 workspace。
伞 crate 重导出上述传输类型，并提供 `HostBuilder::add_http_client` 把命名客户端注册进 DI；
AI 领域包（`rust-webx-ai`）仍不进入伞 crate。

## build — 编译期资源层

| Crate | 职责 |
|-------|------|
| `rust-webx-build` | 构建期把静态目录编译进可执行文件（`build-dependencies`） |

`rust-webx-build` 只在 `build.rs` 中运行，**不依赖任何框架 crate**（无 `core`，
也就不会把 tokio/hyper 拖进构建期）。它扫描目录、给可压缩的文件做 brotli 编码，再把
`EmbeddedAsset` 表写入 `OUT_DIR`，由 `webx::spa::embed_assets!()`（经
`#[webx::main(embed)]`）引入；`Host::build` 若发现已注册的表，会自动叠到 SPA 源上：

```rust
// build.rs — 编译期源树（库名 `webx`）
fn main() -> Result<(), webx::Error> {
    webx::builder()
        .web_root("wwwroot")
        .build()
}
```

应用入口用 `#[webx::main(embed)]` 显式链接该表；`.use_spa("wwwroot")` 只表示
运行期磁盘覆盖目录。不写 `(embed)` 则不烤进二进制。应用代码也可以用
`webx::assets::read_str("…")` 在运行期直接读取表里的文件，见
[在代码里读取内嵌文件](../11-production/openapi-spa.md#在代码里读取内嵌文件)。

它属于**编译期**层：文件集、MIME、内容 `ETag` 与压缩表示都在编译时确定。运行期配置
（`appsettings.json`、端口、密钥、TLS）不在此列，否则每次改配置都要重新编译。

依赖方向因此独立于运行时金字塔：

```
build.rs ──> rust-webx-build          （只读目录 + 写 OUT_DIR）
src/ ──────> rust-webx (伞) ──> host / macros / spa / openapi ──> core
```

## webx — 伞 Crate

统一 re-export，应用只需：

```toml
[dependencies]
rust-webx = "2.9"
```

```rust
use webx::*;
```

同时 re-export `rust_dix`、`async_trait`、`serde` 等常用依赖，减少 `Cargo.toml` 条目。

## 小结

Crate 分层体现了**接口与实现分离**：改 host 实现不影响业务代码，扩展 core trait 不影响已有实现。

下一节：[请求生命周期](request-lifecycle.md)

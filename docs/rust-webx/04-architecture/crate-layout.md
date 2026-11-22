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
| `rust-webx-host` | `webx_host` | Host / 路由 / 中间件 |
| `rust-webx-macros` | `webx_macros` | 过程宏（`#[get]`、`#[handler]`…） |
| `rust-webx-spa` | `webx_spa` | 静态文件 / SPA |
| `rust-webx-openapi` | `webx_openapi` | OpenAPI 生成 + UI |
| `rust-webx-ai` | `webx_ai` | AI 网关领域包（用量/记账/端口；不进入伞 crate） |
| `rust-webx-ai-ef` | `webx_ai_ef` | rust-ef 持久化用量账本（`EfUsageSink`） |
| `rust-webx-ai-gateway` | `webx_ai_gateway` | OpenAI 兼容 `/v1` HTTP 表面（依赖 webx + webx-ai） |
| `rust-webx-client` | `webx_client` | 出站 HTTP（连接池/流式/SSE；伞 crate 只重导出传输类型） |
| `rust-webx-ef` | `webx_ef` | rust-ef 边界适配：`EFError → webx::Error` 脱敏映射 + `EfResultExt`（应用按需依赖，core/伞不链接） |
| `rust-webx-build` | `webx` | 构建期（`build-dependencies`） |

对照 `rust-dix` / `rust-ef`：包名与导入名同样分别是 `rust-dix`→`rust_dix`、`rust-ef`→`rust_ef`。

## core — 契约层

`rust-webx-core` 定义所有公开 trait，**不依赖** hyper、tokio 或任何 HTTP 实现。

设计意图：
- 第三方可基于 core trait 编写可插拔组件
- 单元测试可 Mock 全部接口
- 可替换 HTTP 引擎（如从 hyper 迁移）

核心模块：

| 模块 | 职责 |
|------|------|
| `app` | `IHost` |
| `mediator` | `IRequest`, `IEventRequest`, `IMediator` trait + `Mediator` 实现 |
| `handler` | `IRequestHandler`, `IEventHandler`, `IHostedService` |
| `http` | `IHttpContext`, `IHttpRequest`, `IHttpResponse` |
| `middleware` | `IMiddleware` |
| `routing` | `IRouter`, `IEndpoint`, `RouteMeta` |
| `auth` | `IClaims`, `IAuthenticationHandler`, `IAuthenticationScheme`, `IAuthorizationPolicy`, `IDynamicAuthorizer` |
| `route` | 扫描类型、`HandlerCache`、`RouteEntry`、`IEndpointCatalog` / `EndpointCatalog` |
| `localization` | `TextRef`, `IStringLocalizer`, `StringLocalizer`, `LocalizationOptions` |
| `stream` | `SseEvent`, `SseResult`, `StreamEnd`（`text/event-stream` 响应） |
| `options_monitor` | `IOptionsMonitor`, `OptionsMonitor`（热更新快照 + 变更通知） |
| `metrics` | `IMeter`, `Counter`, `Histogram`, `Gauge`（`NoopMeter`/`MemoryMeter`） |

## host — 运行时层

`rust-webx-host` 实现 core 定义的 HTTP 相关 trait：

| 组件 | 实现 |
|------|------|
| HTTP 服务 | hyper + tokio |
| 路由 | matchit radix 树 `Router` |
| 上下文 | `webx_engine::EngineContext`（应用契约是 `IHttpContext`） |
| 管道 | `MiddlewarePipeline` |
| 认证 | `JwtAuth`, `jwt_middleware`, `AuthenticationMiddleware` + `JwtScheme` / `ApiKeyScheme` 链 |
| 授权 | 端点执行 `#[authorize]` 规则；`resource_auth_middleware` 已弃用（路由前无法读取 `route_pattern`） |
| 端点 | `StubEndpoint`（路由宏端点）, `RequestEndpoint`, `ControllerEndpoint`, `StaticJsonEndpoint`, `StaticHtmlEndpoint` |
| 宿主 | `Host`, `HostBuilder`, `Server` |

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

`crates/payment/` 是与 ai/auth/cache/queue 同级的封闭领域组：

| crate | 导入名 | 职责 |
|---|---|---|
| `rust-webx-pay` | `webx_pay` | 统一模型与端口：`Money`/`PaymentStatus`/`PaymentEvent`、`IPaymentProvider`、`IPaymentWebhookVerifier`、`PaymentWebhook`、`Payments` 运行时（缓存/单飞/失效）、`PaymentScope`、`IPaymentServiceFactory`/`IPaymentProviderFactory`/`IPaymentProviderBuilder` |
| `rust-webx-pay-stripe` | `webx_pay_stripe` | Stripe PaymentIntents/Checkout、退款、`Stripe-Signature` |
| `rust-webx-pay-alipay` | `webx_pay_alipay` | 支付宝 page/wap/app/precreate、RSA2 签名与异步通知 |
| `rust-webx-pay-wechatpay` | `webx_pay_wechatpay` | 微信支付 APIv3、请求签名、平台证书/公钥、AEAD 解密 |
| `rust-webx-pay-paypal` | `webx_pay_paypal` | Orders v2、OAuth2、capture/void/refund、验签 API |
| `rust-webx-pay-adyen` | `webx_pay_adyen` | Checkout payments/captures/refunds、HMAC 回调 |
| `rust-webx-pay-square` | `webx_pay_square` | Payments/Refunds、Payment Links、HMAC 回调 |
| `rust-webx-pay-ef` | `webx_pay_ef` | 基于 rust-ef 的回调 inbox：唯一去重、重试、死信 |

- **不进入伞 crate**：应用显式依赖所需适配器；不需要支付的项目不引入。
- **唯一 HTTP 栈**：适配器只依赖 `rust-webx-client`，不引入服务商 SDK（官方 SDK 无 Rust 版本）。
- **回调路由由应用声明**：crate 只提供原始 body 请求类型、验签与 ACK，不注入隐式路由。
- **多账户/服务商**：一个适配器实例可持有多个商户账户；请求可选账户，回调可用
  `/webhooks/payments/{provider}/{account}`。
- **版本跟随 workspace**：`version.workspace = true`，与 `rust-webx` 同一版本号。

## ef — ORM 边界组

`crates/ef/` 存放把 Web 框架与 ORM 连接起来的边界适配器；`rust-ef` 保持 Web 无关，
core/伞 crate 不链接它：

| crate | 导入名 | 职责 |
|---|---|---|
| `rust-webx-ef` | `webx_ef` | `EFError → webx::Error` 脱敏映射（404/400/409/503/500）与 `EfResultExt::map_ef()` |

- **应用按需依赖**：`rust-webx-ef = "2.7"`，调用点 `.map_ef()?`；参考应用在
  `handlers/src/db.rs` re-export 它。
- **无状态边界**：不建立连接、不读配置；多实例/容器语义与纯函数一致。
- **性能**：成功路径零开销，固定文案是常量，5xx 仅在日志侧拼接 source 链。

## outbox — 事务性 outbox 组

`crates/outbox/` 存放"业务事务与事件原子"的 starter：

| crate | 导入名 | 职责 |
|---|---|---|
| `rust-webx-outbox-ef` | `webx_outbox_ef` | 同事务入队（`enqueue_outbox`）、租约领取、事件派发、重试/死信、保留期清理 |

- **应用按需依赖**：`rust-webx-outbox-ef = "2.7"`；core/umbrella 不链接 rust-ef。
- **表由实体声明**：`webx_outbox` 由官方 rust-ef 迁移生成，`bootstrap` 仅测试。
- **与事件传输分工**：outbox 保证本地事务与事件同生共死；跨进程投递交给 `IEventPublisher`。

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
rust-webx = "1.5"
```

```rust
use webx::*;
```

同时 re-export `rust_dix`、`async_trait`、`serde` 等常用依赖，减少 `Cargo.toml` 条目。

## 小结

Crate 分层体现了**接口与实现分离**：改 host 实现不影响业务代码，扩展 core trait 不影响已有实现。

下一节：[请求生命周期](request-lifecycle.md)

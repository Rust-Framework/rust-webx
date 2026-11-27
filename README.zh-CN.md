[English](README.md) | **简体中文**

<div align="center">

# rust-webx

**一个受 ASP.NET Core 启发的 Rust WebApi 框架，基于 DI + Mediator 构建。**

一个类型安全、约定优于配置的平台——在这里 *请求即端点*：一行属性声明路由，实现一个 Handler，路由、依赖注入、中间件与错误映射全部交给框架。

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Documentation](https://img.shields.io/badge/docs-mdBook-blue.svg)](https://rust-framework.github.io/rust-webx/)
[![CI](https://github.com/Rust-Framework/rust-webx/actions/workflows/ci.yml/badge.svg)](https://github.com/Rust-Framework/rust-webx/actions/workflows/ci.yml)
![Rust](https://img.shields.io/badge/rust-%3E%3D%201.95-orange.svg)

</div>

---

## 目录

- [什么是 rust-webx](#什么是-rust-webx)
- [核心特性](#核心特性)
- [为什么选择 rust-webx](#为什么选择-rust-webx)
- [架构](#架构)
- [请求即端点：Hello World](#请求即端点hello-world)
- [核心概念](#核心概念)
- [消息与缓存适配器](#消息与缓存适配器)
- [可选传输适配器](#可选传输适配器)
- [示例应用](#示例应用)
- [快速开始](#快速开始)
- [开发](#开发)
- [生产就绪能力](#生产就绪能力)
- [环境变量](#环境变量)
- [异常映射](#异常映射)
- [文档](#文档)
- [许可证](#许可证)

---

## 什么是 rust-webx

> **如果你来自 ASP.NET Core，并希望在 Rust 里保留熟悉的 DI + Mediator 工程模型，那么 rust-webx 就是为你准备的。**

**rust-webx** 是一个受 **ASP.NET Core** 启发的 Rust WebApi 框架，以 **DI（依赖注入）+ Mediator（中介者）** 为双核心。通过 *请求即端点* 模式，开发者可以用类型安全的 Rust 代码定义完整的 HTTP API——无需手写路由表、无需手动注册 Handler、无需分散的错误处理。

与其自己拼装路由库、DI 容器、中间件与错误处理：

```
choose a router → hand-write route tables → design your own DI → wire middleware → unify errors → repeat
```

rust-webx 把这些横切关注点内聚到框架层：

| 痛点 | rust-webx 的解法 |
|------|-------------------|
| 路由与处理器脱节 | `IRequest<T>` 携带路由元数据；`#[get("/path")]` 编译时注册 |
| Handler 注册样板代码 | `#[handler]` 自动向 DI 容器注册 Handler |
| 模块间强耦合 | `IMediator::send()` 调度请求；`publish()` 发布事件 |
| 错误与 HTTP 状态码映射分散 | 统一 `Error` 类型 + 引擎错误管线（错误状态在 after hooks 前写入，metrics/tracing 能看到真实状态） |
| 认证授权各自为政 | `use_authentication()` + 声明式 `#[authorize]` |

rust-webx **不是** 全栈 UI 框架（可搭配任意前端）、**不是** ORM（自带或使用 [`rust-ef`](https://crates.io/crates/rust-ef)）、**也不是** 微服务治理平台。

### 适合谁

- **从 .NET 迁到 Rust 的团队。** 你熟悉的词汇几乎一一对应：`IRequest<T>` / `IRequestHandler<T, R>` 就是「请求 + 处理器」，`IMediator::send()` 就是 MediatR 风格的派发，`#[fromclaim] user: IdentityUser` 取代 `HttpContext.User`，`problem+json` 取代手写的异常中间件，`HostBuilder` 就是当年的 `Program.cs`。
- **分层业务服务。** 一个操作 = 一个 Request + 一个 Handler + 一个文件；契约、领域、应用、宿主四层 crate 让依赖方向显式可见，而不是放任一个两千行的 `routes.rs` 长出来。
- **上线首日就要生产能力的 API 服务。** 认证与会话、字段校验、OpenAPI、健康检查、事务性 outbox、实时推送与指标都是框架端口，而不是需要你自己粘起来的一堆 crate。

### 和谁比

| | 组合式 HTTP 工具包（`axum`、`actix-web`、`poem`、`rocket`） | rust-webx |
|---|---|---|
| 代码单元 | 路由处理函数 + 提取器（extractor） | 一个「本身就是端点」的请求类型（`IRequest<R>`）+ 它的 Handler |
| 仍需自己写 | 路由树、DI/State 方案、错误→响应转换、校验与 OpenAPI 选型 | 一套约定覆盖全部：`#[handler]`、`#[fromclaim]`、`#[validate]`、`#[authorize]`，边界错误直接 `?`（`From<EFError>` / `From<RdiError>` 在各自 crate 实现） |
| 注册方式 | 手写路由注册，或自己再加一层宏 | 路由与 Handler 元数据编译期（`inventory`）收集，`Host::build()` 一次装配 |
| 应用级能力 | 自带（BYO） | 认证/会话/Passkey、限流、幂等、outbox、实时、支付、OpenAPI、健康检查均为端口 |
| 逃生通道 | — | Handler 就是普通 Rust；中间件与 after 钩子拿到 `&mut dyn IHttpContext`，HTTP 层始终可达 |

rust-webx 是刻意「有主见」的：它不追求做最小的路由库。如果你想要一个极简 HTTP 工具包、并且享受自己挑选每一层，`axum` 这类框架更合适；如果你要把一个分层的 ASP.NET Core 服务迁到 Rust、并想在对岸保留同一套应用模型，这里的映射是直接的。

## 核心特性

- **DI + 中介者 双核心** — 以 `IRequest<T>` + `IRequestHandler<T, R>` 为载体，框架自动完成路由映射与 DI 解析。
- **编译时路由快捷键** — `#[get("/path")]`、`#[post("/path")]`、`#[put("/path")]`、`#[delete("/path")]` 一行定义完整端点。
- **编译时自动扫描** — 通过 `inventory` 在编译时收集路由元数据，在 `Host::build()` 时自动注册。
- **零配置 Handler 注册** — `#[handler]` 属性宏自动向 DI 容器注册 Handler。
- **认证与授权** — JWT Bearer（HS256/RS256/ES256 + JWKS/kid）、OIDC、Cookie 会话 + CSRF、刷新令牌轮换、TOTP/WebAuthn、mTLS、DPoP、HTTP Basic、RFC 7662/7009、OAuth 2.0 社交登录（GitHub/Google/Microsoft/GitLab/Gitee/钉钉/飞书/微信/微博/QQ/抖音/支付宝/Facebook），以及声明式 `#[authorize]`。机器身份另有四块：非对称 JWT 签发 + JWKS 发布（`Jwt.Signing`）、多 issuer/audience 资源服务器与 `scope` 校验、OAuth 2.0 client credentials 取机器令牌、外部 PDP 客户端——见 [JWT 与 JWKS](docs/rust-webx/09-auth-security/jwt-authentication.md)、[服务账号取 token](docs/rust-webx/09-auth-security/client-credentials.md)、[资源授权](docs/rust-webx/09-auth-security/resource-authorization.md)。快速上手见 [10 分钟认证快速上手](docs/rust-webx/09-auth-security/quickstart.md)。
- **身份即请求字段** — `#[fromclaim] user: IdentityUser` 把认证身份（id、名称、邮箱、租户、角色）绑定为普通字段；Handler 不再解析 token。`#[fromclaim(optional)] viewer: Option<IdentityUser>` 覆盖匿名端点（匿名访问必须显式声明）。
- **声明式校验** — 请求字段上的 `#[validate(length/range/email/custom)]` 在绑定后执行，失败汇入同一个 `400 problem+json` `errors`，并把约束同步进 OpenAPI（`minLength`/`minItems`/`minimum`/`format`）。
- **Keyset 分页、客户端信息、实例级校验** — `KeysetRequest`/`CursorPage` 与 offset `PagedRequest`/`PagedResult` 并存；`#[fromclient] ClientInfo` 绑定对端地址与受信 `X-Forwarded-For`；`RequestUser::require_owner`/`require_same_tenant` 收敛实例级 401/403。
- **事务性 outbox** — `rust-webx-outbox` 提供端口、基于租约的 `OutboxDispatcher`（经 `EventDispatcher`：退避/死信/保留期）与 `InMemoryOutbox` 默认实现，`add_outbox` 注册。同事务入队属于数据适配：EF 参考实现在 `examples/adapters/outbox/ef`（`add_outbox_ef`，非发布）。
- **面向 IM 规模的实时能力** — 一套事件模型、两种投递类（`EventDelivery::Competing` / `Broadcast`）：`RealtimeFanoutEvent` / `RealtimeUserFanoutEvent` 通过同一 `IEventPublisher` 跨节点扇出（Redis Pub/Sub、NATS 广播 subject、RabbitMQ fanout exchange），`RedisRealtimeEventStore` 共享续传日志，`IOnlinePresence`（含 Redis 实现与宿主心跳桥接）共享在线态，`WebSocketLimits::with_session_revalidation` 在 JWT/Cookie 会话被撤销后关闭实时连接（close `1008`）。见[多实例实时部署](docs/rust-webx/11-production/realtime-cluster.md)。
- **移动推送** — `IPushSender` 三态裁决（`Accepted`/`TokenInvalid`/`Retryable`），死亡 token 不会被当成服务器错误；`HostBuilder::use_push` 接入 APNs（`rust-webx-apns`）或 FCM（`rust-webx-fcm`），`InMemoryPushSender` 供测试捕获。见[推送指南](docs/rust-webx/13-extensibility/push-notifications.md)。
- **端口而非 starters** — 每个基础设施能力都是一个端口（`IIdempotencyStore`、`IOutbox`、`ISessionStore`、`IObjectStore`、`IMeter`、`IAdvisoryLock`、`IEmailSender` …）：要么提供需显式装配的内存实现（`IIdempotencyStore`、`IOutbox`、`IObjectStore`、`IAdvisoryLock`、`IMeter`），要么**刻意没有默认实现**（通知与推送发送器）——缺适配器会在启动期以 DI 错误失败，而不是静默 no-op。实体、表和数据/逻辑适配归下游服务：在业务 crate 实现端口，或复制 `examples/adapters/` 里的非发布参考实现，并跑 `testing/contract-tests` 的端口契约。见 [端口与适配器](docs/rust-webx/13-extensibility/integrations.md)。
- **端点元数据服务** — `IEndpointCatalog` 在运行时暴露全部端点（`all()` / `localized()`），权限同步、管理导航与 OpenAPI 共用一份元数据。
- **本地化** — `#[display]` / `#[description]` / `#[category]` 存资源键，全局 `Culture.Default` 决定语言；支持 JSON 资源文件或 `include_str!` 嵌入。
- **开箱即用的生产能力** — 优雅关闭、动态健康检查、安全响应头、请求 ID、CORS、TLS、速率限制、压缩、OpenAPI + SPA 托管。
- **启动期聚合校验** — `HostBuilder::try_build` 一次性按 section+key 排序列出全部非法配置（保留 Production fail-closed 守卫），不再“报第一个错就退出”。
- **Wire 级测试基座与 fuzz 语料** — `TestHost::connect_wire()` 用进程内双端管道直接跑真实 HTTP/1.1 keep-alive、h2c、SSE 字节与畸形请求（不绑端口）；独立 `fuzz/` cargo-fuzz workspace 覆盖 cursor/config/ClientInfo/ProblemDetails/密文解析。见 [test-host.md](docs/design/test-host.md) 与 [stability-gates.md](docs/design/stability-gates.md)。
- **稳定的观测契约** — `webx.http` span 树（request/auth/route/handler/binding/response）与 `webx.connection` 打开/关闭事件；span/事件名与字段作为兼容性契约写入 [observability-contract.md](docs/design/observability-contract.md)。
- **关闭原因与错误码注册表** — `CloseReason`、`IConnection::close_reason()`、`ShutdownToken::is_draining()` 统一 WebSocket/SSE 停机语义；23 条登记错误码（`ERROR_CODES`）与关闭码表防止 `problem+json` 的 `code` 与 close frame 漂移。见 [close-codes.md](docs/design/close-codes.md) 与 [error-codes.md](docs/design/error-codes.md)。
- **准入不变量指标** — `/metrics` 暴露 active/capacity 比例、不变量违规 gauge 与停机 gauge，permit/wait 记账泄漏无需调试器即可发现。

- **文件上传与下载** — `multipart/form-data` 以类型化 `FormFile` 绑定（流式接收、按阈值落盘、大小受控）；下载提供对齐 ASP.NET Core 的结果类型（`FileResult::path` / `::stream` / `::seekable_stream`、`ContentResult`），自带 `Content-Disposition`、`ETag`、`Last-Modified` 与 `Range`。
- **ORM 无关** — 框架不依赖 `rust-ef`；`docbit` 参考应用用极少量胶水代码接入。
- **支付领域组** — 按需引入 `rust-webx-pay*`：Stripe、支付宝、微信支付 v3、PayPal、Adyen、Square 共用
  一套 `IPaymentProvider` 端口，支持运行时 `IPaymentServiceFactory`/`PaymentScope` 配置、前端支付方式目录
  （`PaymentOption`）、多账户/服务商、原始 body 回调验签、各供应商 ACK。持久化回调 inbox 属于数据适配：
  EF 参考实现在 `examples/adapters/payment/ef`（非发布，复制使用）。
  见[支付章](docs/rust-webx/13-extensibility/payments.md)。
- **Mediator 不增加认知负担** — `#[handler]` 对应的 Request 没有 HTTP 路由时，自动作为 `IMediator` handler；响应不要求 `Serialize`。HTTP 能力属于路由，而不是 handler 上的模式开关。

## 为什么选择 rust-webx

| 优势 | 实际意义 |
|---|---|
| **对齐 ASP.NET Core 的开发模型** | 请求即端点：`impl IRequest<R>` 上写 `#[get]`/`#[post]`，实现上写 `#[handler]`，DI 与中间件在 `Host` 一次配置。 |
| **编译期分派，没有字符串路由表** | 路由元数据和直接 Handler bridge 由 `inventory` 收集，`Host::build()` 负责 join，配置不一致启动即失败。 |
| **低分配热路径** | direct JSON 路径不经过 mediator，也不装箱 request/response；`/hello` 参考基准的框架税为 **10.42 allocs/request**（hyper 地板 6.00，Windows 参考；Linux CI 12.42），CI 门禁 ≤13（Windows 参考 ≤12）。 |
| **性能不以正确性为代价** | allocation 计数、wire-level 正确性和稳定性套件都是发布门禁。见[分配预算](docs/design/alloc-budget.md)与[稳定性门禁](docs/design/stability-gates.md)。 |
| **生产级准入与生命周期** | 所有 listener 共享一个全局连接 governor、首请求超时、HTTP/2 keepalive、优雅/立即关闭 guard、Host 单次运行状态、per-worker SSE drain。 |
| **错误语义可信** | 错误响应在逆序 after hooks 之前写入，metrics/tracing/compression 看到真实状态；失败的成功路径不会残留 `cache-control`/`etag`/`content-encoding` 等表示头。 |
| **原生优先的集成** | `rust-dix` 1.1 DI scope、`rust-ef` 2.6+ ORM（已按 2.9.1 配对验证）、JWT/session/password 端口、`rust-webx-client` 都是框架能力，而非应用层临时胶水。 |
| **HTTP 与进程内能力边界清晰** | 无路由的 `#[handler]` 自动是 mediator-only；有路由才增加响应 encoder。没有 `#[handler(in_process)]` 模式开关，也不会把 command handler 意外暴露成 HTTP 端点。 |

以上性能数字是同一台主机同窗口的本地对比，不构成绝对容量承诺；allocation 计数和 wire-level 正确性测试才是发布回归信号。方法与预算见[分配预算](docs/design/alloc-budget.md)。

## 架构

`rust-webx` 是薄伞：运行时核心很小，领域/适配能力拆成独立 crate，按需引入：

```text
rust-webx（伞）
├── core       契约 trait + rust-core 共享契约重导出
├── engine     HTTP 执行：worker/路由/管道/传输
├── host       装配：认证/中间件/健康检查/WebSocket/presence/DI
├── macros     #[get]/#[handler]/#[authorize]/#[derive(WebxRequestMeta)]
├── spa / openapi / client / outbox
└── 可选：ai、auth、cache(redis)、queue(nats/rabbitmq/kafka)、
         payment、push(apns/fcm)、transport(h3)
```

已发布 crate 的权威生成清单（30 个、包名/导入名/publish 状态）见
[crate-layout.md](docs/rust-webx/04-architecture/crate-layout.md)。

### Crate 结构与导入名

已发布 crate 的权威清单（包名、导入名、publish 状态、分组与职责）由 `cargo metadata`
生成，见 [docs/rust-webx/04-architecture/crate-layout.md](docs/rust-webx/04-architecture/crate-layout.md)。
README 不再维护第二份全量清单；上面只列常用依赖类别。

> **包名 vs 导入名**：依赖写 `rust-webx`，代码写 `use webx::*;`。子 crate 规则是
> `rust-webx-<name>` → `webx_<name>`（`rust-webx` → `webx`），完整对照见生成清单。

## 请求即端点：Hello World

声明一个请求、在它上面映射路由、并实现其 Handler——无需维护路由表：

```rust
use webx::*;

struct HelloRequest;

#[get("/hello")]
impl IRequest<String> for HelloRequest {}

#[derive(Default)]
struct HelloHandler;

#[handler] // 编译时注册进 DI 容器
#[async_trait]
impl IRequestHandler<HelloRequest, String> for HelloHandler {
    async fn handle(&mut self, _req: HelloRequest) -> ActionResult<String> {
        Ok("Hello, World! Welcome to Rust WebX.".to_string())
    }
}

#[tokio::main]
async fn main() {
    Host::builder()
        .build()
        .run()
        .await
        .expect("Server failed");
}
```

路由发现、Handler 解析、中间件编排与优雅关闭，全部由 `Host::build()` 自动完成。

### 组合根：装配槽

应用的装配只经过 builder 上固定的几个槽：

| 槽 | 放什么 | 示例 |
|---|---|---|
| `register` | DI 服务、端口与适配器（对象图的唯一来源） | `.register(\|svc\| svc.add_mediator())` |
| `configure` | 本 Host 的 `AppOptions` 运行期开关 | `.configure(\|app\| app.options(\|o\| o.metrics.enabled = false))` |
| `add_options` / `add_options_with` | 一个配置节绑定一个 `T`；`_with` 在绑定后应用代码级覆盖 | `.add_options_with::<SiteConfig>("Site", \|s\| s.public_url = site_url)` |

`use_spa`、`use_authentication`、`use_http_transport`、`use_alt_svc` 等是**同一 builder 状态的糖**，
不引入第三套装配通道。业务代码只消费接口，只有组合根知道具体实现。

六个动词（`add_*` / `use_*` / `register` / `configure` / `add_options*` / `watch_options_file`）
与槽位表见 [api-convergence.md](docs/design/api-convergence.md)。

## 核心概念

| 概念 | 说明 |
|------|------|
| `IRequest<TResponse>` | 携带响应类型的泛型请求标记。`IRequest<()>` 返回 `204 No Content`。 |
| `IRequestHandler<T, R>` | 双类型参数 Handler；`T` 为请求类型，`R` 为响应类型。 |
| `#[get("/path")]` | 路由快捷键，标注在 `impl IRequest<T>` 块上，编译时注册。 |
| `#[post("/path")]` / `#[put("/path")]` / `#[delete("/path")]` | HTTP 方法路由快捷键。 |
| `IMediator` | 中介者；`send()` 分发请求，`publish()` 把事件交给事件传输层（非阻塞、不返回处理器结果）。 |
| `IMiddleware` | 中间件；顺序管道，可短路请求。 |
| `IPipelineBehavior` | Mediator 管道拦截器，可包装请求处理链。 |
| `IEventHandler<T>` | 事件处理器；`publish()` 通过 `IEventPublisher`（默认内存队列）异步投递。 |
| `Error` | 统一错误类型，自动映射到 HTTP 状态码。 |
| `IClaims` / `IAuthenticationHandler` | JWT 认证接口，从 Bearer Token 提取身份。 |
| `RequestUser` | Handler 内实例级归属校验：`require_owner` / `require_same_tenant`（401/403 分离）；路由级规则仍写在 `#[authorize]` 上。 |
| `IDynamicAuthorizer` | 路由级运行时授权（DB/cache/功能开关），在静态 `#[authorize]` 规则之后执行。 |
| `IEndpointCatalog` | 运行时端点元数据（`all()` / `localized()`）；DI 注入或 `host.endpoints()`。 |
| `IStringLocalizer` | 端点文本/OpenAPI 的资源键查找；默认 culture 来自 `Culture.Default`。 |

### 请求处理流程

```
HTTP Request
    │
    ▼
HttpContext::new(req).await            ← 读取 body bytes
    │
    ▼
MiddlewarePipeline::execute()          ← 中间件按注册顺序执行；
    │                                      每个都可能短路响应
    ▼
Router::match_route(ctx)               ← 对 method + path 做 Trie 匹配；
    │                                      {param} 值 → route_params
    ▼
Route matched?  ──No──▶  404 "Not Found"
    │ Yes
    ▼
IEndpoint::handle(ctx)                 ← 调用 handler，序列化响应
    ▼
HttpResponse → hyper::Response         ← 出错时，内置异常中间件映射状态码
    ▼
JSON: RFC 9457 application/problem+json (4xx / 5xx)
```

## 消息与缓存适配器

事件传输与分布式缓存都是可选适配器 crate：一次 `add_*` 注册即接入 `IEventPublisher` 与后台消费者，
`publish()` 调用与 `#[handler]` 事件处理器写法不变。

| crate | 入口 | 投递语义 |
|-------|------|----------|
| [`rust-webx-redis`](https://crates.io/crates/rust-webx-redis) | `add_redis(RedisOptions::new(url))`，同时注册 `IDistributedCache` | 至少一次（Redis Streams consumer group；处理器后 `XACK`，pending 可认领，重试有上限→死信流） |
| [`rust-webx-nats`](https://crates.io/crates/rust-webx-nats) | `add_nats(NatsOptions::new(url))` | 至多一次（core NATS；可选 queue group） |
| [`rust-webx-rabbitmq`](https://crates.io/crates/rust-webx-rabbitmq) | `add_rabbitmq(RabbitMqOptions::new(url))` | 至少一次（quorum topic queue、publisher confirm、手动 ack + 失败重入队、`x-delivery-limit` + 可选 DLX） |
| [`rust-webx-kafka`](https://crates.io/crates/rust-webx-kafka) | `add_kafka(KafkaOptions::new(brokers, topic))` | 单分区 follower；分区内有序；不做 consumer-group offset 提交（重启按配置的 start_offset）；参考/起步实现 |

只缓存：`RedisOptions::cache_only()`。只发布：NATS/RabbitMQ/Kafka 用 `.publisher_only()`；Redis 用
`.without_cache()` + `RedisEventsOptions { consumer_enabled: false, .. }`。详见
[事件发布与订阅](docs/rust-webx/08-mediator-events/event-pub-sub.md) 与
[缓存与速率限制](docs/rust-webx/11-production/caching-rate-limit.md)。

## 可选传输适配器

内置 hyper HTTP/1.1 + HTTP/2 监听之外，engine 提供传输中立的端口（`IHttpTransport` /
`TransportRuntime`）：同一个 router、middleware、绑定、错误写入、DI、准入 governor 与指标。
HTTP/3 是可选 crate，默认构建不会引入 QUIC 依赖。

| crate | 入口 | 说明 |
|-------|------|------|
| [`rust-webx-h3`](https://crates.io/crates/rust-webx-h3) | `H3Transport` + `HostBuilder::use_http_transport(Arc::new(transport))`，可选 `use_alt_svc("h3=\":<udp_port>\"; ma=86400")`、可选 `H3Config::with_webtransport_handler` | QUIC + rustls + h3（`trev-h3` fork）、TLS 1.3 / ALPN `h3`、mTLS、SSE/流式响应、每连接 GOAWAY 排空、共享 `App.MaxConnections` 准入、`/metrics` 传输维度标签、显式 `Alt-Svc`、**HTTP/3 WebSocket（RFC 9220 extended CONNECT）**、**WebTransport adapter-level MVP**（session + bidi stream echo；transport-neutral 端口后续）。 |

## 示例应用

本仓库内置三个完整的参考应用，展示框架本身、分层项目结构（`contracts` / `domain` / `handlers` / `host`）、JWT 认证、RBAC、实时通道与 SPA 托管。每个应用都是 `examples/` 下的独立 workspace（见 [`examples/README.md`](examples/README.md)）：

| 应用 | 说明 | 开发地址 |
|------|------|----------|
| [`docbit`](examples/docbit/) | 作品集 + 博客 + RBAC + 完整文档站（`docs/`）。使用 `rust-ef`（开发与生产均为 SQLite）。 | <http://localhost:5000> |
| [`dmbit`](examples/dmbit/) | 数据中心机房的设备与库存管理（设备/产品/规格/库存管理）。基于 SQLite。 | <http://localhost:5100> |
| [`imbit`](examples/imbit/) | 实时 IM：HTTP + WebSocket 同宿主，房间消息与断线补拉、信令、TURN 凭据、录制。 | <http://localhost:8090> |

本地运行参考应用（每个应用独立 workspace，需带 `--manifest-path`）：

```bash
cargo run --manifest-path examples/docbit/Cargo.toml -p docbit-host   # → http://localhost:5000
cargo run --manifest-path examples/dmbit/Cargo.toml -p dmbit-host     # → http://localhost:5100
cargo run --manifest-path examples/imbit/Cargo.toml -p imbit-host     # → http://localhost:8090（bin 名 imbit）
```

生产部署细节参见 [`examples/docbit/PRODUCTION.md`](examples/docbit/PRODUCTION.md)。

## 快速开始

### 构建

```bash
# 针对某个应用做 release 构建
cargo build --release --manifest-path examples/docbit/Cargo.toml -p docbit-host
cargo build --release --manifest-path examples/dmbit/Cargo.toml -p dmbit-host
```

### 运行参考应用

```bash
cargo run --manifest-path examples/docbit/Cargo.toml -p docbit-host     # 开发模式（SQLite），http://localhost:5000
```

### 发布（裸机）

```bash
# Linux / macOS
chmod +x examples/docbit/publish.sh
./examples/docbit/publish.sh /opt/docbit --production

# Windows
.\examples\docbit\publish.ps1 -Destination D:\deploy\docbit -Production
```

启动前设置 `DATABASE_URL` 与 `JWT_SECRET`，或编辑生成的 `run.sh` / `run.cmd`。

### Docker（可选）

主部署方式为 **`examples/docbit/publish.sh` 裸机 Linux 发布**（见 [docbit/PRODUCTION.md](examples/docbit/PRODUCTION.md)）。Docker 文件仅作参考，不在 CI 中维护：

```bash
# 独立镜像（使用前请本地验证）
docker build -f examples/docbit/Dockerfile .

# 本地栈（docbit + SQLite volume）
cp examples/docbit/.env.example examples/docbit/.env     # 编辑 JWT_SECRET
docker compose -f examples/docbit/docker-compose.yml --env-file examples/docbit/.env up --build
# → http://localhost:8100
```

## 开发

在仓库根运行完整测试套件：

```bash
cargo test --workspace --no-fail-fast
```

### 磁盘占用（Windows 大 checkout）

`target/` 是可丢弃的，但 Cargo 从不删除属于旧指纹的产物，因此每次完整重建（工具链升级、
`Cargo.lock` 更新、`RUSTFLAGS` 变化、切换 feature 组合）都会把上一整代产物留在磁盘上。
[`Cargo.toml`](Cargo.toml) 的 dev/test profile 让**新**构建保持精简（依赖不生成 debug info、
workspace crate 只生成行表、关闭 incremental）。若 `target/` 已经变大，先查看、再回收：

```powershell
.\scripts\clean-target.ps1              # 报告大小，不修改任何文件
.\scripts\clean-target.ps1 -Incremental # 只删 incremental 编译缓存
.\scripts\clean-target.ps1 -All         # cargo clean（所有产物都可重建）
```

调试或快速编辑会话可只对当前 shell 覆盖已提交的 profile：

```powershell
$env:CARGO_PROFILE_DEV_DEBUG = '2'             # workspace crate 完整 debug info
$env:CARGO_PROFILE_DEV_INCREMENTAL = 'true'    # 重新开启增量编译
```

## 生产就绪能力

| 能力 | 状态 |
|------|------|
| 优雅关闭（Ctrl+C / SIGTERM） | ✅ |
| 连接 drain（Production 30 秒） | ✅ |
| Header 读超时（slowloris） | ✅ 30 秒，header 缓冲有界 |
| 健康检查 `/health` `/health/ready`（运行时探针，失败 → 503） | ✅ |
| 安全响应头 + 请求 ID | ✅ 默认启用 |
| JWT production fail-fast | ✅ |
| CORS `*` production fail-fast | ✅ |
| panic 隔离（handler → 500，进程存活） | ✅ |
| OpenAPI UI | 仅 Development 模式 |
| 速率限制 / 压缩 | 应用层 opt-in（`use_middleware`） |
| 文件上传（流式、落盘、限流） | ✅ |
| 文件下载（`FileResult` 形态、流式、`Content-Disposition`、`ETag`、`Range`、`304`、`HEAD`） | ✅ |
| TLS | ✅ 通过 `App.Urls` + `Tls.CertPath/KeyPath` |

## 环境变量

- `APP_ENV` — 应用环境（`Development` / `Production`）。
- `WEBX_APP_BASE` — 应用基础目录。
- `JWT_SECRET` — JWT 签名密钥。所有模式都会读取；`APP_ENV=Production` 额外拒绝短于 32 字节或按知占位符的密钥。
- `APP__Jwt__Secret` — 覆盖 `JWT_SECRET`。
- `DATABASE_URL` — 可选数据库覆盖（docbit 默认使用可执行文件旁的 SQLite `app.db`）。
- `APP__*` — 覆盖 `appsettings.json` 任意键的内联 JSON（如 `APP__App__Urls`、`APP__Cors__Origins`）。

完整变量参考见 `docs/rust-webx/` 下的配置章节。

## 异常映射

`webx::Error` 就是 `rust_core::AppError`（2.9.1 起：kind + 私有字段，构造走构造器、读取走访问器，
2.9 的 enum 已删除）。`kind` 决定默认 status/code/retryable，构造器可以收紧。稳定的
构造器 → `code`/状态/可重试映射、兜底规则与 `ERROR_CODES` 注册表只在
[error-codes.md](docs/design/error-codes.md) 维护；业务代码不要复制整张表。

常用示例：

- `Error::validation(msg)` → `400 validation_failed`
- `Error::not_found(msg)` → `404 not_found`
- `Error::internal(operator)` → `500 internal_error`（operator 只进日志）

边界错误不需要适配层：`EFError` / `RdiError` 在各自 crate 实现 `From`，Handler 直接用 `?`。
分类权威见 [app-error-migration.md](docs/design/app-error-migration.md)。

## 文档

- **在线手册** — 全书发布在 <https://rust-framework.github.io/rust-webx/>（mdBook，中文；由 `.github/workflows/docs.yml` 从 `docs/rust-webx/` 构建）。
- **rust-webx 开发者手册** — 位于 [`docs/rust-webx/`](docs/rust-webx/INDEX.md) 的渐进式披露书籍（16 章，中文）：入门、快速上手、架构、DI 与生命周期、中间件、中介者与事件、认证与安全、配置、生产、项目结构、扩展、最佳实践、案例研究与迁移指南。
- **升级** — 2.9.1 是破坏性补丁（原计划的 3.0 契约迁移并入该补丁）；2.9.2 是加性 IM 硬化
  （无需迁移）；2.9.3 是边界收口补丁，其已登记行为变更（保留扇出事件上的 `#[handler]` 使
  host 构建失败——不要手工注册；Redis `start_id` 默认 `"0"`；Redis presence key 布局）见
  [compatibility.md](docs/design/compatibility.md) §3，升级窗口见
  [known-boundaries.md](docs/design/known-boundaries.md) C4。2.x 加性演进线见
  [2.5 升级指南](docs/rust-webx/16-migration/upgrade-to-2.5.md)；2.9.1 逐条改动见
  [2.9.1 升级指南](docs/rust-webx/16-migration/upgrade-to-2.9.1.md)。
- **rust-ef 参考** — ORM 手册随 [`rust-ef`](https://crates.io/crates/rust-ef) crate 维护（见其 [docs.rs](https://docs.rs/rust-ef) 文档），`docbit` 示例使用。
- `docbit` 参考应用也把这些文档作为实时网站提供（`GET /api/docs/rust-webx/...`）。Monorepo 开发时 `DocService` 按 slug 实时解析 sibling 仓库文档，无需将 sibling 文档复制到 `rust-webx/docs/`；standalone 发布时 `examples/docbit/publish.*` 在打包时从源仓库复制文档进 bundle。可通过 `RUST_FRAMEWORK_ROOT` 显式指定 monorepo 根目录。

## 许可证

[MIT](LICENSE)

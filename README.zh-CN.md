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
- [生产就绪能力](#生产就绪能力)
- [环境变量](#环境变量)
- [异常映射](#异常映射)
- [文档](#文档)
- [许可证](#许可证)

---

## 什么是 rust-webx

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
| 认证授权各自为政 | `add_authentication()` + 声明式 `#[authorize]` |

rust-webx **不是** 全栈 UI 框架（可搭配任意前端）、**不是** ORM（自带或使用 [`rust-ef`](https://crates.io/crates/rust-ef)）、**也不是** 微服务治理平台。

## 核心特性

- **DI + 中介者 双核心** — 以 `IRequest<T>` + `IRequestHandler<T, R>` 为载体，框架自动完成路由映射与 DI 解析。
- **编译时路由快捷键** — `#[get("/path")]`、`#[post("/path")]`、`#[put("/path")]`、`#[delete("/path")]` 一行定义完整端点。
- **编译时自动扫描** — 通过 `inventory` 在编译时收集路由元数据，在 `Host::build()` 时自动注册。
- **零配置 Handler 注册** — `#[handler]` 属性宏自动向 DI 容器注册 Handler。
- **认证与授权** — JWT Bearer（HS256/RS256/ES256 + JWKS/kid）、OIDC、Cookie 会话 + CSRF、刷新令牌轮换、TOTP/WebAuthn、mTLS、DPoP、HTTP Basic、RFC 7662/7009、OAuth 2.0 社交登录（GitHub/Google/Microsoft/GitLab/Gitee/钉钉/飞书/微信/微博/QQ/抖音/支付宝/Facebook），以及声明式 `#[authorize]`。见 [10 分钟认证快速上手](docs/rust-webx/09-auth-security/quickstart.md)。
- **身份即请求字段** — `#[fromclaim] user: IdentityUser` 把认证身份（id、名称、邮箱、租户、角色）绑定为普通字段；Handler 不再解析 token。`#[fromclaim(optional)] viewer: Option<IdentityUser>` 覆盖匿名端点（匿名访问必须显式声明）。
- **声明式校验** — 请求字段上的 `#[validate(length/range/email/custom)]` 在绑定后执行，失败汇入同一个 `400 problem+json` `errors`，并把约束同步进 OpenAPI（`minLength`/`minItems`/`minimum`/`format`）。
- **Keyset 分页、客户端信息、实例级校验** — `KeysetRequest`/`CursorPage` 与 offset `PagedRequest`/`PagedResult` 并存；`#[fromclient] ClientInfo` 绑定对端地址与受信 `X-Forwarded-For`；`RequestUser::require_owner`/`require_same_tenant` 收敛实例级 401/403。
- **事务性 outbox** — 按需引入 `rust-webx-outbox-ef`：同事务入队、`EventDispatcher` 租约派发、退避/死信/保留期，`add_outbox` 注册。
- **端点元数据服务** — `IEndpointCatalog` 在运行时暴露全部端点（`all()` / `localized()`），权限同步、管理导航与 OpenAPI 共用一份元数据。
- **本地化** — `#[display]` / `#[description]` / `#[category]` 存资源键，全局 `Culture.Default` 决定语言；支持 JSON 资源文件或 `include_str!` 嵌入。
- **开箱即用的生产能力** — 优雅关闭、动态健康检查、安全响应头、请求 ID、CORS、TLS、速率限制、压缩、OpenAPI + SPA 托管。
- **启动期聚合校验** — `HostBuilder::try_build` 一次性按 section+key 排序列出全部非法配置（保留 Production fail-closed 守卫），不再“报第一个错就退出”。
- **Wire 级测试基座与 fuzz 语料** — `TestHost::connect_wire()` 用进程内双端管道直接跑真实 HTTP/1.1 keep-alive、h2c、SSE 字节与畸形请求（不绑端口）；独立 `fuzz/` cargo-fuzz workspace 覆盖 cursor/config/ClientInfo/ProblemDetails/密文解析。见 [test-host.md](docs/design/test-host.md) 与 [stability-gates.md](docs/design/stability-gates.md)。
- **稳定的观测契约** — `webx.http` span 树（request/auth/route/handler/binding/response）与 `webx.connection` 打开/关闭事件；span/事件名与字段作为兼容性契约写入 [observability-contract.md](docs/design/observability-contract.md)。
- **关闭原因与错误码注册表** — `CloseReason`、`IConnection::close_reason()`、`ShutdownToken::is_draining()` 统一 WebSocket/SSE 停机语义；22 条已登记错误码（`ERROR_CODES`）与关闭码表防止 `problem+json` 的 `code` 与 close frame 漂移。见 [close-codes.md](docs/design/close-codes.md) 与 [error-codes.md](docs/design/error-codes.md)。
- **准入不变量指标** — `/metrics` 暴露 active/capacity 比例、不变量违规 gauge 与停机 gauge，permit/wait 记账泄漏无需调试器即可发现。

- **文件上传与下载** — `multipart/form-data` 以类型化 `FormFile` 绑定（流式接收、按阈值落盘、大小受控）；下载提供对齐 ASP.NET Core 的结果类型（`FileResult::path` / `::stream` / `::seekable_stream`、`ContentResult`），自带 `Content-Disposition`、`ETag`、`Last-Modified` 与 `Range`。
- **ORM 无关** — 框架不依赖 `rust-ef`；`docbit` 参考应用用极少量胶水代码接入。
- **支付领域组** — 按需引入 `rust-webx-pay*`：Stripe、支付宝、微信支付 v3、PayPal、Adyen、Square 共用
  一套 `IPaymentProvider` 端口，支持运行时 `IPaymentServiceFactory`/`PaymentScope` 配置、前端支付方式目录
  （`PaymentOption`）、多账户/服务商、原始 body 回调验签、各供应商 ACK 与 rust-ef inbox。
  见[支付章](docs/rust-webx/13-extensibility/payments.md)。
- **Mediator 不增加认知负担** — `#[handler]` 对应的 Request 没有 HTTP 路由时，自动作为 `IMediator` handler；响应不要求 `Serialize`。HTTP 能力属于路由，而不是 handler 上的模式开关。

## 为什么选择 rust-webx

| 优势 | 实际意义 |
|---|---|
| **对齐 ASP.NET Core 的开发模型** | 请求即端点：`impl IRequest<R>` 上写 `#[get]`/`#[post]`，实现上写 `#[handler]`，DI 与中间件在 `Host` 一次配置。 |
| **编译期分派，没有字符串路由表** | 路由元数据和直接 Handler bridge 由 `inventory` 收集，`Host::build()` 负责 join，配置不一致启动即失败。 |
| **低分配热路径** | direct JSON 路径不经过 mediator，也不装箱 request/response；`/hello` 参考基准的框架税为 **10.42 allocs/request**（hyper 地板 6.00），CI 门禁 ≤12。 |
| **性能不以正确性为代价** | allocation 计数、wire-level 正确性和稳定性套件都是发布门禁。见[分配预算](docs/design/alloc-budget.md)与[稳定性门禁](docs/design/stability-gates.md)。 |
| **生产级准入与生命周期** | 所有 listener 共享一个全局连接 governor、首请求超时、HTTP/2 keepalive、优雅/立即关闭 guard、Host 单次运行状态、per-worker SSE drain。 |
| **错误语义可信** | 错误响应在逆序 after hooks 之前写入，metrics/tracing/compression 看到真实状态；失败的成功路径不会残留 `cache-control`/`etag`/`content-encoding` 等表示头。 |
| **原生优先的集成** | `rust-dix` 1.0 DI scope、`rust-ef` 2.6+ ORM（已按 2.8.0 配对验证）、JWT/session/password 端口、`rust-webx-client` 都是框架能力，而非应用层临时胶水。 |
| **HTTP 与进程内能力边界清晰** | 无路由的 `#[handler]` 自动是 mediator-only；有路由才增加响应 encoder。没有 `#[handler(in_process)]` 模式开关，也不会把 command handler 意外暴露成 HTTP 端点。 |

以上性能数字是同一台主机同窗口的本地对比，不构成绝对容量承诺；allocation 计数和 wire-level 正确性测试才是发布回归信号。方法与预算见[分配预算](docs/design/alloc-budget.md)。

## 架构

框架被拆分为一组职责聚焦的 crate，通过 `rust-webx` 伞 crate 统一重新导出：

```
┌──────────────────────────────────────────────────────────┐
│              rust-webx  (umbrella crate)                   │
│  re-exports core / host / macros / spa / openapi + rust_dix │
├──────────┬──────────┬──────────┬─────────────────────────┤
│rust-webx-│rust-webx-│rust-webx-│ rust-webx-macros          │
│host      │core      │engine    │ #[get] #[post] #[handler] │
│Host +    │traits +  │worker    │ #[authorize] #[fromclaim] │
│pipeline  │config    │pool +    │                           │
│          │          │dispatch  │                           │
├──────────┴──────────┴──────────┴─────────────────────────┤
│                    rust-webx-core                        │
│  IHost / IHttpContext / IMiddleware / IRequestHandler    │
│  IMediator / AppOptions / Error / AppMode                │
└──────────────────────┬───────────────────────────────────┘
                       │
          ┌────────────┼────────────┐
          ▼            ▼            ▼
   ┌──────────┐ ┌──────────┐ ┌──────────┐
   │ rust-dix │ │  hyper   │ │inventory │
   │ (DI)     │ │ (HTTP)   │ │ (route   │
   └──────────┘ └──────────┘ └────╌─────┘
                                   collection
```

### Crate 结构

```
rust-webx/
├── Cargo.toml                 # workspace 根 (v2.9.0)
├── crates/
│   ├── core/                  # rust-webx-core  — trait 与配置
│   ├── engine/                # rust-webx-engine — 执行引擎：worker 池、直接分发
│   ├── host/                  # rust-webx-host  — Host 构建器、中间件管道
│   │                          #   Trie 路由器、hyper 集成
│   ├── macros/                # rust-webx-macros — 过程宏
│   ├── spa/                   # rust-webx-spa   — 静态文件 / SPA 中间件
│   ├── openapi/               # rust-webx-openapi — OpenAPI 规范生成 + UI
│   ├── client/                # rust-webx-client — 出站 HTTP：连接池、流式、SSE
│   ├── webx/                  # rust-webx       — 伞 crate（重新导出）
│   ├── ai/                    # AI 领域组
│   │   ├── core/              #   rust-webx-ai         — AI 网关领域包
│   │   ├── ef/                #   rust-webx-ai-ef      — rust-ef 持久化用量账本
│   │   └── gateway/           #   rust-webx-ai-gateway — OpenAI 兼容 /v1 表面
│   ├── auth/                  # 认证领域组
│   │   ├── ef/                #   rust-webx-auth-ef — 持久化会话/刷新令牌
│   │   ├── oidc/              #   rust-webx-oidc
│   │   ├── oauth2/            #   rust-webx-oauth2
│   │   ├── mfa/               #   rust-webx-mfa
│   │   └── webauthn/          #   rust-webx-webauthn
│   ├── cache/                 # 第三方缓存适配器
│   │   └── redis/             #   rust-webx-redis — IDistributedCache + 事件传输
│   ├── queue/                 # 第三方消息队列适配器
│   │   ├── nats/              #   rust-webx-nats
│   │   ├── rabbitmq/          #   rust-webx-rabbitmq
│   │   └── kafka/             #   rust-webx-kafka
│   ├── ef/                    # ORM 边界适配器
│   │   └── webx/              #   rust-webx-ef — EFError → webx::Error 脱敏映射
│   ├── outbox/                # 事务性 outbox starter
│   │   └── ef/                #   rust-webx-outbox-ef — 同事务入队 + 租约派发
│   ├── transport/             # 可选的非 hyper 监听后端
│   │   └── h3/                #   rust-webx-h3 — HTTP/3（QUIC + rustls + h3）
│   └── payment/               # 支付领域组（按需引入）
│       ├── core/              #   rust-webx-pay — 统一模型/端口/Payments 运行时
│       ├── stripe/            #   rust-webx-pay-stripe
│       ├── alipay/            #   rust-webx-pay-alipay
│       ├── wechatpay/         #   rust-webx-pay-wechatpay
│       ├── paypal/            #   rust-webx-pay-paypal
│       ├── adyen/             #   rust-webx-pay-adyen
│       ├── square/            #   rust-webx-pay-square
│       └── ef/                #   rust-webx-pay-ef — 回调 inbox（rust-ef）
├── testing/                   # 不发布的测试工具（publish = false）
│                              #   alloc-probe / contract-tests / upgrade-smoke
├── fuzz/                      # 独立的 cargo-fuzz workspace（nightly CI）
├── docbit/                    # 参考应用：作品集 + 博客 + RBAC + 文档
├── dmbit/                     # 参考应用：设备与库存管理
└── imbit/                     # 参考应用：实时 IM（WS + 信令）
```

> **包名 vs 导入名**：依赖写 `rust-webx`，代码写 `use webx::*;`。子 crate 同理：
> `rust-webx-core` → `webx_core`、`rust-webx-host` → `webx_host`、
> `rust-webx-macros` → `webx_macros`、`rust-webx-spa` → `webx_spa`、
> `rust-webx-openapi` → `webx_openapi`、`rust-webx-ai` → `webx_ai`、
> `rust-webx-client` → `webx_client`、`rust-webx-ai-ef` → `webx_ai_ef`、
> `rust-webx-ai-gateway` → `webx_ai_gateway`、`rust-webx-auth-ef` → `webx_auth_ef`、
> `rust-webx-oidc` → `webx_oidc`、`rust-webx-oauth2` → `webx_oauth2`、
> `rust-webx-mfa` → `webx_mfa`、`rust-webx-webauthn` → `webx_webauthn`、
> `rust-webx-redis` → `webx_redis`、`rust-webx-nats` → `webx_nats`、
> `rust-webx-rabbitmq` → `webx_rabbitmq`、`rust-webx-kafka` → `webx_kafka`、
> `rust-webx-ef` → `webx_ef`、`rust-webx-outbox-ef` → `webx_outbox_ef`、
> `rust-webx-pay` → `webx_pay`、`rust-webx-pay-stripe` → `webx_pay_stripe`、
> `rust-webx-pay-alipay` → `webx_pay_alipay`、`rust-webx-pay-wechatpay` → `webx_pay_wechatpay`、
> `rust-webx-pay-paypal` → `webx_pay_paypal`、`rust-webx-pay-adyen` → `webx_pay_adyen`、
> `rust-webx-pay-square` → `webx_pay_square`、`rust-webx-pay-ef` → `webx_pay_ef`、
> `rust-webx-h3` → `webx_h3`。

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

### 组合根：两个槽

应用的装配只经过 builder 上的两个槽：

| 槽 | 放什么 | 示例 |
|---|---|---|
| `register` | DI 服务、端口与适配器（对象图的唯一来源） | `.register(|svc| svc.add_mediator())` |
| `configure` | 本 Host 的 `AppOptions` 运行期开关 | `.configure(\|app\| app.use_options(\|o\| o.metrics.enabled = false))` |

`use_spa`、`add_authentication`、`use_http_transport`、`use_alt_svc` 等是**同一 builder 状态的糖**，
不引入第三套装配通道。业务代码只消费接口，只有组合根知道具体实现。

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
| `IAuthorizationPolicy` | 旧式自定义策略接口；按路由模式检查角色/权限。 |
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
JSON: RFC 7807 application/problem+json (4xx / 5xx)
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
| [`rust-webx-h3`](https://crates.io/crates/rust-webx-h3) | `H3Transport` + `HostBuilder::use_http_transport(Arc::new(transport))`，可选 `use_h3_alt_svc(udp_port)`、可选 `H3Config::with_webtransport_handler` | QUIC + rustls + h3（`trev-h3` fork）、TLS 1.3 / ALPN `h3`、mTLS、SSE/流式响应、每连接 GOAWAY 排空、共享 `App.MaxConnections` 准入、`/metrics` 传输维度标签、显式 `Alt-Svc`、**HTTP/3 WebSocket（RFC 9220 extended CONNECT）**、**WebTransport adapter-level MVP**（session + bidi stream echo；transport-neutral 端口后续）。 |

## 示例应用

本仓库内置两个完整的参考应用，展示框架本身、分层项目结构（`contracts` / `handlers` / `domain`）、JWT 认证、RBAC 与 SPA 托管：

| 应用 | 说明 | 开发地址 |
|------|------|----------|
| [`docbit`](docbit/) | 作品集 + 博客 + RBAC + 完整文档站（`docs/`）。使用 `rust-ef`（开发与生产均为 SQLite）。 | <http://localhost:5000> |
| [`dmbit`](dmbit/) | 数据中心机房的设备与库存管理（设备/产品/规格/库存管理）。基于 SQLite。 | <http://localhost:5100> |

本地运行参考应用：

```bash
cargo run -p docbit-host     # → http://localhost:5000
cargo run -p dmbit-host      # → http://localhost:5100
```

生产部署细节参见 [`docbit/PRODUCTION.md`](docbit/PRODUCTION.md)。

## 快速开始

### 构建

```bash
# 针对某个应用做 release 构建
cargo build --release -p docbit-host
cargo build --release -p dmbit-host
```

### 运行参考应用

```bash
cargo run -p docbit-host     # 开发模式（SQLite），http://localhost:5000
```

### 发布（裸机）

```bash
# Linux / macOS
chmod +x docbit/publish.sh
./docbit/publish.sh /opt/docbit --production

# Windows
.\docbit\publish.ps1 -Destination D:\deploy\docbit -Production
```

启动前设置 `DATABASE_URL` 与 `JWT_SECRET`，或编辑生成的 `run.sh` / `run.cmd`。

### Docker（可选）

主部署方式为 **`docbit/publish.sh` 裸机 Linux 发布**（见 [docbit/PRODUCTION.md](docbit/PRODUCTION.md)）。Docker 文件仅作参考，不在 CI 中维护：

```bash
# 独立镜像（使用前请本地验证）
docker build -f docbit/Dockerfile .

# 本地栈（docbit + SQLite volume）
cp docbit/.env.example docbit/.env     # 编辑 JWT_SECRET
docker compose -f docbit/docker-compose.yml --env-file docbit/.env up --build
# → http://localhost:8100
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
- `JWT_SECRET` — JWT 签名密钥。所有模式都会读取；`APP_ENV=Production` 额外拒绝短于 32 字节或已知占位符的密钥。
- `APP__Jwt__Secret` — 覆盖 `JWT_SECRET`。
- `DATABASE_URL` — 可选数据库覆盖（docbit 默认使用可执行文件旁的 SQLite `app.db`）。
- `APP__*` — 覆盖 `appsettings.json` 任意键的内联 JSON（如 `APP__App__Urls`、`APP__Cors__Origins`）。

完整变量参考见 `docs/rust-webx/` 下的配置章节。

## 异常映射

| `Error` 变体 | HTTP 状态码 | 说明 |
|--------------|------------|------|
| `Error::Http(msg)` | 400 | HTTP 协议错误 |
| `Error::Status(code, msg)` | `code` | 显式指定状态码 |
| `Error::Unauthorized(msg)` | 401 | 认证失败 / 缺少令牌 |
| `Error::Forbidden(msg)` | 403 | 权限不足 |
| `Error::Di(msg)` | 500 | DI 容器错误 |
| `Error::Routing(msg)` | 404 | 路由错误 |
| `Error::Serialization(e)` | 400 | （反）序列化错误 |
| `Error::ResponseSerialization(e)` | 500 | 响应序列化失败（服务端 DTO 缺陷） |
| `Error::State(msg)` | 500 | 服务端状态 / 编程错误 |
| `Error::Internal(msg)` | 500 | 内部错误 |
| `Error::Message(msg)` | 500 | 通用错误消息 |
| `Error::Validation(msg)` | 400 | 校验失败 |
| `Error::NotFound(msg)` | 404 | 资源未找到 |
| `Error::Conflict(msg)` | 409 | 乐观并发 / 状态冲突 |
| `Error::PayloadTooLarge(msg)` | 413 | 请求体或上传文件超限 |
| `Error::UnsupportedMediaType(msg)` | 415 | 请求内容类型不受支持 |
| `Error::Problem(details)` | `details.status` | 字段级绑定/校验问题，携带 `errors` |

## 文档

- **在线手册** — 全书发布在 <https://rust-framework.github.io/rust-webx/>（mdBook，中文；由 `.github/workflows/docs.yml` 从 `docs/rust-webx/` 构建）。
- **rust-webx 开发者手册** — 位于 [`docs/rust-webx/`](docs/rust-webx/INDEX.md) 的渐进式披露书籍（16 章，中文）：入门、快速上手、架构、DI 与生命周期、中间件、中介者与事件、认证与安全、配置、生产、项目结构、扩展、最佳实践、案例研究与迁移指南。
- **升级** — 2.5.0 相对 2.1 为加性演进，见 [2.5 升级指南](docs/rust-webx/16-migration/upgrade-to-2.5.md)。
- **rust-ef 参考** — ORM 手册随 [`rust-ef`](https://crates.io/crates/rust-ef) crate 维护（见其 [docs.rs](https://docs.rs/rust-ef) 文档），`docbit` 示例使用。
- `docbit` 参考应用也把这些文档作为实时网站提供（`GET /api/docs/rust-webx/...`）。Monorepo 开发时 `DocService` 按 slug 实时解析 sibling 仓库文档，无需将 sibling 文档复制到 `rust-webx/docs/`；standalone 发布时 `docbit/publish.*` 在打包时从源仓库复制文档进 bundle。可通过 `RUST_FRAMEWORK_ROOT` 显式指定 monorepo 根目录。

## 许可证

[MIT](LICENSE)

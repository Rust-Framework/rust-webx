# 生态与 Crate 全景

## Workspace 总览

rust-webx 以 Cargo Workspace 组织，各 Crate 职责单一、依赖方向清晰：

```
rust-webx/                    # Workspace 根
├── crates/
│   ├── core/                   # rust-webx-core — 核心 trait（零实现依赖）
│   ├── host/                   # rust-webx-host — Host、管道、路由、认证实现
│   ├── macros/                 # rust-webx-macros — 过程宏
│   ├── spa/                    # rust-webx-spa — SPA 静态托管
│   ├── openapi/                # rust-webx-openapi — OpenAPI 生成
│   ├── ai/                     # rust-webx-ai — AI 网关领域包（可选，版本跟随 workspace）
│   ├── ai-ef/                  # rust-webx-ai-ef — rust-ef 持久化用量账本
│   ├── client/                 # rust-webx-client — 出站 HTTP（连接池/流式/SSE）
│   ├── gateway/                # rust-webx-ai-gateway — OpenAI 兼容 /v1 表面
│   └── webx/                   # rust-webx — 伞 Crate，统一导出
├── docbit/                     # 示例：作品集全栈应用
└── docs/rust-webx/             # 本书文档（仓库根目录）
```

### 包名与导入名

各 Crate 的包名、导入名与职责对照表见 [Crate 分层结构](../04-architecture/crate-layout.md#包名与导入名)。

## 依赖关系图

```mermaid
graph BT
    Webapp[rust-webx 伞 Crate]
    Core[rust-webx-core]
    Host[rust-webx-host]
    Macros[rust-webx-macros]
    SPA[rust-webx-spa]
    OpenAPI[rust-webx-openapi]

    Webapp --> Core
    Webapp --> Host
    Webapp --> Macros
    Webapp --> SPA
    Webapp --> OpenAPI
    Host --> Core
    Host --> OpenAPI
    Host --> SPA
```

**关键原则**：`core` 只定义 trait，不依赖任何实现 Crate。这保证了：

- 自定义中间件只需依赖 `core`
- 测试可 Mock 所有抽象接口
- 可替换 HTTP 实现而不影响业务代码

## 各 Crate 职责

### rust-webx-core

定义框架的**语言**——所有公开 trait 与类型：

| 模块 | 核心类型 |
|------|---------|
| `mediator` | `IRequest<T>`, `IEventRequest`, `IMediator` |
| `handler` | `IRequestHandler<T,R>`, `IEventHandler<T>`, `IHostedService` |
| `http` | `IHttpContext`, `IHttpRequest`, `IHttpResponse`, `IClaimsExt` |
| `middleware` | `IMiddleware` |
| `routing` | `IRouter`, `IEndpoint`, `RouteMeta`, `HttpMethod` |
| `auth` | `IClaims`, `IAuthenticationHandler`, `IAuthorizationPolicy`, `IDynamicAuthorizer` |
| `error` | `Error`, `Result`, `ActionResult` |
| `action` | `IActionResult`, `Response`, `JsonResult`, `ContentResult`, `FileResult`, `NoContentResult`, `StatusCodeResult` |
| `config` | `AppOptions`, `appsettings` 加载 |
| `cache` | `IDistributedCache` |
| `problem` | `ProblemDetails`, `FieldError` |
| `pagination` | `PagedRequest`, `PagedResult` |
| `operation` | `OperationResult`（含 `OperationResult<T>`） |

### rust-webx-host

框架的**运行时**：

- `Host` / `HostBuilder` — 应用宿主与构建器
- `Router` — 基于 Trie 的路由匹配
- `MiddlewarePipeline` — 中间件顺序执行
- `EngineContext` — 当前引擎的 HTTP 上下文具体实现（应用只依赖 `IHttpContext`）
- `JwtAuth` / `ResourceAuthorization` — 认证授权实现
- `MemoryCache`, `RateLimiter`, `CorsMiddleware` 等内置中间件

### rust-webx-macros

编译时魔法：

| 宏 | 作用 |
|----|------|
| `#[get("/path")]` 等 | 路由注册 + HTTP 方法 |
| `#[handler]` | Handler 编译时注册（提交 `HandlerRegistration` 到 `inventory`） |
| `#[authorize]` | 声明式授权元数据 |
| `#[derive(WebxRequestMeta)]` + `#[fromquery]` / `#[frompath]` / `#[frombody]` / `#[fromform]` / `#[fromfile]` / `#[fromheader]` / `#[fromclaim]` | 请求字段来源声明：编译期生成绑定代码与 OpenAPI 参数元数据（字段级属性，非独立宏） |

### rust-webx-spa

`SpaMiddleware`：托管静态文件（磁盘 + 编译内嵌资源），支持前端 History 路由。
`index.html` 回退在**路由未命中之后**执行，且只对显式请求 HTML 的 GET/HEAD 生效：
`/api/*`、请求数据（`Accept: application/json` / `*/*`）的客户端得到真正的 404，而不是 200 HTML。

### rust-webx-openapi

从编译时收集的路由元数据生成 OpenAPI 3.0 规范，并内置自定义 API 文档 UI（`/api/openapi.html`）。

### rust-webx-ai

AI 聚合网关的领域包：统一请求/流 IR、`ChatProvider` 端口、OpenAI 兼容适配器、模型目录与路由、
`CatalogHandle` 热替换、数据面执行器、精确响应缓存与单飞、成本计算、韧性原语（重试/熔断/舱壁）、用量计量与账本端口、tokenizer 兜底、
API key 解析/HMAC 与存储端口、配额预扣-结算、OpenAI 错误信封。它**不进入伞 crate**、不依赖 HTTP 实现，版本跟随 workspace
（`version.workspace = true`，与 `rust-webx` 同版本）。不需要 AI 能力的项目不引入。

### rust-webx-ai-ef

`rust-webx-ai` 的 `IUsageSink` 的 rust-ef 实现：`EfUsageSink` + `ai_usage_events` 实体，
按 `request_id` 幂等写入，每次写使用独立 `DbContext`。outbox/rollup worker 后续加入。

### rust-webx-ai-gateway
OpenAI 兼容的 HTTP 表面：`POST /v1/chat/completions` 的请求 DTO、IR 转换、JSON/SSE 响应与
OpenAI 错误信封。它只做传输适配，路由/provider/配额/用量仍在 `rust-webx-ai`。应用注册
`Arc<GatewayExecutor>`、鉴权 scheme 与 `OpenAiErrorWriter` 后即可暴露兼容端点。

### rust-webx-client

出站 HTTP 客户端：命名客户端、连接池、rustls TLS、流式响应体与上游 SSE 解码。服务端代码需要
调用第三方 API（AI provider、支付、消息队列等）时用它，而不是在业务里另起一套 HTTP 栈。
同样**不进入伞 crate**，版本跟随 workspace。

### rust-webx（伞 Crate）

应用开发者只需：

```toml
[dependencies]
rust-webx = "2.9"
```

```rust
use webx::*;  // 一次导入全部公开 API
```

## 外部依赖

| 依赖 | 用途 |
|------|------|
| `rust-dix` | DI 容器（ServiceCollection / ServiceProvider） |
| `hyper` + `tokio` | HTTP 服务器与异步运行时 |
| `inventory` | 编译时路由与 Handler 元数据收集 |
| `serde` / `serde_json` | JSON 序列化 |
| `jsonwebtoken` | JWT 编解码 |
| `async-trait` | async trait 支持 |

## 应用开发者视角

作为框架使用者，你**通常只需依赖 `rust-webx` 伞 Crate**。仅在以下情况直接接触子 Crate：

- 编写独立中间件库 → 依赖 `rust-webx-core`
- 贡献框架本身 → 修改对应子 Crate

## 示例项目 Docbit

`docbit` 是框架的**官方参考实现**，展示：

- 全栈单体（API + SPA + 文档系统）
- `IHostedService` 数据初始化
- JWT 认证与 `#[authorize]`
- `#[inject]` 依赖注入
- 多模块 handlers（auth、blog、docs、works 等）

运行：`cargo run -p docbit-host`

## 小结

rust-webx 的 Crate 分层体现了**接口与实现分离**的设计：core 定义契约，host 提供运行时，macros 提供编译时便利，伞 Crate 统一开发者体验。

下一章：[快速上手](../02-quickstart/INDEX.md)

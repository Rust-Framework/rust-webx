# Crate 分层结构

## 依赖方向规则

**单向依赖，不可逆向**：

```
应用代码 → rust-webapp (伞) → host / macros / spa / openapi → core
                                                              ↑
                                                         零实现依赖
```

`core` 是金字塔顶端（最抽象），`host` 是运行时实现层，应用代码在最上层。

## core — 契约层

`rust-webapp-core` 定义所有公开 trait，**不依赖** hyper、tokio 或任何 HTTP 实现。

设计意图：
- 第三方可基于 core trait 编写可插拔组件
- 单元测试可 Mock 全部接口
- 未来可替换 HTTP 引擎（如从 hyper 迁移）

核心模块：

| 模块 | 职责 |
|------|------|
| `app` | `IHost` |
| `mediator` | `IRequest`, `IMediator`, `IEventRequest` |
| `handler` | `IRequestHandler`, `IEventHandler`, `IHostedService` |
| `http` | `IHttpContext`, `IHttpRequest`, `IHttpResponse` |
| `middleware` | `IMiddleware` |
| `routing` | `IRouter`, `IEndpoint`, `RouteMeta` |
| `auth` | `IClaims`, `IAuthenticationHandler`, `IAuthorizationPolicy` |
| `route` | 扫描类型、`HandlerCache`、`RouteEntry` |
| `mediator` | `IMediator` trait + `Mediator` 具体实现 |

## host — 运行时层

`rust-webapp-host` 实现 core 定义的 HTTP 相关 trait：

| 组件 | 实现 |
|------|------|
| HTTP 服务 | hyper + tokio |
| 路由 | Trie 树 `Router` |
| 上下文 | `HttpContext`, `HttpRequest`, `HttpResponse` |
| 管道 | `MiddlewarePipeline` |
| 认证 | `JwtAuth`, `jwt_middleware` |
| 授权 | `ResourceAuthorization`, `resource_auth_middleware` |
| 端点 | `RequestEndpoint`, `ControllerEndpoint`, `StaticJsonEndpoint`, `StaticHtmlEndpoint` |
| 宿主 | `Host`, `HostBuilder`, `Server` |

## macros — 编译时层

`rust-webapp-macros` 在编译期生成：

- 路由注册代码（写入 `inventory`）
- Handler DI 注册代码
- 授权元数据收集

应用开发者通过 `#[get]`、`#[handler]` 等使用，无需直接接触宏展开代码。

## spa / openapi — 能力扩展层

| Crate | 职责 |
|-------|------|
| `rust-webapp-spa` | `SpaMiddleware` 静态文件 + History fallback |
| `rust-webapp-openapi` | OpenAPI 3.0 规范生成 + Swagger UI |

二者均依赖 `core`，被 `host` 在 `HostBuilder` 中集成。

## webapp — 伞 Crate

统一 re-export，应用只需：

```toml
[dependencies]
rust-webapp = "0.1"
```

```rust
use rust_webapp::*;
```

同时 re-export `rust_dicore`、`async_trait`、`serde` 等常用依赖，减少 `Cargo.toml` 条目。

## 小结

Crate 分层体现了**接口与实现分离**：改 host 实现不影响业务代码，扩展 core trait 不影响已有实现。

下一节：[请求生命周期](request-lifecycle.md)

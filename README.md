# rust-webx — Rust WebApi Framework

ASP.NET Core 风格的 Rust WebApi 服务框架，基于 [rust-dix](https://crates.io/crates/rust-dix) DI + 中介者模式构建。

## 特性

- **DI + 中介者 双核心** — 以 `IRequest<T>` + `IRequestHandler<T, R>` 为载体，框架自动完成路由映射和 DI 解析
- **编译时路由快捷键** — `#[get("/path")]` `#[post("/path")]` 等一行标注定义完整端点
- **编译时自动扫描** — 通过 `inventory` 编译时收集路由元数据，`Host::build()` 时自动注册
- **零配置 Handler 注册** — `#[handler]` 属性宏自动向 DI 容器注册 Handler
- **身份认证和授权** — JWT Bearer + 基于资源的路由授权
- **rust-ef 集成** — `add_dbcontext` + per-request owned `DbContext`（EFCore 风格 UoW）

## 快速开始

```rust
use rust_webx::*;

struct HelloRequest;

#[get("/hello")]
impl IRequest<String> for HelloRequest {}

#[derive(Default)]
struct HelloHandler;

#[handler]
#[async_trait]
impl IRequestHandler<HelloRequest, String> for HelloHandler {
    async fn handle(&self, _req: HelloRequest) -> Result<String> {
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

## 架构

```
┌──────────────────────────────────────────────────────────┐
│                   rust-webx (umbrella)                   │
│  重新导出 core / host / macros / spa / openapi + rust_dix│
├──────────┬──────────┬──────────┬─────────────────────────┤
│rust-webx-│rust-webx-│rust-webx-│ rust-webx-macros        │
│host      │core      │openapi   │ #[get] #[post] #[handler]│
│Host +    │traits +  │OpenAPI   │ #[authorize]             │
│Pipeline  │config    │生成      │                          │
│+ Router  │mediator  │          │                          │
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
   │ (DI)     │ │(HTTP)    │ │(路由收集) │
   └──────────┘ └──────────┘ └──────────┘
```

### Crate 结构

```
rust-webx/
├── Cargo.toml              # workspace root (v0.2.0)
├── docbit/                 # 参考应用（作品集 + 博客 + RBAC）
└── crates/
    ├── core/               # rust-webx-core — trait 与配置
    ├── host/               # rust-webx-host — Host、管道、路由
    ├── macros/             # rust-webx-macros — 过程宏
    ├── spa/                # rust-webx-spa — 静态资源 / SPA
    ├── openapi/            # rust-webx-openapi — OpenAPI 生成
    └── webx/               # rust-webx — 伞 crate
```

## 核心概念

| 概念 | 说明 |
|------|------|
| `IRequest<TResponse>` | 泛型请求标记，承载响应类型。`IRequest<()>` 返回 204 No Content |
| `IRequestHandler<T, R>` | 双类型参数处理器，`T` 为请求类型，`R` 为响应类型 |
| `#[get("/path")]` | 路由快捷键，标注在 `impl IRequest<T>` 块上，编译时注册 |
| `#[post("/path")]` | POST 路由快捷键 |
| `#[put("/path")]` | PUT 路由快捷键 |
| `#[delete("/path")]` | DELETE 路由快捷键 |
| `IMediator` | 中介者，`send()` 分发请求，`publish()` 发布事件 |
| `IMiddleware` | 中间件，顺序管道，可短路请求 |
| `IPipelineBehavior` | Mediator 管道拦截器，可包装请求处理链 |
| `IEventHandler<T>` | 事件处理器，通过 `publish()` 广播到所有注册的 handler |
| `Error` | 统一错误类型，自动映射 HTTP 状态码 |
| `IClaims` / `IAuthenticationHandler` | JWT 认证接口，从 Bearer Token 提取用户身份 |
| `IAuthorizationPolicy` | 授权策略接口，基于路由模式检查角色/权限 |

## 请求处理流程

```
HTTP Request
    │
    ▼
HttpContext::new(req).await    ←  读取 body bytes
    │
    ▼
MiddlewarePipeline::execute()
    │  中间件按注册顺序调用
    │  每个可设置 response status 短路
    ▼
Router::match_route(ctx)
    │  Trie 树匹配 method + path
    │  提取 {param} 值到 route_params
    ▼
┌──────────────┐
│ Route matched │──No──▶ 404 "Not Found"
└──────┬───────┘
       │ Yes
       ▼
IEndpoint::handle(ctx)
    │  调用 handler，序列化响应
    ▼
HttpResponse → hyper::Response
    │  若 Err → 内置异常中间件映射状态码
    ▼
JSON 响应: RFC 7807 application/problem+json（404/5xx）
```

## 生产就绪能力

| 能力 | 状态 |
|------|------|
| 优雅关闭（Ctrl+C / SIGTERM） | ✅ |
| 连接 drain（Production 30s） | ✅ |
| 健康检查 `/health` `/health/ready`（运行时探针，fail→503） | ✅ |
| 安全响应头 + Request ID | ✅ 默认启用 |
| JWT Production fail-fast | ✅ |
| CORS `*` Production fail-fast | ✅ |
| OpenAPI UI | Development 模式 only |
| 速率限制 / 压缩 | 应用层 opt-in（`use_middleware`） |
| TLS | ✅ 配置 `App.Urls` + `Tls.CertPath/KeyPath` |

环境变量：`JWT_SECRET`、`APP__Jwt__Secret`、`APP__*` 覆盖 appsettings。详见 `docs/rust-webx/`。

## 异常映射

| Error 变体 | HTTP 状态码 | 说明 |
|-----------|-----------|------|
| `Error::NotFound(msg)` | 404 | 资源未找到 |
| `Error::Validation(msg)` | 400 | 参数校验失败 |
| `Error::Serialization(e)` | 400 | 序列化/反序列化错误 |
| `Error::Http(msg)` | 400 | HTTP 协议错误（含 401 未认证、403 禁止访问） |
| `Error::Di(msg)` | 500 | DI 容器错误 |
| `Error::Internal(msg)` | 500 | 内部错误 |
| `Error::Message(msg)` | 500 | 通用错误消息 |
| `Error::Routing(msg)` | 404 | 路由错误 |

## 示例应用

参考应用 **docbit**（`docbit/host`）演示 rust-ef 集成、JWT、RBAC、SPA 托管：

```bash
cargo run -p docbit-host
# http://localhost:5000
```

生产部署见 `docbit/PRODUCTION.md`。

## 依赖

| Package | 版本 | 用途 |
|---------|------|------|
| `rust-dix` | 0.6 | DI 容器 |
| `rust-ef` | 1.5.1 | ORM（可选，docbit 使用） |
| `hyper` | 1 | HTTP 服务器 |
| `tokio` | 1 | 异步运行时 |
| `serde` / `serde_json` | 1 | 序列化 |
| `inventory` | 0.3 | 编译时路由收集 |
| `jsonwebtoken` | 9 | JWT 认证 |

## 许可证

MIT

# LRWF — Rust WebApi Framework

ASP.NET Core 风格的 Rust WebApi 服务框架，基于 [lrdi](https://crates.io/crates/lrdi) DI + 中介者模式构建。

## 特性

- **DI + 中介者 双核心** — 以 `IRequest<T>` + `IRequestHandler<T, R>` 为载体，框架自动完成路由映射和 DI 解析
- **编译时路由快捷键** — `#[get("/path")]` `#[post("/path")]` `#[put("/path")]` `#[delete("/path")]` 一行标注定义完整端点
- **编译时自动扫描** — 通过 `inventory` 编译时收集路由元数据，`Host::build()` 时自动读取并注册
- **零配置 Handler 注册** — `#[handler]` 属性宏自动向 DI 容器注册 Handler，无需任何手动代码
- **内置异常中间件** — `Error` 变体自动映射 HTTP 状态码（404/400/401/403/500），响应格式统一为 `{"error": "msg", "status": code}`
- **事件发布/订阅** — `IEventRequest` + `IEventHandler<T>` + `IMediator::publish()` 实现模块间松耦合通信
- **身份认证和授权** — JWT Bearer Token 认证 + 基于资源（原始路由字符串）的角色/权限授权
- **高内聚低耦合** — `lrwf-core` 只定义 trait，零实现依赖；上层 crate 只依赖 trait 抽象
- **AI 驱动开发友好** — 请求即端点，每个 `IRequest` 自带路由元数据，AI 生成的模块可直接插入系统

## 快速开始

```rust
use lrwf::*;

struct HelloRequest;

#[get("/hello")]
impl IRequest<String> for HelloRequest {}

#[derive(Default)]
struct HelloHandler;

#[handler]
#[async_trait]
impl IRequestHandler<HelloRequest, String> for HelloHandler {
    async fn handle(&self, _req: HelloRequest) -> Result<String> {
        Ok("Hello, World! Welcome to LRWF.".to_string())
    }
}

#[tokio::main]
async fn main() {
    Host::builder()
        .build()
        .run("0.0.0.0:5000")
        .await
        .expect("Server failed");
}
```

## 架构

```
┌──────────────────────────────────────────────────────────┐
│                   lrwf (umbrella)                        │
│  重新导出所有 crate + re-export lrdi/async-trait/serde   │
├──────────┬──────────┬──────────┬─────────────────────────┤
│lrwf-http │lrwf-     │lrwf-di   │ lrwf-macros             │
│Host +    │mediator  │IService- │ #[get] #[post] #[put]   │
│Pipeline  │IMediator │Collection│ #[delete] #[endpoint]    │
│+ Router  │send/     │Ext       │ #[handler]               │
│+ Context │publish   │          │ #[from_body]             │
│+ auth_jwt│          │          │                          │
│+ authz   │          │          │                          │
├──────────┴──────────┴──────────┴─────────────────────────┤
│                    lrwf-core                             │
│  IApplicationBuilder / IHost / IHttpContext / IHttpRequest│
│  IHttpResponse / IMiddleware / IRouter / IEndpoint       │
│  IMediator / IRequest / IEventRequest / IRequestHandler  │
│  IEventHandler / IPipelineBehavior / Error / HttpMethod  │
└──────────────────────┬───────────────────────────────────┘
                       │
          ┌────────────┼────────────┐
          ▼            ▼            ▼
   ┌──────────┐ ┌──────────┐ ┌──────────┐
   │   lrdi   │ │  hyper   │ │inventory │
   │(DI 容器)  │ │(HTTP底层) │ │(编译时   │
   └──────────┘ └──────────┘ │ 路由收集) │
                             └──────────┘
```

### Crate 结构

```
lrwf/
├── Cargo.toml              # workspace root
├── examples/
│   ├── hello_request.rs    # #[handler] 零配置样例
│   └── crud_controller.rs  # CRUD + 手动 DI + LoggingMiddleware
└── crates/
    ├── lrwf-core/          # 核心 trait 定义（零实现依赖）
    ├── lrwf-di/            # DI 扩展 + 编译时扫描类型
    ├── lrwf-http/          # Host 构建器 + 管道 + 路由器 + HTTP 上下文
    ├── lrwf-mediator/      # IMediator 实现 + 管道行为骨架
    ├── lrwf-macros/        # 过程宏（路由/控制器/参数绑定/处理器注册）
    └── lrwf/               # 伞 crate，统一导出所有类型
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
JSON 响应: {"error": "msg", "status": code}
```

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

## 示例

| 示例 | 路径 | 说明 |
|------|------|------|
| hello_request | `examples/hello_request.rs` | Hello World API，演示 `#[get]` + `#[handler]` 零配置自动注册 |
| crud_controller | `examples/crud_controller.rs` | 完整 CRUD API，演示多方法路由 + 手动 DI + LoggingMiddleware |
| auth_example | `examples/auth_example.rs` | JWT 认证 + 资源授权，演示 jwt_middleware + resource_auth_middleware |

### 运行示例

```bash
# Hello World
cargo run --example hello_request
# 访问: http://localhost:5000/hello

# CRUD API
cargo run --example crud_controller
# GET    /api/users
# GET    /api/users/{id}
# POST   /api/users  (JSON body)
# DELETE /api/users/{id}
```

## 依赖

| Package | 版本 | 用途 |
|---------|------|------|
| `lrdi` | 0.1 | DI 容器 |
| `hyper` | 1 | HTTP 服务器 |
| `tokio` | 1 | 异步运行时 |
| `serde` / `serde_json` | 1 | 序列化 |
| `async-trait` | 0.1 | async trait 支持 |
| `inventory` | 0.3 | 编译时路由收集 |
| `jsonwebtoken` | 9 | JWT 认证支持 |
| `thiserror` | 2 | 错误类型派生 |

## 许可证

MIT

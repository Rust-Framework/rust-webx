[English](README.md) | **简体中文**

<div align="center">

# rust-webx

**一个受 ASP.NET Core 启发的 Rust WebApi 框架，基于 DI + Mediator 构建。**

一个类型安全、约定优于配置的平台——在这里 *请求即端点*：一行属性声明路由，实现一个 Handler，路由、依赖注入、中间件与错误映射全部交给框架。

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![CI](https://github.com/Rust-Framework/rust-webx/actions/workflows/ci.yml/badge.svg)](https://github.com/Rust-Framework/rust-webx/actions/workflows/ci.yml)
![Rust](https://img.shields.io/badge/rust-%3E%3D%201.81-orange.svg)

</div>

---

## 目录

- [什么是 rust-webx](#什么是-rust-webx)
- [核心特性](#核心特性)
- [架构](#架构)
- [请求即端点：Hello World](#请求即端点hello-world)
- [核心概念](#核心概念)
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
| 错误与 HTTP 状态码映射分散 | 统一 `Error` 类型 + 内置异常中间件 |
| 认证授权各自为政 | `add_authentication()` + 声明式 `#[authorize]` |

rust-webx **不是** 全栈 UI 框架（可搭配任意前端）、**不是** ORM（自带或使用 [`rust-ef`](https://crates.io/crates/rust-ef)）、**也不是** 微服务治理平台。

## 核心特性

- **DI + 中介者 双核心** — 以 `IRequest<T>` + `IRequestHandler<T, R>` 为载体，框架自动完成路由映射与 DI 解析。
- **编译时路由快捷键** — `#[get("/path")]`、`#[post("/path")]`、`#[put("/path")]`、`#[delete("/path")]` 一行定义完整端点。
- **编译时自动扫描** — 通过 `inventory` 在编译时收集路由元数据，在 `Host::build()` 时自动注册。
- **零配置 Handler 注册** — `#[handler]` 属性宏自动向 DI 容器注册 Handler。
- **认证与授权** — JWT Bearer 认证 + 基于路由模式的资源授权（`#[authorize]`、`#[claims]`）。
- **开箱即用的生产能力** — 优雅关闭、动态健康检查、安全响应头、请求 ID、CORS、TLS、速率限制、压缩、OpenAPI + SPA 托管。
- **ORM 无关** — 框架不依赖 `rust-ef`；`docbit` 参考应用用极少量胶水代码接入。

## 架构

框架被拆分为一组职责聚焦的 crate，通过 `rust-webx` 伞 crate 统一重新导出：

```
┌──────────────────────────────────────────────────────────┐
│              rust-webx  (umbrella crate)                   │
│  re-exports core / host / macros / spa / openapi + rust_dix │
├──────────┬──────────┬──────────┬─────────────────────────┤
│rust-webx-│rust-webx-│rust-webx-│ rust-webx-macros          │
│host      │core      │openapi   │ #[get] #[post] #[handler] │
│Host +    │traits +  │OpenAPI   │ #[authorize] #[claims]    │
│pipeline  │config    │generation│                           │
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
├── Cargo.toml                 # workspace 根 (v0.3.0)
├── crates/
│   ├── core/                  # rust-webx-core  — trait 与配置
│   ├── host/                  # rust-webx-host  — Host 构建器、中间件管道
│   │                          #   Trie 路由器、hyper 集成
│   ├── macros/                # rust-webx-macros — 过程宏
│   ├── spa/                   # rust-webx-spa   — 静态文件 / SPA 中间件
│   ├── openapi/               # rust-webx-openapi — OpenAPI 规范生成 + UI
│   └── webx/                  # rust-webx       — 伞 crate（重新导出）
├── docbit/                    # 参考应用：作品集 + 博客 + RBAC + 文档
└── dmbit/                     # 参考应用：设备与库存管理
```

## 请求即端点：Hello World

声明一个请求、在它上面映射路由、并实现其 Handler——无需维护路由表：

```rust
use rust_webx::*;

struct HelloRequest;

#[get("/hello")]
impl IRequest<String> for HelloRequest {}

#[derive(Default)]
struct HelloHandler;

#[handler] // 编译时注册进 DI 容器
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

路由发现、Handler 解析、中间件编排与优雅关闭，全部由 `Host::build()` 自动完成。

## 核心概念

| 概念 | 说明 |
|------|------|
| `IRequest<TResponse>` | 携带响应类型的泛型请求标记。`IRequest<()>` 返回 `204 No Content`。 |
| `IRequestHandler<T, R>` | 双类型参数 Handler；`T` 为请求类型，`R` 为响应类型。 |
| `#[get("/path")]` | 路由快捷键，标注在 `impl IRequest<T>` 块上，编译时注册。 |
| `#[post("/path")]` / `#[put("/path")]` / `#[delete("/path")]` | HTTP 方法路由快捷键。 |
| `IMediator` | 中介者；`send()` 分发请求，`publish()` 发布事件。 |
| `IMiddleware` | 中间件；顺序管道，可短路请求。 |
| `IPipelineBehavior` | Mediator 管道拦截器，可包装请求处理链。 |
| `IEventHandler<T>` | 事件处理器；通过 `publish()` 广播到所有已注册的 handler。 |
| `Error` | 统一错误类型，自动映射到 HTTP 状态码。 |
| `IClaims` / `IAuthenticationHandler` | JWT 认证接口，从 Bearer Token 提取身份。 |
| `IAuthorizationPolicy` | 授权策略接口；基于路由模式检查角色/权限。 |

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

## 示例应用

本仓库内置两个完整的参考应用，展示框架本身、分层项目结构（`contracts` / `handlers` / `domain`）、JWT 认证、RBAC 与 SPA 托管：

| 应用 | 说明 | 开发地址 |
|------|------|----------|
| [`docbit`](docbit/) | 作品集 + 博客 + RBAC + 完整文档站（`docs/`）。使用 `rust-ef`（开发用 SQLite，生产用 MySQL）。 | <http://localhost:5000> |
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

# 完整本地栈（docbit + MySQL）
cp docbit/.env.example docbit/.env     # 编辑 JWT_SECRET 与 MYSQL_ROOT_PASSWORD
docker compose -f docbit/docker-compose.yml --env-file docbit/.env up --build
# → http://localhost:8100
```

## 生产就绪能力

| 能力 | 状态 |
|------|------|
| 优雅关闭（Ctrl+C / SIGTERM） | ✅ |
| 连接 drain（Production 30 秒） | ✅ |
| 健康检查 `/health` `/health/ready`（运行时探针，失败 → 503） | ✅ |
| 安全响应头 + 请求 ID | ✅ 默认启用 |
| JWT production fail-fast | ✅ |
| CORS `*` production fail-fast | ✅ |
| OpenAPI UI | 仅 Development 模式 |
| 速率限制 / 压缩 | 应用层 opt-in（`use_middleware`） |
| TLS | ✅ 通过 `App.Urls` + `Tls.CertPath/KeyPath` |

## 环境变量

- `APP_ENV` — 应用环境（`Development` / `Production`）。
- `RUST_WEBX_APP_BASE` — 应用基础目录。
- `JWT_SECRET` — JWT 签名密钥（≥32 字符；需 `APP_ENV=Production`）。
- `APP__Jwt__Secret` — 覆盖 `JWT_SECRET`。
- `DATABASE_URL` — 数据库连接串（docbit 生产，如 `mysql://user:pass@host:3306/docbit`）。
- `APP__*` — 覆盖 `appsettings.json` 任意键的内联 JSON（如 `APP__App__Urls`、`APP__Cors__Origins`）。

完整变量参考见 `docs/rust-webx/` 下的配置章节。

## 异常映射

| `Error` 变体 | HTTP 状态码 | 说明 |
|--------------|------------|------|
| `Error::NotFound(msg)` | 404 | 资源未找到 |
| `Error::Validation(msg)` | 400 | 校验失败 |
| `Error::Serialization(e)` | 400 | （反）序列化错误 |
| `Error::Http(msg)` | 400 | HTTP 协议错误（含 401 未认证、403 禁止访问） |
| `Error::Di(msg)` | 500 | DI 容器错误 |
| `Error::Internal(msg)` | 500 | 内部错误 |
| `Error::Message(msg)` | 500 | 通用错误消息 |
| `Error::Routing(msg)` | 404 | 路由错误 |

## 文档

- **rust-webx 开发者手册** — 位于 [`docs/rust-webx/`](docs/rust-webx/INDEX.md) 的渐进式披露书籍（16 章，中文）：入门、快速上手、架构、DI 与生命周期、中间件、中介者与事件、认证与安全、配置、生产、项目结构、扩展、最佳实践、案例研究与迁移指南。
- **rust-ef 参考** — 位于 [`docs/rust-ef/`](docs/rust-ef/INDEX.md) 的 ORM 手册，`docbit` 示例使用。
- `docbit` 参考应用也把这些文档作为实时网站提供（`GET /api/docs/rust-webx/...`）。Monorepo 开发时 `DocService` 按 slug 实时解析 sibling 仓库文档，无需将 sibling 文档复制到 `rust-webx/docs/`；standalone 发布时 `docbit/publish.*` 在打包时从源仓库复制文档进 bundle。可通过 `RUST_FRAMEWORK_ROOT` 显式指定 monorepo 根目录。

## 许可证

[MIT](LICENSE)
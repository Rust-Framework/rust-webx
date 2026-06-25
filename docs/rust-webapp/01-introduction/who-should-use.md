# 适用场景与边界

## 理想适用场景

### 1. 企业级 WebApi 后端

需要模块化、可测试、团队可协作的 API 服务：

- RESTful / JSON API
- 多模块业务（用户、订单、内容管理等）
- 需要认证授权、缓存、限流等横切能力

rust-webapp 的分层约定（contracts / handlers / domain / services）天然适合这类项目。

### 2. ASP.NET Core 团队迁移到 Rust

若你的团队熟悉以下概念，学习曲线将非常平缓：

| ASP.NET Core | rust-webapp |
|-------------|-------------|
| `IRequest<T>` (MediatR) | `IRequest<T>` + `IRequestHandler<T,R>` |
| `IHostedService` | `IHostedService` |
| `appsettings.json` | `appsettings.json` + `AppOptions` |
| Middleware Pipeline | `IMiddleware` 管道 |
| `[Authorize]` | `#[authorize]` |

### 3. AI 辅助 / 模块化开发

每个 API 端点由独立的 Request 结构体 + Handler 组成，**模块边界清晰**：

```
handlers/user/
  get_user.rs    → GetUserRequest + GetUserHandler
  create_user.rs → CreateUserRequest + CreateUserHandler
```

AI 生成的新模块可直接插入 `handlers/` 目录，框架通过编译时扫描自动发现，无需修改中央路由文件。

### 4. 全栈单体应用（API + SPA）

```rust
Host::builder()
    .use_spa("wwwroot")   // 托管 React/Vue/Svelte 构建产物
    .use_auth()
    .build()
```

一个进程同时服务 API 与前端静态资源，适合作品集、管理后台、中小型产品。

## 不太适合的场景

### 极简 API 或原型验证

若只需 2-3 个端点、无 DI 需求、追求最小依赖，Axum 单文件方案更轻量。

### 超高吞吐、极致延迟敏感

框架增加了 DI 解析与 Mediator 调度层。对于纳秒级延迟要求的场景，需评估开销（参见 [性能优化技巧](../14-best-practices/performance-tips.md)）。

### WebSocket 为主的应用

当前版本以 HTTP/JSON 为核心。WebSocket 需自行集成，非框架一等公民。

### 多进程微服务编排

框架解决**单服务内的架构问题**，不包含服务网格、分布式追踪等平台能力（可集成 OpenTelemetry 等）。

## 决策清单

在选用 rust-webapp 前，回答以下问题：

| 问题 | 倾向选用 rust-webapp |
|------|---------------------|
| 端点数量 > 10？ | ✅ |
| 需要 DI 注入数据库/缓存/外部服务？ | ✅ |
| 团队有 ASP.NET / MediatR 背景？ | ✅ |
| 需要 JWT + 角色/权限授权？ | ✅ |
| 需要 OpenAPI 文档？ | ✅ |
| 只是学习 Rust 的第一个 HTTP 程序？ | ⚠️ 可先 Axum，再迁移 |
| 端点 < 3 且无状态？ | ❌ 考虑更轻量方案 |

## 规模参考

| 项目规模 | 推荐结构 |
|---------|---------|
| 小型（< 20 端点） | 单 crate，`src/contracts` + `src/handlers` |
| 中型（20-100 端点） | 单 crate 多模块，或 workspace 拆分 domain |
| 大型（> 100 端点） | workspace 多 crate，共享 `contracts` crate |

Docbit 作品集站点属于**中型全栈单体**的典型范例（参见 [第十五章](../15-case-study/INDEX.md)）。

## 小结

rust-webapp 面向**需要工程化、模块化 WebApi 开发**的场景。它不是万能钥匙，但在其适用范围内，能显著降低架构决策成本，让团队把精力集中在业务逻辑上。

下一节：[生态与 Crate 全景](ecosystem-overview.md)

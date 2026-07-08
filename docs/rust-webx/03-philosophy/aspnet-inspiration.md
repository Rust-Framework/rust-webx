# ASP.NET Core 的启发

## 为什么是 ASP.NET Core

ASP.NET Core 在 .NET 生态中解决了与 rust-webx 相同的问题：**如何在大型团队中统一 Web 后端架构**。其成功模式包括：

- 内置 DI 容器
- 中间件管道
- 配置系统（`appsettings.json`）
- 后台服务（`IHostedService`）
- MediatR 社区实践的 Request/Handler 模式

rust-webx 不是简单移植，而是**提取设计精髓并用 Rust 类型系统重新实现**。

## 概念映射

| ASP.NET Core | rust-webx | 差异说明 |
|-------------|-------------|---------|
| `Startup.cs` / `Program.cs` | `Host::builder()` | 构建器模式，链式配置 |
| `IServiceCollection` | `ServiceCollection` (rust-dicore) | 同名概念 |
| `IRequest<T>` (MediatR) | `IRequest<T>` | 同时承载路由元数据 |
| `IRequestHandler<T,R>` | `IRequestHandler<T,R>` | 双泛型参数一致 |
| `[HttpGet("/path")]` | `#[get("/path")]` | 标注在 impl 块 |
| `[Authorize]` | `#[authorize]` | 编译时元数据 |
| `IHostedService` | `IHostedService` | start/stop 生命周期 |
| `appsettings.json` | `appsettings.json` | 相同文件名与节结构 |
| `UseMiddleware<T>()` | `svc.add_middleware::<T>()` | 注册方式类似 |
| `ProblemDetails` | `ProblemDetails` | RFC 7807 兼容 |

## 借鉴了什么

### 1. 宿主（Host）抽象

ASP.NET Core 的 Generic Host 统一管理配置、DI、生命周期。rust-webx 的 `Host` 同样：

```rust
Host::builder()
    .mode(AppMode::Development)
    .register(|svc| { ... })
    .configure(|app| app.useOptions(|o| { ... }))
    .use_spa("wwwroot")
    .add_authentication()
    .build()
    .run()
    .await?;
```

### 2. 中介者解耦

MediatR 让 Controller 变薄，业务逻辑进入 Handler。rust-webx 更进一步——**连 Controller 都没有**，Request 直接就是端点。

### 3. 配置分层

```
appsettings.json              # 基础配置
appsettings.Development.json  # 开发覆盖（自动合并）
```

与 ASP.NET Core 完全一致的使用体验。

## Rust 化改造

### 编译时 > 运行时

| ASP.NET Core（运行时反射） | rust-webx（编译时） |
|--------------------------|---------------------|
| 控制器发现 | `inventory` 路由收集 |
| Handler 注册 | `#[handler]` 宏 |
| 路由表构建 | `Host::build()` 读取编译期元数据 |

好处：启动更快、错误在编译期暴露、无反射开销。

### 类型安全

```rust
// 编译期保证：响应类型必须一致
#[get("/users/{id}")]
impl IRequest<UserDto> for GetUserRequest {}

impl IRequestHandler<GetUserRequest, UserDto> for GetUserHandler { ... }
//                                    ^^^^^^^ 必须与 IRequest 泛型一致
```

ASP.NET Core 中类型不匹配是运行时错误；rust-webx 中是编译错误。

### 所有权与 Arc

Rust 没有 GC，DI 容器使用 `Arc<T>` 共享所有权。ASP.NET Core 的 Scoped/Singleton 语义通过 `rust-dicore` 的生命周期管理实现。

## 未移植的部分

有意不移植的概念：

| ASP.NET Core | 原因 |
|-------------|------|
| Razor / MVC View | rust-webx 专注 WebApi |
| EF Core 内置 | ORM 由用户自选 |
| SignalR | WebSocket 非当前重点 |
| 过滤器（Filters） | 由 `IPipelineBehavior` + 中间件替代 |

## 迁移友好性

若你来自 ASP.NET Core， mental model 迁移路径：

1. Controller Action → `IRequest` + `IRequestHandler`
2. `Startup.ConfigureServices` → `Host::builder().register()`
3. `Startup.Configure` → 中间件注册 + `use_spa()` 等
4. `IHostedService` → 同名，几乎相同 API

详见 [第十六章迁移指南](../16-migration/from-aspnet-core.md)。

## 小结

rust-webx 是 ASP.NET Core 设计哲学的 Rust 表达：相同的架构舒适度，更强的编译期保证。

下一节：[Rust 惯用法与类型安全](rust-idioms.md)

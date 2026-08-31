# 概念对照表

## ASP.NET Core ↔ rust-webx

| ASP.NET Core | rust-webx | 说明 |
|-------------|-------------|------|
| `Program.cs` / `Startup.cs` | `Host::builder()` | 应用入口 |
| `IServiceCollection` | `ServiceCollection` | DI 注册 |
| `IServiceProvider` | `ServiceProvider` | DI 解析 |
| `IHost` | `IHost` / `Host` | 应用宿主 |
| `IHostedService` | `IHostedService` | 后台服务 |
| `IRequest<T>` (MediatR) | `IRequest<T>` | 请求标记 + 路由 |
| `IRequestHandler<T,R>` | `IRequestHandler<T,R>` | 处理器 |
| `IMediator` | `IMediator` | 中介者 |
| `IMediator.Send()` | `IMediator::send()` | 调度请求 |
| `INotification` | `IEventRequest` | 事件 |
| `INotificationHandler<T>` | `IEventHandler<T>` | 事件处理器 |
| `IPipelineBehavior` | `IPipelineBehavior` | 管道拦截 |
| `[HttpGet]` | `#[get]` | GET 路由 |
| `[HttpPost]` | `#[post]` | POST 路由 |
| `[Authorize]` | `#[authorize]` | 授权 |
| `[FromBody]` | `#[derive(Deserialize)]` | Body 绑定 |
| `[FromRoute]` | 路径同名字段 | 路径参数 |
| `appsettings.json` | `appsettings.json` | 配置文件 |
| `IConfiguration` | `AppOptions` | 配置访问 |
| `ProblemDetails` | `ProblemDetails` | RFC 7807 |
| `UseMiddleware<T>()` | `add_middleware::<T>()` | 中间件注册 |
| `UseAuthentication()` | `add_authentication()` | 认证 |
| `UseAuthorization()` | 自动（`#[authorize]` 宏编译期收集） | 授权 |
| `UseCors()` | `use_cors()` | CORS |
| `UseStaticFiles()` | `use_spa()` | 静态文件 |
| `Controller` | 无（Request 即端点） | 路由处理 |
| `ActionResult<T>` | `Result<T>` | 返回类型 |
| `NotFound()` | `Error::NotFound` | 404 |
| `BadRequest()` | `Error::Validation` | 400 |
| `IApplicationBuilder` | `HostBuilder`（含 `use_middleware`） | 应用配置 |

## Axum ↔ rust-webx

| Axum | rust-webx | 说明 |
|------|-------------|------|
| `Router::new().route()` | `#[get]` / `#[post]` | 路由定义 |
| `handler function` | `IRequestHandler` | 处理器 |
| `State<T>` | DI 注入 | 状态共享 |
| `Extension<T>` | DI 注入 | 请求扩展 |
| `middleware::from_fn` | `IMiddleware` | 中间件 |
| `Json<T>` | 自动序列化 | JSON 响应 |
| `StatusCode` | `Error` 变体 | 状态码 |
| `AppState` | `ServiceCollection` | 应用状态 |
| `axum::serve` | `Host::run()` | 启动服务 |
| `tower::ServiceBuilder` | `MiddlewarePipeline` | 中间件链 |

## 生态对照

| 需求 | ASP.NET Core | Axum | rust-webx |
|------|-------------|------|-------------|
| DI | 内置 | 手动/第三方 | rust-dix |
| ORM | EF Core | sqlx/sea-orm | 用户自选 |
| 认证 | Identity | 手动 | JWT 内置 |
| 文档 | Swagger | utoipa | OpenAPI 内置 |
| 配置 | appsettings | 手动/env | appsettings 内置 |
| 后台任务 | IHostedService | tokio::spawn | IHostedService |

## 小结

这张对照表是迁移时的速查手册。收藏本章，遇到概念疑问时随时查阅。

---

**恭喜读完 rust-webx 开发者手册！**

回到 [目录](../INDEX.md) 查阅具体章节，或运行 `cargo run -p docbit` 开始实践。

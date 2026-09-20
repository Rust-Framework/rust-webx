# ServiceCollection 与服务注册

## DI 容器

rust-webx 使用 `rust-dix` 作为 DI 容器：

```rust
Host::builder()
    .register(|svc| {
        svc.singleton::<MyService>(|_| Arc::new(MyService::new()))
           .singleton::<dyn IMyService>(|_| Arc::new(MyServiceImpl::new()))
    })
    .build()
```

`register()` 闭包接收 `ServiceCollection`，返回配置后的 collection。

## 生命周期

| 生命周期 | 方法 | 语义 |
|---------|------|------|
| Singleton | `svc.singleton::<T>()` | 全局唯一实例 |
| Scoped | `svc.scoped::<T>()` | 每请求一个实例 |
| Transient | `svc.transient::<T>()` | 每次解析新建 |

WebApi 场景主要使用 **Singleton**（数据库连接池、缓存、Repository），需要每请求隔离的
`DbContext` 之类用 **Scoped**。

## 注册 Handler

HTTP 分发不通过 DI 查找 `dyn IRequestHandler`：`#[handler]` 把处理器工厂提交到 inventory，
`Host::build()` 将其收集为 `HandlerCache`，按请求类型查找后在每请求作用域内构造。
因此 `#[handler]` / `#[handler(inject)]` 的处理器不做 DI 注册，也无需手动注册。

需要把处理器注册为 trait object 时（用于非 HTTP 的手动 DI 场景）：

```rust
svc.singleton::<dyn IRequestHandler<GetUserRequest, UserDto>>(
    |resolver| {
        let repo = resolver.get_required::<UserRepository>();
        Arc::new(GetUserHandler { repo })
    }
)
```

## 注册中间件

```rust
svc.add_middleware::<LoggingMiddleware>()
```

需要构造参数的中间件用 `HostBuilder::use_middleware_with(...)`：

```rust
Host::builder().use_middleware_with(|| {
    Arc::new(RateLimitMiddleware::new(10.0, 20)) as Arc<dyn IMiddleware>
})
```

或在 `register()` 里用 `svc.add(ServiceLifetime::Singleton, |_| ...)` 注册为 `dyn IMiddleware`。

## 注册后台服务

```rust
svc.add_hosted_service::<DbInitService>()
```

## 框架自动注册

`Host::build()` 在构建时自动：

- 把 `#[handler]` 提交的 `HandlerRegistration` 收集进 inventory `HandlerCache`（不做 DI 注册）
- `add_authentication()` 时构建 JWT 中间件并加入管道
- `add_memory_cache()` 时把 `Arc<MemoryCache>` 注册为具体实例

需要显式调用的：

- `IMediator` → `svc.add_mediator()`（`Host::build()` 不会自动注册）

## 小结

`register()` 是组合根——所有服务组装在此完成，业务代码只消费接口。

下一节：[依赖注入模式](injection-patterns.md)

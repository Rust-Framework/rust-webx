# ServiceCollection 与服务注册

## DI 容器

rust-webx 使用 `rust-dicore` 作为 DI 容器：

```rust
Host::builder()
    .register(|svc| {
        svc.singleton::<MyService>(|_| Arc::new(MyService::new()))
           .singleton::<dyn IRequestHandler<...>>(|_| Arc::new(...))
    })
    .build()
```

`register()` 闭包接收 `ServiceCollection`，返回配置后的 collection。

## 生命周期

| 生命周期 | 方法 | 语义 |
|---------|------|------|
| Singleton | `svc.singleton::<T>()` | 全局唯一实例 |
| Scoped | `svc.scoped::<T>()` | 每请求一个实例（预留） |
| Transient | `svc.transient::<T>()` | 每次解析新建 |

WebApi 场景主要使用 **Singleton**（数据库连接池、缓存、Repository）。

## 注册 Handler

```rust
svc.singleton::<dyn IRequestHandler<GetUserRequest, UserDto>>(
    |resolver| {
        let repo = resolver.get_required::<UserRepository>();
        Arc::new(GetUserHandler { repo })
    }
)
```

注册为 `dyn IRequestHandler<T, R>` trait object 是**强制要求**。

## 注册中间件

```rust
svc.add_middleware::<LoggingMiddleware>()
svc.add_middleware_instance(jwt_middleware(auth))
```

## 注册后台服务

```rust
svc.add_hosted_service::<DbInitService>()
```

## 框架自动注册

`Host::build()` 自动注册：

- `#[handler]` 收集的所有 Handler
- `IMediator` → `Mediator`
- `use_memory_cache()` 时的 `IDistributedCache`
- `add_authentication()` 时的 JWT 中间件

## 小结

`register()` 是组合根——所有服务组装在此完成，业务代码只消费接口。

下一节：[依赖注入模式](injection-patterns.md)

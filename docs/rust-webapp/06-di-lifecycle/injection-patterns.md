# 依赖注入模式

## inject_attr 模式（推荐）

```rust
#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<LoginRequest, AuthResponse>)]
pub struct LoginHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<LoginRequest, AuthResponse> for LoginHandler { ... }
```

`inject_attr` 自动：
1. 注册 `LoginHandler` 为 singleton
2. 解析 `ctx` 字段的依赖
3. 注册为 `dyn IRequestHandler<LoginRequest, AuthResponse>`

## 手动 Arc 注入

```rust
let ctx = Arc::new(Mutex::new(DbContext::new()));

Host::builder()
    .register(move |svc| {
        let ctx = Arc::clone(&ctx);
        svc.singleton::<Mutex<DbContext>>(move |_| Arc::clone(&ctx));
    })
```

适用于框架外的类型（如第三方 DbContext）。

## FromHttpContext

Handler 可通过 DI 获取 HTTP 上下文：

```rust
// 在需要访问 claims 的 Handler 中
let claims = ctx.claims().ok_or_else(|| Error::Http("Unauthorized".into()))?;
```

## 服务间依赖

```rust
#[inject_attr(singleton)]
pub struct DocService {
    root: String,
}

#[inject_attr(singleton, as = dyn IRequestHandler<GetDocIndexRequest, DocIndex>)]
pub struct GetDocIndexHandler {
    docs: Arc<DocService>,
}
```

依赖链：`GetDocIndexHandler` → `DocService`，由 DI 自动解析。

## 反模式

| ❌ 反模式 | ✅ 替代 |
|---------|--------|
| Handler 内 `UserRepository::new()` | DI 注入 Repository |
| 全局 `static mut` | `Arc<RwLock<T>>` + Singleton |
| Handler 间直接调用 | `IMediator::send()` |

## 小结

生产项目统一使用 `inject_attr` + `#[handler(inject)]`，保持 `main.rs` 只做 Host 配置。

下一节：[IHostedService 后台服务](hosted-services.md)

# 依赖注入模式

## rust-dix inject 模式（推荐）

```rust
#[derive(Inject)]
pub struct LoginHandler {
    auth: Arc<dyn IAuthService>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<LoginRequest, AuthResponse> for LoginHandler { ... }
```

`#[derive(Inject)]` + `#[handler(inject)]` 自动：

1. 注册 Handler 为 singleton
2. 解析字段依赖（如 `Arc<dyn IAuthService>`）
3. 注册为 `dyn IRequestHandler<LoginRequest, AuthResponse>`

## 业务服务：接口在 contracts，实现在 handlers

Handler 和 `IHostedService` **只依赖 trait**，不依赖具体实现类型。

```rust
// contracts/docs.rs — 接口与 DTO（仅依赖框架）
pub trait IDocumentService: Send + Sync {
    fn index(&self, work: &str) -> Result<DocIndex, String>;
    fn content(&self, work: &str, path: &str) -> Result<DocContent, String>;
}

// handlers/docs.rs — 实现 + 自动注册
#[inject]
pub struct DocService {
    paths: Arc<AppPaths>,
}

impl IDocumentService for DocService { ... }

// handlers/docs.rs — Handler 消费接口
#[inject]
pub struct GetDocIndexHandler {
    docs: Arc<dyn IDocumentService>,
}
```

要点：

| 层级 | 位置 | 依赖类型 | 注册方式 |
|------|------|---------|---------|
| 接口 trait | `contracts/` | — | 无注册 |
| Service 实现 | `handlers/` | `Arc<AppPaths>` 等 | `#[inject]` |
| Handler | `handlers/` | `Arc<dyn IDocumentService>` | `#[derive(Inject)]` + `#[handler(inject)]` |

新增业务能力时：**先在 contracts 写 trait + DTO → 在 handlers 写实现并加 `#[inject]` → Handler 注入 `Arc<dyn I…Service>`**，无需改 `main.rs`。

## 组合根：只注册框架外类型

`main.rs` 仅配置 Host；`bootstrap::configure` 只注册 rust-dix 无法自行构造的类型：

```rust
Host::builder()
    .register(common::bootstrap::configure)  // AppPaths + DbContext
    .use_spa(...)
    .add_authentication()
    .build();
```

```rust
pub fn configure(mut svc: ServiceCollection) -> ServiceCollection {
    let paths = AppPaths::resolve();
    svc = svc.singleton::<AppPaths>(move |_| Arc::new(paths));
    svc = svc.singleton::<Mutex<DbContext>>(move |_| { ... });
    svc
}
```

业务 Service（`DocService`、`BlogService`）**不在此手动注册**——由 `#[inject]` 在 handlers 中自动收集。

## DbContext（rust-ef）

```rust
let mut builder = DbContextOptionsBuilder::new();
builder
    .use_sqlite(db_path)
    .add_interceptor(AuditInterceptor);
```

`DbContext` 与 `DbContextOptions` 属于 ORM 基础设施，在 `bootstrap::configure` 注册；实体与迁移在 `domain/`，拦截器在 `common/`。

## 服务间依赖链

```
GetDocIndexHandler (handlers/)
    → Arc<dyn IDocumentService>  (DocService in handlers/)
        → Arc<AppPaths>          (bootstrap)
        → domain entities
```

依赖在编译期由字段类型声明，运行时由 DI 容器解析。

## 反模式

| ❌ 反模式 | ✅ 替代 |
|---------|--------|
| `I…Service` trait 放在 `services/` 或 `handlers/` 私有模块 | trait 定义在 `contracts/` |
| Handler 依赖 `Arc<DocService>` 具体类型 | `Arc<dyn IDocumentService>` |
| 在 `main.rs` 逐个 `singleton::<BlogService>` | handlers 中 `#[inject]` 实现 `IBlogService` |
| `contracts` 引用 `domain` 类型作为 Response | DTO 定义在 contracts，handlers 中映射 |
| Handler 内 `UserRepository::new()` | DI 注入 `Arc<dyn I…Service>` |
| Handler 间直接调用 | `IMediator::send()` |

## 小结

生产项目统一使用 **`contracts` 定义接口 + `handlers` 实现 + `#[inject]` + `#[handler(inject)]`**，`main.rs` 只做 Host 与基础设施配置。

下一节：[IHostedService 后台服务](hosted-services.md)

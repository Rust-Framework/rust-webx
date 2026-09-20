# 依赖注入模式

## `#[inject]` 模式（推荐）

```rust
#[derive(Inject)]
pub struct GetDocContentHandler {
    #[inject]
    docs: Arc<dyn IDocumentService>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<GetDocContentRequest, DocContent> for GetDocContentHandler { ... }
```

两者分工：

1. `#[derive(Inject)]` 只生成构造函数（`__rdi_construct_GetDocContentHandler`）
2. `#[handler(inject)]` 让 inventory 工厂在**每个请求**调用该构造函数，在请求作用域内解析
   字段依赖（如 `Arc<dyn IDocumentService>`），并提交 `HandlerRegistration`

因此它是**编译期、按请求**的处理器工厂，不是 DI 里的 singleton；也不会向 DI 注册
`dyn IRequestHandler<...>`——HTTP 分发通过 inventory `HandlerCache` 查找，不查 DI。

字段只有标注 `#[inject]` 时才从容器解析（`Arc<T>` / `Option<Arc<T>>` / `Vec<Arc<T>>`，
或用 `#[inject(owned)]` 解析 bare `T`）；未标注的字段一律取 `Default::default()`。

## 业务服务：接口在 contracts，实现在 handlers

Handler 和 `IHostedService` **只依赖 trait**，不依赖具体实现类型。

```rust
// contracts/docs.rs — 接口与 DTO（仅依赖框架）
pub trait IDocumentService: Send + Sync {
    fn list_works(&self) -> Result<Vec<String>, String>;
    fn content(&self, work: &str, path: &str) -> Result<DocContent, String>;
}

// handlers/doc_service.rs — 实现，并在 impl 块上注册
#[derive(Inject)]
pub struct DocService;

#[inject]
impl IDocumentService for DocService { ... }

// handlers/docs.rs — Handler 消费接口
#[derive(Inject)]
pub struct GetDocContentHandler {
    #[inject]
    docs: Arc<dyn IDocumentService>,
}
```

要点：

| 层级 | 位置 | 依赖类型 | 注册方式 |
|------|------|---------|---------|
| 接口 trait | `contracts/` | — | 无注册 |
| Service 实现 | `handlers/` | 无，或 `Arc<…>` | `#[derive(Inject)]` + impl 块上的 `#[inject]` |
| Handler | `handlers/` | `Arc<dyn IDocumentService>` | `#[derive(Inject)]` + `#[handler(inject)]` |

新增业务能力时：**先在 contracts 写 trait + DTO → 在 handlers 写实现并在 impl 块上加
`#[inject]` → Handler 注入 `Arc<dyn I…Service>`**，无需改 `main.rs`。

## 组合根：只注册基础设施

`main.rs` 用 `HostBuilder` 注册 rust-dix 无法自行构造的基础设施。扩展方法写在
`host/src/startup/extensions/`：

```rust
Host::builder()
    .register(|svc| svc.add_docbit_db())   // SQLite DbContext
    .use_spa("wwwroot")
    .add_authentication()
    .build();
```

```rust
// host/src/startup/extensions/service_collection.rs
impl ServiceCollectionExt for ServiceCollection {
    fn add_docbit_db(self) -> Self {
        let path = app_base().join("app.db");
        let mut db = DbContextOptionsBuilder::new();
        db.use_sqlite(&path.to_string_lossy());
        let options = Arc::new(db.build());
        self.scoped(move |_| Arc::new(DbContext::from_options(&options).unwrap()))
    }
}
```

业务 Service（如 `DocService`）**不在此手动注册**——由 impl 块上的 `#[inject]` 自动收集。

## DbContext（rust-ef）

`DbContext` 是 **Scoped** 依赖：每个请求，或每个 `IHostedService::start`，都解析一份自己的实例。

```rust
// Handler：按请求解析
#[derive(Inject)]
struct SaveHandler {
    #[inject(owned)]
    ctx: DbContext,
}

// IHostedService：在框架建立的请求作用域内解析
let mut ctx: DbContext = dispatch_provider()
    .get_owned()
    .map_err(|e| Error::Internal(format!("DbContext resolution failed: {}", e)))?;
```

实体与领域逻辑在 `domain/`；建表由 `ctx.ensure_created()` 在启动时完成，框架不提供版本化迁移。

## 服务间依赖链

```
GetDocContentHandler (handlers/)
    → Arc<dyn IDocumentService>   (DocService in handlers/)
        → <app_base>/docs/{work}  (webx::app_base())
        → domain entities
```

依赖在编译期由字段类型声明，运行时由 DI 容器解析。

## 反模式

| ❌ 反模式 | ✅ 替代 |
|---------|--------|
| `I…Service` trait 放在 `services/` 或 `handlers/` 私有模块 | trait 定义在 `contracts/` |
| Handler 依赖 `Arc<DocService>` 具体类型 | `Arc<dyn IDocumentService>` |
| 在 `main.rs` 逐个 `singleton::<DocService>` | 在 impl 块上加 `#[inject]` 自动注册 |
| `contracts` 引用 `domain` 类型作为 Response | DTO 定义在 contracts，handlers 中映射 |
| Handler 内 `UserRepository::new()` | DI 注入 `Arc<dyn I…Service>` |
| Handler 间直接调用 | `IMediator::send()` |

## 小结

生产项目统一使用 **`contracts` 定义接口 + `handlers` 实现 + `#[inject]` + `#[handler(inject)]`**，
`main.rs` 只做 Host 与基础设施配置。

下一节：[IHostedService 后台服务](hosted-services.md)

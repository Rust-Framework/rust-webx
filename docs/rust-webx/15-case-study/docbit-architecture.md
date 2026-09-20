# 架构与模块划分

## 源码结构（workspace）

```
docbit/
├── contracts/src/           # Request/Response/enum/IDocumentService trait
│   ├── docs.rs              # IDocumentService + 文档路由
│   └── blog.rs              # 博客 DTO/路由（无 service 抽象）
├── domain/src/              # 实体、EF 配置、seed
├── handlers/src/            # Handler + Service 实现
│   ├── doc_service.rs       # DocService（impl IDocumentService）
│   └── blog.rs              # 博客 Handler（直接使用 DbContext）
└── host/
    ├── Cargo.toml           # package = "docbit-host"
    ├── build.rs             # webx::builder().web_root("../wwwroot").build()
    └── src/
        ├── main.rs          # 组合根：#[webx::main(embed)]
        ├── lib.rs           # pub mod startup;
        └── startup/
            ├── mod.rs
            ├── extensions/  # Add* / Use* 扩展方法
            ├── hosted/      # DbInitService（IHostedService）
            └── seed/        # 一次性数据初始化（admin 账户等）
```

## main.rs

```rust
#[webx::main(embed)]
async fn main() {
    Host::builder()
        .register(|svc| svc.add_docbit_db())
        .register(|svc| svc.add_mediator())
        .add_options::<SiteConfig>("Site")
        .add_authentication()
        .use_resource_authorization()
        .add_memory_cache()
        .use_spa("wwwroot")
        .build()
        .run()
        .await
        .expect("Server failed");
}
```

`main.rs` 不做业务注册。Handler、`IHostedService`、业务 Service、`IDynamicAuthorizer` 均由 `ServiceCollection::from_injected()` 自动收集。

## startup/extensions/

`Add*` / `Use*` 扩展方法住在这里，只注册框架无法自动构造的基础设施：

- `add_docbit_db()` — 用 `webx::app_base()` 解析 `<app_base>/app.db`，把 `DbContext` 注册为 **Scoped**（每个请求一份 owned 实例）
- `use_production_middleware()` — 生产环境的压缩、计时、请求追踪

业务 Service 在 `handlers/` 通过 `#[derive(Inject)]` + 在 `impl Trait for Type` 上标注 `#[inject]` 自注册。

## startup/hosted/ 与 startup/seed/

`DbInitService` 注入 `Arc<dyn IDocumentService>`，在 `start()` 中：

1. `ctx.ensure_created()` 确保 schema（没有版本化迁移）
2. seed 默认 admin 账户
3. 生成缺失的文档 `INDEX.json`、同步作品 logo 到 `wwwroot`

## 请求数据流

```
GET /api/docs/{work}/index
    → contracts/docs.rs (GetDocIndexRequest)
    → handlers/docs.rs (GetDocIndexHandler)
        → Arc<dyn IDocumentService>::index()
    → DocIndex JSON (contracts DTO)
```

Handler 不感知 `DocService` 具体类型，测试时可替换 mock 实现。

## 小结

Docbit 演示 rust-webx 推荐的完整分层：**契约（含接口）→ Handler 履约 → 领域/基础设施**。面向接口，组合根最小化。

下一节：[可复用的模式提炼](docbit-patterns.md)

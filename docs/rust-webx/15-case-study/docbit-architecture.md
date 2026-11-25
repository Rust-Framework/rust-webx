# 架构与模块划分

## 源码结构（workspace）

```
examples/docbit/
├── contracts/src/           # Request/Response/enum/IDocumentService trait
│   ├── docs/                #   <context>/<operation>.rs：文档浏览与上传
│   │   ├── models.rs        #     DTO / 视图模型
│   │   ├── service.rs       #     IDocumentService 端口
│   │   └── get_doc_index.rs / get_doc_content.rs / list_doc_works.rs / upload_work_docs.rs
│   ├── blog/                #   博客：每个操作一个文件 + models.rs
│   └── auth|category|comment|exhibition|media|rbac|site|tracking|user/
├── domain/src/              # 实体、XxxConfiguration、mapper、seed
│   ├── entities/            #   blog.rs / user.rs / …（实体与 Configuration 同文件）
│   ├── mappers/             #   <context>_mapper.rs（DTO ↔ 实体）
│   ├── configure.rs         #   模型/种子注册
│   └── audit.rs / filters.rs / ids.rs / seed.rs
├── handlers/src/            # 每个操作一个 Handler，目录与 contracts 对应
│   ├── doc_service.rs       #   DocService（impl IDocumentService）
│   ├── blog/create_blog_post.rs / list_blog_posts.rs / …
│   ├── auth|category|comment|exhibition|media|rbac|site|tracking|user/
│   └── authorization.rs / cache.rs / catalog.rs / seo.rs / common.rs   # 共享模块
└── host/
    ├── Cargo.toml           # package = "docbit-host"
    ├── build.rs             # webx::builder().web_root("../wwwroot").build()
    └── src/
        ├── main.rs          # 组合根：#[webx::main(embed)] + try_build()
        ├── lib.rs           # pub mod startup;
        └── startup/
            ├── mod.rs
            ├── services/    # Add* / Use* 扩展方法、JWT 密钥文件
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
        .use_authentication()
        .add_memory_cache(CacheOptions::default())
        .use_spa("wwwroot")
        .build()
        .run()
        .await
        .expect("Server failed");
}
```

`main.rs` 不做业务注册。Handler、`IHostedService`、业务 Service、`IDynamicAuthorizer` 均由 `ServiceCollection::from_injected()` 自动收集。

## startup/services/

`Add*` / `Use*` 扩展方法住在这里，只注册框架无法自动构造的基础设施：

- `sqlite_db_options()` — 用 `webx::app_base()` 解析 `<app_base>/app.db`，产出共享的 `Arc<DbContextOptions>`
- `add_docbit_db(options)` — 把 `DbContext` 注册为 **Scoped**（每个请求一份 owned 实例，共享同一份 options）
- `use_production_middleware()` — 生产环境的压缩、计时、请求追踪（`MiddlewareSpec`）
- `ensure_jwt_secret_file(mode)` — 在框架解析 `Jwt.SecretFile` 之前把密钥文件准备好

业务 Service 在 `handlers/` 通过 `#[derive(Inject)]` + 在 `impl Trait for Type` 上标注 `#[inject]` 自注册。

## startup/hosted/ 与 startup/seed/

`DbInitService` 注入 `Arc<dyn IDocumentService>`，在 `start()` 中：

1. `ctx.bootstrap()` 做空库首装（无迁移历史；已有库走 `ctx.migrate("Migrations")`，`ensure_created` 仅测试用）
2. seed 默认 admin 账户
3. 生成缺失的文档 `INDEX.json`、同步作品 logo 到 `wwwroot`

## 请求数据流

```
GET /api/docs/{work}/index
    → contracts/docs/get_doc_index.rs (GetDocIndexRequest)
    → handlers/docs/get_doc_index.rs (GetDocIndexHandler)
        → Arc<dyn IDocumentService>::index()
    → DocIndex JSON (contracts/docs/models.rs)
```

Handler 不感知 `DocService` 具体类型，测试时可替换 mock 实现。

## 小结

Docbit 演示 rust-webx 推荐的完整分层：**契约（含接口）→ Handler 履约 → 领域/基础设施**。面向接口，组合根最小化。

下一节：[可复用的模式提炼](docbit-patterns.md)

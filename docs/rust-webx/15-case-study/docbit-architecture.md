# 架构与模块划分

## 源码结构

```
docbit/src/
├── main.rs              # 组合根：仅 Host 配置
├── startup.rs           # DbInitService（IHostedService）
├── common/
│   ├── bootstrap.rs     # AppPaths + DbContext（唯一手动 DI）
│   ├── paths.rs         # 数据目录解析
│   └── mod.rs           # 拦截器、授权器（#[inject] 自动注册）
├── contracts/           # Request/Response/enum/I…Service trait
├── handlers/            # Handler + Service 实现（#[inject] + #[handler(inject)]）
│   ├── doc_service.rs   # DocService（impl IDocumentService）
│   └── blog_service.rs  # BlogService（impl IBlogService）
└── domain/              # 实体 + EF 迁移
```

## main.rs

```rust
let host = Host::builder()
    .mode(AppMode::Development)
    .register(common::bootstrap::configure)
    .use_spa(wwwroot)
    .add_authentication()
    .add_memory_cache()
    .build();

host.run().await?;
```

`main.rs` 不做业务注册。Handler、`IHostedService`、业务 Service、`IDynamicAuthorizer` 均由 `ServiceCollection::from_injected()` 自动收集。

## bootstrap.rs

仅注册框架无法自动构造的基础设施：

- `AppPaths` — docs / blog-data / wwwroot / 数据库路径
- `Mutex<DbContext>` — rust-ef SQLite 上下文

业务 Service 在 `handlers/` 通过 `#[inject] (implements I…Service)` 自注册。

## startup.rs

`DbInitService` 注入 `Arc<dyn IDocumentService>`，在 `start()` 中：

1. 运行 EF 迁移（m001–m004）
2. 生成缺失的文档 `INDEX.json`
3. 同步作品集 logo 到 `wwwroot`

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

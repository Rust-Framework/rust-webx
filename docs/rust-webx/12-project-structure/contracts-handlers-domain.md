# Contracts / Handlers / Domain 分层

基于 rust-webx 的业务应用采用**契约驱动、面向接口**的 workspace 结构。关注契约，不关注实现。

## contracts — API 与业务接口契约

**仅依赖框架**（`rust-webx`），**禁止依赖 domain 或 handlers**。

```rust
// contracts/docs.rs — 定义「对外承诺什么」

use serde::Deserialize;
use webx::*;

/// 文档读取服务接口 — 与 Request 同级，同属契约层
pub trait IDocumentService: Send + Sync {
    fn list_works(&self) -> std::result::Result<Vec<String>, String>;
    fn index(&self, work: &str) -> std::result::Result<DocIndex, String>;
    fn content(&self, work: &str, path: &str) -> std::result::Result<DocContent, String>;
}

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct GetDocIndexRequest {
    #[from_route]
    pub work: String,
}

#[get("/api/docs/{work}/index")]
impl IRequest<DocIndex> for GetDocIndexRequest {}
```

特点：

- Request、Response DTO、共享 enum、`I…Service` trait 均在此层
- 路由宏、`#[authorize]` 等元数据在此声明
- 是 OpenAPI 生成的数据源；团队讨论 API 时**只看 contracts**
- **不含** `async fn` 业务实现、数据库访问

> `IDocumentService` 是 docbit 抽取的唯一服务接口；博客等模块由 mediator 直接操作 `DbContext`，见 [可复用的模式提炼](../15-case-study/docbit-patterns.md)。

## handlers — Handler 与 Service 实现

**履约层**：实现 `IRequestHandler` 与 `I…Service`。

```rust
// handlers/doc_service.rs — 定义「如何履约」

use docbit_contracts::docs::{DocIndex, GetDocIndexRequest, IDocumentService};
use std::sync::Arc;
use webx::*;

#[derive(Inject)]
pub struct DocService;

// `#[inject]` 标注在 trait impl 上：注册为 `dyn IDocumentService` 单例
#[inject]
impl IDocumentService for DocService {
    fn index(&self, work: &str) -> std::result::Result<DocIndex, String> {
        // 扫盘 → 组装 DocIndex（contracts DTO）
    }
    // ...
}

#[derive(Inject)]
pub struct GetDocIndexHandler {
    #[inject]
    docs: Arc<dyn IDocumentService>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<GetDocIndexRequest, DocIndex> for GetDocIndexHandler {
    async fn handle(&mut self, req: GetDocIndexRequest) -> Result<DocIndex> {
        self.docs.index(&req.work).map_err(Error::NotFound)
    }
}
```

特点：

- Handler 薄编排；复杂逻辑在 Service 实现中
- Handler **只注入** `Arc<dyn I…Service>`
- `#[derive(Inject)]` + `#[handler(inject)]` 自动注册，无需改 `main.rs`
- 需要数据库时声明 owned 字段 `#[inject(owned)] ctx: DbContext`（EFCore 风格 per-request unit-of-work，无需 `Arc<Mutex>`）
- 返回 `Result<T>`，不直接操作 HTTP

## domain — 持久化实体与配置

```rust
// domain/user.rs — 定义「数据是什么」

use crate::contracts::auth::UserRole;  // 可复用 contracts 枚举

pub struct UserEntity {
    pub id: String,
    pub role: UserRole,
    // ...
}
```

特点：

- 数据库实体、EF 配置、seed
- **可以**引用 contracts 复用枚举或共享 model
- 可依赖框架核心原语（`rust-webx-core`），**禁止**依赖 `host`
- **禁止**引用 handlers

## 数据流

```
HTTP Request
    → contracts (Request 反序列化)
    → handlers (Handler 编排)
    → Arc<dyn I…Service> (handlers 内实现)
    → domain (实体读写)
    → contracts (Response DTO 序列化)
    → HTTP Response
```

## 依赖规则

各层依赖方向与边界见 [职责归属与边界](responsibility-division.md)。

## 反模式

- `contracts` 中 `use crate::domain::*` → DTO 应在 contracts，映射在 handlers
- 独立 `services/` 目录 → 接口归 contracts，实现归 handlers
- Handler 依赖 `Arc<DocService>` → 应使用 `Arc<dyn IDocumentService>`

## 小结

contracts 定义「承诺什么」，handlers 定义「如何履约」，domain 定义「数据是什么」。面向接口开发，让 API、实现与持久化各自独立演化。

下一节：[测试策略](testing-strategy.md)

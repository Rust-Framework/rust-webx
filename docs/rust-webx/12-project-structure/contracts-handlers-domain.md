# Contracts / Handlers / Domain 分层

基于 rust-webx 的业务应用采用**契约驱动、面向接口**的 workspace 结构。关注契约，不关注实现。

## contracts — API 与业务接口契约

**仅依赖框架**（`rust-webx`），**禁止依赖 domain 或 handlers**。

```rust
// contracts/docs/service.rs — 定义「对外承诺什么」

use webx::*;

/// 文档读取服务接口 — 与 Request 同级，同属契约层
pub trait IDocumentService: Send + Sync {
    fn list_works(&self) -> std::result::Result<Vec<String>, String>;
    fn index(&self, work: &str) -> std::result::Result<DocIndex, String>;
    fn content(&self, work: &str, path: &str) -> std::result::Result<DocContent, String>;
}

#[derive(WebxRequestMeta)]
pub struct GetDocIndexRequest {
    #[frompath]
    pub work: String,
}

#[get("/api/docs/{work}/index")]
impl IRequest<DocIndex> for GetDocIndexRequest {}
```

特点：

- Request、Response DTO、共享 enum、`I…Service` trait 均在此层
- 每个请求结构体都派生 `WebxRequestMeta`；字段用 `#[frompath]` / `#[fromquery]` /
  `#[frombody]` / `#[fromform]` / `#[fromheader]` / `#[fromclaim]` 声明来源，
  绑定与 OpenAPI 共用这一处声明
- 需要登录的请求把身份写成 `#[fromclaim] user: IdentityUser`，Handler 直接读 `req.user`，
  见[用户身份与多租户](../05-request-pattern/user-identity.md)
- 路由宏、`#[authorize]`、`#[display]` / `#[description]` / `#[category]` 等元数据在此声明
- 是 OpenAPI 与 `IEndpointCatalog` 的共同数据源；团队讨论 API 时**只看 contracts**
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
    async fn handle(&mut self, req: GetDocIndexRequest) -> ActionResult<DocIndex> {
        self.docs.index(&req.work).map_err(Error::not_found)
    }
}
```

特点：

- Handler 薄编排；复杂逻辑在 Service 实现中
- Handler **只注入** `Arc<dyn I…Service>`
- `#[derive(Inject)]` + `#[handler(inject)]` 自动注册，无需改 `main.rs`
- 需要数据库时声明 owned 字段 `#[inject(owned)] ctx: DbContext`（EFCore 风格 per-request unit-of-work，无需 `Arc<Mutex>`）
- 返回 `ActionResult<T>`，不直接操作 HTTP

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

- 数据库实体、EF 配置、seed、DTO ↔ 实体映射（`mappers/`）
- **可以**引用 contracts 复用枚举或共享 model
- 可依赖框架核心原语（`rust-webx-core`），**禁止**依赖 `host`
- **禁止**引用 handlers

## 映射职责与最佳实践

映射是**下游代码**，不是框架契约。框架给契约、绑定与错误信封，rust-ef 给持久化机制；
DTO、实体与两者之间的转换实现由下游负责，放在下游 domain crate 的
`mappers/<context>_mapper.rs`（`mappers/mod.rs` 汇总），不要堆进一个 `mapper.rs`。

### 职责表

| 归属 | 提供 / 负责 | 不负责 |
|------|-------------|--------|
| 框架（`rust-webx` / `rust-core`） | `IRequest` / `IRequestHandler`、`WebxRequestMeta` 绑定、路由宏与元数据、`Error` / `ActionResult`、`PagedResult` 列表信封、`IPatchable<TPatch>` 契约 | 实体、DTO 字段、DTO ↔ 实体转换实现 |
| rust-ef | `#[derive(EntityType)]` 实体机制、`IEntityTypeConfiguration` 接口与 `ModelBuilder`、`DbContext` 工作单元、查询、`edit` / `detect_changes` / `save_changes`、迁移 | 业务校验、租户与归属规则、DTO |
| 下游（contracts / domain / handlers） | DTO 与 `IRequest`、实体与 `XxxConfiguration` 实现、`TryFrom` / `From<&_>` / `IPatchable` 实现、业务规则、显式租户过滤、Handler 编排 | 另起一套绑定 / 错误信封 / 分页 / 迁移 |

`From` / `Into` / `TryFrom` 属于 `core::convert`，由下游实现；框架**不提供**映射 trait
（唯一例外是 `rust_core::IPatchable`）。本文把它们作为最佳实践指导，不是框架 API；
字段来源、默认值与绑定失败 400 见[参数绑定](../05-request-pattern/parameter-binding.md)。

### 三件套

创建用可失败的 `TryFrom`，读模型用无损的 `From<&Entity>`，部分更新用 `IPatchable`。
`IPatchable` 来自 `rust-core`，实现它的 domain crate 需声明该依赖（`rust-core = "0.1"`）。

```rust
// domain/src/mappers/order_mapper.rs
use rust_core::IPatchable;
use webx_core::{Error, Result};

use crate::entities::Order;
// DTO 来自契约 crate（domain 依赖 contracts，方向见下文）
use contracts::order::{CreateOrderRequest, OrderModel, OrderSummary, UpdateOrderRequest};

/// 创建：`TryFrom` 是可失败转换，领域校验在这里返回错误。
/// `id` / `tenant_id` / 审计列只占位，由 Handler、审计拦截器、数据库决定，
/// 绝不从请求 body 取值。
impl TryFrom<CreateOrderRequest> for Order {
    type Error = Error;

    fn try_from(req: CreateOrderRequest) -> Result<Self> {
        Ok(Self {
            id: String::new(),        // Handler 在 add 前赋值
            tenant_id: String::new(), // Handler 从身份写入
            status: req.status,       // 需要校验时在这里 `?`
            note: req.note,
            created_at: 0,            // AuditInterceptor 覆盖
            updated_at: 0,
        })
    }
}

/// 读：`From<&Entity>` 只做无损投影，不查库、不补算。
impl From<&Order> for OrderModel {
    fn from(order: &Order) -> Self {
        Self {
            id: order.id.clone(),
            status: order.status.clone(),
            note: order.note.clone(),
            created_at: order.created_at,
        }
    }
}

/// 第二投影（列表摘要等）用具名方法，不新增 `ToSummary` 之类的 trait。
impl Order {
    pub fn to_summary(&self) -> OrderSummary {
        OrderSummary {
            id: self.id.clone(),
            status: self.status.clone(),
        }
    }
}

/// 部分更新：只覆盖 `Some` 字段；一个实体有多个更新请求就实现多次该 trait。
impl IPatchable<UpdateOrderRequest> for Order {
    fn patch(&mut self, patch: UpdateOrderRequest) {
        if let Some(status) = patch.status {
            self.status = status;
        }
        if let Some(note) = patch.note {
            self.note = note;
        }
    }
}
```

- **系统字段不来自 body**：`id` 由 Handler（主键生成）或数据库决定，`tenant_id` 由身份写入，
  `created_*` / `updated_*` 由 `AuditInterceptor` 覆盖；`TryFrom` 只负责业务字段与可失败校验。
- **创建不得用 `From` + `Default` 掩盖校验**：`From` 是“不可失败转换”，校验失败必须能被
  `TryFrom` 的 `Err` 表达；字段级规则仍写在 `#[validate(...)]`，两者不要重复。
- **`patch` 不返回 `Result`**：可失败的校验放在请求 `#[validate]`、Handler 或领域规则函数里，
  `patch` 只做赋值；不要为了返回错误再自建 `ApplyTo`。
- **第二投影用具名方法**：`From<&Entity>` 只留给“整行无损投影”，摘要/详情等派生视图不新增 trait。
- 契约 crate 不反向依赖 domain；上面的 `contracts::...` 表示 domain 依赖 contracts，
  这是允许方向（见[职责归属与边界](responsibility-division.md)）。

### Handler 里的写路径

```rust
// 单租户表：按主键取回（多租户表见下）
let order = self.ctx.set::<Order>().query().find(req.id.as_str()).await?
    .ok_or_else(|| Error::not_found("order"))?;

self.ctx.edit(order, |o| o.patch(req.patch));
self.ctx.detect_changes();
self.ctx.save_changes().await?;
```

固定顺序是 `edit → detect_changes → save_changes`：

- `edit` 先 attach 并拍下**原始快照**，再把**跟踪副本**交给闭包；之后的 `detect_changes`
  只把闭包改过的列标脏，`save_changes` 也只 UPDATE 这些列。
- `attach` 只把实体标为 `Unchanged` 并拍照；**先改再 attach** 时快照已包含新值，
  `detect_changes` 看不到差异，会静默不写。
- `update` 直接把实体标为 `Modified`，所有列都会写（docbit 的 `stage_update` 就是这样），
  不适合“只改一部分字段”的语义。
- `detect_changes` 不会自动发生；`edit` 也不自动 save，事务边界仍由 Handler 控制。

多租户表不要用裸主键 `find` 绕过租户：租户条件必须显式写进同一条查询，并放在最前；
租户只来自身份（`require_tenant()` / `IdentityUser`），绝不来自 body 或 query：

```rust
// 请求类型带 `#[fromclaim] user: IdentityUser`
let tenant = req.require_tenant()?;
let q = req.id.as_str();
let order = linq!(self.ctx.set::<Order>(), |o: Order| o.tenant_id == tenant && o.id == q)
    .first_or_default()
    .await?
    .ok_or_else(|| Error::not_found("order"))?;
```

### 禁止项

- 不新增仓储 trait 或泛型包装（`IRepository<T>`、`Repository<T>`、泛型 `Store<T>`）：
  `DbContext` 就是工作单元，查询直接用 `ctx.set::<T>()`。
- 不把映射堆进单文件：按业务上下文拆到 `domain/src/mappers/<context>_mapper.rs`，
  由 `mappers/mod.rs` 汇总。
- 不用已删除的旧入口：`PagedRequest::from_parts`、`OperationResult::ok/ok_with_status`
  在 2.9.1 已移除，见[升级到 2.9.1](../16-migration/upgrade-to-2.9.1.md)。
- 不让 DTO 反向依赖实体：contracts 不 `use domain::...`；转换实现放 domain 或 handlers。
- 不把 `id`、`tenant_id`、审计列放进请求 DTO，也不在 Handler 里用 body 覆盖它们。

> 2.7 示例里的 `ToModel` / `ToEntity` / `ApplyTo` 与 `stage_update` 是示例约定，不是框架 API；
> 2.9.1 的迁移写法见[升级到 2.9.1](../16-migration/upgrade-to-2.9.1.md) §6.5。

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

- `contracts` 中 `use crate::domain::*` → DTO 应在 contracts；映射放 domain 的 `mappers/`，
  不要让 DTO 反向依赖实体
- 独立 `services/` 目录 → 接口归 contracts，实现归 handlers
- Handler 依赖 `Arc<DocService>` → 应使用 `Arc<dyn IDocumentService>`

## 小结

contracts 定义「承诺什么」，handlers 定义「如何履约」，domain 定义「数据是什么」。面向接口开发，让 API、实现与持久化各自独立演化。

下一节：[测试策略](testing-strategy.md)

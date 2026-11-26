# 分页编程模型（契约精简版）

> 类型归属：`PagedRequest` / `PagedResult` / `KeysetRequest` / `CursorPage` /
> `OperationResult` 自 2.9.1 起定义在 **`rust-core` 0.1**；`webx_core::pagination` / `::keyset` /
> `::operation` 是历史路径重导出。本页保留设计理由，权威类型与构造器以 rust-core/rustdoc 为准。
>
> 输入 `PagedRequest`、输出 `PagedResult<T>`，但两者都精简到**只保留服务端才知道的信息**：
> 请求侧去掉手工归一（`from_parts`），响应侧去掉请求回声（`page`/`page_size`/`total_pages`）。
> 端点收敛成一条链：`query.order_by…().to_page(&paging, Model::from)`。

## 1. 为什么可以去掉回声字段

`PagedResult` 里的 `page` / `page_size` / `total_pages` 不是资源状态，而是**请求回声**：

- `page`、`page_size` 是客户端刚发过来的，它一定知道；
- `total_pages` = `ceil(total / page_size)`，客户端一步就能算。

它们存在的唯一理由是"服务端会偷偷改值"——`from_parts` 把 `page_size` 夹到 `1..=100`、把
`page=0` 改成 1。一旦改成**校验而不是夹紧**（越界直接 400），回声就完全多余：

```json
// 旧：{ "items": [...], "total": 120, "page": 1, "page_size": 20, "total_pages": 6 }
// 新：{ "items": [...], "total": 120 }
```

前提（重要）：**clamp 换成 binding 级校验**，否则客户端按自己发的 `page_size=1000` 算页数会和服务端
不一致。默认值（`page=1`、`page_size=20`）也必须落在 binding 层，而不是 handler 里的 `unwrap_or`。

## 2. 固定契约（精简后）

```rust
/// 请求：只有两个字段。默认值（1 / 20）与范围校验（1..=100）在 binding 的
/// `RequestModel for PagedRequest`（`#[fromquery(nested)]`）里完成，越界是 400 字段错误。
#[derive(Deserialize)]
pub struct PagedRequest {
    pub page: u32,
    pub page_size: u32,
}

impl PagedRequest {
    pub fn offset(&self) -> u64;               // (page - 1) * page_size，饱和运算
    pub fn limit(&self) -> u32;                // 即 page_size
}

/// 响应：业务结果 + 本页，扁平在同一个结构里（无嵌套 operation）。
#[derive(Serialize)]
pub struct PagedResult<T> {
    pub success: bool,
    pub status: i32,
    pub message: String,
    pub items: Vec<T>,
    pub total: u64,
}

impl<T> PagedResult<T> {
    pub fn new(items: Vec<T>, total: u64) -> Self;   // success=true / status=0 / "ok"
    pub fn map_items<U>(self, map: impl FnMut(T) -> U) -> PagedResult<U>;
}
```

越界不是"帮你修"，而是明确拒绝：`page_size=1000` → `400 problem+json`
（`errors.page_size = ["must be at most 100"]`），客户端拿到的错误比"被静默改成 100"更可解释。

## 3. 端点：一条链

```rust
async fn handle(&mut self, req: ListOrdersRequest) -> ActionResult<PagedResult<OrderModel>> {
    // req.paging: PagedRequest —— binding 已给默认值、已校验范围
    let page = self
        .orders
        .query()
        .filter(tenant_scope)                       // 租户/权限先于分页
        .order_by_field(Order::created_at)          // 强类型字段常量（见 §3.1）
        .then_by_field(Order::id)                   // 末位唯一列兜底
        .to_page(&req.paging)                       // count + skip/take + 扁平信封
        .await?;
    Ok(page.map_items(OrderModel::from))
}
```

### 3.1 排序必须强类型（与 EF Core 的差别）

EF Core 的 `OrderBy(x => x.CreatedAt)` 能工作，是因为 C# 把 lambda 编译成**表达式树**，运行期可以反射
出成员名。Rust 没有运行期反射：一个 `fn(&Order) -> i64` 的闭包与另一个字段的闭包**类型完全相同**，
运行期拿不到字段名，所以「方法接收闭包」在 Rust 里不可能实现，只能靠宏在编译期看闭包语法。

Rust 的等价物是**派生出的强类型字段常量**，调用点连宏都不需要：

```rust
// rust-ef 实体 derive 生成（与已有的 COLUMN_* 常量同源）
impl Order {
    pub const created_at: Field<Order, i64> = Field::new("created_at");
    pub const id: Field<Order, i64> = Field::new("id");
    pub const amount: Field<Order, Decimal> = Field::new("amount");
}

// QueryBuilder<T>
pub fn order_by_field(self, field: Field<T, impl Sortable>) -> Self;      // ASC
pub fn order_by_field_desc(self, field: Field<T, impl Sortable>) -> Self; // DESC
pub fn then_by_field(self, field: Field<T, impl Sortable>) -> Self;       // 语义同 order_by_field，只表意图
pub fn then_by_field_desc(self, field: Field<T, impl Sortable>) -> Self;
```

收益（全部编译期）：

- 字段拼写错误、字段重命名、跨实体误用 → 编译失败；字符串不再进入排序 API；
- `Field<E, V>` 带 `V: Sortable`，`Vec<u8>`/`JSON` 这类不可排序列在编译期被拒；
- IDE 补全 `Order::` 就能列出全部可排序列；`#[column("legacy_name")]` 改名不影响调用点；
- `column!(Order::created_at)` 与 `Order::created_at` 同源，`column!` 返回 `Field` 而不是裸 `&str`。

常量形式即最终形态：不提供 lambda 糖宏（Rust 无法在运行期从闭包还原成员名，宏糖只增加学习面，
不增加安全性）。它与其语义等价物 EF Core 的 `x => x.CreatedAt` 一样，都是"编译期确定的列"。

**唯一的编译期不可验项**：末位排序列必须唯一。这一点只能靠文档 + `to_page` 的集成测试约定；
若实体元数据暴露唯一键，`to_page` 可在 `debug_assert` 里校验最后一个排序列属于主键/唯一键。

`to_page`（rust-ef 的终端，`crates/core/src/query/builder/page.rs`）：

```rust
impl<T> QueryBuilder<T> {
    /// count（同过滤链、skip/take 之前）+ 本页；直接返回扁平页信封。
    /// 要求：调用前已完成稳定排序（末位唯一列）；否则翻页会漏行/重行。
    pub async fn to_page(self, paging: &PagedRequest) -> EFResult<PagedResult<T>>;
}
```

实现语义：

1. `total = self.clone().count()`（clone 上清 `ORDER BY`/`skip`/`take`，FROM/JOIN/WHERE 不变）；
2. `items = self.skip(offset).take(limit).to_list()`；
3. `PagedResult::new(items, total)`；端点再用 `map_items` 映射 DTO。越界页 `items=[]`、`total` 照实。
4. count 与本页不是同一快照：两者必须一致时包显式事务。

### 2.9.1 分层：执行归 rust-ef，契约归 rust-core，webx 只重导出

分页的**执行**（count + skip/take + 排序语义）与 Web 无关：CLI 导出、后台任务、测试同样需要。
因此终端落在 rust-ef，而不是适配层：

```rust
// rust-ef：与 Web 无关的分页终端（2.9.1）
impl<T> QueryBuilder<T> {
    /// `count`（同一过滤链、在 skip/take 之前）+ 本页；直接返回扁平页信封。
    pub async fn to_page(self, paging: &PagedRequest) -> EFResult<PagedResult<T>>;
}
```

`webx_core::pagination` 只是 `rust_core::pagination` 的 re-export；持久化侧不参与分页或错误翻译
（`rust-webx-ef` 已在 2.9.1 删除；幂等 store 是下游端口实现，参考适配器在
`examples/adapters/idempotency/ef`）。应用直接用 rust-ef 的终端 + `PagedResult::new` 组装。

## 4. 为什么 `from_parts` 能删

`from_parts` 干的三件事现在各有归属：

| 原职责 | 新归属 |
|---|---|
| 缺省值 `1 / 20` | binding 层默认值（`#[serde(default)]` + `#[fromquery]` 支持默认） |
| 越界夹紧 `1..=100` | binding 层校验（`#[validate(range)]` → 400） |
| `Option<u32>` → `u32` | 字段本身就是 `u32`，没有中间态 |

当初的顾虑（binding 没有"默认值"能力，缺值只能靠 `Option<u32>` + `from_parts` 归一）已在 2.9.1
解决：`RequestModel for PagedRequest` 在 `#[fromquery(nested)]` 绑定里给默认值、按 `1..=100`
校验并返回 400，`from_parts` 因此直接删除，不存在"过渡期两套写法"。

## 5. 语义

| 情形 | 结果 |
|---|---|
| 缺省 `page` / `page_size` | 1 / 20（binding 默认） |
| `page_size > 100` 或 `= 0` | 400（`errors.page_size`） |
| `page = 0` | 400（`errors.page`） |
| 越界页 | `items=[]`、`total` 照实 |
| 排序 | 调用前完成，末位唯一列 |
| `total` | 同过滤条件的 `count`（不随 `skip/take` 变化） |

## 6. 与游标分页的关系

游标分页仍用独立的 `KeysetRequest → CursorPage` 契约（追加流/深翻页）。两套契约**判别规则**：
能跳页（用户可见页码）用 offset；只向前续读（消息流/审计日志）用 keyset，同一端点只用一种。

keyset 侧本次**不改**：`KeysetRequest::from_parts(cursor, page_size)` 仍是它的默认值/夹紧入口
（响应 `CursorPage { items, cursor, has_more }`）；`has_more` 是否简化为 `cursor.is_some()`
属于后续评估，不在 2.9.1。

## 7. 影响与迁移

| 旧 | 新 |
|---|---|
| `PagedResult { items, total, page, page_size, total_pages }` | `PagedResult { success, status, message, items, total }` |
| `PagedResult::from_request(items, total, &req)` | `to_page(..)` 或 `PagedResult::new(items, total)` |
| `PagedRequest::from_parts(page, size)` | binding 默认值 + 校验；字段直接是 `u32` |
| `PagedRequest::new` / `clamped` | 删除（没有归一状态） |
| 客户端读 `total_pages` | 客户端 `ceil(total / page_size)` |

破坏性：响应少三个字段、请求越界从"被夹紧"变"400"、构造器删除。客户端与下游 Handler 同批改。

## 8. 验收

- 框架测试：binding 默认值、范围 400、`offset()`、越界页、`total` 与过滤一致。
- `to_page` SQLite 集成测试：与 `count` 同过滤、`LIMIT/OFFSET` 正确、映射后行数、空页。
- 文档：`pagination.md` 已按本模型重写（嵌套绑定 + `to_page` + 扁平信封），不再保留第二套写法。

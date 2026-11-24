# 分页编程模型（契约精简版）

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
/// 请求：字段即绑定字段，默认值 + 范围校验在 binding 层完成。
/// 不再提供 `new` / `from_parts` / `clamped`——没有需要"归一"的状态。
#[derive(Deserialize, WebxRequestMeta)]
pub struct PagedRequest {
    #[serde(default = "one")]                  // binding 默认值
    pub page: u32,                             // #[validate(range(min = 1))]
    #[serde(default = "twenty")]
    pub page_size: u32,                        // #[validate(range(min = 1, max = 100))]
}

impl PagedRequest {
    pub fn offset(&self) -> u32;               // (page - 1) * page_size，饱和运算
}

/// 响应：只保留服务端独有信息。
#[derive(Serialize)]
pub struct PagedResult<T> {
    pub items: Vec<T>,
    pub total: u64,
}

impl<T: Serialize> PagedResult<T> {
    pub fn new(items: Vec<T>, total: u64) -> Self;
}
```

越界不是"帮你修"，而是明确拒绝：`page_size=1000` → `400 problem+json`
（`errors.page_size = ["must be at most 100"]`），客户端拿到的错误比"被静默改成 100"更可解释。

## 3. 端点：一条链

```rust
async fn handle(&mut self, req: ListOrdersRequest) -> ActionResult<PagedResult<OrderModel>> {
    // req.paging: PagedRequest —— binding 已给默认值、已校验范围
    self.orders
        .query()
        .filter(tenant_scope)                       // 租户/权限先于分页
        .order_by(Order::created_at)                // 强类型字段常量（见 §3.1）
        .then_by(Order::id)                         // 末位唯一列兜底
        .to_page(&req.paging, OrderModel::from)     // count + skip/take + map + 信封
        .await
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
pub fn order_by(self, field: Field<T, impl Sortable>) -> Self;      // ASC
pub fn order_by_desc(self, field: Field<T, impl Sortable>) -> Self; // DESC
pub fn then_by(self, field: Field<T, impl Sortable>) -> Self;       // 语义同 order_by，只表意图
pub fn then_by_desc(self, field: Field<T, impl Sortable>) -> Self;
```

收益（全部编译期）：

- 字段拼写错误、字段重命名、跨实体误用 → 编译失败；字符串不再进入排序 API；
- `Field<E, V>` 带 `V: Sortable`，`Vec<u8>`/`JSON` 这类不可排序列在编译期被拒；
- IDE 补全 `Order::` 就能列出全部可排序列；`#[column("legacy_name")]` 改名不影响调用点；
- `column!(Order::created_at)` 与 `Order::created_at` 同源，`column!` 返回 `Field` 而不是裸 `&str`。

若更喜欢 lambda 写法，可提供纯语法糖宏（零运行时成本）：

```rust
.order_by(col!(|o: Order| o.created_at))   // 宏展开为 Order::created_at
```

但推荐常量形式：没有宏、IDE 友好，且与 EF Core 的 `x => x.CreatedAt` 语义等价——都是"编译期确定的列"。

**唯一的编译期不可验项**：末位排序列必须唯一。这一点只能靠文档 + `to_page` 的集成测试约定；
若实体元数据暴露唯一键，`to_page` 可在 `debug_assert` 里校验最后一个排序列属于主键/唯一键。

`to_page`（`webx_ef`，基于 rust-ef `QueryBuilder<T>`）：

```rust
#[async_trait]
pub trait PageExt<T>: Sized {
    /// count（同过滤链、skip/take 之前）+ 本页取数 + 映射 + 信封。
    /// 要求：调用前已完成稳定排序（末位唯一列）；否则翻页会漏行/重行。
    async fn to_page<M: Serialize>(
        self,
        paging: &PagedRequest,
        map: impl FnMut(T) -> M + Send,
    ) -> webx_core::error::Result<PagedResult<M>>;
}
```

实现语义：

1. `total = self.clone().count()`；2. `items = self.skip(offset).take(page_size).to_list()`；
3. `items.into_iter().map(map)`；4. `PagedResult::new(items, total)`。越界页 `items=[]`、`total` 照实。

### 3.0 分层：执行归 rust-ef，信封归 webx，webx_ef 只做翻译

分页的**执行**（count + skip/take + 排序语义）与 Web 无关：CLI 导出、后台任务、测试同样需要。
因此终端应落在 rust-ef，而不是适配层：

```rust
// rust-ef：与 Web 无关的分页终端（新增，加性）
impl<T> QueryBuilder<T> {
    /// `count`（同一过滤链、在 skip/take 之前）+ 本页；`offset/limit` 是中性数字。
    pub async fn to_page(self, offset: u64, limit: u32) -> EFResult<(Vec<T>, u64)>;
}
```

`webx_ef` 只保留两件交叉边界的翻译：`page/page_size → offset/limit` 与
`(items, total) → PagedResult`（外加可选的 `map`）。没有 ORM 执行逻辑，删掉它应用也能直接
用 `rust-ef` 的终端 + `PagedResult::new` 组装。

## 4. 为什么 `from_parts` 能删

`from_parts` 干的三件事现在各有归属：

| 原职责 | 新归属 |
|---|---|
| 缺省值 `1 / 20` | binding 层默认值（`#[serde(default)]` + `#[fromquery]` 支持默认） |
| 越界夹紧 `1..=100` | binding 层校验（`#[validate(range)]` → 400） |
| `Option<u32>` → `u32` | 字段本身就是 `u32`，没有中间态 |

当前 binding 还没有"默认值"能力（`#[fromquery]` 走 `FromStr`，缺值即错，所以现状用
`Option<u32>` + `from_parts`）。要落到本模型，需要给 `#[fromquery]` 增加默认值支持
（`#[fromquery(default = 20)]`，与 `#[validate(range)]` 组合），这是本次唯一的框架侧前置改动。

在它落地之前，`from_parts` 仍是默认值的唯一住处；一旦默认值进 binding，`from_parts` 直接删除。

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

游标分页仍用独立的 `KeysetRequest → CursorPage` 契约（追加流/深翻页），同样按本模型的思路精简：
请求侧去掉 `from_parts`（默认值进 binding），响应侧 `CursorPage { items, cursor, has_more }`
本来就是服务端独有信息（`has_more` 例外，可用 `cursor.is_some()` 表达，后续再评估）。

## 7. 影响与迁移

| 旧 | 新 |
|---|---|
| `PagedResult { items, total, page, page_size, total_pages }` | `PagedResult { items, total }` |
| `PagedResult::from_request(items, total, &req)` | `to_page(..)` 或 `PagedResult::new(items, total)` |
| `PagedRequest::from_parts(page, size)` | binding 默认值 + 校验；字段直接是 `u32` |
| `PagedRequest::new` / `clamped` | 删除（没有归一状态） |
| 客户端读 `total_pages` | 客户端 `ceil(total / page_size)` |

破坏性：响应少三个字段、请求越界从"被夹紧"变"400"、构造器删除。客户端与下游 Handler 同批改。

## 8. 验收

- 框架测试：binding 默认值、范围 400、`offset()`、越界页、`total` 与过滤一致。
- `to_page` SQLite 集成测试：与 `count` 同过滤、`LIMIT/OFFSET` 正确、映射后行数、空页。
- 文档：`pagination.md` 只保留本模型的一段模板，删除第二套写法。

# 统一分页（PagedRequest / PagedResult / Keyset）

列表端点一律使用框架随附的分页类型：**offset**（请求 `PagedRequest`、响应 `PagedResult<T>`）或
**keyset/游标**（请求 `KeysetRequest`、响应 `CursorPage<T>`）。业务模块不要再自定义
`Page<T>`、`PageQuery`、`next_cursor`、`limit + cursor` 之类的分页结构，也不要手写
`{items, total, page, page_size}` 信封——否则前端/客户端要按端点各写一套解码逻辑。

## 契约

| 位置 | 类型 | 说明 |
|------|------|------|
| Query | `#[fromquery(nested)] paging: PagedRequest` | `PagedRequest` 只有 `page: u32` / `page_size: u32` 两个字段；缺省 1 / 20，越界 400 |
| 响应 | `PagedResult<T>` | 200 + `{success, status, message, items, total}` 扁平页信封 |

```rust
#[derive(WebxRequestMeta)]
pub struct ListUsersRequest {
    #[fromquery]
    pub keyword: Option<String>,
    #[fromquery(nested)]
    pub paging: PagedRequest,
}

#[get("/api/users")]
impl IRequest<PagedResult<UserDto>> for ListUsersRequest {}
```

- 请求侧不再把 `page` / `page_size` 写成两个平铺的 `Option<u32>` 字段：`#[fromquery(nested)]`
  把 `paging` 委托给 `PagedRequest` 自己的 `RequestModel` 绑定，并把 `page` / `page_size`
  拼进 OpenAPI 参数。
- 响应侧 `PagedResult<T>` 是**扁平页信封**：`success`、`status`、`message`、`items`、`total`
  五个字段都是结构体直接成员，没有嵌套的 `operation`，也没有页码、页大小的请求回声；
  客户端用自己发出的 `page_size` 计算总页数。
- 结果由 `PagedResult::new(items, total)` 构造；`map_items` 把实体页映射成 DTO 页，
  `with_status` / `with_message` 覆盖业务状态与文案。

## 默认值与校验在 binding 层

`PagedRequest` 没有 `new`、`Default`、serde 默认值，也没有任何归一化方法：默认值与范围
属于 binding，Handler 拿到的一定是已校验的值。

| 情况 | 结果 |
|------|------|
| 未提供 | `page = 1`、`page_size = 20` |
| `page < 1` | `400 problem+json`，`errors.page` |
| `page_size` 不在 `1..=100` | `400 problem+json`，`errors.page_size` |
| 负数 / 非数字 | 绑定阶段 `400`，不会进 Handler |

Handler 不要再写 `unwrap_or` / `clamp` 做二次归一：越界是客户端错误（400），静默夹紧会
让客户端按请求页大小算出的总页数与实际收到的页不一致。

## Handler 示例

**优先用 `to_page`**：同一条过滤链上先 `COUNT` 再取本页，直接返回 `PagedResult<T>`。

```rust
#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListUsersRequest, PagedResult<UserDto>> for ListUsersHandler {
    async fn handle(&mut self, req: ListUsersRequest) -> ActionResult<PagedResult<UserDto>> {
        // 过滤条件只构建一次；租户条件最先 AND，租户只来自身份
        let query = self
            .users
            .query()
            .filter(/* 租户 + keyword 等条件 */);

        // 同一过滤链：count 去掉 ORDER BY/skip/take，本页 skip/take
        let page = query
            .order_by_field_desc(User::created_at)
            .then_by_field_desc(User::id) // 唯一列兜底，翻页不漂移
            .to_page(&req.paging)
            .await?; // 持久层错误在框架边界转换

        // 实体页 → DTO 页；total 与 success/status/message 原样保留
        Ok(page.map_items(UserDto::from))
    }
}
```

**手写等价做法**（需要自定义语句形状时，仍然只用框架类型收口）：

```rust
let total = query.clone().count().await? as u64;
let items = query
    .order_by_field_desc(User::created_at)
    .then_by_field_desc(User::id)
    .skip(req.paging.offset() as usize)
    .take(req.paging.limit() as usize)
    .to_list()
    .await?;
Ok(PagedResult::new(items, total).map_items(UserDto::from))
```

要点：

- `offset() -> u64`、`limit() -> u32` 是分页参数的唯一入口；`offset` 在 `u64` 上计算，
  `page` / `page_size` 再大也不会溢出。
- 排序必须**稳定且唯一**：只按 `created_at` 排序时，同时间戳的行在两次查询间顺序可能变化，
  翻页会重复/漏行；用主键或其它唯一列兜底。
- 强类型排序用 `order_by_field(User::created_at)` / `then_by_field_desc(User::id)`，
  字段类型必须实现 `Sortable`；只有列名来自运行期白名单映射时才用显式 raw 入口
  `order_by_raw` / `order_by_desc_raw`（2.9.1 起 `order_by(&str)` / `order_by_desc(&str)`
  已删除且无别名，见 [upgrade-to-2.9.1.md](../16-migration/upgrade-to-2.9.1.md) §3）。
- 深分页（`page` 很大）仍是 `OFFSET` 扫描：持续追加、需要翻到很深的数据改用下面的
  Keyset 分页；导出/批量场景用独立端点。

## Keyset（游标）分页

offset 分页在并发写入时会翻页重复/漏行，深页还会做全量 `OFFSET` 扫描。对**消息流、审计日志、
事件列表**这类持续追加的数据用 keyset：客户端拿不透明游标，问"这个键之后的 N 行"。

| 位置 | 类型 | 说明 |
|---|---|---|
| Query | `cursor: Option<String>`、`page_size: Option<u32>` | 两个平铺字段；`KeysetRequest::from_parts(cursor, page_size)` 归一（默认 `20`，夹紧 `1..=100`） |
| 响应 | `CursorPage<T>` | `{ items, cursor?, has_more }`；`cursor` 为 `None` 表示没有下一页 |

```rust
#[derive(WebxRequestMeta)]
pub struct ListMessagesRequest {
    #[fromquery] pub cursor: Option<String>,
    #[fromquery] pub page_size: Option<u32>,
    #[fromquery] pub room_id: String,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListMessagesRequest, CursorPage<MessageDto>> for ListMessagesHandler {
    async fn handle(&mut self, req: ListMessagesRequest) -> ActionResult<CursorPage<MessageDto>> {
        let paging = KeysetRequest::from_parts(req.cursor, req.page_size);
        let (created_at, id) = match paging.decoded()?.as_slice() {
            [] => (i64::MIN, String::new()),           // 第一页
            [at, id, ..] => (
                at.parse()
                    .map_err(|_| Error::validation("invalid cursor"))?, // 客户端输入：必须校验
                id.clone(),
            ),
            _ => return Err(Error::validation("invalid cursor")),
        };
        // WHERE room_id = ? AND (created_at, id) > (?, ?)
        // ORDER BY created_at ASC, id ASC LIMIT page_size + 1
        let rows = load_after(&mut self.ctx, &req.room_id, created_at, &id, paging.page_size() + 1)
            .await?;
        Ok(CursorPage::from_rows(rows, paging.page_size(), |row| {
            vec![row.created_at.to_string(), row.id.clone()]
        }))
    }
}
```

要点：

- **多取一行**：查 `page_size + 1` 行，`CursorPage::from_rows` 用多出的行判断 `has_more`
  并把它丢弃；不需要额外 `COUNT`。
- **稳定全序**：游标必须覆盖完整排序键（例如时间戳 + 唯一 id 兜底）；只按时间排序会
  在游标边界跳过或重复行。
- **游标是客户端输入**：`Cursor::decode` 只验证编码，解码出的值必须校验，并且租户/用户 id
  只能来自身份，不能来自游标。
- **编码器用框架的**：`Cursor::encode` / `Cursor::decode` 负责不透明编码；业务只决定排序键组成。
- 第一页传 `cursor=None`；响应 `cursor=None` 表示结束。

## 为什么保留两套（不是重复设计）

两套契约服务的是不同的查询模式，不能用一个类型的“可选字段”同时表达：

| | offset（`PagedRequest` / `PagedResult`） | keyset（`KeysetRequest` / `CursorPage`） |
|---|---|---|
| 能做什么 | 任意跳页、返回 `total`、后台管理/小列表 | 只取“游标之后 N 行”，无 `total`、无跳页 |
| 深页成本 | `OFFSET` 随页数线性变慢 | 按稳定排序键 seek，深页成本稳定 |
| 并发写入 | 翻页可能重复/漏行（除非排序唯一且业务接受） | 以游标为界，不受已读区插入/删除影响 |
| 响应形状 | 扁平业务页 `{success,status,message,items,total}` | 纯游标页 `{items,cursor?,has_more}`，不含业务状态字段 |
| 归一化 | 默认值/`1..=100` 校验在绑定层（越界 400） | `KeysetRequest::from_parts` 负责默认值/夹紧（历史契约） |

把两者合成为一个带 `total: Option<...>`、`cursor: Option<...>` 的“万能页”，会让互斥字段
同时存在、wire 形状依赖运行期取值，客户端也必须理解“什么时候该看哪个字段”。显式两套类型
让不可能状态在类型层不出现；代价是应用要按场景选一套，而不是两套混用。

现状边界（2.x 保持兼容）：offset 与 keyset 的归一化位置不同（前者在绑定层 fail-closed，
后者仍是 `from_parts` 夹紧）；`CursorPage` 也没有 `PagedResult` 的 `success/status/message`
业务信封字段。不要在同一个端点里混用，也不要为某个端点再造第三种分页结构；若未来 major
统一两套契约，按兼容政策单独出迁移指南，不在 2.x 补丁里改。

## 反例

```rust
// ❌ 自建分页结构（框架已有两套）
struct Page<T> { items: Vec<T>, total: u64 }
struct PageQuery { limit: Option<u32>, cursor: Option<String> }

// ❌ 每个端点各写一套默认值与手工夹紧
let page = req.page.unwrap_or(1).max(1);
let page_size = req.page_size.unwrap_or(20).clamp(1, 100);
// ✅ 缺省 1 / 20、越界 400 交给嵌套绑定，Handler 直接
//    `#[fromquery(nested)] paging: PagedRequest` + `query.to_page(&req.paging)`
// ✅ Keyset 才用框架的 `from_parts` 归一
let keyset = KeysetRequest::from_parts(req.cursor, req.page_size);

// ❌ 响应信封自造 { list, count, current, size } / { data, next }
// ✅ 返回 PagedResult<T> 或 CursorPage<T>，字段名由框架固定
```

## 小结

- offset：请求 `#[fromquery(nested)] paging: PagedRequest`（缺省 1 / 20、越界 400 由 binding
  负责）；响应 `PagedResult<T>` 扁平信封（`success`/`status`/`message`/`items`/`total`）。
  `to_page` 在同一过滤链上 count + skip/take；排序末位用唯一列兜底。
- keyset：请求 `cursor` + `page_size` + `KeysetRequest::from_parts`；响应 `CursorPage<T>`
  （多取一行、稳定全序、游标值校验）。
- 两套都用框架类型；不要为某个端点另造第三种分页结构，也不要在两套之间混用（别拿
  `CursorPage` 当 offset 分页，也别把 `PagedResult` 套进游标逻辑）。

下一节：[用户身份与多租户](user-identity.md)

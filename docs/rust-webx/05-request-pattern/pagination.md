# 统一分页（PagedRequest / PagedResult）

列表端点一律使用框架随附的分页类型：请求侧用 `PagedRequest` 表达 `page` / `page_size`，
响应侧用 `PagedResult<T>` 作为统一信封。业务模块不要再自定义 `Page<T>`、`PageQuery`、
`next_cursor`、`limit + cursor` 之类的分页结构，也不要手写 `{items, total, page, page_size}`
信封——否则前端/客户端要按端点各写一套解码逻辑。

## 契约

| 位置 | 类型 | 说明 |
|------|------|------|
| Query | `page: Option<u32>`、`page_size: Option<u32>` | 两个**平铺**字段；不要 `#[serde(flatten)] PagedRequest` |
| 响应 | `PagedResult<T>` | 200 + `{items, total, page, page_size, total_pages}` |

```rust
#[derive(WebxRequestMeta)]
pub struct ListUsersRequest {
    #[fromquery]
    pub page: Option<u32>,
    #[fromquery]
    pub page_size: Option<u32>,
    #[fromquery]
    pub keyword: Option<String>,
}

#[get("/api/users")]
impl IRequest<PagedResult<UserDto>> for ListUsersRequest {}
```

绑定层负责类型合法性：`page` / `page_size` 是 `u32`，负数或非数字在绑定阶段就是
`400 problem+json`（按字段聚合错误），不会进 Handler（见[参数绑定](parameter-binding.md)）。

## 归一化：`clamped()` 与 `from_parts()`

`PagedRequest` 的默认值与边界是框架约定，Handler 不应重复判断：

| 情况 | 结果 |
|------|------|
| 未提供 | `page = 1`、`page_size = 20` |
| `page = 0` | 归一为 `1`（0 无法计算 offset） |
| `page_size = 0` | 归一为 `1`（避免 `total_pages` 除零） |
| `page_size > 100` | 限制为 `100`（防止一次拉全表） |

- `PagedRequest::new(page, page_size)` 原样构造后调用 `.clamped()` 归一。
- `PagedRequest::from_parts(Option<u32>, Option<u32>)`（1.3.0 新增）把「默认值 + 边界」
  合成一步，等价于 `PagedRequest::new(page.unwrap_or(1), page_size.unwrap_or(20)).clamped()`：

```rust
let paging = PagedRequest::from_parts(req.page, req.page_size);
// 省略参数 => 1 / 20；越界值被夹紧，而不是报错
```

| 输入 | `from_parts` 结果 |
|------|------------------|
| `(None, None)` | `page = 1`、`page_size = 20` |
| `(Some(0), Some(0))` | `page = 1`、`page_size = 1` |
| `(Some(3), Some(1_000))` | `page = 3`、`page_size = 100` |
| `(Some(4), None)` | `page = 4`、`page_size = 20` |

## Handler 示例

分页查询有三个必须一起满足的点：**同条件 `count`**、**稳定排序**、**`offset` + `page_size`**。

```rust
#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListUsersRequest, PagedResult<UserDto>> for ListUsersHandler {
    async fn handle(&mut self, req: ListUsersRequest) -> ActionResult<PagedResult<UserDto>> {
        let paging = PagedRequest::from_parts(req.page, req.page_size);

        // 1. 过滤条件只构建一次，count 与列表复用（含租户条件，租户只来自身份）
        let query = self.users.query().filter(/* keyword 等条件 */);

        // 2. 同条件总数
        let total = query.clone().count().await? as u64;

        // 3. 稳定排序（末位用唯一列兜底，避免翻页漂移）+ skip/take
        let items = query
            .order_by_asc(/* created_at */)
            .order_by_asc(/* id */)
            .skip(paging.offset() as usize)
            .take(paging.page_size as usize)
            .all()
            .await?;

        // 4. 统一信封；total_pages 由框架按 page_size 计算
        Ok(PagedResult::from_request(items, total, &paging))
    }
}
```

要点：

- `offset()` 基于**归一化后**的 `page` / `page_size` 计算：`(page - 1) * page_size`（饱和运算）。
- `PagedResult::from_request(items, total, &paging)` 直接复用归一化后的页码，
  不要再把原始 query 值写回响应（否则前端拿到的页码与实际数据不一致）。
- 排序必须**稳定且唯一**：只按 `created_at` 排序时，同时间戳的行在两次查询间顺序可能变化，
  翻页会重复/漏行；用主键或其它唯一列兜底。
- 深分页（`page` 很大）仍是 `OFFSET` 扫描；导出/批量场景用独立端点 + 内部游标，
  不要把它做成列表端点的参数。

## 反例

```rust
// ❌ 自建分页结构
struct Page<T> { items: Vec<T>, total: u64 }
struct PageQuery { limit: Option<u32>, cursor: Option<String> }

// ❌ 每个端点各写一套默认值与边界
let page = req.page.unwrap_or(1).max(1);
let page_size = req.page_size.unwrap_or(20).clamp(1, 100);
// ✅ 一行替代
let paging = PagedRequest::from_parts(req.page, req.page_size);

// ❌ 响应信封自造 { list, count, current, size }
// ✅ 返回 PagedResult<T>，字段名由框架固定
```

## 小结

- 请求：两个平铺的 `Option<u32>` query 字段；Handler 用 `PagedRequest::from_parts` 归一。
- 响应：`PagedResult<T>`，用 `from_request` 构造。
- 查询：同条件 `count` + 稳定排序 + `offset`/`page_size`。

下一节：[用户身份与多租户](user-identity.md)

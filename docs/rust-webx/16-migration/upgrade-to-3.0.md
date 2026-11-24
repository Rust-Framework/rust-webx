# 从 2.9 迁移到 3.0

> 3.0 是破坏性版本：错误载体、分页契约、嵌套绑定、强类型排序与部分 HostBuilder 入口都变了。
> 本文只列**源码迁移动作**；逐条证据与设计见 `docs/design/app-error-migration.md`、
> `pagination-3.0-migration.md`、`api-convergence.md`。

## 1. 错误载体：`webx::Error = rust_core::AppError`

`webx_core::Error` 现在是 `rust_core::AppError` 的别名，2.9 的 19 变体 enum 已删除。

| 2.9 | 3.0 |
|---|---|
| `Error::Internal(msg)` | `Error::internal(msg)`（operator-only detail） |
| `Error::Validation(msg)` | `Error::validation(msg)` |
| `Error::Status(code, msg)` | `Error::status(code, msg)` |
| `Error::Http(msg)` | `Error::bad_request(msg)` |
| `Error::Routing(msg)` | `Error::route_not_found(msg)` |
| `Error::Serialization(e)` | `Error::invalid_json(e)` |
| `Error::ResponseSerialization(e)` | `Error::response_serialization(e)` |
| `Error::TooManyRequests { retry_after_secs, detail }` | `Error::too_many_requests(retry_after_secs, detail)` |
| `Error::ConflictRetryable(msg)` | `Error::conflict_retryable(msg)` |
| `Error::Message(msg)` | `Error::internal(msg)` |
| `Error::Problem(p)` | `p.into()` / `ProblemDetails::to_error()` |

模式匹配改为访问器：

```rust
// before
match error {
    Error::Internal(detail) => log(detail),
    Error::Status(503, _) => retry(),
    Error::Problem(p) => write(p),
    _ => {}
}

// after
tracing::error!(operator = error.operator(), "internal failure");
if error.status_code() == 503 { retry(); }
let problem = ProblemDetails::from_app_error(&error, path, trace_id);
```

- `Result<T>` / `ActionResult<T>` 仍是 `rust_core::Result` 别名；
- `RdiError` / `EFError` 在各自 crate 边界实现 `From`，handler 直接 `?`；
- `Display` 只打印 operator/detail/code，不能用于客户端文案；用 `public_detail()`；
- `x-webx-*` 是保留扩展键：自定义 `IErrorResponseWriter` 必须用
  `webx_core::problem::extensions_for_wire`，`Retry-After` 用 `retry_after_of`。

## 2. 分页契约

```rust
// before (2.9)
let paging = PagedRequest::from_parts(page, page_size);
let page = PagedResult::from_request(items, total, &paging);

// after (3.0)
let paging = PagedRequest { page, page_size }; // binding 负责默认/400 校验
let page = query.to_page(&paging).await?;     // 直接返回 PagedResult<T>
```

- `PagedRequest` 只有 `page` / `page_size` 字段；无 `new/from_parts/clamped/Default`；
  `offset() -> u64`；
- `PagedResult<T>` 是扁平信封 `{success,status,message,items,total}`；
  `new(items, total)`；无 `page/page_size/total_pages` 回声，无 `from_request`；
- 列表请求推荐嵌套绑定：

```rust
#[derive(WebxRequestMeta)]
struct ListOrdersRequest {
    #[fromclaim] user: IdentityUser,
    #[fromquery] keyword: Option<String>,
    #[fromquery(nested)] paging: PagedRequest,
}
```

`page < 1` / `page_size ∉ 1..=100` 是 400 字段错误（不再静默夹紧），且
`page/page_size` 会进入 OpenAPI。

## 3. 强类型排序

```rust
// before
query.order_by(Order::COLUMN_CREATED_AT).order_by_desc("id");

// after
query
    .order_by_field(Order::created_at)
    .then_by_field_desc(Order::id);
```

- `order_by_field*` 要求字段类型实现 `Sortable`；`Vec<u8>`、JSON、任意 struct 编译期拒绝；
- 2.9 的字符串 `order_by` 仍存在一个版本用于迁移（3.0 内保留给 grouped projection 的
  原始表达式入口）；新代码不应使用。

## 4. HostBuilder 入口收敛（A 批）

| 2.9 | 3.0 |
|---|---|
| `use_session_store_cached(store, ttl)` | `use_session_store(Arc::new(CachedSessionStore::new(store, ttl)))` |
| `use_h3_alt_svc(port)` | `use_alt_svc("h3=\":443\"; ma=86400")` |
| `add_localization()` / `add_localization_with(opts)` | `use_localization(LocalizationOptions)` |
| `add_memory_cache()` / `add_memory_cache_with(n)` | `add_memory_cache(CacheOptions::default())` / `...max_entries(n)` |
| `OperationResult::ok(payload, msg)` | `OperationResult::success(msg).with_result(payload)` |
| `OperationResult::ok_with_status(s, p, m)` | `OperationResult::success(m).with_status(s).with_result(p)` |
| `OperationResult::success_with_status(s, m)` | `OperationResult::success(m).with_status(s)` |

`CacheOptions::max_entries(0)` 在注册期拒绝。

## 5. 依赖分层：根 workspace 不再包含 `rust-ef`

3.0 的根 workspace 只包含非 EF 运行时 crate、协议适配器与 `imbit`。EF starters
(`ai/auth/payment/outbox/idempotency` + `webx_ef` 兼容层) 和 docbit/dmbit 参考应用
在独立的 `starters/Cargo.toml` workspace。

```bash
cargo test --workspace
cargo test --workspace --manifest-path starters/Cargo.toml
```

`webx_ef` 仅保留兼容重导出（含 `webx_ef::idempotency::*`），3.0 后应直接依赖
`rust-webx-idempotency-ef`；`map_ef_error` / `EfResultExt` 已删除，持久层错误用 `?`。

## 6. 发布顺序

`rust-core 0.1` → `rust-dix 1.1` → `rust-ef 2.9.1` → `rust-webx 2.9.1`（3.0 分支随后）。
3.0 工作区里的 `[patch.crates-io]` 指向本地兄弟仓，发布后移除。

# 升级到 1.0.2（破坏性变更）

> 目标读者：正在使用 rust-webx 1.0.0 / 1.0.1 的项目。1.0.2 是 1.x 线上的补丁版本，但包含
> 两个**公共命名**的破坏性调整：Handler 的返回类型改名为 `ActionResult<T>`，列表信封
> `PagedResponse<T>` 改名为 `PagedResult<T>`。从 1.0.2 继续升级请接着看
> [升级到 1.1](upgrade-to-1.1.md)。

## 1. 依赖升级

```toml
[dependencies]
rust-webx = "1.0.2"
```

## 2. `handle` 返回类型：`Result<T, Error>` → `ActionResult<T>`

`ActionResult<T>` 就是 ASP.NET Core 的 `ActionResult<T>`：`Ok(value)` 是响应体，`Err(Error)`
由框架映射成 HTTP 状态码与 `problem+json`。它与 `Result<T, Error>` 是同一个类型
（`T` 默认 `()`），所以**返回值不用改**，只要把签名里的类型名换掉：

```rust
// 1.0.0 / 1.0.1
use webx::*;

impl IRequestHandler<GetUserRequest, UserDto> for GetUserHandler {
    async fn handle(&mut self, req: GetUserRequest) -> Result<UserDto> {
        Ok(self.repo.find(&req.id).await?)
    }
}

// 1.0.2+
impl IRequestHandler<GetUserRequest, UserDto> for GetUserHandler {
    async fn handle(&mut self, req: GetUserRequest) -> ActionResult<UserDto> {
        Ok(self.repo.find(&req.id).await?)
    }
}
```

`IPipelineBehavior::handle` 与 `Next::run` 使用同一返回类型名。基础设施代码（仓储、服务）继续用
`Result<T>`；`Result<T>` 仍是通用的 `Result<T, Error>` 别名，没有被删除。

## 3. `PagedResponse<T>` → `PagedResult<T>`

列表端点响应信封改名为 `PagedResult<T>`，与 `OperationResult` 配对：

```rust
// 1.0.0 / 1.0.1
impl IRequest<PagedResponse<UserDto>> for ListUsersRequest {}

// 1.0.2+
impl IRequest<PagedResult<UserDto>> for ListUsersRequest {}
```

`PagedResult` 字段不变：`items`、`total`、`page`、`page_size`、`total_pages`；
`PagedRequest::new/clamped` 构造分页参数的方式也不变。

## 4. 新增：`OperationResult`

1.0.2 随附业务结论 DTO `OperationResult` / `OperationResult<T>`（`success`、业务 `status`、
`message`、可选 `result`），用来替代各模块自定义的 `{ success, status, message }` 信封。
它是普通 DTO、不是传输类型：HTTP 状态码仍由框架决定。可选用，不迁移不影响编译。

## 5. 升级检查清单（grep）

```bash
rg 'PagedResponse' src/            # 应为空（全部换成 PagedResult）
rg -- '-> Result<[^>]*>' src/      # handler 里应为空；基础设施代码可保留 Result<T>
rg 'ActionResult<' src/            # handler / pipeline 的新返回类型
cargo test                        # 全量测试
```

## English summary

rust-webx 1.0.2 is a 1.x patch that renames two public shapes:

- `IRequestHandler::handle` / `IPipelineBehavior::handle` / `Next::run` return
  **`ActionResult<R>`** (the ASP.NET Core `ActionResult<T>` name; the same type as
  `Result<R, Error>`, `T` defaults to `()`). Return values are unchanged — only the
  signature spelling changes. `Result<T>` stays the general fallible alias for
  infrastructure code.
- **`PagedResponse<T>` is renamed `PagedResult<T>`** (same `items`/`total`/`page`/
  `page_size`/`total_pages` fields).
- **New** `OperationResult` / `OperationResult<T>`: an optional business-result DTO
  (`success` + business `status` + `message` + optional `result`); HTTP status stays
  the framework's.

Continue with [upgrade to 1.1](upgrade-to-1.1.md) for the `IActionResult` response model.

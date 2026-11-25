# 错误处理与 ProblemDetails

## Error 类型

框架统一错误类型 `webx::Error` 是 `rust_core::AppError` 的别名（2.9.1 起不再有第二套 enum），
字段私有，用语义构造器创建、用访问器读取：

```rust
use rust_core::AppError; // `webx::Error` 就是它的别名

impl AppError {
    // 400：JSON 解析 / 通用请求错误 / 业务校验
    pub fn invalid_json(error: serde_json::Error) -> Self;
    pub fn bad_request(detail: impl Into<String>) -> Self;
    pub fn validation(detail: impl Into<String>) -> Self;       // 字段级错误用 with_field
    // 401 / 403 / 404 / 409
    pub fn unauthorized(detail: impl Into<String>) -> Self;
    pub fn forbidden(detail: impl Into<String>) -> Self;
    pub fn not_found(detail: impl Into<String>) -> Self;
    pub fn route_not_found(detail: impl Into<String>) -> Self;
    pub fn conflict(detail: impl Into<String>) -> Self;         // 永久 409
    pub fn conflict_retryable(detail: impl Into<String>) -> Self; // 可等待的 409
    // 413 / 415 / 429 / 503
    pub fn payload_too_large(detail: impl Into<String>) -> Self;
    pub fn unsupported_media_type(detail: impl Into<String>) -> Self;
    pub fn too_many_requests(retry_after_secs: u64, detail: impl Into<String>) -> Self;
    pub fn unavailable(detail: impl Into<String>) -> Self;      // 可重试 503
    // 500：消息只进 operator 日志，客户端拿不到 detail
    pub fn internal(operator: impl Into<String>) -> Self;
    pub fn di(operator: impl Into<String>) -> Self;
    pub fn response_serialization(error: serde_json::Error) -> Self;
    pub fn status(status: u16, detail: impl Into<String>) -> Self; // 显式状态码逃生口
}
```

读取用访问器，不再 match 变体：

```rust
let status = err.status_code();      // 429 / 503 / ...
let code = err.code();               // "too_many_requests"
let public = err.public_detail();    // 可展示给客户端的文案
let operator = err.operator();       // 只进日志 / 5xx 细节
let fields = err.fields();           // 绑定/校验的字段问题
```

`kind()` 返回 `rust_core::ErrorKind`（`#[non_exhaustive]`）；跨 crate 判断优先用
`status_code()` / `code()`，不要依赖内部枚举的穷举 match。

## HTTP 状态码映射

| 构造器 | HTTP 状态码 | code | 典型场景 |
|-----------|-----------|------|---------|
| `bad_request(..)` | 400 | `bad_request` | 通用 HTTP 错误 |
| `validation(..)` | 400 | `validation_failed` | 业务校验失败 |
| `invalid_json(e)` | 400 | `invalid_json` | JSON 解析错误 |
| `unauthorized(..)` | 401 | `unauthorized` | 认证失败、缺少令牌 |
| `forbidden(..)` | 403 | `forbidden` | 权限不足 |
| `not_found(..)` | 404 | `not_found` | 资源不存在 |
| `route_not_found(..)` | 404 | `route_not_found` | 路由错误 |
| `conflict(..)` | 409 | `conflict` | 状态冲突 / 并发冲突（永久冲突） |
| `conflict_retryable(..)` | 409 | `conflict_retryable` | 可等待的冲突：冲突方完成后重试可能成功 |
| `payload_too_large(..)` | 413 | `payload_too_large` | 请求体或上传文件超限 |
| `unsupported_media_type(..)` | 415 | `unsupported_media_type` | 请求内容类型不受支持 |
| `too_many_requests(secs, ..)` | 429 | `too_many_requests` | 被限流；同时写出 `Retry-After`（秒） |
| `unavailable(..)` | 503 | `service_unavailable` | 上游不可用；可重试 |
| `internal(..)` / `di(..)` | 500 | `internal_error` / `dependency_injection` | 服务端缺陷 / DI 解析失败；detail 不回显 |
| `status(code, ..)` | `code` | 由状态码推导 | 显式指定状态码 |
| `ProblemDetails::validation(..).to_error()` | `problem.status` | `validation_failed` | 字段级校验/绑定失败，携带 `errors` |

## 机器可读错误码

`problem+json` 在 RFC 9457 标准字段之外，固定携带：

| 成员 | 类型 | 含义 |
|---|---|---|
| `code` | string | 稳定机器码，客户端按它分支，不要解析 `detail` |
| `retryable` | bool | 同一请求重试是否可能成功（429/502/503/504 与 `conflict_retryable` 为 true） |
| `instance` | string | 请求路径（`problem.instance` 为空时自动填充） |
| `traceId` | string | 与响应头 `x-trace-id` 同一个值（扩展成员） |

主要错误码：`bad_request`、`unauthorized`、`forbidden`、`not_found`、`method_not_allowed`、
`conflict`、`conflict_retryable`、`payload_too_large`、`unsupported_media_type`、
`unprocessable_entity`、`too_many_requests`、`validation_failed`、`invalid_json`、
`internal_error`、`service_unavailable`、`bad_gateway`、`gateway_timeout`。端点可通过
`ProblemDetails::new(..).with_code("...")` 自定义。

限流用 `Error::too_many_requests(retry_after_secs, detail)`：引擎写出 `Retry-After` 响应头
（只写一次），不要手写 `Error::status(429, …)` 再自己拼头；`status(429)` 的既有行为保留
（不带头）。可等待的冲突用 `Error::conflict_retryable(detail)`：`409` +
`retryable: true`，表示等冲突方完成后重试，**不是**自动盲目重试；永久冲突仍用
`Error::conflict`（`retryable: false`）。

## 默认错误响应格式

```json
{
  "type": "https://httpstatuses.com/404",
  "title": "Not Found",
  "status": 404,
  "detail": "User abc not found",
  "instance": "/users/abc",
  "code": "not_found",
  "retryable": false,
  "traceId": "4f9c2a1e8b7d4c60"
}
```

错误响应统一为 RFC 9457 `application/problem+json`（取代 RFC 7807）。Handler 返回 `Err` 后，
由 host 的请求处理循环捕获并写出 ProblemDetails，无需在 Handler 中手动设置状态码。
5xx 的 `detail` 固定为通用文案，原始错误只进日志（不向客户端泄漏内部信息）。

## Handler 中的错误处理

```rust
async fn handle(&mut self, req: GetUserRequest) -> ActionResult<UserDto> {
    // 资源不存在
    let user = self.repo.find(&req.id)
        .ok_or_else(|| Error::not_found(format!("User {} not found", req.id)))?;

    // 业务校验
    if user.is_banned {
        return Err(Error::forbidden("Account is banned"));
    }

    Ok(user)
}
```

### ? 运算符

```rust
let data = self.external_api.fetch().await
    .map_err(|e| Error::internal(e.to_string()))?;
```

## ProblemDetails（RFC 9457）

框架支持标准化问题详情；同一字段的多条错误会合并进 `errors` 数组：

```rust
use webx::{FieldError, ProblemDetails};

return Err(ProblemDetails::validation(vec![
    FieldError::new("email", "Invalid format"),
    FieldError::new("email", "Already registered"),
])
.to_error());
```

响应：

```json
{
  "title": "Validation Failed",
  "status": 400,
  "detail": "One or more validation errors occurred",
  "errors": { "email": ["Invalid format", "Already registered"] }
}
```

`ProblemDetails` 还提供 `not_found(...)`、`with_detail(...)`、`with_instance(...)` 与 `to_error()`。
参数绑定失败产生的也是携带 `errors` 的 `AppError`（`ProblemDetails::validation(..).to_error()`），
因此客户端拿到的 `errors` 结构与手动校验完全一致
（见[参数绑定](parameter-binding.md#错误响应)）。

## 持久层错误（rust-ef）

`rust-ef` 是 Web 无关的 ORM（2.9.1 起没有 `webx` feature），并且自己实现了
`From<EFError> for rust_core::AppError` 的脱敏映射。**不需要任何框架侧适配 crate**：
`rust-webx-ef`（`webx_ef`）连同 `map_ef_error` / `EfResultExt` 已在 2.9.1 删除
（见 [upgrade-to-2.9.1.md](../16-migration/upgrade-to-2.9.1.md)），应用在 Handler 里直接用 `?`：

```rust
// Handler：`?` 即完成 EFError → webx::Error 映射，不需要应用侧映射文件
let blogs = self.ctx.set::<Blog>().query().to_list().await?;
```

`NotFound`/`ModelValidation`/`ConcurrencyConflict` 保留业务文案；`Query`/`TypeConversion`
是固定文案的 400，`UniqueViolation` 不回显 constraint/table，`Database` 的瞬态条件
（`SQLSTATE` 08/40/57P0*、SQLite busy/locked、MySQL 1205/1213）与 `Connection`/`Transaction`
一样是可重试 503，其余只在日志展开、客户端拿 500 且无 detail。
运行期 DI 解析失败同样用 `?`（`RdiError` → `Error::di`，500、detail 不回显），不需要本地助手。
完整映射表见 [兼容性政策 §4](../../design/compatibility.md#4-边界约定rust-ef-错误映射)。

## 错误处理最佳实践

| 实践 | 说明 |
|------|------|
| 可预期错误用 `Error` 构造器 | 不用 panic |
| 404 用 `Error::not_found` | 不要用 `internal` 代替 |
| 校验失败用 `Error::validation` | 与 `invalid_json` 区分 |
| 认证失败用 `Error::unauthorized`，权限不足用 `Error::forbidden` | 或依赖中间件统一处理 |
| 不要泄露内部细节 | `internal` 的消息只进 `operator()` 日志，不出现在客户端 |

## 小结

Handler 返回 `ActionResult<T>`：`Ok(T)` 就是响应数据，`Err(Error)` 由框架映射为 HTTP 状态码与
`problem+json`。需要结构化错误时使用 `ProblemDetails`。

下一章：[DI 与生命周期](../06-di-lifecycle/INDEX.md)

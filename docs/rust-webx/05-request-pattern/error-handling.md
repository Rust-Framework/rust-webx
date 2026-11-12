# 错误处理与 ProblemDetails

## Error 类型

框架统一错误类型 `webx::Error`：

```rust
pub enum Error {
    Http(String),
    Status(u16, String),
    Unauthorized(String),
    Forbidden(String),
    Di(String),
    Routing(String),
    Serialization(serde_json::Error),
    Internal(String),
    Message(String),
    Validation(String),
    NotFound(String),
    Conflict(String),
    PayloadTooLarge(String),
    UnsupportedMediaType(String),
    Problem(Box<ProblemDetails>),   // 绑定/校验失败，原样写出
}
```

## HTTP 状态码映射

| Error 变体 | HTTP 状态码 | 典型场景 |
|-----------|-----------|---------|
| `Http` | 400 | 通用 HTTP 错误 |
| `Status(code, _)` | `code` | 显式指定状态码 |
| `Unauthorized` | 401 | 认证失败、缺少令牌 |
| `Forbidden` | 403 | 权限不足 |
| `Di` | 500 | DI 解析失败 |
| `Routing` | 404 | 路由错误 |
| `Serialization` | 400 | JSON 解析错误 |
| `Internal` | 500 | 未预期内部错误 |
| `Message` | 500 | 通用错误 |
| `Validation` | 400 | 业务校验失败 |
| `NotFound` | 404 | 资源不存在 |
| `Conflict` | 409 | 状态冲突 / 并发冲突 |
| `PayloadTooLarge` | 413 | 请求体或上传文件超限 |
| `UnsupportedMediaType` | 415 | 请求内容类型不受支持 |
| `Problem(details)` | `details.status` | 字段级校验/绑定失败，携带 `errors` |

## 默认错误响应格式

```json
{
  "type": "https://httpstatuses.com/404",
  "title": "Not Found",
  "status": 404,
  "detail": "User abc not found"
}
```

错误响应统一为 RFC 7807 `application/problem+json`。Handler 返回 `Err` 后，由 host 的请求处理循环
捕获并写出 ProblemDetails，无需在 Handler 中手动设置状态码。

## Handler 中的错误处理

```rust
async fn handle(&mut self, req: GetUserRequest) -> ActionResult<UserDto> {
    // 资源不存在
    let user = self.repo.find(&req.id)
        .ok_or_else(|| Error::NotFound(format!("User {} not found", req.id)))?;

    // 业务校验
    if user.is_banned {
        return Err(Error::Forbidden("Account is banned".into()));
    }

    Ok(user)
}
```

### ? 运算符

```rust
let data = self.external_api.fetch().await
    .map_err(|e| Error::Internal(e.to_string()))?;
```

## ProblemDetails（RFC 7807）

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
参数绑定失败产生的也是 `Error::Problem`，因此客户端拿到的 `errors` 结构与手动校验完全一致
（见[参数绑定](parameter-binding.md#错误响应)）。

## 错误处理最佳实践

| 实践 | 说明 |
|------|------|
| 可预期错误用 Error 变体 | 不用 panic |
| 404 用 NotFound | 不要用 Internal 代替 |
| 校验失败用 Validation | 与序列化错误区分 |
| 认证失败用 Unauthorized，权限不足用 Forbidden | 或依赖中间件统一处理 |
| 不要泄露内部细节 | Internal 消息不暴露堆栈给客户端 |

## 小结

Handler 返回 `ActionResult<T>`：`Ok(T)` 就是响应数据，`Err(Error)` 由框架映射为 HTTP 状态码与
`problem+json`。需要结构化错误时使用 `ProblemDetails`。

下一章：[DI 与生命周期](../06-di-lifecycle/INDEX.md)

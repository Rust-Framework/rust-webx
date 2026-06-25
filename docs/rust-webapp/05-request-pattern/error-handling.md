# 错误处理与 ProblemDetails

## Error 类型

框架统一错误类型 `rust_webapp::Error`：

```rust
pub enum Error {
    NotFound(String),
    Validation(String),
    Serialization(serde_json::Error),
    Http(String),
    Di(String),
    Internal(String),
    Message(String),
    Routing(String),
}
```

## HTTP 状态码映射

| Error 变体 | HTTP 状态码 | 典型场景 |
|-----------|-----------|---------|
| `NotFound` | 404 | 资源不存在 |
| `Validation` | 400 | 业务校验失败 |
| `Serialization` | 400 | JSON 解析错误 |
| `Http` | 400/401/403 | 认证失败、权限不足 |
| `Di` | 500 | DI 解析失败 |
| `Internal` | 500 | 未预期内部错误 |
| `Message` | 500 | 通用错误 |
| `Routing` | 404 | 路由错误 |

## 默认错误响应格式

```json
{
  "error": "User abc not found",
  "status": 404
}
```

由内置异常中间件自动生成，无需在 Handler 中手动设置状态码。

## Handler 中的错误处理

```rust
async fn handle(&self, req: GetUserRequest) -> Result<UserDto> {
    // 资源不存在
    let user = self.repo.find(&req.id)
        .ok_or_else(|| Error::NotFound(format!("User {} not found", req.id)))?;

    // 业务校验
    if user.is_banned {
        return Err(Error::Http("Account is banned".into()));
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

框架支持标准化问题详情：

```rust
use rust_webapp::{ProblemDetails, FieldError};

let problem = ProblemDetails::validation_error()
    .with_title("Validation failed")
    .with_detail("One or more fields are invalid")
    .with_field_errors(vec![
        FieldError { field: "email".into(), message: "Invalid format".into() },
    ]);
```

适用于需要结构化错误信息的公共 API。

## 错误处理最佳实践

| 实践 | 说明 |
|------|------|
| 可预期错误用 Error 变体 | 不用 panic |
| 404 用 NotFound | 不要用 Internal 代替 |
| 校验失败用 Validation | 与序列化错误区分 |
| 认证/授权用 Http | 或依赖中间件统一处理 |
| 不要泄露内部细节 | Internal 消息不暴露堆栈给客户端 |

## 小结

返回 `Result<T>` 即可，框架负责 Error → HTTP 的映射。需要结构化错误时使用 `ProblemDetails`。

下一章：[DI 与生命周期](../06-di-lifecycle/INDEX.md)

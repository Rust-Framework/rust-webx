# 参数绑定与序列化

## 参数来源

| 来源 | Request 字段要求 | 示例 |
|------|----------------|------|
| 路径参数 | 字段名与 `{param}` 一致 | `id: String` ← `{id}` |
| JSON Body | `#[derive(Deserialize)]` | POST/PUT 请求体 |
| Query String | 字段名与 query key 一致 | `?page=1&size=10` |
| 通配符 | `{*name}` 捕获剩余路径 | 文档内容 API |

## 路径参数

```rust
struct GetUserRequest {
    pub id: String,
}

#[get("/api/users/{id}")]
impl IRequest<UserDto> for GetUserRequest {}
```

路由匹配 `/api/users/abc-123` 后，`id = "abc-123"`。

## JSON Body

```rust
#[derive(Deserialize)]
pub struct CreateUserRequest {
    pub name: String,
    pub email: String,
}

#[post("/api/users")]
impl IRequest<UserDto> for CreateUserRequest {}
```

框架从 `HttpContext` 读取 body bytes 并反序列化为 Request struct。

### 反序列化失败

自动返回 `Error::Serialization`，映射 HTTP 400。

## 响应序列化

Handler 返回的类型必须实现 `Serialize`：

```rust
#[derive(Serialize)]
pub struct UserDto {
    pub id: String,
    pub name: String,
}
```

框架自动设置 `Content-Type: application/json` 并写入响应体。

### 返回原始 JSON 值

```rust
use rust_webx::Json;

async fn handle(&self, req: MyRequest) -> Result<Json<serde_json::Value>> {
    Ok(Json(serde_json::json!({ "custom": true })))
}
```

## 参数绑定宏（预留）

以下宏当前作为**元数据**标注，完整自动绑定在后续版本实现：

```rust
#[derive(Deserialize)]
pub struct SearchRequest {
    #[from_query]
    pub q: String,
    #[from_route]
    pub category: String,
    #[from_body]
    pub filters: FilterDto,
}
```

当前版本需手动在 Handler 中从 `IHttpContext` 读取，或依赖框架的自动字段填充（路径参数 + body 反序列化已内置）。

## 分页

框架提供内置分页类型：

```rust
use rust_webx::{PagedRequest, PagedResponse};

#[derive(Deserialize)]
pub struct ListUsersRequest {
    #[serde(flatten)]
    pub paging: PagedRequest,
}

#[get("/api/users")]
impl IRequest<PagedResponse<UserDto>> for ListUsersRequest {}
```

## 小结

路径参数靠字段名匹配，Body 靠 Deserialize，响应靠 Serialize。保持 Request struct 简洁是 API 设计的关键。

下一节：[错误处理与 ProblemDetails](error-handling.md)

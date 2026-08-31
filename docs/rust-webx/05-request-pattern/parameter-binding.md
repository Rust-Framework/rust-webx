# 参数绑定与序列化

## 参数来源

| 来源 | Request 字段要求 | 状态 |
|------|----------------|------|
| 路径参数 | 字段名与 `{param}` 一致 | ✅ 已实现 |
| JSON Body | `#[derive(Deserialize)]` | ✅ 已实现 |
| Query String | 字段名与 query key 一致 | ✅ 已实现（GET/DELETE，需 `Deserialize`） |
| 通配符 | `{*name}` 捕获剩余路径 | ✅ 已实现 |

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

## Query String

URL 查询参数（如 `?page=1&size=10`）在 **GET/DELETE** 请求中自动绑定到 Request 字段（字段名与 query key 一致）。Request 类型需 `#[derive(Deserialize)]`；路径参数优先于同名 query 参数。

```rust
#[derive(Default, Deserialize)]
pub struct SearchRequest {
    pub q: String,
    pub page: String,
}

#[get("/api/search")]
impl IRequest<SearchResult> for SearchRequest {}
```

`?q=hello&page=2` → `q = "hello"`, `page = "2"`。

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

## 参数绑定宏与 OpenAPI

以下宏标注参数来源；query/path 绑定在 dispatch 层按字段名自动完成：

```rust
#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct SearchRequest {
    #[from_query]
    pub q: String,
    #[from_route]
    pub category: String,
}
```

`#[derive(WebxRequestMeta)]` 将字段元数据注册到 OpenAPI 生成器。也可用 `#[webx_request(query_all)]` 将未标注字段视为 query 参数（排除 `claims` 与 `#[serde(skip)]`）。

`#[from_query]` / `#[from_route]` 不标注时，GET 请求的 query 绑定仍按字段名自动完成，但 OpenAPI 需 `WebxRequestMeta` 才会列出 query 参数。

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

路径参数靠字段名匹配，Body 靠 Deserialize，Query 靠 Deserialize + 字段名匹配，响应靠 Serialize。

下一节：[错误处理与 ProblemDetails](error-handling.md)

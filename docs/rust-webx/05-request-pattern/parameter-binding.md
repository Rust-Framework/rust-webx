# 参数绑定与序列化

请求类型**必须**派生 `#[derive(WebxRequestMeta)]`，字段用 `#[from*]` 标记来源。派生宏在编译期生成
`RequestModel::bind`（运行时绑定）与 `param_meta()`（OpenAPI 元数据），**同一份声明**同时驱动两者，
不会再出现"文档写 query、运行时当 body"的分歧。

```rust
#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct UpdateUserRequest {
    #[frompath]
    pub id: String,                       // /api/users/{id}
    #[fromheader("X-Actor")]
    pub actor: String,                    // 请求头，大小写不敏感
    #[fromclaim("sub")]
    pub operator: String,                 // 认证主体的 sub
    pub name: Option<String>,             // JSON body
}
```

## 参数来源

| 标记 | 来源 | 说明 |
|------|------|------|
| `#[frompath]` / `#[frompath("id")]` | 路由段 `{name}` | 省略参数时用字段名；缺段即 400（`Option` 除外） |
| `#[fromquery]` / `#[fromquery("q")]` | Query String | 重复键与逗号列表都展开 |
| `#[frombody]` | JSON body | 不接收键名参数；键名由 `#[serde(rename)]` / `rename_all` 决定 |
| `#[fromform]` / `#[fromform("title")]` | multipart 文本分段 | 省略参数时用字段名 |
| `#[fromfile]` / `#[fromfile("avatar")]` | multipart 文件分段 | 只能用于 `FormFile` 系列类型 |
| `#[fromheader("X-Name")]` | 请求头 | 省略参数时按字段名转 `-` 小写 |
| `#[fromclaim("key")]` | 认证声明 | `sub`/`roles`/`permissions` 走专用槽位 |
| `#[fromclaim]` + `CurrentUser` | 认证主体 | 统一身份对象，见下文「用户身份」 |
| 无标记 | 按方法推断 | 见下一节 |

`FormFile`、`Option<FormFile>`、`Vec<FormFile>` 来自表单；未标注时按字段名取分段，标注 `#[fromfile]` 可改名。

### 未标记字段的推断顺序（由请求方法决定）

| 请求 | 顺序 | 依据 |
|------|------|------|
| `GET` / `HEAD` / `DELETE` / `OPTIONS` | **Query → 路由** | 先取同名 query；没有该键才取同名 `{name}` |
| `POST` / `PUT` / `PATCH` | **Body → Form → Query** | 先看 JSON body；body 没有该字段再回退到 query |
| `multipart/form-data` | **Form → Query** | 先看表单分段；没有该字段再回退到 query |

写方法优先使用请求体（JSON 或表单），缺失的字段再回退到 query；路径参数不参与回退，
需要时用 `#[frompath]` 显式声明。安全方法没有 body，先把 query 当作输入，再回退到路径。

`#[request(queryall)]` 把未标记字段强制为 Query。显式标记的字段**只**从所声明的来源取，
不参与上面的顺序，也不会回退。

**复杂类型**指没有字符串形式的类型：`ComponentModel`、`Option<ComponentModel>`、
`Vec<ComponentModel>`、`#[serde(flatten)]` 字段。它们未标记时一律从 body 绑定。
自定义标量（`Uuid`、枚举）需要实现 `FromStr` 并**显式标注**来源，才能从 query/path/header/claim 绑定。

## 类型

- 标量：`String`、整数、浮点、`bool`、以及任何实现 `FromStr` 的类型（`Uuid`、枚举等）。
  未标记时只有内置标量参与 Query/路由推断；自定义标量要显式标注来源。
- `Option<T>`：缺失为 `None`；**存在但解析失败是 400**，不会静默成 `None`。
- `Vec<T>`：Query 用重复键（`?id=1&id=2`）或逗号分隔（`?id=1,2`）；缺失为空列表；
  单个元素非法即 400。
- `FormFile` 系列：见[文件上传与下载](file-upload-download.md)。

```rust
#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct SearchRequest {
    pub q: String,          // ?q=hello
    pub page: u32,          // ?page=2
    pub ids: Vec<u32>,      // ?ids=1&ids=2 或 ?ids=1,2
    pub size: Option<u32>,  // ?size=20，可省略
}
```

### 必填与可选

| 字段形态 | 语义 |
|----------|------|
| `T` | 必填：缺失或无法解析 → 400，`errors` 里出现该字段 |
| `Option<T>` | 可选：缺失 → `None`；**存在但非法 → 400**，不会静默成 `None` |
| `Vec<T>` | 可选：缺失 → 空列表；单个元素非法 → 400 |
| `Option<Vec<T>>` | 可选：缺失 → `None`；存在（哪怕是空数组）→ `Some(list)` |
| `FormFile` | 必填，缺失 → 400 |
| `Option<FormFile>` | 可选 |
| `Vec<FormFile>` | 可选，缺失 → 空列表 |
| body 字段 | 非 `Option` 且没有 `#[serde(default)]` 即必填；列表缺失视为空列表 |
| `#[serde(skip)]`、`claims` | 不参与绑定，用 `Default::default()` 填充 |

必填 `#[fromclaim]` 字段在请求**未认证**时返回 401；已认证但声明缺失时按普通必填字段返回 400。
必填的复合类型字段（如 `Address`）在无 body 的方法上返回 400，并提示改用 POST/PUT/PATCH 或显式标注来源。

`#[serde(default)]` 在该字段的**所有来源都没有值**时生效（body、query、路由、表单都取不到才用默认值）；
`#[serde(rename)]` / `rename_all` / `alias` 决定 body 的 JSON 键；`#[serde(flatten)]` 字段没有独立键，
只从 body 绑定。`#[serde(deny_unknown_fields)]` 在 body 上生效（存在 flatten 字段时无法判定）。
body 字段类型错误、`null` 赋给非 `Option` 字段都按该字段返回 400，不会被当作“缺失”回退到 query。

## 用户身份：`CurrentUser`

需要登录的接口不用再手动读 claims、拼 `sub` 和 `tenant`：把身份声明成普通字段，绑定器会从认证主体填充。

```rust
#[derive(WebxRequestMeta)]
pub struct CreateOrderRequest {
    #[fromclaim]
    pub user: CurrentUser,     // 未登录 → 401
    pub sku: String,           // JSON body
    pub quantity: u32,
}
```

Handler 直接使用，不接触 token、`IHttpContext` 或 claims map：

```rust
async fn handle(&mut self, req: CreateOrderRequest) -> Result<OrderView> {
    let user = req.require_user()?;          // &CurrentUser
    self.orders
        .create(CreateOrder {
            tenant_id: user.tenant_id.clone(),
            created_by: user.id.clone(),
            sku: req.sku,
            quantity: req.quantity,
        })
        .await
}
```

| 字段 | 来源 claim | 说明 |
|------|-----------|------|
| `id` | `sub` | 用户唯一标识 |
| `name` | `name` | 显示名，缺失时为空串 |
| `email` | `email` | 邮箱，缺失时为空串 |
| `tenant_id` | `tenant_id` / `tenant` | 租户标识，单租户应用为空串 |
| `roles` | `roles` | 角色数组 |
| `permissions` | `permissions` | 权限数组 |

辅助方法：`is_in_role("admin")`、`has_permission("order:write")`、`display_name()`。

- `Option<CurrentUser>`：匿名可访问的端点；未登录为 `None`，不会 401。
- 派生宏在结构体含该字段时自动实现 `RequestUser`，可直接 `req.user_id()`、`req.user_name()`、
  `req.tenant_id()`、`req.is_in_role("admin")`、`req.require_user()?`。
- 身份字段不进入 OpenAPI 参数表，避免把认证信息暴露成普通参数。
- 一个请求只应有一个 `CurrentUser` 字段；身份是单数。

**省掉了什么**：以前每个需要身份的 handler 都要注入 `IHttpContext`、取 `ctx.claims()`、逐个解析
`subject()`/`tenant`/`roles`，并在每个 handler 重复一遍空值与类型处理；现在这些集中在绑定层，
业务代码只写 `req.user`。

## 优先级

- **写方法**：Body → Form → Query；body/表单里有该字段就优先使用，缺失才回退到 query。
- **安全方法**：Query 优先于路由；同名时 query 的值生效。
- **显式标记**：该字段的唯一来源，读取失败即 400，绝不改用其它来源。
- `#[fromheader]` / `#[fromclaim]`：从不参与隐式推断，避免请求头或令牌里的同名键悄悄覆盖参数。
- 路径参数不参与写方法的隐式回退，需要时显式 `#[frompath]`。

## 错误响应

绑定失败一次性汇总，返回 `400` + `application/problem+json`：

```json
{
  "title": "Validation Failed",
  "status": 400,
  "detail": "One or more validation errors occurred",
  "errors": {
    "page": ["The `page` field must be a valid integer (got `abc`)."],
    "name": ["The `name` field is required."]
  }
}
```

- JSON body 的字段级错误按字段名归入 `errors`；整体 body 解析失败记在 `errors[""]` 下。
- `errors` 的键始终是**字段名**（请求头/声明字段也一样），来源键只出现在 OpenAPI 元数据里。
- body 字段类型错误会立即以该字段的 400 返回（无法为该类型构造占位值）；query/header 等字符串来源的错误会一次性汇总。
- 有**必填** `#[fromclaim]` 字段而请求未认证 → `401`。
- 表单端点收到非 `multipart/form-data` → `415`；JSON 端点收到 multipart 同理。
- 绑定阶段不再有"静默 `Default`"：字段缺失或非法一定会出现在 `errors` 里。

## OpenAPI

`param_meta()` 给出每个字段的来源与类型，路由宏再补上模板里的路径参数（合并时以结构体的类型为准）。
`#[fromclaim]` 属于认证信息，不写进文档；未标记字段按路由方法解析成 body 或 query；
`FormFile` 生成 `multipart/form-data` 的 `string/binary`。

## 响应序列化

Handler 返回的类型必须实现 `Serialize`，框架自动设置 `Content-Type: application/json`。

```rust
#[derive(Serialize)]
pub struct UserDto {
    pub id: String,
}

// 需要完全控制状态码/响应头/流式响应时返回 ResponseData
async fn handle(&mut self, req: MyRequest) -> Result<ResponseData> { ... }
```

下一节：[错误处理与 ProblemDetails](error-handling.md)

# 参数绑定与序列化

请求类型**必须**派生 `#[derive(WebxRequestMeta)]`，字段用 `#[from*]` 标记来源。派生宏在编译期生成
`RequestModel::bind`（运行时绑定）与 `param_meta()`（OpenAPI 元数据），**同一份声明**同时驱动两者，
包括请求体字段级的 OpenAPI schema；不会再出现"文档写 query、运行时当 body"的分歧。

```rust
#[derive(WebxRequestMeta)]
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
| `#[fromclaim]` + `IdentityUser` | 认证主体 | 统一身份对象，见下文「用户身份」 |
| `#[fromclient]` + `ClientInfo` | 可信客户端信息 | 对端地址 + 受信代理链；见下文「客户端信息」 |
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

业务约束（长度、范围、邮箱、自定义规则）用 `#[validate(...)]` 声明，和绑定错误共用同一个
`400` `errors` 信封，并同步进 OpenAPI schema，见[声明式校验](validation.md)。

```rust
#[derive(WebxRequestMeta)]
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
| `#[serde(skip)]` | 不参与绑定，用 `Default::default()` 填充 |

必填 `#[fromclaim]` 字段在请求**未认证**时返回 401；已认证但声明缺失时按普通必填字段返回 400。
复合类型字段（如 `Address`、含 `Option<Address>`）只能从 JSON body 绑定：在无 body 的方法（GET/HEAD/DELETE/OPTIONS）上返回 400，并提示改用 POST/PUT/PATCH，或显式标注 `#[fromquery]`/`#[frompath]` 并为类型实现 `FromStr`。
`Option<Vec<T>>` 的“存在但为空”可表达：`?ids=`（键存在、值为空）绑定为 `Some(vec![])`，键缺失才是 `None`。

`#[serde(default)]` 在该字段的**所有来源都没有值**时生效（body、query、路由、表单都取不到才用默认值）；
`#[serde(rename)]` / `rename_all` / `alias` 决定 body 的 JSON 键；`#[serde(flatten)]` 字段没有独立键，
只从 body 绑定。`#[serde(deny_unknown_fields)]` 在 body 上生效（存在 flatten 字段时无法判定）。
body 字段类型错误、`null` 赋给非 `Option` 字段都按该字段返回 400，不会被当作“缺失”回退到 query。

`serde` 已注册为 `WebxRequestMeta` 的 helper attribute：请求结构体**不需要**为了默认值额外
`#[derive(Deserialize)]`，只 derive `WebxRequestMeta` 就能让查询绑定使用默认值（2.9.1 的
`PagedRequest` 已删除 `from_parts`，见 [upgrade-to-2.9.1.md](../16-migration/upgrade-to-2.9.1.md)：
缺省值与范围校验由 binding 层的 `RequestModel` 实现负责）：

```rust
fn twenty() -> u32 { 20 }

#[derive(WebxRequestMeta)]
pub struct ListOrdersRequest {
    #[serde(default = "twenty")]
    pub page_size: u32,
    #[serde(default)]
    pub filter: String,
}
```


## 用户身份

需要登录的接口把身份声明成普通字段，绑定层负责 `401` 与字段填充：

```rust
#[derive(WebxRequestMeta)]
pub struct CreateOrderRequest {
    #[fromclaim]
    pub user: IdentityUser,   // 未登录 → 401
    pub sku: String,         // JSON body
}
```

Handler 里直接用 `req.user.id`、`req.user.tenant_id`、`req.user.is_in_role("admin")`，
或 `req.require_user()?`。匿名可访问的端点用 `#[fromclaim(optional)] viewer: Option<IdentityUser>`（只写 `#[fromclaim]` 会编译失败）。
完整场景与收益对照见[用户身份与多租户](user-identity.md)。

## 客户端信息

需要在 Handler 内做登录 IP 维度限流、审计或风控时，把 `ClientInfo` 声明成请求字段：

```rust
#[derive(WebxRequestMeta)]
pub struct LoginRequest {
    #[fromclient]
    pub client: ClientInfo,   // 永远可用，无 Option
    pub username: String,
    pub password: String,
}
```

| 字段 | 含义 |
|---|---|
| `peer_addr` | 监听器看到的 TCP 对端（生产通常是反向代理） |
| `client_ip` | 业务应使用的客户端地址：对端，或受信代理后的真实客户端 |
| `forwarded_for` | 解析后的转发链（左→右）；**未启用信任时为空**，绝不把客户端可伪造的头当可信值 |

转发头只在对端属于配置的可信代理时才采信（`ClientInfo` 配置节）：

```json
{
  "ClientInfo": {
    "TrustForwardedHeaders": true,
    "TrustedProxies": ["10.0.0.0/8"]
  }
}
```

`client_ip` 取 `X-Forwarded-For` 中**最右侧的非可信代理** hop（客户端在左侧塞入伪造地址不会生效）；
没有可信转发时它就是对端地址，`forwarded_for` 为空。默认关闭：直连客户端可以随意伪造
`X-Forwarded-For`，只有明确知道代理网段时才开启。

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
- 绑定阶段没有「静默 `Default`」：字段缺失或非法一定会出现在 `errors` 里。

## OpenAPI

`param_meta()` 给出每个字段的来源、类型与 JSON 键，路由宏再补上模板里的路径参数（合并时以结构体的类型为准）。
`#[fromclaim]` 属于认证信息、`#[fromclient]` 属于连接信息，都不写进文档；未标记字段按路由方法解析成 body 或 query。body 字段
**逐个**进入请求体 schema，不再只输出一个 `{"type": "object"}` 占位：

- **写方法（POST/PUT/PATCH）**：`requestBody.content["application/json"].schema` 的
  `properties` 键是字段的 **JSON 键**（`#[serde(rename)]` / `rename_all` 与 `r#type` 的 wire 名），
  类型按字段形状映射为 `string` / `integer` / `number` / `boolean` / `array` / `object`；
  `required` 列出必填字段（`Option<T>`、`Vec<T>`、`#[serde(default)]` 不算必填）。
  `requestBody.required` 由「是否存在必填 body 字段」推导，而不是恒为 `true`。
- `#[serde(flatten)]` 字段没有独立 JSON 键，无法枚举内部属性：schema 保留
  `type: "object"` 并加 `additionalProperties: true` 表示未知键；该字段的必填性仍计入
  `requestBody.required`。
- **无 body 的方法（GET/HEAD/DELETE/OPTIONS）**：不输出 `requestBody`。这些方法上的 body
  字段在运行时同样无法绑定（复合类型/必填值会 400），应改用显式 `#[fromquery]` /
  `#[frompath]`，或把端点改成写方法。
- **表单端点**：`FormFile` 生成 `multipart/form-data` 的 `string/binary`（见
  [文件上传与下载](file-upload-download.md#16-openapi-里的上传端点)），同一端点不会同时
  输出 JSON body。

示例（`PUT /api/blog/{id}` 的请求结构体带 `title: String`、`published: Option<bool>`、
`tags: Vec<String>`）会生成：

```json
{
  "requestBody": {
    "required": true,
    "content": {
      "application/json": {
        "schema": {
          "type": "object",
          "properties": {
            "title": { "type": "string" },
            "published": { "type": "boolean" },
            "tags": { "type": "array" }
          },
          "required": ["title"]
        }
      }
    }
  }
}
```

## 响应序列化

Handler 返回的类型必须实现 `Serialize`，框架自动设置 `Content-Type: application/json`。

```rust
#[derive(Serialize)]
pub struct UserDto {
    pub id: String,
}

// 需要完全控制状态码/响应头/流式响应时，响应类型写 Response，返回 IActionResult
async fn handle(&mut self, req: MyRequest) -> ActionResult<Response> { ... }
```

下一节：[错误处理与 ProblemDetails](error-handling.md)

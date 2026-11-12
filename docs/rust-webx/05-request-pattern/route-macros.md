# 路由宏详解

## 基本用法

路由宏标注在 `impl IRequest<T>` 块上。每个 `impl` 对应的请求结构体都必须派生
`WebxRequestMeta`（绑定与 OpenAPI 元数据都由它生成）；下面的路由片段省略了结构体定义：

```rust
#[get("/api/users")]
impl IRequest<Vec<UserDto>> for ListUsersRequest {}

#[get("/api/users/{id}")]
impl IRequest<UserDto> for GetUserRequest {}

#[post("/api/users")]
impl IRequest<UserDto> for CreateUserRequest {}

#[put("/api/users/{id}")]
impl IRequest<UserDto> for UpdateUserRequest {}

#[delete("/api/users/{id}")]
impl IRequest<()> for DeleteUserRequest {}
```

## 可用宏

| 宏 | HTTP 方法 |
|----|----------|
| `#[get("/path")]` | GET |
| `#[post("/path")]` | POST |
| `#[put("/path")]` | PUT |
| `#[delete("/path")]` | DELETE |

另有元数据宏（不改变 HTTP 方法）：

| 宏 | 作用 |
|----|------|
| `#[display(...)]` | 端点名称（资源键/字面量） |
| `#[description(...)]` | 端点长描述 |
| `#[category(...)]` | 可选分组标签 |
| `#[authorize(...)]` | 授权规则（详见[授权宏](#授权宏)） |

## 端点名称与描述：资源键

端点文本使用**资源引用**（可带字面量兜底），翻译放在资源文件里；语言由全局 `Culture.Default` 决定：

1. `#[display]` / `#[description]`（资源键，可带字面量兜底）；
2. `impl` 块上的 `///` 文档注释；
3. 名称再回退到请求类型名（`CreateBlogPostRequest` → `create blog post`）。

```rust
#[post("/api/blog")]
#[display(key = "endpoint.blog.create.name", text = "博客模块-创建博客")]
#[description(key = "endpoint.blog.create.description", text = "创建一个博客草稿。")]
#[category("博客模块")]
#[authorize(permission = "blog:create")]
impl IRequest<BlogPostModel> for CreateBlogPostRequest {}
```

- `#[display("字面量")]`：固定名称，不查资源；
- `#[display(key = "…")]`：资源键；资源缺失时回退为 key 本身（与 `IStringLocalizer` 一致）；
- `#[display(key = "…", text = "…")]`：资源键，缺失时回退字面量；
- 没写 `#[display]` 时回退 `///` 第一行，再回退类型名；`#[description]` 没写时使用 `///` 全文；
- `#[category("…")]`（可选）：分组标签，写法与 `#[display]` 相同（字面量、`key`、`key + text`）。
  它只用于展示分组（权限树、管理导航、OpenAPI tag），不参与路由匹配和授权；
- `#[display]` / `#[description]` / `#[category]` 写在路由宏之前或之后都可以；`///` 也必须写在
  `impl` 块上（写在请求 struct 上不会被读取）。

资源文件按 culture 一份，放在应用基准目录的 `locales/` 下（`locales/zh-CN.json`、
`locales/en.json`）：

```json
// locales/zh-CN.json
{
  "endpoint.blog.create.name": "博客模块-创建博客",
  "endpoint.blog.create.description": "创建一个博客草稿。"
}
```

默认约定只需一行：

```rust
Host::builder().add_localization().build();
```

`add_localization()` 读取 `locales/<culture>.json`；缺少该目录不算错误。单文件部署可以把资源
编译进二进制：

```rust
Host::builder()
    .add_localization_with(
        LocalizationOptions::none()
            .resources("zh-CN", include_str!("../locales/zh-CN.json"))
            .resources("en", include_str!("../locales/en.json")),
    )
    .build();
```

> 显式 `resources` 覆盖磁盘上同名 culture/key 的条目；用 `LocalizationOptions::new()` 可同时读取
> 约定的 `locales/` 目录，`none()` 表示只使用编译期资源。

`Culture.Default`（appsettings 的 `Culture` 节，或 `APP__Culture__Default`）决定语言。
查找顺序：精确 culture → 父 culture（`zh-Hans-CN` → `zh-Hans` → `zh`）→ 默认 culture →
字面量 → key。

业务代码可注入 `Arc<dyn IStringLocalizer>`：`get(key)` 使用 Host 默认 culture，
`get_in(culture, key)` 显式切换；资源缺失时返回 `LocalizedString { resource_not_found: true }`。

> 没有调用 `add_localization()` 时资源表为空，`key` 会回退为 key 本身；为可读性建议同时提供
> `text` 字面量。`Culture.Default` 对应的资源文件应包含该语言的翻译。

## 运行时读取端点目录

路由在编译期由 `inventory` 收集，`Host::build()` 只读一次，构建本 Host 的 `EndpointCatalog`
并注册进 DI。运行时通过 Host 或 DI 获取：

```rust
let catalog = host.endpoints(); // &dyn IEndpointCatalog
for endpoint in catalog.all() {
    println!("{} {} — {}", endpoint.method, endpoint.path, endpoint.name.key);
}

// 按全局 Culture.Default 解析名称/描述，可直接序列化给权限系统
let endpoints = catalog.localized(host.localizer());
let manifest = serde_json::to_string(&endpoints)?;

// 需要另一种语言的视图时显式指定 culture
let english = catalog.localized_for(host.localizer(), "en");
```

`Host::endpoints()` 返回 `&dyn IEndpointCatalog`；`EndpointCatalog` 是默认实现。通过 DI 注入时
使用 `Arc<dyn IEndpointCatalog>`。

Hosted service / handler 中从容器获取接口，不需要全局函数：

```rust
#[derive(Inject)]
struct PermissionSync {
    #[inject]
    catalog: Arc<dyn IEndpointCatalog>,
    #[inject]
    localizer: Arc<dyn IStringLocalizer>,
}
```

`IEndpointCatalog` 就是 ASP.NET Core `EndpointDataSource` 的对应物：`all()` 返回
`&[EndpointDescriptor]`，`get(method, path)` 按原始 method/path 精确查找。

### 在 handler 中返回所有路由

**方案 A：原始描述符（语言中立）**

只返回 `EndpointDescriptor`（`name` / `description` / `category` 是 `TextRef { key, text }`）：

```rust
#[derive(WebxRequestMeta)]
struct ListEndpointsRequest;

#[get("/api/endpoints")]
#[authorize(role = "admin")]
impl IRequest<Vec<EndpointDescriptor>> for ListEndpointsRequest {}

#[derive(Inject)]
struct ListEndpointsHandler {
    #[inject]
    catalog: Arc<dyn IEndpointCatalog>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListEndpointsRequest, Vec<EndpointDescriptor>> for ListEndpointsHandler {
    async fn handle(&mut self, _req: ListEndpointsRequest) -> ActionResult<Vec<EndpointDescriptor>> {
        Ok(self.catalog.all().to_vec())
    }
}
```

**方案 B：解析后的 `EndpointInfo`（管理界面展示）**

需要解析后的名称/描述/分类时，再注入 localizer：

```rust
#[derive(Inject)]
struct ListEndpointsHandler {
    #[inject]
    catalog: Arc<dyn IEndpointCatalog>,
    #[inject]
    localizer: Arc<dyn IStringLocalizer>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListEndpointsRequest, Vec<EndpointInfo>> for ListEndpointsHandler {
    async fn handle(&mut self, _req: ListEndpointsRequest) -> ActionResult<Vec<EndpointInfo>> {
        Ok(self.catalog.localized(&*self.localizer))
    }
}
```

`#[derive(Inject)]` 不是黑盒：它就是 ASP.NET Core 构造函数注入的等价物，生成大致如下：

```rust
// 示意
fn __rdi_try_construct_ListEndpointsHandler(
    resolver: &dyn IServiceResolver,
) -> Result<Arc<ListEndpointsHandler>, RdiError> {
    Ok(Arc::new(ListEndpointsHandler {
        catalog: resolver.get::<dyn IEndpointCatalog>()?,
    }))
}
```

Handler 内**没有 ambient DI 入口**：需要容器服务的 handler 用
**构造注入**——`#[handler(inject)]` + `#[derive(Inject)]` 的 `#[inject]` 字段（见上一节），或把依赖
作为参数传给用例函数。后台任务（hosted service）注入 `Arc<dyn ScopeFactory>` 后自行开 scope。

`IEndpointCatalog`、`EndpointDescriptor`、`EndpointInfo` 都由 `webx::*` 导出。`EndpointDescriptor`
也实现了 `Serialize`，因此原始列表可直接作为 JSON 返回：

```json
{
  "method": "POST",
  "path": "/api/blog",
  "name": { "key": "endpoint.blog.create.name", "text": "创建博客" },
  "description": { "key": "endpoint.blog.create.description", "text": "" },
  "category": { "key": "", "text": "博客模块" },
  "authorization": [
    { "roles": ["admin"], "permissions": [] },
    { "roles": [], "permissions": ["blog:create"] }
  ]
}
```

`catalog.localized(..)` 返回解析后的 `EndpointInfo`，按 `path`、`method` 排序，顺序稳定，可反复同步：

| 字段 | 含义 |
|------|------|
| `method` | 大写 HTTP 方法（`GET`/`POST`/…），与 `HttpMethod::as_str()` 及运行时鉴权收到的值一致 |
| `path` | 原始声明的路由模式，如 `/api/blog/{slug}` |
| `name` | 资源解析后的名称，回退到字面量或 key |
| `description` | 资源解析后的描述 |
| `category` | `#[category]` 解析后的分组标签；未设置时为空串 |
| `authorization` | `#[authorize]` 规则数组；每项 `{ roles, permissions }`，规则之间全部满足，空数组表示公开 |

`authorization` 的结构与运行时判定一致：每个 `#[authorize]` 是一条规则，所有规则都要满足；
规则内 `roles` / `permissions` 是 any-of 列表，两个维度同时存在时为 AND。权限系统可以原样读取
这个数组，不需要再解析宏文本：

```json
{
  "method": "POST",
  "path": "/api/blog",
  "name": "博客模块-创建博客",
  "description": "创建一个博客草稿。",
  "category": "博客模块",
  "authorization": [
    { "roles": ["admin"], "permissions": [] },
    { "roles": [], "permissions": ["blog:create"] }
  ]
}
```

用 `(method, path)` 作为权限/资源表的主键，把 `name` 作为权限或菜单名称，同步逻辑就不必再
手工维护一份 url 映射表。`path` 是路由模式而不是具体请求路径，因此同一路由在参数变化时
键保持稳定。

> 只有路由宏声明的端点会出现在目录中。Host 直接注册的基础设施端点（`/health`、`/metrics`、
> `/api/openapi.*`、SPA 回退）不属于应用权限，不在其中。

## 路径参数

`{name}` 语法定义动态段：

```rust
#[derive(WebxRequestMeta)]
struct GetUserRequest {
    pub id: String,       // 绑定 {id}
}

#[get("/api/users/{id}")]
impl IRequest<UserDto> for GetUserRequest {}
```

框架在路由匹配后，将提取的值填入同名字段。

### 通配符路径

```rust
#[derive(WebxRequestMeta)]
struct GetDocContentRequest {
    pub work: String,
    pub any: String,      // 绑定 {*any} 剩余路径
}

#[get("/api/docs/{work}/content/{*any}")]
impl IRequest<DocContent> for GetDocContentRequest {}
```

## 授权宏

```rust
// 需要认证（任意已登录用户）
#[get("/api/auth/me")]
#[authorize]
impl IRequest<UserView> for AuthMeRequest {}

// 需要 admin 角色
#[post("/api/users")]
#[authorize(role = "admin")]
impl IRequest<UserDto> for CreateUserRequest {}

// 任意一个角色即可
#[get("/api/reports")]
#[authorize(roles = ["admin", "auditor"])]
impl IRequest<Vec<Report>> for ListReportsRequest {}

// 需要特定 permission
#[put("/api/settings")]
#[authorize(permission = "settings:write")]
impl IRequest<SettingsDto> for UpdateSettingsRequest {}

// 两条规则都必须满足：admin 角色 且 具备导出权限
#[post("/api/reports/export")]
#[authorize(role = "admin")]
#[authorize(permission = "reports:export")]
impl IRequest<Response> for ExportReportsRequest {}
```

授权元数据在编译期收集为 `RouteEntry.authorization`（`&[AuthorizeRule]`）。每个 `#[authorize]` 是一条规则，所有规则全部满足才放行；规则内 `roles` / `permissions` 是 any-of 列表，两个维度同时存在时为 AND。`add_authentication()` 只负责启用 JWT；规则由 endpoint 在 dispatch 前执行。详见 [authorize 宏](../09-auth-security/authorize-macro.md)。

需要 DB/cache 驱动的路由授权时，注册 [IDynamicAuthorizer](../09-auth-security/authorize-macro.md#动态授权idynamicauthorizer)；它只在声明了 `#[authorize]` 的路由上运行。

## 常见错误

### ❌ 标注在 struct 上

```rust
#[get("/hello")]  // 错误！
struct HelloRequest;
```

### ✅ 标注在 impl 块上

```rust
struct HelloRequest;

#[get("/hello")]  // 正确
impl IRequest<String> for HelloRequest {}
```

### ❌ 路径不匹配

```rust
struct GetUserRequest { id: String }

#[get("/api/users")]  // 缺少 {id}，字段无法绑定
impl IRequest<UserDto> for GetUserRequest {}
```

## 小结

路由宏是「请求即端点」的语法糖。一行 `#[get]` 替代传统框架中路由表 + 处理器映射的两处定义。

下一节：[Handler 注册策略](handler-registration.md)

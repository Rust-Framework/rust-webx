# 升级到 0.6（破坏性变更）

0.6 把端点元数据、授权规则和本地化统一到一份编译期元数据上：

- `RouteEntry` 的 name/description 改为 `TextRef`，新增 `category` 与 `authorization` 规则组；
- `#[authorize]` 支持多条规则（规则间 AND），403 返回数组字段；
- 新增 `IEndpointCatalog` / `EndpointDescriptor` / `EndpointInfo` 运行时端点目录；
- OpenAPI 生成需要显式传入 `IStringLocalizer`；
- 端点文本使用资源键，语言由全局 `Culture.Default` 决定。

Cargo 依赖版本改为 `0.6.0`；导入路径仍是 `webx::*`。

## 变更总览

| 项目 | 0.5.1 | 0.6.0 |
|------|-------|-------|
| 路由名称 | `RouteEntry.summary: &'static str` | `RouteEntry.name: TextRef` |
| 路由描述 | `RouteEntry.description: &'static str` | `RouteEntry.description: TextRef` |
| 分组 | 无 | `RouteEntry.category: TextRef` + `#[category]` |
| 授权 | `required_role` / `required_permission` 两个标量 | `authorization: &'static [AuthorizeRule]` |
| 403 扩展字段 | `required_role` / `required_permission` 字符串 | `required_roles` / `required_permissions` 数组 |
| 端点目录 | 无 | 新增 `Host::endpoints()` / DI `Arc<dyn IEndpointCatalog>` |
| OpenAPI | `generate_openapi_spec(title, version)` | `generate_openapi_spec(title, version, &dyn IStringLocalizer)` |
| 多语言 | 编译期字符串（无本地化） | 资源键 + `Culture.Default` + `locales/<culture>.json` |
| 自定义端点 | `StubEndpoint` 的 role/permission/policy 字段 | `StubEndpoint.authorization: &'static [AuthorizeRule]` |

## 1. 路由名称、描述与分类

0.5.1 没有 `#[display]` / `#[description]` / `#[category]`：

- 名称由请求类型名自动生成（如 `CreateBlogPostRequest` → “Create blog post”）；
- 描述来自 `impl` 块上的 `///` 文档注释。

0.6.0 可以把这些文本显式声明为资源键，并增加分组：

```rust
// 0.6.0：可选；不写时仍按 0.5.1 的规则回退
#[get("/api/blog")]
#[display(key = "endpoint.blog.create.name", text = "创建博客")]
#[description(key = "endpoint.blog.create.description", text = "创建一个博客草稿。")]
#[category("博客模块")]
impl IRequest<BlogPostModel> for CreateBlogPostRequest {}
```

- `#[display]` / `#[description]` / `#[category]` 可以写在路由宏之前或之后；
- `TextRef { key, text }` 保存资源键和字面量兜底，不再保存“已翻译文本”；
- `///` 仍然是未显式声明时的回退；名称最后回退到请求类型名；
- OpenAPI 的 `summary` 仍来自 `name`，`category` 非空时作为 `tag`。

## 2. 授权：从两个标量到规则组

```rust
// 0.5.1：最终落成 required_role / required_permission 两个字符串，多个同类声明会互相覆盖
#[authorize(role = "admin")]
#[authorize(permission = "reports:export")]

// 0.6.0：每条 #[authorize] 是一条规则，规则之间 AND
#[authorize(role = "admin")]
#[authorize(permission = "reports:export")]
```

新增能力：

```rust
#[authorize(roles = ["admin", "auditor"])]        // 任一角色
#[authorize(permissions = ["a", "b"])]            // 任一权限
#[authorize(role = "admin", permission = "x")]    // 同一规则内 AND
```

读取元数据：

```rust
for entry in inventory::iter::<RouteEntry> {
    for rule in entry.authorization {
        // rule.roles / rule.permissions 都是 &'static [&'static str]
    }
}
```

403 前端适配：

```json
{
  "required_roles": ["admin"],
  "required_permissions": []
}
```

不再有 `required_role` / `required_permission` 字符串字段。

## 3. 新增端点目录

0.5.1 没有公开的运行时端点目录；0.6.0 新增 Host 级 `IEndpointCatalog`：

```rust
let catalog = host.endpoints();                    // &dyn IEndpointCatalog
let all = catalog.all();                           // &[EndpointDescriptor]
let localized = catalog.localized(host.localizer()); // Vec<EndpointInfo>
```

在 Handler / `IHostedService` 中注入：

```rust
#[derive(Inject)]
struct ListEndpointsHandler {
    #[inject]
    catalog: Arc<dyn IEndpointCatalog>,
}
```

`EndpointDescriptor.path` 是声明的原始路由模式（如 `/api/blog/{slug}`），不是具体请求路径；
`method` 是大写 HTTP 方法。`all()` 是语言中立元数据，`localized()` / `localized_for(culture)`
才解析名称/描述/分类。

## 4. OpenAPI

```rust
// 0.5.1
let spec = generate_openapi_spec("My API", "1.0.0");

// 0.6.0
let spec = generate_openapi_spec("My API", "1.0.0", host.localizer());
```

OpenAPI 使用 Host 的默认 culture（`Culture.Default`），不跟随请求头。若需要另一语言的规范，
用对应 `Culture.Default` 构建 Host，或自定义生成逻辑。

## 5. 本地化

```json
{ "Culture": { "Default": "zh-CN" } }
```

```text
locales/zh-CN.json
locales/en.json
```

```rust
Host::builder()
    .add_localization()   // 约定读取 locales/<culture>.json
    .build();
```

单文件部署：

```rust
Host::builder()
    .add_localization_with(
        LocalizationOptions::none()
            .resources("zh-CN", include_str!("../locales/zh-CN.json"))
            .resources("en", include_str!("../locales/en.json")),
    )
    .build();
```

没有调用 `add_localization()` 时资源表为空，`key` 会回退为 key 本身；建议同时写 `text` 或使用
`#[display("字面量")]`。

## 6. StubEndpoint 与兼容层

自定义 `StubEndpoint` 的构造改为规则列表：

```rust
StubEndpoint {
    // ...
    authorization: &[AuthorizeRule {
        roles: &["admin"],
        permissions: &[],
    }],
}
```

- `auth_required_role` / `auth_required_permission` / `resource_policy` 字段已删除；
- `use_resource_authorization()` 已过时且为 no-op；
- `build_resource_policy_from_routes()` 已过时，不再参与 HTTP 分发；
- `resource_auth_middleware` 仅用于自定义 `IAuthorizationPolicy`。

## 迁移检查清单

- [ ] Cargo 依赖与 workspace 版本改为 `0.6.0`
- [ ] 自定义 `RouteEntry` / `StubEndpoint` 构造改用 `authorization` 规则组
- [ ] 读取 `RouteEntry.summary` 的代码改为 `RouteEntry.name` / `TextRef`
- [ ] 手动调用 `generate_openapi_spec` 时补 `host.localizer()`
- [ ] 前端 403 处理改为 `required_roles` / `required_permissions` 数组
- [ ] 需要多语言/管理端展示时，为端点添加 `#[display]` / `#[description]` / `#[category]` 并配置 `Culture.Default` 与 `locales/<culture>.json`
- [ ] 需要权限同步/管理菜单时接入 `Host::endpoints()` 或 `Arc<dyn IEndpointCatalog>`
- [ ] 需要运行时授权的应用评估 `IDynamicAuthorizer`（仅对有 `#[authorize]` 的路由生效）
- [ ] 运行 `cargo test --workspace` 与 `cargo clippy --workspace --all-targets`

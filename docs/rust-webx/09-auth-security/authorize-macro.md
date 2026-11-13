# authorize 宏与声明式授权

## 基本用法

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

// 两条规则都必须满足
#[post("/api/reports/export")]
#[authorize(role = "admin")]
#[authorize(permission = "reports:export")]
impl IRequest<Response> for ExportReportsRequest {}

// 动态委托：静态 gate 只要求已认证，声明规则整包交给 IDynamicAuthorizer 裁决
#[delete("/api/orders/{id}")]
#[authorize(dynamic, permission = "order.delete")]
impl IRequest<()> for DeleteOrderRequest {}
```

`#[authorize(dynamic)]` 可与 `role` / `permission` 写在同一属性里；多条 `#[authorize]` 中只要有
一条带 `dynamic`，整个端点就进入动态模式（声明规则本身保持不变，只是改由动态授权器评判）。

## 模型：规则组

`#[authorize]` 在编译期写入 `RouteEntry.authorization: &[AuthorizeRule]`，`AuthorizeRule` 只有两个
字段：

```rust
pub struct AuthorizeRule {
    pub roles: &'static [&'static str],        // any-of；空 = 不限制
    pub permissions: &'static [&'static str],  // any-of；空 = 不限制
}
```

判定规则：

- **规则之间 AND**：每个 `#[authorize]` 都必须满足；
- **规则内 any-of**：`roles` 中任一命中即可，`permissions` 同理；
- **两个维度 AND**：同时声明 `role` 和 `permission` 时，两者都要满足；
- **空规则**：bare `#[authorize]` 只要求有效的已认证身份。

这样多条 `#[authorize]` 不再被压缩成两个标量字段，而是完整保留为一组规则；权限系统直接读取
`Host::endpoints()` 返回的 `IEndpointCatalog` 中的 `authorization` 数组即可（见 [路由宏](../05-request-pattern/route-macros.md#运行时读取端点目录)）。

`RouteEntry` 另有 `authorization_dynamic: bool`：`#[authorize(dynamic)]` 把它置为 `true`，其余
端点一律为 `false`（包括公开端点）。规则列表本身不因 dynamic 改变——dynamic 只改变**谁来评判**
这些规则。

## 编译期收集与执行

元数据在编译期写入 `RouteEntry`；`StubEndpoint` 在 dispatch 前检查：

### 静态模式（默认，与 1.2.0 完全一致）

- 没有规则（`authorization` 为空）→ 公开端点；
- 有规则但无 claims → `401 Unauthorized`（统一由 host 写出，带 `WWW-Authenticate: Bearer` 挑战）；
- 有 claims 但存在未满足的规则 → `403 Forbidden`，problem 扩展字段为
  `required_roles` / `required_permissions`（第一条未满足规则的数组）。

### 动态模式（`#[authorize(dynamic)]`）

- 有规则但无 claims → 仍然 `401 Unauthorized`（认证是动态模式的前提，不会被跳过）；
- 有 claims → **跳过** claims 快照上的 `matches` 判定，把整条声明（`&'static [AuthorizeRule]`）
  交给已注册的 `IDynamicAuthorizer`；由它决定放行或返回 `403`；
- 没有注册任何 `IDynamicAuthorizer` → 保持"无动态检查即通过"，即已认证的请求会被放行。
  **这是文档化的行为，不是安全默认值**：使用 dynamic 就必须注册授权器（或接受"仅认证"），
  否则声明的 permission 不会被执行。

> 动态模式下 `required_roles` / `required_permissions` 不再由框架的静态 403 产生；拒绝时的
> problem 详情由你的授权器返回的 `Error::Forbidden` 决定。

## 与 add_authentication() 的关系

`add_authentication()` 只负责启用 JWT 认证，让受保护端点能拿到 claims；`#[authorize]` 的规则
由 endpoint 自己执行，不依赖额外开关。未启用认证时，受保护路由因无 claims 返回 401，规则
并非只收集不执行。

## 在 Handler 里读取身份

授权通过后，身份可以直接作为 Request 字段出现，不必再注入 `IHttpContext` 或手动解析 claims：

```rust
#[derive(WebxRequestMeta)]
pub struct UpdateProfileRequest {
    #[fromclaim]
    pub user: IdentityUser,       // 未登录 → 401
    pub display_name: String,    // JSON body
}

#[put("/api/profile")]
#[authorize]
impl IRequest<ProfileView> for UpdateProfileRequest {}
```

Handler 里 `req.user.id`、`req.user.tenant_id`、`req.user.is_in_role("admin")` 随取随用；
匿名可访问的端点用 `#[fromclaim(optional)] viewer: Option<IdentityUser>`。详见
[用户身份与多租户](../05-request-pattern/user-identity.md)。

## 动态授权：IDynamicAuthorizer

`#[authorize]` 适合**编译期固定**的角色/权限要求。如果授权依赖运行时数据（数据库里的
路由授权、功能开关、缓存中的黑名单/许可），用 `IDynamicAuthorizer`。

### 接口与参数

```rust
#[async_trait]
pub trait IDynamicAuthorizer: Send + Sync {
    // 1.1 起：只看身份 + 路由 + 方法
    async fn authorize(
        &self,
        claims: &dyn IClaims,
        route_pattern: &str, // 原始路由模式，如 /api/blog/{slug}
        method: &str,        // 大写方法，如 POST
    ) -> Result<()>;

    // 1.2.0 起：额外拿到端点声明的角色/权限（DeclaredRules 视图，legacy）
    async fn authorize_with_rules(
        &self,
        claims: &dyn IClaims,
        route_pattern: &str,
        method: &str,
        declared: &DeclaredRules<'_>,
    ) -> Result<()>;

    // 1.3.0 起的零分配主路径：直接传 RouteEntry.authorization 切片
    async fn authorize_with_declared_rules(
        &self,
        claims: &dyn IClaims,
        route_pattern: &str,
        method: &str,
        declared: &'static [AuthorizeRule],
    ) -> Result<()>;
}
```

只覆写**任意一个**即可：`authorize_with_declared_rules` 的默认实现转调
`authorize_with_rules`（再默认转调 `authorize`），旧的 1.1 / 1.2 实现无需改代码。框架调用的是
`authorize_with_declared_rules`；`DeclaredRules` 与 `authorize_with_rules` 标记为 legacy，
计划在 2.0 移除。

- `route_pattern` 与 `IEndpointCatalog` 的 `path`、运行时 `route_pattern()` 是同一个字符串；
  它**不是**具体请求路径，参数变化不会改变键，适合做授权资源键。
- `method` 是大写 HTTP 方法，和 `EndpointDescriptor.method` 一致。
- 执行到这里时 `claims` 一定存在：无 claims 的请求在静态阶段已经 401。
- `declared` 是端点 `#[authorize]` 的**原始切片**：每条属性一个 `AuthorizeRule`，规则之间 AND、
  规则内 any-of。动态模式下它是授权器的完整输入，不属于"提前拦截"的 gate。
- 拿不到 `IHttpContext`、路径参数或请求体。需要按资源实例（例如「只能改自己的博客」）判断时，
  应在 handler 里做，或自定义 `IEndpoint`；不要硬塞进 `IDynamicAuthorizer`。

### 执行顺序与触发条件

静态端点：

```text
路由匹配（route_pattern 可用）
  → 静态 #[authorize] 规则（失败直接 401/403，动态不再执行）
  → 已注册的 IDynamicAuthorizer（按注册顺序，全部通过才放行）
  → Handler
```

动态端点（`#[authorize(dynamic)]`）：

```text
路由匹配（route_pattern 可用）
  → 认证检查（无 claims → 401）
  → 已注册的 IDynamicAuthorizer 拿整条声明裁决（未注册 → 直通）
  → Handler
```

几个容易踩的点：

1. **动态授权只在路由有 `#[authorize]` 时运行**（包括 bare `#[authorize]`）。完全没有
   `#[authorize]` 的公开路由不会触发它。想用纯动态策略，至少加一个 bare `#[authorize]`
   要求已认证；更推荐 `#[authorize(dynamic)]`，让意图显式化。
2. 多个 `IDynamicAuthorizer` 是 **AND**：任意一个返回 `Err` 即拒绝，后续不再执行。
   静态端点里静态规则与动态授权之间也是 AND（先过静态规则）；动态端点里静态规则不参与判定。
3. 动态端点**必须**注册授权器才有权限检查；未注册时框架只保证"已认证"。

### 注册方式

自动注册（推荐，等价于 `#[inject]` 的构造函数注入）：

```rust
#[derive(Inject)]
pub struct RouteGrantAuthorizer {
    #[inject]
    grants: Arc<dyn IRouteGrantStore>,
}

#[inject]
#[async_trait]
impl IDynamicAuthorizer for RouteGrantAuthorizer { /* ... */ }
```

也可以手动注册：

```rust
svc.singleton::<dyn IDynamicAuthorizer>(|_| Arc::new(MyAuthorizer::default()))
```

自动注册的授权器是 Host 级单例：不要直接注入 scoped `DbContext`；需要数据库时使用单例授权缓存/仓储，
或在变更事件里刷新缓存。授权器每次受保护请求都会执行，避免同步网络/DB 阻塞热路径。

### 场景 1：数据库/cache 路由授权（推荐用法，"DB 为权限真相"）

这是 `IEndpointCatalog` 与权限同步最典型的配合方式：

1. 启动或定时任务用 `IEndpointCatalog` 同步资源表：`(method, path)` 作为主键，`name` 作为
   显示名，`authorization` 作为声明的默认要求；`category` 可作为菜单分组；
2. 管理端把资源授权给角色/用户（`role ↔ resource`）；
3. `IDynamicAuthorizer` 在请求时用 `(method, route_pattern)` 查当前用户的授权缓存/DB。

路由声明用 **`#[authorize(dynamic, permission = "...")]`**：

```rust
// 权限只存在于数据库：token 不需要 permissions 快照，也不需要重新登录。
#[delete("/api/orders/{id}")]
#[authorize(dynamic, permission = "order.delete")]
impl IRequest<()> for DeleteOrderRequest {}
```

动态模式下静态 gate 只保证"已认证"，`permission = "order.delete"` 这段声明通过
`authorize_with_declared_rules` 原样交给授权器——它可以选择直接信任 DB grants（按
`(method, route_pattern)` 判定），也可以把声明当作"默认要求"再叠加自己的策略：

```rust
pub trait IRouteGrantStore: Send + Sync {
    fn allows(&self, user: &str, method: &str, route: &str) -> bool;
}

#[derive(Inject)]
pub struct RouteGrantAuthorizer {
    #[inject]
    grants: Arc<dyn IRouteGrantStore>,
    #[inject]
    catalog: Arc<dyn IEndpointCatalog>,
}

#[inject]
#[async_trait]
impl IDynamicAuthorizer for RouteGrantAuthorizer {
    async fn authorize_with_declared_rules(
        &self,
        claims: &dyn IClaims,
        route_pattern: &str,
        method: &str,
        declared: &'static [AuthorizeRule],
    ) -> Result<()> {
        // 只接受目录中声明的路由，避免 DB 里残留的旧授权生效。
        if self.catalog.get(method, route_pattern).is_none() {
            return Err(Error::Forbidden(format!(
                "unknown route {method} {route_pattern}"
            )));
        }

        // `declared` 里有端点的 permission 原文，可写审计、可做"声明 → 策略"映射。
        let _declared_permissions = declared.iter().flat_map(|rule| rule.permissions);

        if self.grants.allows(claims.subject(), method, route_pattern) {
            Ok(())
        } else {
            Err(Error::Forbidden(format!(
                "no grant for {method} {route_pattern}"
            )))
        }
    }
}
```

同步侧只需要遍历目录，不需要再手工维护路由字符串：

```rust
for endpoint in catalog.all() {
    // endpoint.method: "POST"
    // endpoint.path:   "/api/blog"（原始路由模式）
    // endpoint.name:   TextRef，用 localizer 解析成显示名
}
```

`grants` 建议来自进程内缓存（启动加载 + 变更失效），不要在每次请求里直接查库；授权器是
热路径，DB 延迟会直接加到所有受保护 API。

如果授权完全由 DB grants 决定：用 `#[authorize(dynamic)]`（或 `#[authorize(dynamic, permission = ...)]`
保留声明的默认 permission 供审计/同步），**不要**写死 `#[authorize(role = "admin")]`——静态模式下
它会先拦掉非 admin，动态 grants 根本没机会执行；dynamic 模式下它只会作为声明交给授权器。

### 场景 2：功能开关 / 维护模式

按路由模式 + 方法做运行时开关，不需要请求参数：

```rust
// self.flags / self.catalog 等字段在结构体上省略
if method == "POST"
    && route_pattern.starts_with("/api/billing/")
    && self.flags.maintenance_mode()
{
    return Err(Error::Forbidden(
        "billing writes are temporarily disabled".into(),
    ));
}
Ok(())
```

功能开关、维护模式、按路由的授权缓存都适合放在这里；全局限流、CORS 这类请求级策略
仍应使用中间件，不要混入授权器。

> 租户或资源实例隔离通常需要路径参数（`{tenant}` / `{id}`）的具体值。`IDynamicAuthorizer`
> 只能看到路由模式，看不到具体值；这类判断放到 handler 或自定义 endpoint。

### 不要用它做资源所有权判断

`IDynamicAuthorizer` 只拿到 `claims`、`route_pattern`、`method`，没有 `IHttpContext`、路径参数
或请求体。因此：

| 需求 | 正确位置 |
|------|---------|
| 「用户是否有权访问这类路由」 | `#[authorize]` 或 `IDynamicAuthorizer` |
| 「用户是否能修改 id=123 这条记录」 | Handler 内读取路径参数/实体后判断 |
| 「租户只能访问自己租户的数据」 | Handler 内按路径参数/实体判断 |
| 全局限流、CORS、压缩 | 中间件 |
| 自定义策略对象（旧式） | `IAuthorizationPolicy` + `resource_auth_middleware` |

### 选择哪种授权

| 授权方式 | 数据来源 | 时机 | 适合 |
|---------|---------|------|------|
| `#[authorize]` | 编译期元数据 + token claims | endpoint 内、动态之前 | 固定角色/权限、bare 认证 |
| `#[authorize(dynamic)]` | 编译期声明作为输入，运行时由授权器裁决 | endpoint 内（跳过 claim gate） | DB 为权限真相、每请求动态判定 |
| `IDynamicAuthorizer` | DB/cache/配置 | 静态规则之后（dynamic 端点上代替静态规则） | 路由级动态授权、功能开关、维护模式 |
| Handler 检查 | 请求上下文/实体 | handler 内 | 资源实例所有权、租户隔离 |
| `IAuthorizationPolicy` | 自定义 | 手动中间件/端点 | 旧式自定义策略 |

静态与动态的差异：

| | 静态（默认） | 动态（`#[authorize(dynamic)]`） |
|---|---|---|
| 无 claims | 401 | 401 |
| 声明规则由谁评判 | `StubEndpoint`（claims 快照） | `IDynamicAuthorizer` |
| claims 缺 permission | 403（动态不执行） | 交给授权器，可能放行 |
| 未注册授权器 | 声明规则照常执行 | 只要求已认证（直通） |
| 新增授权何时生效 | 重新登录/刷新 token 快照 | 下一次请求（授权器查 DB/cache） |
| 声明规则是否保留 | 是 | 是（作为授权器输入） |

错误映射遵循 `Error`：`Error::Forbidden` → 403，`Error::Unauthorized` → 401；多个授权器
按注册顺序执行，第一个 `Err` 决定响应。

## Docbit 实例

```rust
// contracts/auth.rs
#[get("/api/auth/me")]
#[authorize]
impl IRequest<UserView> for AuthMeRequest {}
```

任何携带有效 JWT 的用户可访问 `/api/auth/me` 获取自己的信息。管理路由由 docbit 的
`RoleAuthorizer`（`IDynamicAuthorizer`）额外约束——它是简化示例（按路由前缀 + `admin` 角色）；
需要按数据库授权表逐条判断时，参考上面的「场景 1：数据库/cache 路由授权」。

> `RoleAuthorizer` 中 `/api/comments*` 直接放行，所有权检查在 handler 内；这符合
> `IDynamicAuthorizer` 只做路由级判断的边界。

## 小结

`#[authorize]` 将授权要求声明在路由旁，与 ASP.NET Core 的 `[Authorize]` 体验一致。规则组模型
同时支持：仅认证、any-of 角色、any-of 权限、角色 AND 权限，以及多条规则的组合。
`#[authorize(dynamic)]` 在同一套声明之上把判定权交给 `IDynamicAuthorizer`，让"DB 是权限唯一
真相 + 每请求动态判定"成立，而不再依赖登录期的 permissions 快照。

下一节：[安全最佳实践](security-best-practices.md)

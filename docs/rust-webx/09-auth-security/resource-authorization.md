# 基于资源的授权（自定义策略）

> 日常端点的静态授权（`#[authorize]`）见 [authorize 宏](authorize-macro.md)；运行时按 DB/cache
> 数据决定的授权见 [IDynamicAuthorizer](authorize-macro.md#动态授权idynamicauthorizer)。本页讲
> 自定义策略对象 `ResourceAuthorization` / `IAuthorizationPolicy` 及其旧式中间件用法。

## 核心概念

授权以**路由模式**（route pattern）为资源键：

```
实际请求: GET /api/users/abc-123
资源键:   /api/users/{id}    ← route_pattern()
```

用户的 roles/permissions 与资源键匹配决定是否允许访问。

## ResourceAuthorization

```rust
use webx::ResourceAuthorization;

let policy = ResourceAuthorization::new()
    .allow_role("/api/admin/users", "admin")
    .allow_role("/api/users/{id}", "user");
    // .allow_permission(...) — 需 claims 侧支持对应 permission
```

## HostBuilder 集成

```rust
Host::builder()
    .add_authentication()  // JWT；#[authorize] 规则由端点自己执行
    .build()
```

`add_authentication()` 注册 JWT 中间件，让受保护端点拿到 claims。`#[authorize]` 的规则是端点
声明的一部分，由 `StubEndpoint` 在路由匹配后逐条执行（规则之间 AND，规则内 any-of），不需要
构建额外策略，也没有第二个开关。

> 端点直接执行 `#[authorize]` 规则；策略式「每 path 一组角色/权限」无法忠实表达同一端点的
> 多条规则，因此框架不提供策略构建入口。

当前 HTTP 应用的常见做法：

- `add_authentication()` → JWT 中间件
- `#[authorize]` / `#[authorize(role = "…")]` / `#[authorize(permission = "…")]` → 端点规则，由 endpoint 执行
- `#[authorize(dynamic, …)]` → 静态 gate 只要求已认证，声明交给 `IDynamicAuthorizer`（"DB 为权限真相"）
- `IDynamicAuthorizer` → 应用自定义的动态路由授权（如 docbit `RoleAuthorizer`），详见[动态授权](authorize-macro.md#动态授权idynamicauthorizer)
- `ResourceAuthorization` + `resource_auth_middleware` → 仅用于自定义 `IAuthorizationPolicy`

## 手动 middleware（高级）

`resource_auth_middleware` 仍可用于自定义 `IAuthorizationPolicy`，但须在路由匹配之后才能读取 `route_pattern()`。内置的路由授权已在 endpoint 层处理此场景。

## 资源键的匹配方式

资源键按**精确字符串**匹配（`HashMap` 键查找），不是路径通配符：策略中的键必须与 `route_pattern()` 完全一致，否则 `covers_route()` 返回 `false`、规则不会生效。例如 `/api/admin/**` 永远不会匹配到真实路由。

```rust
.allow_role("/api/admin/users", "admin")
```

## 小结

资源授权将路由模式与用户身份关联，实现声明式访问控制。端点直接执行 `#[authorize]` 规则；
`Host::endpoints()` 返回的 `IEndpointCatalog` 可把这些端点及其规则组导出给权限系统。

下一节：[authorize 宏与声明式授权](authorize-macro.md)

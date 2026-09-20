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

// 需要特定 permission
#[put("/api/settings")]
#[authorize(permission = "settings:write")]
impl IRequest<SettingsDto> for UpdateSettingsRequest {}
```

## 编译期收集

`#[authorize]` / `#[authorize(role = "…")]` / `#[authorize(permission = "…")]` 在编译期写入 `RouteEntry` 元数据，由 `StubEndpoint` 在 dispatch 前检查 claims。

## 与 add_authentication() 的关系

bare `#[authorize]` 展开为角色 `"authenticated"`，`StubEndpoint` 在 dispatch 前检查 claims；因此未启用 `add_authentication()` 时，无 claims 的请求仍会返回 401，元数据并非只收集不执行。认证与资源授权的组合方式详见 [资源授权](resource-authorization.md) 与 [中间件顺序](../07-middleware/ordering-strategy.md)。

## Docbit 实例

```rust
// contracts/auth.rs
#[get("/api/auth/me")]
#[authorize]
impl IRequest<UserView> for AuthMeRequest {}
```

任何携带有效 JWT 的用户可访问 `/api/auth/me` 获取自己的信息。管理路由由 docbit 的 `RoleAuthorizer`（`IDynamicAuthorizer`）额外约束。

## 小结

`#[authorize]` 将授权要求声明在路由旁，与 ASP.NET Core 的 `[Authorize]` 体验一致。支持 bare `#[authorize]`、`role = "…"` 与 `permission = "…"`。

下一节：[安全最佳实践](security-best-practices.md)

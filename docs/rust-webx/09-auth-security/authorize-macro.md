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

| 配置 | 效果 |
|------|------|
| 无 `add_authentication()` | JWT 未启用；`#[authorize]` 元数据收集但不执行 |
| `add_authentication()` | 注册 **JWT 中间件**；路由级 `#[authorize]` 在 `StubEndpoint` 内检查；`IDynamicAuthorizer` 可扩展策略 |
| `.use_resource_authorization()` | 从路由元数据构建 `ResourceAuthorization`，在 endpoint 层（路由匹配后）额外校验 |

详见 [资源授权](resource-authorization.md) 与 [中间件顺序](../07-middleware/ordering-strategy.md)。

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

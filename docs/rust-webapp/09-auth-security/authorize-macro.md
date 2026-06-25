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

// 需要特定权限
#[put("/api/settings")]
#[authorize(permission = "settings:write")]
impl IRequest<SettingsDto> for UpdateSettingsRequest {}
```

## 编译期收集

`#[authorize]` 宏在编译期记录授权要求，`Host::build()` 时 `collect_authorizers()` 自动构建 `ResourceAuthorization` 策略。

## 与 use_auth() 的关系

| 配置 | 效果 |
|------|------|
| 无 `use_auth()` | `#[authorize]` 元数据收集但不执行 |
| `use_auth()` | JWT 认证 + 授权策略生效 |

## Docbit 实例

```rust
// contracts/auth.rs
#[get("/api/auth/me")]
#[authorize]
impl IRequest<UserView> for AuthMeRequest {}
```

任何携带有效 JWT 的用户可访问 `/api/auth/me` 获取自己的信息。

## 小结

`#[authorize]` 将授权要求声明在路由旁，与 ASP.NET Core 的 `[Authorize]` 体验一致。

下一节：[安全最佳实践](security-best-practices.md)

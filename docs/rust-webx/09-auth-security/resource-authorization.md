# 基于资源的授权

## 核心概念

授权以**路由模式**（route pattern）为资源键：

```
实际请求: GET /api/users/abc-123
资源键:   /api/users/{id}    ← route_pattern()
```

用户的 roles/permissions 与资源键匹配决定是否允许访问。

## ResourceAuthorization

```rust
use rust_webx::authz::ResourceAuthorization;

let policy = ResourceAuthorization::new()
    .allow_role("/api/admin/**", "admin")
    .allow_role("/api/users/{id}", "user");
    // .allow_permission(...) — 需 claims 侧支持对应 permission
```

## HostBuilder 集成

```rust
Host::builder()
    .add_authentication()
    .use_resource_authorization()  // 从 #[authorize] 元数据构建策略
    .build()
```

`use_resource_authorization()` 在 **endpoint 层**执行策略（路由匹配后、`route_pattern()` 可用），而非 pipeline 中间件层。这避免了 `resource_auth_middleware` 在路由匹配前无法获知 pattern 的问题。

当前 HTTP 应用的常见做法：

- `add_authentication()` → JWT 中间件
- `#[authorize]` / `#[authorize(role = "…")]` / `#[authorize(permission = "…")]` → endpoint 内检查
- `.use_resource_authorization()` → 可选，从编译期元数据构建 `ResourceAuthorization`
- `IDynamicAuthorizer` → docbit 等应用的自定义策略（如 `RoleAuthorizer`）

## 手动 middleware（高级）

`resource_auth_middleware` 仍可用于自定义 `IAuthorizationPolicy`，但须在路由匹配之后才能读取 `route_pattern()`。框架内置的 `.use_resource_authorization()` 已在 endpoint 层处理此场景。

## 通配符

`/**` 匹配所有子路径：

```rust
.allow_role("/api/admin/**", "admin")
```

## 小结

资源授权将路由模式与用户身份关联，实现声明式访问控制。通过 `.use_resource_authorization()` 可从 `#[authorize]` 元数据自动构建策略。

下一节：[authorize 宏与声明式授权](authorize-macro.md)

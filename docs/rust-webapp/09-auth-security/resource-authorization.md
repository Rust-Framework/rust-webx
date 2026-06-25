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
use rust_webapp::authz::ResourceAuthorization;

let policy = ResourceAuthorization::new()
    .allow_role("/api/admin/**", "admin")
    .allow_role("/api/users/{id}", "user")
    .allow_permission("/api/settings", "settings:write");
```

`use_auth()` 自动从 `#[authorize]` 编译期元数据构建策略。

## 授权流程

```
jwt_middleware 设置 claims
    ↓
resource_auth_middleware 读取 route_pattern()
    ↓
比对 claims.roles() / claims.permissions()
    ↓
通过 → 继续  |  拒绝 → 403
```

## 通配符

`/**` 匹配所有子路径：

```rust
.allow_role("/api/admin/**", "admin")
```

## 小结

资源授权将路由模式与用户身份关联，实现声明式访问控制。

下一节：[authorize 宏与声明式授权](authorize-macro.md)

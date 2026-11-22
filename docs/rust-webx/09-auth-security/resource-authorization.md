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
- `IAuthorizationPolicy` 对象 → 仅作为策略数据模型保留；不要把它接到中间件上

## 实例级归属（owner / tenant）

路由级规则只回答"能不能访问这个端点"；"这条数据属于谁"只有 Handler 知道（资源形状是应用决定的，
框架不做资源授权器）。把 401/403 判定收敛到两个 `RequestUser` helper，实例校验仍然写在 Handler：

```rust
#[handler(inject)]
#[async_trait]
impl IRequestHandler<GetOrderRequest, OrderDto> for GetOrderHandler {
    async fn handle(&mut self, req: GetOrderRequest) -> ActionResult<OrderDto> {
        // 查询条件就带身份作用域，避免"越权读到后才 403"泄露存在性
        let predicate = Predicate::<Order>::always()
            .and(linq!(|o: Order| o.id == req.id))
            .and(linq!(|o: Order| o.tenant_id == req.user.tenant_id));
        let order = self.orders
            .query()
            .filter(predicate)
            .first()
            .await?
            .ok_or_else(|| sample::Error::not_found("order"))?;

        req.require_owner(&order.user_id)?;      // sub 不匹配 → 403
        req.require_same_tenant(&order.tenant_id)?; // 跨租户 → 403
        Ok(order.into())
    }
}
```

| helper | 匿名 | 已认证但无租户声明 | 不匹配 |
|--------|------|--------------------|--------|
| `require_owner(owner_id)` | 401 | —（与租户无关） | 403 `resource owner required` |
| `require_same_tenant(tenant_id)` | 401 | 403 `no tenant claim` | 403 `cross-tenant access is not allowed` |

要点：

- `owner_id` / `tenant_id` 来自**已加载的资源**，绝不来自 body/query；否则调用方可以自称 owner 或租户。
- 403 文案不包含资源 id 或 owner id，避免信息泄露。
- 更复杂的 membership（团队、角色表）用应用侧查询 helper 实现，最终仍落到 401/403 语义；
  不需要、也不应该扩展成通用"资源授权器"。

## 手动 middleware（已弃用）

`resource_auth_middleware` 在路由匹配**之前**运行，拿不到 `route_pattern()`，因此无法真正授权。1.5.0 起它被标记为 deprecated，并在请求期 fail-closed（返回 403 并提示迁移）；请不要在新代码中使用。路由级授权请使用 `#[authorize]` 静态规则或 `#[authorize(dynamic)]` + `IDynamicAuthorizer`，两者都在 endpoint 层、路由匹配之后执行。

## 资源键的匹配方式

资源键按**精确字符串**匹配（`HashMap` 键查找），不是路径通配符：策略中的键必须与 `route_pattern()` 完全一致，否则 `covers_route()` 返回 `false`、规则不会生效。例如 `/api/admin/**` 永远不会匹配到真实路由。

```rust
.allow_role("/api/admin/users", "admin")
```

## 小结

资源授权将路由模式与用户身份关联，实现声明式访问控制。端点直接执行 `#[authorize]` 规则；
`Host::endpoints()` 返回的 `IEndpointCatalog` 可把这些端点及其规则组导出给权限系统。

下一节：[authorize 宏与声明式授权](authorize-macro.md)

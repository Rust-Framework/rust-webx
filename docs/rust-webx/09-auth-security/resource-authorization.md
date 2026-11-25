# 资源授权与实例级归属

> 路由级授权（“能不能访问这个端点”）属于端点声明：静态 `#[authorize]` 或
> `#[authorize(dynamic)]` + `IDynamicAuthorizer`，细节见
> [authorize 宏与声明式授权](authorize-macro.md)。本页讲两者之间的分界：
> **路由级规则在端点里，实例级归属（这条记录属于谁）在 Handler 里**。

## 两个问题，两个位置

| 问题 | 数据来源 | 正确位置 |
|------|---------|---------|
| 这类路由允许哪些角色/权限/租户访问？ | 编译期声明 + token claims，或 DB/cache | `#[authorize]` / `#[authorize(dynamic)]` + `IDynamicAuthorizer` |
| 这条 `id=123` 的记录属于当前用户/租户吗？ | 已加载的资源实体 | Handler 内 `RequestUser::require_owner` / `require_same_tenant` |

框架不做“资源授权器”：资源形状是应用定义的（`order.user_id`、`blog.tenant_id`、
团队 membership 表……），只有 Handler 能判断。框架只提供 401/403 的判定 helper，
避免每个 Handler 手写状态码。

## 路由级：资源键是路由模式

动态授权以**路由模式**（route pattern）作为授权资源键，而不是具体请求路径：

```
实际请求: GET /api/users/abc-123
资源键:   /api/users/{id}    ← route_pattern()
```

参数变化不改变键，所以路由模式适合做主键（`IEndpointCatalog`、DB 授权表、缓存键）；
具体路径参数只在 Handler 里可见。

端点声明要授权的路由，并把声明交给动态授权器：

```rust
// 静态：编译期固定的角色/权限
#[delete("/api/orders/{id}")]
#[authorize(permission = "order.delete")]
impl IRequest<()> for DeleteOrderRequest {}

// 动态：静态 gate 只要求已认证，声明整包交给 IDynamicAuthorizer 裁决
#[delete("/api/orders/{id}")]
#[authorize(dynamic, permission = "order.delete")]
impl IRequest<()> for DeleteOrderRequest {}
```

授权器实现 `IDynamicAuthorizer::authorize_with_declared_rules` 时拿到的 `declared`
就是 `RouteEntry.authorization` 的原始切片（`&'static [AuthorizeRule]`，规则之间 AND、
规则内 any-of），因此可以直接把 `(method, route_pattern)` 与声明一起用于 DB grants 判断
（完整结构体、DI 注入与缓存建议见 [authorize 宏](authorize-macro.md) 的「场景 1」）：

```rust
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
        let _declared = declared.iter().flat_map(|rule| rule.permissions);
        if self.grants.allows(claims.subject(), method, route_pattern) {
            Ok(())
        } else {
            Err(Error::forbidden(format!(
                "no grant for {method} {route_pattern}"
            )))
        }
    }
}
```

- `#[authorize(dynamic)]` 路由存在但没注册授权器 → `Host::build()` 失败（fail-closed）；
- 授权器看不到路径参数、请求体、`IHttpContext`；
- token 里不需要 permissions 快照：DB 是权限真相时用 dynamic，授权改动下一次请求即生效。

`use_authentication()` 只注册 JWT 中间件，让受保护端点拿到 claims；`#[authorize]` 的规则是
端点声明的一部分，由 `StubEndpoint` 在路由匹配后逐条执行，不需要构建额外策略，也没有第二个
开关。

## 实例级归属（owner / tenant）

路由级规则通过后，Handler 仍然要校验这条数据属于谁。把 401/403 判定收敛到两个
`RequestUser` helper：

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

        req.require_owner(&order.user_id)?;         // sub 不匹配 → 403
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

- `owner_id` / `tenant_id` 来自**已加载的资源**，绝不来自 body/query；否则调用方可以自称
  owner 或租户。
- 403 文案不包含资源 id 或 owner id，避免信息泄露。
- 更复杂的 membership（团队、角色表）用应用侧查询 helper 实现，最终仍落到 401/403 语义；
  不需要、也不应该扩展成通用“资源授权器”。
- 想彻底不泄露存在性时，把归属条件并进查询（如上例的 `tenant_id`），越权请求直接得到 404。

## 小结

资源授权分两层：路由级要求写在端点旁（`#[authorize]` 静态规则，或 `#[authorize(dynamic)]`
把声明交给 `IDynamicAuthorizer::authorize_with_declared_rules`）；实例级归属写在 Handler 里
（`require_owner` / `require_same_tenant`）。`Host::endpoints()` 返回的 `IEndpointCatalog`
可把端点及其 `authorization` 规则组导出给权限系统。

下一节：[authorize 宏与声明式授权](authorize-macro.md)

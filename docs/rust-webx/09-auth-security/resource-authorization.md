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

## 外部 PDP（`IAuthorizationClient`）

权限真相在外部策略服务（PDP）时，用端口 + 适配器，不把角色表搬进框架：

| 项 | 位置 |
|----|------|
| 端口 | `webx::IAuthorizationClient`（`check` / `check_batch`） |
| 入参与结果 | `webx::AccessCheck { subject, tenant_id, permission, resource, context }` → `AccessDecision::{Allow, Deny}` |
| HTTP 适配器 | `webx::{AuthorizationHttpConfig, HttpAuthorizationClient}` |
| 装配 | `HostBuilder::use_authorization_client(Arc<dyn IAuthorizationClient>)` |

```rust
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use webx::*;

let config = AuthorizationHttpConfig::new("https://pdp.example.com/v1/check")
    .with_bearer_token(pdp_token)                 // 可选：PDP 自己的客户端凭据
    .with_timeout(Duration::from_secs(2))         // 100ms–10s
    .with_max_batch(50)                           // 1–500
    .with_cache_ttl(Duration::from_secs(30));     // 1s–300s；默认**不缓存**
let client = Arc::new(HttpAuthorizationClient::new(config, http, clock)?);

Host::builder()
    .use_authentication()
    .use_authorization_client(client)
    .build();
```

线上契约：`POST` 一个对象 `{"subject","tenant_id","permission","resource","context"}`
（缺省项为 `null`，空 `context` 为 `{}`），返回 `{"decision":"allow"}` 或 `{"decision":"deny"}`
（大小写不敏感）。`check_batch` 是同一对象的 JSON 数组，返回**等长**数组。别的形状
（`{"allowed":true}`、多余/缺失元素）一律按 malformed 处理，不猜。

调用点在 Handler 里显式发起，Deny → 403，`Err` 交给 `?` → 503：

```rust
#[derive(Inject)]
pub struct PayInvoiceHandler {
    #[inject]
    authz: Arc<dyn IAuthorizationClient>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<PayInvoiceRequest, InvoiceDto> for PayInvoiceHandler {
    async fn handle(&mut self, req: PayInvoiceRequest) -> ActionResult<InvoiceDto> {
        let decision = self
            .authz
            .check(&AccessCheck {
                subject: req.user.id.clone(),
                tenant_id: Some(req.user.tenant_id.clone()),
                permission: "invoice.pay".to_string(),
                resource: Some(req.id.to_string()),
                context: BTreeMap::new(),
            })
            .await?; // PDP 不可用 → 503，fail-closed
        if decision != AccessDecision::Allow {
            return Err(Error::forbidden("not allowed to pay this invoice"));
        }
        // ...
    }
}
```

- `Ok(Deny)` 是正常裁决（403），`Err` 是"PDP 没能裁决"（503）——两者不能混，
  任何失败都**不会**降级成 `Allow`。
- 非 2xx（含 PDP 回 401/403）、超时、连接失败、响应体超 256 KiB、JSON 形状不符 → 503（可重试）。
- 决策缓存只缓存 `Allow`：`Deny` 不缓存。**缓存的 `Allow` 在 TTL 内不会再询问 PDP**，所以撤权/禁用
  最迟 `cache_ttl`（≤300s）后生效；需要立即生效时调用 `invalidate()` 或关闭缓存。PDP 故障期间，
  已缓存的 `Allow` 仍会放行（TTL 缓存的固有语义），未命中时才 fail-closed。键覆盖 `subject`/`tenant`/
  `permission`/`resource`/有序 `context`，条目上限 65536，满了按 FIFO 淘汰；`invalidate()` 清空。
- 每次检查（execute + 读响应体 + JSON 解析）整体受 `timeout` 约束：PDP 即使先回响应头再慢滴 body，
  也不会把一次授权检查拖过预算。
- `check_batch` 超过 `max_batch` 是配置错误（500），不是 PDP 故障。
- 注册是"注册端口"：框架**不会**自动给路由挂 PDP。要么 Handler 显式调用，要么在
  `#[authorize(dynamic)]` 的 `IDynamicAuthorizer` 里转调该端口（那条路仍然拿不到路径参数）。
- 不要建角色/权限目录：框架只声明端口与适配器，权限模型、角色层级与决策逻辑属于 PDP 侧；
  框架也不提供决策实现。

判定与边界见 [federated-auth.md](../../design/federated-auth.md)。

## 小结

资源授权分两层：路由级要求写在端点旁（`#[authorize]` 静态规则，或 `#[authorize(dynamic)]`
把声明交给 `IDynamicAuthorizer::authorize_with_declared_rules`）；实例级归属写在 Handler 里
（`require_owner` / `require_same_tenant`）。`Host::endpoints()` 返回的 `IEndpointCatalog`
可把端点及其 `authorization` 规则组导出给权限系统。

权限真相在外部 PDP 时，第三层不是"再写一个授权器"，而是 Handler 显式调用
`IAuthorizationClient`：Deny → 403，PDP 不可用 → 503。框架只提供端口与 HTTP 适配器，
不定义角色/权限模型。

下一节：[authorize 宏与声明式授权](authorize-macro.md)

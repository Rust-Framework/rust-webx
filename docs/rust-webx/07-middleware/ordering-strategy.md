# 中间件编排策略

## 实际执行顺序

`Host::build()` 按以下顺序把中间件装入管道（与 `crates/host/src/server.rs` 的装配顺序一致）：

```
1.  RequestId                ← 为所有请求生成/透传 ID
2.  ClientInfo（可选）        ← ClientInfo.TrustForwardedHeaders；解析 X-Forwarded-For/X-Real-IP
3.  Metrics（可选）           ← Metrics.enabled；放在 CORS/RateLimit 之前，429/预检也计数
4.  自定义 BeforeCors 槽      ← use_middleware(MiddlewareSpec::new::<M>().at(MiddlewareSlot::BeforeCors))
5.  CORS（可选）              ← Cors.Origins 非空；只有真正的预检（Origin + Access-Control-Request-Method）在此短路
6.  RateLimit（可选，认证前） ← use_rate_limiter 显式 store 或 RateLimit.enabled；按客户端 IP 计桶
7.  CSRF（可选）              ← use_csrf；在认证之前拒绝跨站请求，只作用于携带会话 Cookie 的请求
8.  Authentication            ← use_authentication() / 注册的认证方案（JWT、Session Cookie、API Key）
9.  RateLimitPolicy（可选）   ← use_rate_limit_policy；在认证之后，User/Tenant/ApiKey 主体可解析
10. Idempotency（可选）       ← use_idempotency 或 Idempotency.Enabled；在认证之后按主体/租户分键
11. 自定义 AfterAuth 槽（默认）← use_middleware(spec) / DI 注册的 IMiddleware，此处可读到 claims
12. WebSocket（可选）         ← 升级请求走完认证后短路 101，会话接管连接
13. SPA（可选）               ← 静态文件；跳过 /api/*，不吞 API 404
14. 自定义 AfterSpa 槽        ← use_middleware(MiddlewareSpec::new::<M>().at(MiddlewareSlot::AfterSpa))
15. Router / Endpoint         ← 框架内置（最终处理器）
```

安全响应头（`SecurityHeaders`）由引擎的响应 finalizer 统一添加，不在上面的
中间件链里；公开的 `SecurityHeadersMiddleware` 只用于自定义管道，宿主不会重复添加。

## 中间件槽位

| 槽位 | 位置 | 用途 |
|------|------|------|
| `MiddlewareSlot::BeforeCors` | RequestId/ClientInfo/Metrics 之后、CORS 之前 | 需要观察所有请求（包括预检、被 CORS 短路）的中间件 |
| `MiddlewareSlot::AfterAuth` | 认证/授权类内置中间件之后、WebSocket/SPA 之前（**默认**） | 需要读 `ctx.claims()` 的鉴权/租户/审计中间件 |
| `MiddlewareSlot::AfterSpa` | 静态文件之后、Router 之前 | 依赖“静态文件已处理”的兜底钩子 |

```rust
use webx::{MiddlewareSlot, MiddlewareSpec};

Host::builder()
    .use_middleware(MiddlewareSpec::new::<TenantGuard>())    // 默认 AfterAuth：能拿到身份
    .use_middleware(MiddlewareSpec::new::<PreflightAudit>().at(MiddlewareSlot::BeforeCors))
    .use_middleware(
        MiddlewareSpec::factory(|| Arc::new(Fallback::new()) as Arc<dyn IMiddleware>)
            .at(MiddlewareSlot::AfterSpa),
    )
    .build();
```

直接注册进 DI 容器的 `IMiddleware`（如 `svc.add_middleware::<T>()`）也会进入默认的
`AfterAuth` 槽，排在 builder 注册的中间件之后。

## 原则

| 原则 | 说明 |
|------|------|
| RequestId 最外层 | 每个请求（含被短路/报错的请求）都有可追踪 ID |
| ClientInfo 尽早 | 所有读取客户端地址的组件（限流、幂等、认证、绑定）都读同一份解析结果 |
| 安全头由 finalizer 覆盖 | 包括错误响应在内的所有响应都带安全头 |
| CORS 在短路型中间件之前 | 413/429/401 等错误响应也要带 CORS 头，浏览器才读得到 |
| Metrics 在 CORS/RateLimit 之前 | 预检与被限流的请求同样计入指标 |
| CSRF 在认证之前 | 跨站请求在查询会话存储之前就被拒绝 |
| 认证在业务中间件之前 | 默认槽在认证之后，`ctx.claims()` 非空 |
| 授权类限流/幂等在认证之后 | `RateLimitPolicy` 与幂等键需要已解析的主体/租户 |
| WebSocket 在认证与业务中间件之后 | 升级请求先过同一套认证管线，再交出连接 |
| SPA 不处理 `/api/*` | 未匹配的 API 路径由 Router 返回 404，而非 index.html |
| 日志尽可能早 | `BeforeCors` 槽可记录预检等会被短路的请求 |

## 认证与授权

`use_authentication()` 注册认证方案中间件（位于 RateLimitPolicy 与 Idempotency 之前、
SPA 之前）。路由级 `#[authorize]` 授权在 endpoint 内执行——路由匹配后规则可用，不需要
额外的策略开关。授权语义详见 [资源授权与实例级归属](../09-auth-security/resource-authorization.md)。

## 小结

中间件顺序影响行为正确性。使用 `HostBuilder` 内置方法与 `MiddlewareSlot` 显式槽位，
可避免大部分排序问题；默认槽在认证之后，是“能拿到身份”的默认位置。

下一章：[中介者与事件](../08-mediator-events/INDEX.md)

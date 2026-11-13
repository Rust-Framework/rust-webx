# 中间件编排策略

## 实际执行顺序

`Host::build()` 按以下顺序把中间件装入管道：

```
1. SecurityHeaders          ← 安全头
2. RequestId                ← 为所有请求生成 ID
3. 自定义 BeforeCors 槽      ← use_middleware_at(.., MiddlewareSlot::BeforeCors)
4. CORS                     ← 只有真正的预检（Origin + Access-Control-Request-Method）在此短路
5. Metrics（可选）           ← Metrics.enabled；放在 RateLimit 之前，429 也会计数
6. RateLimit（可选）         ← RateLimit.enabled；CORS 已就位，429 带 ACAO
7. JWT Auth                 ← add_authentication() 时
8. 自定义 AfterAuth 槽（默认）← use_middleware* / DI 注册的 IMiddleware，此处可读到 claims
9. SPA                      ← 静态文件；跳过 /api/*，不吞 API 404
10. 自定义 AfterSpa 槽       ← use_middleware_at(.., MiddlewareSlot::AfterSpa)
11. Router / Endpoint       ← 框架内置（最终处理器）
```

## 中间件槽位

| 槽位 | 位置 | 用途 |
|------|------|------|
| `MiddlewareSlot::BeforeCors` | RequestId 之后、CORS 之前 | 需要观察所有请求（包括预检、被 CORS 短路）的中间件 |
| `MiddlewareSlot::AfterAuth` | JWT 之后、SPA 之前（**默认**） | 需要读 `ctx.claims()` 的鉴权/租户/审计中间件 |
| `MiddlewareSlot::AfterSpa` | 静态文件之后、Router 之前 | 依赖“静态文件已处理”的兜底钩子 |

```rust
use webx::MiddlewareSlot;

Host::builder()
    .use_middleware::<TenantGuard>()                              // 默认 AfterAuth：能拿到身份
    .use_middleware_at::<PreflightAudit>(MiddlewareSlot::BeforeCors)
    .use_middleware_with_at(MiddlewareSlot::AfterSpa, || Arc::new(Fallback::new()))
    .build();
```

直接注册进 DI 容器的 `IMiddleware`（如 `svc.add_middleware::<T>()`）也会进入默认的
`AfterAuth` 槽，排在 builder 注册的中间件之后。

## 原则

| 原则 | 说明 |
|------|------|
| 安全头最外层 | SecurityHeaders 覆盖包括错误响应在内的所有响应 |
| CORS 最靠外（除安全头/RequestId） | 413/429/401 等错误响应也要带 CORS 头，浏览器才读得到 |
| Metrics 在 RateLimit 之前 | 被限流的请求同样计入指标 |
| 认证在自定义业务中间件之前 | 默认槽在 JWT 之后，`ctx.claims()` 非空 |
| JWT 在 SPA 之前 | API 请求先经过认证中间件 |
| SPA 不处理 `/api/*` | 未匹配的 API 路径由 Router 返回 404，而非 index.html |
| 日志尽可能早 | `BeforeCors` 槽可记录预检等会被短路的请求 |

## 认证与授权

`add_authentication()` 注册 **JWT 中间件**（位于 SPA 之前）。路由级 `#[authorize]` 授权在
endpoint 内执行——路由匹配后规则可用，不需要额外的策略开关。授权语义详见
[资源授权](../09-auth-security/resource-authorization.md)。

## 小结

中间件顺序影响行为正确性。使用 `HostBuilder` 内置方法与 `MiddlewareSlot` 显式槽位，
可避免大部分排序问题；默认槽在认证之后，是“能拿到身份”的默认位置。

下一章：[中介者与事件](../08-mediator-events/INDEX.md)

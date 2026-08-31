# 中间件编排策略

## 推荐顺序

```
1. RequestId / Tracing     ← 最早，为所有请求生成 ID
2. SecurityHeaders         ← 安全头
3. CORS                    ← 预检请求可能在此短路
4. RateLimit               ← 限流
5. Metrics                 ← 指标
6. 自定义业务中间件         ← 通过 DI 注册 IMiddleware
7. JWT Auth                ← 认证（设置 claims）
8. SPA                     ← 静态文件；跳过 /api/*，不吞 API 404
9. Router / Endpoint       ← 框架内置（最终处理器）
```

## 原则

| 原则 | 说明 |
|------|------|
| 认证在授权之前 | JWT 必须先解析 claims |
| 限流在业务之前 | 尽早拒绝过载请求 |
| JWT 在 SPA 之前 | API 请求先经过认证中间件 |
| SPA 不处理 `/api/*` | 未匹配的 API 路径由 Router 返回 404，而非 index.html |
| 日志尽可能早 | 记录所有请求包括被拒绝的 |

## add_authentication() 行为

`add_authentication()` 注册 **JWT 中间件**（在 SPA 之前）。路由级授权通过 `#[authorize]` 在 `StubEndpoint` 内检查；动态授权通过 DI 注册的 `IDynamicAuthorizer` 实现。

> **注意：** `ResourceAuthorization` 中间件存在但未在 `HostBuilder` 中自动注册。详见 [资源授权](../09-auth-security/resource-authorization.md)。

## 小结

中间件顺序影响行为正确性。使用 `HostBuilder` 内置方法可避免大部分排序问题。

下一章：[中介者与事件](../08-mediator-events/INDEX.md)

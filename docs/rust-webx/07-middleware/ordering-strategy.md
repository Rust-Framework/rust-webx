# 中间件编排策略

## 实际执行顺序

`Host::build()` 按以下顺序把中间件装入管道：

```
1. SecurityHeaders         ← 安全头
2. RequestId               ← 为所有请求生成 ID
3. RateLimit（可选）        ← RateLimit.enabled 为 true 时
4. Metrics（可选）          ← Metrics.enabled 为 true 时
5. 自定义业务中间件          ← 通过 DI 注册的 IMiddleware
6. CORS                    ← 预检请求可能在此短路
7. JWT Auth                ← add_authentication() 时
8. SPA                     ← 静态文件；跳过 /api/*，不吞 API 404
9. Router / Endpoint       ← 框架内置（最终处理器）
```

## 原则

| 原则 | 说明 |
|------|------|
| 安全头最外层 | SecurityHeaders 覆盖包括错误响应在内的所有响应 |
| 认证在授权之前 | JWT 必须先解析 claims |
| 限流在业务之前 | 尽早拒绝过载请求 |
| CORS 在 Auth 之前 | 预检请求不带令牌，需要先被处理 |
| JWT 在 SPA 之前 | API 请求先经过认证中间件 |
| SPA 不处理 `/api/*` | 未匹配的 API 路径由 Router 返回 404，而非 index.html |
| 日志尽可能早 | 记录所有请求包括被拒绝的 |

## 认证与授权

`add_authentication()` 注册 **JWT 中间件**（位于 SPA 之前）。路由级授权通过 `#[authorize]`
在 endpoint 内检查；基于路由元数据的资源授权用 `.use_resource_authorization()` 启用，
构建期生成 `ResourceAuthorization` 策略。授权语义详见 [资源授权](../09-auth-security/resource-authorization.md)。

## 小结

中间件顺序影响行为正确性。使用 `HostBuilder` 内置方法可避免大部分排序问题。

下一章：[中介者与事件](../08-mediator-events/INDEX.md)

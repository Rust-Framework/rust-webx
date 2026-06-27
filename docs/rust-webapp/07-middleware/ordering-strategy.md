# 中间件编排策略

## 推荐顺序

```
1. RequestId / Tracing     ← 最早，为所有请求生成 ID
2. SecurityHeaders         ← 安全头
3. CORS                    ← 预检请求可能在此短路
4. RateLimit               ← 限流
5. Compression             ← 压缩
6. JWT Auth                ← 认证（设置 claims）
7. Resource Auth           ← 授权（检查 claims）
8. 自定义业务中间件
9. SPA                     ← 最后，非 API 路径 fallback
10. Router / Endpoint      ← 框架内置
```

## 原则

| 原则 | 说明 |
|------|------|
| 认证在授权之前 | JWT 必须先解析 claims |
| 限流在业务之前 | 尽早拒绝过载请求 |
| SPA 在路由之前 | 静态文件优先于 API 路由 |
| 日志尽可能早 | 记录所有请求包括被拒绝的 |

## add_authentication() 自动编排

`add_authentication()` 自动按正确顺序注册 JWT + Resource Auth 中间件，无需手动排序。

## 小结

中间件顺序影响行为正确性。使用 `HostBuilder` 内置方法可避免大部分排序问题。

下一章：[中介者与事件](../08-mediator-events/INDEX.md)

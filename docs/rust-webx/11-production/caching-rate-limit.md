# 缓存与速率限制

## 内存缓存

```rust
Host::builder().add_memory_cache()
```

Handler 通过 `Arc<MemoryCache>` 或 `IDistributedCache` trait 注入使用。

## 速率限制

### appsettings（推荐）

```json
{
  "RateLimit": {
    "Enabled": true,
    "RequestsPerSecond": 20,
    "BurstSize": 40,
    "MaxTrackedIps": 10000
  }
}
```

框架在 `Host::build()` 时读取 `RateLimit` 节，启用后自动注册 `RateLimitMiddleware`（无需手写 `use_middleware`）。

| 字段 | 说明 | 默认 |
|------|------|------|
| `Enabled` | 是否启用 | `false` |
| `RequestsPerSecond` | 每个客户端 IP 的 sustained 速率 | `100` |
| `BurstSize` | 突发容量 | `200` |
| `MaxTrackedIps` | 最大跟踪 IP 数（最旧淘汰） | `10000` |
| `TrustProxy` | 采信 `X-Forwarded-For` / `X-Real-IP`，仅在反向代理会覆盖这两个头时开启 | `false` |
| `TrustedProxies` | 可信代理的 CIDR 列表（如 `["10.0.0.0/8", "192.168.1.0/24"]`）；只有直连对端落在此列表内才采信转发头，客户端 IP 取 `X-Forwarded-For` 中最右侧的**非可信代理** hop | `[]` |

桶的键来自连接的**对端地址**（`IHttpRequest::remote_addr`，由 host 在 accept 时记录）。
`TrustProxy=true` 但 `TrustedProxies` 为空时框架会忽略转发头并打 `warn`——否则客户端可以轮换
`X-Forwarded-For` 绕过限流；反之 `false` 且框架拿不到对端地址时，框架不会把所有人放进
同一个桶，而是跳过限流并打 `debug` 日志。

环境变量示例：`APP__RateLimit__Enabled=true`

### 代码注册（可选）

```rust
Host::builder()
    .use_middleware_with(|| {
        Arc::new(RateLimitMiddleware::new(10.0, 20)) as Arc<dyn IMiddleware>
    })
```

超限返回 RFC 7807 `429` + `application/problem+json`，并携带 `Retry-After`（按 `RequestsPerSecond` 推出的整秒等待时间）。

## 指标

```json
{
  "Metrics": {
    "Enabled": true
  }
}
```

启用后暴露 `GET /metrics`（Prometheus text 格式），并自动记录 2xx/4xx/5xx 计数。

## 小结

`add_memory_cache()` 一行启用缓存；`RateLimit` / `Metrics` 配置节用于生产防护与可观测性。

下一节：[OpenAPI 与 SPA 托管](openapi-spa.md)

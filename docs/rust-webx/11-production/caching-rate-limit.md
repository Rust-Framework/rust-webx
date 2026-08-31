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
| `RequestsPerSecond` | 每 IP  sustained 速率 | `100` |
| `BurstSize` | 突发容量 | `200` |
| `MaxTrackedIps` | 最大跟踪 IP 数（LRU 淘汰） | `10000` |

环境变量示例：`APP__RateLimit__Enabled=true`

### 代码注册（可选）

```rust
Host::builder()
    .use_middleware_with(|| {
        Arc::new(RateLimitMiddleware::new(10.0, 20)) as Arc<dyn IMiddleware>
    })
```

超限返回 RFC 7807 `429` + `application/problem+json`。

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

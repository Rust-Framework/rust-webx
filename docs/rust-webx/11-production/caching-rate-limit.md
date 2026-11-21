# 缓存与速率限制

## 内存缓存

```rust
Host::builder().add_memory_cache()
```

Handler 通过 `Arc<MemoryCache>` 或 `IDistributedCache` trait 注入使用。

## Redis 分布式缓存

多实例部署时用 `rust-webx-redis` 把缓存放到共享 Redis；同一个连接也承载事件传输
（见[事件发布与订阅](../08-mediator-events/event-pub-sub.md)）。

```toml
[dependencies]
rust-webx-redis = "2.5"
```

```rust
use std::time::Duration;
use webx::*;
use webx_redis::{RedisHostBuilderExt, RedisOptions};

Host::builder()
    .add_redis(
        RedisOptions::new("redis://127.0.0.1:6379/0")
            .with_prefix("app:")
            .with_default_ttl(Duration::from_secs(300))
            .with_sliding_ttl(Duration::from_secs(60)),
    )
    .build();
```

`add_redis` 一次注册 `RedisCache`（具体类型）与 `Arc<dyn IDistributedCache>` 端口；Handler 依赖端口，
不依赖具体实现：

```rust
use std::sync::Arc;

#[derive(Inject)]
pub struct ProductHandler {
    #[inject]
    cache: Arc<dyn IDistributedCache>,
}
```

`get` 返回 `Option<Vec<u8>>`；未命中回源后用 `set(key, bytes, None)` 写回（`None` 表示使用默认
TTL）。

- **前缀**：`with_prefix` 追加到每个缓存键前，等价 ASP.NET Core 的 `InstanceName`；多个应用共用一个
  Redis 库时必须设置。
- **TTL**：`with_default_ttl` 是默认绝对过期时间；单次 `set` 仍可用 `DistributedCacheEntryOptions`
  覆盖。`with_sliding_ttl` 在 `get`/`refresh` 命中时滑动续期，实际剩余时间取滑动窗口与绝对截止时间
  的较小者（读取会在 Redis 上多一次 `PEXPIRE`）。
- **懒连接与超时**：注册本身不连接 Redis；第一次缓存/事件/健康检查操作才建连（受
  `connect_timeout_seconds` 限制），之后由连接管理器自动重连；每条命令还受
  `command_timeout_seconds`（默认 5s）约束，Redis 卡住不会永久占用请求或消费者。因此 Redis
  暂时不可用不会让 Host 启动失败：缓存操作返回 `CacheError::Unavailable`，事件发布返回可重试
  的 503。
- **单节点**：URL 是单节点端点（`redis://` / `rediss://`，`rediss` 已启用 rustls）；Redis
  Cluster / Sentinel 需要独立适配器，端口 `IDistributedCache` / `IEventPublisher` 不变。
- **健康检查**：通过 `HostBuilder::add_redis` 注册时自动挂 `redis` 健康检查（`PING`），失败让
  `/health/ready` 返回 503。只做缓存、不发事件时用 `.cache_only()`，不注册事件发布器/消费者。

> **13 字节信封**：缓存值前有 13 字节头（版本 1 + 8 字节绝对过期时间 + 4 字节滑动窗口），由
> `RedisCache` 编解码。非本适配器写入的裸值会原样返回（不做过期判断）；反过来，其他客户端直接读
> 这些键会看到 13 字节前缀，跨语言共享缓存时应自行包一层协议。`clear()` 按 `key_prefix` 做
> `SCAN MATCH`；**没有前缀时 `clear()` 是 no-op**（避免清空整个库），请始终设置前缀。

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

超限返回 RFC 9457 `429` + `application/problem+json`，并携带 `Retry-After`（按 `RequestsPerSecond` 推出的整秒等待时间）。

### 分布式限流

多实例部署时，内存桶各自计数会让实际限额翻倍。框架把计数存储抽成端口 `IRateLimiter`：

```rust
use webx::IRateLimiter;

struct RedisLimiter { /* Redis 连接 / Lua 脚本 */ }

#[async_trait::async_trait]
impl IRateLimiter for RedisLimiter {
    async fn check(&self, key: &str) -> webx::RateLimitDecision {
        // 在共享存储上原子扣减令牌，返回 allowed / retry_after_secs
        webx::RateLimitDecision::allow()
    }
}

Host::builder().use_middleware_with(|| {
    Arc::new(RateLimitMiddleware::with_limiter(Arc::new(RedisLimiter { /* ... */ })))
        as Arc<dyn IMiddleware>
})
```

默认实现 `InMemoryRateLimiter` 按 CPU 并行度分片（1–32 片），把原先的单全局 `Mutex` 从
热路径移除；`MaxTrackedIps` 仍是全局上限。分片内淘汰是近似的：极端 key 倾斜时某个分片可能
提前淘汰，若需要严格的每键保留语义，请用共享存储实现端口。

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

`add_memory_cache()` 一行启用进程内缓存；多实例用 `add_redis(...)` 换共享缓存与事件传输；
`RateLimit` / `Metrics` 配置节用于生产防护与可观测性。

下一节：[OpenAPI 与 SPA 托管](openapi-spa.md)

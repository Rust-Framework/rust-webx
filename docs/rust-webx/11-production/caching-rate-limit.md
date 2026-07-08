# 缓存与速率限制

## 内存缓存

```rust
Host::builder().use_memory_cache()
```

注册 `MemoryCache` 为 `IDistributedCache` 实现。

### 在 Handler 中使用

```rust
use rust_webx::{IDistributedCache, DistributedCacheExtensions};

let cached = self.cache.get_or_create("user:123", async {
    self.repo.find("123").await
}).await?;
```

## 速率限制

```rust
svc.add_middleware::<RateLimitMiddleware>()
```

基于令牌桶算法限制请求频率，防止 API 滥用。

## 缓存策略

| 场景 | 策略 |
|------|------|
| 读多写少的配置 | 长 TTL 缓存 |
| 用户 Session | 中等 TTL + 主动失效 |
| 实时数据 | 不缓存 |

## 小结

`use_memory_cache()` 一行启用缓存；速率限制保护 API 免受滥用。

下一节：[OpenAPI 与 SPA 托管](openapi-spa.md)

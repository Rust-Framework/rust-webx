# 精确响应缓存

对确定性请求（`temperature == 0`、非流式）缓存完整响应，减少上游调用与成本。缓存键含租户，
不会跨租户复用。

```rust
use std::sync::Arc;
use std::time::Duration;
use webx_ai::{GatewayExecutor, MemoryGatewayCache};

let cache = Arc::new(MemoryGatewayCache::new(1024));
let executor = GatewayExecutor::new(catalog, router)
    .with_cache(cache)
    .with_cache_ttl(Duration::from_secs(60))
    .with_single_flight();
```

## 什么会缓存

- 非流式（`stream == false`）；
- `temperature == Some(0.0)`；
- key = `SHA256(tenant_id + NUL + CanonicalRequest 序列化)`，包含 model、messages、tools、
  采样参数与 extensions。

其余请求不生成 key，直接走上游。

## 命中语义

- 命中直接返回 `CanonicalResponse`；
- **不调用 provider、不预扣配额、不写 usage**；
- 请求指标 status=`cache_hit`，并记 `ai_gateway_cache_hits_total`；
- 未命中记 `ai_gateway_cache_misses_total`，成功后 `put`；
- 失败响应不缓存。

## TTL 与容量

- 默认 TTL 60s；`with_cache_ttl` 覆盖。
- `MemoryGatewayCache::new(max_entries)` 有容量上限；`put` 时清理过期项，仍满则驱逐一个旧项。
- 过期项在 `get`/`put` 时惰性清理，没有后台任务。

## 单飞

`with_single_flight()` 后，同 key 的并发请求共享一次 `complete_uncached`：

- 第一个请求初始化 `OnceCell`，其余等待同一结果（成功或错误都广播）；
- flight 完成后从 map 移除；
- 只对可缓存 key 生效（非流 + `temperature==0`）；
- 避免缓存击穿时的上游雪崩，但不改变缓存 key 契约。

## 边界

- 已实现：精确 key、租户隔离、温度/流式过滤、内存缓存、命中指标、执行器接线、单飞。
- 未实现：Redis 分布式缓存、按租户/模型策略开关、SSE 帧重放、单飞超时/取消指标。
- 不要缓存含随机性（temperature > 0）或依赖外部状态的请求；不要把 API key/tenant 明文放进
  缓存键（已哈希，tenant 只参与哈希）。
- prompt caching 是 provider 能力，透传即可，不要和网关缓存混用。

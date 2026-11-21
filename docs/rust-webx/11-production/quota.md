# 配额与预扣-结算

IP 限流（`RateLimit` 配置节）保护进程不被单个来源打满；**配额**回答的是业务问题：
"这个租户/API key/模型每分钟能发多少请求、多少 token、几条并发流、花多少钱"。

配额不能"先检查、后扣费"：两个并发请求会同时通过检查，造成超卖。`rust-webx-ai` 提供
**预扣-结算**：

```text
reserve（扣估算 + 占并发槽） → 调用上游 → settle（真实 usage 多退少补）
                                     └→ 失败且未产生用量：release（全额退回）
```

## 限额

```rust
use std::sync::Arc;
use webx_ai::{MemoryQuotaStore, QuotaKey, QuotaLimits, StaticQuotaPolicy};

let key = QuotaKey::new("tenant-1", "key-1", "fast");

let limits = QuotaLimits::default()
    .requests_per_minute(500)
    .tokens_per_minute(200_000)
    .tokens_per_day(5_000_000)
    .concurrent_streams(8)
    .budget_micros(50_000_000); // 50 USD

let policy = StaticQuotaPolicy::new(QuotaLimits::unlimited())
    .with_limits(key.clone(), limits);
let store = MemoryQuotaStore::new(Arc::new(policy));
```

`QuotaLimits` 每项都是 `Option`：`None` = 无限。`IQuotaPolicy` 负责解析某个 key 的限额；
生产实现应走缓存（LRU/版本失效），不要在热路径每请求查库。

## 调用顺序

```rust
use webx_ai::{IQuotaStore, ReserveRequest, Usage};

// 1. 入口：按估算预扣（prompt + 预计 completion；流式再加一个并发槽）
let reservation = store.reserve(
    ReserveRequest::new(key.clone(), 1_200)
        .streams(1)                       // 非流式用 0
        .estimated_cost_micros(3_000)     // 可选：预算维度
).await?;

// 2. 上游成功：用真实 usage / 成本结算（多退少补，释放并发槽）
store.settle(reservation, Usage::new(1_050, 380), 2_900).await?;

// 3. 上游未开始就失败：全额退回
// store.release(reservation).await?;
```

- `settle`/`release` 按 `lease_id` **幂等**；调用两次不会重复扣/退。
- `settle` 允许实际用量超过限额（请求已经发生）；超卖由下一次 `reserve` 拒绝，账不能丢。
- 租约默认 60s；到期只释放并发槽，估算的 token/成本保持计费（保守，不低估）。
- 断连时在流的完成回调里 `settle`（见 [流式响应 SSE](../05-request-pattern/streaming-sse.md)）。

## 超限响应

`QuotaError` 直接带映射信息：

| 变体 | `http_status()` | 说明 |
|------|-----------------|------|
| `Exceeded { dimension, limit, retry_after }` | 429 | `retry_after` 写 `Retry-After` |
| `Unavailable` | 503 | 配额存储不可用 |
| `Invalid` | 400 | 请求本身不合法 |

OpenAI 兼容表面再把 429 转成 `insufficient_quota` 信封（G8 的错误写入器）。

```rust
match store.reserve(request).await {
    Ok(reservation) => { /* 继续调用上游 */ }
    Err(error) if error.is_exceeded() => {
        let status = error.http_status();
        let retry = error.retry_after();
        // 返回 429 + Retry-After；不要泄露具体 key
    }
    Err(error) => { /* 503/400，按 http_status() 映射 */ }
}
```

## 实现与边界

- 当前实现 `MemoryQuotaStore`：单进程、固定分钟窗口 + UTC 日窗口、`std::sync::Mutex<HashMap>`（临界区无 I/O）。
- 多实例部署必须实现 `IQuotaStore`（Redis Lua 原子脚本 / 数据库事务）；端口不变。
- 配额与 IP 限流是两层：先过进程保护，再过业务配额。
- 不要用 `settle` 之后的值去"修正"已经发给客户端的额度；额度以 `reserve` 的原子结果为准。

单测覆盖 RPM/TPM/TPD/并发/预算、结算多退少补、释放退款、幂等、租约过期与按 key 覆盖
（`crates/ai/core/src/quota.rs`）。

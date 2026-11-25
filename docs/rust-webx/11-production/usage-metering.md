# 用量计量与账本

AI 网关按 token 计费，所以**每个结束的请求都必须恰好产生一条用量事件**，无论它是正常完成、
客户端断开，还是上游失败。`rust-webx-ai` 提供事件模型、写入端口、幂等内存实现与聚合函数：

```text
请求结束（on_close） → UsageEvent → IUsageSink::record（按 request_id 幂等）
                                              └→ 账本 → outbox → 计费/对账
                                              └→ rollup_events（报表）
```

## 事件

```rust
use webx_ai::{Usage, UsageEvent, UsageStatus};

let event = UsageEvent::new(
    request_id,                    // 幂等键
    tenant_id,
    key_id,
    model,                         // 对外模型/别名
    provider,                      // 实际上游
    Usage::new(prompt_tokens, completion_tokens),
    UsageStatus::Completed,        // Completed | ClientGone | UpstreamError
)
.with_cost(cost_micros)            // 整数 micro-USD
.with_latency(latency_ms)
.with_ttft(ttft_ms)
.with_timestamp(unix_secs);        // 0 时由 sink 盖章
```

- `Usage` 有 4 个桶：prompt / completion / cached / reasoning；`total_tokens()` 是 saturating 求和。
- 金额一律整数 micro-USD（1 USD = 1_000_000），**不要用浮点**。
- 执行器用 `ICostCalculator`/`StaticCostTable` 计算成本；cached/reasoning 桶单独计价，
  不重复计入 base token。成本进入配额结算、`UsageEvent.cost_micros` 与
  `ai_gateway_cost_micros_total`。
- 断连时在流的 `StreamEnd::ClientDisconnected` 回调里估算并写入 `ClientGone`。

## 写入

```rust
use webx_ai::{IUsageSink, MemoryUsageSink};

let sink = MemoryUsageSink::new();
sink.record(event).await?;
```

生产实现应满足：

- `request_id` 唯一索引 + `ON CONFLICT DO NOTHING`（或等价 upsert-ignore）。
- 写失败记录日志并返回 `UsageError::Unavailable`（→ 503），但**不要让已经完成的响应回滚**。
- 与业务写在同一事务里时只写事件；外发计费走 outbox，不阻塞请求。

`MemoryUsageSink` 是单进程实现（测试/小部署）：首写优先、重复 `request_id` 忽略、`created_at`
为 0 时自动盖章。

## 估算

上游通常只在最后一个 chunk 给出 usage；缺失时用 tokenizer 兜底：

```rust
use webx_ai::{HeuristicTokenizer, ITokenizer};

let tokenizer = HeuristicTokenizer::default(); // 4 bytes/token
let estimate = tokenizer.count(model, prompt);
```

`HeuristicTokenizer` 只是数量级估算（英文约 4 bytes/token；CJK/emoji 差异大）。需要精确计价时
换真实 tokenizer 表；权威值始终以上游 usage 为准。

## 持久化账本（参考适配器）

`examples/adapters/ai/ef`（`publish = false`，不随框架发布）提供幂等的 `EfUsageSink` 参考实现，
实体表 `ai_usage_events`；复制进业务仓、改成自己的包名后使用（下面的 `webx_ai_ef` 是参考实现导入名）：

```rust
use std::sync::Arc;
use webx_ai_ef::EfUsageSink;

let sink = EfUsageSink::from_options(Arc::clone(&db_options));
let executor = GatewayExecutor::new(catalog, router)
    .with_usage(Arc::new(sink));
```

- `request_id` 是主键；先查再插，重复结算直接返回；并发唯一冲突
  （`EFError::UniqueViolation`）也按首写优先处理。
- 每次 `record` 新建一个 `DbContext`（不跨请求持有 scoped unit-of-work）。
- `created_at_unix_secs == 0` 时在转换时盖章。
- 账本与 outbox 在同一个 `save_changes` 写入；`without_outbox()` 可关闭 outbox。
- 表由 `DbContext::ensure_created()`（测试）或迁移创建。

## Outbox 与 rollup

```rust
use webx_ai_ef::{EfUsageOutbox, EfUsageRollup};

let outbox = EfUsageOutbox::from_options(Arc::clone(&db_options));
for row in outbox.pending(100).await? {
    match send_to_billing(&row).await {
        Ok(()) => { outbox.mark_delivered(&row.request_id).await?; }
        Err(error) => { outbox.mark_failed(&row.request_id, &error.to_string()).await?; }
    }
}

let rollup = EfUsageRollup::from_options(Arc::clone(&db_options));
let rows = rollup.rollup_window(window_start, 60).await?; // 幂等
```

- `pending` 只返回 `delivered=false` 且 `next_attempt_at<=now` 的行。
- `mark_failed` 按尝试次数指数退避（30s · 2^n，封顶），并记录截断后的错误。
- `rollup_window` 从账本读 `[start, start+60)`，调用纯函数 `rollup_events`，按
  `(window_start, tenant, model)` 幂等插入 `ai_usage_rollups`。
- 后台 worker 已提供：`UsageOutboxWorker`（批量投递 + ack/退避）与 `UsageRollupWorker`
  （每 tick 聚合上一窗口），实现 `PeriodicBackgroundService`，用
  `add_background_service(BackgroundServiceSpec::with_factory(..))` 注册即可。
- 后台 worker 的典型装配：

```rust
use webx::BackgroundServiceSpec;

let outbox_worker = UsageOutboxWorker::new(outbox, handler, 100, Duration::from_secs(5));
let rollup_worker = UsageRollupWorker::new(rollup, Duration::from_secs(60), Duration::from_secs(30));

Host::builder().register(move |svc| {
    let outbox_worker = outbox_worker.clone();
    let rollup_worker = rollup_worker.clone();
    svc.add_background_service(BackgroundServiceSpec::with_factory(
            move |_| Arc::new(outbox_worker.clone())))
        .add_background_service(BackgroundServiceSpec::with_factory(
            move |_| Arc::new(rollup_worker.clone())))
})
```

## 聚合

```rust
use webx_ai::rollup_events;

let windows = rollup_events(events, 60); // 按 60s 窗口、tenant+model 分组
```

`rollup_events` 是纯函数：固定窗口、`requests`/`usage`/`cost_micros` 求和。报表 worker 可以
按分钟/小时调用它；不要在请求路径里做聚合。

## 边界

- 已有：框架内存 sink（`IUsageSink` 端口在 `rust-webx-ai`）、参考适配器的 EF 账本/outbox/rollup 与后台 workers。
- 事件是追加写；修正用量应写冲正事件，不要原地改历史。
- 不要把 prompt/response 文本写进事件；只保留计量与标识字段。

单测覆盖事件 serde、token 桶、saturating、estimator、sink 幂等/盖章/空 id、rollup 分组与窗口
（`crates/ai/core/src/usage.rs`）；EF 账本/outbox/rollup 与 workers（`examples/adapters/ai/ef/src/{lib,worker}.rs`，
8 个 SQLite 测试，用 `cargo test --workspace --manifest-path examples/adapters/Cargo.toml` 运行）。

# 韧性：重试、熔断与舱壁

网关的上游会超时、限流、间歇 5xx。`rust-webx-ai` 把三件事拆成可组合的小原语，执行器负责接线：

```rust
use std::sync::Arc;
use std::time::Duration;
use webx_ai::{Bulkhead, CircuitBreaker, GatewayExecutor, RetryPolicy};

let executor = GatewayExecutor::new(catalog, router)
    .with_retry_policy(
        RetryPolicy::default()
            .with_max_attempts(3)
            .with_base_delay(Duration::from_millis(100))
            .with_max_delay(Duration::from_secs(2))
            .with_jitter(true),
    )
    .with_circuit_breaker(Arc::new(CircuitBreaker::new(5, Duration::from_secs(10))))
    .with_bulkhead(Arc::new(Bulkhead::new(64, Duration::from_millis(50))));
```

## 重试

- 只重试 `ProviderError::is_retryable()`：连接失败、超时、429、5xx；`InvalidRequest`/
  `Unauthorized`/`Protocol` 不重试。
- 退避 `min(base * 2^(n-1), max)`，默认加 50–100% jitter；`Retry-After` 优先。
- 最多 `max_attempts` 次（执行器还会被 `RoutePlan` 的候选数截断）。
- sleep 前检查请求 deadline；会越界就直接返回最后一个错误。
- 非流可以换候选；**流一旦返回就不再重试**（首字节规则）。

## 熔断

按 `(provider, model)` 计：

```
Closed --连续失败 >= threshold--> Open
Open --cooldown 到期--> HalfOpen（只放一个探针）
HalfOpen --探针成功--> Closed
HalfOpen --探针失败--> Open
```

执行器在调用前 `allow(&ModelKey)`；成功/失败回写。熔断打开时不会调用 provider，也不会产生
usage，直接返回 503 类错误。

## 舱壁

每个 provider 一个 `Semaphore`：

- `max_concurrent` 限制在飞请求；
- `max_wait` 内拿不到槽就快速失败（503 类），避免排队拖垮自己；
- 流式请求的 permit 会一直持有到流结束或客户端断开，防止“建流即释放”导致超卖。

## 与健康/配额的顺序

1. 路由 plan（能力/健康过滤）；
2. 配额预扣；
3. 熔断 `allow`；
4. 舱壁获取；
5. provider 调用；
6. 结算/计量。

熔断/舱壁拒绝在第 3/4 步就返回，不产生上游调用与 usage。

## 边界

- 已实现：重试/退避/jitter/`Retry-After`、熔断、舱壁、执行器接线与 6 个测试。
- 未实现：入站请求 deadline 的框架级取消、对冲（hedging）、熔断状态进 `/health/ready`。
- 不要在执行器外再写一套重试循环；否则会绕过首字节规则与配额结算。

# 分布式锁与租约（IAdvisoryLock）

跨实例互斥、配额原子性、单次任务去重、迁移/种子初始化互斥：这些场景需要"同一时刻只有一个
持有者"，而不是进程内 `Mutex`。框架把这件事做成端口 `IAdvisoryLock`，用**租约**而不是永久锁：

```text
try_acquire(key, ttl) -> Option<Lease>   冲突返回 None（不是错误）
acquire_with_timeout(key, ttl, wait)     最多等 wait：10ms 起、倍增、上限 200ms 退避；
                                         超时返回 Ok(None)，后端 Err 原样上抛
release(&lease)       -> bool            token 不匹配（已过期/被重新获取）返回 false
renew(&lease)         -> bool            长任务续期；丢失返回 false
```

## 选择实现

| 形态 | 实现 | 说明 |
|---|---|---|
| 测试 / 单进程 | `InMemoryAdvisoryLock` | 不跨进程；冲突语义与真实后端一致 |
| 多实例 | `rust-webx-redis` 的 `RedisAdvisoryLock` | `SET NX PX` + Lua compare-and-delete/pextend |
| 数据库 | 应用/rust-ef 侧实现同一 trait | PostgreSQL advisory lock 或 SQLite 单写者降级 |

```rust
use std::sync::Arc;
use std::time::Duration;
use webx::*;

// 单进程/测试
Host::builder().use_advisory_lock(Arc::new(InMemoryAdvisoryLock::new())).build();

// 多实例（rust-webx-redis）：键变成 {prefix}lock:{key}
use webx_redis::{RedisHostBuilderExt, RedisOptions};
Host::builder()
    .add_redis_advisory_lock(RedisOptions::new("redis://cache:6379").with_prefix("app:"))
    .build();
```

host **默认不注册**任何锁：进程内默认实现会让单机测试通过、上线后悄悄失去互斥。缺实现时
Handler 的 `Arc<dyn IAdvisoryLock>` 解析失败。

## Handler 内使用

```rust
#[derive(Inject)]
pub struct ConsumeQuotaHandler {
    #[inject]
    lock: Arc<dyn IAdvisoryLock>,
}

let Some(lease) = self
    .lock
    .acquire_with_timeout(
        &format!("quota:{}", req.tenant_id),
        Duration::from_secs(5),
        Duration::from_millis(200), // 短等待；不想等就传 Duration::ZERO
    )
    .await?
else {
    return Err(Error::too_many_requests(1, "quota is busy; retry later"));
};

let remaining = load_and_decrement().await?; // 临界区保持短：一次读-改-写
self.lock.release(&lease).await?;
```

- **等待策略由框架提供**：短等待用 `acquire_with_timeout`（10ms 起、倍增、上限 200ms；
  超时 `Ok(None)`；后端 `Err` 不重试、原样上抛；`wait = Duration::ZERO` 只尝试一次；
  取消时不会自动释放刚拿到的租约，靠 TTL 兜底）。业务不要自己写 `loop + sleep + retry`。
- **临界区要短**：租约只覆盖一次读-改-写或一次幂等检查；不要把用户等待、上游长调用放进去。
- **长任务用 `renew`**：`ttl` 内没结束就续期；丢失租约必须停止写入（防止两个持有者都写）。
- **冲突不是错误**：`Ok(None)` 表示别人持有；后端不可用才是 `Err`（可重试 503）。忙时返回
  `Error::too_many_requests(..)` 让引擎写 `Retry-After`，不要手写 `Status(429, ..)`。
- **TTL 必须大于临界区**：容器被 kill 后，过期租约可被重新获取，不会永久死锁。
- **优先原子 SQL**：单条 `UPDATE ... WHERE remaining > 0` 已经原子，不必加锁；只有跨多步的
  读-改-写才需要租约。

## 与配额组合（预扣-结算）

AI 网关的 `IQuotaStore`（见[配额与预扣-结算](quota.md)）在应用层做原子预扣；如果配额实现是
"读 → 判断 → 写"，用租约把这三步串起来：

```text
lease = try_acquire("quota:{tenant}", 2s)
  ?: 读取剩余额度（缓存/DB）
  ?: 判断并写入预扣
release(lease)
```

多实例下租约来自 Redis/DB，进程内实现只用于单节点与测试。

## 部署语义

- **多实例**：所有实例使用同一后端与同一 key 命名空间（统一 prefix）；租约过期给容器崩溃兜底。
- **容器**：无本地状态；滚动升级时持有者被杀，租约到点自动释放，无需人工清理。
- **性能**：acquire/release 各一次后端往返；不要在每次请求都加锁——只在真正需要互斥的路径上。

## 边界

- 不是事务：租约不参与数据库事务，临界区内失败自行回滚/补偿。
- 不保证公平：没有等待队列，`acquire_with_timeout` 只是有界退避重试；等待多久由调用方的
  `wait` 决定（`Duration::ZERO` = 单次尝试），抢不到时的业务结论（429 / 跳过 / 排队）由调用方给。
- 时钟：TTL 由后端（Redis `PX`）或进程时钟计算；跨实例以共享后端为准。
- `InMemoryAdvisoryLock` 的 `held_count()` 会清理过期项，仅用于诊断/测试。

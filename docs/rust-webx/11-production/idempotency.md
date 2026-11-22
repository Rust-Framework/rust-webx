# 请求级幂等（Idempotency-Key）

重试是常态：客户端超时重发、网关重放、用户双击。**幂等键**让状态变更请求最多执行一次，
重试拿回第一次的响应，并发重复请求得到 `409` 而不是重复扣款。

```text
POST /api/orders        Idempotency-Key: 3f9c2b1e-…
  第一次：执行 Handler，2xx 响应入存储
  重试：  同一响应重放，带 Idempotency-Replayed: true
  并发：  第二个请求 409 + Retry-After: 1
```

## 启用

配置节（默认关闭）：

```json
{
  "Idempotency": {
    "Enabled": true,
    "TtlSeconds": 86400,
    "MaxTrackedKeys": 10000,
    "MaxBodyBytes": 1048576
  }
}
```

或显式注入存储（多实例/自建存储时必须用这个）：

```rust
use std::sync::Arc;
use std::time::Duration;
use webx::*;

Host::builder()
    .use_idempotency(
        Arc::new(InMemoryIdempotencyStore::new()), // 单节点；多实例换成共享实现
        Duration::from_secs(24 * 3600),
    )
    .build();
```

显式调用优先于配置节；存储键由「主体 + 方法 + 路径 + 请求头」组成：

| 请求头 | 行为 |
|---|---|
| 没有 `Idempotency-Key` | 正常执行，不做幂等 |
| 非 `POST`/`PUT`/`PATCH`/`DELETE` | 不做幂等（GET 本来就幂等） |
| 键为空 / 超过 255 / 含不可见字符 | `400` + `problem+json` |
| 首次请求 | 执行；2xx 且非 `text/event-stream`、body ≤ `MaxBodyBytes` 时缓存响应 |
| 同键重试 | 重放状态码、`content-type`、`content-encoding`、body，附 `Idempotency-Replayed: true` |
| 同键并发 | `409 Conflict` + `Retry-After: 1` |
| Handler 返回错误 / 非 2xx / body 过大 / 流式响应 | 释放键，允许重试重新执行 |
| 主体不同 | 键空间隔离（用户 A 的键不会命中用户 B 的响应） |

匿名请求以客户端 IP 作为主体；没有 IP 也没有身份时不启用幂等（无法区分调用方）。

## 实现端口

```rust
use webx_core::idempotency::{IdempotencyBegin, IdempotentResponse, IIdempotencyStore};

#[async_trait::async_trait]
impl IIdempotencyStore for MyRedisStore {
    async fn begin(&self, key: &str, ttl: Duration) -> Result<IdempotencyBegin> { /* SET NX + 读响应 */ }
    async fn complete(&self, key: &str, response: IdempotentResponse, ttl: Duration) -> Result<()> { /* 存响应 */ }
    async fn release(&self, key: &str) -> Result<()> { /* DEL 预约 */ }
}
```

三态协议：`Started`（执行并 `complete`/`release`）、`InProgress`（409）、`Replay(bytes)`。
存储实现只需保证 `begin` 的 `SET NX` 原子性；中间件负责其余状态。

## 部署语义

- **单进程**：`InMemoryIdempotencyStore`（有界 + TTL 惰性清理），零外部依赖。
- **多实例**：重试可能落到另一节点，必须用共享存储（Redis `SET NX` + 响应值，或数据库唯一键
  + 响应列）；只在一台机器上装内存存储等于没装。
- **容器**：预约带 TTL，实例被杀后下一次重试可重新执行；不会留下本地状态。
- **性能**：每个带键的写请求多两次存储往返；响应 body 以字节缓存，受 `MaxBodyBytes` 与
  `MaxTrackedKeys` 双重约束。

## 边界

- **不是唯一约束的替代**：数据库唯一索引仍是最终防线；幂等键只保证"同一键最多一次"。
- **不比较请求体**：同键但不同的 body 会重放第一次响应（标准的键复用语义）。客户端要改内容
  必须换新键；需要参数一致性校验时，在应用侧比较请求摘要并返回 409。
- 只对状态变更方法生效；GET/HEAD/OPTIONS 不缓存。
- 流式响应（SSE）不缓存：返回后键被释放，重试会重新执行。
- 幂等键不做长期审计：`TtlSeconds` 内有效，过期后同一键会重新执行。

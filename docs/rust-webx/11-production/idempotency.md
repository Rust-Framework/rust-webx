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
        Arc::new(InMemoryIdempotencyStore::new()), // 单节点；多实例改用 EfIdempotencyStore（见下文）
        Duration::from_secs(24 * 3600),
    )
    .build();
```

多实例部署用数据库持久化实现（`rust-webx-ef`），见「数据库持久化（多实例）」一节。

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

## 数据库持久化（多实例）

内存存储只在单进程内可见：多实例部署下重试落到另一节点会再执行一次 Handler。
`rust-webx-ef` 提供 `IIdempotencyStore` 的 rust-ef 实现 `EfIdempotencyStore`，把预约与
响应存进数据库表 `webx_idempotency`（主键 = 幂等键），所有实例共享同一份状态。

### 建表与迁移

表结构由实体（`webx_ef::idempotency::IdempotencyRecord`）声明，DDL 交给官方 rust-ef
迁移工具生成，不要手写 `CREATE TABLE`：

```rust
use rust_ef::db_context::{DbContext, DbContextOptionsBuilder};
use webx_ef::idempotency::register_idempotency_model;

let mut db = DbContextOptionsBuilder::new();
db.use_postgres(&database_url); // 或 use_mysql / use_sqlite
let options = std::sync::Arc::new(db.build());
options.create_provider().expect("database provider"); // 启动期快速失败

let mut ctx = DbContext::from_options(&options)?;
ctx.set::<Order>();                   // 应用自己的实体
register_idempotency_model(&mut ctx); // 把 webx_idempotency 加进模型
ctx.migrate("Migrations").await?;     // 已有库升级；新库首装用 ctx.bootstrap()
```

主键是完整协议键的 SHA-256 摘要（`key_hash`，64 hex）：中间件的协议键 = 主体 + 方法 + 路径 +
客户端 `Idempotency-Key`（≤255 字符），路径没有长度上限，所以固定宽度的摘要才是安全的索引列；
完整键另存在 `key` 列（TEXT，仅供排障，不加索引）。`expires_at` 上的 `#[index]` 服务于清理扫描。

### 装配

`use_idempotency` 要求在 host build 之前拿到 store 实例，而 `EfIdempotencyStore::new`
接收应用的 `Arc<DbContextOptions>`（不是 DI 的 `ScopeFactory`），所以顺序是：先建 options、
注册模型并迁移，再建 store，最后交给 `HostBuilder`。宿主 DbContext 注册使用同一份
options，provider/连接池/模型缓存共享：

```rust
use std::sync::Arc;
use std::time::Duration;
use webx::*;
use webx_ef::idempotency::{EfIdempotencyStore, IdempotencyHousekeeping};

let store = Arc::new(EfIdempotencyStore::new(Arc::clone(&options)));
let housekeeping = IdempotencyHousekeeping::new(Arc::clone(&store));

Host::builder()
    .register({
        let options = Arc::clone(&options);
        move |svc| {
            svc.scoped::<DbContext>(move |_| {
                Arc::new(DbContext::from_options(&options).expect("DbContext"))
            })
        }
    })
    .use_idempotency(Arc::clone(&store), Duration::from_secs(24 * 3600))
    .register(move |svc| {
        svc.add_background_service_with(move |_| Arc::new(housekeeping.clone()))
    })
    .build();
```

`Idempotency` 配置节仍只启用单进程内存存储；多实例必须显式注入本实现。

### 清理服务

`IdempotencyHousekeeping` 实现 `PeriodicBackgroundService`：默认每小时 tick 一次，
只删除「过期超过 24 小时」的行（`with_interval` / `with_grace` 可调，默认
`DEFAULT_CLEANUP_INTERVAL` / `DEFAULT_CLEANUP_GRACE`）。tick 是一条
`WHERE expires_at < now - grace` 的索引 `DELETE`；多个实例同时清理是安全的
（同一条过期行删两次是 no-op）。

### TTL 与 lease 语义

- **TTL 是租约**：Handler 必须在 TTL 内完成并 `complete`，否则租约到期后重试可以抢占该键，
  两个 Handler 可能并发执行。TTL 取值参考最慢 Handler 的 P99（要留余量）。
- 实例被杀 → 预约行到期 → 下一次重试 `begin` 用条件 UPDATE 抢占
  （`WHERE key_hash = ? AND expires_at <= ?`，只有一个赢家），并清空旧响应。
- 已完成的键在 `expires_at` 前重放响应；过期后按上面的通用语义重新执行。

### 并发正确性

跨实例不依赖 Redis 锁，只有两条原子操作：

1. `begin` 首次 INSERT 撞主键唯一约束 → 丢弃失败实体跟踪后按「已存在」处理；
2. 过期抢占、完成写入都是条件 UPDATE（CAS），由 affected 行数决定谁赢。

因此两个实例对同一键最多一个进入 `Started`。`complete`/`release` 的 CAS 都带
`expires_at > now`：对已过期/已释放的预约 affected = 0，只记 debug 不报错——迟到的响应
不会复活已过期的预约。端口没有 per-attempt token，因此「租约过期后被别的实例抢占」与
「旧 owner 迟到完成」在极小窗口内仍可能相互覆盖；把 TTL 设成大于 Handler 最坏耗时即可规避。

### 边界

- 表行数由「TTL + grace + 清理频率」约束；响应 body 另受中间件 `MaxBodyBytes` 限制
  （默认 1 MiB，超限的响应释放键而不入库）。
- body 与 `content-encoding` 原样入库、原样重放，所以不要把敏感头/Cookie 放进缓存响应。
- 多实例之间只共享数据库，不共享进程内状态；`EfIdempotencyStore` 不写本地磁盘。

## 部署语义

- **单进程**：`InMemoryIdempotencyStore`（有界 + TTL 惰性清理），零外部依赖。
- **多实例**：重试可能落到另一节点，必须用共享存储——数据库持久化实现见上一节
  （`rust-webx-ef::EfIdempotencyStore`，推荐），或用 Redis `SET NX` + 响应值自建
  `IIdempotencyStore`；只在一台机器上装内存存储等于没装。
- **容器**：预约带 TTL，实例被杀后下一次重试可重新执行；不会留下本地状态。
- **性能**：每个带键的写请求多两次存储往返；响应 body 以字节缓存，受 `MaxBodyBytes` 与
  `MaxTrackedKeys` 双重约束（数据库实现另受表行数与清理频率约束，见上一节）。

## 边界

- **不是唯一约束的替代**：数据库唯一索引仍是最终防线；幂等键只保证"同一键最多一次"。
- **不比较请求体**：同键但不同的 body 会重放第一次响应（标准的键复用语义）。客户端要改内容
  必须换新键；需要参数一致性校验时，在应用侧比较请求摘要并返回 409。
- 只对状态变更方法生效；GET/HEAD/OPTIONS 不缓存。
- 流式响应（SSE）不缓存：返回后键被释放，重试会重新执行。
- 幂等键不做长期审计：`TtlSeconds` 内有效，过期后同一键会重新执行。

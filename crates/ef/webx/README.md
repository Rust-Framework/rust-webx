# rust-webx-ef

The `rust-webx` boundary for `rust-ef`.

`rust-ef` is Web-agnostic (since 2.6 it does not even have a `webx` feature), so
translating `EFError` into `webx::Error` belongs at the application boundary.
This crate owns that decision once:

```rust
use webx_ef::EfResultExt;

let user = ctx
    .set::<User>()
    .filter(linq!(|u: User| u.id == id))
    .first_or_default()
    .await
    .map_ef()?; // EFError -> webx::Error

ctx.save_changes().await.map_ef()?;
```

| `EFError` | HTTP | Client-visible detail |
|---|---|---|
| `NotFound` | 404 | the caller-authored message |
| `ModelValidation` | 400 | the caller-authored message |
| `ConcurrencyConflict` | 409 | the caller-authored message |
| `Query` / `TypeConversion` | 400 | fixed text (SQL/column names never echoed) |
| `UniqueViolation` | 409 | fixed text (constraint/table never echoed) |
| `Connection` / `Transaction` | 503 | fixed text, retryable |
| everything else | 500 | none (`public_detail() = None`); message + source chain stay in the operator log |

`rust-webx-core` and the `rust-webx` umbrella never link this crate: add
`rust-webx-ef` to the application (or use the reference apps' `db.rs` helpers)
and call `.map_ef()?` at persistence call sites.

## 持久化幂等存储（多实例）

`webx_host::idempotency::InMemoryIdempotencyStore` 只看得见本进程的键，多实例部署下重试
落到另一台机器会再执行一次 Handler。`EfIdempotencyStore` 把预约与响应存进数据库表
`webx_idempotency`（主键 = 幂等键的 SHA-256 摘要，完整键单独存一列供排障），
所有实例共享同一份状态：

```text
begin(key) ──► Started      唯一键 INSERT 或 CAS 抢占过期行
           ├─► InProgress   另一个请求持有该键 → 409 + Retry-After
           └─► Replay(rsp)  已存响应 → 重放字节，不再执行 Handler
```

```toml
[dependencies]
rust-webx-ef = "2.8"
```

1. 模型注册 + 建表/迁移（表结构由实体声明，DDL 交给官方 rust-ef 迁移工具）：

```rust
use rust_ef::db_context::{DbContext, DbContextOptionsBuilder};
use webx_ef::idempotency::register_idempotency_model;

let mut db = DbContextOptionsBuilder::new();
db.use_postgres(&database_url);
let options = std::sync::Arc::new(db.build());
options.create_provider().expect("database provider"); // 启动期快速失败

let mut ctx = DbContext::from_options(&options)?;
ctx.set::<Order>();                     // 应用自己的实体
register_idempotency_model(&mut ctx);   // 新增 webx_idempotency 表
ctx.migrate("Migrations").await?;       // 新库首装用 ctx.bootstrap()
```

2. 装配存储、幂等中间件与清理服务（把**同一份** `Arc<DbContextOptions>` 交给宿主
   DbContext 与存储，provider/连接池/模型缓存共享）：

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

租约到期后 `complete`/`release` 的 CAS 会拒绝写入，迟到的响应不会复活过期预约；
`use_idempotency` 必须在 host build 之前拿到 store，所以 store 持有 `DbContextOptions`
而不是 DI 的 `ScopeFactory`（options 是启动时唯一天然存在的数据库依赖）。`Idempotency`
配置节仍只启用单进程内存存储；多实例必须显式注入本存储。

语义要点：

- **跨实例正确性**依赖主键唯一 + 条件 UPDATE（CAS），不需要 Redis 锁：同一键只有一个
  实例能 INSERT 成功或抢占成功。
- **TTL 是租约**：Handler 必须在 TTL 内完成，否则重试会抢占并可能并发执行；实例被杀后
  预约到期，下一次重试自动接管。
- **complete 只更新 `state = 'in_progress'` 的行**：预约已过期/被释放时静默丢弃迟到响应，
  不会复活旧预约。
- **清理**：`IdempotencyHousekeeping` 默认每小时 tick 一次，删除「过期超过 24h」的行
  （`with_interval` / `with_grace` 可调）；多个实例同时清理是安全的。
- **不是唯一约束的替代**：业务唯一性仍靠唯一索引；键过期后同一键会重新执行。

完整说明见 `docs/rust-webx/11-production/idempotency.md`。

## 设计边界

- **错误映射**：纯函数，无状态、无连接；同一映射在任何部署形态下一致。
- **幂等存储**：显式持有应用传入的 `Arc<DbContextOptions>`，每次操作开一个短生命周期
  `DbContext`（不跨 `.await` 共享），表结构只由官方迁移创建，运行时不自建 schema。
- **容器化**：映射不读取配置、不建立连接；幂等存储不写本地状态，实例被杀后靠 TTL 恢复。
- **性能**：`map_ef` 成功路径零开销（只在 `Err` 分支分配），脱敏文案是静态常量，
  5xx 只在日志侧拼接 source 链；幂等存储每个带键写请求多 2–3 次数据库往返
  （begin + complete，重试时 1 次 begin），响应 body 受中间件 `MaxBodyBytes` 约束。

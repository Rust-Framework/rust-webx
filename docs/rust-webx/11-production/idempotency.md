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
    "LeaseSeconds": 60,
    "MaxTrackedKeys": 10000,
    "MaxBodyBytes": 1048576,
    "AllowAnonymous": false,
    "Paths": ["/api/orders", "/api/payments"],
    "SkipPaths": ["/api/payments/webhook"]
  }
}
```

> `AllowAnonymous`（默认 `true`，兼容既有部署）：匿名请求按解析后的客户端 IP 分键。若匿名接口的
> `Idempotency-Key` 由请求体确定性推导，同一 NAT/反代 IP 下的不同客户端可能互相重放响应
> （最危险的是登录类接口把 A 的令牌重放给 B）——对外部署建议显式设为 `false`，只对已认证主体启用幂等。
>
> `Production` 模式下若 `Enabled=true`、`AllowAnonymous=true` 且 `Paths` 为空，宿主启动时
> 会打一条 **warn**（不改变行为）：这等于「所有路径 + 匿名按 IP 分键」，同一出口 IP 后的
> 客户端可能重放彼此响应。对外部署请设 `AllowAnonymous=false` 和/或收敛 `Paths`。

> `LeaseSeconds`（默认 60）：进行中的预约会阻塞并发重试多久；它与 `TtlSeconds`（已完成响应的
> 重放保留期）分离。租约必须大于 Handler 的最坏耗时，否则 Handler 可能被执行第二次；
> 中间件下发给存储的租约是 `min(LeaseSeconds, TtlSeconds)`。

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

### 装配顺序（2.8 DI 自动装配）

`Idempotency.Enabled = true` 但未显式调用 `use_idempotency` 时，Host 按顺序选择存储：

1. DI 容器里已注册的 `dyn IIdempotencyStore`（例如
   `svc.instance::<dyn IIdempotencyStore>(Arc::new(EfIdempotencyStore::new(options)))`）——取第一个；
2. 都没有时用有界内存存储（`MaxTrackedKeys` 控制上限）。

显式 `use_idempotency(store, ttl)` 永远优先于以上两者（重放保留期 TTL 来自该调用）；
`Paths` / `SkipPaths`、`MaxBodyBytes`、`AllowAnonymous`、`LeaseSeconds` 无论走哪条装配路径
都来自配置节（租约默认 60s，下传时取 `min(LeaseSeconds, ttl)`）。

### 路由过滤（Paths / SkipPaths）

`Paths` 为空（默认）= 所有路径都启用幂等（兼容既有部署）；非空时只对匹配前缀的请求生效。
匹配按**路径分段边界**进行：`/api/orders` 命中 `/api/orders` 与 `/api/orders/123`，不命中
`/api/orders-archive`；条目末尾的 `/` 忽略，`"/"` 等价于全部路径。`SkipPaths` 优先于 `Paths`。
不在范围内的请求在读取/校验 `Idempotency-Key` **之前**直接放行，不产生存储往返。

多实例部署用数据库持久化实现（`rust-webx-ef`），见「数据库持久化（多实例）」一节。

存储键由「租户 + 主体 + 方法 + 路径 + 查询串 + 请求头」组成
（查询串按 key/value 排序后入键，`POST /items?mode=a` 与 `?mode=b` 不会互相重放）：
已认证请求的主体是 `tenant:<tenant>:user:<sub>`（无租户 claim 时为 `user:<sub>`），
匿名请求用 `ClientInfo` 解析后的客户端 IP（`ip:<client>`，未安装 `ClientInfo` 中间件时
为对端地址）——同一用户在两个租户下的相同方法/路径/键不会互相重放。

各动态段进入 scope 前会做**单射转义**：`%`、`:`、`?`、`&`、`=` 分别转义成
`%25`、`%3A`、`%3F`、`%26`、`%3D`（查询串的 key/value 同样转义）。因此主体/租户/路径/查询值里
即使含有分隔符，也不可能拼出与另一个请求相同的键（例如路径 `/a:POST:/b` 与主体
`u:POST:/a` + 路径 `/b` 不再碰撞）；普通取值（`u-1`、`/api/orders`、`mode=a`）的键与 2.7
逐字节一致，升级不需要处理在途键。

| 请求头 | 行为 |
|---|---|
| 没有 `Idempotency-Key` | 正常执行，不做幂等 |
| 非 `POST`/`PUT`/`PATCH`/`DELETE` | 不做幂等（GET 本来就幂等） |
| 路径不在 `Paths` 内或命中 `SkipPaths` | 不做幂等（不读键、不访问存储） |
| 键为空 / 超过 255 / 含不可见字符 | `400` + `problem+json` |
| 首次请求 | 执行；2xx 且非 `text/event-stream`、body ≤ `MaxBodyBytes` 时缓存响应 |
| 同键重试 | 重放状态码、`content-type`、`content-encoding`、可重放响应头 + body，附 `Idempotency-Replayed: true` |
| 同键并发 | `409 Conflict` + `Retry-After: 1` |
| Handler 返回错误 / 非 2xx / body 过大 / 流式（SSE）/ 文件下载响应 | 释放键，允许重试重新执行 |
| 主体不同 | 键空间隔离（用户 A 的键不会命中用户 B 的响应） |
| 租户不同 | 键空间隔离（`tenant_id` 进入键；同一 sub 跨租户不重放） |

### 可重放响应头

缓存的响应除状态码/`content-type`/`content-encoding`/body 外，还保存白名单内的响应头并在重放时
按白名单顺序写回：`Location`、`ETag`、`Last-Modified`、`Cache-Control`（不区分大小写；白名单外
的头——包括 `Set-Cookie`、`Authorization`——从不保存、从不重放）。响应头集合以 JSON 存储；超过
8 KiB 的头集合不缓存（body 仍照常缓存）。`Content-Range` 等一次性/传输相关头不在白名单内，重放时
不会恢复（文件下载本来就整体释放键，不缓存）。

重放是安全边界：内存 store 与 EF store 在重放时都会再按白名单过滤一遍存储的响应头并把名字
规范为小写——即使自定义 `complete` 写入了 `Set-Cookie`/`Authorization` 之类的头，也不会出现在
重放响应里。

匿名请求以 `ClientInfo` 解析后的客户端 IP 作为主体（反代后是真实客户端而不是代理地址）；
没有 IP 也没有身份时不启用幂等（无法区分调用方）。匿名主体只按 IP 隔离，配置
`AllowAnonymous=false` 可完全关闭匿名幂等（见上）。

| 场景 | 行为 |
|---|---|
| Handler 成功但存储 `complete` 失败（DB 瞬断/列溢出） | 中间件再尝试 `release` 释放预约并记 warn/error，客户端可立即重试 |

## 实现端口

```rust
use webx_core::idempotency::{
    IIdempotencyStore, IdempotencyBegin, IdempotencyLease, IdempotentResponse,
};

#[async_trait::async_trait]
impl IIdempotencyStore for MyRedisStore {
    async fn begin(&self, key: &str, ttl: Duration) -> Result<IdempotencyBegin> { /* SET NX + 读响应 */ }
    async fn complete(&self, key: &str, response: IdempotentResponse, ttl: Duration) -> Result<()> { /* 存响应 */ }
    async fn release(&self, key: &str) -> Result<()> { /* DEL 预约 */ }
}
```

三态协议：`Started`（执行并 `complete`/`release`）、`InProgress`（409）、`Replay(response)`。
存储实现只需保证 `begin` 的 `SET NX` 原子性；中间件负责其余状态。`IdempotentResponse` 携带
`headers`（白名单内的可重放响应头，空 `Vec` 表示没有）。

`IdempotentResponse`、`IdempotencyLease`、`IdempotencyBegin` 都是 `#[non_exhaustive]`（2.8）：
自定义 store 用 `IdempotentResponse::new(status, content_type, content_encoding, body)` 构造、
`.with_headers(..)` 附加可重放头，`IdempotencyLease::new(token)` 构造令牌，`match IdempotencyBegin`
时补 `_` 通配分支。这样 2.x 后续给这些类型加字段/状态不需要再发破坏性版本。

### 租约围栏令牌（fencing token）

上面的最小实现有竞态：预约过期（或内存淘汰）后重试抢占同一键，旧 Handler 迟到的
`complete`/`release` 仍可能命中新租约。能区分「谁持有租约」的存储应实现三个**默认方法**覆盖的
租约变体（旧 store 不改代码也不受影响，中间件调用的默认实现会退回 token-less 行为）。
2.8 起租约方法显式接收两个时长——`retention`（已完成响应的重放保留期）与 `lease`
（进行中预约的租约）：中间件调用 `begin_lease(key, retention, lease)` 时传
`lease = min(LeaseSeconds, retention)`；`complete_lease` 只接收 `retention`：

```rust
#[async_trait::async_trait]
impl IIdempotencyStore for MyRedisStore {
    // ... begin/complete/release 同上 ...

    async fn begin_lease(
        &self,
        key: &str,
        retention: Duration, // 已完成响应的重放保留期（TtlSeconds）
        lease: Duration,     // 进行中预约的租约（LeaseSeconds，≤ retention）
    ) -> Result<(IdempotencyBegin, Option<IdempotencyLease>)> {
        // 抢占成功时生成新的不透明 token 并按 lease 设置预约过期（如 SET key token NX PX lease）
        let token = new_token();
        Ok((IdempotencyBegin::Started, Some(IdempotencyLease::new(token))))
    }

    async fn complete_lease(
        &self,
        key: &str,
        lease: Option<&IdempotencyLease>,
        response: IdempotentResponse,
        retention: Duration, // 写入的响应按 retention 保留
    ) -> Result<()> {
        // 仅当存储中的 token 仍等于 lease.token 时写入响应（Lua/CAS）
        Ok(())
    }

    async fn release_lease(&self, key: &str, lease: Option<&IdempotencyLease>) -> Result<()> {
        // 同样按 token 做 DEL 的 CAS
        Ok(())
    }
}
```

语义：`begin_lease` 返回 `Started` 时同时返回本次尝试的令牌，中间件在 `complete_lease` /
`release_lease` 原样传回；存储把「token 仍匹配」并入 CAS 条件，因此租约被抢占/过期后旧 token 的
`complete`/`release` 是 **no-op**（即使新租约仍然 live）。令牌内容对中间件不透明，实现可自选
（UUID/随机串/单调序号），只要求每次成功的抢占都生成新值、成功后清空。

`InMemoryIdempotencyStore` 与 `EfIdempotencyStore` 都实现了真正的令牌语义；自定义 store 若只实现
`begin`/`complete`/`release`，仍然正确，只是保留「TTL 必须大于 Handler 最坏耗时」这一较弱保证
——默认实现会忽略传入的 `lease`，用 `retention` 作为预约期。

## 数据库持久化（多实例）

内存存储只在单进程内可见：多实例部署下重试落到另一节点会再执行一次 Handler。
`rust-webx-ef` 提供 `IIdempotencyStore` 的 rust-ef 实现 `EfIdempotencyStore`，把预约与
响应存进数据库表 `webx_idempotency`（主键 = 幂等键），所有实例共享同一份状态。

真库验证：`crates/ef/webx/tests/idempotency_realdb_test.rs` 提供由
`RUST_EF_PG_URL` / `RUST_EF_MYSQL_URL` 驱动的 `#[ignore]` 测试（覆盖唯一键 INSERT 竞争回落、
条件 UPDATE/DELETE、Unix 秒 TTL 列与二进制 body）；本地或 CI 用
`cargo test -p rust-webx-ef --features postgres,mysql --tests -- --ignored` 运行，CI 的 `realdb`
job 用 `postgres:16` + `mysql:8` service 容器执行。默认构建不引入这两个 provider。

### 建表与迁移

表结构由实体（`webx_ef::idempotency::IdempotencyRecord`）声明，DDL 交给官方 rust-ef
迁移工具生成，不要手写 `CREATE TABLE` / `ALTER TABLE`：

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

主键是完整协议键的 SHA-256 摘要（`key_hash`，64 hex）：中间件的协议键 = 租户 + 主体 + 方法 +
路径 + 查询串 + 客户端 `Idempotency-Key`（≤255 字符），路径/查询串没有长度上限，所以固定宽度的
摘要才是安全的索引列；完整键另存在 `key` 列（TEXT，仅排障，截断到 1024 字符，不加索引）。
`expires_at` 上的 `#[index]` 服务于清理扫描。

2.8 新增两个**可空**列，旧表升级是加性变更（可空列，无回填）：

| 列 | 类型 | 含义 |
|---|---|---|
| `attempt` | `VARCHAR(64)` NULL | 当前持有预约的 fencing token；旧行/已完成行为 `NULL` |
| `headers` | `TEXT` NULL | 可重放响应头，JSON 数组 `[["name","value"], …]`；旧行/无头响应为 `NULL` |

已有库用官方迁移链路升级：应用内 `save_registered_snapshot` 导出新快照 →
`rust-ef migration add … --snapshot` 生成迁移 → 启动时 `ctx.migrate("Migrations")`。
旧行（`attempt`/`headers` 为 `NULL`）继续可用：被抢占时写入新 token，重放时按「无响应头」处理。

> 该表含业务响应 body（可能包含订单/报表数据）、响应头（如 `Location`/`ETag`）与协议键
> （含主体与路径），属于敏感数据面：生产环境按最小权限授予读写，不要导出/外共享；
> 不要在缓存响应里放 Set-Cookie / Authorization 等敏感头（框架也不会重放它们）。

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
    // 也可以只做下一行注册 + 配置节 "Idempotency": { "Enabled": true }，
    // 由 host 自动发现 DI 里的 IIdempotencyStore（2.8）。
    .use_idempotency(Arc::clone(&store), Duration::from_secs(24 * 3600))
    .register(move |svc| {
        svc.add_background_service_with(move |_| Arc::new(housekeeping.clone()))
    })
    .build();
```

`Idempotency` 配置节单独使用时只启用单进程内存存储；多实例用上文的显式注入
（或 DI 注册 + `Enabled=true` 自动发现）。

### 清理服务

`IdempotencyHousekeeping` 实现 `PeriodicBackgroundService`：默认每小时 tick 一次，
只删除「过期超过 24 小时」的行（`with_interval` / `with_grace` 可调，默认
`DEFAULT_CLEANUP_INTERVAL` / `DEFAULT_CLEANUP_GRACE`）。tick 是一条
`WHERE expires_at < now - grace` 的索引 `DELETE`；多个实例同时清理是安全的
（同一条过期行删两次是 no-op）。

### 租约、围栏与保留期（lease / fencing / retention）

`EfIdempotencyStore` 把「预约租约」与「重放保留期」分开，并用 token 围栏迟到写入：

- **租约**（`begin_lease` 的 `expires_at`，来自中间件下传的 `lease`，
  即 `Idempotency.LeaseSeconds`，默认 60s，且被 `TtlSeconds` 截断为 `min(LeaseSeconds, TtlSeconds)`）：
  Handler 必须在租约内完成并 `complete`；实例崩溃后下一次重试在约 60s 后即可抢占并重新执行，
  不必等完整 TTL。租约应大于 Handler 的最坏耗时。store **完全按传入的 lease 写入预约过期**，
  不做内部 `min(ttl)` 纠正（2.8 起租约是中间件的职责）；直接调用 `begin(key, ttl)` 的旧路径
  没有独立租约，预约期就是 `ttl`。
- **围栏令牌**（`attempt` 列）：每次成功抢占都写入新的随机 token 并返回给中间件
  （`begin_lease` → `IdempotencyLease`）。`complete_lease`/`release_lease` 的 CAS 条件为
  `state = 'in_progress' AND expires_at > now AND attempt = ?`，成功写入时清空 `attempt`。
  因此「租约过期后被别的实例抢占」与「旧 owner 迟到完成/release」不再可能互相覆盖：旧 token
  的写入 affected = 0。
- **保留期**（`complete_lease` 的 `expires_at`，由 `use_idempotency(..., ttl)` / `TtlSeconds` 决定）：
  已完成响应在保留期内重放；过期后同一键会重新执行。
- 实例被杀 → 租约到期 → 下一次重试 `begin` 用条件 UPDATE 抢占
  （`WHERE key_hash = ? AND expires_at <= ?`，只有一个赢家），写入新 token 并清空旧响应。
- 多实例机器时钟偏差仍会缩短/拉长租约窗口：部署需启用 NTP，租约保留余量。

#### 日志含义（EF store）

- 完成写入 affected = 0 时记 **warn**（2.8 起，之前是 debug）：
  `key_hash` + `lease`（本次 attempt token）说明「这条预约已不再属于该尝试」——它可能因为
  Handler 超过 `LeaseSeconds` 而租约过期、被 release、或被重试抢占。此时响应不会写入 DB，
  客户端拿到的仍是本次 Handler 的结果；但同键的下一次重试可能**再次执行 Handler**。
  生产上如果频繁出现该 warn，说明 `LeaseSeconds` 小于 Handler 最坏耗时或存在时钟偏差，
  应调大 `Idempotency.LeaseSeconds`（它会以 `TtlSeconds` 为上限）。
- `release` 成功删除进行中的预约记 debug；`complete` 覆盖/迟到写入不会记 error（按协议是 no-op）。
- 清理任务（`IdempotencyHousekeeping`）删除过期行时记 debug（`purged` 计数）；
  响应体超过列容量、可重放响应头超过 8 KiB 时记 warn，并分别释放预约 / 丢弃响应头。

### 并发正确性

跨实例不依赖 Redis 锁，只有两条原子操作：

1. `begin` 首次 INSERT 撞主键唯一约束 → 丢弃失败实体跟踪后按「已存在」处理；
2. 过期抢占、完成写入都是条件 UPDATE（CAS），由 affected 行数决定谁赢；带 token 的
   `complete_lease`/`release_lease` 把 `attempt = ?` 一起放进 CAS。

因此两个实例对同一键最多一个进入 `Started`，且任何时刻只有一个 token 能完成/释放该键。

### 边界

- 表行数由「保留期 + grace + 清理频率」约束；响应 body 受中间件 `MaxBodyBytes` 与
  store 的列容量双重限制。`EfIdempotencyStore` 在 MySQL 上把上限压到约 60 KiB（MySQL
  `Vec<u8>` → `BLOB` 只有 64 KiB），超限时释放预约并允许重试（不缓存该响应）；
  PostgreSQL/SQLite 由 `MaxBodyBytes` 决定。用 `EfIdempotencyStore::with_max_body_bytes`
  显式对齐两者。
- 可重放响应头以 JSON 存储，集合超过 8 KiB 时不缓存响应头（body 仍缓存），避免撑爆
  MySQL `TEXT` 列。
- 未注册 `IdempotencyHousekeeping` 时过期行不会自动清理，DB 会随请求量无限增长：
  多实例/生产部署必须注册清理服务（见上）。
- 幂等中间件在**路由匹配之前**执行（鉴权之后）：用 `Paths`/`SkipPaths` 把范围收敛到真正需要
  保护的写接口；未匹配路由的请求不会产生存储往返。
- body 与 `content-encoding` 原样入库、原样重放；响应头只重放白名单。不要把敏感头放在
  会被缓存的响应里（`Set-Cookie`/`Authorization` 即使出现也不会被保存）。
- 多实例之间只共享数据库，不共享进程内状态；`EfIdempotencyStore` 不写本地磁盘。

## 部署语义

- **单进程**：`InMemoryIdempotencyStore`（有界 + TTL 惰性清理 + 每预约 fencing token），
  零外部依赖。达到 `MaxTrackedKeys` 时按优先级淘汰：先已完成条目（Handler 已执行过），
  其次过期条目，最后才淘汰进行中的预约（让客户端重试重新执行）。进程内状态只对本进程可见，
  多实例必须换共享存储。
- **多实例**：重试可能落到另一节点，必须用共享存储——数据库持久化实现见上一节
  （`rust-webx-ef::EfIdempotencyStore`，推荐），或用 Redis `SET NX` + token CAS + 响应值
  自建 `IIdempotencyStore`；只在一台机器上装内存存储等于没装。
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

# 事务性 outbox（框架派发器 + 适配器存储）

业务事务提交后要发事件，常见做法是"先提交再 publish"——中间崩溃就丢事件；
"先 publish 再提交"则会发出现已回滚的事件。**事务性 outbox** 把事件行写进同一个数据库事务，
由后台任务在提交后派发：

```text
begin tx
  insert order
  insert webx_outbox(event_name, payload)
commit            ← 业务与事件原子
dispatcher tick   → 派发事件 → delivered_at  / 重试 / 死信
```

框架把它拆成**两层**：派发语义（端口、派发器、重试/死信/保留期）在已发布的 `rust-webx-outbox`；
"事件行存哪里、跟哪个事务一起提交"是数据适配，归下游——参考实现在 `examples/adapters/outbox/ef`。

| | 框架 `rust-webx-outbox`（已发布） | 参考适配器 `examples/adapters/outbox/ef`（非发布） |
|---|---|---|
| 提供 | `IOutbox`、`OutboxEntry`、`OutboxOptions`、`OutboxDispatcher`、`InMemoryOutbox` | rust-ef 实体（`webx_outbox` 表）、`enqueue_outbox`、`add_outbox_ef` |
| 存储 | 进程内内存表（重启即丢） | 数据库表，多实例共享 |
| 事务 | 无（入队写入内存表，不参与业务事务） | 与业务行同一 Unit of Work 提交/回滚 |
| 装配 | `add_outbox(store, OutboxOptions::default())`（或用 `add_outbox_with`） | 复制后 `add_outbox_ef(OutboxOptions::default())` |
| 依赖 | 不链接 `rust-ef` | `rust-webx` + `rust-ef` + 应用数据库 |

## 1. 用框架默认实现（单进程）

```toml
[dependencies]
rust-webx = "2.9"
rust-webx-outbox = "2.9"
```

```rust
use std::sync::Arc;
use webx::*;
use webx_outbox::*; // `IOutbox` / `InMemoryOutbox` / `OutboxOptions` / `add_outbox`

Host::builder()
    // 注册 Arc<dyn IOutbox> + 派发器；InMemoryOutbox 就是零配置的内存存储
    // （`new(6)` 的参数是进入死信前的最大尝试次数，与 OutboxOptions::max_attempts 对齐）
    .add_outbox(Arc::new(InMemoryOutbox::new(6)), OutboxOptions::default())
    .build();
```

`add_outbox(store, options)` 接收一个已经建好的 `Arc<dyn IOutbox>`；需要容器（如 `ScopeFactory`、`DbContext`）
来构建存储时用 `add_outbox_with(|resolver| .., options)`。存储实现由生产者侧提供，派发器由框架装配。

生产者侧：`InMemoryOutbox` 提供 `enqueue(id, event_name, payload)` 直接入队；派发器按 tick 领取并调用本进程
已注册的 `#[handler] IEventHandler<T>`，不需要另写派发代码。注意 `IOutbox` 是**派发器使用的存储端口**
（`claim` / `complete` / `fail` / `dead_letter` / `purge`），生产者入队能力由具体存储提供
（内存实现是 `enqueue`，EF 实现是 `enqueue_outbox`）。

内存实现只保证**本进程内最终派发**：进程重启丢事件、多实例之间不共享，也没有"与业务事务同生共死"的语义。
用于原型与单进程测试；越过这条线换下一节的 EF 存储。

## 2. 用参考适配器（EF 存储、同事务）

```bash
cp -r examples/adapters/outbox/ef crates/my-service-outbox
```

复制后改包名/导入名与实体，然后三步接入：

1. 应用注册 DbContext 时把 outbox 表加进模型：

```rust
use my_service_outbox::register_outbox_model;

let mut ctx = DbContext::from_options(&options)?;
register_outbox_model(&mut ctx); // 自己的实体也在这里 set
// 生产：ctx.migrate("Migrations")；测试：ctx.bootstrap()
```

2. 生产者在**同一事务**里入队（`enqueue_outbox` 只 `add` 到当前 Unit of Work，由调用方保存）：

```rust
use my_service_outbox::OutboxDbContextExt;

ctx.use_transaction(|ctx| {
    Box::pin(async move {
        ctx.add(order);
        ctx.enqueue_outbox(&OrderPlaced { order_id })?; // 生成 id 的便捷方法
        ctx.save_changes().await
    })
})
.await?;
```

3. 宿主启动派发器（EF 存储，复用框架的 `OutboxDispatcher`）：

```rust
use my_service_outbox::*;

Host::builder()
    .register(|services| services.add_dbcontext(|options| { /* 应用自己的 DbContext */ }))
    .add_outbox_ef(OutboxOptions::default()) // EF 存储 + 框架派发器
    .build();
```

适配器只实现存储（`IOutbox` 的 EF 版本 + 同事务入队），领取、退避、死信、保留期清理全部复用框架派发器，
不要复制一份 worker。

## 3. 选项（`OutboxOptions`）

`add_outbox` / `add_outbox_ef` 接收同一个 `OutboxOptions`（`serde(default)`，只覆盖需要的字段）：

| 字段 | 默认 | 含义 |
|---|---|---|
| `enabled` | `true` | 是否启动后台派发器；端口始终注册，可关掉后手动重放 |
| `interval_seconds` | `5` | tick 周期 |
| `batch_size` | `32` | 每 tick 领取行数（单个 tick 有界） |
| `lease_seconds` | `60` | 领取租约；派发器崩溃后最多这么久被其他实例回收 |
| `max_attempts` | `6` | 失败多少次后进死信 |
| `retry_base_seconds` | `2` | 指数退避基数（2s、4s、8s…，封顶 `base * 2^10`） |
| `retention_days` | `7` | 已投递/死信行保留天数（`0` = 不清理） |

需要配置节时用 `add_options::<OutboxOptions>("Outbox")` 绑定（键名跟随字段的 serde 命名），
存储相关配置（连接串/池）仍归应用的 DbContext，不在 outbox 选项里。

## 4. 语义

- **恰好一次入库、至少一次投递**（EF 存储）：handler 成功后才写 `delivered_at`；两者之间崩溃会重投，
  所以 handler 必须幂等（用业务唯一键或 `IIdempotencyStore` 的键都行）。
- **单实例内不重复派发**：领取是原子操作（EF 存储是 CAS 更新 `locked_until`），两个实例不会同时拿到同一行。
- **多实例**：所有实例共享同一数据库；派发器可随实例横向扩展，租约保证不重复（仅 EF 存储；
  内存存储是进程内的，多实例等于各自丢各自的事件）。
- **容器**：无本地状态；实例被杀后租约到期自动回收，重试时间在数据库里。
- **无 handler 的事件**：直接进死信（重试永远不会成功），`last_error` 写明原因。
- **事务性**：EF 存储下 `enqueue_outbox` 只是 `add`，事件行与业务行同一事务提交/回滚；
  不要先 commit 业务再入队。内存实现没有这条语义。
- **性能**：EF 存储下入队是一次 INSERT（无额外往返）；派发每 tick 一次批量 SELECT + 每行 1 次
  CAS UPDATE + 1 次完成 UPDATE，批量大小受 `batch_size` 约束。

## 5. 迁移与表

`webx_outbox` 表由参考适配器的实体声明（`id` 主键、`event_name`/`next_attempt_at` 索引），
用官方 rust-ef 迁移工具生成 DDL；`bootstrap`/`ensure_created` 只用于测试。
框架 crate 不声明任何表，也不在运行期建表。

## 6. 测试

```bash
cargo test --workspace --manifest-path examples/adapters/Cargo.toml
```

参考适配器的测试覆盖：同事务入队 + 租约 claim + 死信 + 保留期清理（SQLite）；多实例/真库场景
用 env-gated 测试，在 nightly 单独跑。框架侧 `cargo test -p rust-webx-outbox` 覆盖端口契约与
派发器语义（租约、退避、死信）。

## 7. 边界

- 不是消息队列：跨节点"发布出去"仍由 broker 适配器完成；outbox 只保证**本地事务与事件同生共死**，
  跨进程投递交给事件传输（`IEventPublisher`）。
- 按键序：不保证全局顺序；同一业务键的顺序需要应用侧串行（例如同一行只允许一个未完成事件）。
- 重复 id 入队返回 `UniqueViolation`（EF 存储），调用方可视为"已入队"（确定性 id 使生产者重试幂等）。
- 接入方式（默认实现 / 复制参考适配器 / 自己实现端口 + 契约测试）的总览见
  [端口与适配器（下游接入）](integrations.md)。

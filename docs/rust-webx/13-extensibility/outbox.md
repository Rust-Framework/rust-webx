# 事务性 outbox（rust-webx-outbox-ef）

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

## 接入

```toml
[dependencies]
rust-webx-outbox-ef = "2.7"
```

1. 应用注册 DbContext（`add_dbcontext`）时把 outbox 表加进模型：

```rust
use webx_outbox_ef::register_outbox_model;

let mut ctx = DbContext::from_options(&options)?;
register_outbox_model(&mut ctx); // 自己的实体也在这里 set
// 生产：ctx.migrate("Migrations")；测试：ctx.bootstrap()
```

2. 生产者在**同一事务**里入队（`enqueue` 只 `add` 到当前 Unit of Work，由调用方保存）：

```rust
use webx_outbox_ef::OutboxDbContextExt;

ctx.use_transaction(|ctx| {
    Box::pin(async move {
        ctx.add(order);
        ctx.enqueue_outbox(&OrderPlaced { order_id })?; // 生成 id 的便捷方法
        ctx.save_changes().await
    })
})
.await?;
```

3. 宿主启动派发器：

```rust
Host::builder()
    .add_outbox(OutboxOptions::default()) // 后台服务 + Arc<dyn IOutbox>
    .build();
```

事件由**本进程已注册的 `#[handler] IEventHandler<T>`** 接收（与 broker consumer 同一套注册），
不需要另写派发代码。

## 选项（`Outbox` 配置节）

| 字段 | 默认 | 含义 |
|---|---|---|
| `Enabled` | `true` | 是否启动后台派发器（端口始终注册，可关掉后手动重放） |
| `IntervalSeconds` | `5` | tick 周期 |
| `BatchSize` | `32` | 每 tick 领取行数（单个 tick 有界） |
| `LeaseSeconds` | `60` | 领取租约；派发器崩溃后最多这么久被其他实例回收 |
| `MaxAttempts` | `6` | 失败多少次后进死信 |
| `RetryBaseSeconds` | `2` | 指数退避基数（2s、4s、8s…，封顶 `base * 2^10`） |
| `RetentionDays` | `7` | 已投递/死信行保留天数（0 = 不清理） |

## 语义

- **恰好一次入库、至少一次投递**：handler 成功后才写 `delivered_at`；两者之间崩溃会重投，
  所以 handler 必须幂等（用业务唯一键或 `IIdempotencyStore` 的键都行）。
- **单实例内不重复派发**：领取是 CAS 更新（`locked_until`），两个实例不会同时拿到同一行。
- **多实例**：所有实例共享同一数据库；派发器可随实例横向扩展，租约保证不重复。
- **容器**：无本地状态；实例被杀后租约到期自动回收，重试时间在数据库里。
- **无 handler 的事件**：直接进死信（重试永远不会成功），`last_error` 写明原因。
- **事务性**：`enqueue` 只是 `add`，事件行与业务行同一事务提交/回滚；不要先 commit 业务再入队。
- **性能**：入队是一次 INSERT（无额外往返）；派发每 tick 一次批量 SELECT + 每行 1 次 CAS UPDATE
  + 1 次完成 UPDATE，批量大小受 `BatchSize` 约束。

## 迁移与表

`webx_outbox` 表由实体声明（`id` 主键、`event_name`/`next_attempt_at` 索引），
用官方 rust-ef 迁移工具生成 DDL；`bootstrap`/`ensure_created` 只用于测试。

## 边界

- 不是消息队列：跨节点"发布出去"仍由 broker 适配器完成；outbox 只保证**本地事务与事件同生共死**，
  跨进程投递交给事件传输（`IEventPublisher`）。
- 按键序：不保证全局顺序；同一业务键的顺序需要应用侧串行（例如同一行只允许一个未完成事件）。
- 重复 id 入队返回 `UniqueViolation`，调用方可视为"已入队"（确定性 id 使生产者重试幂等）。

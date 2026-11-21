# 事件发布与订阅

事件用于**跨模块的异步副作用**。`publish` 只负责把事件交给传输层：它不等待处理器、不返回处理器结果，也不让处理器的慢或失败影响发布者的请求。

## 定义事件

事件是**消息**（serde DTO），因为同一份事件要能走进程内队列，也能走 Redis / 消息队列：

```rust
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct UserCreatedEvent {
    pub user_id: String,
    pub email: String,
}

impl IEventRequest for UserCreatedEvent {}
```

`IEventRequest::event_name()` 默认返回 Rust 全限定类型名，作为跨进程路由键。类型改名/移动会改变线上事件名；需要稳定线上名时手动覆盖：

```rust
impl IEventRequest for UserCreatedEvent {
    fn event_name() -> &'static str { "user.created" }
}
```

## 定义事件处理器

事件 Handler 的构造方式与请求 Handler **完全一致**，只有两种：

```rust
#[derive(Default)]
pub struct SendWelcomeEmailHandler;

#[handler] // 默认构造；无需任何 DI 注册
#[async_trait]
impl IEventHandler<UserCreatedEvent> for SendWelcomeEmailHandler {
    async fn handle(&self, event: UserCreatedEvent) -> Result<()> {
        tracing::info!("Sending welcome email to {}", event.email);
        Ok(())
    }
}
```

需要注入依赖时用 `#[handler(inject)]` + `#[derive(Inject)]`：

```rust
#[derive(Inject)]
pub struct AuditHandler {
    #[inject]
    audit: Arc<dyn IAuditSink>,
}

#[handler(inject)]
#[async_trait]
impl IEventHandler<UserCreatedEvent> for AuditHandler {
    async fn handle(&self, event: UserCreatedEvent) -> Result<()> {
        self.audit.record(event.user_id).await
    }
}
```

宿主只需要：

```rust
Host::builder()
    .register(|svc| svc.add_mediator())
    .build()
```

| 写法 | 构造方式 | 需要注册什么 |
|---|---|---|
| `#[handler]` | `Default::default()` | 无 |
| `#[handler(inject)]` | `#[derive(Inject)]` 构造器 | 只注册依赖，不注册 handler |

每个事件都会新建一个 Handler 实例（与每请求新建请求 Handler 同理）；**共享可变状态应放进注入的依赖**（如 `Arc<AppState>`），不要把状态放在 Handler 字段里。同一事件有多个 Handler 时全部广播。

## 发布事件

```rust
async fn handle(&mut self, req: CreateUserRequest) -> ActionResult<UserDto> {
    let user = self.repo.create(&req).await?;

    // 事件入队即返回；不会等待邮件发送
    self.mediator.publish(UserCreatedEvent {
        user_id: user.id.clone(),
        email: user.email.clone(),
    }).await?;

    Ok(user)
}
```

`publish` 的返回值只表示**事件是否被传输层接受**：

| 返回值 | 含义 |
|--------|------|
| `Ok(())` | 已入队 / 已交给 broker；处理器稍后执行 |
| `Err` | 事件**未被接受**：无本地注册、队列已满（可重试 503）、broker 不可用 |

处理器失败只记录日志，不会出现在 `publish` 的返回值里。

## 默认传输：内存队列

`add_mediator()` 注册以下默认实现：

- `InMemoryEventPublisher`：有界队列（容量 `DEFAULT_EVENT_QUEUE_CAPACITY`），`publish` 用 `try_send`，队列满返回可重试的 503，绝不阻塞发布者；
- `InMemoryEventWorker`：`BackgroundService`，随 Host 启动/停止，每条事件开独立 DI scope，顺序投递；
- `EventDispatcher`：按 `event_name` 解码 JSON，广播给 `#[handler]` /
  `#[handler(inject)]` 生成的 inventory 处理器。

投递语义是 **at-most-once**：进程崩溃会丢失队列中的事件；需要可靠投递时使用 outbox + 持久化 broker。

处理器之间互相隔离：一个处理器返回错误或 panic 不会阻止其他处理器执行，也不会杀死 worker。

## 可观测性

`add_mediator()` 同时注册进程级 `EventMetrics`，`/metrics`（启用 metrics 时）输出：

| 指标 | 含义 |
|---|---|
| `events_published_total` | 被传输层接受的事件数 |
| `events_publish_failed_total` | 编码失败、队列满、broker 不可用等未被接受的事件数 |
| `events_dispatch_failed_total` | 至少一个 Handler 失败/panic 的派发次数 |
| `events_dropped_no_handler_total` | 无订阅者、按 no-op 丢弃的事件数 |
| `events_dead_lettered_total` | 持久化适配器写入死信目的地的事件数（当前为 Redis） |
| `events_consumer_reconnects_total` | broker 消费者重连尝试次数 |

这些计数器只增不减，按进程维度聚合；需要按事件名拆分时，在 Handler 内使用应用自己的 `IMeter`，不要把高基数标签塞进框架计数器。

## 使用内置 broker 适配器

生产环境不需要手写传输层：四个适配器 crate 各提供一行注册入口，内部注册 `IEventPublisher`
实现和后台消费者。完整语义/边界见
[broker-capability-matrix.md](../../design/broker-capability-matrix.md)。`publish()` 调用点不变；消费者只拿到 `event_name` + JSON，交回
`EventDispatcher` 路由，所以事件处理器只需 `#[handler]` / `#[handler(inject)]`，与内存队列
完全一致。发布事件仍需注册 Mediator：
`.register(|svc| svc.add_mediator())`（`add_redis` / `add_nats` / `add_rabbitmq` / `add_kafka`
只替换传输层，不替代 `add_mediator`）。

```toml
[dependencies]
rust-webx-redis = "2.5"       # 分布式缓存 + Redis Streams 事件
rust-webx-nats = "2.5"        # NATS core 事件
rust-webx-rabbitmq = "2.5"    # RabbitMQ（AMQP）事件
rust-webx-kafka = "2.5"       # Kafka 单分区事件（参考/起步适配器）
```

### 投递语义对照

| 适配器 | 传输 | 投递语义 | 消费模型 |
|--------|------|----------|----------|
| `rust-webx-redis` | Redis Streams（`XADD` / `XREADGROUP`） | **at-least-once**：处理器返回后才 `XACK`；消费者崩溃留下的 pending 由 `XAUTOCLAIM` 认领；失败超过 `max_deliveries` 后进 `dead_letter_stream` 或丢弃，`XAUTOCLAIM` 的 deleted ids 自动 `XACK` | consumer group 内多实例分摊；stream 用 `MAXLEN ~` 封顶；组首次创建时 `start_id` 默认 `$`（只看新消息），设 `0` 可重放保留期内的历史 |
| `rust-webx-nats` | core NATS | **at-most-once**：断连或没有订阅者即丢 | 默认每个订阅者都收到；`with_queue_group` 后组内负载均衡 |
| `rust-webx-rabbitmq` | AMQP topic exchange + **quorum queue** | **at-least-once**：publisher confirm + 手动 ack，处理器失败 requeue；broker 的 `x-delivery-limit`（默认 5）到顶后死信（配置了 DLX）或丢弃，不会无限重投 | 同队列竞争消费（需要每个服务各收一份时用不同队列名）；`prefetch` 控制未确认上限；需 RabbitMQ 3.8+，需幂等 |
| `rust-webx-kafka` | Kafka（纯 Rust `rskafka`） | 单分区 follower；分区内有序；**不做 consumer-group offset 提交**，重启从 `start_offset` 重新开始 | 一个适配器消费一个分区（`with_partition`）；参考/起步实现 |

### Redis Streams

默认同时注册分布式缓存和事件传输；缓存细节（前缀、TTL、健康检查）见
[缓存与速率限制](../11-production/caching-rate-limit.md)。

```rust
use std::time::Duration;
use webx::*;
use webx_redis::{RedisHostBuilderExt, RedisOptions};

Host::builder()
    .add_redis(
        RedisOptions::new("redis://127.0.0.1:6379/0")
            .with_prefix("app:")
            .with_default_ttl(Duration::from_secs(300)),
    )
    .build();
```

消费组首次创建时默认从 `$` 开始；要重放保留期内的历史事件，设置
`RedisEventsOptions { start_id: "0".into(), .. }`。失败事件默认重投 5 次
（`max_deliveries`，按 `XPENDING` 的 delivery count 精确计数），之后写入
`dead_letter_stream`（配置时）或丢弃；死信流可用 `XADD`/`XREADGROUP` 或运维脚本重放。
Redis 6.2 以下可设 `claim_pending: false` 关闭 `XAUTOCLAIM` 回收（注意这会同时停掉
pending 重试与 PEL 清理）。`XREADGROUP` 与 `XAUTOCLAIM` 是顺序执行的（claim 不在读取
进行中抢占），避免取消读取后消息只进 PEL 而丢失回复。

死信重放用同一个 consumer：

```rust
let replayed = consumer.replay_dead_letter(100).await?;
```

它按 `XRANGE` 读取死信流、把原始 `event`/`payload` 重新 `XADD` 到源流，成功后才
`XDEL` 死信条目；崩溃窗口可能产生重复，语义仍是 at-least-once。malformed 死信条目
只告警、保留原样，不会阻塞后续重放。

### NATS core

```rust
use webx_nats::{NatsHostBuilderExt, NatsOptions};

Host::builder()
    .add_nats(
        NatsOptions::new("nats://127.0.0.1:4222")
            .with_subject("app.events")
            .with_queue_group("workers"), // 组内分摊；省略则每个实例都收到
    )
    .build();
```

### RabbitMQ

```rust
use webx_rabbitmq::{RabbitMqHostBuilderExt, RabbitMqOptions};

Host::builder()
    .add_rabbitmq(
        RabbitMqOptions::new("amqp://guest:guest@127.0.0.1:5672/%2f")
            .with_topology("app.events", "user.created", "app.users"),
    )
    .build();
```

队列以 **quorum** 类型声明（RabbitMQ 3.8+），`x-delivery-limit` 默认 5；设置
`dead_letter_exchange` 后超限消息进入该 DLX，否则由 broker 丢弃。DLX 需由部署方预先创建；
已有的同名 classic 队列需换队列名或删除后重建。发布使用 `mandatory=true`：路由不到队列时
返回可重试的 503，而不是静默丢弃。死信重放由 RabbitMQ shovel/管理插件或一个消费 DLX 的
`#[handler]` 负责，适配器只投递到 DLX。

### Kafka（参考/起步适配器）

```rust
use webx_kafka::{KafkaHostBuilderExt, KafkaOptions, KafkaStartOffset};

Host::builder()
    .add_kafka(
        KafkaOptions::new("127.0.0.1:9092", "webx.events")
            .with_partition(0)
            .with_start_offset(KafkaStartOffset::Latest),
    )
    .build();
```

Kafka 适配器是**参考/起步实现**：单分区 follower，只连明文 `host:port` broker，**不做
consumer-group offset 提交**，重启时按 `start_offset` 重新开始（`Latest` 会跳过停机期间的
新消息，`Earliest` 会重放保留期内的消息）。多分区、rebalance、TLS/SASL 与提交策略需要替换
成组感知的适配器；`IEventPublisher` / `EventDispatcher` 契约不变。

### 开关与自定义传输

- Redis：`.cache_only()` 只要缓存；`.without_cache()` + `RedisEventsOptions { consumer_enabled: false, .. }` 只发不收。
- NATS / RabbitMQ / Kafka：`.publisher_only()` 只发不收。
- 其他 broker：实现 `IEventPublisher`（把 `event_name` + `payload` 写入 broker），用
  `Host::builder().use_event_publisher(Arc::new(...))` 替换默认实现；消费端复用
  `EventDispatcher::dispatch`（解码 + 新 DI scope + 广播）。也可在 `.register` 里
  `svc.instance::<dyn IEventPublisher>(...)`，`add_mediator()` 不会覆盖已有注册。

## 跨节点实时扇出（复用同一传输）

实时房间广播在**单进程**内直接走 `IConnectionRegistry`（即时）；**跨进程**时不新增 pub/sub
端口，而是复用本文的事件传输：

```rust
// 任意节点发布；各节点的事件 consumer 把消息交给框架注册的 fan-out Handler，
// 后者写入本节点房间内的连接（at-most-once，尽力而为）。
mediator
    .publish(RealtimeFanoutEvent::to_room("room-1", "hello"))
    .await?;
```

这样只有一条跨进程传输通道（`IEventPublisher`）、一套 wire（`event_name` + JSON）和一次
适配器实现（Redis/NATS starter 已实现事件传输）。语义/取舍见
[架构 ADR](../../design/mediator-handler-architecture.md)。

## 与直接调用 / send 的选择

| 场景 | 推荐 |
|------|------|
| 创建用户后发邮件、写日志、更新缓存（不需要结果） | `publish()` 事件 |
| 需要返回值 / 强一致 | `mediator.send()` 或直接调用 Service |
| 必须可靠投递（不能丢） | outbox + 持久化 broker，见后台任务/outbox 场景 |

## 小结

- 事件是 serde DTO，`event_name()` 是路由键；
- `publish` 是非阻塞、无结果的分发，处理器失败由传输层记录；
- 默认内存队列开箱即用；生产换 Redis / NATS / RabbitMQ / Kafka 用对应适配器一行注册，
  处理器代码与 `#[handler]` + DI 注册方式不变。

下一章：[认证与授权](../09-auth-security/INDEX.md)

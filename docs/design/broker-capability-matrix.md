# Broker 适配器能力矩阵与边界

> 目标：把 **可移植的端口契约** 与 **不可移植的投递语义** 分开。核心只承诺
> `IEventPublisher` / `IEventHandler` / `EventMessage`；适配器明确声明自己的语义和边界，
> 不用“最低共同点”牺牲可靠性，也不把 broker 特性泄漏到 core。
>
> 2.9.2 起事件传输分**两种投递类**（`EventDelivery::Competing` / `Broadcast`）：事件类型通过
> `IEventRequest::delivery()` 声明，`IMediator::publish` 据此分派到
> `IEventPublisher::publish` / `IEventPublisher::broadcast`。线格式（`EventMessage::to_wire`）、
> `EventDispatcher` 与 `#[handler]` 注册完全不变——一种事件模型，两种投递类。§2 逐适配器声明
> 广播能力；竞争投递的既有语义不变。

## 1. 总览

| 能力 | Redis | NATS | RabbitMQ | Kafka |
|---|---|---|---|---|
| crate | `rust-webx-redis` | `rust-webx-nats` | `rust-webx-rabbitmq` | `rust-webx-kafka` |
| 入口 | `add_redis` | `add_nats` | `add_rabbitmq` | `add_kafka` |
| 传输模型 | Streams consumer group（竞争）+ Pub/Sub（广播） | core NATS pub/sub（竞争 queue group，广播独立 subject） | quorum topic queue（竞争）+ fanout exchange（广播） | single-partition follower |
| **投递类** | 竞争 + 广播 | 竞争 + 广播 | 竞争 + 广播 | **仅竞争**（广播不支持，见 §2） |
| 投递语义 | 竞争 **at-least-once**；广播 **at-most-once** | 竞争/广播均 **at-most-once** | 竞争 **at-least-once**（有界重试）；广播 **at-most-once** | **at-most-once 近似**（不提交 offset） |
| 失败重试 | 竞争：`XAUTOCLAIM` + `max_deliveries`；广播：无（Pub/Sub 断连即丢） | 无（断连即丢） | 竞争：quorum `x-delivery-limit` 重入队；广播：无（handler 失败只记录） | 无（重启按 `start_offset`） |
| 死信 | `dead_letter_stream` + `replay_dead_letter`（`add_redis` 后 resolve `Arc<RedisEventConsumer>`） | 无 | 竞争：可选 DLX（broker-side）；广播：无 | 无 |
| 顺序 | 单 stream 内有序 | 单订阅者内有序；广播不保证跨节点顺序 | 单队列内有序（重试会换位）；广播不保证跨节点顺序 | 单分区内有序 |
| 多实例 | 竞争：consumer group 分摊；广播：每实例各收一份 | 竞争：queue group 分摊；广播：每实例各收一份（不设 queue group） | 竞争：同一 quorum queue 竞争消费；广播：每节点独占 auto-delete 队列 | **不支持组**；每实例独立 follower；无广播 |
| 认证/TLS | URL（含 ACL/user），`rediss://` 已启用 | URL（`tls://` 需 feature） | URL（AMQP URI），凭据在 URL | **仅明文** `host:port` |
| 拓扑自动创建 | stream/group（`MKSTREAM`）；广播频道无需创建 | 无 | 竞争：exchange/queue/binding；广播：fanout exchange + 独占队列 | 可选 topic（`create_topic`） |
| 健康检查 | `PING` | TCP/连接 | `connection`/channel 可用性 | broker metadata / topic |
| 额外能力 | `IDistributedCache`；消费者阻塞读用独立连接（`block_ms` 不受 `command_timeout` 影响） | — | — | — |

## 2. 投递类（2.9.2 D1）

`publish` 走竞争通道，`broadcast` 走广播通道；由事件类型的 `delivery` 自动选择，调用方不传
通道参数。广播是 **at-most-once**：`broadcast` 返回 `Err` 只表示传输**未接受**（连接失败、
不支持广播），接收方 handler 失败由传输记录日志、不重投；需要跨节点顺序或重放时用业务侧
持久化（如 `IRealtimeEventStore` 续传）兜底。

| 适配器 | 投递类 | 竞争投递 / 传输模型 | 广播实现机制 | 广播语义 | 失败模式 / 不支持项 |
|---|---|---|---|---|---|
| InMemory（core 默认） | 竞争 + 广播 | 有界 mpsc 队列 + 单 worker | 同一有界队列（单进程 = 全节点） | at-most-once | 队列满返回可重试 503；无本地 handler 的广播事件是记录性 no-op |
| Redis | 竞争 + 广播 | Streams consumer group（现状） | Pub/Sub 频道 `broadcast_channel`（默认 `<stream>:broadcast`） | at-most-once（Pub/Sub 无 PEL、不重投） | 订阅端断连期间的广播事件丢失；需要持久化扇出时用 Streams + 应用侧多 consumer group |
| NATS | 竞争 + 广播 | `subject`；是否分摊由 `queue_group` 决定（现状） | 独立 `broadcast_subject`（默认 `<subject>.broadcast`），**不设 queue group** | at-most-once（core NATS，无 ack/持久化） | 无订阅者时消息直接丢弃；广播订阅失败只记日志并保留竞争消费；JetStream 持久广播不支持 |
| RabbitMQ | 竞争 + 广播 | 持久 topic exchange + quorum queue（`x-delivery-limit` 有界重试，现状；`publish` 在队列不可路由（如部署初期拓扑未就绪）时返回可重试 503，应用按退避重试） | `broadcast_exchange` fanout（默认 `<exchange>.broadcast`）；每进程声明**独占 auto-delete** 队列并绑定 | at-most-once（handler 失败只记录、不重投） | 节点离线期间的广播事件丢失；广播不写 DLX、不做重投；连接断开时排他队列被 broker 删除 |
| Kafka | **仅竞争** | 单分区 follower（不提交 offset） | **不支持** | — | 单分区 follower 没有 consumer group，也没有“每个消费者各收一份”的原语，无法在不伪造语义的前提下实现广播；`IEventPublisher::broadcast` 保持 trait 默认 **fail-closed**（显式错误），不会静默降级为单节点竞争投递 |

Kafka 要支持广播需要独立适配器（组协调/rebalance 客户端，如 `rdkafka`；或 topic-per-node
拓扑 + 自己的端口契约），不在当前 crate 内假装支持。

## 3. 明确不支持（当前基线）

| 能力 | 现状 | 原因 / 边界 |
|---|---|---|
| Redis Cluster / Sentinel | 不支持 | 当前用单节点 `redis::Client`；Cluster 需要 `ClusterClient` 与 MOVED/ASK 处理，Sentinel 需要拓扑发现；应作为独立适配器，端口契约不变 |
| Redis Pub/Sub 作为竞争通道 | 不支持 | 竞争投递仍用 Streams（at-least-once + PEL）；Pub/Sub 无持久化，仅用于广播（at-most-once） |
| NATS JetStream | 不支持 | 当前是 core NATS，语义 at-most-once；JetStream 需要 stream/consumer/ack，应为独立 crate |
| NATS 广播持久化 | 不支持 | core NATS 广播无 ack/重投；节点离线期间的广播事件丢失，持久广播需要 JetStream |
| RabbitMQ 广播持久化 / 重投 | 不支持 | 广播走 fanout + 每节点独占 auto-delete 队列（连接关闭时被 broker 删除）；节点离线期间的广播事件丢失（符合实时扇出 at-most-once 语义） |
| RabbitMQ classic queue | 不优先 | 默认声明 quorum queue（3.8+）；同名 classic queue 会 `PRECONDITION_FAILED`，需改名或删除重建 |
| Kafka 广播投递 | 不支持 | 见 §2：单分区 follower 无 consumer group/扇出原语；trait 默认 `broadcast` fail-closed 显式报错，不做静默降级 |
| Kafka consumer group / offset commit | 不支持 | `rskafka` 的 follower 模式简单、无 C 工具链；组协调/rebalance/commit 需要另一客户端（如 `rdkafka`）或独立适配器 |
| Kafka TLS/SASL | 不支持 | 当前只接受 `host:port` 明文；安全集群需扩展 options 与 `rskafka` TLS/SASL 配置 |
| 跨 broker 统一事务/outbox | 不在端口内 | 用应用侧 outbox + broker 的幂等/重投；不要假装一个跨 broker 事务 |

## 4. 演进规则（不增加用户心智负担）

1. **一个端口，多种实现**：新增能力优先做**独立 crate**（如 `rust-webx-redis-cluster`、
   `rust-webx-nats-jetstream`），不把可选依赖塞进现有 crate 的默认 feature。
2. **入口保持 `add_*`**：用户只换 crate + `add_*`，业务 `publish`/`#[handler]` 不变。
3. **语义必须显式**：每个 crate 的 rustdoc/README 必须声明投递语义、重试/死信、顺序、
   认证边界与**投递类**（竞争/广播）；禁止用“exactly-once”描述未实现的能力。
4. **核心零依赖**：core/host 不依赖任何 broker SDK；适配器只实现端口。
5. **先测量再优化**：新增 broker 先接入 `stability.yml` 的 `broker-integration`，
   用真实启动时间收敛 health `start_period/retries`；不凭猜测设置超时。
6. **可观测性统一**：死信/重连计数走 `EventMetrics`；按事件名的高基数维度留给应用 `IMeter`。
7. **广播 fail-closed**：适配器不实现广播时保留 trait 默认行为（显式错误），禁止把广播
   静默降级为竞争投递。

## 5. 当前验证状态

- 四个适配器在 PR CI 跑编译、单元、注册与共享契约（无 broker 时 env-gated 用例提前返回）。
- 2.9.2 广播用例（同样 env-gated）：Redis Pub/Sub 广播频道一发多收、NATS 广播 subject 两个
  无 queue group 订阅者各收一份、RabbitMQ fanout 两个独立独占队列各收一份、Kafka
  `broadcast` fail-closed 单测；nightly `broker-integration`（service containers）跑同一批
  用例。
- 真实 broker 由 nightly `broker-integration`（service containers）覆盖：发布/消费、
  `EventTransportContract`、Redis 缓存 TTL/DLQ/replay、Rabbit quorum/DLX。
- 本地 `docker-compose.brokers.yml` 与 `scripts/test-brokers.*` 可复现同一拓扑；
  无容器环境可用远端 Redis 或独立 `nats-server` 跑真实用例。

## 6. 无需 Docker 的验证路径

```powershell
# Redis：使用已有远端端点
$env:WEBX_REDIS_URL = 'redis://...'
cargo test -p rust-webx-redis --test integration -- --nocapture

# NATS：winget 安装独立 server，无需容器
winget install NATSAuthors.NATSServer
nats-server -p 4222
$env:WEBX_NATS_URL = 'nats://127.0.0.1:4222'
cargo test -p rust-webx-nats --test integration -- --nocapture
```

RabbitMQ（需要 Erlang + broker）和 Kafka（需要 JVM + broker）没有同等轻量的单文件
Windows 发行版；不要用假 broker 冒充真实语义。它们的真实语义由 nightly service
containers 验证，PR 仍然跑单元/注册/共享契约。所有 env-gated 用例（含广播）在未设置
`WEBX_*_URL` 时提前返回并通过。

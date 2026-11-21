# Broker 适配器能力矩阵与边界

> 目标：把 **可移植的端口契约** 与 **不可移植的投递语义** 分开。核心只承诺
> `IEventPublisher` / `IEventHandler` / `EventMessage`；适配器明确声明自己的语义和边界，
> 不用“最低共同点”牺牲可靠性，也不把 broker 特性泄漏到 core。

## 1. 总览

| 能力 | Redis | NATS | RabbitMQ | Kafka |
|---|---|---|---|---|
| crate | `rust-webx-redis` | `rust-webx-nats` | `rust-webx-rabbitmq` | `rust-webx-kafka` |
| 入口 | `add_redis` | `add_nats` | `add_rabbitmq` | `add_kafka` |
| 传输模型 | Streams consumer group | core NATS pub/sub | quorum topic queue | single-partition follower |
| 投递语义 | **at-least-once** | **at-most-once** | **at-least-once**（有界重试） | **at-most-once 近似**（不提交 offset） |
| 失败重试 | `XAUTOCLAIM` + `max_deliveries` | 无（断连即丢） | quorum `x-delivery-limit` 重入队 | 无（重启按 `start_offset`） |
| 死信 | `dead_letter_stream` + `replay_dead_letter` | 无 | 可选 DLX（broker-side） | 无 |
| 顺序 | 单 stream 内有序 | 单订阅者内有序 | 单队列内有序（重试会换位） | 单分区内有序 |
| 多实例 | consumer group 分摊 | queue group 分摊 | 同一 quorum queue 竞争消费 | **不支持组**；每实例独立 follower |
| 认证/TLS | URL（含 ACL/user），`rediss://` 已启用 | URL（`tls://` 需 feature） | URL（AMQP URI），凭据在 URL | **仅明文** `host:port` |
| 拓扑自动创建 | stream/group（`MKSTREAM`） | 无 | exchange/queue/binding | 可选 topic（`create_topic`） |
| 健康检查 | `PING` | TCP/连接 | `connection`/channel 可用性 | broker metadata / topic |
| 额外能力 | `IDistributedCache` | — | — | — |

## 2. 明确不支持（当前基线）

| 能力 | 现状 | 原因 / 边界 |
|---|---|---|
| Redis Cluster / Sentinel | 不支持 | 当前用单节点 `redis::Client`；Cluster 需要 `ClusterClient` 与 MOVED/ASK 处理，Sentinel 需要拓扑发现；应作为独立适配器，端口契约不变 |
| Redis Pub/Sub | 不支持 | 选择 Streams 是为了 at-least-once + PEL；Pub/Sub 无持久化 |
| NATS JetStream | 不支持 | 当前是 core NATS，语义 at-most-once；JetStream 需要 stream/consumer/ack，应为独立 crate |
| RabbitMQ classic queue | 不优先 | 默认声明 quorum queue（3.8+）；同名 classic queue 会 `PRECONDITION_FAILED`，需改名或删除重建 |
| Kafka consumer group / offset commit | 不支持 | `rskafka` 的 follower 模式简单、无 C 工具链；组协调/rebalance/commit 需要另一客户端（如 `rdkafka`）或独立适配器 |
| Kafka TLS/SASL | 不支持 | 当前只接受 `host:port` 明文；安全集群需扩展 options 与 `rskafka` TLS/SASL 配置 |
| 跨 broker 统一事务/outbox | 不在端口内 | 用应用侧 outbox + broker 的幂等/重投；不要假装一个跨 broker 事务 |

## 3. 演进规则（不增加用户心智负担）

1. **一个端口，多种实现**：新增能力优先做**独立 crate**（如 `rust-webx-redis-cluster`、
   `rust-webx-nats-jetstream`），不把可选依赖塞进现有 crate 的默认 feature。
2. **入口保持 `add_*`**：用户只换 crate + `add_*`，业务 `publish`/`#[handler]` 不变。
3. **语义必须显式**：每个 crate 的 rustdoc/README 必须声明投递语义、重试/死信、顺序、
   认证边界；禁止用“exactly-once”描述未实现的能力。
4. **核心零依赖**：core/host 不依赖任何 broker SDK；适配器只实现端口。
5. **先测量再优化**：新增 broker 先接入 `stability.yml` 的 `broker-integration`，
   用真实启动时间收敛 health `start_period/retries`；不凭猜测设置超时。
6. **可观测性统一**：死信/重连计数走 `EventMetrics`；按事件名的高基数维度留给应用 `IMeter`。

## 4. 当前验证状态

- Redis：真实端点 **7/7** 用例（cache TTL/sliding/clear、发布/消费、`start_id` 重放、
  未注册 no-op、`max_deliveries` → DLQ、`replay_dead_letter`）通过。
- NATS：**独立 `nats-server` 二进制**（非 Docker）真实 **2/2** 通过（roundtrip +
  `EventTransportContract`）。
- RabbitMQ / Kafka：编译、单元、注册、共享 `EventTransportContract` 通过；
  真实 broker 由 `.github/workflows/stability.yml` 的 `broker-integration` nightly 覆盖。
- 本地 Docker：`docker-compose.brokers.yml` 已通过 `docker compose config` 解析校验；
  当前工作站 Docker Desktop 引擎受 Windows 虚拟化功能影响，修复前不阻塞其他验证路径。

## 5. 无需 Docker 的验证路径

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
containers 验证，PR 仍然跑单元/注册/共享契约。

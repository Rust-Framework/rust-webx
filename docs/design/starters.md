# Starter 家族与端口契约测试（2.4-7）

> 原则：**框架拥有端口，starter 只做适配与装配**。跨进程只有一条传输通道——
> `IEventPublisher`/`EventDispatcher`（见 [架构 ADR](mediator-handler-architecture.md)）；
> starter 不引入第二套 pub/sub 或 dispatch 机制。

## 1. 已交付：可复用契约测试（2.4-7）

新增 crate **`crates/contract-tests`**（`rust-webx-contract-tests`，不发布）：为纯端口提供
行为级契约套件，任何适配器（内存实现/S3/SFU）都跑同一套断言。

| 套件 | 端口 | 断言 |
|---|---|---|
| `object_store_contract(&dyn IObjectStore)` | 对象存储 | put/get/head/delete、元数据（size/content-type/etag）、分片续传（同 part 重试、连续编号校验）、abort、路径穿越拒绝 |
| `media_plane_contract(&dyn IMediaPlaneAdapter)` | 媒体面 | 会话生命周期、轨道发布/订阅替换、未知会话 404、基线 QoS 形状、leave 幂等、`close_room` 清理 |

内存参考实现（`InMemoryObjectStore` / `InMemoryMediaPlaneAdapter`）在 PR CI 跑通全部套件
（`cargo test -p rust-webx-contract-tests`）。

**事件传输契约**：`crates/webx/tests/event_bus_test.rs` 覆盖发布语义、Handler 隔离与
consumer 路由；适配器级契约由本 crate 的 **`EventTransportContract`**（2.5）提供，
四个 broker 适配器的 `tests/integration.rs` 都已接入同一入口（见 §4）。契约本身在 PR CI
用 loopback 传输自测（`cargo test -p rust-webx-contract-tests`）。

### 适配器如何接入

```toml
[dev-dependencies]
rust-webx-contract-tests = { path = "../contract-tests" }
```

```rust
#[tokio::test]
async fn s3_store_satisfies_the_contract() {
    let store = S3ObjectStore::connect(&config).await.expect("s3");
    webx_contract_tests::object_store_contract(&store).await;
    // 适配器特有行为（预签名、分片大小）在共享套件之后单独断言。
}
```

## 2. Starter 清单与状态

| Starter | 端口/依赖 | 配置（starter 自持，`AppOptions` 之外） | 契约测试 | 状态 |
|---|---|---|---|---|
| **postgres** | rust-ef `DbContext` + 迁移（`bootstrap`/`migrate`） | `Database:ConnectionString`、`Database:Pool` | 迁移/bootstrap 烟测 + 就绪探针 | rust-ef 2.5 已提供；参考应用 docbit 在用；独立 starter crate 排入 2.5 |
| **redis** | `IDistributedCache` + 事件传输 `IEventPublisher`（+ consumer 后台服务） | `RedisOptions`（url/prefix/TTL/stream） | `event_bus_test` 语义 + env-gated `tests/integration.rs` | 已落地 `crates/redis`（`add_redis`） |
| **nats** | 事件传输 `IEventPublisher` | `NatsOptions`（url/subject/queue group） | 同上 | 已落地 `crates/nats`（`add_nats`） |
| **rabbitmq** | 事件传输 `IEventPublisher`（durable topic/queue） | `RabbitMqOptions`（url/exchange/routing key/queue） | env-gated `tests/integration.rs` | 已落地 `crates/rabbitmq`（`add_rabbitmq`） |
| **kafka** | 事件传输 `IEventPublisher`（单分区 follower） | `KafkaOptions`（brokers/topic/partition/start offset） | env-gated `tests/integration.rs` | 已落地 `crates/kafka`（`add_kafka`）；无 consumer-group offset 提交 |
| **s3** | `IObjectStore`（可选 `presign_put`） | `Storage:Endpoint/Bucket/Region/AccessKey/SecretKey` | `object_store_contract` | 契约套件已就绪；真实适配器（2.5，MinIO nightly） |
| **otel** | `IMeter` + `/metrics` 镜像 | `Otel:Endpoint`、`Otel:ServiceName`、采样 | 指标名/标签契约 + `MemoryMeter` 断言（含 `RealtimeMetrics::export_to`（scrape 时导出增量）） | 契约方向已定；OTLP exporter（2.5，可选 feature） |

> 为什么真实 SDK 适配器不在 PR CI：Redis/NATS/S3/OTLP 需要外部服务或容器；PR 用内存实现
> 与契约测试保证语义，真实服务在 `stability.yml` 的 nightly job 用 `services:` 跑同一契约。

## 3. 事件传输适配器（Redis/NATS/…）

事件传输只有一个端口 `IEventPublisher`，消费端复用框架的 `EventDispatcher`：

```rust
// publisher 侧：实现 IEventPublisher（event_name + payload 原样写入 broker）
#[async_trait]
impl IEventPublisher for MyBrokerPublisher {
    async fn publish(&self, message: EventMessage) -> Result<()> {
        self.send(message.event_name, message.payload).await
    }
}

// consumer 侧：从 broker 取回 EventMessage，路由回同一批 #[handler] / #[handler(inject)] 事件处理器
dispatcher.dispatch(&EventMessage::from_wire(bytes)?).await?;
```

装配：`.use_event_publisher(Arc::new(publisher))`（`add_mediator()` 不会覆盖应用注册）。
**实时跨节点扇出自动复用该通道**：发布 `RealtimeFanoutEvent` 即可，各节点的框架
fan-out handler 会写入本节点房间连接（见 [架构 ADR](mediator-handler-architecture.md) §2/§6）。

## 4. 真实 broker 集成（nightly，已闭环）

能力/语义边界以 [broker-capability-matrix.md](broker-capability-matrix.md) 为权威；
四个适配器都带 env-gated 集成测试（无 broker 时提前返回并通过，PR CI 零依赖），并已接入
`.github/workflows/stability.yml` 的 **`broker-integration`** job（service containers：
redis:7 / nats:2 / rabbitmq:3 / apache/kafka KRaft）：

| 适配器 | 环境变量 | 覆盖 |
|---|---|---|
| redis | `WEBX_REDIS_URL` | `IDistributedCache` roundtrip + 事件 transport（publisher → consumer group → `EventDispatcher` → Handler；失败重投/死信） |
| nats | `WEBX_NATS_URL` | 事件 transport 发布/消费（`EventMessage::to_wire/from_wire`） |
| rabbitmq | `WEBX_RABBITMQ_URL` | 事件 transport 发布/消费（durable exchange/queue） |
| kafka | `WEBX_KAFKA_BROKERS`（可选 `WEBX_KAFKA_TOPIC`） | 事件 transport 发布/消费（分区 consumer） |

本地（一键起 broker、等健康、跑完整适配器套件、自动清理）：

```text
pwsh ./scripts/test-brokers.ps1
# 或 Linux/macOS:
./scripts/test-brokers.sh
# 只校验 compose（不需要 Docker 引擎）：
pwsh ./scripts/test-brokers.ps1 -ValidateOnly
BROKER_VALIDATE_ONLY=1 ./scripts/test-brokers.sh
```

等价的手工步骤：

```text
docker compose -f docker-compose.brokers.yml up -d --wait
# NATS 无 healthcheck；等待 4222 后再跑
WEBX_REDIS_URL=redis://127.0.0.1:6379 \
WEBX_NATS_URL=nats://127.0.0.1:4222 \
WEBX_RABBITMQ_URL=amqp://guest:guest@127.0.0.1:5672/%2f \
WEBX_KAFKA_BROKERS=127.0.0.1:9092 \
  cargo test -p rust-webx-redis -p rust-webx-nats -p rust-webx-rabbitmq \
             -p rust-webx-kafka --tests -- --nocapture
docker compose -f docker-compose.brokers.yml down -v
```

健康等待：Redis `start_period 5s`、RabbitMQ `20s`、Kafka `30s`，CI 的 TCP
兜底等待上限 240s（覆盖 Kafka 最坏情况）。首次 nightly 全绿后，按 job 实际耗时
下调 `start_period/retries`；本地若 broker 未就绪，先看
`docker compose -f docker-compose.brokers.yml ps` 的 health 状态，不要直接跑测试。
若 `docker version` 返回 500，先修复 Docker Desktop 引擎/启动 `com.docker.service`
（这是本机环境问题，不是 compose 文件错误）。

每个适配器除自有的 roundtrip/重投/死信测试外，还跑共享的
`EventTransportContract`（同一事件名 + JSON payload 经 broker 到达框架
`EventDispatcher`）。


每个适配器只复用框架的 `IEventPublisher`、`EventMessage` 帧与 `EventDispatcher`，不复制
框架 dispatch/序列化；协议专属的重连/退避/健康检查各自实现（避免过度抽象）。

## 5. 验收与后续

- 当前：`cargo test -p rust-webx-contract-tests` 全绿（object-store / media-plane / event-transport
  三组契约，传输契约用 loopback 自测）；`event_bus_test` 覆盖事件语义；四个 broker 适配器的
  真实服务集成已接入 nightly（见上）；
- 后续：nightly `broker-integration` 首次真实运行确认服务健康参数；S3/OTel starter 的示例与契约；
  postgres starter 提供 `Database` 配置与迁移辅助。

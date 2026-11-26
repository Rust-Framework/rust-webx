# 端口与适配器归属（Ports & Adapters）

> 原则：**框架层只解决契约收敛；下游业务定义实体、做数据/逻辑适配。**
>
> 本页是**归属权威**：谁能发布什么、`examples/adapters/` 是什么、契约测试门禁、`*-ef` 的定位。
> 怎么接入（三种方式与代码）见 [端口与适配器（下游接入）](../rust-webx/13-extensibility/integrations.md)。
>
> 跨进程只有一条传输通道——`IEventPublisher`/`EventDispatcher`（见
> [架构 ADR](mediator-handler-architecture.md)）；适配器不引入第二套 pub/sub 或 dispatch 机制。

## 1. 发布序列只含框架 crate

| 位置 | 内容 | 是否发布 |
|---|---|---|
| `crates/` | 框架本体（core/engine/host/macros/spa/openapi/client/webx）、领域组（ai/auth/cache/queue/payment/push/transport）、`rust-webx-outbox` | **是**（家族内部精确 pin `=2.9.3`） |
| `examples/` | 三个参考应用（各自独立 workspace）与 `examples/adapters/` 参考适配器 | 否 |
| `testing/` | `contract-tests`、`alloc-probe`、`upgrade-smoke`、`load-probe` | 否 |
| `fuzz/` | 独立 cargo-fuzz workspace | 否 |

发布顺序（只列发布物）：

```text
rust-core → rust-dix → rust-ef
  → rust-webx 家族：rust-webx-core/host/engine/macros/spa/openapi/client/webx、
    领域组各 crate（含 rust-webx-apns / rust-webx-fcm）、rust-webx-outbox
```

- 家族内部用精确 pin（`=2.9.3`）互约束，`rust-webx-outbox` 与家族同版本；
- `rust-ef` 在应用侧保持 semver 范围（`"2.6"`），不要收紧成精确 pin（如 `=2.9.1`），否则 ORM 发布前 lockfile 不可解析；
- `examples/adapters/*` 全部 `publish = false`，**不进入发布序列**，也不参与 semver 检查；
- `scripts/check-publish-order.py` 校验 `publish.sh` / `publish.yml` 与 workspace 成员顺序一致；
  非发布成员（`publish = false`）不进入这份清单。

## 2. `examples/adapters/`：非发布参考实现

`examples/adapters/` 是**独立 workspace**（`examples/adapters/Cargo.toml`，自带 lockfile），
与根 workspace、三个参考应用都互不隶属：

```text
examples/adapters/
├── Cargo.toml                 # 独立 workspace，全部成员 publish = false
├── ai/ef/                     # rust-webx-ai-ef    用量账本参考实现
├── auth/ef/                   # rust-webx-auth-ef  会话/refresh 存储参考实现
├── idempotency/ef/            # rust-webx-idempotency-ef  幂等 store 参考实现
├── outbox/ef/                 # rust-webx-outbox-ef       outbox EF 存储参考实现
└── payment/ef/                # rust-webx-pay-ef          回调 inbox 参考实现
```

定位是**参考实现：可复制，不可依赖**。

- **为什么移出发布序列**：这些 crate 必须精确跟随 `rust-ef` 和下游数据库；表结构、迁移、租户字段
  又必须由下游业务模型决定。把它们发成 crate，等于把"某家的数据适配"塞进框架发布面；
- **复制即归属**：下游 `cp -r examples/adapters/outbox/ef crates/my-service-outbox` 后，包名、实体、
  表、迁移全部由下游自己的仓库决定，随业务演进，不需要等框架发版；
- **不承诺 API 稳定**：参考实现为了示例清晰可以改表名/列/注册函数签名，下游不应把它当 SDK；
- **仍然受完整测试约束**：参考实现自带测试（含 env-gated 真库测试），复制后可以直接沿用。

## 3. `*-ef` 的定位

| 参考适配器 | 实现的端口 | 依赖 | 发布 | 下游用法 |
|---|---|---|---|---|
| `idempotency/ef` | `IIdempotencyStore` | `rust-webx` + `rust-ef` | 否 | 复制进业务仓，或自己实现端口 |
| `outbox/ef` | `IOutbox`（EF 存储） | 同上 | 否 | 复制后 `add_outbox_ef` |
| `payment/ef` | 回调 inbox（`IPaymentEventHandler`） | `rust-webx` + `rust-webx-pay` + `rust-ef` | 否 | 复制后 `add_payment_inbox_with_handler` |
| `auth/ef` | `ISessionRepository` / `IRefreshTokenStore` | `rust-webx` + `rust-ef` | 否 | 复制后注册 store |
| `ai/ef` | `IUsageSink`（+ 账本 outbox/rollup worker） | `rust-webx` + `rust-webx-ai` + `rust-ef` | 否 | 复制后 `EfUsageSink` |

规则：

- 业务代码里**不出现** `webx_*_ef` / `rust_webx_*_ef` 的导入名；出现即说明把参考实现当依赖了；
- 复制进下游仓库后它**是下游自己的 crate**，可以改名、可以裁剪，也需要自己维护迁移；
- 框架侧不为它们保留兼容壳：`rust-webx-ef`（`webx_ef`）兼容壳已在 2.9.1 删除， <!-- docs:allow-stale-api -->
  持久层错误映射不需要任何框架 crate（`rust-ef` 提供 `From<EFError>`，handler 直接 `?`）。

## 4. 分类法：`crates/` 与 `examples/` 的边界

`crates/` 不按厂商平铺，而是**固定框架集 + 领域组**；新增适配器只能进已有组，数量不随第三方产品线性增长：

| 位置 | 成员 | 说明 |
|---|---|---|
| `crates/<framework>` | core / engine / host / macros / spa / openapi / client / webx / build | 已发布的框架本体，依赖方向只向内 |
| `crates/transport/` | h3 | 可选的非 hyper 监听后端（HTTP/3） |
| `crates/ai/` | core / gateway | AI 网关领域：端口与 IR、OpenAI 兼容表面 |
| `crates/auth/` | oidc / oauth2 / mfa / webauthn | 认证协议适配器；HTTP 认证 scheme 管线仍在 `host` |
| `crates/cache/` | redis | 第三方缓存；Redis 同时提供事件传输 |
| `crates/queue/` | nats / rabbitmq / kafka | 第三方消息队列（事件传输） |
| `crates/push/` | apns / fcm | 移动推送协议适配器（`IPushSender`）；WebPush 适配器待办，端口已支持 |
| `crates/payment/` | core / stripe / alipay / wechatpay / paypal / adyen / square | 支付领域：端口与供应商协议适配器 |
| `crates/outbox/` | `rust-webx-outbox` | 事务性 outbox 的端口、派发器与内存实现（不链接 rust-ef） |
| `examples/adapters/<domain>/ef` | ai / auth / idempotency / outbox / payment | 非发布参考实现：与 ORM/数据库耦合的数据适配 |
| `examples/{docbit,dmbit,imbit}` | 参考应用 | 各自独立 workspace，验收框架能力 |
| `testing/` | contract-tests 等 | 非发布测试工具；`crates/` 只允许可发布 crate |

依赖方向（P10「小核大生态」）：

- 框架 crate 与 umbrella **不得**依赖任何领域组 crate（适配器必须 opt-in）；
- 领域组 crate 只依赖 `rust-webx-core`/`rust-webx-client`/`rust-webx-host` 的端口；
  只有 `ai/gateway` 这类应用表面允许依赖 umbrella；
- `examples/adapters/*` 可以依赖 `rust-webx` + `rust-ef`（示例需要），但**没有任何框架 crate 反向依赖它们**；
- 根 workspace 与 `cargo test --workspace` 都不含 `rust-ef`；参考适配器单独
  `cargo test --workspace --manifest-path examples/adapters/Cargo.toml`；
- 需要契约套件的适配器（broker、对象存储、媒体面）通过相对 `path` 复用 `testing/contract-tests`，
  不复制契约套件；参考适配器提供自己端口的行为测试，作为下游复制后的模板。

门禁：`scripts/check-crate-layout.py`（CI Lint 调用 `scripts/check-crate-layout.sh`）校验成员分类、
孤儿 crate、框架→领域反向依赖、umbrella 是否链接适配器，以及 **`crates/` 只含可发布 crate、
非发布的测试工具必须位于 `testing/`、参考适配器必须 `publish = false`**；放错位置即 CI 失败。

## 5. 契约测试门禁

端口契约在 **`testing/contract-tests`**（`rust-webx-contract-tests`，不发布）。
它把端口文档写明的行为固化成可复用套件，任何实现都跑同一套断言：

| 套件 | 端口 | 断言 |
|---|---|---|
| `object_store_contract(&dyn IObjectStore)` | 对象存储 | put/get/head/delete、元数据（size/content-type/etag）、分片续传（同 part 重试、连续编号校验）、abort、路径穿越拒绝 |
| `media_plane_contract(&dyn IMediaPlaneAdapter)` | 媒体面 | 会话生命周期、轨道发布/订阅替换、未知会话 404、基线 QoS 形状、leave 幂等、`close_room` 清理 |
| `EventTransportContract` | 事件传输 `IEventPublisher` | 同一事件名 + JSON payload 经传输到达框架 `EventDispatcher` |

三条门禁：

1. **框架自带的默认/内存实现必须过套件**（`cargo test -p rust-webx-contract-tests`），
   它们同时是套件的自测基准；
2. **`crates/` 里的可选适配器必须接同一套件**：四个 broker 适配器的 `tests/integration.rs` 都通过
   `EventTransportContract`，Redis 另跑 `IDistributedCache` 往返；
3. **新增端口必须同批新增套件**（端口 + 默认实现 + 契约套件同一批提交）；
   参考适配器与原实现一起跑同一套件，否则评审不通过。

适配器接入方式：

```toml
[dev-dependencies]
rust-webx-contract-tests = { path = "../../../testing/contract-tests" }
```

```rust
#[tokio::test]
async fn s3_store_satisfies_the_contract() {
    let store = S3ObjectStore::connect(&config).await.expect("s3");
    webx_contract_tests::object_store_contract(&store).await;
    // 适配器特有行为（预签名、分片大小）在共享套件之后单独断言。
}
```

> 为什么真实 SDK 适配器不在 PR CI：Redis/NATS/S3/OTLP 需要外部服务或容器；PR 用内存实现与契约测试
> 保证语义，真实服务在 `stability.yml` 的 nightly job 用 `services:` 跑同一契约。

## 6. 事件传输适配器（仍属框架发布面）

传输/协议类适配器（broker、支付供应商、认证协议）留在 `crates/` 并随框架发布——它们不绑定下游数据模型。
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
**实时跨节点扇出自动复用该通道**：发布 `RealtimeFanoutEvent` / `RealtimeUserFanoutEvent` 即可，
各节点的框架 fan-out handler 会写入本节点房间/用户连接（见
[架构 ADR](mediator-handler-architecture.md) §2/§6）。

2.9.2 起事件有**投递类**（`EventDelivery::{Competing, Broadcast}`，由 `IEventRequest::delivery()`
声明，默认 `Competing`）：`IMediator::publish` 按类分派到 `IEventPublisher::publish` /
`IEventPublisher::broadcast`。广播投递要求适配器路由到广播通道（Redis Pub/Sub、NATS 广播
subject、RabbitMQ fanout），未声明能力的适配器由 `broadcast` 默认实现 **fail-closed** 报错
（Kafka 明确不支持）；语义矩阵见 [broker 能力矩阵](broker-capability-matrix.md)。

真实 broker 的能力/语义边界以 [broker-capability-matrix.md](broker-capability-matrix.md) 为权威。
四个适配器都带 env-gated 集成测试（无 broker 时提前返回并通过，PR CI 零依赖），并已接入
`.github/workflows/stability.yml` 的 **`broker-integration`** job（service containers）：

| 适配器 | 环境变量 | 覆盖 |
|---|---|---|
| redis | `WEBX_REDIS_URL` | `IDistributedCache` roundtrip + 事件 transport（publisher → consumer group → `EventDispatcher` → Handler；失败重投/死信） |
| nats | `WEBX_NATS_URL` | 事件 transport 发布/消费（`EventMessage::to_wire/from_wire`） |
| rabbitmq | `WEBX_RABBITMQ_URL` | 事件 transport 发布/消费（durable exchange/queue） |
| kafka | `WEBX_KAFKA_BROKERS`（可选 `WEBX_KAFKA_TOPIC`） | 事件 transport 发布/消费（分区 consumer） |

本地一键起 broker、等健康、跑完整适配器套件、自动清理：

```text
pwsh ./scripts/test-brokers.ps1
# 或 Linux/macOS:
./scripts/test-brokers.sh
# 只校验 compose（不需要 Docker 引擎）：
pwsh ./scripts/test-brokers.ps1 -ValidateOnly
BROKER_VALIDATE_ONLY=1 ./scripts/test-brokers.sh
```

每个适配器只复用框架的 `IEventPublisher`、`EventMessage` 帧与 `EventDispatcher`，不复制框架
dispatch/序列化；协议专属的重连/退避/健康检查各自实现（避免过度抽象）。

## 7. 验收

- `cargo test -p rust-webx-contract-tests` 全绿（object-store / media-plane / event-transport 三组契约，
  传输契约用 loopback 自测）；`event_bus_test` 覆盖事件语义；四个 broker 适配器的真实服务集成在 nightly；
- `cargo test --workspace --manifest-path examples/adapters/Cargo.toml` 覆盖五个参考适配器
  （默认 SQLite + env-gated Postgres/MySQL）；
- 发布清单只含框架 crate；`crates/` 无 `publish = false`，参考适配器无发布物。

# 端口与适配器（下游接入）

> **一句话**：框架只交付**端口契约**与**默认实现**；实体、表结构、事务边界和业务数据适配由下游定义。
> 接入 = 依赖 `rust-webx` → 在业务 crate 实现端口（或复制 `examples/adapters/*` 的参考实现）→
> 跑端口契约测试 → 在组合根 `register` / `use_*` 装配。
>
> 归属、发布序列与门禁的权威在 [端口与适配器归属](../../design/integrations.md)；本页只讲**怎么接**。

## 1. 框架给什么、不给什么

框架层的职责只有一条：**把能力收敛成稳定的 trait 契约**，再附一个能直接跑起来的默认实现。

| 框架交付 | 框架不交付 |
|---|---|
| 端口 trait 与值类型（`IIdempotencyStore`、`IOutbox`、`IObjectStore` …） | 集成层 / starters 这类"一站式套装"（下游按端口自行组装） |
| 零配置默认实现（内存、no-op），单进程与测试可直接用 | 与具体 ORM/数据库耦合的**已发布** crate（`*-ef` 只存在于 `examples/adapters/`） |
| 组合根装配动词：`use_*` 替换端口、`add_*` 增加能力、`register` 手工注册 | 业务实体、表结构、迁移脚本、多租户字段 |
| 端口契约测试（`testing/contract-tests`，不发布） | 数据库 / 消息队列 / 对象存储 / KMS 的选型与连接管理 |

**判据**：业务代码里只允许出现端口 trait 与值类型；任何具体实现（端口实现、实体、表）
都必须来自下游自己的 crate —— 要么在业务 crate 里写，要么把参考实现复制进去。

## 2. 端口清单

| 端口 | 框架内默认实现 | 自动注册 | 参考适配器（`examples/adapters/`，非发布） |
|---|---|---|---|
| `IIdempotencyStore` | `InMemoryIdempotencyStore`（有界 + TTL + fence token） | `Idempotency.Enabled=true` 时取 DI 最后一个注册，没有才用内存 | `idempotency/ef` → `EfIdempotencyStore`（`webx_idempotency` 表，多实例） |
| `IOutbox` | `InMemoryOutbox` + `OutboxDispatcher`（租约、退避、死信、保留期） | `add_outbox(store, OutboxOptions)`（`store` 传 `Arc<dyn IOutbox>`，用 `InMemoryOutbox` 即零配置） | `outbox/ef` → `add_outbox_ef`（EF 表 + 同事务 `enqueue_outbox`） |
| `ISessionStore`（jti 吊销） | `InMemorySessionStore` | 否（未注册 = 关闭吊销校验） | 无；下游实现吊销表 |
| `ISessionRepository` / `IRefreshTokenStore` | `InMemorySessionRepository` / `InMemoryRefreshTokenStore` | 否 | `auth/ef` → `EfSessionRepository` / `EfRefreshTokenStore` |
| `ITokenIssuer` | `JwtTokenIssuer` | 启用认证且 `Jwt.Secret` / `Jwt.SecretFile` 非空时 | 无 |
| `IPasswordHasher` | `Argon2PasswordHasher`（`BoundedPasswordHasher` 包裹；默认 feature `password-argon2`） | 是 | 无 |
| `IEmailSender` / `ISmsSender` | 无默认注册；`InMemoryEmailSender` / `InMemorySmsSender` 供测试 | 否（`use_email` / `use_sms`） | 无；SMTP/SES/Twilio 在宿主实现 |
| `ISecretProtector` / `ISecretStore` | 无默认 protector；`AesGcmSecretProtector` + `EnvSecretStore` / `FileSecretStore` / `ChainSecretStore` 可直接装配 | 否（`use_secret_protector`） | 无；KMS/信封加密在宿主实现 |
| `IObjectStore` | `InMemoryObjectStore` | 否 | 无；S3/OSS 等由下游实现并跑契约 |
| `IMeter` | `NoopMeter`；`Metrics.Enabled` 时换成 `MemoryMeter` 并接 `/metrics` | 是（仅内存指标桥） | 无；OTLP exporter 在宿主实现 |
| `IAdvisoryLock` | 无默认注册；`InMemoryAdvisoryLock` 供单节点/测试 | 否（`use_advisory_lock`） | 无；`rust-webx-redis` 的 `RedisAdvisoryLock` 是已发布实现 |
| `IRateLimiter` | `InMemoryRateLimiter` | `RateLimit` 配置节启用时 | 无；共享限流（Redis）在宿主实现 |
| `IEventPublisher` | 进程内内存队列 | `add_mediator()` 装配；`use_event_publisher` 替换 | 已发布的 broker 适配器 `rust-webx-redis` / `nats` / `rabbitmq` / `kafka`（`crates/`） |
| `IHealthCheck` | 无默认 | 否（`add_health_check(HealthCheckSpec::sync / async_check / async_arc)`，异步探针各自超时） | 无；DB/HTTP 探针在宿主实现 |
| `IRealtimeEventStore` | `InMemoryRealtimeEventStore` | 宿主存在 WebSocket 路由时 | 无；共享回放存储由下游实现 |

两点约定：

- **"自动注册 = 否"不是静默降级**：没有默认实现的端口，未显式注册时 DI 解析失败（启动期或首个请求即可见），
  不会悄悄换成内存实现——多实例场景下静默降级比启动失败更危险。
- 领域组 crate 自己的端口（`rust-webx-pay` 的 `IPaymentProvider`、`rust-webx-ai` 的 `IUsageSink` /
  `IApiKeyStore`、`rust-webx-auth` 的协议端口 …）由该 crate 提供；它们的持久化参考实现同样只在
  `examples/adapters/{payment,ai,auth}/ef`。支付回调 inbox 的 `IPaymentEventHandler` 属于参考适配器
  自带端口，不是框架端口。

## 3. 三种接入方式

### 3.1 用默认实现（单进程 / 原型 / 测试）

不要为了"以后可能要持久化"提前接数据库：默认实现就是给单进程与自动化测试用的。

```rust
use std::sync::Arc;
use webx::*;
use webx_outbox::*; // `add_outbox` 扩展 trait、`InMemoryOutbox`、`OutboxOptions`

Host::builder()
    // 幂等：只打开配置节，存储走 DI 里的（或默认的内存）实现
    .configure(|app| app.options(|o| o.idempotency.enabled = true))
    // outbox：内存存储 + 框架派发器（需要容器构建存储时用 `add_outbox_with`）
    .add_outbox(Arc::new(InMemoryOutbox::new(6)), OutboxOptions::default())
    .build();
```

边界写在明面上：进程重启即丢失、多实例之间不共享、容器重建后不可恢复。越过这条线就必须换共享实现
（3.2 或 3.3），不要把内存实现带进生产多实例。

### 3.2 复制参考适配器（要持久化时的默认选择）

`examples/adapters/` 下的五个适配器是**参考实现**，不是依赖项：复制进你的仓库，改成你自己的 crate。

```bash
cp -r examples/adapters/outbox/ef crates/my-service-outbox
```

复制后按顺序改四处：

1. **包名与导入名**：改成自己仓库的名字（例如 `my-service-outbox` / `my_service_outbox`）；
2. **实体与表**：把参考实体换成本服务的实体，表名/列/索引/约束声明在实体与其 `XxxConfiguration` 里，
   DDL 交给官方 rust-ef 迁移生成（不要手写 `CREATE TABLE`）；
3. **事务与租户**：确认入队发生在业务事务内，租户/主体只来自 `IdentityUser`，不接受 body/query；
4. **装配**：在组合根注册自己的扩展 trait / store，保持端口类型不变。

为什么不发布成 crate：

- **契约收敛 ≠ 数据适配**：框架只该收敛端口；表结构、迁移、租户字段每家的业务模型都不同；
- **版本耦合**：`*-ef` 必须精确跟随 `rust-ef` 与下游数据库版本，发布出去就等于把 ORM 版本塞进框架发布序列；
- **归属清晰**：复制后代码归下游所有，`publish = false`，可以随便改，不需要等框架发版。

### 3.3 自己实现 + 跑契约测试

只实现端口，不复制任何参考代码，是同样合法的路径——前提是**跑同一套契约**。

```toml
[dev-dependencies]
rust-webx-contract-tests = { path = "../../../testing/contract-tests" }
```

```rust
#[tokio::test]
async fn my_object_store_satisfies_the_contract() {
    let store = my_storage::S3ObjectStore::connect(&config).await.expect("s3");
    webx_contract_tests::object_store_contract(&store).await;
    // 实现特有的行为（预签名、分片大小）在共享套件之后单独断言。
}
```

现有套件（`testing/contract-tests`，不发布）：

| 套件 | 端口 | 断言范围 |
|---|---|---|
| `object_store_contract` | `IObjectStore` | put/get/head/delete、元数据（size/content-type/etag）、分片续传、abort、路径穿越拒绝 |
| `media_plane_contract` | `IMediaPlaneAdapter` | 会话生命周期、轨道发布/订阅替换、未知会话 404、QoS 形状、leave 幂等、`close_room` |
| `EventTransportContract` | `IEventPublisher` | 同一事件名 + JSON payload 经传输到达框架 `EventDispatcher` |

- 套件是**行为级**的：只断言端口文档写明的语义，不锁实现细节（顺序、批量、线格式）；
- 框架自带的内存实现必须跑通全部套件：`cargo test -p rust-webx-contract-tests`；
- 其余端口暂时没有共享套件，框架用自身行为测试锁语义（例如幂等 store 的租约围栏、outbox 的租约与死信）。
  为某个端口新增共享套件时，套件与原实现同批进 `testing/contract-tests`，并让参考适配器一起跑。

参考适配器自身也带测试（例如 `examples/adapters/outbox/ef/tests/`、`idempotency/ef/tests/`），
复制后可以直接沿用；真实服务（Postgres/MySQL 等）的验证用 env-gated `#[ignore]` 测试，CI 单独跑。

## 4. outbox：框架派发器 + 适配器存储

outbox 是"两层边界"的最好例子：**派发语义归框架，存储归下游**。

| | 框架 `rust-webx-outbox`（已发布） | 参考适配器 `examples/adapters/outbox/ef`（非发布） |
|---|---|---|
| 端口与类型 | `IOutbox`、`OutboxEntry`、`OutboxOptions` | rust-ef 实体（`webx_outbox` 表） |
| 存储 | `InMemoryOutbox`（单进程，重启即丢） | EF `DbContext`，多实例共享 |
| 同事务入队 | 不适用（框架只做派发）；单进程生产用 `InMemoryOutbox::enqueue` | `enqueue_outbox`（只 `add` 到当前 Unit of Work） |
| 派发 | `OutboxDispatcher`：租约领取、指数退避、死信、保留期清理 | 复用框架派发器，不重复实现 |
| 装配 | `add_outbox(store, OutboxOptions)` | `add_outbox_ef(OutboxOptions)` |
| 依赖 | 不链接 `rust-ef` | 链接 `rust-ef` 与应用的数据库 |

选型规则：

- 单进程、测试、原型 → `add_outbox(Arc::new(InMemoryOutbox::new(6)), OutboxOptions::default())`；
- 多实例 / 需要"业务与事件同生共死"→ 复制 `outbox/ef`（或自己实现 `IOutbox`）并 `add_outbox_ef`；
- 两者不要同时装：端口只有一个，`use_*` 语义是**后写胜**（后注册覆盖先注册）。

派发器的重试/死信/租约语义、配置节与迁移细节见 [事务性 outbox](outbox.md)。

## 5. 不要依赖内部 `*-ef` crate

`ai-ef` / `auth-ef` / `idempotency-ef` / `outbox-ef` / `pay-ef` 这五个适配器：

- 全部 `publish = false`：**crates.io 上没有它们**，写进 `Cargo.toml` 只会在解析依赖时失败；
- 没有版本对可钉：它们跟随 `examples/adapters/` 的示例仓演进，不参与框架发布序列；
- 依赖它们等于把"参考实现"当 SDK：实现细节（表名、实体、租约列）随时可能为了示例清晰而修改。

正确姿势只有两条：**在业务 crate 实现端口**，或**复制参考实现并归自己维护**（§3.2）。
`rust-webx-ef`（`webx_ef`）兼容壳已在 2.9.1 删除，持久层错误映射不需要任何框架侧 crate——
`rust-ef` 的 `From<EFError>` 让 handler 直接 `?` 即可（见 [兼容性政策](../../design/compatibility.md)）。

## 6. 接入自检

- 业务 crate 的 `Cargo.toml` 里只有 `rust-webx`（+ 领域 crate + 自己的 ORM），没有 `*-ef`；
- `grep` 业务代码：没有 `use webx_*_ef` / `rust_webx_*_ef` 的导入名；
- 端口实现跑过对应契约套件（或参考适配器的等价测试）；
- 内存实现只出现在单进程/测试路径，生产路径有共享实现；
- 组合根只装配一次：`use_*` 替换端口，业务代码不 `new` 第二份实现。

下一章：[最佳实践](../14-best-practices/INDEX.md)

# 适配器可扩展性架构评审（rust-webx 2.5 → 2.6）

> 评审对象：事件传输异步化 + 四个 broker/cache 适配器（`crates/redis`、`crates/nats`、`crates/rabbitmq`、`crates/kafka`）
> 评审人：独立架构评审（只读，未改动任何 Rust/Cargo/文档源文件）
> 分支 / 时间：`release/2.5`；初审 08:03–08:09，修复轮 1 复核 08:15–08:18，**终审 08:19–08:23**（2026-11-21，Asia/Shanghai）
> 状态提醒：四个适配器目录仍为未跟踪文件（`?? crates/redis/` 等），`crates/core/src/mediator/event.rs` 为未跟踪新文件。**本报告全程静态复核：未运行 `cargo build`/`test`/`clippy`**（避免持有 target 锁）；上游报告的「适配器 30 个测试 + core 事件测试通过、clippy `-D warnings` 通过」未被本评审独立执行验证，正文以源码与测试断言内容为准。

## 0. 评审方法与证据

只读命令：`git status` / `git diff` / `cargo tree` / `cargo metadata`、`Get-ChildItem` + 行数统计、`Select-String`/ripgrep、`rust-dix 1.0.0` / `rskafka 0.6.0` / `lapin 4.12.3` / `async-nats 0.53.0` 源码（registry 缓存）核对。
凡属**源码推断而非运行验证**的结论，正文标注「推断」。

---

## 1. 结论摘要

**总体判断：上轮遗留的两条 must-fix（毒消息无界重投、Redis PEL 无界增长）已关闭，评审范围内不再有 must-fix。** 当前形态是「语义正确 + 有界失败 + 有死信出口」的 starter 家族：未注册事件 ack、处理器失败重试有上限、超限进死信或丢弃、PEL 会清理、发布清单与文档对齐。**剩下的全部是「面向生产的增强项」**（指标、真实 broker CI、去重），不是返工项。

### 评分表（括号内为上一轮分）

| 维度 | 分 | 变化 | 证据 |
|---|---|---|---|
| 核心隔离 | **5**（5） | — | 终审重跑：`cargo tree -p rust-webx-core --depth 1` 与 umbrella 深度 2 过滤 broker 均为空；四个适配器 `-i` 均只有自身；core 无 feature、无驱动依赖 |
| API 一致性 | **3**（3） | — | 四份 `*HostBuilderExt`/`*ServiceCollectionExt` 同构；新 option 命名一致（`max_deliveries` / `dead_letter_stream` / `dead_letter_exchange`）。仍存：`connect_timeout_seconds` vs `connection_timeout_seconds`（`crates/nats/src/options.rs:19`）、Redis 无 `publisher_only()`、options 无法真正从配置喂给 `add_*`（§6.8）、4 处 `.expect()`；以及需文档化的 publish-fail-closed vs consume-fail-open 分叉（§4.6） |
| 失败语义 | **4**（4） | — | **A 关闭**：Redis `max_deliveries`（默认 5）+ `dead_letter_stream`（`crates/redis/src/options.rs:146-152`）、失败时 `XPENDING` 取投递次数、超限 XADD 死信后 `XACK`（`crates/redis/src/events.rs:130-175`）；Rabbit quorum 队列 + `x-delivery-limit`（默认 5）+ 可选 DLX（`crates/rabbitmq/src/events.rs:264-298`）。未到 5：Rabbit `mandatory=false`（`events.rs:84`）在路由不到队列时仍静默 confirm（§6.9 R2）；Kafka/NATS 的弱语义是明示的 scope limit |
| 可运维性 | **3**（2） | ↑1 | 有界重试 + 死信出口（Redis 死信 stream 带 `attempts`/`source_id`/`error` 字段，`events.rs:304-333`；Rabbit DLX 可配）+ classic→quorum 迁移已入档（`event-pub-sub.md:170-172`）。未到 4：指标仍为零、health 仍只探连接、payload 仍无上限、自定义 CA 无入口 |
| 可测试性 | **3**（3） | — | 4 个适配器共 **33 个测试函数**（19 单元 + 9 免 broker 注册 + 5 env-gated 集成）；新增 `xautoclaim_reply_reports_deleted_ids_for_pel_cleanup`（`redis/events.rs:472`）与 `xpending_reply_exposes_the_delivery_count`（`:484`）。未到 4：CI 仍无 broker（5 个 env-gated 集成用例空跑通过）、Rabbit 队列参数内联在 `ensure_topology` 里无法单测（§6.9 R3）、仍未接 `crates/contract-tests` |
| 依赖/构建负担 | **4**（4） | — | 发布路径已闭环（`publish.yml:49`、`publish.sh:88,187-190`）；workspace 仍需三平台编译 4 个 SDK，属结构性成本 |
| 文档 | **4**（4） | — | CHANGELOG 2.6.0 段已写清 Redis「trimmed-entry PEL cleanup / max_deliveries → dead-letter stream」（`CHANGELOG.md:106-107`）与 Rabbit「quorum topic queue / x-delivery-limit / optional DLX」（`:111-112`）；手册表格与正文同步（`event-pub-sub.md:114,116,140,170-172`）；README 两个驱动行更新（`README.md:249,251`）。未到 5：无 example、无每驱动文档页 |

均分 **26/35 ≈ 3.7**（初审 22/35 ≈ 3.1，修复轮 1 为 25/35 ≈ 3.6）。

---

## 2. 核心污染检查

### 2.1 依赖方向（终审不变）

| 检查 | 结果 |
|---|---|
| `cargo tree -p rust-webx-core --depth 1` 过滤 broker | 空 |
| `cargo tree -p rust-webx --depth 2` 过滤 broker | 空 |
| `cargo tree -i <四个适配器>` | 各自只有自身 |
| core/host/macros 源码提及 broker | 仅 doc 注释（`crates/core/src/mediator/event.rs:18,96`、`crates/host/src/server.rs:391`） |

方向：`core ← host ← {redis, nats, rabbitmq, kafka}`。三轮修复（no-op 语义、wire 版本、有界重试/PEL 清理）**一次都没有把驱动语义搬进 core**——毒消息与死信完全落在适配器 + broker 侧，这是本设计最重要的纪律，且被守住了。

### 2.2 为适配器而改的 core（一次性成本，累计 +54 行）

| 文件 | 现状 | 三轮累计变化 |
|---|---|---|
| `crates/core/src/mediator/event.rs` | 767 行（生产 505 + 内联测试 262） | 较初审 +54 行：wire 版本字节与校验、两处 permanent no-op、4 个测试 |
| `crates/core/src/mediator.rs` | `event_name()` 默认 `type_name`（`:68`）、`publish` 需 `Serialize`（`:130`） | 未变 |
| `crates/core/src/route/ext.rs` | `add_mediator()` 注册默认传输（`:198-247`） | 未变 |
| `crates/macros/src/handler.rs` | `#[handler]` 生成 `EventRegistration`（`:316-386`） | 未变 |
| `crates/host/src/server.rs` | `use_event_publisher`（`:406`） | 未变 |

合计约 **1,075 行**（含测试）。仍是「一次性端口 + 默认实现」，不随驱动数量增长。

### 2.3 `to_wire`/`from_wire`：可版本化，但不等于「完全 additive」

- 格式（`crates/core/src/mediator/event.rs:100-116`）：`u8` 版本（`EVENT_WIRE_VERSION = 1`，`:155`）+ `u32` 大端名字长度 + UTF-8 名字 + payload；`from_wire`（`:119-151`）先校验版本，未知版本 `Err("unsupported event wire version N")`，截断/超长检查保留。
- 真实价值：把「版本缺失导致的静默误解析」变成「显式拒绝」——旧无版本帧首字节是名字长度最高字节 `0x00`，现在会被干净拒绝。
- **不要过度解读**：只有 1 字节版本、没有 flags/header 长度字段。加 `traceparent`/投递次数仍要升 v2 并双版本兼容；这是「可版本化」，CHANGELOG（`:117`）写「additive wire change」偏乐观，建议在 wire 契约处写明「v2 消费者必须继续接受 v1」。
- `MAX_EVENT_NAME_BYTES` 仍 64 KiB（`:158`）；payload 仍无上限（§6.2）。

### 2.4 全局状态 / feature flag

- 无 feature flag、无 broker 类型进 core（三轮一致）。
- `inventory::collect!(EventRegistration)`（`:189`）仍是链接期单例表；同名不同事件「先到先得 + 一条 warn」。
- **需文档化的语义分工**：
  - publish 端（仅内存传输）：无本地注册 → `Err`，fail-closed（`:368`；`crates/webx/tests/event_bus_test.rs:133-147` 仍断言 `Error::Di`）；
  - consume 端（所有传输）：无注册/无订阅者 → `warn` + `Ok(())`，fail-open，让 at-least-once 传输 ack（`:280-293`、`:448-459`）。
  两者都对，但同一事件名在两种传输下行为不同（内存 publish 报错、broker 静默丢弃）。手册尚未写这条，见 §7.4 N2。

---

## 3. 扩展成本实测（终审口径）

### 3.1 文件与行数（未跟踪文件，`git diff --stat` 看不到）

| crate | 文件 | 生产源码 | 内联测试 | 集成测试 | Cargo.toml | 合计 |
|---|---|---|---|---|---|---|
| `crates/redis` | 9 | 1,220 | 152 | 217 | 33 | **1,622** |
| `crates/nats` | 8 | 429 | 35 | 161 | 34 | **659** |
| `crates/rabbitmq` | 8 | 657 | 42 | 171 | 34 | **904** |
| `crates/kafka` | 8 | 594 | 26 | 185 | 34 | **839** |
| 合计 | **33** | **2,900** | 255 | 734 | 135 | **4,024** |

（`src` 合计 3,155 行，其中 255 行 `#[cfg(test)]` 内联单测；`tests/` 目录 734 行 = 免 broker 的注册测试 + `WEBX_*_URL` 门控的集成测试。三轮里 redis 从 1,069 → 1,220 生产行（+151：死信、XPENDING、PEL 清理、`start_id`/`claim_pending`），rabbitmq 从 625 → 657（+32：quorum 参数与选项）。）
单文件最大：`crates/redis/src/events.rs` 507、`crates/redis/src/cache.rs` 339、`crates/rabbitmq/src/events.rs` 312、`crates/redis/src/lib.rs` 191。

### 3.2 每个适配器必须实现的注册面（不变）

5 件事 + 2 个扩展 trait：`*Options`（serde + `new` + `From<&str>`）、`*Connection`（懒连 + 超时 + `ping`）、`impl IEventPublisher`、`impl BackgroundService`（`run_once` + 退避）、`impl IHealthCheck`；`*ServiceCollectionExt::add_*`/`add_*_with`、`*HostBuilderExt::add_*`/`add_*_with` + `add_health_check_async`。

用到的框架公共 API 全部已公开，无需改 core/host：

```
webx_core::mediator::{EventDispatcher, EventMessage, IEventPublisher}
// to_wire/from_wire 内部用公开常量 EVENT_WIRE_VERSION（适配器不直接 import）
webx_core::hosted::{delay, BackgroundService, CancellationToken}
webx_core::error::{Error, Result}
webx_core::route::ext::IServiceCollectionExt
webx_host::server::HostBuilder | webx_host::health::{HealthStatus, IHealthCheck}
rust_dix::ServiceCollection / IServiceResolver::provider_arc()
```

### 3.3 「加第 5 个驱动要改什么？」

| 必须改 | 位置 |
|---|---|
| 新建 crate | `crates/<drv>/{Cargo.toml, src/{lib,options,connection,events,health}.rs, tests/registration.rs}`（约 400–700 行） |
| workspace 成员 + 依赖 | 根 `Cargo.toml`（`version = "=x.y.z"` 由 CI `Internal version pins` 强制） |
| 锁文件 | `Cargo.lock` |
| 发布清单 | `publish.yml:49` + `publish.sh:88`（`internal_deps`）与 `:178-192`（`publish_crate` 顺序） |
| 文档 | `README.md` / `README.zh-CN.md` / `event-pub-sub.md` / `CHANGELOG.md` / `starters.md` |

**结论：仍不需要改 core/host/macros。** 本案还提供了一个反例证据：给 Redis/Rabbit 加「有界重试 + 死信」这种传输特有语义时，改动**全部**落在适配器与 options 上（`max_deliveries`/`dead_letter_stream`/quorum 参数），core 一行未动——说明端口抽象在扩展方向上是对的。

---

## 4. 四驱动语义对照（终审）

| 维度 | redis（Streams） | nats（core） | rabbitmq（AMQP，quorum） | kafka（rskafka） |
|---|---|---|---|---|
| 投递保证 | **at-least-once**：dispatch 成功才 `XACK`（`events.rs:132`）；pending 由 `XAUTOCLAIM` 认领；**失败有界**（`max_deliveries`） | **at-most-once**（无 ack/无重放，明示） | **at-least-once**：publisher confirm + 手动 ack；**broker 侧有界**（`x-delivery-limit`） | **弱 at-most-once**：不提交 offset（明示 starter 限制） |
| 未注册事件 | **ack + warn**（dispatch 返回 Ok） | 丢弃 + warn | **ack + warn** | 丢弃 + warn |
| 处理器失败 | 计投递次数；<上限留 pending 等 `XAUTOCLAIM`，≥上限 → 可选写死信 stream → `XACK` + error 日志（`events.rs:133-171`） | 丢弃 + 日志 | `nack(requeue=true)`；quorum 队列把消息放回**队尾**（不再队头阻塞），超 `x-delivery-limit` 后进 DLX 或丢弃（`ensure_topology:264-286`） | 丢弃 + offset 日志 |
| 顺序 | 单 stream 内有序；批次串行 | 发布者内近似有序 | 单队列内有序；重入队改变顺序（但不再饿死后续消息） | 单分区内有序 |
| 背压 / 上界 | `MAXLEN ~`（默认 100k）+ `COUNT 64` + `start_id`/`claim_pending` 可配 | 无（依赖 `max_payload`） | `prefetch` 32；队列参数含 `x-delivery-limit`，**未设 `x-max-length`** | `max_wait_ms` 长轮询；无速率控制 |
| 健康检查 | `PING` | `flush()`（错误已脱敏） | `connection.status()` | `list_topics()` |
| 停机 | `select!` 于 `ct.cancelled()` | 同 | 同 | 同 |
| 关键限制 | `claim_pending=false` 时无 PEL 清理/无上限（§6.9 R1）；死信 stream 与主流同名会自环 | 订阅前消息永久丢失 | 需 RabbitMQ 3.8+；classic 同名队列需改名/重建；无 `x-max-length` | 单分区、无 offset 提交、**纯明文（已入档）** |

### 4.1 Kafka：单分区 / 无 offset 提交 / 纯明文（已入档，维持 scope limit）

- 单分区 follower，无 rebalance；无 `commit`：`Latest` 丢停摆期消息、`Earliest` 重放。
- 明文限制三处明示：`crates/kafka/src/options.rs:5-8`、`CHANGELOG.md:114`、`event-pub-sub.md:179-181`。
- **仍开（性能）**：`ensure_topic()` 在 `partition()` 里先于缓存检查执行（`crates/kafka/src/events.rs:44-50`），每次都 `list_topics()`（`:226-233`）→ 每条 publish 一次多余 metadata RPC。

### 4.2 NATS：at-most-once（已脱敏）

- 无 ack/持久化/JetStream；订阅者不在线即丢。
- 连接错误经 `redact_url`（`crates/nats/src/connection.rs:40-48`，实现 `:64-81`，单测 `:87-94`），`/health/ready` 不再回显凭据。
- 残余（低）：`NatsConnection::url()`（`:27-30`）仍返回原文。

### 4.3 Redis：毒消息有界 + PEL 清理（**A/B 均关闭**）

- **B 关闭**：`parse_claim_reply` 现返回 `(messages, deleted_ids)`，从 `XAUTOCLAIM` 的 `parts[2]` 取被裁剪但仍挂 PEL 的 id（`crates/redis/src/events.rs:353-370`）；`run_once` 在投递认领条目**之前**逐个 `XACK`（`:106-114`）。测试 `xautoclaim_reply_reports_deleted_ids_for_pel_cleanup`（`:472`）。
- **A 关闭**：失败时用 `XPENDING key group <id> <id> 1` 精确查询该条目的投递次数（`:287-302`，解析 `:372-383`）；`attempts >= max_deliveries` 时先 `XADD` 死信（带 `event/payload/attempts/source_id/error`，`:304-333`）再 `XACK`（`:142-161`）；未达上限则保持 pending（`:162-170`）。**顺序正确**：XADD 失败会 `Err` 返回，条目仍在 PEL，不会「既删源又没写死信」。
- 残余：`claim_pending=false`（Redis <6.2）时 `XAUTOCLAIM` 根本不跑 → `max_deliveries` 与 PEL 清理同时失效，条目永久 pending；options doc 未点明这个耦合（§6.9 R1）。死信 stream 与主流同名时消息会自环（未校验）。

### 4.4 RabbitMQ：broker 侧有界（**A 关闭**）

- 队列以 `x-queue-type=quorum` + `x-delivery-limit=max_deliveries`（默认 5）声明，可选 `x-dead-letter-exchange` + `x-dead-letter-routing-key`（`crates/rabbitmq/src/events.rs:264-298`）；publisher 与 consumer 共用 `ensure_topology`，参数一致，不会互相 `PRECONDITION_FAILED`。
- `nack(requeue=true)` 保留（应用侧语义不变），但 quorum 队列把重入队消息放**队尾**：上一个版本担心的「毒消息队头回插饿死后续」在 quorum 下自然消失；超限后由 broker 进 DLX 或丢弃。
- 迁移已入档：`event-pub-sub.md:170-172` 写明 RabbitMQ 3.8+、DLX 需预建、**已存在的同名 classic 队列需换名或删除重建**——这是本轮最有价值的文档补充（否则升级即 `queue.declare` 失败）。
- 残余：DLX 若未预建且 `mandatory=false`，超限消息会在 DLX 处静默丢弃；`x-dead-letter-strategy`（默认 at-most-once）在 DLX 不可达时也可能丢消息；队列参数内联在 `ensure_topology`，无法单测（§6.9 R3）。

### 4.5 共性：消费者串行（不变）

一次一条：`prefetch=32` 只起预取缓冲作用，吞吐 ≈ 单条延迟；I/O 型 handler 靠同队列多实例扩展。文档仍未说明。

### 4.6 缺失注册是「永久 no-op」（修复轮 1，维持）

| 路径 | 行为 | 位置 |
|---|---|---|
| 事件名无 `#[handler]` 注册 | `warn` + `Ok(())` → 传输 ack | `crates/core/src/mediator/event.rs:280-293` |
| 有注册但 DI 无 `IEventHandler<T>` | `warn` + `Ok(())` | `:448-459` |
| 处理器返回 `Err` / panic | 汇总 `Err`（多处理器全部执行） | `:461-488` |
| 内存传输 publish 无本地注册 | `Err(Error::Di)`，fail-closed | `:368`；测试 `crates/webx/tests/event_bus_test.rs:133-147` |
| broker 传输 publish | 不看本地注册 | 各适配器 `publish` |

设计正确（永久失败不该重试、跨服务事件不该在无订阅者时堵塞），但属**静默丢弃**：拼错 `event_name`、漏 `#[handler]`、漏 DI 注册都只剩一条 warn。在指标为零的现状下生产上几乎不可发现 → N2 仍是最值得做的一项。

---

## 5. 重复与抽象评估

### 5.1 四份 `execute` 循环仍逐字相同

Redis `crates/redis/src/events.rs:185-207`、NATS `crates/nats/src/events.rs:107-129`、Rabbit `crates/rabbitmq/src/events.rs:223-245`、Kafka `crates/kafka/src/events.rs:200-222`——**各 23 行**，行级 diff 差异仍为 4/4/6 行（只有 log 字段名与文本）。**92 行里 78 行完全相同。** 三轮修复都没有动这段（说明它确实稳定），但也说明它仍在四处漂移风险中。

### 5.2 其他重复

| 重复物 | 位置 | 规模 | 评价 |
|---|---|---|---|
| `redact_url` | `crates/{redis,rabbitmq,nats}/src/connection.rs` | 17 行 × 3，逐字相同 | 修 M2 时复制了第三份；应上收为公共函数（含统一单测） |
| `*HealthCheck` | 四个 `health.rs` | 30–33 行 × 4 | 探针语义不同，抽象收益低，可保留 |
| 注册闭包（publisher + worker + `.expect`） | 四个 `lib.rs` 各约 12 行 | 48 | 可用 `add_event_consumer(|provider| …)` 收掉，顺带消灭 4 个 panic 点 |
| 懒连接两种写法 | `OnceCell<_>`（redis/nats）vs `Mutex<Option<Arc<_>>>`（rabbit/kafka） | 59–116 × 4 | 统一写法即可，不建议泛型化 |
| 交付语义的「重试/上限」实现 | Redis（应用侧 XPENDING+死信）vs Rabbit（broker 侧 x-delivery-limit+DLX） | — | **不应统一**：两者分别是所在传输的惯用做法，硬抽会得到一个谁都不像的抽象 |

### 5.3 方案权衡（结论不变）

| 方案 | 判断 |
|---|---|
| A. `webx_core::hosted::run_consumer_worker` + 公共 `redact_url` | **推荐**：消 78 行逐字重复 + 2 份 `redact_url`；退避语义单点、可加 jitter、可接指标、有测试 |
| B. 新建 `webx-adapter-kit` | 不推荐：为约 50 行付一次发布税（成员、钉版本、publish 清单、semver、MSRV、文档） |
| C. 接受重复 | 仅在适配器冻结时可接受 |

**行数估算**：`run_consumer_worker` 约 35–45 行 + 4 处 2–3 行调用 → 净减 35–45 行；`redact_url` 上收约 20 行替换 51 行 → 净减约 30 行。合计净减 65–75 行，并消除 4 个 `.expect` 与 3 份漂移风险。

---

## 6. 风险与缺口（终审状态）

### 6.1 凭据与 TLS

| 驱动 | 状态 |
|---|---|
| redis | ✅ `rediss://` + URL 凭据 + 错误串脱敏；`url()` 返回原文（有 doc 警告） |
| nats | ✅ 泄露关闭；**仍开**：自定义 CA/客户端证书/NKey/JWT 无入口（`ConnectOptions` 未接 `require_tls`/`add_root_certificates`/`add_client_certificate`；推断自 async-nats 0.53 `connector.rs:1321-1336`） |
| rabbitmq | ✅ `amqps://` 可用、脱敏；**仍开**：自定义 CA/客户端证书/SASL EXTERNAL 无入口 |
| kafka | ✅ 明文已明示为 scope limit；转正需 `tls_config`/`sasl_config` |

### 6.2 Payload 大小（仍开）

事件名上限 64 KiB（`event.rs:158`），payload 无上限。NATS 超 `max_payload` 返 503（SDK 文本）；Rabbit/Kafka 受 broker 限制；Redis 大 value 吃 `MAXLEN` 配额。建议加 `max_payload_bytes`，超限在 publish 前返回确定性错误。

### 6.3 毒消息（**已关闭**）

| 驱动 | 坏帧 | 永久失败 |
|---|---|---|
| redis | 不 ack → 重投（有界） | ✅ 有界 → 死信 stream / 丢弃 |
| rabbitmq | ✅ ack 掉 | ✅ `x-delivery-limit` → DLX / 丢弃（非队头重排） |
| nats / kafka | 丢弃 | 丢弃（明示 at-most-once / starter 限制） |

残余见 §6.9 R1/R2。

### 6.4 可观测性（仍开，重要性最高）

- **指标：零**（四个适配器源码对 `IMeter` 零引用）。缺：发布量/失败率、handler 失败率、死信数、重连次数、pending/lag。
- **trace：断的**；但 wire 版本字节已就位，加 `traceparent` 现在是「v2 + 双读」的增量工作。
- **日志**：字段名不统一（`stream`/`subject`/`queue`/`topic`）；no-op 丢弃与死信只有日志。

### 6.5 无 broker 的 CI（仍开）

`.github/workflows/ci.yml:40` 仅 `cargo test --workspace`；5 个 env-gated 集成用例无 broker 时空跑通过。新增的单元测试覆盖了 `XPENDING`/`XAUTOCLAIM` 解析，但 ack/requeue/死信/quorum 参数这些**真实 broker 行为**仍未在 CI 里跑过。`starters.md:80-81` 已把 nightly `services:` + `event_transport_contract` 列为后续计划。

### 6.6 发布（关闭；版本簿记提醒仍在）

- `publish.yml:49`、`publish.sh:187-190`、`publish.sh:88`（`internal_deps` → `core host`）、`:23-26` 顺序注释均一致。
- 提醒：适配器 `version.workspace = true` 当前为 `2.5.0`，CHANGELOG 归 `2.6.0`；若 2.5.0 已发布，需 `./publish.sh --do --ver 2.6.0` 统一 bump（`check-docs.sh` 不会发现这项）。

### 6.7 启动期 broker 不可用（仍成立）

- 注册不连接；`add_health_check_async` 3s 超时（`crates/host/src/health.rs:23`、`server.rs:1205`）→ 503。
- 体验问题：`connect_timeout_seconds` 默认 5s > 探针 3s，broker 挂时 `detail` 恒为 `timed out after 3000ms`。
- 语义缺口：health 只探连接不探消费者；消费者卡死时 `/health/ready` 仍 `pass`。

### 6.8 其他（状态）

1. **配置绑定仍断**：`RedisOptions` doc 建议 `add_options::<RedisOptions>("Redis")`（`crates/redis/src/options.rs:8-9`），但 `add_options` 只把值注册进 DI（`crates/host/src/server.rs:452-470`），`add_redis(options)` 在建容器前就要值；缺 `add_*_from_config(section)`。
2. **重复注册静默覆盖**：四份 `lib.rs` 仍 `instance::<dyn IEventPublisher>` 无条件追加（`crates/redis/src/lib.rs:87` 等），rust-dix 取最后一个（`provider.rs:765`，推断）；`add_redis` 默认开事件传输，所以「Redis 缓存 + RabbitMQ 事件」会双消费且无告警（需 `.cache_only()`）。
3. **4 处 `provider_arc().expect(...)`** 仍在。
4. **Redis cache 信封兼容性**：`decode_entry` 以 `bytes[0]==1 && len>=13` 判定信封并丢前 13 字节（`crates/redis/src/cache.rs:255-266`），`0x01` 开头的二进制值会被静默截断。
5. **`event_name()` 默认 `type_name`**：重命名即换 wire 名；无编译期提示。
6. **`::webx::` 绝对路径**：`#[handler]` 生成代码依赖 umbrella；只用 `webx_core` + 适配器须手写 `EventRegistration`（`crates/nats/tests/integration.rs:26-46`）。

### 6.9 本轮新增的残余风险（均为低/中，不构成 must-fix）

| # | 残余 | 说明 | 建议 |
|---|---|---|---|
| **R1** | `claim_pending=false` 时上限与 PEL 清理**同时失效** | `XAUTOCLAIM` 是取 `deleted_ids` 与投递计数的唯一入口（`events.rs:104`），关闭后失败条目永久 pending；`options.rs:143-145` 的 doc 只说「新条目仍走 XREADGROUP」 | doc 补一句「关闭 = 放弃有界重试与 PEL 清理」 |
| **R2** | Rabbit `mandatory=false` | `BasicPublishOptions::default()`（`events.rs:84`）→ 路由不到队列时 broker 丢弃而 confirm 仍 `Ack` | 设 `mandatory: true` + 处理 `basic.return`，或文档写明「topology 必须与消费者一致」 |
| **R3** | quorum/DLX 队列参数无法单测 | 内联在 `ensure_topology`（`events.rs:268-286`），需要 `Channel` 才能跑 | 抽 `fn queue_arguments(&RabbitMqOptions) -> FieldTable` 纯函数 + 单测 |
| **R4** | Redis 死信 stream 可与主流同名 | 未校验，`dead_letter_stream == stream` 时事件自环 | 启动时校验并返回配置错误 |
| **R5** | 死信无「再投递」入口 | 死信只是存放（stream/DLX），没有重放工具 | 文档给出 `XADD`/shovel 的运维做法即可 |

---

## 7. 验收结论

### 7.1 是否「足够简单稳定、不干扰核心」？

**是——无放行条件。**

- 「不干扰核心」：成立，三轮修复零核心语义外溢（毒消息/死信全在适配器 + broker 侧）。
- 「足够简单」：成立（每驱动 3 trait + 2 扩展 trait；最简驱动 nats 429 行生产代码）。
- 「稳定」：**成立（在明示的 scope limit 内）**——数据面无界重投路径已全部封死：未注册 → ack；处理器失败 → 有界 → 死信/丢弃；Redis PEL 会清理。NATS at-most-once、Kafka 无 offset 提交/明文是文档化的能力边界，不是缺陷。

### 7.2 全部 must-fix 的关闭状态

| # | 问题 | 关闭 | 证据 |
|---|---|---|---|
| M1（轮 1） | 未注册事件被当作 handler 失败 → 无限重投 | ✅ | `crates/core/src/mediator/event.rs:280-293`、`:448-459`、`:461-488`；测试 `:625-644` |
| M2（轮 1） | NATS 凭据经 `/health/ready` 泄漏 | ✅ | `crates/nats/src/connection.rs:40-48,64-81`；单测 `:87-94` |
| M3（轮 1） | 发布清单漏四 crate + CHANGELOG 归属错误 | ✅ | `publish.yml:49`、`publish.sh:88,187-190`、`CHANGELOG.md:96,138,140` |
| **A（轮 2）** | **永久失败无限重投（Rabbit / Redis）** | ✅ **关闭** | Redis：`max_deliveries` 默认 5（`crates/redis/src/options.rs:146-148,167`）、`dead_letter_stream`（`:149-151,168`）、`XPENDING` 精确计数（`events.rs:287-302`，解析 `:372-383`）、超限 XADD 死信后 XACK（`:142-161`）、未超限留 pending（`:162-170`）；测试 `xpending_reply_exposes_the_delivery_count`（`:484`）。Rabbit：quorum + `x-delivery-limit`（默认 5）+ 可选 DLX（`crates/rabbitmq/src/events.rs:264-298`；`crates/rabbitmq/src/options.rs:20-28,40-41`），publisher/consumer 共用 `ensure_topology`；迁移已入档（`event-pub-sub.md:170-172`） |
| **B（轮 2）** | **Redis PEL `deleted_ids` 被丢弃 → PEL 无界增长** | ✅ **关闭** | `parse_claim_reply` 返回 `(messages, deleted_ids)` 并读 `parts[2]`（`crates/redis/src/events.rs:353-370`）；`run_once` 在投递前逐个 `XACK`（`:106-114`）；测试 `xautoclaim_reply_reports_deleted_ids_for_pel_cleanup`（`:472`） |

**评审范围内已无 must-fix。** 剩余的 §6.9 R1–R5 与 §6.2/§6.4/§6.5 均为「生产增强项」，不阻塞当前 starter 定位。

### 7.3 Top 3 nice-to-have（更新）

| # | 项 | 理由 |
|---|---|---|
| **N1** | 事件指标（`events.published/publish_failed/handler_failed/dead_lettered/dropped_unregistered/consumer_reconnects`）+ wire v2 trace header；手册写清「未注册事件 = 静默丢弃」「死信在哪查」 | 唯一仍能造成「线上 silently wrong」的缺口：no-op 与死信都只有日志；wire 版本已就位，header 是增量 |
| **N2** | `webx_core::hosted::run_consumer_worker` + 公共 `redact_url` | 78 行逐字重复 + `redact_url` 三份；顺手消灭 4 个 `.expect` |
| **N3** | nightly `services:`（redis/nats/rabbitmq/kafka）+ `event_transport_contract`；Rabbit 队列参数抽纯函数单测；`ensure_topic` 结果缓存；`add_*_from_config(section)` | 与 `starters.md:80-81` 计划一致；把 ack/requeue/DLX/quorum 行为纳入 CI |

### 7.4 建议处理顺序

N1（指标，让其余一切可见）→ N3 的 broker CI（否则 ack/DLX 行为仍只有人读代码保证）→ N2（去重）→ §6.9 的 R2/R3/R4 小修。

---

## 8. Follow-up verification（三轮变更复核）

复核时间 08:15–08:23；**只读**：未执行 `cargo build`/`test`/`clippy`。

| # | 上游声称 | 复核证据 | 结论 |
|---|---|---|---|
| 1 | `EventDispatcher::dispatch` 未注册 → warn + `Ok(())` | `crates/core/src/mediator/event.rs:280-293`；测试 `:625-633` | ✅ 属实 |
| 2 | `dispatch_event_handlers` 无 DI handler → warn + `Ok(())` | `:448-459`；测试 `:635-644` | ✅ 属实 |
| 3 | 处理器失败仍 `Err` | `:461-488`；测试 `:646-671`、panic `:673-691` | ✅ 未被削弱 |
| 4 | NATS `redact_url` + 单测 | `crates/nats/src/connection.rs:40-48,64-81`；测试 `:87-94` | ✅ 属实 |
| 5 | `publish.yml` 含四 crate | `.github/workflows/publish.yml:49`；版本一致性循环覆盖 `$CRATES` 全表 | ✅ 属实；顺序（host 后、webx 前）依赖正确 |
| 6 | `publish.sh` 含四 crate + `internal_deps` | **仓库根** `publish.sh`（非 `scripts/`）：`:187-190`、`:88`（`core host`）、`:23-26` 注释 | ✅ 属实（注意路径口径） |
| 7 | wire 版本字节 + 拒绝未知版本 | `:100-116`、`:119-151`（`HEADER_LEN = 5`）、`:155`；测试 `:604-615`、`:617-623` | ✅ 属实；旧无版本帧按 `version 0` 明确拒绝 |
| 8 | CHANGELOG 2.6.0 / 表述 / Kafka 限制 | `CHANGELOG.md:96`、`:138`、`:140`、`:114`、`:117` | ✅ 属实，与代码一致（`crates/core/src/mediator.rs:64,130`） |
| 9 | `starters.md` 四适配器 + env-gated | `docs/design/starters.md:4`、`:45-48`、`:79`、`:80-81` | ✅ 属实 |
| 10 | Redis `start_id` / `claim_pending` | `crates/redis/src/options.rs:128-131,143-145`（默认 `"$"` / `true`）；`events.rs:185`、`:104` | ✅ 属实 |
| 11 | Redis `max_deliveries` + `dead_letter_stream` + XPENDING | `options.rs:146-152,167-168`；`events.rs:133-175`（失败分支）、`:287-302`（XPENDING 精确 id 范围）、`:304-333`（死信 XADD 含 attempts/source_id/error）；测试 `:484` | ✅ 属实；XADD 在 XACK 之前，XADD 失败不会丢源条目 |
| 12 | Rabbit quorum + `x-delivery-limit` + 可选 DLX | `crates/rabbitmq/src/events.rs:264-298`（`x-queue-type=quorum`、`x-delivery-limit`、可选 `x-dead-letter-exchange`/`-routing-key`）；`crates/rabbitmq/src/options.rs:20-28,40-41`（默认 5 / 空）；publisher 与 consumer 均调用 `ensure_topology` | ✅ 属实；重入队由队头变队尾，副作用是顺序改变（已在 §4 表格标注） |
| 13 | Redis PEL 清理（`parts[2]` → XACK） | `crates/redis/src/events.rs:353-370`、`:106-114`；测试 `:472` | ✅ 属实；清理在投递之前执行，顺序正确 |
| 14 | 适配器 30 测试 + core 事件测试 + clippy 通过 | 本评审未执行；实测测试函数数 **33**（19 单元 + 9 免 broker 注册 + 5 env-gated 集成），与「全部通过」不矛盾（5 个集成用例无 broker 时空跑） | ⚠️ 未独立验证（只读约束）；数量与结构已核对 |

**复核未发现**：core/host 新增编译期依赖、新增 feature、wire 版本校验遗漏、发布列表回归、`redact_url` 漏用、XADD/XACK 顺序错误、publisher/consumer 队列参数不一致。
**复核未运行**：`cargo test`/`clippy`（上游报告通过，但本评审只做静态确认）；`cargo tree -i` 期间偶发 package-cache 锁等待，未影响结果。

---

*本报告只读产出；除本文件外未修改任何 Rust / Cargo / 文档源文件。*

---

## 9. 终审后的小修（Lead，08:24+）

§6.9 的 R1–R5 已在终审后处理；报告主体保留终审时点结论，本节只记录增量：

| 项 | 处理 | 证据 |
|---|---|---|
| R1 `claim_pending=false` 耦合 | `RedisEventsOptions::claim_pending` 文档明确「同时停掉 pending 重试、`max_deliveries` 与 PEL/deleted-id 清理」 | `crates/redis/src/options.rs` |
| R2 Rabbit `mandatory=false` 静默丢弃 | 发布改为 `mandatory=true`；`Ack(Some(returned))` 映射为可重试 503，不再把「未路由」当成功 | `crates/rabbitmq/src/events.rs` |
| R3 队列参数不可单测 | 抽出 `queue_arguments(&RabbitMqOptions) -> FieldTable`，新增 quorum/`x-delivery-limit`/DLX 两个单测 | `crates/rabbitmq/src/events.rs` |
| R4 DLQ 自环 | `dead_letter_stream == stream` 时告警并降级为丢弃，不向源流自环 | `crates/redis/src/events.rs` |
| R5 死信重放入口 | 文档给出 Redis `XADD`/`XREADGROUP` 与 RabbitMQ shovel/DLX consumer 两种重放方式 | `docs/rust-webx/08-mediator-events/event-pub-sub.md` |

**Lead 验证（本轮实际执行）**：`cargo test` 四适配器 + core 事件测试通过；`cargo clippy -p core -p redis -p nats -p rabbitmq -p kafka --all-targets --no-deps -- -D warnings` 通过；`cargo check --workspace --all-targets` 通过；`RUSTDOCFLAGS=-D warnings cargo doc ...` 通过。R2/R3/R4 的 broker 级行为仍需 nightly `services:` 验证（N3）。

### 9.1 真实 Redis 验证（用户提供的单节点端点，凭据不回显）

`WEBX_REDIS_URL` 指向真实 Redis（TCP 可达、AUTH/PING 通过），执行
`cargo test -p rust-webx-redis --test integration -- --nocapture`：**6/6 通过**。

| 用例 | 覆盖 | 结果 |
|---|---|---|
| `cache_roundtrip` | `set/get/exists/remove` + `PING` 健康探针 | ✅ |
| `cache_ttl_sliding_size_legacy_and_clear` | 绝对 TTL、滑动续期、`size_limit`、无信封裸值、前缀 `clear` | ✅ |
| `event_roundtrip` | 发布→consumer group→handler→`XACK` | ✅ |
| `start_id_zero_replays...` | 组创建在 `0` 时重放消费者启动前的事件 | ✅ |
| `unregistered_event_is_acked...` | 未注册事件 no-op + `XACK`，不留在 PEL | ✅ |
| `failing_event_reaches_dead_letter...` | handler 失败 → `XPENDING` 计数 → `max_deliveries=2` → 死信流 + `XACK` | ✅ |

真实服务暴露并已修复的 3 个问题（静态评审未发现）：

1. **`XREADGROUP` 取消竞态**：原实现用 `select!` 定时器与读取竞争；Redis 已把消息移入 PEL 后
   读取 future 被 timer 取消，回复丢失，消息只能等 claim。改为 **claim 与 read 顺序执行**
   （claim 只在两次读取之间跑），实时测试从 5s 超时变为 2s 内通过。
2. **`XPENDING` 扩展回复形状解析错**：带 `start end count` 时 Redis 返回
   `[[id, consumer, idle, delivery_count]]`，原解析按 summary 的 `parts[3][0][3]` 读取，
   `attempts` 恒为 0，`max_deliveries` 永不触发。改为同时识别两种形状并取条目最后一项；
   实时 DLQ 用例通过（attempts=1→2→丢弃）。
3. **`XAUTOCLAIM` 只读第一批 + cancelled/deleted ids 未处理**：改为游标循环、逐批 `XACK`
   deleted ids；malformed entry 也显式 ack，避免 PEL 毒化。

同时补上 `command_timeout_seconds`（`ConnectionManagerConfig::set_response_timeout`）与
`start_id` / `claim_pending` / `max_deliveries` / `dead_letter_stream` 的实时语义验证。
NATS/RabbitMQ/Kafka 在本次会话没有外部端点可测，但仓库已有
`.github/workflows/stability.yml` 的 `broker-integration` job（redis/nats/rabbitmq/kafka
service containers），由 nightly 兜底；本次仍以 Redis 真实端点作为主证据。

### 9.2 事件 Handler 的自动装配（反馈闭环）

反馈「事件 Handler 的 DI 注册仍是显式的」属实，但初版修复引入了 `auto` / `manual` 新词汇，
反而增加了选择成本。终版按**与请求 Handler 完全一致**的两种构造方式收敛，不新增模式：

- `#[handler]`：生成路由 + `EventHandlerRegistration`，用 `Default` 构造，**零 DI 注册**；
- `#[handler(inject)]`：用 `#[derive(Inject)]` 构造，**只注册依赖**，不注册 handler；
- 每个事件新建 handler，共享状态放入注入依赖（与“每请求新建请求 Handler”同一模型）；
- `EventDispatcher` 从 inventory 构造 handler；只有事件**没有** inventory handler 时才回退到
  手动 DI（框架内部 fan-out / 旧测试），因此两种来源不会重复执行，且手动 DI 不是用户可选模式；
- `IEventHandler<T>` **保持不变**，不引入 `Any` 父 trait，semver 门禁解除。

回归：`crates/webx/tests/event_bus_test.rs` 新增 default（无 DI）、inject（依赖注入）、
同事件多 handler fan-out 用例，全部通过；11 个 core 事件测试与 7 个 event_bus 测试保持通过。
这次是**有意的行为变更**：`#[handler]` 在事件 impl 上不再只是路由元数据，而是和请求
Handler 一样负责构造；自定义构造器统一走 `#[handler(inject)]` + `#[derive(Inject)]`。

### 9.3 N1 可观测性 / 死信重放

- `EventMetrics` 由 `add_mediator()` 注册，`/metrics` 输出 `events_published_total`、
  `events_publish_failed_total`、`events_dispatch_failed_total`、
  `events_dropped_no_handler_total`、`events_dead_lettered_total`、
  `events_consumer_reconnects_total`；无新配置/模式。
- Redis `replay_dead_letter(count)` 用 `XRANGE` → `XADD` → `XDEL` 重放，成功后才删死信；
  真实 Redis DLQ 用例已覆盖（`failing_event_reaches_dead_letter_and_acks`）。
- 剩余运维项：RabbitMQ broker-side DLX 无法在应用侧精确计数；完整 replay 工具可后续
  做成独立 CLI，不进入框架 API。

**SemVer 本地复核（2.6 轮）**：`cargo semver-checks` 对 `rust-webx-core` /
`rust-webx-engine` / `rust-webx-host` / `rust-webx` 均以 `v2.1.0` 为基线通过
（每包 201 项检查，0 failure）；`IEventHandler` 未加 `Any` 约束，门禁解除。

### 9.4 非 Docker 验证（按“不要死磕 Docker”要求）

- Redis：远端真实端点 **7/7**。
- NATS：`winget` 安装独立 `nats-server`（2.15.3），本机 4222 端口真实
  **2/2**（roundtrip + `EventTransportContract`）。
- RabbitMQ/Kafka：无轻量单文件 Windows broker，不伪造真实语义；保留单元/注册/
  共享契约 + nightly service containers。
- 本地 Docker 的 compose 文件与脚本已做客户端校验；引擎修复不再作为其他验证的前置条件。

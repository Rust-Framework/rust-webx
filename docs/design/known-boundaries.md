# 已知边界台账（Known Boundaries）

> 本页是 rust-webx 实时（WebSocket / 续传 / presence / 事件传输 / broker）相关**边界**的
> 唯一权威台账。手册与 design 页只讲"怎么用"和"为什么这样决策"，边界结论一律指向本页，
> 不再重复细节；修复或改变一条边界时，同批更新本页对应行与相关文档的指针。
>
> 分类：
>
> - **C1 代码边界（本轮修复）**：2.9.2 工作树已修复并交付契约/测试；保留说明是为了防止回退或误用。
> - **C2 适配器/实现缺口**：需要独立 crate 或应用侧端口实现，框架不假装支持。
> - **C3 设计内接受**：当前设计内明确接受的行为边界，不是缺陷，不承诺在 2.x 改变。
> - **C4 部署/运营约束**：升级、混布、运维窗口的约束，需要部署流程配合。
>
> 状态取值：**已修复** / **接受** / **待独立适配器** / **运营约束**。
> 关联页：[IM 实时硬化](im-hardening.md)（2.9.2 决策与验收）、
> [broker 能力矩阵](broker-capability-matrix.md)（各适配器完整语义）、
> [下游能力缺口](upstream-gaps.md)、[兼容性政策](compatibility.md)、
> [多实例实时部署](../rust-webx/11-production/realtime-cluster.md)。

## C1 代码边界（本轮修复，2.9.2）

| 编号 | 边界 | 证据 | 影响 | 状态 | 替代路径与验收 |
|---|---|---|---|---|---|
| C1-01 | Redis 续传的**物理保留仍是条数**（`XTRIM MAXLEN ~` 近似裁剪，`max_events` 默认 1024 封顶内存）；新增 `RedisRealtimeEventStore::with_max_age(Duration)` 是**逻辑回放窗口**：`resume` 过滤 `now - created_at > max_age` 的事件并置 `truncated=true`，全部被过滤时 `next_cursor` 前进；`latest_cursor` 仍跟随物理保留。 | [`realtime.rs`](../../crates/cache/redis/src/realtime.rs)、[`realtime_max_age.rs`](../../crates/cache/redis/tests/realtime_max_age.rs) | 不能据此认为 Redis 按时间删数据；时间语义只在回放侧，Redis 内存仍由条数决定 | 已修复 | `with_max_age` 是 store builder，**不**经 `RedisRealtimeEventStoreOptions`/`appsettings`；`cargo test -p rust-webx-redis --test realtime_max_age`（`max_age_window_filters_old_events_and_advances_the_cursor`） |
| C1-02 | Redis 事件传输 `start_id` 默认改为 `"0"`：消费组首次创建时重放 stream 内保留历史，避免"事件已发布、消费组尚未创建"的启动窗口丢事件；handlers 是 at-least-once，必须幂等。`"$"` 是显式 opt-in。已有 consumer group 上该值被 Redis 忽略。 | [`options.rs`](../../crates/cache/redis/src/options.rs)、[`events.rs`](../../crates/cache/redis/src/events.rs)（`ensure_group`） | 新组启动会重放 `max_len` 内历史；handler 不幂等会重复处理 | 已修复 | 显式设置 `RedisEventsOptions::start_id = "$"`；升级影响见 C4-03 |
| C1-03 | `add_redis` 在 **host 启动期**用 hosted service 校验本地配置（stream/group/broadcast_channel/start_id/max_len/read_count/block_ms/claim_idle_ms/max_deliveries/dead_letter_stream 以及 URL 解析，**不联网**），失败让 host 启动失败（typed `di` 错误），而不是在后台重连循环里永久静默重试。 | [`validation.rs`](../../crates/cache/redis/src/validation.rs)、[`options.rs`](../../crates/cache/redis/src/options.rs)、[`lib.rs`](../../crates/cache/redis/src/lib.rs) | 配置错误在启动暴露；校验 hosted service 的 `order() = -10_000`，先于消费者/worker 启动 | 已修复 | `cargo test -p rust-webx-redis --test validation`（`invalid_event_options_fail_host_startup`、`invalid_start_id_fails_host_startup`）；错误文案含 `invalid Redis configuration` |
| C1-04 | 坏帧/不可解析 entry：id 可读但 `event`/`payload` 缺失的 entry 按 **raw id `XACK`**，不再留在 PEL 反复毒化消费者；只记 warn 日志。 | [`events.rs`](../../crates/cache/redis/src/events.rs)（`ack_malformed`/`parse_entries`） | 坏 entry 被丢弃（不可重放）；告警可见但没有独立指标 | 已修复 | 无需配置；`cargo test -p rust-webx-redis --lib`（`malformed_entries_are_reported_for_acking`） |
| C1-05 | `XPENDING` 解析失败视为**传输错误**（不再当 0 次投递）：错误向上传播、消费者按重连/退避处理，entry 保持 pending 等 `XAUTOCLAIM` 再投，避免真正达到 `max_deliveries` 的毒消息被永远当作首投。 | [`events.rs`](../../crates/cache/redis/src/events.rs)（`pending_delivery_count`/`parse_pending_count`） | 回复形状异常时可见（错误而非静默 0），但本轮投递会失败重试 | 已修复 | `cargo test -p rust-webx-redis --lib`（`xpending_reply_exposes_the_delivery_count`） |
| C1-06 | Redis presence 身份**稳定**：ZSET member = JSON 数组 `[user_id, connection_id, node_id]`（不含 join 时间戳）；新增 HASH `{prefix}:members:{scope}`（field = 身份，value = `PresenceMember` JSON，保存 `joined_at`）；join/leave/touch/members/count 共 5 个 Lua 脚本，共享批量过期清理（256/批）。因此**跨节点/重启**的 `leave`/`touch` 能命中同一成员，不再依赖进程内索引。 | [`presence.rs`](../../crates/cache/redis/src/presence.rs)、[`presence_identity.rs`](../../crates/cache/redis/tests/presence_identity.rs) | 身份可含分隔符（如管道符）而不串号；`members()` 能回读 `joined_at`；清理不会让 HASH 积累陈旧 field | 已修复 | 部署无需配置；`cargo test -p rust-webx-redis --test presence_identity`（`cross_node_leave_and_touch_use_the_stable_identity`、`two_instances_see_the_same_members_and_refresh_without_duplicates`） |
| C1-07 | presence `join` 的容量检查与写入在**同一脚本原子完成**（`ZCARD` 对比 `max_members_per_scope`）：scope 满返回可重试 503 且**不写成员状态**；并发 join 不会越界。 | [`presence.rs`](../../crates/cache/redis/src/presence.rs)（`JOIN_SCRIPT`） | 容量上限按 scope 计（默认 10 万）；满时新连接无法被加入共享在线态 | 已修复 | `cargo test -p rust-webx-redis --test presence_identity`（`concurrent_joins_cannot_overshoot_capacity`） |
| C1-08 | presence 心跳在 host 内部改为 **32 路有界并发** touch，周期仍是 TTL/3（最小 1s）；单个 touch 失败只记日志、下一轮重试，不中断其他成员。 | [`presence.rs`](../../crates/host/src/presence.rs)（`heartbeat`） | 大节点不会因串行 round-trip 在 TTL 内续不完；同时并发有上界 | 已修复 | `cargo test -p rust-webx-host --lib presence` |
| C1-09 | 框架**保留事件**（`RealtimeFanoutEvent`/`RealtimeUserFanoutEvent`）由 host 启动期校验：应用对它们写 `#[handler]` 会让 host 构建失败（typed 错误，带事件名与 handler 名），而不是静默替换 DI 桥接、导致跨节点扇出丢失。 | [`fanout.rs`](../../crates/core/src/realtime/fanout.rs)（`validate_reserved_event_handlers`）、[`server.rs`](../../crates/host/src/server.rs)（`prepare`）、[`reserved_event_handler_test.rs`](../../crates/host/tests/reserved_event_handler_test.rs) | 应用不能订阅保留事件；自定义跨节点事件必须用独立 `IEventRequest` 类型 | 已修复 | `cargo test -p rust-webx-host --test reserved_event_handler_test`；错误文案含 `framework-reserved` 与事件名 |
| C1-10 | 兼容 2.9.2 presence 旧数据：旧 ZSET member（整段 `PresenceMember` JSON、无 HASH）在 `members()` 中回退解码；过期清理按分数删除并顺带 `HDEL`；新节点的 `leave`/`touch` 不命中旧 JSON member，旧条目随 TTL 自然过期。 | [`presence.rs`](../../crates/cache/redis/src/presence.rs)（`decode_members`/`decode_identity`）、[`presence_identity.rs`](../../crates/cache/redis/tests/presence_identity.rs) | 升级窗口可能出现同一身份的旧 JSON + 新身份两条记录（见 C4-02） | 已修复 | `cargo test -p rust-webx-redis --test presence_identity`（`legacy_json_members_are_still_readable`） |
| C1-11 | `PresenceMember::key()` 不再是裸 `\|` 拼接，改为**长度前缀**身份编码：id 内含分隔符时不会与另一个 `(user, connection, node)` 三元组碰撞（in-memory 实现用该 key 作索引）；同时 scope/member 的 key 组件限制为 ≤512B 且无控制字符。 | [`presence.rs`](../../crates/core/src/realtime/presence.rs)（`PresenceMember::key`/`validate`）、[`presence.rs`](../../crates/cache/redis/src/presence.rs)（`validate_component`） | 若应用把 `key()` 当展示字符串或持久化键，格式变了；它只承诺同一身份稳定，不承诺跨版本字面格式 | 已修复 | `cargo test -p rust-webx-core --lib realtime::presence`（`member_identity_encoding_cannot_be_ambiguous`、`oversized_or_control_key_components_are_rejected`） |

## C2 适配器/实现缺口（需独立 crate 或应用实现端口）

| 编号 | 边界 | 证据 | 影响 | 状态 | 替代路径与验收 |
|---|---|---|---|---|---|
| C2-01 | **PG 续传日志实现缺失**：框架不提供 PG 版 `IRealtimeEventStore`；Redis 实现只有条数物理保留 + 逻辑 `max_age` 回放窗口（C1-01），不提供持久/墙钟级时间保留。 | [`event_store.rs`](../../crates/core/src/realtime/event_store.rs)（端口）、[`realtime.rs`](../../crates/cache/redis/src/realtime.rs)、[integrations.md](../rust-webx/13-extensibility/integrations.md) | 需要"按时间持久保留、跨 Redis 重建仍可回放"时，框架侧无法提供 | 待独立适配器 | 应用实现 `IRealtimeEventStore`（PG 表：subscription + event_id + created_at），接 `HostBuilder::use_realtime_event_store`，跑与 `InMemoryRealtimeEventStore` 相同的契约 |
| C2-02 | **Redis Cluster / Sentinel 不支持**：`RedisConnection` 是单节点客户端（`redis://`/`rediss://`）；且 presence 的 Lua 脚本跨 ZSET + HASH **两个 key**，Cluster 下不同 slot 会 `CROSSSLOT`，需要 hash tag 或专用适配器。单机（直连 master）行为与 2.9.2 相同，Sentinel 拓扑发现不在实现内。 | [`presence.rs`](../../crates/cache/redis/src/presence.rs) 模块注释、[`options.rs`](../../crates/cache/redis/src/options.rs)、[broker 能力矩阵](broker-capability-matrix.md) §3 | Cluster 拓扑下 presence/脚本不可直接用；Sentinel 拓扑发现不在实现内 | 待独立适配器 | 独立 `rust-webx-redis-cluster`/Sentinel 适配器（端口契约不变）；Cluster key 需同 slot（`{...}` hash tag） |
| C2-03 | **Kafka 广播不支持**：单分区 follower 没有 consumer group/扇出原语；`IEventPublisher::broadcast` 保持 trait 默认 **fail-closed**（显式 `di` 错误），不静默降级为单节点竞争投递。 | [`event.rs`](../../crates/core/src/mediator/event.rs)（`IEventPublisher::broadcast` 默认实现）、[broker 能力矩阵](broker-capability-matrix.md) §2 | 实时跨节点扇出不能选 Kafka；Kafka 只用于竞争投递（任务/领域事件） | 待独立适配器 | 换支持广播的传输，或独立 Kafka 组感知适配器；`broadcast` 失败按"传输未接受"处理 |
| C2-04 | **Kafka consumer group / offset commit 与 TLS/SASL 不支持**：适配器的 follower 模式只连明文 `host:port`，重启按 `start_offset` 重新开始；组协调/rebalance/提交需要另一客户端（如 `rdkafka`）。 | [`crates/queue/kafka`](../../crates/queue/kafka)、[broker 能力矩阵](broker-capability-matrix.md) §3 | 不能承诺 exactly-once 或精确续传；多分区与安全集群需替换实现 | 待独立适配器 | 独立组感知适配器；当前用 `start_offset` + 应用幂等兜底（at-most-once 近似） |
| C2-05 | **NATS JetStream 不支持**：当前是 core NATS（at-most-once）；持久广播/消费确认/重投需要 JetStream（stream/consumer/ack），应为独立 crate。 | [`crates/queue/nats`](../../crates/queue/nats)、[broker 能力矩阵](broker-capability-matrix.md) §3 | 节点离线期间的广播事件丢失；不能把 core NATS 当持久队列 | 待独立适配器 | JetStream 独立适配器；需要可靠投递时用持久化 broker + outbox/续传 |
| C2-06 | **WebPush 仅端口**：`PushPlatform::WebPush` 与 `IPushSender` 端口存在，但 WebPush 适配器未交付（APNs/FCM 已发布适配器）。 | [im-hardening.md](im-hardening.md) §4、[push-notifications.md](../rust-webx/13-extensibility/push-notifications.md) | 选 WebPush 平台会得到 `Err`（无实现），不能假装端到端可用 | 待独立适配器 | 不要注册 WebPush 实现；需要时实现 `IPushSender`（VAPID/加密），按 `PushOutcome` 三态映射 |

## C3 设计内接受

| 编号 | 边界 | 证据 | 影响 | 状态 | 替代路径与验收 |
|---|---|---|---|---|---|
| C3-01 | `resume` 单次回放体量受 `max_events` 限制（默认 1024 条，**无额外分页参数**）；更早的事件由 `truncated=true` 告知，客户端必须按快照重建，不能继续增量。 | [`realtime.rs`](../../crates/cache/redis/src/realtime.rs)、[`event_store.rs`](../../crates/core/src/realtime/event_store.rs) | 大断档无法靠一次 `resume` 补齐；实时日志不是消息存储 | 接受 | 业务快照 + 增量重放；更大的消息历史用业务存储，不要把实时日志当消息表 |
| C3-02 | `IRealtimeEventStore::latest_cursor` 端口返回 `u64`；Redis 查询失败记 warn 并返回 `0`（端口签名不变，不改成错误返回）。 | [`realtime.rs`](../../crates/cache/redis/src/realtime.rs)（`latest_cursor`） | 失败时 `0` 可能被误读为"初始游标"；依赖 readiness/日志发现 | 接受 | 使用共享日志前先看健康检查；不要把 `0` 当权威游标用于计费/权限判断 |
| C3-03 | 未配置 `dead_letter_stream` 时，超过 `max_deliveries` 的事件按 `error` 日志丢弃，**没有独立的"丢弃"指标**（`events_dead_lettered_total` 只在写死信流时计数）。 | [`events.rs`](../../crates/cache/redis/src/events.rs)（`deliver`）、[event-pub-sub.md](../rust-webx/08-mediator-events/event-pub-sub.md) 指标表 | 无 DLQ 部署只能靠日志排查毒消息 | 接受 | 生产配置 `dead_letter_stream` + 告警；死信 replay 通过 DI resolve `RedisEventConsumer`（见 [event-pub-sub.md](../rust-webx/08-mediator-events/event-pub-sub.md) Redis Streams 段） |
| C3-04 | presence `members()` 一次性返回全量成员、**无分页**；TTL 只接受整数秒（`Duration` 的亚秒部分被截断；0 秒 TTL 是 typed 400）。 | [`presence.rs`](../../crates/cache/redis/src/presence.rs)（`members`/`ttl_secs`）、[`presence.rs`](../../crates/core/src/realtime/presence.rs) | 大 scope 的成员列表是 O(n) 响应；无法做毫秒级 TTL | 接受 | 计数用 `count`；大规模成员清点走业务数据模型，不要把 presence 当成员表 |
| C3-05 | presence **计数口径**：`count` 是**成员数**，身份 = `(user_id, connection_id, node_id)`；host 桥接把 `node_id` 当 connection slot，因此每个 `(user, node)` 一个成员——同一用户多设备在同一节点只算 1；`room:` scope 由业务显式 `join`/`leave`，框架不自动跟踪房间成员。 | [`presence.rs`](../../crates/host/src/presence.rs)、[`presence.rs`](../../crates/cache/redis/src/presence.rs)、[im-hardening.md](im-hardening.md) §3.2 | 不能用 `count` 推导唯一用户数或设备数；跨节点房间成员需业务维护 | 接受 | 唯一用户数/设备清单用业务数据或本地注册表；房间成员由业务在 join/leave 时调用 presence |
| C3-06 | 广播投递是 **at-most-once**，不重投、不保证跨节点顺序；节点离线期间的广播事件丢失（Redis Pub/Sub、NATS core、RabbitMQ 临时队列随连接删除）。框架没有跨节点"立即踢线"通道。 | [`fanout.rs`](../../crates/core/src/realtime/fanout.rs)、[broker 能力矩阵](broker-capability-matrix.md) §2、[websocket.md](../rust-webx/05-request-pattern/websocket.md) | 实时投递不能当可靠消息；跨节点吊销只能靠共享会话存储 + 复查周期 | 接受 | 可靠消息用持久化 + outbox + `IRealtimeEventStore` 续传；立即踢线由应用广播控制事件后各节点本地关连接 |
| C3-07 | InMemory 事件传输在没有本地 `#[handler]` 注册时，`publish`/`broadcast` 返回 `di` 错误（**fail-closed，不是 no-op**）；只有事件名存在本地 inventory 注册（含框架 fanout 的 routing 注册）时才入队。 | [`event.rs`](../../crates/core/src/mediator/event.rs)（`InMemoryEventPublisher`） | 忘记写 `#[handler]` 会在 publish 处立刻暴露；框架保留事件有 core routing 注册，不受影响 | 接受 | 补 `#[handler]` 或换成真实 broker；把 `di` 错误当装配缺陷处理 |
| C3-08 | presence 心跳是"尽力续期"：单轮部分成员 touch 失败只记日志并在下一轮重试，不保证一轮内全部成功；节点长期不可达时成员按 TTL 过期（这是期望行为）。 | [`presence.rs`](../../crates/host/src/presence.rs)（`heartbeat`）、[realtime-cluster.md](../rust-webx/11-production/realtime-cluster.md) §6 | 瞬时网络抖动不会误删在线成员，但也不会立即恢复已过期成员 | 接受 | 监控心跳失败与成员数骤降；不要依赖 presence 做计费/写权限判断 |

## C4 部署/运营约束

| 编号 | 边界 | 证据 | 影响 | 状态 | 替代路径与验收 |
|---|---|---|---|---|---|
| C4-01 | **滚动升级混跑**：2.9.1 与 2.9.2 节点同时在线时，`RealtimeFanout`/`RealtimeUserFanout`（广播类）的跨节点扇出不完整（2.9.1 节点没有广播通道订阅，2.9.1 发布方也只走竞争投递），且不报错。必须**全量升级后再启用广播扇出**，或在升级窗口暂停跨节点扇出。 | [`options.rs`](../../crates/cache/redis/src/options.rs)（广播注释）、[`fanout.rs`](../../crates/core/src/realtime/fanout.rs)、[broker 能力矩阵](broker-capability-matrix.md) §2 | 混跑期间客户端在不同节点看到的消息不一致 | 运营约束 | 停跨节点扇出 → 全量升级 → 确认所有节点为 2.9.2 后恢复；客户端重连用续传兜底 |
| C4-02 | **Redis presence 升级窗口**：旧格式（整段 `PresenceMember` JSON，2.9.2 旧实现）与新格式（稳定身份 + HASH 载荷）成员可能同时存在（同一逻辑身份短暂双条目），旧条目在 ≤1 个 TTL（默认 60s）内自然过期；混布期以新节点为准，旧实现节点读到新身份会按 malformed 跳过，其 `join` 会再写一条旧格式。 | [`presence.rs`](../../crates/cache/redis/src/presence.rs) 模块注释、[`presence_identity.rs`](../../crates/cache/redis/tests/presence_identity.rs) | 升级后短时间 `count`/`members` 可能偏大或重复 | 运营约束 | 维护窗口内全量升级；接受 ≤1 TTL 偏差；期间不要按 presence 计数做计费/写权限判断 |
| C4-03 | **`start_id` 默认改 `"0"` 的升级影响**：已有 consumer group 上 `start_id` 被忽略；新建组默认会重放 `max_len` 内的保留历史（at-least-once），必须确认 handler 幂等，否则重复副作用。需要只看新消息时显式 `start_id = "$"`。 | [`options.rs`](../../crates/cache/redis/src/options.rs)（`start_id` 文档）、[`events.rs`](../../crates/cache/redis/src/events.rs)（`ensure_group`） | 切换/扩组后可能重复处理历史事件 | 运营约束 | 新组上线前审计 handler 幂等；用 `max_len` 限制可重放历史体积；灰度时先小流量验证 |

## 验收命令

```bash
# 文档门禁（必须输出 ok，版本 2.9.2）
"C:\Program Files\Git\bin\bash.exe" scripts/check-docs.sh
python scripts/check-publish-order.py

# 本轮改动的代码测试
cargo test -p rust-webx-redis --lib
cargo test -p rust-webx-redis --test validation
cargo test -p rust-webx-redis --test realtime_max_age
cargo test -p rust-webx-redis --test presence_identity   # 需 WEBX_REDIS_URL；未设置时 env-gated 用例跳过
cargo test -p rust-webx-host --lib presence
cargo test -p rust-webx-host --test reserved_event_handler_test

# 全量门禁
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test --workspace --doc
cargo doc --workspace --no-deps

# 真实 broker 集成（docker-compose.brokers.yml 或 WEBX_*_URL 指向的远端）
bash scripts/test-brokers.sh
```

对应测试名（失败时先定位这些）：

- `max_age_window_filters_old_events_and_advances_the_cursor`
  （`crates/cache/redis/tests/realtime_max_age.rs`）；
- `invalid_event_options_fail_host_startup`、`invalid_start_id_fails_host_startup`
  （`crates/cache/redis/tests/validation.rs`）；
- `cross_node_leave_and_touch_use_the_stable_identity`、`concurrent_joins_cannot_overshoot_capacity`、
  `legacy_json_members_are_still_readable`、`two_instances_see_the_same_members_and_refresh_without_duplicates`
  （`crates/cache/redis/tests/presence_identity.rs`）；
- `events.rs` 单测 `xpending_reply_exposes_the_delivery_count`、`malformed_entries_are_reported_for_acking`
  （`crates/cache/redis/src/events.rs`）；
- `a_reserved_fanout_handler_fails_host_build`（`crates/host/tests/reserved_event_handler_test.rs`）。

## 维护规则

1. 任何边界行为改变（修复、放宽、适配器交付）必须在同一次变更里更新本页对应行、
   相关文档的指针与 `CHANGELOG.md`；"已修复"行保留，用来解释现行为什么这样实现。
2. 新边界先定级（C1–C4）再写行；C1 必须有源码/测试证据与可运行的验收命令，C2 必须写明
   "待独立适配器"的替代路径，C3 必须说明为什么接受，C4 必须说明部署流程如何绕开。
3. 本页不堆 API 用法；手册只写"怎么用 + 指向本页"，避免两处描述同一事实。

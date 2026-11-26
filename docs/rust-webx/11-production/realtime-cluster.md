# 多实例实时部署

单实例部署时，连接注册表、presence、续传日志与事件传输都在进程内，零外部依赖即可运行。
多实例部署把这些状态拆成两类：**节点本地**（连接、房间索引、实时指标）与**必须共享**
（续传日志、在线态、会话吊销、跨节点扇出）。本页给出共享这一半的装配配方、广播传输选择、
吊销复查与容量边界；WebSocket 能力本身见 [WebSocket 会话](../05-request-pattern/websocket.md)。

> 设计权威：[IM 实时硬化](../../design/im-hardening.md)。本页只讲部署配方，不重复 API 语义。

## 1. 拓扑

```text
浏览器 / 移动端
      │  wss（LB 只需转发 Upgrade，不需要会话粘滞）
      ▼
 ┌─────────┐   ┌─────────┐   ┌─────────┐
 │ 节点 A  │   │ 节点 B  │   │ 节点 C  │   每节点：IConnectionRegistry + realtime 指标
 └────┬────┘   └────┬────┘   └────┬────┘
      │             │             │
      └────── 共享基础设施 ────────┘
        · IRealtimeEventStore（Redis Streams 续传日志）
        · IOnlinePresence（Redis 身份 ZSET + 成员 HASH 在线态 + 心跳 TTL）
        · ISessionStore / ISessionRepository（jti / Cookie 会话吊销）
        · IEventPublisher 广播通道（Redis Pub/Sub、NATS 广播 subject、Rabbit fanout）
```

连接建立后绑定在单个节点上，不要求负载均衡做粘滞；跨节点一致由共享存储与广播事件完成。
滚动升级时旧实例按 Graceful Shutdown 以 close `1001` 结束会话，客户端重连到任意新实例后
用续传日志补齐断档。

## 2. 必须共享的四类状态

| 状态 | 端口与装配 | 框架默认 | 不共享的后果 |
|---|---|---|---|
| 续传日志 | `IRealtimeEventStore`，`use_realtime_event_store(store)` | `InMemoryRealtimeEventStore`（进程内，每 subscription 1024 条） | 重连落到别的节点时回放为空/`truncated=true`，只能整段重建 |
| 在线态 | `IOnlinePresence`，`use_online_presence(store)` / `use_online_presence_with(store, ttl)` | 未注册（该端口无自动默认；注册表内部的 presence 只是节点局部） | 全局在线数、跨节点房间成员不可见；`user:`/`tenant:` 计数各自为政 |
| 会话吊销 | `ISessionStore`（JWT `jti`）+ `ISessionRepository`（Cookie 会话），`use_session_store` + Cookie scheme | 未注册（`ISessionStore` 不自动装配；Cookie scheme 必须显式给 `ISessionRepository`） | 在一个节点登出后，其他节点上的长连接继续存活 |
| 跨节点扇出 | `IEventPublisher::broadcast`（广播投递类），`use_event_publisher` 或 broker `add_*` | 内存队列（单进程） | 广播类事件报错（fail-closed）或只到一个节点 |

未显式装配时行为保持 2.9 的单实例语义——这是**刻意**的：多实例所需的共享实现绝不被静默替换
成内存实现（那会让单机测试通过、上线失去一致性）。

## 3. 装配

```rust
use std::sync::Arc;
use std::time::Duration;
use webx::*;
use webx_redis::{
    RedisConnection, RedisHostBuilderExt, RedisOnlinePresence, RedisOptions,
    RedisRealtimeEventStore, RedisRealtimeEventStoreOptions,
};

// 1) 共享续传日志与在线态（rust-webx-redis，复用同一连接）
let redis = Arc::new(RedisConnection::new(
    "redis://cache:6379/0",
    Duration::from_secs(5), // connect timeout
    Duration::from_secs(5), // command timeout
));
let event_store = Arc::new(RedisRealtimeEventStore::with_options(
    Arc::clone(&redis),
    RedisRealtimeEventStoreOptions::new()
        .with_prefix("app:rt") // 默认 webx:rt
        .with_max_events(4096), // 默认 1024
));
let presence = Arc::new(RedisOnlinePresence::new(Arc::clone(&redis)));
// 2) 共享会话吊销：JWT 用 ISessionStore，Cookie 用 ISessionRepository（实现由应用提供）
let session_store: Arc<dyn ISessionStore> = /* 共享吊销表 */;

Host::builder()
    // 缓存/事件传输复用同一连接（add_redis 内部会自建连接，这里用 add_redis_with 共享）
    .add_redis_with(
        Arc::clone(&redis),
        RedisOptions::new("redis://cache:6379/0").with_prefix("app:"),
    )
    // 续传：跨节点回放与全局单调游标
    .use_realtime_event_store(event_store)
    // 在线态：use_online_presence 负责 first/last 桥接与 TTL/3 心跳（内部实现不暴露）
    .use_online_presence(presence)
    // 吊销：复查时 JWT 查 jti、Cookie 查会话仓库
    .use_session_store(session_store)
    .use_websocket_limits(
        WebSocketLimits::default()
            .with_session_revalidation(Duration::from_secs(15))
            .with_max_connection_lifetime(Duration::from_secs(30 * 60))
            .close_at_token_expiry(),
    )
    .use_websocket_origins(vec!["https://app.example.com".to_string()])
    .build();
```

要点：

- 两个 Redis 实现都只是端口实现：直接用宿主已有的 `use_realtime_event_store` /
  `use_online_presence` 装配，没有额外注册动词，也没有第二套状态模型；
- `add_redis` 会把共享的 `Arc<RedisConnection>` 注册为 singleton：续传日志/在线态实现可以直接
  用 `host.provider().get::<RedisConnection>()`（返回 `Arc<RedisConnection>`）复用，避免每个
  端口再建连接；事件消费者的阻塞读使用它自己的独立连接（无 response timeout），不与
  缓存/发布/健康检查共用；
- `use_online_presence` 的内部接线不需要业务参与：`IPresenceObserver` 的原子 first/last 转移被
  映射到 `user:<id>` / `tenant:<id>` scope，心跳按 TTL 的三分之一续期（默认 TTL 60s）；
- 两个 Redis 实现各有默认 key 前缀（续传 `webx:rt`、在线态 `webx:presence`），与缓存的
  `key_prefix` 相互独立；多环境/多服务共用一个 Redis 时用**不同的 prefix** 隔离，例如
  `app:rt` / `app:presence`；
- Cookie 站点用 `add_authentication_scheme(SessionCookieScheme)` + `use_csrf`，会话仓库换成
  共享实现；浏览器接入细节见[浏览器接入](../05-request-pattern/websocket.md#浏览器接入cookie--csrf--origin)；
- 配置只来自 `appsettings` / `APP__` 环境变量，不要把连接串写进代码。

## 4. 广播传输选择

实时扇出使用**广播投递类**（`EventDelivery::Broadcast`）：`IMediator::publish` 自动调用
`IEventPublisher::broadcast`，适配器把事件路由到广播通道。竞争投递（领域事件、任务）语义不变。

| 传输 | 注册入口 | 广播实现 | 语义 / 注意 |
|---|---|---|---|
| 进程内 | `add_mediator()` 默认 | 同一有界队列（单进程即全节点） | 仅单实例 |
| Redis | `add_redis(RedisOptions)` | Pub/Sub 频道（默认 `<stream>:broadcast`） | 广播 at-most-once、无持久化；竞争投递仍是 Streams at-least-once |
| NATS | `add_nats(NatsOptions)` | 独立广播 subject（默认 `<subject>.broadcast`，**无 queue group**） | `queue_group` 只作用于竞争投递；可用 `with_broadcast_subject` 覆盖 |
| RabbitMQ | `add_rabbitmq(RabbitMqOptions)` | fanout exchange（默认 `<exchange>.broadcast`）+ 每节点独占/自动删除队列 | 每节点各自的队列随连接消失；可用 `with_broadcast_exchange` 覆盖 |
| Kafka | `add_kafka(KafkaOptions)` | **不支持**：`broadcast` 保持 fail-closed 显式报错 | 不假装“每节点收到”；需要跨节点扇出请换传输或改用共享状态方案 |

完整能力矩阵（含持久化/重试/顺序边界）见
[broker 能力矩阵](../../design/broker-capability-matrix.md)；投递类与失败语义见
[投递类与跨节点扇出](../05-request-pattern/websocket.md#投递类与跨节点扇出)。

选择建议：

- 已经用 Redis 做缓存/续传 → 复用同一 Redis 的 Pub/Sub（多一跳不影响在线投递语义）；
- 已经有 NATS → 广播 subject 最轻量；
- 需要广播与竞争投递都落在同一持久化 broker、且能接受 fanout 拓扑 → RabbitMQ；
- Kafka 只做竞争投递（任务/领域事件），实时扇出另选传输或落为共享状态轮询。

## 5. 续传：共享日志与游标

`RedisRealtimeEventStore` 把每个 subscription 映射为两个 key：

```text
{prefix}:{subscription}:seq   INCR 分配全局单调游标（u64）
{prefix}:{subscription}:log   Stream，显式 ID 为 {event_id}-0
append  = INCR + XADD + XTRIM MAXLEN ~（有界条数）
resume  = 从 ({cursor} 起 XRANGE；latest_cursor = XREVRANGE COUNT 1
```

- 游标仍是 `u64`：Stream ID 的毫秒段就是全局单调 `event_id`，多实例共享同一游标，
  `IRealtimeEventStore` 契约不变；
- `append`（`INCR` → `XADD` 显式 id → `XTRIM`）与 `resume` 的读取都在 Lua 脚本里执行：
  并发追加不会交错，resume 看到的最老/最新 id 与回放范围来自同一快照，不会被并发的 `XTRIM`
  漏报 gap；sequence key 被误删/回档时脚本从 Stream top 自愈，下一次 append 继续（首次失败
  返回可重试 503）；顺序是**每个 subscription 内单调**，不是跨 subscription 的墙钟顺序；
- 物理保留只按**条数**（`max_events`）：Redis 实现用 `XTRIM MAXLEN ~` 近似裁剪（Streams
  无法原子地按时间裁剪）。`max_age` 通过 `RedisRealtimeEventStore::with_max_age(Duration)`
  提供**逻辑回放窗口**：`resume` 过滤 `now - created_at > max_age` 的事件并置
  `truncated=true`（全被过滤时游标前进），`latest_cursor` 仍跟随物理保留；`with_max_age`
  是 store builder，不经 `RedisRealtimeEventStoreOptions`/`appsettings`。框架**不提供 PG
  实现**：需要持久/按时间保留时由应用实现同一端口（见
  [known-boundaries.md](../../design/known-boundaries.md) C1-01 / C2-01）；
- `truncated=true` 表示游标之后的事件已被保留策略淘汰，客户端必须按快照重建，不能继续增量；
- 写入侧 `append` 与消息广播放在同一处（同一业务事务之外），由共享 store 负责 id 与顺序；
  跨节点投递仍由广播通道完成——续传日志只回答“游标之后还有什么”。

续传的体量上限（`resume` 受 `max_events` 限制、无额外分页）、`latest_cursor` 失败返回 0、
PG 实现缺口等边界见 [known-boundaries.md](../../design/known-boundaries.md) C3-01 / C3-02 / C2-01。

## 6. 在线态与心跳

Redis 实现每个 scope 用**两个 key**（`RedisOnlinePresence`）：

```text
{prefix}:scope:{scope}     ZSET：member = 稳定身份 JSON 数组 [user_id, connection_id, node_id]，score = 过期时刻
{prefix}:members:{scope}   HASH：field = 身份，value = PresenceMember JSON（含 joined_at）
```

| 项 | 语义 |
|---|---|
| scope | `user:<id>` / `tenant:<id>` 由宿主桥接；`room:<id>` 由业务显式 `join`/`leave`（框架不自动跟踪房间成员） |
| member | 身份 = `(user_id, connection_id, node_id)`。宿主桥接把 `node_id` 当 connection slot，即每个 **(user, node)** 一个成员：同一用户多设备在同一节点合并为一条在线记录；设备清点仍用本节点注册表。**`count` 是成员数，不是唯一用户数** |
| TTL | 默认 60s（`use_online_presence_with` 可调），心跳每 TTL/3 续期；节点崩溃后成员在 TTL 内过期，不留永久幽灵在线 |
| 原子性 | join/leave/touch/members/count 各是**单个 Lua 脚本**，共享批量过期清理（256/批，ZSET 与 HASH 同步清理）；`join` 的容量检查与写入在同一脚本（满 → 可重试 503 且不写成员状态）；`touch` 用 `ZADD XX`，过期后心跳**不会复活**成员、也不重写 `joined_at`；身份不含 join 戳，跨节点/重启的 `leave`/`touch` 命中同一成员 |
| 心跳 | 宿主每 TTL/3 对本节点成员续期，**32 路有界并发** touch；单个失败只记日志、下一轮重试 |
| 校验 | 空 scope / 零 TTL / 超过 512B 或含控制字符的身份组件是 typed `400`；两种实现都按 `max_members_per_scope` 有界（内存默认 10 万；Redis 可配），满时返回可重试 503 而不是无限增长 |
| 旧数据 | 兼容读取 2.9.2 旧 JSON member；旧条目随 TTL 在 ≤1 TTL 内收敛（升级窗口见 [known-boundaries.md](../../design/known-boundaries.md) C4-02） |

用法：全局在线数读 `IOnlinePresence::count`（**成员数**口径），成员列表读 `members`（**全量
返回**、按 user/connection 排序、无分页）；**不要**用各节点 `realtime_connections_active`
相加来判断用户是否在线（连接数、成员数与唯一用户数都不是一回事）。
房间在线态由业务在 join/leave 房间时调用 `PresenceScope::room(...)` 的 `join/leave/touch`。

presence 的完整边界（身份、容量、分页、秒级 TTL、心跳并发、旧数据升级窗口）见
[known-boundaries.md](../../design/known-boundaries.md) C1-06…C1-08 / C3-04 / C3-05 / C4-02。

## 7. 吊销复查与滚动升级

吊销链路是跨节点一致性的关键：

1. 登出 / 全端下线 → 删除共享 `ISessionStore` 的 `jti` 或共享 `ISessionRepository` 的会话；
2. 各节点按 `WebSocketLimits::with_session_revalidation(interval)` 复查本地连接凭据；
   已吊销 → close `1008`（`CloseReason::SessionRevoked`）；
3. 复查周期就是吊销生效的**最长窗口**，生产推荐 10–30s；
4. 瞬时存储错误**不会**关闭连接（记日志、下轮重试）；存储长期不可用会推迟吊销生效，
   用 readiness 告警兜住。

同节点需要立即生效时调用 `close_user_connections(registry, user_id, 1008, "session revoked")`；
跨节点没有“立即踢线”的框架通道（框架关不掉别人节点上的 socket），需要秒级一致时由应用广播
一条控制事件，各节点收到后调用同一函数。

滚动升级：

- 旧实例收到停机信号后按主机排空语义以 close `1001` 收尾（见
  [优雅关闭与可观测性](graceful-shutdown.md)）；
- 客户端重连到新实例，带 `ResumeRequest { subscription, cursor }` 从共享日志续传；
- `close_at_token_expiry` 与 `max_connection_lifetime` 决定连接的最长寿命，升级窗口内客户端
  会按 close code 重新认证（策略见
  [客户端重连策略](../05-request-pattern/websocket.md#客户端重连策略)）；
- **混跑约束**：2.9.1 与 2.9.2 节点同时在线时，广播类事件（`RealtimeFanoutEvent` /
  `RealtimeUserFanoutEvent`）的跨节点扇出**不完整且不报错**——必须先全量升级，或在升级窗口
  暂停跨节点扇出（见 [known-boundaries.md](../../design/known-boundaries.md) C4-01）；
  presence 旧数据在 ≤1 TTL 内收敛（C4-02）。

## 8. 指标、readiness 与告警

- `/metrics` 的 `realtime_*` 指标（`realtime_connections_active`、`closes{code}`、
  `messages_*`、`dropped_total`、`rate_limited_total`、dispatch 直方图）都是**本节点**计数；
  全局数字按实例求和，或读共享在线态（见 [指标（IMeter）](metrics.md)）；
- 共享依赖（Redis/NATS/RabbitMQ）的就绪由应用接 `IHealthCheck`：用异步探针
  （`add_health_check_async` / `_with_timeout`，每项独立超时）检查连接与一次轻量命令，
  失败让 readiness 返回 503；`add_redis` 已为同一连接注册 Redis 探针；
  只装配实时状态（`use_realtime_event_store` / `use_online_presence`）而没有 `add_redis` 时，
  需自己补一条 Redis health check；
  **liveness 不要依赖传递依赖**，否则 Redis 抖动会引发全量重启；
- 建议告警：`realtime_connection_closes_total{code="1008"}` 异常升高（吊销/限流/令牌问题）、
  续传 `truncated` 比例、presence 心跳失败与成员数骤降、广播 `publish` 失败率；
- 广播 `broadcast` 返回 `Err` 表示传输未接受（连接失败、队列满、不支持广播），发布方记日志，
  客户端重连后用续传补齐——不要把它当 500 抛给用户，也不要静默重试到阻塞请求。

## 9. 容量与已知边界

容量关注点：

- 每连接：一条入站任务 + 一条 writer task + 有界出站队列（`outbound_queue`）；容量估算按
  “节点连接数 × 队列深度”算内存上界；
- 复查与心跳是周期性开销：复查每连接每周期一次存储读（可按周期批量/缓存优化），
  心跳每节点每 TTL/3 一次成员续期；
- 广播每事件 = 1 次 broker 发布 + 每节点一次派发与本地扇出，不随节点数放大消息体；
- 保留策略决定 Redis Stream 内存：`max_events` 按 subscription 计，热门房间要单独评估。

已知边界（摘要；完整台账、证据与验收见
[known-boundaries.md](../../design/known-boundaries.md)）：

- 广播 **at-most-once**，不重投，跨节点不保证顺序；可靠投递用持久化 + 续传 / outbox（C3-06）；
- Kafka 适配器不支持广播（显式报错），不提供“伪 fanout”（C2-03）；
- 跨节点吊销有复查窗口，不是秒级即时；API Key 凭据本版不做运行期复查（C3-06）；
- room membership 与「谁该收到」属业务数据模型，框架只维护节点本地注册表与显式 presence
  scope；presence `count` 是成员数（每 (user, node) 一个），`members()` 全量、TTL 整数秒
  （C3-04 / C3-05）；
- Redis 共享实现是单节点客户端：Cluster/Sentinel 需独立适配器，presence 的双 key Lua 脚本
  在 Cluster 下还需 hash tag（C2-02）；
- Redis 续传物理保留只按条数，`max_age` 是逻辑回放窗口，框架不提供 PG 实现
  （C1-01 / C2-01）；
- 混跑 2.9.1/2.9.2 时广播类跨节点扇出不完整（C4-01）；presence 旧数据 ≤1 TTL 收敛（C4-02）。

## 10. 相关

- [WebSocket 会话](../05-request-pattern/websocket.md)：连接、消息即请求、投递类、Cookie 接入、吊销与重连。
- [长连接关闭码与排空语义](../../design/close-codes.md)：`SessionRevoked` 等 close code 的权威表。
- [broker 能力矩阵](../../design/broker-capability-matrix.md)：各传输的投递/重试/顺序边界。
- [IM 实时硬化设计](../../design/im-hardening.md)：五项能力的决策与验收。
- [优雅关闭与可观测性](graceful-shutdown.md)、[指标（IMeter）](metrics.md)。

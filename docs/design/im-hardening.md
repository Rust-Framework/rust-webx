# IM 实时硬化（2.9.2）：事件广播、共享实时状态、推送、连接吊销与浏览器接入

> **文档性质：2.9.2 设计历史 + 决策背景**。本页记录 2.9.2 的原始设计与验收口径，保留原文以便
> 回溯；2.9.3 起的边界修复（`start_id` 默认值、`max_age` 逻辑回放窗口、presence 稳定身份 +
> HASH、保留事件 handler 启动校验）以 [known-boundaries.md](known-boundaries.md) 为唯一权威，
> 本页行为描述与之冲突时以台账为准。
>
> 基线：2.9.1（设计时）。落地项当时必须同批交付实现、测试、手册与 CHANGELOG；本页只保留
> 决策、API 形态、适配器语义与验收边界，不复制手册正文。
>
> 来源：2.9.1 的 IM/音视频能力评估列出的 5 个必须先处理的坑（见
> [`../rust-webx/05-request-pattern/websocket.md`](../rust-webx/05-request-pattern/websocket.md)
> 与 2.9.1 CHANGELOG）。本页是这 5 项的处置结论。

## 0. 问题与目标

2.9.1 已经具备单实例 IM 的实时底座（WS 接入、消息即请求、连接注册表、presence 转移、续传契约、
连接级背压/限流、实时指标），但离生产级 IM/会议还有 5 个必须先填的坑：

| # | 坑（2.9.1 现状） | 2.9.2 目标 |
|---|---|---|
| P1 | 跨节点房间广播默认走 broker 竞争消费语义，`RealtimeFanoutEvent` 实际只到一个节点；无用户定向投递；多实例在线数不准 | 事件传输显式区分**竞争投递**与**广播投递**；新增用户定向广播事件；给出各 broker 的广播实现与矩阵 |
| P2 | `IRealtimeEventStore` 只有进程内实现；presence 只有进程内注册表 | 发布 Redis 共享续传日志 + `IOnlinePresence` 端口/内存实现/Redis 实现 + 宿主 first/last 桥接与心跳 |
| P3 | 通知端口只有 email/sms；无移动推送，离线消息无法触达 | 新增 `IPushSender` 端口 + `InMemoryPushSender` + APNs/FCM 适配器 crate |
| P4 | `ISessionStore` 吊销不会关闭 WS 长连接；JWT/cookie 会话可一直用到断开 | 连接凭据复查（宿主内部校验器 + `WebSocketLimits`），吊销后由框架关闭会话（close 1008） |
| P5 | 浏览器接入配方缺失：imbit 只演示 Bearer；无 Cookie/CSRF/Origin 端到端；手册无配方 | imbit 演示 Cookie 会话 + CSRF + Origin 白名单的完整链路与 e2e；手册新增浏览器接入章节 |

**非目标**：媒体面（SFU/RTP/编解码）仍不进框架（宪法 D2/D11，见
[media-contract.md](media-contract.md)）；消息持久化/全局序号/未读数/关系链/审核仍属业务域。

## 1. 设计原则回检（宪法 P1–P12）

- **P1 一个关注点一种写法**：广播不新增第二套 pub/sub 端口，而是给现有唯一事件传输增加
  「投递类」；推送沿用通知端口的「端口 + 无默认实现 + 缺适配器即解析失败」模型。
- **P9 端口而非插件**：每个新端口 = 稳定 trait + 内存实现 + 至少一个真实适配器 + 契约测试
  （`rust-webx-contract-tests` 不发布，适配器按相对 path 复用）。
- **P10 小核心**：core 只放契约与内存实现；Redis/NATS/RabbitMQ 适配在各自 crate；APNs/FCM
  不链接进 core/host/umbrella。
- **P3 / P12 默认安全、控制面 fail-closed**：广播在传输不支持时**显式报错**（不静默降级为
  单节点）；吊销复查对「已判定吊销」关闭会话，存储错误仅记录并下轮重试（长连接不能因一次
  瞬时错误误杀，也不能在明确吊销时继续存活）。
- **P7 一切有界**：广播订阅、presence TTL、推送重试预算都有上界；新增队列沿用有界队列。
- **兼容**：全部为加性变更；`NatsOptions` / `RedisEventsOptions` 新字段有默认值；新默认行为
  不改变既有事件（`Competing` 仍是默认投递类）。破坏性清单为空。

## 2. D1：事件投递类（P1）

### 2.1 语义

```rust
/// 事件到达订阅端的方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EventDelivery {
    /// 每个 broker 消费组/队列恰好一个消费者收到（领域事件、任务）。
    #[default]
    Competing,
    /// 每个节点都收到一份（实时扇出、缓存失效、跨节点控制消息）。
    Broadcast,
}
```

- `IEventRequest` 增加 `fn delivery() -> EventDelivery { EventDelivery::Competing }`（带默认值，
  既有实现无需改动）。`RealtimeFanoutEvent` / 新增 `RealtimeUserFanoutEvent` 覆盖为 `Broadcast`。
- `IEventPublisher` 增加 `async fn broadcast(&self, message: EventMessage) -> Result<()>`，
  默认实现返回 `Error::di`（**fail-closed**：传输未声明广播能力时，发布广播事件是启动/调用期
  错误，而不是悄悄只发一个节点）。
- `IMediator::publish` 按 `T::delivery()` 分派到 `publish` / `broadcast`；事件编码、
  `EventDispatcher`、DI 与 handler 注册完全不变（**一种事件模型，两种投递类**）。
- 线格式不变：广播类由适配器路由到广播通道（Redis Pub/Sub、NATS 广播 subject、RabbitMQ
  fanout exchange），消费者侧按通道区分，不需要在 `EventMessage` 上增加字段。

### 2.2 框架事件

| 事件 | 事件名 | 投递 | 宿主默认 handler |
|---|---|---|---|
| `RealtimeFanoutEvent { room, payload }` | `webx.realtime.fanout` | Broadcast | 写本节点 `room_connections` |
| `RealtimeUserFanoutEvent { user_id, payload }`（新增） | `webx.realtime.user-fanout` | Broadcast | 写本节点 `user_connections` |

两个事件仍各只允许一个框架 handler：保留事件由 host 启动校验
（`validate_reserved_event_handlers`），应用对它们写 `#[handler]` 会让 host 构建**直接失败**
（错误带事件名与 handler 名），避免静默替换 DI 桥接导致跨节点扇出丢失。该校验只覆盖
`#[handler]` 的 inventory 注册；**手工 DI 注册**保留事件的 `IEventHandler` 不在检查范围，
会与框架 DI handler 同时执行（双投递），因此保留事件一律不要手工注册。

### 2.3 适配器矩阵（摘要，权威在 broker 能力矩阵）

各适配器（含 core 的 InMemory 默认）的完整投递类、语义与失败模式以
[broker-capability-matrix.md](broker-capability-matrix.md) §2 为唯一权威，本页不再复制表格：
Redis 走 Pub/Sub 广播频道（`broadcast_channel`）、NATS 走独立广播 subject
（`broadcast_subject`，无 queue group）、RabbitMQ 走 fanout exchange
（`broadcast_exchange`，每节点独占 auto-delete 队列）；Kafka 单分区 follower 没有广播原语，
`broadcast` 保持 trait 默认 fail-closed 并登记为不支持。

### 2.4 失败语义

- `publish`/`broadcast` 返回 `Err` 表示传输**未接受**事件（连接失败、队列满、不支持广播），
  调用方（handler）决定是否失败请求；实时扇出场景通常记日志并依赖客户端重连补拉。
- 广播是 at-most-once：接收方 handler 失败由传输记录，不重投；业务要用「持久化 + 续传」
  兜底（D2）。
- 广播不保证跨节点顺序；单节点内仍按传输顺序写入本节点连接。

## 3. D2：共享实时状态（P2）

### 3.1 `RedisRealtimeEventStore`（`crates/cache/redis`）

> 本节边界的权威台账：[known-boundaries.md](known-boundaries.md) C1-01 / C1-02 / C1-03 / C2-01。

- 每个 subscription 两个 key：`{prefix}:{subscription}:seq`（`INCR` 分配游标）与
  `{prefix}:{subscription}:log`（Stream，显式 ID `{event_id}-0`）；
- `append` = 单个 Lua 脚本原子完成 `INCR` + `XADD` 显式 ID `{event_id}-0` + `XTRIM MAXLEN ~`（多节点并发时拆成多条命令会因 Stream ID 不递增而失败）；`resume` = 从 `({cursor}` 起 `XRANGE`；
  `latest_cursor` = `XREVRANGE COUNT 1`；`next_cursor` 与 `truncated` 语义与内存实现一致
  （首个保留事件 id > cursor+1 即视为被保留策略淘汰）；
- 游标仍为 `u64`：Stream ID 的毫秒段就是全局单调 `event_id`，因此多实例共享同一游标，无需改
  `IRealtimeEventStore` 契约；
- 物理保留只支持**条数**（`max_events`，`XTRIM MAXLEN ~` 近似裁剪）；`max_age` 通过
  `RedisRealtimeEventStore::with_max_age(Duration)` 提供**逻辑回放窗口**：`resume` 过滤
  `now - created_at > max_age` 的事件并置 `truncated=true`，全部被过滤时 `next_cursor`
  前进；`latest_cursor` 仍跟随物理保留。`with_max_age` 是 store builder，**不经**
  `RedisRealtimeEventStoreOptions`/`appsettings`；
- 框架不提供 PG 实现：需要持久/按时间保留时由应用实现同一端口（见
  [known-boundaries.md](known-boundaries.md) C2-01）；
- 事件传输（同一 `add_redis`）的消费组 `start_id` 默认 `"0"`：组首次创建时重放 stream 保留
  历史，避免"事件已发布、消费组尚未创建"的启动窗口丢事件；handlers 是 at-least-once，必须
  幂等；`"$"` 为显式 opt-in。配置在 host 启动期校验（本地、不联网）：只对**无法工作的配置**
  硬失败（URL 空/不可解析、`events.stream`/`group` 为空、`start_id` 非法含首尾空白/溢出 `u64`）；
  2.9.2 会 clamp 的数值（`max_len`/`read_count`/`block_ms`/`claim_idle_ms`/`max_deliveries = 0`）
  与 `dead_letter_stream == stream` 只 warn 并保持可用——见
  [known-boundaries.md](known-boundaries.md) C1-02 / C1-03。

### 3.2 `IOnlinePresence` 端口（`crates/core/src/realtime/presence.rs`）

> 本节边界的权威台账：[known-boundaries.md](known-boundaries.md) C1-06 / C1-07 / C1-08 /
> C3-04 / C3-05。计数口径与宿主桥接一致：**`count` 是成员数，不是唯一用户数**。

```rust
pub struct PresenceScope(String); // user:/tenant:/room: + 自定义前缀
pub struct PresenceMember { user_id, connection_id, node_id, joined_at_unix_secs }

#[async_trait]
pub trait IOnlinePresence: Send + Sync {
    async fn join(&self, scope: &PresenceScope, member: PresenceMember, ttl: Duration) -> Result<u64>;
    async fn leave(&self, scope: &PresenceScope, member: &PresenceMember) -> Result<u64>;
    async fn touch(&self, scope: &PresenceScope, member: &PresenceMember, ttl: Duration) -> Result<()>;
    async fn members(&self, scope: &PresenceScope) -> Result<Vec<PresenceMember>>;
    async fn count(&self, scope: &PresenceScope) -> Result<u64>;
}
```

- 内存实现 `InMemoryOnlinePresence`：`HashMap<scope, HashMap<member, expiry>>`，读写时按 TTL 清理；
- Redis 实现 `RedisOnlinePresence`：每个 scope **两个 key**——ZSET（member = 稳定身份 JSON
  数组 `[user_id, connection_id, node_id]`，score = 过期时刻）与 HASH
  `{prefix}:members:{scope}`（field = 身份，value = `PresenceMember` JSON，保存 `joined_at`）。
  join/leave/touch/members/count 各是单个 Lua 脚本（内含批量过期清理），`join` 的容量检查与
  写入在同一脚本原子完成（满 → 可重试 503，不写成员状态）；身份不含 join 戳，`leave`/`touch`
  按三元组值寻址、不再依赖进程内索引；但 host 桥接的 `node_id` 是**每进程**
  `node-<pid>-<启动毫秒>-<序号>`，进程重启后旧成员不可寻址、按 TTL 过期。TTL 由应用/宿主心跳续期；
- **宿主桥接**：`HostBuilder::use_online_presence(store)` 注册 store、内置
  `内部桥接器`（把 `IPresenceObserver` 的 first/last 映射到 `user:` / `tenant:` scope）
  与 `内部心跳服务` 后台服务（按 TTL/3 续期本节点成员，**32 路有界并发** touch）。桥接器以
  `node_id` 作为 connection slot，即每个 **(user, node)** 一个成员：同一用户在同一节点的多设备
  合并为一条在线记录（设备清单归本地注册表）；`count` 因此是成员数而非唯一用户数；节点崩溃后
  成员在 TTL 内过期；
- 房间 scope 由业务显式调用（`room:{id}`），框架不自动跟踪 room membership（注册表的 room
  索引是节点局部的，跨节点权威归属业务数据模型）。

### 3.3 多实例部署配方（写入 WebSocket 手册）

1. 实时扇出：`RealtimeFanoutEvent`/`RealtimeUserFanoutEvent` + 支持 Broadcast 的传输（Redis
   Pub/Sub / NATS 广播 subject / Rabbit fanout）；
2. 在线态：`use_online_presence(Arc::new(RedisOnlinePresence::...))`；
3. 续传：`use_realtime_event_store(Arc::new(RedisRealtimeEventStore::...))`；
4. 吊销：共享 `ISessionStore`/`ISessionRepository` + `WebSocketLimits::with_session_revalidation`；
5. 升级顺序：广播扇出要求 ≥2.9.2 全量升级——**≤2.9.1 与 ≥2.9.2 混跑**时扇出不完整，
   **2.9.2 → 2.9.3 可滚动**；presence 的 2.9.2 旧数据到 2.9.3 新格式在 ≤1 TTL 内收敛。
   两者见 [known-boundaries.md](known-boundaries.md) C4-01 / C4-02。

## 4. D3：移动推送（P3）

### 4.1 端口（`crates/core/src/push.rs`）

```rust
pub enum PushPlatform { Apns, Fcm, WebPush }
pub enum PushPriority { Normal, High }
pub struct PushNotification { title, body, badge: Option<u32>, sound: Option<String> }
pub struct PushMessage {
    platform: PushPlatform,
    token: String,
    notification: Option<PushNotification>,
    data: serde_json::Value,
    collapse_key: Option<String>,
    ttl: Option<Duration>,
    priority: PushPriority,
}
/// 适配器把供应商裁决映射为三态，而不是把 410/404 当 500。
pub enum PushOutcome { Accepted, TokenInvalid, Retryable { retry_after: Option<Duration> } }

#[async_trait] pub trait IPushSender: Send + Sync {
    async fn send(&self, message: PushMessage) -> Result<PushOutcome>;
}
```

- `HostBuilder::use_push(Arc<dyn IPushSender>)`：与 email/sms 相同——**默认不注册**，缺适配器时
  DI 解析失败；
- `InMemoryPushSender`（host crate）捕获消息供测试；
- 适配器 crate（不发布进 core/host）：
  - `rust-webx-apns`（`crates/push/apns`）：HTTP/2 + token 鉴权（ES256，team/key id + p8），
    `apns-topic`/`apns-push-type`/`apns-priority`/`apns-expiration`/`apns-collapse-id`；
    200→Accepted，400 `BadDeviceToken`/410 `Unregistered`→TokenInvalid，429/500/503→Retryable；
  - `rust-webx-fcm`（`crates/push/fcm`）：FCM HTTP v1 + service-account OAuth2（RS256→access
    token 缓存），`messages:send`；200→Accepted，404/400 `UNREGISTERED`→TokenInvalid，
    429/503→Retryable；
- 传输复用 `rust-webx-client::IHttpClient`（可注入 mock，测试不联网）；token 缓存、并发刷新、
  关闭安全（不把 service account 私钥写日志）都有单测。
- WebPush 先登记为「端口支持、适配器待办」，不在本版假装支持（避免半成品加密实现）。

## 5. D4：会话吊销与长连接联动（P4）

### 5.1 连接凭据复查

宿主内部按认证方式构造凭据校验器（JWT `jti` → `ISessionStore::is_revoked`；Cookie 会话 id →
`ISessionRepository::get`）；校验器是 host 内部类型，**不新增公开端口**。公开面只有
`WebSocketLimits::session_revalidation` 一个间隔开关：

- `WebSocketLimits::with_session_revalidation(Duration)`（默认 `None`，保持既有会话行为；生产
  手册要求显式开启，imbit 演示开启）；
- 宿主在升级时按认证方式构造校验器：
  - JWT：`jti` + `ISessionStore::is_revoked`；
  - Cookie：`SessionCookieScheme` 的 `ISessionRepository::get`（会话不存在/过期即失效）；
  - API key：本版不复查（key 校验端口无法枚举），文档写明；
- 会话运行到点复查：`Ok(false)` → 记录 `CloseReason::SessionRevoked` 并发送 close `1008`；
  `Err` → 记录日志并在下一轮重试（不因瞬时存储错误误杀长连接）；
- `ConnectContext` 不新增公开字段；校验器由 host 的 session 持有，不进入业务 API。

### 5.2 立即踢线与跨节点

- core 增加自由函数 `close_user_connections(registry, user_id, code, reason)`：调用
  `user_connections` 逐连接 `close`，返回关闭数——**不**改 `IConnectionRegistry` trait 的
  dyn 兼容性；
- 跨节点：所有节点共享 `ISessionStore`/`ISessionRepository` 时，`revoke_all_for_subject` 后各
  节点在复查周期内关闭本地连接；同一节点可同时调用 `close_user_connections` 立即生效；
- 复查周期是「最长存活窗口」，文档给推荐值（服务端 10–30s）。

## 6. D5：浏览器接入配方（P5）

框架已有 `SessionCookieScheme` + `ISessionRepository` + `CsrfMiddleware` + `use_websocket_origins`；
缺的是端到端配方与参考实现。2.9.2 交付：

1. **会话交换端点**：`POST /api/session`（Bearer）→ `Set-Cookie: <HttpOnly; Secure; SameSite>`
   （imbit 新增；浏览器随后用 Cookie 升级 WS，`Authorization` 不参与）；
2. **imbit 装配**：`add_authentication_scheme(SessionCookieScheme)` + `use_csrf` +
   `use_websocket_origins`，登录/登出/并发刷新语义与 Bearer 并存；
3. **e2e**：Cookie 升级成功；错误 Origin 403；登出吊销后复查关闭（close 1008）；Bearer 客户端
   不受 CSRF 影响；
4. **手册**：WebSocket 章节新增「浏览器接入：Cookie + CSRF + Origin」与「令牌过期/吊销与重连」
   两节；认证章节补会话交换端点模式；`close-codes.md` 登记 `SessionRevoked`。

## 7. 兼容性与发布

- 全部为加性 API：新枚举/新端口/新事件/新字段（带默认）/新 crate；无删除、无默认行为变更
  （`WebSocketLimits` 新字段默认关闭；`EventDelivery` 默认 Competing；NATS 竞争语义不变）。
- 新 crate 进入发布面：`rust-webx-apns`、`rust-webx-fcm`（`publish.sh` 与
  `publish.yml` 同序更新，`check-publish-order.py` 门禁）。
- `docs/rust-webx/INDEX.json` 增加对应手册页；`CHANGELOG.md` 顶层 `[2.9.2]` 记录 EN/CN；
  `compatibility.md` 追加 2.9.2 加性字段登记（非破坏例外）。
- 版本：**2.9.2（加性补丁，维护者决定）**：不删除 API、不改默认行为、不需要迁移；
  `Cargo.toml` 家族互钉、`examples/*` 与 `INDEX.json` 同步。

## 8. 验收清单

| 项 | 验收 |
|---|---|
| D1 | core 单测（投递类分派、默认 broadcast 报错、user fanout 路由）；各适配器 env-gated 集成测试覆盖广播到达与竞争/广播双消费端拓扑断言（Kafka 显式 fail-closed） |
| D2 | `InMemoryRealtimeEventStore` 与 Redis 实现跑同一契约；`IOnlinePresence` 内存/Redis 同一契约（join/leave/touch/过期/计数）；宿主桥接集成测试（first/last + 心跳 + 节点崩溃过期） |
| D3 | 端口契约测试（参数校验/三态映射）；APNs/FCM mock HTTP 测试（正确 header/body、200/410/429/503 映射、token 缓存与 401 刷新）；host `InMemoryPushSender` 测试 |
| D4 | host 集成测试：JWT 吊销→close 1008；cookie 会话删除→close 1008；瞬时错误不误杀；`close_user_connections` 计数 |
| D5 | imbit e2e：Cookie+Origin 升级、错误 Origin 403、登出后复查关闭、CSRF 拦截；手册/迁移/CHANGELOG 更新 |
| 门禁 | `cargo fmt --check`、`cargo clippy --workspace --all-targets -D warnings`、`cargo test --workspace`、`examples` 矩阵、`scripts/check-docs.sh`、`scripts/check-publish-order.py`、`scripts/check-crate-layout.sh` |

## 9. 第一性原理与对抗审查（2.9.2）

### 9.1 心智负担账本：每个新概念都落在已有模型上

| 新增概念 | 复用的既有能力 | 为什么不是第二套架构 |
|---|---|---|
| `EventDelivery` + `IEventPublisher::broadcast` | 唯一事件传输、`IMediator::publish`、`EventDispatcher`、DI | 广播只是投递类；不新增 `IRealtimeBus`/第二个 pub-sub 端口 |
| `RealtimeUserFanoutEvent` | `RealtimeFanoutEvent` 同族 | 用户定向 = 房间扇出的地址变体；宿主各一个 handler |
| `IPushSender` | `IEmailSender`/`ISmsSender` 的形状（端口 + 无默认实现 + `use_*`） | 不引入通知中心/推送路由层；供应商差异在适配器 crate |
| `IOnlinePresence` | `IRealtimeEventStore` 的「端口 + 内存默认 + 共享适配器」范式；`IPresenceObserver` 现成原子转移 | 不新增 presence 协调服务；桥接/心跳只是既有 observer + BackgroundService 的组合 |
| `WebSocketLimits::with_session_revalidation` | 现有 `close_at_token_expiry`/`max_connection_lifetime` 同一 limits 表 | 校验器是 host 内部实现；业务只多一个开关，不学新端口 |
| 浏览器接入 | `SessionCookieScheme`/CSRF/`use_websocket_origins` 全部已存在 | 本版只补配方与 e2e，不新增认证概念 |

### 9.2 对抗审查中被否决的方案

1. **独立实时总线端口**（`IRealtimeBus`）：与唯一事件传输语义重复，形成第二套跨进程模型 → 用投递类扩展。
2. **公开 `IConnectionCredential`**：本版只有 JWT/Cookie 两种实现、应用无法插拔 → 降为 host 内部 trait，公开面只留 interval 开关。
3. **Redis 专用注册动词**：`add_redis_realtime_event_store` 等与 `add_redis`/`use_*` 形成第三套装配词汇 → 只交付端口实现，装配复用 `use_realtime_event_store` / `use_online_presence`。
4. **每 provider 一个推送端口**：供应商差异放适配器 → 单一 `IPushSender` + 三态裁决。
5. **框架自动跟踪 room membership**：与业务数据模型冲突 → 只提供 `room:` scope，join/leave 归业务。
6. **presence 心跳默认开启**：单机部署不应多一个后台任务和存储写 → 显式 `use_online_presence` 才装配。

### 9.3 失败模式与默认值清单

- 传输不支持广播 → 发布 `Err`（fail-closed），绝不静默只发一个节点；
- 吊销复查遇到存储错误 → 只记日志并重试，不把抖动当吊销；未注册 store/repo 时复查等效关闭（文档写明）；
- presence 心跳失败 → 只记日志，不影响会话；
- 默认部署（无 Redis/NATS）→ 全进程内，单机语义与 2.9.1 相同；
- 空 token/非法 scope/零 TTL → typed `400`，不静默接受。

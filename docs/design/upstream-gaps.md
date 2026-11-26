# 下游能力缺口处置结论

> 来源：下游 RXStudio 的 `docs/design/upstream-framework-gaps.md`（基于 rust-webx 2.6.0 /
> rust-ef 2.6.0 / rust-dix 1.0.0 的落地反馈）。处置基线：rust-webx 2.9.3 / rust-ef 2.9.1 /
> rust-dix 1.1.0；运行期边界台账见 [known-boundaries.md](known-boundaries.md)，本页不重复其细节。
>
> 本页是**处置结论**，不是手册：每项只保留判定、落点与边界；实现细节在代码、rustdoc 与
> `docs/rust-webx/` 中维护。2.x 以加性变更为主（2.9.1 是已登记的破坏性补丁，见
> [`compatibility.md`](compatibility.md) §1），落地项已同批交付实现、测试、手册与 CHANGELOG。

## 判定口径

| 判定 | 含义 | 下游动作 |
|---|---|---|
| 已覆盖 | 公开能力直接满足 | 删除应用侧临时实现 |
| 不做 | 与框架边界或已定架构决策冲突 | 按"边界"列的替代路径实现 |

## 跨环境设计约束

云端部署不是"单进程能跑"就够；每项能力都同时回答四组问题，答案写进对应手册章节或 rustdoc：

| 维度 | 必须回答 | 落地方式 |
|---|---|---|
| 单体（单进程） | 默认实现是否自足（无外部依赖即可启动/测试）？故障如何降级？ | 框架默认实现 + 单测/集成测试 |
| 分布式（多实例） | 哪些状态是进程局部的？跨节点一致性靠什么（事件传输 / 共享 store / 共享 registry）？重复投递、乱序、游标语义如何定义？ | 文档写明边界；端口留给共享实现；幂等/顺序测试 |
| 容器化（云原生） | 重启是否安全（无本地持久状态依赖）？readiness/liveness 是否受影响？优雅停机（排空/取消/关闭码）如何表现？配置是否只来自 `AppSettings`/`APP__`？ | 重启语义写入手册；关闭路径测试；无启动期外部强依赖 |
| 性能 | 每请求/每连接分配上限、锁边界（不跨 await、临界区无 I/O）、内存是否有界、热路径是否 O(1)/无额外系统调用；新增观测是否可聚合且索引有界 | 分配预算（`alloc` 门禁）或代码注释说明；有界性测试 |

## 处置结论

| # | 下游诉求 | 判定 | 落点 | 边界 |
|---|---|---|---|---|
| 1 | 分布式实时事件总线 / 连接注册表 | **已覆盖（2.7.0）** | 本地 `IConnectionRegistry`（含 user/room/tenant 在线数）+ presence 原子转移/观察者；跨节点复用唯一事件传输 `RealtimeFanoutEvent`；手册给出共享注册表/事件回传两种多实例 presence 配方 | 不新增第二套 pub/sub；跨节点在线数由应用共享 store 或事件聚合（最终一致），计数权威用共享 store |
| 2 | WS 断线续传协议 | **已覆盖（2.7.0）** | `IRealtimeEventStore` + `RealtimeEvent`/`ResumeRequest`/`ResumeFrame`；`InMemoryRealtimeEventStore`（有界保留）；`HostBuilder::use_realtime_event_store` | 下游把 `/api/im/sync` 的存储层换成该端口即可；线协议帧名由应用决定 |
| 3 | WS 安全生命周期（max lifetime / JWT exp） | **已覆盖（2.7.0）** | `WebSocketLimits::with_max_connection_lifetime` + `close_at_token_expiry()`；close `1001`/`1008` | 下游删硬编码 55 分钟 |
| 4 | 多端 presence 原子语义 | **已覆盖（2.7.0）** | 在线数（`user/_room/_tenant_connection_count`）+ `IPresenceObserver`/`PresenceTransition`（`is_first`/`is_last`，注册表锁内计算）；`HostBuilder::use_presence_observer` | 下游删"前后对比 count"的写法，改用转移回调 |
| 5 | MFA / TOTP / 恢复码 | **已覆盖（2.7.0）** | `crates/auth/mfa`：TOTP（含重放保护）、恢复码（只存哈希）、`amr`/`acr`；登录两步签发配方见 `09-auth-security/mfa.md`，`tests/login_flow_test.rs` 覆盖密码步不满足 step-up、TOTP 重放 401、恢复码单次使用 | 账号 MFA 状态与第 2 因子编排属应用账号模型；secret 落库保护见 #8 |
| 6 | 邮箱验证 / 密码重置 | **已覆盖（2.7.0）** | `IEmailSender`/`ISmsSender` + `EmailMessage`/`SmsMessage` 校验；`HostBuilder::use_email`/`use_sms`（2.7.0 引入时名为 `use_email_sender`/`use_sms_sender`，2.9.1 改名）；有界捕获实现（默认不注册，缺适配器即解析失败） | 账号状态与流程 handler 属应用；一次性令牌用 `OneTimeSecret` + 防枚举配方 |
| 7 | 登录失败 / 锁定策略 | **已覆盖** | `crates/host/src/login.rs`：`ILoginThrottle`/`MemoryLoginThrottle`（账号+客户端、指数退避、窗口、有界） | 下游删 `auth_login_attempt` |
| 8 | 敏感配置 / 密钥保护 | **已覆盖（2.7.0）** | `ISecretStore`（env/file/chain）+ `ISecretProtector`/`AesGcmSecretProtector`（AES-256-GCM、`webx1:<key-id>:…` 自描述密文、key ring 轮换、篡改失败关闭）；`HostBuilder::use_secret_protector` | KMS/Vault 实现同一 trait；TOTP secret 等按 key id 轮换 |
| 9 | 资源 / 实例级授权 | **已覆盖（2.7.0）** | 路由级 `#[authorize(dynamic, …)]` + `IDynamicAuthorizer`；实例级 `RequestUser::require_owner` / `require_same_tenant`（401/403 分离、403 不泄露 id） | 不做资源授权器：资源形状只有应用知道，实例归属仍在 Handler |
| 10 | 权限目录 / 策略管理 | **不做** | `IEndpointCatalog`（`Host::endpoints()`）导出端点与规则 | 角色种子/授予子集是应用领域模型；框架不建第二套权限库 |
| 11 | 版本化迁移基线与历史 | **已覆盖** | rust-ef `ctx.migrate(dir)` + `MigrationStore` + `__ef_migrations_history`；`bootstrap`；`MigrationEngine::diff/apply_diff`；`has_drift` | 下游删 `sys_schema_migration`；readiness 用 `has_drift()` 接 `IHealthCheck` |
| 12 | 分布式锁 / 配额原子性 | **已覆盖（2.7.0）** | `IAdvisoryLock`/`AdvisoryLease`（`try_acquire`/`release`/`renew`，冲突 `None`、过期可回收）；`InMemoryAdvisoryLock`；Redis `RedisAdvisoryLock` + `add_redis_advisory_lock`；`HostBuilder::use_advisory_lock` | 配额优先原子 SQL；多步读改写用租约；PG advisory lock 归 rust-ef 侧实现同一 trait |
| 13 | 应用级 EF→webx 错误映射 | **已覆盖（2.7.0；2.9.1 补齐瞬态与 DI，删除 `webx_ef`）** | `rust-ef` 2.9.1 的 `From<EFError> for AppError`（`rust-webx-ef`/`webx_ef` 已在 2.9.1 删除，不需要任何框架侧适配 crate）：2.9.1 起 `EFError::Database` 的瞬态条件（`SQLSTATE` 08/40/57P0*、SQLite `busy`/`locked` 扩展码、MySQL 1205/1213）直接产出可重试 503，`From<RdiError>` 把运行期解析失败转成 `di(operator)`（500、隐藏 detail） | core/umbrella 不链接 rust-ef；应用不写 `common_mapper.rs` 与 `map_di_error` |
| 14 | 分布式限流策略层 | **已覆盖（2.7.0；2.8 统一转发头）** | `RateLimitPolicy`/`RateLimitRule`（exact > 最长 prefix > Any；Ip/User/Tenant/ApiKey + 匿名回退 IP）+ `use_rate_limit_policy`；`use_rate_limiter` 正式入口；`IRateLimiter` 端口与 429 契约不变；**2.8 起客户端地址统一取自 `ClientInfo` 解析结果（policy 与配置节行为一致；`RateLimit.TrustProxy/TrustedProxies` 旧键已在 2.9.1 删除）** | 多实例把每条规则的 limiter 换成共享实现 |
| 15 | 指标 / SLO 导出 | **已覆盖（2.7.0）** | `IMeter::snapshot()` + `MetricSample`/`MetricKind`；`MemoryMeter` 结构化存储 + 基数上限（`Metrics.MaxSeries`、`with_max_series`、`dropped_series`）；`/metrics` 渲染业务 counter/gauge/histogram（summary） | OTel 实现补 `snapshot()` 或继续自带 exporter；SLO 保留为 PromQL 配方 |
| 16 | 请求级幂等 | **已覆盖（2.8.0 持久化 + 租约围栏）** | `IIdempotencyStore`（`begin`/`complete`/`release` + 默认实现的 `begin_lease`/`complete_lease`/`release_lease` 围栏令牌）+ `InMemoryIdempotencyStore` + `Idempotency-Key` 中间件（主体+方法+路径+键；并发 409；2xx 重放含可重放响应头 `headers`）；**多实例用共享 store：参考适配器 `examples/adapters/idempotency/ef`（非发布，可复制）或自实现端口**；`Idempotency.Enabled` 时装配顺序「显式 `use_idempotency` > DI 注册的 `IIdempotencyStore` > 内存默认」；`Idempotency.Paths`/`SkipPaths` 限定幂等路径 | 请求体一致性校验仍归应用；跨实例无共享库时仍用内存实现（仅单实例正确） |
| 17 | 事务性 outbox | **已覆盖（2.7.0；框架端口与派发器在 `rust-webx-outbox`）** | 框架 crate `rust-webx-outbox`：`IOutbox`/`OutboxEntry`/`OutboxOptions`、`OutboxDispatcher`（`PeriodicBackgroundService` + `EventDispatcher`，指数退避/死信/保留期）、`InMemoryOutbox`、`add_outbox`；**同事务入队与数据库表是数据适配**，参考实现在 `examples/adapters/outbox/ef`（`enqueue_outbox` + `add_outbox_ef`，非发布） | 多实例共享 DB；跨进程投递仍走 `IEventPublisher` |
| 18 | Handler 访问客户端信息 | **已覆盖（2.7.0）** | `#[fromclient] ClientInfo`（`peer_addr`/`client_ip`/`forwarded_for`）+ `ClientInfo` 配置节（`TrustForwardedHeaders`/`TrustedProxies`，最右侧非可信 hop）；host 中间件在鉴权/绑定前解析 | 登录 IP 维度限流直接读该字段 |
| 19 | keyset / cursor 分页 | **已覆盖（2.7.0）** | `KeysetRequest`（cursor + page_size、`from_parts`/`decoded`）+ `Cursor` 编解码（4 KiB 上限、typed 400）+ `CursorPage<T>`（`from_rows` 多取一行精确 `has_more`）；与 offset 并存 | 排序键与租户作用域仍由 Handler 校验 |
| 20 | 声明式校验 / OpenAPI | **已覆盖（2.7.0）** | `#[validate(length/range/email/custom)]` 编译期展开、绑定后执行并汇入同一 `400 errors`；`ParamMeta::constraints` 让 spec 带 `minLength`/`maxLength`/`minItems`/`maxItems`/`minimum`/`maximum`/`format` | 跨字段/查库校验留在 Handler |

## 2026-11 2.9.2：IM 实时硬化（2.9.1 评估的 5 项）

来源：2.9.1 的 IM/音视频能力评估列出的 5 个必须先处理的坑；决策与验收见
[im-hardening.md](im-hardening.md)。判定口径同上：**已覆盖** = 公开能力直接满足；
**部分覆盖** = 端口/配方已交付，但仍有明确划归应用或后续版本的边界。

| # | 2.9.1 缺口 | 判定 | 落点 | 边界 |
|---|---|---|---|---|
| R1 | 跨节点房间广播默认走竞争消费（实际只到一个节点）；无用户定向投递 | **已覆盖（2.9.2）** | `EventDelivery::{Competing, Broadcast}` + `IEventRequest::delivery()` + `IEventPublisher::broadcast`（默认 fail-closed）+ `RealtimeUserFanoutEvent`；Redis Pub/Sub、NATS 广播 subject、RabbitMQ fanout 适配；Kafka 明确登记不支持 | 广播是 at-most-once、不保证跨节点顺序；可靠投递仍需持久化 + 续传 / outbox；不新增第二套 pub/sub 端口（沿用 #1 架构决策） |
| R2 | `IRealtimeEventStore` 只有进程内实现；presence 只有进程内注册表 | **已覆盖（2.9.2）** | `RedisRealtimeEventStore`（Redis Streams，全局单调游标）、`IOnlinePresence` 端口 + `InMemoryOnlinePresence` / `RedisOnlinePresence`、`HostBuilder::use_online_presence`（presence 桥接 + 心跳）、`use_realtime_event_store` | Redis 续传物理保留是**条数**（`XTRIM MAXLEN ~`）；`max_age` 是 `RedisRealtimeEventStore::with_max_age` 的**逻辑回放窗口**（不是启动拒绝），框架不提供 PG 实现；`room:` scope 由业务显式维护；Redis Cluster/Sentinel 需独立适配器。见 [known-boundaries.md](known-boundaries.md) C1-01 / C2-01 / C2-02 |
| R3 | 通知端口只有 email/sms；无移动推送，离线消息无法触达 | **部分覆盖（2.9.2）** | `IPushSender` 端口 + `PushMessage`/`PushOutcome` 三态 + `HostBuilder::use_push` + `InMemoryPushSender`；发布面新增 `rust-webx-apns` / `rust-webx-fcm` | WebPush 仅端口、适配器未交付（**C2**，见 [known-boundaries.md](known-boundaries.md) C2-06）；设备令牌表、重试编排与「必达」的 outbox 属应用；端口不做自动重试 |
| R4 | `ISessionStore` 吊销不会关闭 WS 长连接 | **已覆盖（2.9.2）** | `WebSocketLimits::with_session_revalidation`（宿主内部按 JWT `jti` / Cookie 会话复查）、`CloseReason::SessionRevoked`（close `1008`）、`close_user_connections`（同节点立即） | 跨节点在复查周期内生效（推荐 10–30s），不是秒级；API Key 凭据本版不复查；瞬时存储错误只重试、不误杀连接 |
| R5 | 浏览器接入配方缺失（只有 Bearer 演示，无 Cookie/CSRF/Origin 端到端） | **已覆盖（2.9.2，配方而非新 API）** | `SessionCookieScheme` + `use_csrf` + `use_websocket_origins` + 会话交换端点模式；WebSocket 手册新增浏览器接入与吊销/重连章节；imbit e2e 覆盖 Cookie 升级、错误 Origin 403、登出后 close `1008` | 大部分能力 2.9 已存在，本版补端到端配方与验收；Origin 是精确白名单（不做通配）；API Key 会话的复查边界同 R4 |

## 架构决策

- **#1 不新增 `IRealtimeBus`**：跨进程实时消息已经走唯一的事件传输端口（`RealtimeFanoutEvent`
  复用 `IEventPublisher` + broker 适配器），再建一个 pub/sub 端口会形成第二套跨进程模型。
  下游要的可靠性（`event_id`/`sequence`、断线回放）由 #2 的 `IRealtimeEventStore` 与续传帧承担。
- **#2 续传是日志回放，不是第二套 pub/sub**：`IRealtimeEventStore` 只回答"游标之后还有哪些事件"，
  跨节点投递仍由事件传输完成；共享实现负责多实例回放：Redis Stream 实现已交付
  （`RedisRealtimeEventStore`）；PG 表需应用实现 `IRealtimeEventStore`，框架不提供
  （见 [known-boundaries.md](known-boundaries.md) C2-01）。
- **#9/#10 授权边界**：框架执行端点声明规则（静态 + 动态）；资源实例的归属判断依赖应用数据模型，
  只能留在 Handler。框架补 owner/tenant 校验 helper，但不提供 `IResourceAuthorizer` 与权限目录，
  避免第二套权限模型。
- **#13/#17 依赖分层**：框架只给端口 + 默认实现（如 `rust-webx-outbox` 的 `InMemoryOutbox` 与派发器），
  core/umbrella 不链接 rust-ef；与 ORM/数据库耦合的持久化适配是下游的数据适配，
  参考实现在 `examples/adapters/`（全部 `publish = false`，可复制不可依赖）。
- **#11 已有能力**：下游清单写于 2.6.0 之前；2.6.0 的 `ctx.migrate` + 历史表 + 快照/diff 已覆盖
  "versioned migration runner + schema drift readiness"，无需新增 API。

## rust-ef 侧残留（本仓不可修，已上报）

- **provider 把驱动/传输失败压成 `EFError::Query`，到应用边界就是 400。** rust-ef 2.9.0 只在
  唯一约束上做类型化提升；`map_pg_error` 在 `as_db_error()` 为 `None`（连接被重置、驱动超时）时
  返回 `EFError::query`，`map_sqlx_error` 对非 `Database` 变体（`SqlxError::Io`/`PoolClosed`/
  `PoolTimedOut`）同样返回 `EFError::query`，SQLite 的 `Prepare error`/`Row read error` 也一样
  （`rust-ef-postgres/src/error_map.rs:45`、`rust-ef-mysql/src/error_map.rs:44`、
  `rust-ef-sqlite/src/sync_ops.rs:93`）。`Query` 在 webx 边界按契约映射为 400，因此"查询途中断线"
  会被当成客户端数据错误。**正确修法在 rust-ef**：驱动/传输失败归 `EFError::Connection`，编码/解码
  失败归 `EFError::TypeConversion`，`Query` 只留给真正的语句生成/投影形状错误（那才是 500 类）。
  webx 侧无法区分，应用侧更无法区分，故不在应用重写。

## HTTP/3 依赖与 WebTransport 结论

- **WebSocket over HTTP/3（RFC 9220 extended CONNECT）已解决**：上游 `h3 0.0.8` 的
  `ext::Protocol` 只接受 `webtransport`/`connect-udp`，无法表达 `:protocol: websocket`；
  切换为 `trev-h3 0.3.0` / `trev-h3-quinn 0.3.0` / `trev-h3-webtransport 0.3.0`
  （MIT fork，精确钉版；`h3-quinn` 的 datagram feature 带入传递依赖 `trev-h3-datagram 0.3.0`），
  新增 `Protocol::WEBSOCKET`。`rust-webx-h3` 已实现 extended CONNECT → `IUpgradeSession`
  桥接：成功隧道提交为 HTTP `200`，RFC 合规客户端缺失的 `sec-websocket-key` 由 adapter
  合成，非 101（含 2xx 无 upgrade）→ 错误/problem 响应，普通 `CONNECT` → 400，普通请求
  不能伪造 `Upgrade` 头；e2e 覆盖文本/二进制回显与 extended CONNECT 负例。
- **WebTransport（adapter-level MVP）已交付**：fork 的 `trev-h3-webtransport 0.3.0`
  提供 session/stream/datagram API；`H3Config::with_webtransport_handler` 接受
  `:protocol: webtransport` session 并把具体 `WebTransportServerSession` 交给应用，
  `IWebTransportHandler::authorize(&info)` 是 pre-accept 钩子（**fail-closed：默认拒绝**；
  失败 → 403 且不发送 200，e2e 覆盖默认拒绝与显式拒绝路径），`on_session` 返回后最多等 1s `session.closed()`；e2e 覆盖 bidi
  stream echo，datagram API 已暴露但**尚无 e2e**。**transport-neutral** 的 WebTransport
  端口（engine trait + host 路由/DI/生命周期/指标）仍是后续工作（future work）：
  streams/datagrams 不能无损映射到 `IUpgradeSession` 的字节流。
- **独立实现互操作（已跑通）**：`crates/transport/h3/examples/interop_server.rs`
  （`/health` + `/echo`）；quiche-client smoke 与 h3i 0.7.0 replay 均 200、exit 0；
  h3spec v0.1.15 为 **86 examples / 21 failures / 1 pending** 的**上游 `trev-h3 0.3.0`
  基线**（失败在 H3/QPACK 错误处理与 1 个 quinn/rustls TLS 层，不在 adapter/pipeline），
  用作**可选 preview 传输**的回归基线而非 2.9 稳定面缺陷。建议进 CI 非阻塞/夜间 job 并固定版本/digest。
- **新增风险**：`rust-webx-h3` 依赖 h3 fork，需要跟踪 fork 维护、精确钉版并在 CI
  做回归；RustSec advisory 按 crate 名匹配，fork 包名 `trev-*` 不会命中上游
  `h3`/`h3-quinn` 的既有 advisory，需人工跟踪上游修复。后续若上游 h3 支持 RFC 9220/
  WebTransport 完整 API，可评估切回上游。

## 2026-11 第二轮下游反馈（RXStudio）

RXStudio 基于 2.9.0 上报的 38 条（下游仓 `docs/upstream-feedback.md`）按"先加性修复、
后 2.9.1 架构"分批判定。跨仓计划、逐条状态、对抗审查中的冲突与取舍见跨仓文档
`docs/upstream-feedback-plan.md`（不在本仓维护）；本轮 webx 侧修复已并入未发布的 2.9.1
（见 `CHANGELOG.md`），P1（`rust-core` 契约层）与 P2（分页/排序/API 收敛）的设计见本仓
[`app-error-migration.md`](app-error-migration.md) 与跨仓 `docs/design/pagination-3.0-migration.md`。
本页只保留第一轮（2.6→2.9）的处置结论，不重复第二轮内容。

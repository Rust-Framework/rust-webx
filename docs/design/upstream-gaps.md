# 下游能力缺口处置结论（2.8.0）

> 来源：下游 RXStudio 的 `docs/design/upstream-framework-gaps.md`（基于 rust-webx 2.6.0 /
> rust-ef 2.6.0 / rust-dix 1.0.0 的落地反馈）。处置基线：rust-webx 2.8.0 / rust-ef 2.8.0 /
> rust-dix 1.0.0。
>
> 本页是**处置结论**，不是手册：每项只保留判定、落点与边界；实现细节在代码、rustdoc 与
> `docs/rust-webx/` 中维护。2.x 只做加性变更，落地项已同批交付实现、测试、手册与 CHANGELOG。

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
| 6 | 邮箱验证 / 密码重置 | **已覆盖（2.7.0）** | `IEmailSender`/`ISmsSender` + `EmailMessage`/`SmsMessage` 校验；`HostBuilder::use_email_sender`/`use_sms_sender`；有界捕获实现（默认不注册，缺适配器即解析失败） | 账号状态与流程 handler 属应用；一次性令牌用 `OneTimeSecret` + 防枚举配方 |
| 7 | 登录失败 / 锁定策略 | **已覆盖** | `crates/host/src/login.rs`：`ILoginThrottle`/`MemoryLoginThrottle`（账号+客户端、指数退避、窗口、有界） | 下游删 `auth_login_attempt` |
| 8 | 敏感配置 / 密钥保护 | **已覆盖（2.7.0）** | `ISecretStore`（env/file/chain）+ `ISecretProtector`/`AesGcmSecretProtector`（AES-256-GCM、`webx1:<key-id>:…` 自描述密文、key ring 轮换、篡改失败关闭）；`HostBuilder::use_secret_protector` | KMS/Vault 实现同一 trait；TOTP secret 等按 key id 轮换 |
| 9 | 资源 / 实例级授权 | **已覆盖（2.7.0）** | 路由级 `#[authorize(dynamic, …)]` + `IDynamicAuthorizer`；实例级 `RequestUser::require_owner` / `require_same_tenant`（401/403 分离、403 不泄露 id） | 不做资源授权器：资源形状只有应用知道，实例归属仍在 Handler |
| 10 | 权限目录 / 策略管理 | **不做** | `IEndpointCatalog`（`Host::endpoints()`）导出端点与规则 | 角色种子/授予子集是应用领域模型；框架不建第二套权限库 |
| 11 | 版本化迁移基线与历史 | **已覆盖** | rust-ef `ctx.migrate(dir)` + `MigrationStore` + `__ef_migrations_history`；`bootstrap`；`MigrationEngine::diff/apply_diff`；`has_drift` | 下游删 `sys_schema_migration`；readiness 用 `has_drift()` 接 `IHealthCheck` |
| 12 | 分布式锁 / 配额原子性 | **已覆盖（2.7.0）** | `IAdvisoryLock`/`AdvisoryLease`（`try_acquire`/`release`/`renew`，冲突 `None`、过期可回收）；`InMemoryAdvisoryLock`；Redis `RedisAdvisoryLock` + `add_redis_advisory_lock`；`HostBuilder::use_advisory_lock` | 配额优先原子 SQL；多步读改写用租约；PG advisory lock 归 rust-ef 侧实现同一 trait |
| 13 | 应用级 EF→webx 错误映射 | **已覆盖（2.7.0）** | `rust-webx-ef`（`crates/ef/webx`）：`map_ef_error` + `EfResultExt`；参考应用 re-export 同一映射 | core/umbrella 不链接 rust-ef |
| 14 | 分布式限流策略层 | **已覆盖（2.7.0；2.8 统一转发头）** | `RateLimitPolicy`/`RateLimitRule`（exact > 最长 prefix > Any；Ip/User/Tenant/ApiKey + 匿名回退 IP）+ `use_rate_limit_policy`；`use_rate_limiter` 正式入口；`IRateLimiter` 端口与 429 契约不变；**2.8 起客户端地址统一取自 `ClientInfo` 解析结果（policy 与配置节行为一致，旧 `RateLimit.TrustProxy/TrustedProxies` 被忽略）** | 多实例把每条规则的 limiter 换成共享实现 |
| 15 | 指标 / SLO 导出 | **已覆盖（2.7.0）** | `IMeter::snapshot()` + `MetricSample`/`MetricKind`；`MemoryMeter` 结构化存储 + 基数上限（`Metrics.MaxSeries`、`with_max_series`、`dropped_series`）；`/metrics` 渲染业务 counter/gauge/histogram（summary） | OTel 实现补 `snapshot()` 或继续自带 exporter；SLO 保留为 PromQL 配方 |
| 16 | 请求级幂等 | **已覆盖（2.8.0 持久化 + 租约围栏）** | `IIdempotencyStore`（`begin`/`complete`/`release` + 默认实现的 `begin_lease`/`complete_lease`/`release_lease` 围栏令牌）+ `InMemoryIdempotencyStore` + `Idempotency-Key` 中间件（主体+方法+路径+键；并发 409；2xx 重放含可重放响应头 `headers`）；**多实例用 `rust-webx-ef::idempotency::EfIdempotencyStore`（rust-ef 表 + 唯一键 + CAS 租约 + TTL 清理）**；`Idempotency.Enabled` 时装配顺序「显式 `use_idempotency` > DI 注册的 `IIdempotencyStore` > 内存默认」；`Idempotency.Paths`/`SkipPaths` 限定幂等路径 | 请求体一致性校验仍归应用；跨实例无共享库时仍用内存实现（仅单实例正确） |
| 17 | 事务性 outbox | **已覆盖（2.7.0）** | `rust-webx-outbox-ef`（`crates/outbox/ef`）：`OutboxMessage` + 同事务 `enqueue_outbox`、`IOutbox` 租约领取、`OutboxDispatcher`（`PeriodicBackgroundService` + `EventDispatcher`）指数退避/死信/保留期；`add_outbox` | 多实例共享 DB；跨进程投递仍走 `IEventPublisher` |
| 18 | Handler 访问客户端信息 | **已覆盖（2.7.0）** | `#[fromclient] ClientInfo`（`peer_addr`/`client_ip`/`forwarded_for`）+ `ClientInfo` 配置节（`TrustForwardedHeaders`/`TrustedProxies`，最右侧非可信 hop）；host 中间件在鉴权/绑定前解析 | 登录 IP 维度限流直接读该字段 |
| 19 | keyset / cursor 分页 | **已覆盖（2.7.0）** | `KeysetRequest`（cursor + page_size、`from_parts`/`decoded`）+ `Cursor` 编解码（4 KiB 上限、typed 400）+ `CursorPage<T>`（`from_rows` 多取一行精确 `has_more`）；与 offset 并存 | 排序键与租户作用域仍由 Handler 校验 |
| 20 | 声明式校验 / OpenAPI | **已覆盖（2.7.0）** | `#[validate(length/range/email/custom)]` 编译期展开、绑定后执行并汇入同一 `400 errors`；`ParamMeta::constraints` 让 spec 带 `minLength`/`maxLength`/`minItems`/`maxItems`/`minimum`/`maximum`/`format` | 跨字段/查库校验留在 Handler |

## 架构决策

- **#1 不新增 `IRealtimeBus`**：跨进程实时消息已经走唯一的事件传输端口（`RealtimeFanoutEvent`
  复用 `IEventPublisher` + broker 适配器），再建一个 pub/sub 端口会形成第二套跨进程模型。
  下游要的可靠性（`event_id`/`sequence`、断线回放）由 #2 的 `IRealtimeEventStore` 与续传帧承担。
- **#2 续传是日志回放，不是第二套 pub/sub**：`IRealtimeEventStore` 只回答"游标之后还有哪些事件"，
  跨节点投递仍由事件传输完成；共享实现（Redis Stream/PG 表）负责多实例回放。
- **#9/#10 授权边界**：框架执行端点声明规则（静态 + 动态）；资源实例的归属判断依赖应用数据模型，
  只能留在 Handler。框架补 owner/tenant 校验 helper，但不提供 `IResourceAuthorizer` 与权限目录，
  避免第二套权限模型。
- **#13/#17 新 crate**：`crates/ef/webx`（`ef` 组）与 `crates/outbox/ef`（`outbox` 组）都是
  opt-in starter，core/umbrella 不链接 rust-ef。
- **#11 已有能力**：下游清单写于 2.6.0 之前；2.6.0 的 `ctx.migrate` + 历史表 + 快照/diff 已覆盖
  "versioned migration runner + schema drift readiness"，无需新增 API。

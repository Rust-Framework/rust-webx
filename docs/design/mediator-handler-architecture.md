# 中介者 Handler 体系与事件分发架构（ADR）

> 状态：已采纳（2.3/2.4 架构裁决，事件体系以本地最新迭代为准）
> 范围：`crates/core/src/{handler,mediator,pipeline,route}`、`crates/macros`、`crates/host` 的
> 请求/事件分发；不涉及 HTTP 中间件 onion（见 `middleware-next.md`）与媒体面（见 `media-contract.md`）。

## 1. 结论先行

1. **两种执行路径，一个 Handler 契约**：HTTP 请求有"直连快路径"与"Mediator 路径"两种分发，
   二者构造同一个 `IRequestHandler`、共享 `HandlerCache`/宏生成工厂；**语义差异由构建期规则
   兜底**：只要本 Host 注册了任意 typed pipeline behavior，HTTP 全部改走 Mediator 路径，保证
   behavior 对 HTTP 与进程内调用一致。
2. **事件是异步消息，不是"另一个 mediator"**：`IMediator::publish` 只把 `EventMessage`
   （`event_name` + JSON payload）交给 `IEventPublisher`；默认有界内存队列 + 后台 worker，
   也可换成 Redis/NATS 等 broker。事件 Handler 通过 DI 解析、每事件一个 scope、互相隔离。
3. **跨进程只有一条传输通道**：领域事件与实时跨节点扇出共用 `IEventPublisher`
   （`RealtimeFanoutEvent`）。**不再保留 `IPubSub` 端口**——它与事件传输职责重叠、且在 2.3
   交付后没有任何框架内消费者（Redis/NATS starter 已实现 `IEventPublisher`）。
4. **新增能力先复用既有执行模型**：TestHost 走 Mediator、录制/媒体端口走契约 crate、
   realtime 指标通过 `IMeter` 桥接导出；不在框架内造第二套 dispatch/pub-sub/metrics 机制。

## 2. 分层与组件

```text
HTTP 请求
 ├─ 直连快路径（Host 未注册 typed pipeline behavior）
 │    Router → DirectRouteTarget → HandlerDispatch(宏)
 │      = bind_request + 构造 Handler + handle() + 路由响应编码器
 │    （不经过 Mediator，也不创建请求 scope 的 mediator 链）
 └─ Mediator 路径（注册了 typed pipeline behavior，或非直接路由）
      RouteDispatch(宏) → Mediator::send → dispatch()
        = HandlerCache 查注册 + 每请求 DI scope + typed behavior 链 + call 桥

进程内调用
  IMediator::send / send_boxed → dispatch()（同一 Mediator 路径）

事件
  IMediator::publish → EventMessage(event_name,payload) → IEventPublisher
    →（默认）有界 mpsc → InMemoryEventWorker(BackgroundService)
    → EventDispatcher → EventRegistration 解码 + 新 DI scope
    → inventory 构造 Handler（Default/Inject）逐个广播（隔离/捕获 panic）
  或（broker 适配器）publisher 写 broker；各节点消费者用 EventDispatcher 回到同一 Handler

实时
  单进程：IConnectionRegistry 直接按房间/用户扇出（同步、即时）
  跨进程：publish(RealtimeFanoutEvent) → 事件传输 → 各节点 fan-out handler
          → registry.room_connections(room) → send_text（尽力而为、at-most-once）
```

### 2.1 Handler 契约与注册表

| 组件 | 职责 | 位置 |
|---|---|---|
| `IRequestHandler<T,R>` | 单一请求处理器；`&mut self` 支持 owned-DbContext | `crates/core/src/handler.rs` |
| `IEventHandler<T>` | 事件副作用处理器；`&self`，与请求 Handler 相同的 `#[handler]` / `#[handler(inject)]` 构造 | 同上 |
| `HandlerCache` / `HandlerRegistration` | 按 `TypeId` 查工厂 + call 桥；`BoxedDispatch` 供 `send_boxed` | `crates/core/src/route/scan.rs` |
| `HandlerDispatch`（宏） | HTTP 直连：绑定请求、构造 Handler、调用、返回 `EndpointResult` | `crates/macros/src/handler.rs` |
| `RouteDispatch`/`RouteDispatchById`（宏） | HTTP 经 Mediator：绑定后 `mediator.send` | `crates/macros/src/endpoint.rs` |
| `#[handler]` 两种构造 | 默认 `Default::default()`；`#[handler(inject)]` 每请求 DI scope + 依赖注入 | 宏 + `IRequestHandler` |

约束：一个 `(request, response)` 组合只能注册一个 `HandlerDispatch`/`HandlerRegistration`
（Host 构建期报错）。事件 Handler 与请求 Handler 使用同一套构造规则：
`#[handler]`（`Default`，零 DI 注册）或 `#[handler(inject)]`
（`#[derive(Inject)]`，只注册依赖）。每个事件新建 handler，共享状态放入注入依赖；
不再提供事件专属的 `auto`/`manual` 模式或额外注册 helper。

### 2.2 Typed pipeline 与 HTTP 中间件的边界

| 机制 | 作用域 | 顺序 | 短路语义 |
|---|---|---|---|
| `IPipelineBehavior<Req,Rsp>` | Mediator 请求（含 HTTP 的 Mediator 路径） | 注册顺序，第一个最外层 | 返回 `Rsp` 不调用 `Next` |
| `IMiddleware` / `NextStage` | HTTP 传输层 onion | 插槽 + 注册顺序 | `Break` 或 continuation 不推进 |

两者是**不同层**：HTTP 直连快路径不跑 typed behavior；注册了 typed behavior 时 Host
自动关闭直连快路径，保证 behavior 在 HTTP 上生效（`TypedPipelineRegistered` DI 标记，
按 Host 隔离，避免同进程多 Host 互相降级）。这是本架构唯一"双路径"取舍，已用
`event_bus`/`mediator_test`/`pipeline` 测试锁定。

## 3. 事件分发体系（最新实现）

### 3.1 契约

- `IEventRequest`：`Clone + Send + 'static`；`event_name()` 默认全限定类型名，可覆盖为稳定 wire 名；
- `EventMessage { event_name, payload }`：与传输无关；`to_wire()/from_wire()` 提供单 body 传输
  （NATS/RabbitMQ）的 `u32 长度 + 名称 + JSON` 帧；
- `IEventHandler<T>`：`async fn handle(&self, event: T) -> Result<()>`；
- `IEventPublisher`：`async fn publish(&self, EventMessage) -> Result<()>`，**只表示传输已接收**。

### 3.2 语义（必须保持）

| 维度 | 语义 |
|---|---|
| 阻塞 | `publish` 不等待 Handler；Handler 失败不影响发布者（不返回其错误） |
| 失败 | 本地队列满 → 可重试 503；无本地注册 → fail-closed `Di` 错误（不静默丢弃）；broker 不可用 → 传输错误 |
| 投递 | 默认内存传输 at-most-once；进程崩溃丢队列；可靠投递用 outbox + 持久 broker |
| 顺序 | 单 worker 顺序消费；多 Handler 之间无顺序保证（逐个 await） |
| 隔离 | 每事件新 DI scope；Handler 失败/panic 不阻止其余 Handler，不杀 worker |
| 容量 | 有界队列（`DEFAULT_EVENT_QUEUE_CAPACITY`），`try_send`，永不阻塞发布者 |
| 注册 | `#[handler]` / `#[handler(inject)]` 生成路由 + 构造注册，与请求 Handler 同一模型；每个事件新建 handler |

### 3.3 Broker 适配器契约（Redis/NATS/…）

publisher 侧实现 `IEventPublisher`（写 `event_name`+payload）；consumer 侧循环
`EventMessage::from_wire`（或直接构造）后调用 `EventDispatcher::dispatch`。禁止在适配器里
重新实现 Handler 查找/DI scope/序列化——这些是框架职责，复用 `EventDispatcher`。

## 4. 本次迭代新增内容的复用矩阵（不重复原则）

| 新增内容 | 复用的既有能力 | 是否新增抽象 | 裁决 |
|---|---|---|---|
| `IConnectionRegistry`（2.3-4a） | 无（实时连接索引是新的领域） | 端口 | 保留；房间/用户/presence 唯一来源 |
| 跨节点实时扇出（2.3-4b） | `IEventPublisher` + `EventDispatcher` + `EventMessage` wire | **移除**原 `IPubSub` 端口 | **收敛为 `RealtimeFanoutEvent`**：单一跨进程传输、单一 wire、适配器一次实现 |
| `RealtimeMetrics`（2.3-6） | Prometheus 端点；`IMeter`（新增桥接） | 热路径原子收集器 | 保留为**零开销收集器**；`snapshot.export_to(&dyn IMeter)` 供 OTel/自定义导出，见 §5 |
| 消息即请求（2.3-3） | `IMediator::send` + `#[handler]` + typed pipeline | 无（WS 帧只是新传输适配） | 正确复用，无重复 dispatch |
| 信令/TURN（2.4-1） | 无（媒体控制面新领域） | 类型 + 凭据算法 | 保留（core 仅承载契约） |
| `IObjectStore`/录制（2.4-2） | 无既有对象存储端口（上传 spool 是请求体临时存储，语义不同） | 端口 + 元数据 | 保留；`RecordingMeta` 是录制控制面契约，不是 CRUD 实体 |
| `IMediaPlaneAdapter`（2.4-3） | `IMeter`（QoS 指标） | 端口 | 保留；契约测试 crate 复用 |
| `NextStage` 中间件（2.4-4） | 无（HTTP onion 的 continuation） | 传输层机制 | 保留；与 typed pipeline 明确分层，命名从 `Next` 改名避免碰撞 |
| `TestHost`/`IClock`/chaos（2.4-5） | `Mediator`、`HandlerCache`、DI scope、`BackgroundService` | 测试装配 + 时钟端口 | 保留；TestHost 与 TestServer 合并到单一 `testing` 模块 |
| 契约测试 crate（2.4-7） | 端口契约（对象存储/媒体面） | 测试基座 | 保留；pubsub 套件随端口移除，事件传输由 `event_bus_test` + broker nightly 集成覆盖 |
| Redis/NATS starter（并行 2.5 工作） | `IEventPublisher`（+ `IDistributedCache`） | 适配器 | 正确复用事件传输；**不**再实现第二套 pub/sub 端口 |

## 5. 观测分层（避免第二套 metrics 模型）

```text
热路径（零分配原子收集器）: HttpMetrics / AdmissionStats / RealtimeMetrics
    └─ /metrics 直接渲染 Prometheus 文本（冻结指标名）
应用/外部导出（通用端口）: IMeter（AI executor、媒体 QoS、业务指标）
    └─ 应用注册 exporter（OpenTelemetry/自定义）
桥接: RealtimeMetrics::export_to(&dyn IMeter)（scrape 时只导出增量，避免重复计数）
    ├─ 在 /metrics scrape 时把实时计数镜像进已注册的 IMeter
    └─ 未注册 IMeter 时零成本（不执行）
```

不做的事：不在 WS 每消息热路径调用 `IMeter::counter()`（tag 拼接 + map 查找会破坏分配预算）；
不把 `IMeter` 强塞进 `/metrics` 渲染（会改变已冻结的 Prometheus 名/格式）。

## 6. 扩展点选择矩阵

| 需求 | 选择 |
|---|---|
| 同步请求/响应，需要返回值 | `IMediator::send`（或 HTTP 路由，自动走同一 Handler） |
| 跨模块横切（校验/事务/缓存），按请求类型 | typed `IPipelineBehavior<Req,Rsp>` |
| 传输层横切（CORS/压缩/超时/重试/限流） | `IMiddleware`（continuation 用 `NextStage`） |
| 跨模块异步副作用（发邮件、写审计、缓存失效） | `IMediator::publish` + `IEventHandler<T>` |
| 跨进程、需要 broker/持久化 | 自定义 `IEventPublisher`（Redis/NATS starter）+ `EventDispatcher` consumer |
| 实时房间广播（单进程即时） | `IConnectionRegistry::room_connections` 直接写帧 |
| 实时房间广播（跨节点） | `publish(RealtimeFanoutEvent::to_room(room, payload))` |
| 长任务/轮询 | `BackgroundService`/`PeriodicBackgroundService`（不要手写 worker） |

## 7. 取舍与闭环状态（2026-11-21）

### 事件 Handler 门禁（已解除）

1. **事件 Handler 注册 API**：已收敛为与请求 Handler 相同的 `#[handler]` /
   `#[handler(inject)]`；本 ADR §2.1/§3 已同步，不再有事件专属注册模式。
   **semver 门禁已解除**：`IEventHandler` 保持 `Send + Sync`，未引入 `Any`；dispatch 采用
   inventory-first，只有事件没有任何 inventory handler 时才回退 DI（框架 fan-out/旧测试），
   两种来源不会重复执行。

### 已闭环

2. **`IPubSub` 移除**：2.3 引入的端口与事件体系重复；已删除并替换为 `RealtimeFanoutEvent`
   （进程内扇出仍走 registry，不经事件队列）。历史台账条目标注"已收敛"。
3. **TURN/媒体配置**：已迁入 `AppOptions.turn`（`TurnSection`），Host 启动期校验并注册
   `TurnRestConfig`；半配置/非法配置是启动错误；`use_turn_config` 提供显式覆盖。参考应用不再
   手工构造端口配置（`TURN_*` 仅作开发环境映射）。验收：`turn_config_test` 4 项 +
   `crates/webx/tests/signaling_e2e_test.rs`。
4. **直连快路径守卫**：`DirectRouteTarget::direct()` 只对无授权/无动态授权/无路由 body cap 的
   路由返回 `Some`；引擎直连路径仍校验 declared body limit。验收：host `endpoint` 单测
   （四条守卫条件）+ `direct_dispatch_guard_test`（未认证 401 / 无角色 403 / 有角色 200）。
   引擎 `DirectTarget` 仅用于引擎/裸路由，rustdoc 已标注"无路由守卫"。
5. **事件传输适配器**：Redis/NATS/RabbitMQ/Kafka 已接入 `stability.yml` 的 `broker-integration`
   job（service containers），consumer 复用 `EventDispatcher`、帧复用
   `EventMessage::to_wire/from_wire`；协议专属重连/退避/健康检查各自实现（YAGNI，不抽公共基类）。
   适配器级共享契约已落地：`crates/contract-tests` 的 `EventTransportContract`（PR CI 用 loopback
   自测；四个适配器 `tests/integration.rs` 均接入同一入口，nightly 跑真实 broker）；本地一键
   验证用 `docker-compose.brokers.yml`。验收：默认测试跳过、nightly job 真跑。

## 8. 跨进程扇出的边界（实现约束）

- `webx.realtime.fanout` 是**框架保留事件名**：只允许框架注册的一个
  `IEventHandler<RealtimeFanoutEvent>`（Host 默认装配）。应用再注册同事件 Handler 会导致重复投递；
- 目前只承载**文本帧**；二进制帧需应用自定义事件 + Handler（保持传输层不解释载荷）；
- 投递失败由每连接背压策略计数（at-most-once），不阻塞发布者；
- 跨节点 presence/在线状态不在框架内同步：应用用事件或共享存储实现（框架只提供单实例 registry presence）；
- 没有 WebSocket 路由的 Host 不装配 fan-out handler：若仍发布该事件，传输会接受、worker 会记录
  "no registered IEventHandler" 警告并按 no-op 丢弃（避免 at-least-once 传输无限重投）；这是
  把配置错误暴露出来的 fail-loud 日志行为。

## 9. 质量核验记录（2026-11-21）

| 维度 | 检查 | 结果 |
|---|---|---|
| 分层 | core 不依赖 host/engine；starter 只依赖 core+host（不依赖 umbrella） | 通过（`cargo tree`/Cargo.toml 复核） |
| 重复抽象 | 全仓扫描 "新增 pub/sub、dispatch、metrics、clock、test harness" | 移除 `IPubSub`；metrics 桥接改增量；TestHost/TestServer 合并；契约套件去掉重复端口 |
| 事件语义 | publish 非阻塞、失败隔离、无本地注册 fail-closed、队列满 503、broker consumer 复用 `EventDispatcher` | `event_bus_test` 7 项通过（含 inventory 自动注册路径） |
| 跨进程扇出 | 同房两连接收到、异房不泄漏、事件传输替换语义 | `realtime_fanout_test` 通过 |
| 指标一致性 | Prometheus 权威渲染 + `IMeter` 增量镜像（重复 scrape 不重复计数） | 单测 + `realtime_metrics_test` 通过 |
| Handler 一致性 | HTTP 直连 vs Mediator 双路径、typed behavior 注册后自动切换；事件 Handler 与请求 Handler 统一 `#[handler]` 自动注册 | 既有 `mediator_test`/pipeline 测试 + `event_bus_test` 7 项；`IEventHandler` 未改，semver 门禁解除 |
| 适配器复用 | Redis/NATS/RabbitMQ/Kafka 均实现 `IEventPublisher`、consumer 复用 `EventDispatcher`+`BackgroundService`+`delay`，不复制框架 dispatch | 代码复核通过；`EventTransportContract` 共享套件 + 四适配器接入（PR CI loopback 自测 2 项通过）；`broker-integration` workflow 与 `docker-compose.brokers.yml` YAML 已校验；协议专属 reconnect/backoff/健康检查各自实现（YAGNI） |
| 时钟边界 | `IClock` 是应用/业务时钟端口；安全敏感的 token 过期在认证端口内部完成，不引入第二套时间抽象 | 通过（ADR 说明） |
| 公开 API | 新增类型/方法均有默认或加性签名；`cargo semver-checks` 对 v2.1.0 | 通过（core/engine/host/webx 4/4，各 ~201 checks） |
| 配置一致性 | TURN 由 `AppOptions.turn` 单一来源；Host 启动校验；无环境变量分叉 | `turn_config_test` 4 项通过 |
| 直连守卫 | 快路径只对无守卫路由生效；守卫/body cap/动态授权均回退到 handle | host `endpoint` 单测 + `direct_dispatch_guard_test` 通过 |
| 文档 | ADR + 章节文档 + 台账/CHANGELOG 一致，单一权威落点 | 文档链接校验 0 问题 |

## 10. 质量门禁

- `cargo test -p rust-webx --test {event_bus_test,mediator_handler_test,realtime_fanout_test,turn_config_test,direct_dispatch_guard_test}`；
- `cargo test -p rust-webx-host --lib`（endpoint 直连守卫单测）；
- `cargo test -p rust-webx-{redis,nats,rabbitmq,kafka} --test integration`（nightly broker job）；
- `cargo test -p rust-webx-core --lib`（事件注册/序列化/隔离单测）；
- 新增能力必须回答：复用了哪个执行模型？是否新增第二套 dispatch/transport/metrics？
  答案写进本 ADR 或对应设计文档，评审时对照引用。

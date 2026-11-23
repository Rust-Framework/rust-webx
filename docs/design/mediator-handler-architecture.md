# 处理器与事件架构

> 本文是 HTTP 请求、进程内 Mediator、事件传输与实时扇出的**当前**执行模型。
> 只描述一条主路径与明确的端口边界，不保留历史方案或版本台账。

## 1. 两个入口，一套语义

| 入口 | 用途 | 执行路径 |
|---|---|---|
| HTTP 端点 | `#[get]`/`#[post]`/… 标注的 `IRequest<R>` | 路由 → 绑定 → Handler → 响应编码；无授权/路由 body cap 时走 direct 快路径 |
| `IMediator::send` | 进程内命令/查询 | typed dispatch → 同一 Handler 构造与 typed pipeline |

两者共享：Handler 构造规则（`#[handler]` / `#[handler(inject)]`）、DI 请求 scope、
`IPipelineBehavior`、错误映射。HTTP 能力属于路由，不属于 handler 模式开关。

- 每个请求一个 DI scope；Handler 由 scope 构造，`handle(&mut self)`。
- 直接分发由 Host 级 DI marker 决定，不使用进程全局开关。
- 路由冲突、重复 Handler、非法 `#[authorize]` 在启动期 fail-fast。

## 2. 事件：单一传输通道

```text
业务 Handler ──publish(EventMessage)──▶ IEventPublisher ──▶ broker / in-memory queue
                                                             │
broker consumer ──EventMessage::to_wire/from_wire──▶ EventDispatcher ──▶ #[handler] IEventHandler<T>
```

- `IMediator::publish` 是 fire-and-forget：编码 `EventMessage` 后交给 `IEventPublisher`，
  不等待、不返回订阅者错误；慢订阅者不能拖慢发布请求。
- 默认 `InMemoryEventPublisher` 是有界队列 + 后台 drain；队列满返回可重试 503。
- `EventMessage` 是唯一跨进程帧（version byte + name + payload）；适配器只负责传输，
  不复制 dispatch/序列化。
- `#[handler]` / `#[handler(inject)]` 在 `impl IEventHandler<T>` 上生成事件注册；
  无本地注册的事件是 logged no-op，避免 at-least-once 传输无限重投。

## 3. 实时扇出

- 连接注册表（`IConnectionRegistry`）是进程内连接/用户/房间/presence 的唯一来源。
- 跨节点扇出发布 `RealtimeFanoutEvent`，各节点的框架 handler 写入本节点房间连接；
  不新增第二套 pub/sub 端口。
- 实时指标由热路径原子收集器汇总，`export_to(&dyn IMeter)` 在 scrape 时导出增量。

## 4. 端口复用矩阵

| 能力 | 复用的既有机制 | 结论 |
|---|---|---|
| 连接注册表 | 无既有等价物 | 新端口；房间/用户/presence 唯一来源 |
| 跨节点扇出 | `IEventPublisher` + `EventDispatcher` + `EventMessage` | 收敛为 `RealtimeFanoutEvent`，不引入 `IPubSub` |
| 实时指标 | `IMeter` | 热路径收集器 + scrape 导出 |
| 消息即请求 | `IMediator::send` + `#[handler]` + typed pipeline | 复用，无第二套 dispatch |
| 信令/TURN | 无既有等价物 | 类型 + 凭据算法，core 只承载契约 |
| 对象存储/录制 | 无既有端口 | `IObjectStore` + `RecordingMeta` |
| 媒体面 | `IMeter`（QoS 指标） | `IMediaPlaneAdapter` + 契约测试 |
| 中间件 continuation | HTTP onion | `NextStage`；与 typed pipeline 分层 |
| 测试基座 | Mediator/HandlerCache/DI/BackgroundService | `TestHost` + `IClock` + chaos |
| 契约测试 | 端口 trait | `rust-webx-contract-tests` |
| Broker 适配器 | `IEventPublisher`（+ `IDistributedCache`） | 只做传输，不做第二套端口 |

## 5. 明确不做

- 不做第二套路由/DI/错误/结果/后台/中间件/事件模型。
- 不在 core 出现领域词（IM、媒体、AI、支付）；领域能力进独立 crate。
- 不引入 ambient DI；不在授权器里用路由字符串反推权限字面量。
- 不用 "last wins" 处理注册冲突。

## 6. 验证

- `event_bus_test`：发布语义、Handler 隔离、broker 替换、consumer 路由；
- 四个 broker 适配器的 `tests/integration.rs` 跑共享 `EventTransportContract`；
- `websocket_*` / `realtime_*` / `test_host_test` 覆盖实时与进程内分发；
- `cargo-semver-checks` 对 `v2.7.0` 通过（core/engine/host/webx）。

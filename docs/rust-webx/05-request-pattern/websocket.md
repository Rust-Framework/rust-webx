# WebSocket

框架在**同一容器、两种生命周期**下提供 WebSocket：HTTP 请求仍是短生命周期，WebSocket
会话是连接级长生命周期，二者共享准入、指标与优雅排空。

## 注册端点

```rust
use std::sync::Arc;
use webx::*;

struct EchoHandler;

#[async_trait::async_trait]
impl IWebSocketHandler for EchoHandler {
    async fn on_connect(&self, connection: Arc<dyn IConnection>) -> Result<()> {
        // connection.meta() -> ConnectionMeta { id, transport: WebSocket, remote_addr }
        while let Some(message) = connection.recv().await {
            match message? {
                RealtimeMessage::Text(text) => connection.send_text(text).await?,
                RealtimeMessage::Binary(bytes) => connection.send_binary(bytes).await?,
                RealtimeMessage::Close { .. } => break,
            }
        }
        Ok(())
    }
}

let host = Host::builder()
    .use_websocket("/ws", Arc::new(EchoHandler))
    .build();
```

- 路径匹配是**精确匹配**（不含路径参数）；同一路径的普通 HTTP 请求返回 `426 Upgrade Required`。
- 浏览器无法在升级请求上设置 `Authorization`，认证应走 Cookie 或查询参数（认证中间件在
   接入同一管线；本页只描述当前能力）。

## 协议与生命周期

| 阶段 | 归属 |
|---|---|
| 升级请求（`Connection: Upgrade` + `Upgrade: websocket`） | HTTP 管线 → `WebSocketMiddleware` 短路 |
| `101` 握手（`Sec-WebSocket-Accept` 由 `Sec-WebSocket-Key` 派生） | host（`tokio-tungstenite` 帧层） |
| 原始升级字节流 | 引擎以 `UpgradedTransport` 交给 `IUpgradeSession::run` |
| 帧编解码 / ping-pong / close | host `WebSocketConnection`（`IConnection`） |
| 连接计数、准入、排空 | 引擎（连接 future 在升级 IO 关闭前保持 pending） |

升级成功后连接仍占用一个准入 permit，`/metrics` 的 `engine_connections_active` 包含
WebSocket 会话；优雅停机时引擎按排空预算等待会话结束。

## 消息抽象

`IConnection::recv` 返回 `RealtimeMessage::{Text, Binary, Close}`，ping/pong 由帧层自动
处理、不暴露给业务。`send_*` 在发送失败时返回 `Error::Internal`；背压与连接级限流
（有界发送队列、msg/s、帧上限）属于 `WebSocketLimits`，业务侧应把发送视为可能等待的异步操作。

## 连接作用域

每条 WebSocket 连接由宿主创建一个 **connection DI scope**：`Scoped` 服务在该连接内
共享同一实例，会话结束时随作用域一起释放（逆序 cleanup）。业务代码通过
`IConnection::context()` 解析：

```rust
#[async_trait::async_trait]
impl IWebSocketHandler for PresenceHandler {
    async fn on_connect(&self, connection: Arc<dyn IConnection>) -> Result<()> {
        // 连接级状态：同连接所有消息共享，不同连接互相隔离
        let state = connection
            .context()
            .resolve::<ConnectionState>()
            .expect("registered as scoped");
        let trace = connection.context().trace_id(); // 升级请求的 trace id
        let Some(user) = connection.context().identity() else {
            return Err(Error::Unauthorized("login required".into()));
        };
        let _ = (state, trace, user);
        Ok(())
    }
}
```

注册方式与普通 DI 相同：

```rust
Host::builder()
    .register(|svc| svc.scoped(|_| Arc::new(ConnectionState::default())))
    .use_websocket("/ws", Arc::new(PresenceHandler))
    .build();
```

要点：

- 连接作用域的生命周期 = 会话生命周期；**不要**把它缓存到 Singleton（captive dependency）。
- `ConnectionContext::resolve` 在作用域 dispose 后返回 `None`（不 panic）。
- 身份来自升级请求：当前版本在 WS 中间件阶段捕获，cookie/查询参数认证在
   接入同一管线后生效；`identity()` 未认证时为 `None`。
- 每条消息的**子作用域**（message scope）属于"消息即请求"：消息进入
  `IRequest<T>` 管线时创建并释放；届时 `#[inject]` 可直接用于消息 Handler。

## 消息即请求

`use_websocket_request::<Req, Rsp>("/rpc")` 把每个文本帧当作**一个请求**：JSON 解码为
`Req` → Mediator 按 TypeId 分发到与 HTTP **同一个** `#[handler] IRequestHandler` →
`Rsp` 编码为 JSON 文本帧回发。失败回结构化错误帧：

```json
{"error":{"code":"validation_failed","message":"text is required","retryable":false}}
```

**身份只来自升级请求，绝不来自帧。** 帧是客户端可控 JSON，所以消息请求类型上**不要**声明
`#[fromclaim]` / `#[fromheader]` / `#[fromclient]` 字段：这些绑定器会从「请求」取值，而这条路径的
「请求」就是帧，客户端因此能自选身份、角色或客户端 IP。需要身份时读连接上下文：
`RequestContext::identity()`（handler 内）或 `connection.context().identity()`（`on_connect`）。

```rust
#[derive(serde::Deserialize)]
struct EchoRequest { text: String }

#[derive(serde::Serialize)]
struct EchoReply { text: String, connection_count: u32 }

impl IRequest<EchoReply> for EchoRequest {}

#[derive(Inject)]
struct EchoMessageHandler {
    #[inject] probe: Arc<MessageScopeProbe>,   // 每消息作用域
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<EchoRequest, EchoReply> for EchoMessageHandler {
    async fn handle(&mut self, req: EchoRequest) -> ActionResult<EchoReply> {
        // 连接级上下文：即使 handler 依赖来自每消息作用域，仍可拿到连接
        let context = RequestContext::connection().expect("connection in scope");
        let counter = context.resolve::<ConnectionCount>().expect("connection scope");
        Ok(EchoReply { text: req.text, connection_count: counter.increment() })
    }
}

Host::builder()
    .register(|svc| svc.add_mediator()
        .scoped(|_| Arc::new(MessageScopeProbe::new()))
        .scoped(|_| Arc::new(ConnectionCount::default())))
    .use_websocket_request::<EchoRequest, EchoReply>("/rpc")
    .build();
```

作用域语义（测试锁定）：

| 范围 | 生命周期 | 观察方式 |
|---|---|---|
| 连接作用域 | 整个 WebSocket 会话 | `RequestContext::connection()?.resolve::<T>()` 同一实例跨消息 |
| 每消息作用域 | 单帧请求 | Handler 的 `#[inject]` 依赖每帧重建（mediator 创建/释放） |

需要 `add_mediator()`。当前绑定是 **JSON 解码**（帧没有 HTTP path/query）；
升级请求的身份由下方「升级鉴权」接入。

## 升级鉴权

WebSocket 端点注册在认证中间件**之后**（管道顺序：
`... → auth → user(AfterAuth) → WebSocket → SPA → Router`），因此升级请求与普通 HTTP
走**同一条认证管线**：Cookie / Bearer / API Key 方案都生效，`ctx.claims()` 在
WebSocket 中间件运行时已填充。

- 认证成功：身份在连接生命周期内固定，`connection.context().identity()` 与消息
  Handler 内的 `RequestContext::identity()/operator_id()` 一致；
- 未携带凭据：按匿名升级（`identity()` 为 `None`），是否拒绝由业务 Handler 决定；
- 显式无效凭据：认证中间件在握手前返回 `401`，WebSocket 握手失败；
- 身份来自升级请求，连接中途不重新认证（需要会话续期时由业务层处理）。

### Origin 校验

浏览器在跨站 WebSocket 握手时会自动携带 cookie，因此不匹配的握手必须在升级前拒绝：

```rust
Host::builder().use_websocket_origins(vec!["https://app.example.com".to_string()])
```

- 显式白名单（非空）：`Origin` 缺失或不匹配 → `403`；
- 未显式配置：回落到 `Cors.Origins`（非空且不含 `*` 时），规则同上；
- 两者都没有：不校验，适合非浏览器客户端（它们不带 `Origin`）。

非浏览器客户端在启用了 CORS 白名单的部署里必须显式 `use_websocket_origins(Vec::new())`。

```rust
#[async_trait::async_trait]
impl IWebSocketHandler for PresenceHandler {
    async fn on_connect(&self, connection: Arc<dyn IConnection>) -> Result<()> {
        let user = connection.context().identity();   // 升级请求认证结果
        // ...
        Ok(())
    }
}
```

## 帧绑定错误

`use_websocket_request` 用 `serde_path_to_error` 解码帧，错误分为两类：

| 情况 | `code` | `field` | 示例 |
|---|---|---|---|
| JSON 语法/EOF 错误 | `invalid_message` | 无 | `not-json` |
| 字段类型/缺失等数据错误 | `validation_failed` | 类型错误时给出路径（`text`、`items[2].id`） | `{"text":123}` |
| 业务错误（`Error::Validation` 等） | 同 HTTP `Error::code()` | 无 | Handler 返回 `Err` |

缺失字段（serde 在 struct 根报错）没有路径段，`message` 中会写明
`missing field \`text\``；这两类错误都不会关闭连接。

## 连接注册表与扇出

存在 WebSocket 路由时，host 会默认注册以下实时服务（`try_add_*`，应用可覆盖）：

- `IConnectionRegistry`（`InMemoryConnectionRegistry`）：连接 id、认证用户、房间三个索引；
- `RealtimeMetrics`：实时指标收集器（`/metrics` 直接渲染，并可镜像到 `IMeter`）；
- 框架事件 Handler（`IEventHandler<RealtimeFanoutEvent>`）：消费事件传输上的跨节点扇出消息，
  写入本节点房间连接。事件传输本身是 `IEventPublisher`（默认内存队列，可替换 Redis/NATS）。

会话在握手成功后自动 `register`，会话结束时 `unregister`。Handler 从连接作用域解析注册表：

```rust
#[async_trait::async_trait]
impl IWebSocketHandler for RoomHandler {
    async fn on_connect(&self, connection: Arc<dyn IConnection>) -> Result<()> {
        let registry = connection
            .context()
            .resolve::<dyn IConnectionRegistry>()
            .expect("host registers the registry for WebSocket routes");
        while let Some(message) = connection.recv().await {
            if let RealtimeMessage::Text(text) = message? {
                if let Some(room) = text.strip_prefix("join:") {
                    registry.join(connection.id(), room);
                } else if let Some(room) = text.strip_prefix("say:") {
                    for peer in registry.room_connections(room) {
                        peer.send_text(text.clone()).await?;
                    }
                }
            }
        }
        Ok(())
    }
}
```

| API | 说明 |
|---|---|
| `len()` / `get(id)` / `is_empty()` | 在线连接数与单连接查找 |
| `user_connections(user_id)` | 某认证用户的所有连接（多端登录） |
| `join(id, room)` / `leave(id, room)` / `room_connections(room)` | 房间索引与扇出 |
| `user_connection_count` / `room_connection_count` / `tenant_connection_count` | 按用户/房间/租户的在线数（无需拉连接列表） |
| `presence(user_id)` | `Presence { status, connections }` |
| `set_presence(user_id, Away, ttl)` | 显式 away；TTL 过期且仍有连接时回到 Online；无连接即 Offline |

### 第一端 / 最后一端（原子转移）

“上线广播一次、下线广播一次”不要用 `user_connection_count` 前后对比：两个连接并发进入时，
双方都可能看到“不是第一端”。注册表在**同一个锁内**同时完成索引更新与转移计算，
通过 `IPresenceObserver` 交付：

```rust
struct PresenceBroadcaster;

#[async_trait::async_trait]
impl IPresenceObserver for PresenceBroadcaster {
    async fn on_presence_transition(&self, transition: PresenceTransition) {
        if transition.is_first() {
            // 用户上线（仅一次）：广播/写在线表
        } else if transition.is_last() {
            // 用户下线（仅一次）：广播/清理
        }
        // transition: user_id / tenant_id / connections / direction
    }
}

Host::builder()
    .use_websocket("/ws", Arc::new(handler))
    .use_presence_observer(Arc::new(PresenceBroadcaster))
    .build();
```

观察者在 `on_connect` 之前（Connected）与会话注销之后（Disconnected）调用，只对携带身份的连接触发；
匿名连接不产生转移。多个观察者可用 `use_presence_observer` 多次注册。观察者应只做扇出/发事件，
不要阻塞会话。

替换实现：

```rust
Host::builder()
    .use_websocket("/room", Arc::new(RoomHandler))
    .use_connection_registry(Arc::new(MyRegistry::default()))
    .use_event_publisher(Arc::new(RedisEventPublisher::new(redis)))
    .build();
```

跨节点扇出：发布事件即可（单进程直接 `room_connections` 仍可即时扇出）：

```rust
mediator.publish(RealtimeFanoutEvent::to_room("room-1", "hello")).await?;
```

注意：

- 扇出直接 `await` 每个连接；慢客户端由每连接有界队列/丢弃策略隔离；
- 进程内注册表只覆盖单实例；跨节点扇出复用事件传输（`RealtimeFanoutEvent`，at-most-once，
  Handler 失败由传输层记录）——详见 [架构 ADR](../../design/mediator-handler-architecture.md)；
- 不要在 Singleton 里长期持有 `Arc<dyn IConnection>` 之外的连接状态，会话销毁由 `unregister` 决定。

## 连接级限流与背压

`HostBuilder::use_websocket_limits(WebSocketLimits)` 对**所有** WebSocket 路由生效：

| 限制 | 默认 | 超限行为 |
|---|---|---|
| `max_frame_bytes` | 1 MiB | 回 `{"error":{"code":"frame_too_large"}}`，连接保留 |
| `max_messages_per_second` | 0（不限） | 回 `rate_limited` 错误帧 + close `1008`，连接关闭 |
| `max_bytes_per_second` | 0（不限） | 同上 |
| `outbound_queue` + `on_queue_full` | 64 深，`Drop` | `Drop`：丢弃并计入 `dropped_outbound`；`Disconnect`：发送失败并关闭 |
| `idle_timeout` | `None` | 关闭连接（close `1001`） |
| `max_connection_lifetime` | `None` | 绝对生命周期到点关闭（close `1001`，reason `connection lifetime exceeded`）；入站流量不会续期 |
| `close_at_token_expiry` | `false` | 升级令牌的 `exp` 到点即关闭（close `1008`，reason `access token expired`）；与 `max_connection_lifetime` 取较早者 |

```rust
use std::time::Duration;

Host::builder()
    .use_websocket("/ws", Arc::new(handler))
    .use_websocket_limits(
        WebSocketLimits::default()
            .with_max_frame_bytes(256 * 1024)
            .with_max_messages_per_second(50)
            .with_outbound_queue(32, BackpressurePolicy::Drop)
            .with_idle_timeout(Duration::from_secs(120))
            .with_max_connection_lifetime(Duration::from_secs(3600))
            .close_at_token_expiry(),
    )
    .build();
```

浏览器客户端无法刷新升级时所用的凭据：长连接默认不会因 JWT 过期而断开。需要“令牌过期即重连”的部署
启用 `close_at_token_expiry()`；把 `max_connection_lifetime` 设得略小于令牌有效期，还可以让客户端在
过期前主动重连（服务端到点关闭时客户端按 close code 重连即可）。

实现要点：

- **出站有序队列**：每次连接一条有界 `mpsc` + 独立 writer task；`send_*` 只入队（`try_send`），慢客户端永远不会阻塞调用方（扇出安全）；
- **入站限速**：1 秒滑窗（消息数 + 字节数），违规先错误帧再关闭，`Recv` 返回 `None`；
- **生命周期**：`idle_timeout` 由接收循环计时；`max_connection_lifetime` / 令牌 `exp` 是绝对期限，会话与 Handler 的接收循环竞速，到点主动关闭（Handler 之后从 `recv()` 得到 `None`）；
- **统计**：`IConnection::stats()` 返回 `ConnectionStats`（in/out、字节、`dropped_outbound`、`rate_limited`）， 会把它接到 `/metrics`。

## 断线续传与回放

框架提供统一的续传契约与可替换的事件日志：连接断开期间产生的事件仍可回放。

| 类型 | 作用 |
|---|---|
| `RealtimeEvent { event_id, subscription, payload, created_at_unix_secs }` | 一条被保留的事件；`event_id` 进程内单调递增，就是游标 |
| `ResumeRequest { subscription, cursor }` | 客户端重连时声明“我已处理到 `cursor`”（`null` 表示从头） |
| `ResumeFrame { subscription, events, next_cursor, truncated }` | 服务端回放；`truncated=true` 表示游标之后的事件已被保留策略淘汰，客户端必须按快照重建 |
| `IRealtimeEventStore` | 端口：`append` / `resume` / `latest_cursor`，可换 Redis Stream、PG 表等共享实现 |
| `InMemoryRealtimeEventStore` | 默认实现：每 subscription 保留 1024 条（`RealtimeEventStoreLimits` 可配 `max_age`） |

存在 WebSocket 路由时 host 自动注册默认（进程内）实现在连接作用域里；多实例部署用
`HostBuilder::use_realtime_event_store(Arc<dyn IRealtimeEventStore>)` 换成共享实现。

```rust
struct ChatHandler;

#[async_trait::async_trait]
impl IWebSocketHandler for ChatHandler {
    async fn on_connect(&self, connection: Arc<dyn IConnection>) -> Result<()> {
        let store = connection
            .context()
            .resolve::<dyn IRealtimeEventStore>()
            .expect("host registers the store");
        while let Some(message) = connection.recv().await {
            let RealtimeMessage::Text(text) = message? else { continue };
            let request: ResumeRequest = serde_json::from_str(&text)?;
            let frame = store.resume(request).await?;
            connection.send_text(serde_json::to_string(&frame)?).await?;
        }
        Ok(())
    }
}
```

写入侧形状：`store.append(subscription, payload).await?` 与消息广播在同一处调用；`payload` 由应用定义
（通常是客户端帧的 JSON 字符串），框架只负责 id、保留期与回放顺序。

在线数：`IConnectionRegistry::user_connection_count` / `room_connection_count` /
`tenant_connection_count` 直接返回本进程计数（默认实现按连接列表计算，内存实现是 O(1) 索引），
presence 判断不再由业务自己数连接。

## 部署形态（单进程 / 多实例 / 容器）

| 形态 | 行为 | 需要做什么 |
|---|---|---|
| 单进程 | 注册表、presence 转移、事件日志、指标全在进程内，零外部依赖 | 无 |
| 多实例 | 房间扇出只在节点内直发；跨节点用 `RealtimeFanoutEvent`（事件传输 + broker，at-most-once）；在线数与 presence 是节点局部的 | 用 `use_connection_registry` 换成共享注册表，或用事件同步 presence；续传改用共享 `IRealtimeEventStore`（Redis Stream/PG 表），并由共享 store 分配全局单调游标 |
| 容器 | 不依赖本地持久状态；重启后事件日志为空，重连客户端会得到空回放或 `truncated=true`，必须按快照重建；滚动升级时旧实例以 close `1001`/`1008` 让客户端重连 | 客户端按 close code 重连并按 `truncated` 重同步；容器配置只来自 `appsettings`/`APP__`；优雅停机沿用 host 的排空 |

性能边界（已实现）：

- presence 观察者列表在装配期解析一次，接受连接时零 DI 枚举/分配；
- `register`/`unregister` 单次互斥临界区（纯索引，无 I/O、无 await），在线数 O(1)；
- 事件日志有界（默认每 subscription 1024 条），回放 O(保留条数)，`next_cursor` 支持增量拉取；
- presence 观察者只在真实转移时调用（每连接最多两次：上线/下线）。

### 多实例 presence 配方

框架不新增第二套 pub/sub：跨节点在线数二选一，都由应用在适配器层落地。

1. **共享注册表（推荐，计数权威）**：`IPresenceObserver` 的 first/last 转移在应用层写共享
   store——Redis 用 `SADD presence:room:<id> <conn>` / 断开 `SREM`，查询 `SCARD`；连接键带实例
   id 与心跳 TTL，节点崩溃后自动过期。节点内 `IConnectionRegistry` 仍负责本节点扇出。
2. **事件回传（最终一致）**：把 `PresenceTransition` 经 `IEventPublisher` 发布成
   `RealtimeFanoutEvent`（或业务事件），各节点按 `conn_id + joined/left` 幂等更新聚合计数。
   只适合展示型 presence，不能用于计费或写权限判断。

两者都复用现有的唯一事件传输：`crates/queue/*` 的 broker 适配或 `crates/cache/redis`。
`RealtimeMetrics.realtime_connections_active` 始终是**本节点**计数；全局数字由共享 store 提供，
或在监控端按实例求和。

### HTTP/3 上的 WebSocket

WebSocket 会话建立在 `IUpgradeSession` + `UpgradedTransport` 之上，与传输无关。`rust-webx-h3`
通过 RFC 9220 extended CONNECT（`:protocol: websocket`）复用同一套 handler、registry、presence
与续传语义：

- 客户端发送 HTTP/3 `CONNECT` + `:protocol: websocket`；
- h3 adapter 把请求改写为标准 WebSocket upgrade 形式（GET + `upgrade`/`connection`）交给
  host middleware（鉴权/身份/限制走同一中间件路径，但鉴权/限流没有单独 e2e）；RFC 合规 h3
  客户端**不发送** h1 的 `Sec-WebSocket-Key`/`Accept` 握手头，adapter 在缺失时合成
  `sec-websocket-key`，仅用于满足现有 host middleware 的握手实现（h1 key/accept 交换不用于 h3）；
- 成功后 extended CONNECT 以 HTTP `200` 提交（不是 h1 的 `101`），h3 request stream 与
  `UpgradedTransport` 双向桥接；文本/二进制帧有 e2e 回显；优雅排空会等待 active bridge
  会话（≤5s），但 host 关闭路径（close frame）尚无专门 e2e，暂不宣称 close handshake 与
  h1/h2 完全等价；
- engine 非 101 的 WS extended CONNECT 变成错误/problem 响应，不写正常 2xx body；
  普通 `CONNECT` 返回 `400`；非 WS 的 extended CONNECT 返回 `501` problem+json；
  普通 h3 请求的 `upgrade`/`connection` 头在 adapter 入口被移除，不能伪造升级。

依赖说明：2.9 的 h3 适配层使用 `trev-h3 0.3.0` fork（上游 `h3 0.0.8` 无法表达
`:protocol: websocket`），精确钉版并隔离在可选 crate `rust-webx-h3` 内。WebTransport
以 adapter-level MVP 交付（`H3Config::with_webtransport_handler`；`IWebTransportHandler::authorize`
pre-accept 钩子 fail-closed 默认拒绝，失败 → 403；session + bidi stream echo，datagram API 暴露但无 e2e）；面向
`webx` 的 transport-neutral WebTransport 端口仍是后续工作。

## 实时观测

存在 WebSocket 路由时，host 注册进程级 `RealtimeMetrics`（`IConnection::stats()` 的聚合版），
`/metrics`（`Metrics.Enabled=true`）额外输出：

| 指标 | 类型 | 说明 |
|---|---|---|
| `realtime_connections_active` | gauge | 注册表中的活跃连接（与 registry 联动） |
| `realtime_connections_opened_total` / `_closed_total` | counter | 会话开/关 |
| `realtime_messages_in_total` / `_out_total` | counter | 消息数 |
| `realtime_message_bytes_in_total` / `_out_total` | counter | 消息字节 |
| `realtime_messages_dropped_total` | counter | 背压丢弃 |
| `realtime_rate_limited_total` | counter | 入站限流触发次数 |
| `realtime_connection_closes_total{code}` | counter | 按关闭码（1000/1001/1008/…） |
| `realtime_message_dispatch_seconds` | histogram | 消息即请求的分发延迟 |

定位顺序：连接异常先看 `closes{code}`（1008=限流或令牌过期、1001=空闲或生命周期到期），吞吐问题看
`dropped_total`/`rate_limited_total`，延迟看 dispatch 直方图。

## 当前边界

- 已支持自定义会话 Handler、连接作用域、消息即请求、升级鉴权与帧绑定错误、进程内注册表/房间扇出/presence、跨节点扇出（事件传输上的 `RealtimeFanoutEvent`）、连接级背压与限流、连接级绝对生命周期（含令牌过期）、断线续传契约与事件日志、按 user/room/tenant 的在线数、presence 原子转移（first/last）、`realtime_*` 指标；
- 事件日志与在线数默认都是进程局部的：多实例部署用共享 `IRealtimeEventStore` / 共享注册表实现，或继续用事件传输同步 presence；
- 非文本/二进制控制帧（Ping/Pong）不进入业务层。

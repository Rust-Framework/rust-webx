# IM 参考应用（imbit）

`examples/imbit/` 是实时执行模型的端到端验收应用：**只用框架公开能力**实现一个可运行的
即时通讯服务，证明 HTTP + WebSocket 两种生命周期在同一容器内协调工作。

2.9.2 起它同时验收「IM 实时硬化」的接入侧能力（设计权威见
[im-hardening.md](im-hardening.md)）：**浏览器 Cookie 接入**（D5：会话交换 + CSRF + Origin 白名单）、
**吊销联动**（D4：logout 后长连接 close `1008`）、**事件总线广播**（D1：房间/用户定向扇出走
`RealtimeFanoutEvent` / `RealtimeUserFanoutEvent`）、**共享在线态**（D2：`IOnlinePresence` 端口 +
宿主桥接）。D3 移动推送不在本示例内（由 `rust-webx-apns` / `rust-webx-fcm` 适配器覆盖）。
示例仍是**单实例、内存态**：所有共享端口在本进程里有一个内存实现，生产替换为
共享适配器时 Handler 代码不变（见[与生产形态的差距](#与生产形态的差距明确边界)）。

## 运行

四层结构与 docbit/dmbit 一致（`contracts` / `domain` / `handlers` / `host`），宿主的包名是
`imbit-host`（bin 名仍是 `imbit`）：

```text
cargo run --manifest-path examples/imbit/Cargo.toml -p imbit-host       # 默认 127.0.0.1:8090
IMBIT_ADDR=0.0.0.0:9000 cargo run --manifest-path examples/imbit/Cargo.toml -p imbit-host
```

开发模式使用内置 JWT 密钥（`DEV_JWT_SECRET`）；生产用 `JWT_SECRET` 或 `Jwt.SecretFile`。

参考应用自己的环境变量（框架配置仍以配置节为准，`APP__<Section>__<Key>` 覆盖）：

| 变量 | 作用 | 默认 |
|---|---|---|
| `IMBIT_ADDR` | 监听地址 | `127.0.0.1:8090` |
| `IMBIT_STORAGE_DIR` | 录制分片改用本地文件 `IObjectStore` | 内存对象存储 |
| `IMBIT_ORIGINS` | 允许握手的浏览器 Origin，逗号分隔、精确匹配（scheme+host+port） | 未设置 = 不校验 Origin |
| `IMBIT_REVALIDATION_MS` | WebSocket 长连接复查升级凭据的间隔（框架下限 100 ms） | `500`（演示值；生产建议 10–30 s） |
| `TURN_SECRET` / `TURN_URLS` / `TURN_TTL_SECONDS` | TURN 配置节默认值 | 本地 3478 |

`APP_ENV` 未设置时参考应用按 `Development` 运行（内置 JWT 密钥）；显式 `APP_ENV=Production`
时必须提供真实密钥，否则启动失败（框架安全校验）。

## 能力覆盖

| 业务能力 | 使用的框架能力 |
|---|---|
| 登录 / 注册（首次登录自动建号） | `#[post]` + `ITokenIssuer`（框架端口签发 JWT）+ `IPasswordHasher`（Argon2 默认实现） |
| 鉴权（HTTP 与 WS 共用） | 请求字段 `#[fromclaim] user: IdentityUser`；WS 升级请求走同一认证管线。**Bearer 与 Cookie 并存**：`SessionCookieScheme` 遇到 `Authorization` 主动让位 |
| **浏览器会话交换** | `POST /api/session`（Bearer）→ `SessionCookieScheme::issue_session` + 自定义 `Response`（`ContentResult` 一次写 JSON 与两个 `Set-Cookie`）；`HttpOnly` 会话 cookie 的会话后端是 `ISessionRepository` |
| **CSRF** | `use_csrf(CsrfConfig::double_submit())`：只拦「带会话 cookie 的变更型请求」，`X-CSRF-Token` 与可读 CSRF cookie 常量时间比较；Bearer 客户端不受影响 |
| **Origin 白名单** | `use_websocket_origins`（`IMBIT_ORIGINS`）；列表非空时握手缺失/不匹配 Origin → `403`，未设置则不校验（非浏览器客户端不带 Origin） |
| 建群 / 入群 / 群列表 | `#[post]`/`#[get]` + `#[frompath]`/`#[fromclaim]` 绑定 |
| 消息收发 | `use_websocket_request::<ImRequest, ImReply>`（消息即请求，Mediator 分发同一 Handler 管线） |
| **房间广播（事件总线）** | Handler 注入 `Arc<Mediator>` → `publish(RealtimeFanoutEvent::to_room(room, frame))`；广播类事件由默认内存传输交给框架 fan-out handler，写**本节点** `room_connections`（有界出站队列，慢客户端不阻塞发送方） |
| **1:1 信令定向** | `publish(RealtimeUserFanoutEvent::to_user(user_id, frame))` → 框架 handler 写本节点 `user_connections`（覆盖同一用户多端）；房间广播信令走 `RealtimeFanoutEvent` |
| 幂等重试 | 服务端 `(room, user, client_msg_id)` 去重：重试返回原 seq，`duplicate=true`，不重复发布广播事件 |
| 断线重连 + 离线补拉 | WS `history { after_seq }` + HTTP `GET /api/rooms/{id}/messages?after_seq=`（同一消息日志，广播 at-most-once 的兜底） |
| **在线状态（共享端口）** | `use_online_presence(InMemoryOnlinePresence)`：框架内部把每用户首/末连接的原子转移写进 `user:<id>` scope，并按 TTL/3 心跳续期；`presence` 命令 = 房间成员（领域 `ImStore` 的 membership）× `IOnlinePresence::members(user:<id>)` 在线过滤 |
| **信令** | `use_websocket_request` 同一管线：SDP offer/answer 定向（`user_connections`）、ICE 房间广播、成员/目标权限校验；`delivered` 为**本节点**目标连接数（跨节点异步投递不可计数） |
| **TURN 凭据** | `Turn` 配置节（`AppOptions.turn`，Host 启动期校验）→ `TurnRestConfig::credentials_for`（coturn REST：username=`expiry:user_id`、HMAC-SHA1 派生、TTL 限制）；`GET /api/turn`（Bearer）下发 |
| **录制** | `IObjectStore` 端口（内存 fake / 本地文件 real，含分片断点续传）+ `RecordingMeta` 播放索引；HTTP 分片上传/完成/回放，重试幂等，非成员 403 |
| **吊销 → 断线联动** | `use_session_store(InMemorySessionStore)` + `WebSocketLimits::default().with_session_revalidation(IMBIT_REVALIDATION_MS)`；`POST /api/logout` 吊销升级 token 的 `jti` 后，宿主在复查周期内记录 `CloseReason::SessionRevoked` 并以 close `1008` / reason `session revoked` 关闭长连接（Cookie 会话删除同理，按 `ISessionRepository` 复查） |
| 连接生命周期 | 连接作用域（`RequestContext::connection()`）+ 每消息子作用域（Mediator 创建/释放） |

## 协议

HTTP：

```text
POST /api/login                     {"username","password"} -> {"token","user_id","username"}
POST /api/session                   Authorization: Bearer <token>
                                    -> 200 {"user_id","username"}
                                       Set-Cookie: __Host-webx_session=<opaque>; Path=/; Max-Age=43200; HttpOnly; Secure; SameSite=Lax
                                       Set-Cookie: __Host-webx_csrf=<token>; Path=/; Max-Age=43200; Secure; SameSite=Lax
POST /api/logout                    Authorization: Bearer <token>（吊销该 token 的 jti）
POST /api/rooms                     {"name"} -> RoomView（Bearer 或 会话 cookie + X-CSRF-Token）
POST /api/rooms/{room_id}/join      -> RoomView
GET  /api/rooms                     -> [RoomView]
GET  /api/rooms/{room_id}/messages?after_seq=N -> { room_id, messages }
GET  /api/turn                      -> TURN 凭据
```

浏览器会话要点：

- 会话 cookie **不可被 JS 读取**（`HttpOnly`）；CSRF cookie 可读，前端把它的值复制进
  `X-CSRF-Token`。带会话 cookie 的 `POST/PUT/PATCH/DELETE` 缺少或不匹配 → `403`；
  `Sec-Fetch-Site: cross-site` 直接 `403`。纯 Bearer 请求没有会话 cookie，不受 CSRF 影响。
- WebSocket 升级是 `GET`（安全方法），CSRF 不拦握手；跨站风险由 `IMBIT_ORIGINS` 白名单兜住。
- 本示例的 `POST /api/logout` 吊销 Bearer token 的 `jti`；Cookie 会话版本的生产写法是在登出
  端点调用 `SessionCookieScheme::revoke_session` + 清 cookie，之后已连接 WS 同样在复查周期内收到
  close `1008`（框架 host 测试覆盖 Cookie 路径，本示例 e2e 覆盖 JWT 的 `jti` 路径）。

WebSocket `/ws/im`（JSON 帧，`type` 标记）——请求/响应与事件共存：

```text
--> {"type":"join","room_id":"r1"}                     <-- {"type":"joined","room_id":"r1","members":2}
--> {"type":"send","room_id":"r1","client_msg_id":"m1","text":"hi"}
                                                       <-- {"type":"ack","room_id":"r1","seq":1,"duplicate":false}
                                                       <-- {"type":"message","seq":1,"from":"alice","text":"hi",...}  （房间广播）
--> {"type":"history","room_id":"r1","after_seq":1}    <-- {"type":"history","messages":[...]}
--> {"type":"presence","room_id":"r1"}                 <-- {"type":"presence","online":["alice","bob"]}
--> {"type":"signal","room_id":"r1","to":"bob","kind":"offer","payload":{"sdp":"v=0"}}
                                                       <-- {"type":"signaled","delivered":1}
                                                       <-- {"type":"signal","message":{...}}   （目标连接 / 房间连接）
```

- 升级凭据二选一：`Authorization: Bearer <jwt>`（非浏览器客户端）或会话 cookie（浏览器，
  无 `Authorization`）；两者得到同一个 `IdentityUser`。
- 错误帧：`{"error":{"code","message","retryable"[,"field"]}}`（与 HTTP Error 同一契约）。
- 广播是**异步、at-most-once**：`send` 的 ack 只表示消息已落库并已提交广播事件；接收方等待帧
  即可，断线期间的消息用 `history` 补拉。幂等重试不重复广播。
- `signal` 的 `delivered` 是本节点目标连接数的估算（定向 = 本节点该用户连接数；房间广播 =
  本节点除发送方外的房间连接数）；跨节点投递由事件传输异步完成，不可在此计数。
- 房间广播帧会写入所有本地房间连接，**包括发送方**（与消息广播一致）；客户端按
  `message.from` 过滤自己的帧。
- 信令与 TURN 的完整控制面契约见 [media-contract.md](media-contract.md)。

## 事件与在线态数据流

1. **广播**：WS Handler 校验身份/成员后 `IMediator::publish(RealtimeFanoutEvent |
   RealtimeUserFanoutEvent)`；两个事件都是框架保留的**广播投递类**事件。
2. **传输**：默认 `InMemoryEventPublisher`（有界队列 + 后台 worker）→ `EventDispatcher` →
   框架注册的 fan-out handler → 本节点 `room_connections` / `user_connections` 逐个写帧
   （连接自身的有界出站队列负责背压/丢弃）。
3. **多实例**（代码不变）：把 `IEventPublisher` 换成支持广播的传输（Redis Pub/Sub、NATS 广播
   subject、RabbitMQ fanout；矩阵见 [broker-capability-matrix.md](broker-capability-matrix.md)），
   每个节点都会收到一份并写自己的连接。
4. **在线态**：连接建立/销毁时宿主把首/末连接转移写进 `IOnlinePresence` 的 `user:<id>` scope；
   `presence` 命令先取房间成员 id，再逐个查共享在线集合。多实例换 `RedisOnlinePresence`
   （`use_online_presence`）后同一段业务代码即可跨节点。

## 验收测试

每个 e2e 文件是**独立测试二进制**（各自进程），环境变量（`IMBIT_ORIGINS` 等）不会串扰：

| 测试二进制 | 覆盖 |
|---|---|
| `e2e_im_test.rs` | 登录/建群/入群；WS 收发与房间广播；相同 `client_msg_id` 重试不重复广播；断线重连 `history` 补拉；`presence` 在线名单；匿名 WS 结构化错误帧 |
| `signaling_e2e_test.rs` | WebRTC 信令：1:1 offer/answer 定向、房间广播（`delivered` 计本节点除发送方外的房间连接；帧会回写本节点全部房间连接）、非成员/未知目标错误；TURN REST 凭据 |
| `recording_e2e_test.rs` | 录制控制面 + 分片断点续传 + 原始字节回放（`IRequest<Response>`）、非成员 403 |
| `browser_session_e2e_test.rs` | `POST /api/session` 换两个 cookie（断言 `HttpOnly`/`Secure`/`SameSite` 属性）→ 仅用 cookie 升级 WS → 收发；带 cookie 无/错 `X-CSRF-Token` → `403`、正确 → `200` 且身份与 Bearer 同一 subject；纯 Bearer POST 不受 CSRF 影响；匿名换会话 → `401` |
| `logout_close_e2e_test.rs` | JWT 登录 → WS 连接 → `POST /api/logout` 吊销 `jti` → 复查周期内收到 close `1008`（reason `session revoked`）；另一用户连接不受影响 |
| `websocket_origin_e2e_test.rs` | `IMBIT_ORIGINS` 设置时：缺失 Origin → `403`、错误 Origin → `403`、白名单 Origin → `101` 且会话可用；未设置时不校验 Origin（既有 Bearer/CLI 客户端不受影响） |

```text
cargo test --manifest-path examples/imbit/Cargo.toml
cargo test --manifest-path examples/imbit/Cargo.toml -p imbit-host --test logout_close_e2e_test
```

## 与生产形态的差距（明确边界）

- **状态在进程内存**：`ImStore` / `RecordingRegistry` / `InMemorySessionRepository` /
  `InMemorySessionStore` / `InMemoryOnlinePresence` / `InMemoryEventPublisher` 都是单进程实现，
  重启丢失、不能横向扩展。生产按端口替换：rust-ef `DbContext`（业务数据）、
  `EfSessionRepository`（Cookie 会话，见 `examples/adapters/auth/ef`）、共享 `ISessionStore`
  （JWT 吊销）、`RedisOnlinePresence`（在线态 + 心跳）、`RedisRealtimeEventStore`（续传日志）、
  支持 `Broadcast` 的事件传输。Handler 表面不变。
- **扇出是 at-most-once**：事件传输异步、不保证跨节点顺序、失败由传输记录而不重投；可靠性由
  「先落库 + `history` 补拉」兜底。需要「不丢事件」时用 outbox + 持久 broker。
- **在线态 scope**：宿主桥接只自动维护 `user:` / `tenant:` scope；本示例的「房间在线成员」由
  「房间成员 × user scope」推导，没有单独的 `room:<id>` scope（跨节点房间成员归属业务数据模型，
  框架不自动跟踪）。presence 的 TTL（默认 60 s）内崩溃节点可能残留在线成员，Redis 实现同样以
  TTL 收敛。
- **吊销窗口**：复查周期就是「吊销后仍在线的最大窗口」。示例用 500 ms 便于演示；生产 10–30 s，
  且会话存储必须多实例共享。API Key 凭据本版不做运行期复查（文档明示边界）；同一节点也可用
  `close_user_connections` 立即踢线。
- **浏览器接入的生产前提**：Cookie 是 `Secure`（必须 TLS 终结）、`IMBIT_ORIGINS` 必须显式配置；
  本示例的 `POST /api/session` 只演示交换，生产还应轮换 session id、权限变更后删除会话、
  提供 Cookie 版登出（删除会话 + 清 cookie）。本示例的登出仍吊销 Bearer 的 `jti`。
- **推送**：本示例未接入 `IPushSender`（离线触达由 `history` 补拉替代）；生产接
  `rust-webx-apns` / `rust-webx-fcm`，端口默认不注册、缺适配器即 DI 解析失败。
- **未实现输入长度/频率的全局配额**（当前用 `use_websocket_limits` 的连接级限流兜底）。

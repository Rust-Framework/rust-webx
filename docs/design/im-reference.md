# IM 参考应用（imbit）

`imbit/` 是实时执行模型的端到端验收应用：**只用框架公开能力**实现一个可运行的
即时通讯服务，证明 HTTP + WebSocket 两种生命周期在同一容器内协调工作。

## 运行

```text
cargo run -p imbit            # 默认 127.0.0.1:8090
IMBIT_ADDR=0.0.0.0:9000 cargo run -p imbit
```

开发模式使用内置 JWT 密钥（`DEV_JWT_SECRET`）；生产用 `JWT_SECRET` 或 `Jwt.SecretFile`。

## 能力覆盖

| 业务能力 | 使用的框架能力 |
|---|---|
| 登录 / 注册（首次登录自动建号） | `#[post]` + `ITokenIssuer`（框架端口签发 JWT）+ `IPasswordHasher`（Argon2 默认实现） |
| 鉴权（HTTP 与 WS 共用） | 请求字段 `#[fromclaim] user: IdentityUser`；WS 升级请求走同一认证管线（Bearer JWT） |
| 建群 / 入群 / 群列表 | `#[post]`/`#[get]` + `#[frompath]`/`#[fromclaim]` 绑定 |
| 消息收发 | `use_websocket_request::<ImRequest, ImReply>`（消息即请求，Mediator 分发同一 Handler 管线） |
| 房间广播 | `IConnectionRegistry::room_connections` + `send_text`（有界出站队列，慢客户端不阻塞发送方） |
| 幂等重试 | 服务端 `(room, user, client_msg_id)` 去重：重试返回原 seq，`duplicate=true`，不重复广播 |
| 断线重连 + 离线补拉 | WS `history { after_seq }` + HTTP `GET /api/rooms/{id}/messages?after_seq=`（同一消息日志） |
| 在线状态 | `IConnectionRegistry::room_connections` 按连接身份过滤 → presence 名单 |
| **信令** | `use_websocket_request` 同一管线：SDP offer/answer 定向、ICE 广播（排除发送方）、成员/目标权限校验 |
| **TURN 凭据** | `Turn` 配置节（`AppOptions.turn`，Host 启动期校验）→ `TurnRestConfig::credentials_for`（coturn REST：username=`expiry:user_id`、HMAC-SHA1 派生、TTL 限制）；`GET /api/turn`（Bearer）下发 |
| **录制** | `IObjectStore` 端口（内存 fake / 本地文件 real，含分片断点续传）+ `RecordingMeta` 播放索引；HTTP 分片上传/完成/回放，重试幂等，非成员 403 |
| 连接生命周期 | 连接作用域（`RequestContext::connection()`）+ 每消息子作用域（Mediator 创建/释放） |

## 协议

HTTP：

```text
POST /api/login                     {"username","password"} -> {"token","user_id","username"}
POST /api/rooms                     {"name"} -> RoomView
POST /api/rooms/{room_id}/join      -> RoomView
GET  /api/rooms                     -> [RoomView]
GET  /api/rooms/{room_id}/messages?after_seq=N -> { room_id, messages }
```

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
                                                       <-- {"type":"signal","message":{...}}   （目标连接）
```

错误帧：`{"error":{"code","message","retryable"[,"field"]}}`（与 HTTP Error 同一契约）。
信令与 TURN 的完整控制面契约见 [media-contract.md](media-contract.md)。

## 验收测试（`imbit/tests/e2e_im_test.rs`）

真实 HTTP + 真实 WebSocket 客户端端到端跑通：

1. 两个用户登录（错误密码 → 401），JWT 作为 WS 升级凭据；
2. alice 建群、bob 入群（HTTP）；
3. 两人 WS `join` 后，alice `send` → ack seq=1，bob 收到广播；
4. 用相同 `client_msg_id` 重试 → ack `duplicate=true`、**不重复广播**；
5. alice 断线期间 bob 发消息；alice 重连后 `history after_seq=1` 恰好补回该条，HTTP 补拉一致；
6. `presence` 返回在线成员 alice+bob；
7. 未认证 WS `join` → `{"error":{"code":"unauthorized"}}`（结构化错误帧，不断连接语义）。

```text
cargo test -p imbit --test e2e_im_test
```

## 与生产形态的差距（明确边界）

- 状态在进程内存（`ImStore`）：重启丢失；生产替换为 rust-ef DbContext，Handler 表面不变；
- 单实例扇出：房间广播直接走 `IConnectionRegistry`；跨节点发布 `RealtimeFanoutEvent`（事件传输适配 broker，见 [架构 ADR](mediator-handler-architecture.md)）；
- 未实现输入长度/频率的全局配额（当前用 `use_websocket_limits` 的连接级限流兜底）。

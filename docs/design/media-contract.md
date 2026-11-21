# 媒体控制面与适配契约（v2.4）

> 原则（宪法 D2/D11）：**媒体面不进 core**。框架只提供控制面原语（信令消息类型、
> 房间控制、TURN 凭据签发、QoS 指标摄入端口与契约），SFU/媒体服务器作为适配器存在。
> 本文定义 v2.4-1 已交付的部分与 v2.4-3 的适配契约。

## 1. 控制面与媒体面的边界

| 层 | 归属 | 内容 |
|---|---|---|
| 控制面（core/host） | 框架 | 信令消息、房间成员/权限、TURN REST 凭据、QoS 指标标签规范 |
| 业务控制面（参考应用） | `imbit/` | 房间创建/加入、成员校验、把信令路由给目标连接 |
| 媒体面（适配器） | 外部/SFU | SDP 协商、ICE、RTP/RTCP、转码、录制、SFU 转发（2.4-3 契约 + `crates/media-*`） |

框架不依赖任何编解码器、RTP 栈或 SFU 实现；换 SFU（LiveKit/mediasoup/Janus/自研）
不需要改 core。

## 2. 信令（`webx::SignalingMessage`）

```rust
pub struct SignalingMessage {
    pub room_id: String,
    pub from: String,                   // 服务端填充（认证身份），客户端不可伪造
    pub to: Option<String>,             // Some = 1:1 定向；None = 房间广播
    pub kind: SignalingKind,            // Offer | Answer | IceCandidate
    pub payload: serde_json::Value,     // SDP / candidate 等不透明载荷
}
```

传输：复用实时通道（IM 参考应用里就是消息即请求的 `{"type":"signal",...}` 帧）。
服务端职责（参考实现 `imbit/src/handlers.rs`）：

1. **鉴权**：只接受已认证连接（JWT/cookie）；
2. **成员校验**：sender 必须是房间成员（否则 `forbidden`）；定向目标也必须是成员；
3. **服务器打标**：`from` 用连接身份覆盖，`to` 用客户端提供的用户名解析为目标 user id；
4. **路由**：`to=Some` → `IConnectionRegistry::user_connections`（多端都收到）；
   `to=None` → `room_connections` 且排除发送方；
5. **结果**：`signaled { delivered }`；目标离线时 delivered=0（信令尽力而为，重连后重协商）。

### 典型流程

**1:1 通话**

```text
alice --signal{offer, to=bob}--> server --> bob
bob   --signal{answer, to=alice}--> server --> alice
alice --signal{ice_candidate, to=bob}--> ...  (trickle，双向)
```

**群组通话（SFU 或 mesh）**

```text
alice --signal{offer, to=bob}--> bob
alice --signal{ice_candidate}（无 to）--> 房间其他成员
（所有成员按 SFU 适配器约定发布/订阅；媒体流本身不经框架）
```

### 错误契约

| 场景 | WebSocket 错误帧 |
|---|---|
| 未认证 | `{"error":{"code":"unauthorized",...}}` |
| 非房间成员 | `code: "forbidden"` |
| 房间不存在 | `code: "not_found"` |
| 目标用户不存在 / 非成员 | `code: "not_found"` / `"forbidden"` |
| 载荷格式错误 | `code: "invalid_message"` / `"validation_failed"`（含 `field`） |

## 3. TURN REST 凭据（coturn `use-auth-secret`）

配置走框架配置节（单一来源，启动校验；见 [appsettings](../rust-webx/10-configuration/appsettings.md)）：

```json
{ "Turn": { "Secret": "…", "Urls": ["turn:turn.example:3478"], "TtlSeconds": 3600, "Realm": "example" } }
```

Host 启动时把它转成并注册 `TurnRestConfig`（`#[inject] Arc<TurnRestConfig>` 注入）；
代码侧也可 `HostBuilder::use_turn_config(...)` 覆盖。凭据签发：

```rust
let credentials = config.credentials_for(user_id)?; // 或 credentials_for_at(user_id, now)
```

算法（coturn REST API，RFC 5766 长期凭据的临时形态）：

```text
expiry   = now_unix + ttl_seconds
username = "{expiry}:{user_id}"
credential = base64(HMAC-SHA1(TURN_SECRET, username))
```

参考实现 `GET /api/turn[?ttl_seconds=60]`（Bearer 认证）返回：

```json
{ "urls": ["turn:turn.example:3478"],
  "username": "1700003600:u1",
  "credential": "VxXpjCSFPdJWG82MEry7sqq2IbM=",
  "ttl_seconds": 3600 }
```

安全要求：`Turn.Secret`（或 `APP__Turn__Secret`）只存在服务端，且必须与 coturn
`static-auth-secret=` 一致；username 绑定到认证 user id；TTL 受限（最长 24h）；不得把静态
TURN 密码下发到客户端。参考应用的 `TURN_SECRET`/`TURN_URLS` 只是开发环境到该配置节的便捷映射。

## 4. 媒体面适配契约（2.4-3 已交付）

SFU/媒体服务器实现 `webx::IMediaPlaneAdapter`（core 只定义契约，不含任何媒体栈）：

```rust
#[async_trait]
pub trait IMediaPlaneAdapter: Send + Sync {
    async fn join(&self, room_id: &str, user_id: &str) -> Result<MediaSession>;
    async fn leave(&self, session_id: &str) -> Result<()>;              // 幂等
    async fn publish(&self, session_id: &str, tracks: Vec<MediaTrackInfo>) -> Result<()>;
    async fn subscribe(&self, session_id: &str, tracks: Vec<MediaTrackInfo>) -> Result<()>;
    async fn stats(&self, session_id: &str) -> Result<MediaStats>;
    async fn close_room(&self, room_id: &str) -> Result<()>;
    fn record_qos(&self, meter: &dyn IMeter, session: &MediaSession, stats: &MediaStats) { ... }
}
```

- `MediaSession { session_id, room_id, user_id, published, subscribed }`：一个参与者在
  一个房间的媒体会话（同一用户多端 = 多个 session）；
- `MediaTrackInfo { id, kind: Audio|Video|Screen, label, paused }`；
- `MediaStats { rtt_ms, jitter_ms, packet_loss_ratio, bitrate_kbps, frames_dropped,
  resolution }`：`sanitized()` 把负值归零、丢包率钳到 `0..=1`；
- 错误：未知 session/room → `NotFound`，非法参数 → `Validation`。

### QoS 指标摄入

默认 `record_qos` 向 `IMeter` 写（标签 `room`、`user`）：

| 指标 | 类型 | 含义 |
|---|---|---|
| `media_rtt_ms` | gauge | 往返时延 |
| `media_jitter_ms` | gauge | 抖动 |
| `media_packet_loss_ratio` | gauge | 丢包率（0..=1） |
| `media_bitrate_kbps` | gauge | 估算码率 |
| `media_frames_dropped_total` | counter | 丢帧累计（仅 >0 时记） |

应用把 `IMeter` 注册进 host（`use_meter`）即可与 HTTP/WebSocket 指标一起导出；录制
切片时长/大小走 2.4-2 的 `RecordingMeta`。

### 参考适配器与契约测试

`InMemoryMediaPlaneAdapter` 随 core 发布（确定性 fake + 契约参考实现）：
`join/leave/publish/subscribe/stats/close_room` 与 `set_stats` 注入采样；契约测试
`contract_suite` 对任意 `IMediaPlaneAdapter` 断言会话生命周期、轨迹替换、404 语义、
QoS 指标写入（含丢包钳制）与房间关闭清理。真实 SFU 适配器在各自 crate 里复用同一
契约测试（2.4-7 starter 家族）。

**验收**：`cargo test -p rust-webx-core --lib media::`（2 项契约测试通过）。

## 6. 录制与对象存储（2.4-2）

框架只提供**存储端口**与录制元数据模型，不绑定云 SDK：

```rust
#[async_trait]
pub trait IObjectStore: Send + Sync {
    async fn put(&self, key, bytes, content_type) -> Result<ObjectMeta>;
    async fn get(&self, key) -> Result<Option<StoredObject>>;
    async fn head(&self, key) -> Result<Option<ObjectMeta>>;
    async fn delete(&self, key) -> Result<()>;
    async fn begin_upload(&self, key, content_type) -> Result<UploadId>;   // 断点续传
    async fn put_part(&self, upload, part, bytes) -> Result<PartEtag>;     // 幂等重试
    async fn list_parts(&self, upload) -> Result<Vec<PartEtag>>;           // 恢复进度
    async fn complete_upload(&self, upload) -> Result<ObjectMeta>;         // 要求 part 从 1 连续
    async fn abort_upload(&self, upload) -> Result<()>;
    fn presign_put(&self, key, expires) -> Option<String> { None }         // 云适配器可选
}
```

- 两个实现随 core 发布：`InMemoryObjectStore`（fake，测试/演示）与
  `FileSystemObjectStore`（真实本地持久化，含路径穿越校验、分片合并、SHA1 etag）。
  S3 适配器归入 2.4-7 starter（实现同一端口，可返回预签名 URL 让客户端直传）。
- **录制元数据**：`RecordingMeta { id, room_id, owner_id, status, content_type,
  segments[part,key,size,duration_ms,started_at_unix], total_size, total_duration_ms,
  created_at_unix, completed_at_unix }`；`RecordingStatus::{Recording,Completed,Failed}`。
  播放索引用 `RecordingMeta::replace_segment`（重试同一 part 时**重算** total，不会膨胀）。

参考应用端点（`imbit`，成员校验见前文）：

```text
POST /api/recordings                              {"room_id","content_type"} -> RecordingView
POST /api/recordings/{id}/parts/{part}            {"data_base64","duration_ms","started_at_unix"} -> RecordingView
POST /api/recordings/{id}/complete                -> RecordingView（status=completed）
GET  /api/recordings/{id}                         -> RecordingView（播放索引）
GET  /api/recordings/{id}/parts/{part}            -> 原始字节（ContentResult + 录制 content-type）
```

断点续传语义：同一 `part` 重复上传是幂等替换（存储对象覆盖 + 索引替换），因此客户端
断线后只需重传未确认的分片；`complete` 要求至少一个分片。生产传输建议 multipart
或预签名直传，参考应用用 base64 JSON 保持依赖最小。

实现：`imbit/src/recording.rs`；验收：`imbit/tests/recording_e2e_test.rs`（分片上传、
重试不重复计入、完成、按 content-type 回放、非成员 403、缺分片 404、空录制 400）。

## 7. 验收总览（v2.4-1 / 2.4-2 / 2.4-3）

- 单测：[signaling.rs](../../crates/core/src/realtime/signaling.rs)（TURN REST 已知向量、
  配置校验、信令消息往返）、[storage.rs](../../crates/core/src/storage.rs)（内存/文件系统
  两种实现的 put/get/续传/中断/路径校验、录制索引与重算）、
  [media.rs](../../crates/core/src/media.rs)（适配器契约 + QoS 指标写入/钳制）；
- 端到端：[signaling_e2e_test.rs](../../imbit/tests/signaling_e2e_test.rs)、
  [recording_e2e_test.rs](../../imbit/tests/recording_e2e_test.rs)；
- 信令与聊天共用同一 WS 管线（鉴权、限流、背压、指标）——见
  [im-reference.md](im-reference.md) 与 `docs/rust-webx/05-request-pattern/websocket.md`。

# 长连接关闭码与排空语义

> 本文是 rust-webx 长连接**终止语义**的权威落点：WebSocket close frame code、SSE
> `StreamEnd`、HTTP 连接排空三者如何对应，以及应用侧应依据哪个类型分支。
> 当前基线 2.8.0；HTTP `problem+json` 的稳定错误码见
> [error-codes.md](error-codes.md)。

---

## 1. 分层：原因（in-process）与 wire code（on-wire）

| 层 | 类型 | 回答的问题 |
|---|---|---|
| 原因 | [`CloseReason`](../../crates/core/src/realtime.rs)（`#[non_exhaustive]`） | 框架**为什么**结束了这条连接 |
| 在线码 | [`CloseReason::code()`](../../crates/core/src/realtime.rs) | 服务端发出的 RFC 6455 close code |
| 观察点 | `IConnection::close_reason()` | 业务代码/观测侧读取已记录的原因（默认 `None`） |
| SSE 对端 | [`StreamEnd`](../../crates/core/src/stream.rs) | 流为什么结束（SSE 没有 wire close code） |
| 主机排空 | [`ShutdownToken::is_draining()`](../../crates/core/src/hosted.rs) | 停机是否已开始、连接是否该收尾 |

`CloseReason` 是**非 exhaustive** 的：2.x 新增原因不会破坏下游 `match`。
`code()` 返回 `Option<u16>`，为将来只存在于进程内、不写关闭帧的原因保留加性空间；
当前每个原因都有 wire code。

## 2. WebSocket 关闭原因与 wire code

| 原因 | 触发条件 | 服务端 wire code | 客户端可观察信号 | SSE 对应 `StreamEnd` |
|---|---|---|---|---|
| `ClientClosed` | 对端发 close frame / 对端断开 / 读端 EOF | 回显对端握手（无码时为空 close frame） | close frame 或 EOF | `ClientDisconnected` |
| `ServerShutdown` | 主机开始优雅停机（`ShutdownToken` 被取消） | `1001`（going away） | close frame `1001` | `ServerShutdown`（先写 `event:server_shutdown`） |
| `SessionCompleted` | 应用 Handler 正常返回 | `1000` | close frame `1000` | `Completed` |
| `IdleTimeout` | `WebSocketLimits::idle_timeout` 内无入站帧 | `1001` | close frame `1001` | —（SSE 用 `keep_alive`） |
| `LifetimeExceeded` | `max_connection_lifetime` 到期（流量不延期） | `1001` | close frame `1001` | `Deadline`（`max_lifetime`） |
| `TokenExpired` | `close_at_token_expiry` 且升级令牌 `exp` 已过 | `1008` | close frame `1008` | —（用 `on_server_shutdown` 收尾） |
| `ProtocolError { code }` | 协议/策略违规关闭，例如入站限流 | `code`（限流为 `1008`） | close frame + 先行的 `{"error":{"code":...}}` 帧 | `Failed` |
| `Internal` | Handler 返回 `Err` / 内部错误结束会话 | `1011`（记录用；正常路径不会由框架主动写） | 连接结束/错误帧 | `Failed` |

语义细节：

1. **原因先记录、后发帧**：框架在写 close frame 之前先
   `record_close_reason`，因此 `close_reason()` 与客户端观察到的 wire code 一致；
   同一个连接只保留**第一个**原因，收尾路径不会覆盖它。
2. **应用主动 `IConnection::close(code, reason)`**：这是应用自己的语义，
   `close_reason()` 保持 `None`——应用选的 wire code 才是权威（同时进入
   `connection.closed` 指标 `outcome`）。框架只在“框架决定关闭”时记录原因。
3. **客户端 close code 透传**：对端发送的 close code/reason 通过
   [`RealtimeMessage::Close`](../../crates/core/src/realtime.rs) 交给 Handler，
   `CloseReason` 只表达“谁发起”，不重复携带对端码。
4. **限流关闭**：先发送结构化错误帧（`code=rate_limited`），再以 `1008` 关闭，
   记录为 `ProtocolError { code: 1008 }`。
5. **SSE 没有 wire close code**：chunked body 结束即结束。`SseResult::on_close`
   报告的 `StreamEnd` 是 SSE 侧的权威原因，与 `CloseReason` 的映射见上表；
   框架不会把 SSE 结束映射成 WebSocket close frame。

## 3. 排空（drain）语义

主机收到关闭信号（Ctrl+C / SIGTERM / `ServerHandle::shutdown()` / `IHost::stop()`）后：

```
信号 → shutdown_token.cancel()（同时置 is_draining() == true）
     → 长连接主动收尾（WS: 1001；SSE: server_shutdown 事件）
     → 停止接受新连接 → 等待在途请求（DrainTimeout / StreamDrainTimeout）
     → hosted services 收尾 → provider.dispose()
```

- `ShutdownToken::is_cancelled()` 与 `is_draining()` 观察**同一个事件**；
  `is_cancelled` 面向生命周期代码，`is_draining` 面向上层“连接该开始收尾了”的意图。
  排空预算从该事件之后开始计算，因此等待 `is_draining()` 再收尾仍然有时间
  冲刷最后一帧/完成 close 握手。
- WebSocket 会话把 Handler future 与 `shutdown_token.cancelled()` 一起
  `select!`：停机时记录 `ServerShutdown` 并写 `1001`，而不是等排空预算耗尽后被
  abort（旧行为下客户端只会看到连接被切断，无法区分部署与故障）。
- SSE 由 `SseResult::on_server_shutdown` 写最后一帧
  `event:server_shutdown\ndata:{}\n\n` 后正常结束，报告 `StreamEnd::ServerShutdown`。
- 普通 HTTP 短请求没有 `CloseReason`：它们要么完成，要么由
  `Server.RequestTimeoutSeconds` 取消为 `504`；连接级排空由引擎发送
  `Connection: close`（见
  [优雅关闭与可观测性](../rust-webx/11-production/graceful-shutdown.md)）。

## 4. 与 quiche `draining` 概念的对应

框架借用了 quiche 的分层，但语义落在应用会话而不是 QUIC 连接上：

| quiche 概念 | rust-webx 对应 | 差异 |
|---|---|---|
| `ConnectionError`（本地原因枚举） | `CloseReason` | 非 exhaustive，2.x 可加 |
| `is_draining()`（进入 draining 状态） | [`ShutdownToken::is_draining()`](../../crates/core/src/hosted.rs) | 主机级而非单连接级；所有长连接共享同一事件 |
| `WireErrorCode`（线上错误码） | `CloseReason::code()` / RFC 6455 close code | WebSocket 标准码空间（1000–4999） |
| `ApplicationClose`（应用自选关闭） | `IConnection::close(code, reason)` | `close_reason()` 不记录，wire code 为权威 |
| transport error | 读端 `Err` → Handler 收到 `Error::Internal` | 没有独立 wire code，靠错误帧/日志诊断 |

## 5. 稳定性承诺

- wire code 与既有字符串**不再改变**：`1001`（lifetime / idle / shutdown）、
  `1008`（token expiry / rate limit）、`1000`（正常完成）保持原语义。
- 新增原因 = 新增 `CloseReason` 变体（加性）；新增 wire code 必须在本文表格登记。
- 应用自定义 close code（4000–4999）不属于框架契约，直接走 `IConnection::close`。

## 6. 相关

- [error-codes.md](error-codes.md)：HTTP `problem+json` 的稳定 `code` 注册表。
- [优雅关闭与可观测性](../rust-webx/11-production/graceful-shutdown.md)：排空预算与部署清单。
- 源码：[`crates/core/src/realtime.rs`](../../crates/core/src/realtime.rs)、
  [`crates/core/src/hosted.rs`](../../crates/core/src/hosted.rs)、
  [`crates/core/src/stream.rs`](../../crates/core/src/stream.rs)、
  [`crates/host/src/websocket.rs`](../../crates/host/src/websocket.rs)。

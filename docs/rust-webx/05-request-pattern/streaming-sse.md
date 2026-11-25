# 流式响应（SSE）

普通端点返回一次 JSON；当服务端需要**边产生边推送**（大模型逐 token 输出、进度、日志、实时事件）时，
用 `SseResult` 返回 `text/event-stream`。它和文件下载一样属于「自控响应」：请求类型写
`IRequest<Response>`，Handler 返回 `Ok(Box::new(SseResult::new(events)))`。

> 不需要按 SSE 协议编码、只是发送任意字节流（CSV、音视频分片）时，用
> [`FileResult::stream`](file-upload-download.md)；`SseResult` 只负责 SSE 帧与生命周期。

## 契约

| 项 | 值 |
|----|----|
| 请求类型 | `IRequest<Response>` |
| 返回 | `SseResult` |
| 状态码 | 固定 `200` |
| `content-type` | `text/event-stream; charset=utf-8` |
| `cache-control` | `no-store`（框架设置；可用 `.header(..)` 覆盖） |
| 传输 | chunked，无 `Content-Length`；不压缩 |
| 结束 | 源流结束 / 客户端断开 / `max_lifetime` 到点 / 源流出错 / 主机优雅停机 |

## 最小示例

```rust
use std::time::Duration;
use webx::*;

#[derive(Debug, Default, WebxRequestMeta)]
pub struct StreamProgressRequest {
    #[frompath]
    pub id: String,
}

#[get("/api/jobs/{id}/events")]
impl IRequest<Response> for StreamProgressRequest {}

#[derive(Inject)]
pub struct StreamProgressHandler {
    #[inject]
    pub jobs: Arc<JobService>,
}

#[handler(inject)]
#[async_trait::async_trait]
impl IRequestHandler<StreamProgressRequest, Response> for StreamProgressHandler {
    async fn handle(&mut self, req: StreamProgressRequest) -> ActionResult<Response> {
        let events = self.jobs.events(&req.id).await?; // Stream<Item = Result<SseEvent>>
        Ok(Box::new(SseResult::new(events).keep_alive(Duration::from_secs(15))))
    }
}
```

`futures_util` 的 `Stream` 可直接交给 `SseResult`；项目需要把 `futures-util` 加进依赖。

## 事件编码

```rust
SseEvent::data("hello")                       // data:hello
SseEvent::event("progress", "42")             // event:progress\ndata:42
SseEvent::data("a\nb")                        // data:a\ndata:b（多行自动拆分）
SseEvent::event("tick", "1")
    .with_id("evt-7")                         // id:evt-7
    .with_retry(1500)                         // retry:1500（毫秒）
```

- `data` 中的 `\n` 会拆成多条 `data:` 行；SSE 客户端会再用 `\n` 拼回。
- `event` / `id` 不允许包含 CR、LF、NUL；框架返回 `Error::validation` 并结束流（防止协议行注入）。
- 空 `data` 仍输出一条空的 `data:` 行，保证事件会被分发。

## 生命周期与计量

| 方法 | 作用 |
|------|------|
| `keep_alive(Duration)` | 空闲时发送 `: ping` 注释帧，避免代理/网关在静默期断开 |
| `max_lifetime(Duration)` | 限制单条流的总时长，防止僵尸流占满并发槽 |
| `on_server_shutdown(CancellationToken)` | 主机开始排空时写最后一帧 `event:server_shutdown\ndata:{}` 并正常结束 |
| `on_close(FnOnce(StreamEnd))` | 流结束时**恰好一次**回调：`Completed` / `ClientDisconnected` / `Failed` / `Deadline` / `ServerShutdown` |
| `header(k, v)` | 追加/覆盖响应头（CORS 由中间件负责，通常不需要） |

```rust
let usage = Arc::clone(&self.usage);
let log = Arc::clone(&self.log);
Ok(Box::new(
    SseResult::new(events)
        .keep_alive(Duration::from_secs(15))
        .max_lifetime(Duration::from_secs(600))
        .on_server_shutdown(self.shutdown.token().clone())
        .on_close(move |end| {
            // 回调是同步的：不要在这里 await；结算放到后台任务。
            let end = matches!(end, StreamEnd::Completed);
            tokio::spawn(async move { usage.settle(end).await });
        }),
))
```

断连时框架会丢弃响应 body 并触发 `ClientDisconnected`，上游请求随之取消；计费/配额释放应在这个
回调里完成，而不是在 Handler 返回之后（那时流还没有结束）。

`StreamEnd` 与统一关闭语义 `CloseReason` 的映射（`Completed → SessionCompleted`、
`ClientDisconnected → ClientClosed`、`Deadline → LifetimeExceeded`、
`ServerShutdown → ServerShutdown`、`Failed → Internal/ProtocolError`）以及与 WebSocket
close code 的对照见[关闭码与排空语义](../../design/close-codes.md)。

## 优雅停机：`server_shutdown` 事件

默认的排空策略是"等连接自己结束，超时后再 abort"，长流会被截断。`SseResult::on_server_shutdown`
让流自己收尾：

1. Handler 注入框架注册的停机令牌（`Host::build` 总是注册）：

   ```rust
   #[derive(Inject)]
   pub struct EventsHandler {
       #[inject]
       pub shutdown: Arc<ShutdownToken>,
   }
   ```

2. 返回流时调用 `.on_server_shutdown(self.shutdown.token().clone())`。
3. 收到 Ctrl+C / SIGTERM / `ServerHandle::shutdown()` / `IHost::stop()` 时，框架同时：
   - 取消令牌；
   - 通知监听器停止接受新连接。
4. 流在下一个 poll 写 `event:server_shutdown\ndata:{}`，然后正常关闭，`on_close` 收到
   `StreamEnd::ServerShutdown`。客户端据此重连到新实例，而不是把断流当成上游故障。

停机预算仍由 `Server.DrainTimeoutSeconds` / `Server.StreamDrainTimeoutSeconds` 控制（见
[优雅关闭](../11-production/graceful-shutdown.md)）；`server_shutdown` 让正常路径不再依赖超时。

## 错误语义

- 源流产出 `Err`：框架正常结束 HTTP 流并记 `StreamEnd::Failed`；**不会**再写 JSON 错误体
  （响应头已经发出）。需要客户端看到错误时，自己发一个 `SseEvent::event("error", "...")`
  再结束。
- 请求在返回 `SseResult` 之前失败（鉴权、绑定、配额）：仍是普通 `Err(Error)`，框架写
  `problem+json` 和对应状态码。

## 部署提醒

- 反向代理要关闭响应缓冲：nginx `proxy_buffering off;`，Caddy `flush_interval -1`。
- 不要对 `text/event-stream` 开 gzip（框架的压缩中间件已跳过）。
- 滚动发布时给客户端处理 `server_shutdown`：收到后重连（可带 `Last-Event-ID` 续传）即可平滑切换。
- SSE 不是 WebSocket：单向、纯文本帧、复用 HTTP/1.1 或 HTTP/2 连接；双向实时通信用独立方案。

## 测试

`crates/webx/tests/sse_test.rs` 覆盖了：响应头、chunked、逐帧到达（不缓冲）、客户端断开触发
`ClientDisconnected`、`ServerHandle::shutdown()` 后收到 `event:server_shutdown` 并触发
`ServerShutdown`。单元测试在 `crates/core/src/stream.rs`，覆盖编码与
`Completed`/`Deadline`/`ServerShutdown` 路径。

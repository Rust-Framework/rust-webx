# 优雅关闭与可观测性

## 优雅关闭

`Host::run()` 监听 shutdown 信号（Ctrl+C / SIGTERM）：

```
收到信号 → 停止接受新连接
        → IHostedService::stop() 逆序（每个 ≤30s，BackgroundService 先取消令牌）
        → 进行中请求完成（连接收到 Connection: close）
        → provider.dispose() → 退出
```

hosted services **先于连接排空**停止：后台 worker 不会在进程已经在关闭时继续访问数据库/缓存，
`BackgroundService` 的取消令牌在信号到达时立刻触发（长 `sleep` 用 `background::delay` 即时返回）。

关闭信号用 `watch` 通道保存，不是一次性的唤醒：`ServerHandle::shutdown()` / `IHost::stop()` 即使在服务器开始监听之前调用也不会丢失。
排空预算由 `Server` 配置决定：

```json
{
  "Server": {
    "DrainTimeoutSeconds": 30,
    "StreamDrainTimeoutSeconds": 300,
    "RequestTimeoutSeconds": 0
  }
}
```

超时后仍未完成的连接会被中止；`stop()` 只执行一次：信号到达时的 watcher 与排空后的收尾共用同一个 `OnceCell`。

### Worker 池（2.0）

2.0 的执行引擎是“单 acceptor + N 个 `current_thread` worker runtime”：每个 worker 独占一个
Tokio runtime 和 hyper `auto::Builder`，连接通过有界 channel 交接，不会跨核迁移。默认
`Workers = available_parallelism()`（`Development` 下限到 8，避免测试同时起大量主机时线程膨胀），范围 `1..=64`：

```json
{ "Server": { "Workers": 8 } }
```

也可用环境变量 `APP__Server__Workers` 覆盖；`1` 适合调试/单核容器，`0` 表示自动。
`App.MaxConnections` 仍是**全局**连接上限，引擎内部按 worker 分摊预算。

### 入站请求超时

`Server.RequestTimeoutSeconds`（默认 `0` = 关闭）给单个请求一个截止时间：从路由命中后开始，
覆盖绑定 + Handler 执行。超时后框架取消 Handler future 并返回 `504`（仍走统一的 problem+json /
错误写入器）。它**不**限制流式响应体：`SseResult` Handler 很快返回，之后流由
`max_lifetime` / 停机排空预算约束。

```json
{ "Server": { "RequestTimeoutSeconds": 120 } }
```

适合给上游调用/大请求一个上限；如果业务需要按路由差异化 deadline，应在 Handler/出站客户端层
自行设置（`OutboundRequest::timeout`）。

### 长流（SSE）排空

- 普通连接使用 `DrainTimeoutSeconds`（默认 30s）。
- 只要连接上产生过 `text/event-stream` 响应，就使用 `StreamDrainTimeoutSeconds`（默认 300s）。
- 主机按连接跟踪 SSE 活动：首个 SSE 响应标记，连接结束时释放。
- `SseResult::max_lifetime` 仍应设置，兜底防止僵尸流占满并发槽。
- 预算最小 1s；超时后 abort 剩余连接。

**注意**：当前是“连接级”预算，不是“单条流”精确预算；有 SSE 的连接会享受较长的总预算。
Handler 注入 `Arc<ShutdownToken>` 并调用 `SseResult::on_server_shutdown` 后，流会在停机时收到
`event:server_shutdown` 并正常结束（见[流式响应](../05-request-pattern/streaming-sse.md)），
不再依赖排空超时。

## 可观测性

### 请求追踪

- `RequestIdMiddleware`：每个请求生成唯一 `X-Request-Id`
- `RequestTracing`：结构化请求日志，并写入 `x-trace-id` **响应头与请求上下文**；
  Handler / 审计 helper 用 `webx::RequestContext::trace_id()` 读到与响应头一致的值
- `TimingMiddleware`：`Server-Timing` 响应头

### 启用 tracing

```rust
tracing_subscriber::fmt::init();
```

### 日志最佳实践

```rust
tracing::info!(request_id = %id, path = %path, "Request started");
tracing::error!(error = %e, "Handler failed");
```

## 生产部署清单

- [ ] `AppMode::Production`
- [ ] 强 JWT Secret
- [ ] TLS 证书配置
- [ ] 健康检查端点配置
- [ ] `Server.DrainTimeoutSeconds` / `StreamDrainTimeoutSeconds` 按部署调整
- [ ] `Server.RequestTimeoutSeconds` 按上游 p99 设置（0 表示不限）
- [ ] SSE `max_lifetime` 与 `on_server_shutdown` 设置
- [ ] 日志聚合（stdout → 日志服务）
- [ ] 速率限制启用

## 小结

框架内置优雅关闭和基础可观测性，生产环境配合 tracing + 日志聚合即可。

下一章：[项目组织与职责划分](../12-project-structure/INDEX.md)

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
    "RequestTimeoutSeconds": 30
  }
}
```

超时后仍未完成的连接会被中止；`stop()` 只执行一次：信号到达时的 watcher 与排空后的收尾共用同一个 `OnceCell`。

### Worker 池

执行引擎是“单 acceptor + N 个 `current_thread` worker runtime”：每个 worker 独占一个
Tokio runtime 和 hyper `auto::Builder`，连接通过有界 channel 交接，不会跨核迁移。默认
`Workers = available_parallelism()`（`Development` 下限到 8，避免测试同时起大量主机时线程膨胀），范围 `1..=64`：

```json
{ "Server": { "Workers": 8, "FirstRequestTimeoutSeconds": 10 } }
```

也可用环境变量 `APP__Server__Workers` 覆盖；`1` 适合调试/单核容器，`0` 表示自动。
`App.MaxConnections` 是**单个引擎实例的全局**准入上限：所有 http/https listener
和 worker 共享一个 Semaphore，不会按 worker/listener 重复计算。首请求超时关闭
只建连不发送首个请求（或不完整 h2 preface）的连接，避免静默连接占满准入槽。

### 入站请求超时

`Server.RequestTimeoutSeconds`（默认 `30` 秒）给单个请求一个截止时间：从路由命中后开始，
覆盖绑定 + Handler 执行。超时后框架取消 Handler future 并返回 `504`（仍走统一的 problem+json /
错误写入器）。它**不**限制流式响应体：`SseResult` Handler 很快返回，之后流由
`max_lifetime` / 停机排空预算约束。

`0` 表示关闭——只在 `Development` 允许（调试/浸泡测试）；`Production` 会在启动期拒绝 `0`，
因为挂起的 Handler 会永久占用一个连接与准入许可。大文件上传等确实可能超过 30 秒的端点，
应显式调高预算而不是关闭：

```json
{ "Server": { "RequestTimeoutSeconds": 120 } }
```

适合给上游调用/大请求一个上限；如果业务需要按路由差异化 deadline，应在 Handler/出站客户端层
自行设置（`OutboundRequest::timeout`）。

### 过载卸载（准入超时）

`App.MaxConnections` 是全局准入上限。已 accept 但拿不到准入槽的连接不再无限等待内核
backlog，而是等待 `Server.AdmissionTimeoutMs`（默认 `1000`）后收到：

```http
HTTP/1.1 503 Service Unavailable
retry-after: 1
connection: close
```

- `0` 表示无限等待（旧行为）；`Production` 在启动期拒绝 `0`。
- 指标：`engine_connections_admitted_total`、`engine_connections_rejected_total`、
  `engine_admission_waiting`（队列深度）。
- 过载时"尽快拒绝"优于排队：排队只会把延迟转成超时，再放大成重试风暴。

```json
{ "App": { "MaxConnections": 2048 }, "Server": { "AdmissionTimeoutMs": 500 } }
```

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
- [ ] `Server.RequestTimeoutSeconds` 按上游 p99 设置（默认 30；Production 不允许 0）
- [ ] SSE `max_lifetime` 与 `on_server_shutdown` 设置
- [ ] 日志聚合（stdout → 日志服务）
- [ ] 速率限制启用

## 小结

框架内置优雅关闭和基础可观测性，生产环境配合 tracing + 日志聚合即可。

下一章：[项目组织与职责划分](../12-project-structure/INDEX.md)

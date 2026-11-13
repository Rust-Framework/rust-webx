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
优雅关闭的超时（排空窗口）在 Development 下为 5s、其他模式为 30s；超时后仍未完成的连接会被中止。
`stop()` 只执行一次：信号到达时的 watcher 与排空后的收尾共用同一个 `OnceCell`。

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
- [ ] 日志聚合（stdout → 日志服务）
- [ ] 速率限制启用

## 小结

框架内置优雅关闭和基础可观测性，生产环境配合 tracing + 日志聚合即可。

下一章：[项目组织与职责划分](../12-project-structure/INDEX.md)

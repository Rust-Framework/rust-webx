# 优雅关闭与可观测性

## 优雅关闭

`Host::run()` 监听 shutdown 信号（Ctrl+C / SIGTERM）：

```
收到信号 → 停止接受新连接 → 等待进行中请求完成 → IHostedService::stop() 逆序 → 退出
```

## 可观测性

### 请求追踪

- `RequestIdMiddleware`：每个请求生成唯一 `X-Request-Id`
- `RequestTracing`：结构化请求日志
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

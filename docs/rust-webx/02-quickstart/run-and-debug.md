# 运行、调试与验证

## 启动服务

### 基本运行

```bash
cargo run
```

默认监听 `appsettings.json` 中 `App.Urls` 配置的地址（默认 `http://0.0.0.0:5000`）。

### 指定开发模式

```rust
Host::builder()
    .mode(AppMode::Development)
    .build()
    .run()
    .await?;
```

开发模式会合并 `appsettings.Development.json`，并启用更详细的日志。

### 运行本仓库示例

```bash
# Docbit 作品集
cargo run -p docbit

# 仅编译检查
cargo check --workspace
```

## 验证端点

### curl

```bash
curl -v http://localhost:5000/hello
curl -H "Authorization: Bearer <token>" http://localhost:5000/api/auth/me
```

### OpenAPI / Swagger UI

框架在开发模式下自动暴露 OpenAPI 端点。访问：

```
http://localhost:5000/swagger
```

可交互测试所有已注册路由。

### 健康检查

```
GET /health
```

返回服务健康状态，适用于容器编排探针。

## 调试技巧

### 启用 tracing 日志

在 `main.rs` 开头：

```rust
tracing_subscriber::fmt::init();
```

中间件如 `RequestTracing`、`TimingMiddleware` 会输出请求耗时与路径。

### 常见启动错误

| 错误现象 | 原因 | 解决 |
|---------|------|------|
| `No handler registered` | Handler 未注册到 DI | 检查 `#[handler]` 或手动 `singleton` 注册 |
| `route not found` 404 | 路由未收集 | 确认 `#[get]` 标注在 `impl IRequest` 上 |
| 端口占用 | 5000 已被使用 | 修改 `App.Urls` |
| 类型不匹配 panic | Request/Handler 响应类型不一致 | 确保 `IRequest<T>` 的 T = Handler 的 R |

### 编译时检查清单

```bash
cargo check 2>&1 | head -50   # 查看编译错误
cargo test -p rust-webx-host # 运行框架集成测试
```

## 热重载

Rust 暂无内置热重载。开发时推荐使用 [cargo-watch](https://crates.io/crates/cargo-watch)：

```bash
cargo install cargo-watch
cargo watch -x run
```

## 生产构建

```bash
cargo build --release
./target/release/my-api
```

Workspace 的 release profile 已配置 LTO 与 strip，产物体积与性能均经过优化。

## 小结

运行 rust-webx 应用只需 `cargo run`；验证通过 curl、Swagger UI 或集成测试。遇到问题时，先查 Handler 注册与路由标注两个最高频原因。

本章完成！下一章进入设计哲学：[第三章](../03-philosophy/INDEX.md)

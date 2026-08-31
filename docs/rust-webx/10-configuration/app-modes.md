# AppMode 与环境切换

## 模式

```rust
pub enum AppMode {
    Development,
    Production,
}
```

## 设置

```rust
Host::builder()
    .mode(AppMode::Development)
    .build()
```

## 行为差异

| 行为 | Development | Production |
|------|------------|------------|
| 加载 Development.json | ✅ | ❌ |
| 详细错误信息 | ✅ | 精简 |
| Swagger UI | 可用 | 建议关闭 |
| 日志级别 | debug | info/warn |

## 环境变量

```bash
RUST_LOG=debug cargo run
```

配合 `tracing` 使用。

## 小结

通过 `AppMode` 切换开发与生产行为，配合分层配置文件实现环境隔离。

下一节：[自定义配置节](custom-options.md)

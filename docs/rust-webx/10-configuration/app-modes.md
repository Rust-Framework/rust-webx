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
| 环境 overlay | `appsettings.Development.json` | `appsettings.Production.json` |
| 内置 API 文档 | 仅 Development | 不注册 |
| 日志级别 | `RUST_LOG` 控制（默认 `info`） | 同 Development |

## 环境变量

```bash
APP_ENV=Production cargo run
RUST_LOG=debug cargo run
```

`APP_ENV` 选择运行模式（`Production` / `Prod` 或 `Development` / `Dev`，未设置时默认 `Development`）；`RUST_LOG` 控制 `tracing` 日志过滤级别（默认 `info`）。

## 小结

通过 `AppMode` 切换开发与生产行为，配合分层配置文件实现环境隔离。

下一节：[自定义配置节](custom-options.md)

# 创建项目与依赖

## 新建 Cargo 项目

```bash
cargo new my-api
cd my-api
```

## 配置 Cargo.toml

```toml
[package]
name = "my-api"
version = "0.1.0"
edition = "2021"

[dependencies]
rust-webx = "0.2"
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
async-trait = "0.1"
```

若在本仓库内开发，使用 path 依赖：

```toml
rust-webx = { path = "../crates/webx" }
```

## 可选：appsettings.json

在项目根目录创建配置文件（框架自动加载）：

```json
{
  "App": {
    "Name": "My API",
    "Urls": ["http://0.0.0.0:5000"]
  },
  "Jwt": {
    "Secret": "change-me-in-production"
  },
  "Cors": {
    "Origins": ["*"],
    "Methods": ["GET", "POST", "PUT", "DELETE", "OPTIONS"],
    "Headers": ["Content-Type", "Authorization"]
  }
}
```

开发环境可额外提供 `appsettings.Development.json`，框架在 `AppMode::Development` 下自动合并。

## 项目目录建议

即使是 Hello World，也建议从一开始就采用约定结构：

```
my-api/
├── Cargo.toml
├── appsettings.json
└── src/
    ├── main.rs
    ├── contracts/     # 请求定义（路由 + DTO）
    │   └── mod.rs
    └── handlers/      # 处理器实现
        └── mod.rs
```

在 `main.rs` 中声明模块：

```rust
mod contracts;
mod handlers;
```

## 从 Workspace 模板创建

也可直接参考本仓库的 `docbit` 项目：

```bash
cargo run -p docbit
```

Docbit 展示了完整的中型项目结构，详见 [第十五章案例研究](../15-case-study/INDEX.md)。

## 小结

依赖只需 `rust-webx` 一个伞 Crate 即可获得全部框架能力。下一节编写第一个端点。

下一节：[Hello World 详解](hello-world.md)

# 推荐目录结构

## 标准布局

```
my-app/
├── Cargo.toml
├── appsettings.json
├── appsettings.Development.json
├── wwwroot/                  # SPA 前端（可选）
└── src/
    ├── main.rs               # 组合根：Host 启动，仅框架级配置
    ├── startup.rs            # IHostedService 初始化
    ├── common/               # 共享工具、bootstrap、拦截器
    ├── contracts/            # 契约：Request/Response/enum/I…Service
    ├── handlers/             # 应用层：Handler + Service 实现
    └── domain/               # 实体 + 迁移
```

**不设** `services/`、`requests/` 目录。业务接口在 `contracts/`，实现在 `handlers/`。

## 模块声明

```rust
// main.rs
mod common;
mod contracts;
mod handlers;
mod domain;
mod startup;
```

## 各目录说明

| 目录 | 内容 | 依赖 |
|------|------|------|
| `contracts/` | `IRequest`、DTO、enum、`IBlogService` 等 trait | 仅框架 |
| `handlers/` | `LoginHandler`、`BlogService`（impl `IBlogService`） | contracts、domain |
| `domain/` | `UserEntity`、migrations | contracts（可选）、无框架 |
| `common/bootstrap.rs` | `AppPaths`、`DbContext` 手动注册 | 基础设施 |
| `appsettings.json` | 端口、JWT、缓存等框架配置 | — |

## 规模扩展

### 中型项目（单 crate 多文件）

按业务域拆分 contracts 与 handlers（同名校验）：

```
contracts/
├── auth.rs       # LoginRequest + IAuthService trait
├── blog.rs       # Blog DTO + IBlogService trait
└── docs.rs

handlers/
├── auth.rs       # AuthService + LoginHandler
├── blog.rs       # BlogService + BlogHandlers
└── docs.rs
```

### 大型项目（workspace 多 crate）

```
my-app/
├── crates/
│   ├── api/          # Host 入口（main.rs）
│   ├── contracts/    # 共享契约 crate
│   ├── handlers/     # 实现 crate
│   └── domain/       # 领域模型 crate
```

跨 crate 时仍遵守：contracts 不依赖 domain；domain 可依赖 contracts。

## 小结

从一开始就采用约定结构，避免项目长大后的重构痛苦。详见 [Contracts / Handlers / Domain 分层](contracts-handlers-domain.md)。

下一节：[职责归属与边界](responsibility-division.md)

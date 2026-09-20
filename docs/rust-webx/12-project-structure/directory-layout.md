# 推荐目录结构

## 标准布局（workspace）

真实应用（docbit、dmbit）与框架本身都按 **workspace** 组织：每个职责一个 crate，`host` 是唯一的可执行入口。

```
my-app/
├── Cargo.toml               # [workspace] members = ["contracts", "domain", "handlers", "host"]
├── appsettings.json
├── appsettings.Development.json
├── wwwroot/                 # SPA 前端（可选）
├── contracts/               # 契约：Request/Response/enum/I…Service
│   └── src/
├── domain/                  # 领域：实体、EF 配置、seed
│   └── src/
├── handlers/                # 应用层：Handler + Service 实现
│   └── src/
└── host/                    # 组合根 + startup
    ├── Cargo.toml           # name = "my-app-host"
    ├── build.rs             # webx::builder().web_root("../wwwroot").build()
    └── src/
        ├── main.rs          # 组合根：#[webx::main(embed)]
        ├── lib.rs
        └── startup/
            ├── mod.rs
            ├── extensions/  # Add* / Use* 扩展方法
            ├── hosted/      # IHostedService 实现
            └── seed/        # 一次性数据初始化
```

**不设** `services/`、`requests/` 目录。业务接口在 `contracts/`，实现在 `handlers/`。

## 模块声明

`host` crate 声明 `startup` 子模块，其余 crate 各自声明自己的模块：

```rust
// host/src/lib.rs
pub mod startup;

// host/src/startup/mod.rs
pub mod extensions;
pub mod hosted;
pub mod seed;
```

## 各目录说明

| 目录 | 内容 | 依赖 |
|------|------|------|
| `contracts/` | `IRequest`、DTO、enum、`IDocumentService` 等 trait | 仅框架 |
| `handlers/` | `DocService`（impl `IDocumentService`）、各 `IRequestHandler` | contracts、domain |
| `domain/` | `UserEntity`、EF 配置、seed | contracts（可选）、无 host |
| `host/startup/extensions/` | `Add*` / `Use*` DI 扩展；用 `webx::app_base()` 定位数据库 | 基础设施 |
| `host/startup/hosted/` | `IHostedService`（schema 初始化、seed、索引构建） | handlers |
| `appsettings.json` | 端口、JWT、缓存等框架配置 | — |

## 规模扩展

### 单应用

`contracts` / `domain` / `handlers` / `host` 四个 crate 即可（见上）。

### 多应用（monorepo）

多个应用各自拥有四个 crate，共享同一个 workspace：

```
Cargo.toml                # members = ["docbit/*", "dmbit/*", ...]
docbit/
├── contracts/
├── domain/
├── handlers/
└── host/
dmbit/
├── contracts/
├── domain/
├── handlers/
└── host/
```

跨 crate 时仍遵守：contracts 不依赖 domain；domain 可依赖 contracts 与框架核心原语。

## 小结

从一开始就采用约定结构，避免项目长大后再调整结构的代价。详见 [Contracts / Handlers / Domain 分层](contracts-handlers-domain.md)。

下一节：[职责归属与边界](responsibility-division.md)

# 推荐目录结构

## 标准布局

```
my-app/
├── Cargo.toml
├── appsettings.json
├── appsettings.Development.json
├── wwwroot/                  # SPA 前端（可选）
└── src/
    ├── main.rs               # 组合根：Host 启动
    ├── startup.rs            # IHostedService 初始化
    ├── common/               # 共享工具、拦截器
    ├── contracts/            # API 契约（路由 + DTO）
    │   ├── mod.rs
    │   ├── auth.rs
    │   └── user.rs
    ├── handlers/             # 请求处理器
    │   ├── mod.rs
    │   ├── auth.rs
    │   └── user.rs
    ├── services/             # 领域服务
    │   ├── mod.rs
    │   └── email.rs
    └── domain/               # 实体 + 迁移
        ├── mod.rs
        ├── user.rs
        └── migrations/
```

## 模块声明

```rust
// main.rs
mod common;
mod contracts;
mod handlers;
mod services;
mod domain;
mod startup;
```

## 规模扩展

### 中型项目（单 crate 多文件）

保持上述结构，按业务域拆分 contracts/handlers：

```
contracts/
├── auth.rs
├── user.rs
├── blog.rs
└── docs.rs
```

### 大型项目（workspace 多 crate）

```
my-app/
├── crates/
│   ├── api/          # Host 入口
│   ├── contracts/    # 共享契约
│   ├── domain/       # 领域模型
│   └── infra/        # 基础设施
```

## 小结

从一开始就采用约定结构，避免项目长大后的重构痛苦。

下一节：[职责归属与边界](responsibility-division.md)

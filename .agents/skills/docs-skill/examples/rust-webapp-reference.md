# rust-webapp 参考实现

当需要**完整、已验证**的书籍样例时加载本文件。其他框架项目仅复用**结构模式**，不复用下列路径/API 字面量。

## 路径

| 项 | 值 |
|----|-----|
| slug | `rust-webapp` |
| 文档根 | `docs/rust-webapp/` |
| 维护说明 | `docs/README.md` |
| 预览 | `cargo run -p docbit` → 作品集 → rust-webapp |
| API | `GET /api/docs/rust-webapp/index`、`/content/{path}` |

## 约定（摘自 FOREWORD）

- 导入：`use rust_webapp::*;`
- 符号：`✅` / `❌`；路径参数 `{id}`
- 示例项目：`docbit`，`cargo run -p docbit`

## 全书 parts（已存在，勿强行改结构）

| Part | 章号 | 主题 |
|------|------|------|
| 入门与认知 | 01–02 | 认识、快速上手 |
| 设计思想与架构 | 03–04 | 哲学、架构 |
| 核心开发模式 | 05–08 | 请求、DI、中间件、中介者 |
| 安全、配置与生产 | 09–11 | 认证、配置、生产 |
| 工程化与进阶 | 12–14 | 结构、扩展、最佳实践 |
| 案例与迁移 | 15–16 | Docbit、迁移 |

## 推荐对照文件

| 用途 | 路径 |
|------|------|
| 前言与阅读路径 | `docs/rust-webapp/FOREWORD.md` |
| 全书目录 | `docs/rust-webapp/INDEX.md` |
| 站点元数据 | `docs/rust-webapp/INDEX.json` |
| 概念章 INDEX | `docs/rust-webapp/01-introduction/INDEX.md` |
| 动手章 INDEX | `docs/rust-webapp/02-quickstart/INDEX.md` |
| 教程小节 | `docs/rust-webapp/02-quickstart/hello-world.md` |
| 渐进式披露原文 | `docs/rust-webapp/03-philosophy/progressive-disclosure.md` |
| 设计原则 | `docs/rust-webapp/03-philosophy/design-principles.md` |

## L0–L4 在本项目的映射

```
L0: 02-quickstart/hello-world.md
L1: 02-quickstart/first-crud.md
L2: 09-auth-security, 10-configuration, 11-production
L3: 05–08 各核心模式章
L4: 12–14 工程化；15–16 案例与迁移
```

## 编写 rust-webapp 文档时

1. 仍先完成 [project-bootstrap](../reference/project-bootstrap.md) Profile（可快速填表）
2. 对照上表路径 mimic 结构与语气
3. 代码示例与 `rust_webapp` crate 源码一致
4. 框架编码技能见同仓库 `lrwf-skill`（API 细节），本书保持指南形态

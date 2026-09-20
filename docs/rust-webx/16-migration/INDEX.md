# 第十六章 迁移指南

本章面向**从其它框架迁移到 rust-webx** 的读者：概念如何对应、代码如何改写、
哪些习惯需要放下。

框架自身的版本升级记录不进入手册，统一维护在项目根目录的 `CHANGELOG.md`。

## 本章小节

| 小节 | 内容 |
|------|------|
| [升级到 0.5（破坏性变更）](upgrade-to-0.5.md) | 导入路径改名、SPA 内嵌模型调整 |
| [从 ASP.NET Core 迁移](from-aspnet-core.md) | .NET 开发者迁移路径 |
| [从 Axum / Actix 迁移](from-axum-actix.md) | Rust 原生框架迁移 |
| [概念对照表](concept-mapping.md) | 完整概念映射 |

## 下一步

已经在用 rust-webx？从 [升级到 0.5](upgrade-to-0.5.md) 开始。
从其它框架过来？看 [从 ASP.NET Core 迁移](from-aspnet-core.md) 或
[从 Axum / Actix 迁移](from-axum-actix.md)；遇到不熟悉的术语时查
[概念对照表](concept-mapping.md)。

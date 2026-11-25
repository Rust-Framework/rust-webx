# 第十六章 迁移指南

本章面向**升级 rust-webx 或从其它框架迁移**的读者：升级步骤、概念对应与代码改写。
版本变更记录统一维护在 `CHANGELOG.md`；兼容性承诺与 LTS 规则见
[compatibility.md](../../design/compatibility.md)。

## 本章小节

| 小节 | 内容 |
|------|------|
| [升级到 2.5（2.x 加性演进）](upgrade-to-2.5.md) | 2.1 → 2.5 升级步骤、可选能力清单、稳定面与弃用承诺 |
| [升级到 2.9.1（破坏性补丁）](upgrade-to-2.9.1.md) | Error 载体、分页、嵌套绑定、raw 排序、组合根动词、2.x 兼容层删除（授权/密钥/限流旧键）、`webx_ef` 删除、`*-ef` 移出发布序列与 outbox 拆分 |
| [从 ASP.NET Core 迁移](from-aspnet-core.md) | .NET 开发者迁移路径 |
| [从 Axum / Actix 迁移](from-axum-actix.md) | Rust 原生框架迁移 |
| [概念对照表](concept-mapping.md) | 完整概念映射 |

## 下一步

已经在用 2.x？直接看 [升级到 2.5](upgrade-to-2.5.md) 与
[升级到 2.9.1](upgrade-to-2.9.1.md)。

从其它框架过来？看 [从 ASP.NET Core 迁移](from-aspnet-core.md) 或
[从 Axum / Actix 迁移](from-axum-actix.md)；遇到不熟悉的术语时查
[概念对照表](concept-mapping.md)。

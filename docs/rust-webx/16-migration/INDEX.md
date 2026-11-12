# 第十六章 迁移指南

本章面向**从其它框架迁移到 rust-webx** 的读者：概念如何对应、代码如何改写、
哪些习惯需要放下。

框架自身的版本升级记录统一维护在项目根目录的 `CHANGELOG.md`；破坏性版本额外提供升级页。

## 本章小节

| 小节 | 内容 |
|------|------|
| [升级到 1.1（破坏性变更）](upgrade-to-1.1.md) | ResponseData 换成 IActionResult 族、IRequest<Response>、OpenAPI 占位组件 |
| [升级到 1.0（破坏性变更）](upgrade-to-1.0.md) | rust-dix 0.10 对齐、删除 ambient DI 与 erased pipeline、dispose/async build |
| [升级到 0.10（破坏性变更）](upgrade-to-0.10.md) | typed pipeline、移除全局 provider shim、try_build |
| [升级到 0.9（破坏性变更）](upgrade-to-0.9.md) | 默认安全、身份 fail-closed、错误/协议与扩展点收口 |
| [升级到 0.6（破坏性变更）](upgrade-to-0.6.md) | 端点目录、授权规则组、资源键本地化 |
| [升级到 0.5（破坏性变更）](upgrade-to-0.5.md) | 导入路径改名、SPA 内嵌模型调整 |
| [从 ASP.NET Core 迁移](from-aspnet-core.md) | .NET 开发者迁移路径 |
| [从 Axum / Actix 迁移](from-axum-actix.md) | Rust 原生框架迁移 |
| [概念对照表](concept-mapping.md) | 完整概念映射 |

## 下一步

已经在用 rust-webx？直接看 [升级到 1.1](upgrade-to-1.1.md)（1.0.2 → 1.1）；
从 0.9/0.10 起步则先看 [升级到 1.0](upgrade-to-1.0.md)，更早的版本按其历史升级页逐级迁移。
从其它框架过来？看 [从 ASP.NET Core 迁移](from-aspnet-core.md) 或
[从 Axum / Actix 迁移](from-axum-actix.md)；遇到不熟悉的术语时查
[概念对照表](concept-mapping.md)。

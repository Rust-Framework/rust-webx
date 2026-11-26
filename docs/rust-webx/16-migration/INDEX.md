# 第十六章 迁移指南

本章面向**升级 rust-webx 或从其它框架迁移**的读者：升级步骤、概念对应与代码改写。
版本变更记录统一维护在 `CHANGELOG.md`；兼容性承诺与 LTS 规则见
[compatibility.md](../../design/compatibility.md)。

## 版本与升级路径（当前 2.9.3）

| 发布 | 类型 | 升级动作 |
|------|------|----------|
| 2.9.1 | **破坏性补丁**（semver 例外：Error 载体、分页/绑定/排序契约、组合根动词、兼容层删除） | 按[升级到 2.9.1](upgrade-to-2.9.1.md) 逐项改源码 |
| 2.9.2 | 加性补丁（广播投递类、共享实时状态/在线态、推送端口、会话吊销复查；不改默认行为） | 不需要改代码；新能力按需 opt-in |
| 2.9.3 | **行为变更补丁**（Redis 事件 `start_id` 默认 `"0"`、保留扇出事件 handler fail-closed、presence 身份/布局变更） | 核对 handler 幂等与 presence 升级窗口；见 [known-boundaries.md](../../design/known-boundaries.md) C4 与 [compatibility.md](../../design/compatibility.md) §3/§7 |

- 当前发布 **2.9.3**；N-2 源码兼容基线 = **2.9.1**（`testing/upgrade-smoke` 用 `=2.9.1`
  发布与当前工作区两份源码编译），下一个 minor 发布时整体前移。
- 补丁级的逐条变更与验收证据在 `CHANGELOG.md` 与
  [known-boundaries.md](../../design/known-boundaries.md)；本手册只给升级摘要与指针，
  不复制设计文档的边界细节。

## 本章小节

| 小节 | 内容 |
|------|------|
| [升级到 2.5（2.x 加性演进）](upgrade-to-2.5.md) | 2.1 → 2.5 升级步骤、可选能力清单、稳定面与弃用承诺 |
| [升级到 2.9.1（破坏性补丁）](upgrade-to-2.9.1.md) | Error 载体、分页、嵌套绑定、raw 排序、组合根动词、2.x 兼容层删除（授权/密钥/限流旧键）、`webx_ef` 删除、`*-ef` 移出发布序列与 outbox 拆分 |
| [从 ASP.NET Core 迁移](from-aspnet-core.md) | .NET 开发者迁移路径 |
| [从 Axum / Actix 迁移](from-axum-actix.md) | Rust 原生框架迁移 |
| [概念对照表](concept-mapping.md) | 完整概念映射 |

## 下一步

已经在用 2.x？从 2.9.0 及更早升级先按[升级到 2.9.1](upgrade-to-2.9.1.md) 改源码，
再按上表核对 2.9.2 / 2.9.3 的加性与行为变更；2.5 的加性演进路径保留在
[升级到 2.5](upgrade-to-2.5.md)。

从其它框架过来？看 [从 ASP.NET Core 迁移](from-aspnet-core.md) 或
[从 Axum / Actix 迁移](from-axum-actix.md)；遇到不熟悉的术语时查
[概念对照表](concept-mapping.md)。

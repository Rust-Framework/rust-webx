# 文档权威映射（Document Authority / SSOT）

> 目的：让每一个**可漂移的事实**只有一个权威落点；其它页面只写使用结论 + 指针。
> 本页不是教程，而是文档维护契约。修改权威页或新增事实域时，同批更新本页与
> `scripts/check-docs-drift.py` 的校验规则。

## 规则

1. **一个事实一个权威页**：完整表/完整清单/完整枚举只允许出现在权威页；其它页只允许
   一两句摘要 + 相对链接，不复制行级细节。
2. **能生成的绝不手写**：crate 清单、Docs `INDEX.md`/`SUMMARY.md` 等由脚本生成，CI
   校验生成结果与工作树一致。
3. **删除/改名先登记**：先在 [deprecations.md](deprecations.md) 登记 `removed` 项与替换项，
   再删代码；live 手册的旧 API 扫描由该登记驱动。
4. **版本声明只在一个地方**：当前版本、N-2、补丁政策以
   [compatibility.md](compatibility.md) 为准；其它页引用版本号时只写“当前发布 X”且不重复政策。
5. **发布门禁**：`bash scripts/check-docs.sh` 必须通过；CI 的 Lint/Docs 工作流都依赖它。

## 事实域 → 权威页

| 事实域 | 唯一权威 | 允许的其它写法 | 校验方式 |
|---|---|---|---|
| 已发布 crate 清单、包名/导入名、publish 状态 | [crate-layout.md](../rust-webx/04-architecture/crate-layout.md)（清单段自动生成） | 手册/README 只提所需 crate，不列全量清单 | `gen-crate-inventory.py --check` + `check-docs-drift.py` crate 名扫描 |
| 共享契约归属（`AppError`、`Paged*`、`Keyset*`、`OperationResult`、`IPatchable`） | [crate-layout.md](../rust-webx/04-architecture/crate-layout.md) + `rust-core` rustdoc | “定义在 rust-core，webx_core 重导出历史路径”一句 | `check-docs-drift.py` 归属模式扫描 |
| offset 分页模型、`PagedRequest`/`PagedResult` 形状 | [pagination.md](../rust-webx/05-request-pattern/pagination.md)（使用）+ [pagination-model.md](pagination-model.md)（设计理由） | 其它页只写字段摘要 + 链接 | 迁移页与 deprecations 校验 |
| keyset 分页、`KeysetRequest`/`CursorPage` | [pagination.md](../rust-webx/05-request-pattern/pagination.md) | 同上 | 同上 |
| 实时/续传/presence/Redis 边界 | [known-boundaries.md](known-boundaries.md) | 使用页保留必要行为 + 指针 | 重复权威表扫描 + 评审 |
| 联邦认证能力判定（F1–F4）、API 形状、落地轮次与"不做"清单 | [federated-auth.md](federated-auth.md) | 手册只写用法 + 指针，不复制判定表 | 评审 + 重复权威表扫描 |
| broker 投递语义（竞争/广播/at-least-once 等） | [broker-capability-matrix.md](broker-capability-matrix.md) | 使用页只写结论 + 链接 | 重复权威表扫描 |
| HTTP 错误码 / WebSocket 关闭码 | [error-codes.md](error-codes.md) / [close-codes.md](close-codes.md) | 使用页引用码值 | 重复权威表扫描 |
| 稳定指标名/标签 | [compatibility.md](compatibility.md)（稳定面）+ [metrics.md](../rust-webx/11-production/metrics.md)（用法） | 页面给示例，不复制全表 | 旧指标名扫描（`http.requests`） |
| 端口默认注册语义（哪些需显式装配） | [integrations.md](../rust-webx/13-extensibility/integrations.md) §端口表 | README/教程只写一句结论 | 评审 + 该表为唯一清单 |
| rust-dix / rust-ef 配对版本 | [compatibility.md](compatibility.md) §4 + [upstream-gaps.md](upstream-gaps.md)（历史） | 页面写“当前配对”不展开历史 | 旧版本声明扫描（v2.7.0 / rust-dix 1.0 / rust-ef 2.9.0） |
| 兼容性政策、版本号、N-2 | [compatibility.md](compatibility.md) | README/手册只写“当前发布 X” | 版本声明扫描 |
| 已删除/改名 API 与替换项 | [deprecations.md](deprecations.md) | 迁移指南按版本记录 | 旧 API 扫描（由本表解析） |
| 框架设计准则（不可违反的架构约束） | [framework-constitution.md](framework-constitution.md) | 各专题页给场景结论 | 评审 |
| IM 硬化设计历史 | [im-hardening.md](im-hardening.md)（2.9.2 历史，不再作为现状） | 现状边界一律指向 known-boundaries | 链接/评审 |
| mdBook 章节索引 | `docs/rust-webx/INDEX.json`（生成 `INDEX.md`/`SUMMARY.md`） | 手册正文不再手写全量目录 | `gen-mdbook-summary.py` + `check-docs.sh` |

## 新增/变更事实的流程

1. 先改代码或权威页（二者同批提交）；
2. 若删除/改名公开 API：在 [deprecations.md](deprecations.md) 加一行 `removed`，并在迁移指南记录；
3. 删除其它页面的重复表，改成摘要 + 权威页链接；
4. 运行 `bash scripts/check-docs.sh`（含 drift checker 与生成器校验）；
5. 发布前再跑一次全量审计，生成文件 clean、门禁全绿才允许打 tag。

## 历史说明的豁免

当前手册中仍保留少量“某 API 已于 2.9.1 删除”的教学性说明。此类**具体行**用
`<!-- docs:allow-stale-api -->` 显式标记，drift checker 只豁免该行；整页豁免不再允许，避免一个历史脚注掩盖同页新出现的旧 API。

## 当前已知限制

- 自动校验覆盖“可判定事实”（crate 名、API 名、版本声明、重复表签名）；叙述性文字
  是否准确的语义漂移仍需发布前人工抽检，不能由脚本保证。
- 本项目 2.x 不新增 `#[deprecated]`；删除即登记并走迁移指南（见 compatibility §5）。
- `crate-inventory.inc` 是生成文件：任何手改都会被 CI 的 `--check` 判为失败。

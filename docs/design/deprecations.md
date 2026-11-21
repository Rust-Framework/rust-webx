# 弃用登记（Deprecations）

> 政策见 [compatibility.md](compatibility.md) §7。2.x 线**不新增 `#[deprecated]`**
> （避免下游 `-D warnings` 构建失败）；本表从 3.0 起成为强制登记处。CI 将检查每个
> `#[deprecated]` 都有对应条目（3.0 起）。

## 登记规则

- 一项一行：`项 | 引入版本 | 计划移除 | 替代项 | 迁移指南 | 负责人 | 状态`；
- 状态取值：`planned`（已决定但未标记）/ `deprecated`（已标记）/ `removed`；
- 窗口 ≥ 2 个 minor：N 标记 → 最早 N+2 移除；
- 移除前必须：迁移指南 + CHANGELOG + 专项验收测试全部就绪。

## 当前 2.x 线（无弃用）

| 项 | 引入 | 计划移除 | 替代项 | 迁移指南 | 状态 |
|---|---|---|---|---|---|
| （无） | — | — | — | — | — |

### 2.2 撤回的历史弃用

`init_jwt_secret` / `jwt_secret` 曾在 2.2 开发期被误标 `#[deprecated(since = "2.5.0")]`，
已撤回（见 compatibility.md §4）；迁移说明保留在 rustdoc 注释中，3.0 再随大版本窗口
重新标记并入表。

## 3.0 计划窗口（草稿）

| 项 | 引入 | 计划移除 | 替代项 | 状态 |
|---|---|---|---|---|
| 配置 section 的 `#[non_exhaustive]`（字段字面量构造） | 3.0.0 | —（一次性破坏） | `Default` + 配置绑定 | `planned` |
| `Pipeline { stages }` 结构体字面量构造 | 3.0.0 | — | `Pipeline::new(stages)` | `planned` |
| 1.x 兼容垫片（若仍存在） | 3.0.0 | 3.2 | 见迁移指南 | `planned` |

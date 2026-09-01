# Docbit 项目 REF 框架审查报告

> **最新状态（2026-09-01）** — 架构迭代 Phase 0–4 已完成；框架版本已同步 rust-dix 0.7 / rust-ef 1.8.2 / rust-webx 0.3.6。下文「历史审查（2026-06-29）」保留作对照。

## 当前架构（2026-09-01）

| 能力 | 状态 | 实现位置 |
|------|------|----------|
| Per-request `DbContext`（owned，无 Mutex） | ✅ | `#[inject(owned)]` + `host::register_db_context` |
| 全局软删除查询过滤器 | ✅ | `docbit/domain/src/filters.rs` + `prepare_context` |
| Handler 持久化规范 | ✅ | `set` → `add`/`update` → 一次 `save_changes` |
| 写后 reload（含导航） | ✅ | `handlers` 内 `ef_require_by_id!` |
| 审计字段（RequestContext） | ✅ | `docbit/domain/src/audit.rs` + mapper（按需实体） |
| ORM 错误映射 | ✅ | `handlers/src/db.rs` `EfResultExt` / `save_changes` |
| HTTP + Mediator 管道行为链 | ✅ | `build_pipeline_chain` + `IPipelineBehavior` |
| 路由诊断 | ✅ | `format_route_diagnostics` / `docbit-host --doctor` |
| E2E 集成测试 | ✅ | `docbit/host/tests/e2e_test.rs`（10/10） |

### 框架版本

- rust-ef **1.8.3**、rust-dix **0.7**、rust-webx **0.3.6**（框架无 ORM 依赖）
- docbit 4-crate：`contracts` / `domain` / `handlers` / `host`（无独立 persistence 层）

### 仍有意保留的模式

- **显式 `update()`**：rust-ef `find()` 返回 detached 实体，软删除/更新需 `set.update(entity)`（R6，框架行为）
- **Comment 创建**：UUID 主键，insert 前分配 id，无 last-insert-id 回查竞态（R4a 已消除）
- **DbInitService**：启动期一次性 `configure_for_init`，合法 captive dependency

### 验证

```text
cargo test -p docbit-host --test e2e_test  →  10 passed
cargo run -p docbit-host -- --doctor       →  49 routes, all [ok]
```

---

## 历史审查（2026-06-29）

> 审查依据：lref-skill（Rust Entity Framework / EFCore 风格 ORM 领域技能指导）  
> 审查范围：`docbit` 子工作区（contracts/domain/handlers/host）+ 本地 `rust-webx`  
> 当时框架版本：rust-ef 1.2.0、rust-dicore 0.4.1、rust-webx 0.1.0

### 一、审查结论概览（历史）

| 红旗 | 优先级 | 定性 | 修复状态 |
|------|--------|------|----------|
| R1 | P0 | 全局单例 `Arc<Mutex<DbContext>>` | ✅ 已修复 → owned per-request |
| R4a | P0 | Comment 回查并发竞态 | ✅ UUID 主键 + insert 前赋 id |
| R4b | P1 | Resource 按 value 回查竞态 | ✅ 合并 scope；长期建议 value unique |
| R3a | P1 | 重复手动 `!is_deleted` | ✅ 全局 `has_query_filter` |
| R3b | P1 | 遗漏 `is_deleted` 过滤 | ✅ 已修复 |
| R5 | P2 | 字符串列名 API | ✅ 已修复 |
| R6 | P2 | 显式 `update()` 而非 `detect_changes()` | ⚠️ 框架行为，模式正确 |
| R2 | P3 | 链式内联 `linq!` | ⏸️ 可读性偏好 |

### 二、历史详细记录

<details>
<summary>展开 2026-06-29 原始审查正文</summary>

#### R1（P0）：全局单例 `Arc<Mutex<DbContext>>` — 已修复

Per-request DI scope + `#[inject(owned)] DbContext`，`add_ef_dbcontext` 在每次创建实例时调用 `prepare_context`。

#### R4a / R4b：回查竞态 — 已缓解

UUID 字符串主键在 insert 前由 `new_id()` 分配，save 后可用 `ef_require_by_id!` 精确 reload。

#### R3a：全局查询过滤器 — 已修复

`docbit/domain/src/filters.rs` 为 Blog、Category、Comment、Exhibition、Resource、Role、User 注册 `has_query_filter(|x| !x.is_deleted)`。

#### R5：字符串列名 — 已修复

Tracking 列表改用 `linq!(...; order_by t.visited_at desc)`。

#### R6：显式 update — 保留

`find()` 返回 detached 实体；显式 `update()` 是当前正确模式。

</details>

### 三、历史框架缺口（多数已关闭）

| 缺口 | 2026-07-08 状态 |
|------|-----------------|
| `add_dbcontext` 不可 CRUD | ✅ `add_ef_dbcontext` + owned `DbContext` |
| 无 `has_query_filter` | ✅ rust-ef 1.5 + domain filters |
| HTTP 无 per-request scope | ✅ endpoint dispatch + owned handler |
| save_changes 不回填 id | ✅ UUID 主键，无需回填 |
| 无 RequestContext 审计 | ✅ Phase 4 mapper 集成 |

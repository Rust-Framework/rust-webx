# Docbit 项目 REF 框架审查报告

> 审查日期：2026-06-29
> 审查依据：lref-skill（Rust Entity Framework / EFCore 风格 ORM 领域技能指导）
> 审查范围：`d:\GitCode\RF\rust-webapp\docbit` 子工作区（contracts/domain/handlers/host 4-crate）+ 本地 `rust-webapp` 框架相关改动
> 框架版本：rust-ef 1.2.0、rust-dicore 0.4.1、rust-webapp 0.1.0（本地）

---

## 一、审查结论概览

| 红旗 | 优先级 | 定性 | 修复状态 |
|------|--------|------|----------|
| R1 | P0 | 全局单例 `Arc<Mutex<DbContext>>` 反模式 | ✅ 已修复（per-request Scoped） |
| R4a | P0 | Comment 回查并发竞态 | ⚠️ 部分修复（框架缺口限制） |
| R4b | P1 | Resource 按 value 回查竞态 | ⚠️ 部分修复（框架缺口限制） |
| R3a | P1 | 重复手动 `!x.is_deleted` 谓词 | ⚠️ 框架缺口（无全局查询过滤器） |
| R3b | P1 | 遗漏 `is_deleted` 过滤 | ✅ 已修复（3 处补齐） |
| R5 | P2 | 字符串列名 API | ✅ 已修复 |
| R6 | P2 | 显式 `update()` 而非 `detect_changes()` | ❌ 框架缺口（不可修） |
| R2 | P3 | 链式内联 `linq!` 未分离绑定 | ⏸️ 不在本次修复范围 |

---

## 二、详细审查与修复记录

### R1（P0）：全局单例 `Arc<Mutex<DbContext>>` 反模式 — 已修复

**定性**：违反 lref-skill「DbContext 生命周期」要求。EFCore 最佳实践是 per-request Scoped（每请求一个 DbContext，unit-of-work 隔离）。全局单例导致：
1. 跨请求变更追踪污染（ChangeTracker 在请求间不清理）
2. 虚假并发争用（所有请求串行化在同一 Mutex 上）
3. 性能退化（无并发 DB 访问能力）

**修复方案**（执行 Risk #1 fallback）：
1. **框架层**（`crates/macros/src/endpoint.rs:225-236`）：dispatch 函数改为每请求创建 DI Scope：
   ```rust
   let provider = ::rust_webapp::global_provider();
   let scope = provider.create_scope();
   let handler: ::std::sync::Arc<dyn ::rust_webapp::IRequestHandler<#ty, #rsp_type>> =
       scope.get_optional().ok_or_else(|| ...)?;
   ```
2. **注册层**（`docbit/host/src/main.rs:60-68`）：`svc.singleton::<Mutex<DbContext>>` → `svc.scoped::<Mutex<DbContext>>`
3. **Handler 层**（11 个 handler 文件）：`#[inject]` → `#[inject(scoped)]`（共 49 处 `impl IRequestHandler`）

**保留为 Singleton 的 3 处**（合法）：
- `RoleAuthorizer`（`impl IDynamicAuthorizer`）— 无状态鉴权器
- `DocService`（`impl IDocumentService`）— 无状态文档服务
- `DbInitService`（`impl IHostedService`）— 一次性启动代码（captive dependency 但启动期无并发，可容忍）

**未走 `add_dbcontext` API 的原因**（框架缺口）：rust-ef 1.2.0 的 `add_dbcontext` 注册 `Arc<dyn IDbContext>`，但：
- `IDbContext` trait 仅有 `provider`/`change_tracker`/`save_changes(&mut self)`/`begin_transaction`，**无 `set`/`query` 方法**，无法做 CRUD
- `save_changes(&mut self)` 需 `&mut`，但 scope cache 持强 `Arc`，`Arc::get_mut` 失败
- 故 `add_dbcontext` 在 1.2.0 不可直接用于 CRUD handler；保留 `Mutex<DbContext>` + Scoped 注册是当前可工作的最小修复

---

### R4a（P0）：Comment 回查并发竞态 — 部分修复

**位置**：`docbit/handlers/src/comment.rs` `CreateCommentHandler`

**定性**：`save_changes` 后按 `(blog_id, user_id)` 非唯一字段回查 + `max_by_key(c.id)` 取最新。跨请求并发插入同 `(blog_id, user_id)` 时，`max(id)` 可能取到他人记录。

**修复**：
- 合并 insert + 回查为单 lock scope（消除不必要的 re-lock）
- 保留 `max_by_key(c.id)` 近似回查

**框架缺口（不可修部分）**：rust-ef 1.2.0 的 `save_changes` **不回填自增 id**：
- `ChangeExecutor::execute_inserts` 的 `on_key_backfill` 回调以 `0` 占位（非真实 last-insert-id）
- `save_one_set` 传入空闭包 `|_, _| {}`
- `IAsyncConnection` trait 无 `last_insert_rowid()` 方法
- 故无法用 `find(inserted_id)` 精确回查

**剩余风险**：跨请求 DB 层竞态仍存在（per-request Scoped DbContext 消除了请求内竞态，但 DB 层并发不可控）。低严重度（用户可能瞬间看到错误评论，无数据损坏）。

**框架改进建议**：
1. `IAsyncConnection` 增加 `last_insert_rowid() -> i64`
2. `execute_inserts` 真实回填 id 到 `on_key_backfill` 回调
3. `save_changes` 后 tracked entity 的自增 PK 字段被填充
4. 或：`DbContext` 提供 `find_after_insert<T>()` API

---

### R4b（P1）：Resource 按 value 回查竞态 — 部分修复

**位置**：`docbit/handlers/src/rbac.rs:253-278` `CreateResourceHandler`

**定性**：`Resource.value` 无 `#[unique]`，按 value 回查在并发下可能取到他人记录。

**修复**：合并 insert + 回查为单 lock scope。

**框架缺口**：同 R4a（无法 `find(id)`）。

**长期修复建议**：
1. 框架暴露 last-insert-id（同 R4a）
2. 为 `Resource.value` 加 `#[unique]`（需先验证历史数据无重复，否则 `ensure_created` 可能失败）

---

### R3a（P1）：重复手动 `!x.is_deleted` 谓词 — 框架缺口

**定性**：违反 lref-skill「全局查询过滤器」要求。EFCore 的 `HasQueryFilter` 在 EntityTypeBuilder 配置一次，所有查询自动应用。rust-ef 1.2.0 的 `EntityTypeBuilder` 仅暴露 `to_table/property_named/has_key_named/has_keys/has_data`，**无 `has_query_filter`**。

**当前状态**：20+ 处 `linq!` 手动添加 `!x.is_deleted` 谓词。功能正确但易遗漏（见 R3b）。

**框架改进建议**：`EntityTypeBuilder` 增加 `has_query_filter(|x| !x.is_deleted)` API，`QueryBuilder` 自动应用。

---

### R3b（P1）：遗漏 `is_deleted` 过滤 — 已修复

**定性**：软删除数据泄漏风险。

**修复清单**（3 处补齐）：
| 文件 | 位置 | 修复 |
|------|------|------|
| `docbit/handlers/src/rbac.rs:73` | `CreateRoleHandler` 按 name 回查 | `r.name == q` → `r.name == q && !r.is_deleted` |
| `docbit/handlers/src/category.rs:124` | `CreateCategoryHandler` 按 slug 回查 | `c.slug == q` → `c.slug == q && !c.is_deleted` |
| `docbit/host/src/startup.rs:117` | admin 用户按 email 回查 | `u.email == ADMIN_EMAIL` → `u.email == ADMIN_EMAIL && !u.is_deleted` |

**跳过**：`rbac.rs:367` `ListAuthorizesHandler` — `Authorize` 实体无 `is_deleted` 字段（联结表仅 `created_at`）。

---

### R5（P2）：字符串列名 API — 已修复

**位置**：`docbit/handlers/src/tracking.rs:73-77`

**定性**：违反 lref-skill「类型安全查询」要求。`order_by_desc_column("visited_at")` 用字符串列名，无编译期检查，列名拼错运行时才报错。

**修复**：
```rust
// 改前
ctx.set::<Tracking>().query().order_by_desc_column("visited_at").to_list()

// 改后
linq!(ctx.set::<Tracking>(); order_by t.visited_at desc).to_list()
```

---

### R6（P2）：显式 `update()` 而非 `detect_changes()` — 框架缺口（不可修）

**定性**：违反 lref-skill「变更追踪」要求。EFCore 模式：`Find(id)` 返回 tracked 实体 → 修改字段 → `SaveChanges()` 自动 `DetectChanges()` 标记为 Modified。当前代码用显式 `set.update(entity)` 重新附加。

**框架缺口**：rust-ef 1.2.0 的 `find()` 经 `filter_column().first_or_default()` 返回 **detached 实体**（不附加到 ChangeTracker）。故 EFCore 的 `find → modify → detect_changes` 模式不成立。

**当前模式**（正确）：
```rust
let mut role = ctx.set::<Role>().query().find(id).await?;  // detached
role.is_deleted = true;
ctx.set::<Role>().update(role);  // 重新附加为 Modified
ctx.save_changes().await?;
```

**框架改进建议**：
1. `DbContext::find<T>(id)` 将实体附加到 ChangeTracker（state=Unchanged，存原始快照）
2. 暴露 `tracked_entries_mut()` 的安全访问入口
3. `save_changes()` 自动调用 `detect_changes()`（1.2.0 已有此行为，但因 `find` 不 attach 而无效）

**保留**：约 15 处显式 `update()` 不变。

---

### R2（P3）：链式内联 `linq!` — 不在本次范围

**定性**：~30 处 `linq!` 调用内联在表达式位置，未用 `let expr = linq!(...); set.filter(expr)` 分离绑定。可读性偏好，非功能缺陷。本次不修复。

---

## 三、框架改动记录

### `crates/macros/src/endpoint.rs`（rust-webapp 框架）

**改动**：`generate_dispatch_fn` 第 225-236 行，handler 解析从根 provider 改为 per-request scope。

**影响**：全工作区所有 `#[get]/#[post]/...` 路由的 dispatch 均走 scope 路径。Singleton handler 从 scope 解析时，rust-dicore 0.4.1 的 root-resolution-degrades-to-transient 行为保证 Singleton 仍每次新建（向后兼容）。

**验证**：`cargo build --workspace` 通过（45.92s → 后续增量 6-8s）。

### `Cargo.toml`（workspace 依赖）

```toml
rust-dicore = "0.4.1"        # 解决 create_scope 降级问题
rust-dicore-macros = "0.4.1"
rust-ef = "1.2"              # 大幅简化 REF 调用
rust-ef-sqlite = "1.2"
rust-ef-mysql = "1.2"
```

---

## 四、框架缺口汇总（待 rust-ef / rust-webapp 补齐）

| 缺口 | 影响 | 阻塞的红旗 |
|------|------|-----------|
| `IDbContext` trait 无 `set`/`query` 方法 | `add_dbcontext` 不可用于 CRUD | R1 完整修复 |
| `save_changes` 不回填自增 id | 无法 `find(inserted_id)` 精确回查 | R4a/R4b 完整修复 |
| `IAsyncConnection` 无 `last_insert_rowid()` | 同上 | R4a/R4b |
| `find()` 返回 detached 实体 | `detect_changes` 模式不成立 | R6 |
| `EntityTypeBuilder` 无 `has_query_filter` | 无全局软删除过滤器 | R3a |
| rust-webapp 0.1.0 HTTP 管道无 per-request scope | **已修复**（本次） | R1 |

---

## 五、验证结果

### 编译验证
```
cargo build --workspace  →  Finished `dev` profile in 6-8s（仅 snake_case 风格警告）
```

### grep 验证
- `Arc<Mutex<DbContext>>` 在 `docbit/` 下：✅ 仍存在（Risk #1 fallback 保留，但改为 Scoped 注册）
- `self.ctx.lock().await` 在 `docbit/` 下：✅ 仍存在（fallback 模式）
- `order_by_desc_column("` 在 `docbit/handlers/` 下：✅ 零匹配（R5 已修）
- `#[inject]`（无参数）在 `docbit/handlers/` 下：✅ 仅 2 处（authorizer.rs、doc_service.rs，合法 Singleton）
- `#[inject(scoped)]` 在 `docbit/handlers/` 下：✅ 49 处（所有 IRequestHandler）

### 冒烟测试
**未执行**（本次聚焦编译 + 静态审查）。建议手动验证：
- POST 注册 / POST 登录 / GET me
- POST blog / GET list / PUT blog / DELETE blog（软删除）
- POST comment（验证回查返回正确记录）
- RBAC：创建 Role/Resource/Authorize、列表
- Tracking 列表排序正常

---

## 六、剩余风险

1. **R4a/R4b 跨请求竞态**：rust-ef 1.2.0 不暴露 last-insert-id，`max_by_key`/`first_or_default` 回查在并发下可能取到他人记录。低严重度（无数据损坏），待框架补齐。
2. **DbInitService captive dependency**：Singleton `IHostedService` 持有 Scoped `Mutex<DbContext>`。启动期无并发，可容忍；但严格意义上违反 DI 规则。长期修复：DbInitService 在 `start()` 内通过 `IServiceProvider` 解析 DbContext，而非构造器注入。
3. **R3a 全局查询过滤器缺失**：20+ 处手动 `!x.is_deleted` 易遗漏（R3b 已证明）。待框架补齐 `has_query_filter`。
4. **R6 变更追踪不完整**：`find` 不 attach，`detect_changes` 无效。当前显式 `update()` 模式正确但冗余。

---

## 七、修复文件清单

| 文件 | 改动 |
|------|------|
| `Cargo.toml` | rust-ef 1.2 + rust-dicore 0.4.1 |
| `crates/macros/src/endpoint.rs` | per-request scope（dispatch） |
| `docbit/host/src/main.rs` | singleton → scoped 注册 |
| `docbit/host/src/startup.rs` | 补 `!u.is_deleted` |
| `docbit/handlers/src/auth.rs` | 5 处 `#[inject(scoped)]` |
| `docbit/handlers/src/blog.rs` | 7 处 `#[inject(scoped)]` |
| `docbit/handlers/src/cache.rs` | 1 处 `#[inject(scoped)]` |
| `docbit/handlers/src/category.rs` | 4 处 `#[inject(scoped)]` + 补 `!c.is_deleted` |
| `docbit/handlers/src/comment.rs` | 3 处 `#[inject(scoped)]` + 合并 lock scope |
| `docbit/handlers/src/docs.rs` | 3 处 `#[inject(scoped)]` |
| `docbit/handlers/src/exhibition.rs` | 4 处 `#[inject(scoped)]` |
| `docbit/handlers/src/rbac.rs` | 13 处 `#[inject(scoped)]` + 补 `!r.is_deleted` + 合并 lock scope |
| `docbit/handlers/src/site.rs` | 1 处 `#[inject(scoped)]` |
| `docbit/handlers/src/tracking.rs` | 2 处 `#[inject(scoped)]` + R5 linq! order_by |
| `docbit/handlers/src/user.rs` | 6 处 `#[inject(scoped)]` |

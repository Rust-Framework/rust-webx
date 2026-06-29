# Docbit 项目 REF 框架审查与修复计划

## 一、任务概述

依据 **lref-skill** 领域技能指导，审查 `d:\GitCode\RF\rust-webapp\docbit` 子工作区（4-crate：contracts/domain/handlers/host）的 REF 框架使用，产出**审查报告 + 直接修复 P0/P1 问题**。

**用户决策（已确认）**：
- 受框架能力限制的红旗一律**按 lref-skill 严格定性**为违规/反模式，并推动框架补齐。
- **必须升级 rust-ef 到 1.2.0**（用户强调 1.2.0 大幅简化了 REF 调用）。
- **rust-webapp 必须支持 per-request scope**（架构设计最佳实践要求）。
- R1（消除 `Arc<Mutex<DbContext>>` 单例）**完整修复**：同时改本地框架 + docbit。

---

## 二、当前状态分析（审查发现）

### 项目结构
- `docbit/contracts`：DTO/Request/Service trait，不依赖 rust-ef
- `docbit/domain`：11 个实体（`#[derive(EntityType)]` + `#[table]` + `#[primary_key]` + `#[auto_increment]`），含 `From<Entity> for Model` 转换、`ToModel/ToEntity/ApplyTo` mapper trait、种子数据
- `docbit/handlers`：14 个 handler 模块、41+ `IRequestHandler`，直接使用 `DbContext`
- `docbit/host`：组合根（`main.rs` + `DbInitService`/`IHostedService`）

### 红旗清单（按 lref-skill 定性）

| ID | 优先级 | 问题 | 位置 |
|----|--------|------|------|
| R1 | **P0** | `Arc<Mutex<DbContext>>` 全局 Singleton 反模式（46 处注入点）—— 全应用 DB 访问序列化、跟踪污染 | `docbit/host/src/main.rs:60-64` + 9 个 handler 文件 + `startup.rs` |
| R4a | **P0** | `save_changes` 后按非唯一字段回查 + 并发竞态 | `docbit/handlers/src/comment.rs:82-97`（按 `blog_id+user_id` + `max_by_key` 取最新） |
| R4b | **P1** | 按 `Resource.value`（无 `#[unique]`）回查 | `docbit/handlers/src/rbac.rs:261-275` |
| R3a | **P1** | 重复手动 `!x.is_deleted` 谓词 20+ 处，无全局查询过滤器 | 全 handler 层 |
| R3b | **P1** | 遗漏 `is_deleted` 过滤（软删除数据泄漏风险） | `rbac.rs:73,367`、`category.rs:124`、`startup.rs:117` |
| R5 | **P2** | 字符串列名 API（无编译期检查） | `docbit/handlers/src/tracking.rs:75` |
| R6 | **P2** | 显式 `update()` 而非 `detect_changes()`（约 15 处） | 全 handler update/soft-delete 流程 |
| R2 | **P3** | 链式内联 `linq!`，未用 `let expr = linq!(...); set.filter(expr)` 分离绑定 | ~30 处 `linq!` 调用 |

### 框架缺口（探索确认）

1. **本地 rust-webapp 0.1.0 HTTP 管道不为每请求创建 DI Scope**：
   - `crates/macros/src/endpoint.rs:228` `generate_dispatch_fn` 硬编码 `let provider = ::rust_webapp::global_provider();` 然后 `provider.get_optional::<dyn IRequestHandler<...>>()`。
   - 从根 `ServiceProvider` 解析 Scoped 服务退化为每次新建（transient），违反 EFCore unit-of-work 语义。
2. **handler 默认 Singleton**：docbit 所有 handler 用 `#[inject]`（无参数），rust-dicore-macros 默认 `Singleton`。当前是 captive dependency（Singleton handler 持有 Singleton `Mutex<DbContext>`）。
3. **`add_dbcontext` 不在本地 `crates/core/src/di/ext.rs`**：但 rust-ef 1.2.0 的 `rust_ef::di::DbContextServiceCollectionExt` 扩展 trait 可直接 `use` 引入，无需改本地 ext.rs。
4. **rust-dicore 0.3.3 Scoped 基础设施完备**：`ServiceLifetime::Scoped`、`Scope`、`ServiceProvider::create_scope()`、`Scope: IServiceResolver` 均就位，无缺口。

### API 张力（需在 Phase 1 验证）
`IDbContext::save_changes(&mut self)` 需 `&mut`，而 `add_dbcontext` 解析为 `Arc<dyn IDbContext>`（只给 `&`）。1.2.0 docs 明确宣称 "Handlers simply declare `ctx: Arc<dyn IDbContext>` — no manual `create_scope()` needed"，故 1.2.0 内部应有自洽的访问模式（可能 `DbContext` 内部含 `Mutex`，或 scope cache 不持强引用使 `Arc::get_mut` 可行）。**升级后第一步必须读 1.2.0 源码确认**。

---

## 三、假设与决策

1. **rust-ef 升级到 1.2.0**：`Cargo.toml:46-48` 三个依赖 `1.1` → `1.2`。
2. **R1 完整修复**：改 `crates/macros/src/endpoint.rs`（per-request scope）+ docbit 注册/handler 字段/生命周期。
3. **handler 生命周期改 Scoped**：`#[inject]` → `#[inject(scoped)]`，配合 per-request scope。
4. **`save_changes(&mut self)` 访问模式**：Phase 1 读 1.2.0 源码确认；若需 `&mut`，则框架改动需保证 scope 内可获 `&mut`（如 `DbContext` 内部 `Mutex` 或 scope 解析返回可独占实例）。
5. **R6 `detect_changes` 可达性**：通过 `IDbContext::change_tracker_mut(&mut self) -> &mut ChangeTracker` 调用 `ChangeTracker::detect_changes()`；若 `&mut` 不可得，则保留显式 `update()` 并在报告中标注。
6. **R3 全局查询过滤器**：1.2.0 `EntityTypeBuilder` 仅暴露 `to_table/property_named/has_key_named/has_keys/has_data`，**未见 `has_query_filter`**。故 R3 长期方案（全局过滤器）在报告中标注为"框架待补齐"，本次直接修复聚焦补遗漏的 `is_deleted` 谓词。
7. **审查报告输出位置**：`d:\GitCode\RF\rust-webapp\docbit\REVIEW.md`。
8. **不动 docbit 之外的其他示例**（`examples/blog`、`examples/soft_delete`）：但 `endpoint.rs` 框架改动会影响全工作区，需 `cargo build --workspace` 验证。

---

## 四、修复方案（分阶段执行）

### Phase 0：升级 rust-ef 到 1.2.0
- **文件**：`d:\GitCode\RF\rust-webapp\Cargo.toml` 第 46-48 行
- **改动**：
  ```toml
  rust-ef = "1.2"
  rust-ef-sqlite = "1.2"
  rust-ef-mysql = "1.2"
  ```
- **验证**：`cargo update -p rust-ef -p rust-ef-sqlite -p rust-ef-mysql` → `cargo build --workspace`（预期 1.2.0 API 简化可能引发编译错误，记录具体错误供后续 Phase 参考）

### Phase 1：确认 1.2.0 API 访问模式（只读调研）
- 读 `~/.cargo/registry/src/.../rust-ef-1.2.0/src/db_context.rs` 与 `di.rs`：
  - 确认 `add_dbcontext` 在 1.2.0 的签名（是否去掉 `::<DbContext>` 类型参数）
  - 确认 `save_changes(&mut self)` 在 `Arc<dyn IDbContext>` 上的调用路径（`Arc::get_mut` 可行？`DbContext` 内部 `Mutex`？还是 trait 提供其他入口？）
  - 确认 `ChangeTracker::detect_changes()` 通过 `change_tracker_mut()` 的可达性
- **产出**：在计划执行记录中写明访问模式结论，指导 Phase 2-4 的具体写法

### Phase 2：框架改造 — rust-webapp per-request scope
- **文件**：`d:\GitCode\RF\rust-webapp\crates\macros\src\endpoint.rs` 第 215-251 行 `generate_dispatch_fn`
- **改动**：第 228 行附近
  ```rust
  // 改前
  let provider = ::rust_webapp::global_provider();
  let handler: ::std::sync::Arc<dyn ::rust_webapp::IRequestHandler<#ty, #rsp_type>> =
      provider.get_optional().ok_or_else(|| ...)?;

  // 改后
  let provider = ::rust_webapp::global_provider();
  let scope = provider.create_scope();
  let handler: ::std::sync::Arc<dyn ::rust_webapp::IRequestHandler<#ty, #rsp_type>> =
      scope.get_optional().ok_or_else(|| ...)?;
  ```
- **评估**：`crates/macros/src/handler.rs:48-55` `HandlerCache` 路径（启动时一次性构造并缓存）—— docbit 不用此路径（用 `#[inject]` 而非 `#[handler]`），但若 `HandlerCache` 缓存 Singleton handler 无 Scoped 依赖则不受影响；需确认不破坏。
- **验证**：`cargo build -p rust-webapp-macros` → `cargo build --workspace`

### Phase 3：R1 修复 — docbit DI 注册
- **文件**：`d:\GitCode\RF\rust-webapp\docbit\host\src\main.rs` 第 44-65 行 `register_db_context`
- **改动**：
  ```rust
  use rust_ef::di::DbContextServiceCollectionExt as _;
  use rust_ef::db_context::IDbContext;

  fn register_db_context(svc: &mut ServiceCollection) {
      svc.add_dbcontext(|options| {
          // 按 appsettings 配置选择 sqlite / mysql
          options.use_sqlite("data source=...");  // 或 use_mysql(...)
      });
  }
  ```
  - 移除 `singleton::<Mutex<DbContext>>` 注册与 `DbContext::from_options` 手动构造
  - 移除 `use tokio::sync::Mutex;` 等不再需要的导入
- **文件**：`d:\GitCode\RF\rust-webapp\docbit\host\src\startup.rs` 第 28-36 行 `DbInitService`
  - 字段 `ctx: Arc<Mutex<DbContext>>` → `ctx: Arc<dyn IDbContext>`
  - `#[inject]` → `#[inject(scoped)]`
  - 业务方法内 `let mut ctx = self.ctx.lock().await;` → 直接用 `self.ctx`（按 Phase 1 确认的访问模式调 `save_changes`）

### Phase 4：R1 修复 — 所有 handler 字段类型与业务代码
- **文件清单**（9 个 handler + startup）：
  - `docbit/handlers/src/user.rs`（6 处）
  - `docbit/handlers/src/blog.rs`（7 处）
  - `docbit/handlers/src/auth.rs`（5 处）
  - `docbit/handlers/src/category.rs`（4 处）
  - `docbit/handlers/src/comment.rs`（3 处）
  - `docbit/handlers/src/exhibition.rs`（4 处）
  - `docbit/handlers/src/rbac.rs`（13 处）
  - `docbit/handlers/src/tracking.rs`（2 处）
  - `docbit/handlers/src/docs.rs`（若注入 DbContext）
  - `docbit/handlers/src/doc_service.rs`（`DocService` 字段）
- **统一改动**：
  1. 结构体字段 `ctx: Arc<Mutex<DbContext>>` → `ctx: Arc<dyn IDbContext>`
  2. `#[inject]` → `#[inject(scoped)]`
  3. 导入 `use rust_ef::db_context::IDbContext;`，移除 `use tokio::sync::Mutex;` / `use rust_ef::db_context::DbContext;`
  4. 业务代码：`let mut ctx = self.ctx.lock().await;` → 去锁直接用 `self.ctx`（`&self.ctx` 或按 Phase 1 模式获 `&mut`）；`ctx.set::<T>()` / `ctx.save_changes()` 调用按确认的 `&mut` 访问模式调整
- **验证**：grep 确认无 `Arc<Mutex<DbContext>>` 残留、无 `self.ctx.lock().await` 残留

### Phase 5：R4 修复 — 并发竞态 + 非唯一回查
- **文件**：`docbit/handlers/src/comment.rs` 第 82-97 行 `CreateCommentHandler`
  - **改前**：`save_changes` 后按 `blog_id == q && user_id == q` 回查 + `max_by_key(|c| c.id)` 取最新（并发竞态）
  - **改后**：`save_changes` 后实体自增 id 已自动填充，直接 `ctx.set::<Comment>().query().find(inserted_id).await?` 回查
- **文件**：`docbit/handlers/src/rbac.rs` 第 261-275 行 `CreateResourceHandler`
  - **改前**：按 `value == q` 回查（`Resource.value` 无 `#[unique]`）
  - **改后**：`save_changes` 后用填充的 id 直接 `find(id)` 回查
  - **可选**：`docbit/domain/src/entities/resource.rs` 为 `value` 加 `#[unique]`（业务上 value 应唯一），需确认无历史重复数据
- **验证**：grep 确认无 `max_by_key` 回查模式残留

### Phase 6：R3 修复 — 补遗漏的 is_deleted 过滤
- **文件 + 位置**：
  - `docbit/handlers/src/rbac.rs:73` Role by `name` 回查 → 加 `!r.is_deleted`
  - `docbit/handlers/src/rbac.rs:367` `ListAuthorizesHandler` `to_list()` → 加 `!a.is_deleted`（若 Authorize 有 is_deleted 字段；若无则跳过）
  - `docbit/handlers/src/category.rs:124` Category by `slug` 回查 → 加 `!c.is_deleted`
  - `docbit/host/src/startup.rs:117` admin 用户回查 → 加 `!u.is_deleted`
  - `docbit/handlers/src/rbac.rs:270` / `comment.rs:89`：Phase 5 改为 `find(id)` 后此项自动消除
- **验证**：逐处核对 `linq!` 谓词含 `!x.is_deleted`（对有 is_deleted 字段的实体）

### Phase 7：R5 修复 — 字符串列名
- **文件**：`docbit/handlers/src/tracking.rs` 第 75 行
  - **改前**：`ctx.set::<Tracking>().query().order_by_desc_column("visited_at").to_list()`
  - **改后**：`linq!(ctx.set::<Tracking>(), |t: Tracking| t.user_id == q; order_by t.visited_at desc).to_list()`（或无过滤的等价 `linq!` 形式，按上下文）
- **验证**：grep 确认无 `_column("` 字符串列名 API 残留

### Phase 8：R6 修复 — detect_changes 替代显式 update
- **前提**：Phase 1 确认 `change_tracker_mut().detect_changes()` 在 `Arc<dyn IDbContext>` 上可达
- **适用处**：约 15 处 `set.update(entity)` → 改为 `find` → 修改字段 → `ctx.change_tracker_mut().detect_changes()` → `save_changes`
- **文件**：`user.rs:138-164`、`blog.rs:205-237`、`category.rs:140-168`、`comment.rs:108-143`、`exhibition.rs:88-96,124-153`、`rbac.rs:88-115,122-145,285-313,319-341`、`auth.rs:288-299`、`startup.rs` admin 更新
- **若 `&mut` 不可达**：保留显式 `update()`，在报告中标注"R6 受 IDbContext trait `&mut self` 约束暂不可修，建议框架暴露 `detect_changes` 的 `&self` 入口"
- **验证**：grep `\.update\(` 数量减少；cargo build 通过

### Phase 9：生成审查报告
- **文件**：`d:\GitCode\RF\rust-webapp\docbit\REVIEW.md`
- **内容**：
  - 审查依据（lref-skill 条目对标）
  - R1-R8 完整清单（位置、定性、修复状态）
  - 框架改动记录（`crates/macros/src/endpoint.rs` per-request scope）
  - 按 skill 严格定性的框架缺口（R3 全局查询过滤器待补齐）
  - 剩余风险（R2 链式 linq 未重构、R6 部分受 trait 约束）
  - 验证结果

---

## 五、验证步骤（执行末尾）

1. `cargo build --workspace` —— 全工作区编译通过
2. `cargo test --workspace`（若有测试）—— 现有测试不回归
3. **grep 验证**：
   - `Arc<Mutex<DbContext>>` 在 `docbit/` 下零匹配
   - `self.ctx.lock().await` 在 `docbit/` 下零匹配
   - `order_by_desc_column("` 在 `docbit/handlers/` 下零匹配
   - `max_by_key.*id` 回查模式在 `docbit/handlers/` 下零匹配
4. **冒烟测试**：启动 `docbit/host`，验证关键 API：
   - POST 注册 / POST 登录 / GET me
   - POST blog / GET list / PUT blog / DELETE blog（软删除）
   - POST comment（验证回查返回正确记录）
   - RBAC：创建 Role/Resource/Authorize、列表
   - Tracking 列表排序正常
5. **endpoint scope 验证**：在 dispatch 处加临时日志确认每请求 `create_scope()` 被调用（验证后移除）

---

## 六、风险与回退

1. **`save_changes(&mut self)` × `Arc<dyn IDbContext>` 张力**：Phase 1 验证。若 1.2.0 设计不自洽，框架改动需扩展（如 `DbContext` 内部 `Mutex` 或 trait 暴露 `&self` 入口）。回退：保留 `Arc<Mutex<DbContext>>` 但改为 Scoped 注册（每请求一个 Mutex 包裹实例），消除全局单例但仍保留锁。
2. **`endpoint.rs` 改动影响全工作区**：`examples/blog`、`examples/soft_delete` 等示例若用 `#[get]/#[post]` 也会走 scope 路径。Singleton handler 从 scope 解析会退化为每次新建（行为变化）。需 `cargo build --workspace` + 各示例冒烟。
3. **handler 改 Scoped 后的 captive dependency**：若某 handler 还注入了其他 Singleton 服务，Scoped handler 持有 Singleton 合法（反向则违规）。需审查 handler 全部依赖。`DocService`（`IDocumentService`）当前注册方式需确认。
4. **R6 `detect_changes` 不可达**：保留显式 `update()`，报告标注。
5. **`Resource.value` 加 `#[unique]`**：若 DB 已有重复 value 数据，`ensure_created` 可能失败。回退：仅改回查为 `find(id)`，不加 `#[unique]`。

---

## 七、执行顺序（关键路径）

Phase 0 → Phase 1（调研，阻塞 Phase 2-4 写法）→ Phase 2（框架）→ Phase 3（docbit 注册）→ Phase 4（handler 字段）→ `cargo build` 里程碑 → Phase 5/6/7（代码层 P0/P1/P2）→ Phase 8（R6，条件性）→ Phase 9（报告）→ 验证。

**里程碑检查点**：Phase 4 完成后必须 `cargo build --workspace` 通过，再继续后续 Phase，避免错误累积。

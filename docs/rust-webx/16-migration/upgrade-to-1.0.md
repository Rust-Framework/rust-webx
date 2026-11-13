# 升级到 1.0（破坏性变更）

> 目标读者：正在使用 rust-webx 0.9 / 0.10 的项目。1.0 是 **rust-dix 0.10 语义对齐 + 删除全部
> 兼容层** 的大版本：DI 行为、Host 生命周期与扩展点 API 都有 breaking 变化。历史升级页
> （0.5 / 0.6 / 0.9 / 0.10）保留，仅用于从更早版本逐级升级。

## 1. 依赖升级

```toml
[dependencies]
rust-webx = "1.0"
rust-dix = "0.10"        # 直接依赖 rust-dix 时对齐
rust-dix-macros = "0.10"
rust-ef = "2.1"          # 使用 rust-ef 时（2.1 起依赖 rust-dix 0.10）
```

MSRV 随 rust-dix 0.10 提升到 **1.95**。

## 2. DI 语义：采用 rust-dix 0.10（会影响现有代码）

| 0.9/0.10 行为 | 1.0（rust-dix 0.10） |
|---|---|
| root provider 解析 Scoped 可成功（root-scope 缓存） | **默认严格校验**：root 解析 Scoped → `Err(ScopedFromRootProvider)`；`provider.scope()` 内正常；`get_owned`/`try_get_owned` 仍可从 root（调用方持有、不缓存） |
| 单服务解析「先注册优先」 | **后注册优先**（MEDI 语义）；框架默认注册已改用 `try_add*`，用户实现仍可覆盖 |
| `get_all` 含 keyed 注册 | 只含 **非 keyed**；keyed 用 `get_all_keyed::<T>()` |
| `get_owned` 遇 Singleton panic | 返回 `Err(OwnedNotSupported)` |
| `RdiError` 可穷举 | `#[non_exhaustive]`，新增 `ScopedFromRootProvider` / `ProviderDisposed` / `ScopeDisposed` 等 |

**迁移**：root 解析 Scoped 的代码改为显式 scope 或 `get_owned`：

```rust
// 0.9：root 解析（1.0 报 ScopedFromRootProvider）
let ctx = provider.get::<DbContext>()?;

// 1.0
let scope = provider.scope();
let ctx = scope.get::<DbContext>()?;          // 共享实例（&self）
// 或
let mut ctx = provider.get_owned::<DbContext>()?;  // 调用方持有（&mut self）

// 迁移期逃生门（不推荐长期使用）：
let host = Host::builder().lenient_scope_validation()/* ... */.build();
```

## 3. 删除 ambient DI（`dispatch_provider` / `run_in_scope`）

1.0 **删除**了 task-local DI 影子：`dispatch_provider()`、`dispatch_handler_cache()`、
`DispatchRuntime::run()` / `run_in_scope()` 不再存在。rust-dix 的显式 scope/factory 取代它们：

- **Handler**：没有 ambient 入口；用 `#[handler(inject)]` 的构造注入（`#[inject]` / `#[inject(owned)]` 字段）。
- **HTTP endpoint**：框架内部通过 `IHttpContext::dispatch_runtime()` 取 provider + handler registry，
  应用代码不感知。
- **后台任务 / hosted service**：注入 rust-dix 内建 `Arc<dyn ScopeFactory>`，显式开 scope：

```rust
#[derive(Inject)]
pub struct DbInitService {
    #[inject]
    services: Arc<dyn ScopeFactory>,
}

#[inject]
#[async_trait]
impl IHostedService for DbInitService {
    async fn start(&self) -> Result<()> {
        let scope = self.services.create_scope();
        let mut ctx: DbContext = scope.get_owned()?;
        // ... scope 释放即释放 Scoped 依赖
        Ok(())
    }
}
```

- **Mediator**：构造签名变为 `Mediator::new(provider, Arc<HandlerCache>)`；`add_mediator()` 仍可按
  DI 注入使用（Host 会把 registry 注册进容器）。
- **测试/多 Host**：直接用 `host.provider()` / `host.dispatch_runtime()`，不再需要 `run(...)` 包裹。

## 4. Pipeline 只剩 typed 版

删除 0.10 保留的 erased 兼容层：`IErasedPipelineBehavior`、`BoxedNextFn`、`BoxedPipelineFuture`、
`add_pipeline` / `add_pipeline_with`、`build_pipeline_chain` 全部移除。

```rust
// 0.10 legacy（1.0 编译失败）
impl IErasedPipelineBehavior for X { /* req: Box<dyn Any>, next: BoxedNextFn */ }

// 1.0
#[async_trait]
impl IPipelineBehavior<CreateOrderRequest, OrderView> for RequireNote {
    async fn handle(&self, req: CreateOrderRequest, next: Next<CreateOrderRequest, OrderView>)
        -> ActionResult<OrderView> { next.run(req).await }
}
svc.add_typed_pipeline::<RequireNote, CreateOrderRequest, OrderView>();
```

## 5. 其它删除与新增

- 删除 `register_handlers!`（0.2 起 deprecated）：HTTP handler 用 `#[handler]`；非 HTTP 场景用
  rust-dix `#[inject]`/`inject!` 注册。
- 删除 `build_resource_policy_from_routes()`（0.6 起 deprecated）：授权规则由端点执行，动态授权用
  `IDynamicAuthorizer`。
- 新增 `HostBuilder::try_build_async()`：async singleton（如 rust-ef `add_dbcontext_async`）用
  `async_singleton` 注册后，必须走 `try_build_async().await`（sync `try_build` 会报
  `AsyncServiceRequiresBuildAsync`）。
- 新增 `HostBuilder::lenient_scope_validation()`：仅迁移期恢复 0.7 root-scope 行为。
- Host 停机时调用 `provider.dispose()`：singleton 逆序释放、cleanup 逆序执行；dispose 后解析返回
  `ProviderDisposed`（幂等）。

## 6. 升级检查清单（grep）

```bash
# 1) 被删除/改名的 API
rg 'dispatch_provider|dispatch_handler_cache|run_in_scope|ACTIVE_RUNTIME' src/
rg 'IErasedPipelineBehavior|BoxedNextFn|BoxedPipelineFuture|add_pipeline\b|build_pipeline_chain' src/
rg 'register_handlers!|build_resource_policy_from_routes' src/

# 2) root 解析 Scoped 的调用（改为 scope 或 get_owned）
rg 'provider\.get::<.*Context|provider\.get::<.*Db' src/

# 3) 依赖未对齐
cargo tree -d | rg 'rust-dix'
# 期望：只有 rust-dix v0.10.x / rust-dix-macros v0.10.x
```

## 7. 验证建议

1. `cargo test` 全量通过（严格 scope 下的失败按第 2 节改写）；
2. 启动一次 `run()`，确认 hosted services 的 scope 释放与 `provider.dispose()` 停机路径；
3. 若有本地开发用 root 解析 Scoped 的历史代码，优先按 §2 改写而不是长期开启 lenient。

---

## English summary

rust-webx 1.0 aligns with rust-dix 0.10 and removes every compatibility shim:

- **DI semantics**: strict scope validation is the default (resolving a Scoped service from the root
  returns `ScopedFromRootProvider`; `get_owned` is the exception), last-registration-wins,
  `get_all` excludes keyed registrations (`get_all_keyed`), `RdiError` is `#[non_exhaustive]`.
  `HostBuilder::lenient_scope_validation()` is a migration-only escape hatch.
- **Ambient DI removed**: `dispatch_provider()`, `dispatch_handler_cache()`,
  `DispatchRuntime::run()/run_in_scope()` are gone. Handlers use constructor injection via
  `#[handler(inject)]`; background services inject rust-dix's `Arc<dyn ScopeFactory>` and open
  scopes explicitly; `Mediator::new(provider, handler_cache)` takes explicit handles.
- **Typed pipeline only**: `IErasedPipelineBehavior`, `add_pipeline*` and the erased bridge are
  removed; implement `IPipelineBehavior<Req, Rsp>` with `Next<Req, Rsp>`.
- **Deprecated APIs removed**: `register_handlers!`, `build_resource_policy_from_routes()`.
- **Lifecycle & async**: `Host` disposes the rust-dix provider on shutdown
  (`ProviderDisposed` afterwards); `try_build_async().await` supports async singletons such as
  rust-ef's `add_dbcontext_async`.
- Migration checklist: grep for the removed symbols above, replace root Scoped resolution with
  `provider.scope()` / `get_owned()`, and keep dependencies on rust-dix 0.10 / rust-ef 2.1.

# P0 架构债清理 — 五项一次性消除

## Context

rust-webapp 框架当前呈现"定义齐备、实现参差"的特征。三大跨层架构债正在系统性侵蚀设计意图：

1. **管道无短路 + 无注入开口**：`IMiddleware::invoke` 返回 `Result<()>` 无短路信号；`pipeline` 是 `HostBuilder::build()` 内局部变量。导致 `RateLimitMiddleware` 设置 429 后仍跑 final\_handler、5 个内置中间件（request\_id/timing/request\_tracing/security\_headers/rate\_limit）用户无法启用。
2. **IPipelineBehavior 空壳 + Mediator::send 用 root provider**：trait 已定义但 [mediator/pipeline.rs](file:///e:/GitCode/RF/rust-webapp/crates/core/src/mediator/pipeline.rs) 是空壳，`Mediator::send` 全程未串联行为链；且用 root provider 解析 scoped 服务会降级为 transient，与 HTTP 路径行为不一致。
3. **双范式断裂**：`#[controller]` 仅存 base\_path static、`#[http_get]` 等纯 pass-through、`request!` 直接 `compile_error!`。文档宣称双范式但实际仅 IRequest 可用，且 docbit/examples 零使用。

健康检查接入是同一批"伪实现"症状：`HealthCheckRegistry` 是死代码，`/health/ready` 返回硬编码 JSON。

**目标**：一次性消除五项 P0 阻塞性架构债，使框架主干真正闭环可用。**docbit 应用层零破坏**（不修改任何 handler 代码、不修改 main.rs）。

## 关键约束

* docbit 完全通过 `#[get/post/put/delete]`（contracts）+ `#[handler(inject)]`（handlers）耦合，P0 不能改这两个宏的调度 contract

* 用户偏好：微软风格"极致易用"、模块化、简洁实现、无兼容包袱、显式变量绑定

* 新框架无历史包袱 — 可大胆改 trait 签名，不做向后兼容

## 执行顺序与方案

### 阶段 1：P0-4 删除双范式断裂（最独立、零破坏、先清场）

**改动清单**：

1. 删除整文件：

   * [crates/macros/src/controller.rs](file:///e:/GitCode/RF/rust-webapp/crates/macros/src/controller.rs)

   * [crates/macros/src/route.rs](file:///e:/GitCode/RF/rust-webapp/crates/macros/src/route.rs)

2. 修改 [crates/macros/src/endpoint.rs](file:///e:/GitCode/RF/rust-webapp/crates/macros/src/endpoint.rs)：删除 `request_macro_impl`（L47-53）

3. 修改 [crates/macros/src/lib.rs](file:///e:/GitCode/RF/rust-webapp/crates/macros/src/lib.rs)：移除 `controller`、`http_get`、`http_post`、`http_put`、`http_delete`、`request` 六个宏的导出（L24-27、L114-135、L223-225）。同时移除 `mod controller;`、`mod route;`

4. 修改 [crates/webapp/src/lib.rs](file:///e:/GitCode/RF/rust-webapp/crates/webapp/src/lib.rs#L72-75)：从 `pub use rust_webapp_macros::{...}` 中移除 `controller, http_delete, http_get, http_post, http_put, request`

5. 修改 [crates/macros/Cargo.toml](file:///e:/GitCode/RF/rust-webapp/crates/macros/Cargo.toml)：description 改为 `rust-webapp macros: route shortcuts + handler auto-registration`（去掉"控制器"字样）

6. 修改 [crates/core/src/route/scan.rs](file:///e:/GitCode/RF/rust-webapp/crates/core/src/route/scan.rs)：L7、L47、L87 doc comment 中的 `#[controller]` 字样删除或改为 `#[get/post/...]`

7. 文档同步（仅删除虚假宣传）：

   * [README.md](file:///e:/GitCode/RF/rust-webapp/README.md) L86、L92、L163 删除 `crud_controller` 示例引用、"控制器"字样

   * `docs/rust-webapp/05-request-pattern/route-macros.md`、`docs/rust-webapp/01-introduction/ecosystem-overview.md`、`docs/rust-webapp/13-extensibility/custom-endpoints.md` 删除 controller 范式段落（不重写，只删除）

**验证**：`cargo build --workspace` 通过；docbit 不需任何改动。

***

### 阶段 2：P0-5 Mediator::send 改用 scope provider（为 P0-3 铺路）

**改动文件**：[crates/core/src/mediator/default.rs](file:///e:/GitCode/RF/rust-webapp/crates/core/src/mediator/default.rs)

**当前 L57-61**：

```rust
// Use the root provider as resolver. Scoped services resolved from root
// degrade to transient (fresh instance per call) per rust-dicore 0.5 semantics
let resolver: &dyn IServiceResolver = self.provider.as_ref();
let handler = (entry.factory)(resolver);
```

**改为**：

```rust
let scope = self.provider.create_scope();
let resolver: &dyn IServiceResolver = &scope;
let handler = (entry.factory)(resolver);
```

scope 作为局部变量在 send 调用期间存活，handler 与 behaviors 共享同一 scope。

**保持不变**：

* `Mediator` struct 字段 `provider: Arc<ServiceProvider>`（root）

* `Mediator::new(Arc<ServiceProvider>)` 构造签名

* `add_mediator` 仅翻转 `MEDIATOR_ACTIVE` 标志，不注册 Mediator 自身（避免 captive dependency）

**新增测试** [crates/host/tests/mediator\_test.rs](file:///e:/GitCode/RF/rust-webapp/crates/host/tests/mediator_test.rs)：注册一个带状态计数器的 Scoped handler，连续两次 `Mediator::send` 验证每次获得新实例（计数器不累加）。

**验证**：`cargo test -p rust-webapp-host mediator_test` 通过。

***

### 阶段 3：P0-3 IPipelineBehavior 链实现

**改动文件**：[crates/core/src/mediator/pipeline.rs](file:///e:/GitCode/RF/rust-webapp/crates/core/src/mediator/pipeline.rs)（当前空壳）

**实现内容**：新增 `build_chain` 函数，将 `Vec<Arc<dyn IPipelineBehavior>>` + 终端 `BoxedNextFn` 构造为嵌套链。

```rust
pub(crate) fn build_chain(
    behaviors: Vec<Arc<dyn IPipelineBehavior>>,
    terminal: BoxedNextFn,
) -> BoxedNextFn {
    // 反向折叠：terminal 是最内层，最后一个 behavior 最先执行
    let mut next = terminal;
    for behavior in behaviors.into_iter().rev() {
        let inner_next = next;
        next = Box::new(
            move |req: Box<dyn Any + Send>, svc: Arc<dyn IServiceResolver>| -> BoxedPipelineFuture {
                let b = Arc::clone(&behavior);
                Box::pin(async move { b.handle(req, inner_next, svc).await })
            },
        );
    }
    next
}
```

**改动文件**：[crates/core/src/mediator/default.rs](file:///e:/GitCode/RF/rust-webapp/crates/core/src/mediator/default.rs) `send` 方法

在 P0-5 改造基础上，L61 之前插入行为链解析：

```rust
let scope = self.provider.create_scope();
let resolver: &dyn IServiceResolver = &scope;

// 解析行为链（singleton 即可，无需 scope）
let behaviors = self.provider.get_all::<dyn IPipelineBehavior>();

// 构造终端 next：调用 handler factory + call
let entry_clone = Arc::clone(&entry);
let terminal: BoxedNextFn = Box::new(
    move |req: Box<dyn Any + Send>, svc: Arc<dyn IServiceResolver>| -> BoxedPipelineFuture {
        let entry = Arc::clone(&entry_clone);
        Box::pin(async move {
            let resolver_ref: &dyn IServiceResolver = svc.as_ref();
            let handler = (entry.factory)(resolver_ref);
            let result = (entry.call)(handler, req).await?;
            Ok(Box::new(result) as Box<dyn Any + Send>)
        })
    },
);

let chain = build_chain(behaviors, terminal);
let result_box = chain(Box::new(req), Arc::clone(&self.provider) as Arc<dyn IServiceResolver>).await?;
let result = result_box.downcast::<R>().map_err(|_| Error::Internal("response type mismatch".into()))?;
return Ok(*result);
```

注意：`entry.call` 签名需对照 [crates/core/src/route/scan.rs](file:///e:/GitCode/RF/rust-webapp/crates/core/src/route/scan.rs) 中的 `HandlerRegistration` 确认（factory 返回 `Box<dyn Any>`，call 接收 handler + req 返回 `Result<Box<dyn Any + Send>>`）。

**保持不变**：

* `IPipelineBehavior` trait 签名（[crates/core/src/pipeline.rs](file:///e:/GitCode/RF/rust-webapp/crates/core/src/pipeline.rs) L35-42，用户明确要求不可改 trait）

* `add_pipeline::<T>()` 注册入口（singleton + `Arc::new(T::default())`）

* `BoxedNextFn` / `BoxedPipelineFuture` 类型别名

**新增测试** [crates/host/tests/mediator\_test.rs](file:///e:/GitCode/RF/rust-webapp/crates/host/tests/mediator_test.rs)：实现 `LoggingBehavior` + `ValidationBehavior` 两个测试 behavior，验证：

1. 调用顺序：behavior1 → behavior2 → handler → 反向回到 behavior2 → behavior1
2. behavior 可以短路（不调 next 直接返回）
3. 无 behavior 注册时直接走 handler（向后兼容）

**验证**：`cargo test -p rust-webapp-host mediator_test` 全部通过；现有 send/publish 测试不破坏。

***

### 阶段 4：P0-1 管道短路重构 + use\_middleware 开口

#### 4a. IMiddleware trait 签名变更

**改动文件**：[crates/core/src/middleware.rs](file:///e:/GitCode/RF/rust-webapp/crates/core/src/middleware.rs) L15-30

**当前**：

```rust
#[async_trait::async_trait]
pub trait IMiddleware: Send + Sync {
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<()>;
    async fn after(&self, _ctx: &mut dyn IHttpContext) -> Result<()> { Ok(()) }
}
```

**改为**：

```rust
use std::ops::ControlFlow;

#[async_trait::async_trait]
pub trait IMiddleware: Send + Sync {
    /// 返回 `ControlFlow::Continue(())` 继续管道；`ControlFlow::Break(())` 短路（跳过后续 invoke + final_handler + 未注册的 after）
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<ControlFlow<()>>;
    async fn after(&self, _ctx: &mut dyn IHttpContext) -> Result<()> { Ok(()) }
}
```

**短路语义**（写入 doc comment）：

* `Break`：跳过后续中间件的 `invoke` 与 `after`，跳过 `final_handler`

* **已执行** **`invoke`** **的中间件仍反向执行** **`after`**（用于日志/追踪/计时收尾）

* `Err`：等同 `Break` 但表示错误，应同时设置响应状态码

#### 4b. MiddlewarePipeline::execute 重写

**改动文件**：[crates/host/src/pipeline.rs](file:///e:/GitCode/RF/rust-webapp/crates/host/src/pipeline.rs) L39-58

```rust
pub async fn execute(&self, ctx: &mut dyn IHttpContext, final_handler: HandlerFn) -> Result<()> {
    let mut executed: Vec<&Arc<dyn IMiddleware>> = Vec::with_capacity(self.middlewares.len());

    // 正向：任一 Break 即停止
    for middleware in &self.middlewares {
        match middleware.invoke(ctx).await? {
            ControlFlow::Continue(()) => executed.push(middleware),
            ControlFlow::Break(()) => {
                // 跳过 final_handler 与后续 invoke，但仍反向执行已 invoke 的 after
                for mw in executed.iter().rev() {
                    mw.after(ctx).await?;
                }
                return Ok(());
            }
        }
    }

    final_handler(ctx).await?;

    // 反向：仅对已 invoke 成功的中间件执行 after
    for middleware in executed.iter().rev() {
        middleware.after(ctx).await?;
    }
    Ok(())
}
```

#### 4c. 改造现有 9 个中间件

| 中间件                         | 文件                                                                                                         | 改动                                        |
| --------------------------- | ---------------------------------------------------------------------------------------------------------- | ----------------------------------------- |
| `AuthMiddleware`            | [auth\_jwt.rs:181-189](file:///e:/GitCode/RF/rust-webapp/crates/host/src/auth_jwt.rs#L181-189)             | invoke 末尾 `Ok(ControlFlow::Continue(()))` |
| `ResourceAuthMiddleware`    | [authz.rs:151-173](file:///e:/GitCode/RF/rust-webapp/crates/host/src/authz.rs#L151-173)                    | `Err` 保持；成功路径返回 `Continue`                |
| `CorsMiddleware`            | [cors.rs:60-116](file:///e:/GitCode/RF/rust-webapp/crates/host/src/cors.rs#L60-116)                        | **OPTIONS 分支返回** **`Break(())`**（核心修复）    |
| `RateLimitMiddleware`       | [rate\_limit.rs:122-142](file:///e:/GitCode/RF/rust-webapp/crates/host/src/rate_limit.rs#L122-142)         | **429 分支返回** **`Break(())`**（核心修复）        |
| `RequestIdMiddleware`       | [request\_id.rs:24-29](file:///e:/GitCode/RF/rust-webapp/crates/host/src/request_id.rs#L24-29)             | 末尾 `Continue`                             |
| `TimingMiddleware`          | [timing.rs:29-39](file:///e:/GitCode/RF/rust-webapp/crates/host/src/timing.rs#L29-39)                      | 末尾 `Continue`                             |
| `RequestTracing`            | [request\_tracing.rs:31-64](file:///e:/GitCode/RF/rust-webapp/crates/host/src/request_tracing.rs#L31-64)   | 末尾 `Continue`                             |
| `SecurityHeadersMiddleware` | [security\_headers.rs:26-52](file:///e:/GitCode/RF/rust-webapp/crates/host/src/security_headers.rs#L26-52) | 末尾 `Continue`                             |
| `SpaMiddleware`             | [crates/spa/src/spa.rs](file:///e:/GitCode/RF/rust-webapp/crates/spa/src/spa.rs)                           | **命中静态文件后返回** **`Break(())`**             |

每个中间件仅需在 `invoke` 末尾把 `Ok(())` 改为 `Ok(ControlFlow::Continue(()))`，OPTIONS/429/静态命中分支改为 `Ok(ControlFlow::Break(()))`。

#### 4d. HostBuilder 暴露 use\_middleware

**改动文件**：[crates/host/src/server.rs](file:///e:/GitCode/RF/rust-webapp/crates/host/src/server.rs) HostBuilder（L62-211）

新增字段：

```rust
struct HostBuilder {
    // ... existing
    user_middlewares: Vec<Arc<dyn IMiddleware>>,
}
```

新增方法：

```rust
pub fn use_middleware<M: IMiddleware + Send + Sync + 'static>(mut self, middleware: M) -> Self {
    self.user_middlewares.push(Arc::new(middleware));
    self
}

pub fn use_middleware_arc(mut self, middleware: Arc<dyn IMiddleware>) -> Self {
    self.user_middlewares.push(middleware);
    self
}
```

build() 中（L250-254）改造：

```rust
let mut pipeline = MiddlewarePipeline::new();
// 统一通过 use_middleware 注册（顺序确定，符合"无历史包袱"原则）
for mw in &self.user_middlewares {
    pipeline.add_middleware(Arc::clone(mw));
}
```

**完全废弃 DI** **`get_all::<dyn IMiddleware>()`** **收集模式**：顺序不确定是 bug 而非便利，新框架无历史包袱不做兼容。原 L251-254 的 DI 收集代码删除。

**内置中间件改造**：原 build() 中硬编码追加的 CorsMiddleware (L273)、SpaMiddleware (L287)、jwt\_middleware (L298) 保留为框架内部硬编码（这些是框架内置能力，非用户中间件），但在 doc comment 中明确说明用户中间件先于内置中间件执行。

#### 4e. 测试改造与新增

**改动文件**：[crates/host/tests/pipeline\_test.rs](file:///e:/GitCode/RF/rust-webapp/crates/host/tests/pipeline_test.rs)

* 现有 6 个测试中的 5 个 test-only mock 中间件需把 `Ok(())` 改为 `Ok(ControlFlow::Continue(()))`

* "invoke Err 短路"测试保持不变

* **新增测试**：

  * invoke 返回 `Break` 跳过 final\_handler 且跳过后续 invoke

  * invoke 返回 `Break` 时已执行 invoke 的中间件仍执行 after

  * RateLimitMiddleware 429 后 final\_handler 不被调用

  * CorsMiddleware OPTIONS 后 final\_handler 不被调用

  * `use_middleware` 注册顺序与执行顺序一致

**验证**：`cargo test -p rust-webapp-host` 全部通过；`cargo test -p rust-webapp-spa` 通过。

***

### 阶段 5：P0-2 健康检查端点接入 Registry

#### 5a. HealthCheckRegistry 新增 snapshot 方法

**改动文件**：[crates/host/src/health.rs](file:///e:/GitCode/RF/rust-webapp/crates/host/src/health.rs)

新增方法：

```rust
impl HealthCheckRegistry {
    /// 返回所有检查项的当前状态快照，用于 /health/ready 端点构造结构化 JSON
    pub async fn snapshot(&self) -> Vec<(String, HealthStatus)> {
        let checks = self.checks.lock().await;
        let mut result = Vec::with_capacity(checks.len());
        for (name, check_fn) in checks.iter() {
            result.push((name.clone(), check_fn()));
        }
        result
    }
}
```

注意：当前 `checks: Mutex<Vec<...>>`，需确认是 `tokio::sync::Mutex` 还是 `std::sync::Mutex`。若是 std Mutex 则 `snapshot` 改为同步函数（避免持有锁跨 await）。检查后决定签名。

#### 5b. HostBuilder 新增健康检查注册入口

**改动文件**：[crates/host/src/server.rs](file:///e:/GitCode/RF/rust-webapp/crates/host/src/server.rs)

新增字段：

```rust
struct HostBuilder {
    // ... existing
    health_registry: Arc<HealthCheckRegistry>,
}
```

HostBuilder::new() 默认创建空 registry。

新增方法：

```rust
pub fn add_health_check<F>(self, name: impl Into<String>, check: F) -> Self
where
    F: Fn() -> HealthStatus + Send + Sync + 'static,
{
    self.health_registry.register(name, Arc::new(check));
    self
}
```

#### 5c. 替换 /health/ready 静态端点

**改动文件**：[crates/host/src/server.rs](file:///e:/GitCode/RF/rust-webapp/crates/host/src/server.rs) L402-408

将 `StaticJsonEndpoint {"status":"ready"}` 替换为新的 `HealthEndpoint`（或动态闭包 endpoint）：

```rust
let registry = Arc::clone(&self.health_registry);
router.add_route("GET", "/health/ready", Arc::new(HealthEndpoint::new(registry)));
```

`HealthEndpoint::handle` 实现：

* 调用 `registry.snapshot()` 获取 `Vec<(String, HealthStatus)>`

* 整体状态：任一 fail → 503；任一 warn → 200 with warning；全部 pass → 200

* JSON 格式：`{"status":"pass|warn|fail","checks":[{"name":"...","status":"...","detail":"..."}]}`

* Content-Type: `application/health+json`（遵循 RFC 8407 draft-health-check）

`/health` 和 `/health/live` 保持 `StaticJsonEndpoint` 不变（无业务依赖）。

#### 5d. 测试

**新增测试** [crates/host/tests/health\_test.rs](file:///e:/GitCode/RF/rust-webapp/crates/host/tests/health_test.rs)：

* 空 registry 时 `/health/ready` 返回 `{"status":"pass","checks":[]}` + 200

* 注册 1 个 pass + 1 个 warn → 200 with warning

* 注册 1 个 fail → 503

* registry 注册多次 → snapshot 返回全部

**验证**：`cargo test -p rust-webapp-host health_test` 通过。

***

## 验证策略（端到端）

每阶段单独验证 + 全部完成后整体验证：

```bash
# 阶段 1 (P0-4)
cargo build --workspace
# 验证：编译通过，docbit 不需改动

# 阶段 2 (P0-5)
cargo test -p rust-webapp-host mediator_test

# 阶段 3 (P0-3)
cargo test -p rust-webapp-host mediator_test
# 验证：现有测试 + 新增 pipeline behavior 链测试

# 阶段 4 (P0-1)
cargo test -p rust-webapp-host
cargo test -p rust-webapp-spa
cargo test -p rust-webapp-core
# 验证：所有现有测试 + 新增短路测试

# 阶段 5 (P0-2)
cargo test -p rust-webapp-host health_test

# 整体冒烟
cargo build --workspace --release
cargo test --workspace
# 启动 docbit 验证：
# - cargo run -p docbit-host
# - curl http://localhost:5000/health/ready  (应返回 {"status":"pass","checks":[]})
# - curl http://localhost:5000/api/blogs     (业务接口正常)
# - 多次 curl 验证 rate_limit 短路生效（若启用）
```

## 不在本次范围

* 新功能叠加（HTTP/2、CompressionMiddleware 实现、CSP/HSTS、OpenAPI schemas 修复、路由 405）

* docbit 应用层重构（领域事件示例、分页列表、RFC 7807 错误模型、IClock/ICurrentUser 抽象）

* `#[derive(ApplyTo)]` 宏

* core 抽象 IServiceProvider trait 解耦 rust\_dicore

* `#[from_body/route/query]` 真正实现

* 这些是 P1/P2 项，待 P0 五项稳定后另行规划

## 风险与回滚

* **风险 1**：`Mediator::send` 改用 scope 后，若 rust-dicore 0.5 的 `create_scope` 在并发下有性能问题 → 回滚到 root provider，但记录为已知缺陷

* **风险 2**：`IPipelineBehavior` 链构造对 `Box<dyn Any>` 的类型擦除可能有边界 case → 测试覆盖足够则可暴露

* **风险 3**：`IMiddleware` trait 签名变更波及 9 个实现 + 5 个 test mock → 工作量明确，编译错误会逐一暴露，回滚成本低

整体回滚策略：每阶段一个 commit，编译失败立即回滚该阶段。

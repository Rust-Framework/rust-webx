# LRWF 生产就绪迭代开发计划

> **文档版本**：v1.0  
> **创建日期**：2026-06-11  
> **基线**：基于完整代码审计（7,900 行 Rust / 68 个源文件 / 6 个 crate）  
> **适用版本**：LRWF v0.1.x → v0.3.0  
> **总工期**：8–12 周（分 4 个阶段）

---

## 目录

1. [当前状态诊断](#1-当前状态诊断)
2. [核心设计原则](#2-核心设计原则)
3. [Phase 1：安全基线（第 1–2 周）](#3-phase-1安全基线第-12-周)
4. [Phase 2：性能重构（第 3–5 周）](#4-phase-2性能重构第-35-周)
5. [Phase 3：API 精炼（第 6–7 周）](#5-phase-3api-精炼第-67-周)
6. [Phase 4：生态就绪（第 8 周及以后）](#6-phase-4生态就绪第-8-周及以后)
7. [技术决策记录 (ADR)](#7-技术决策记录-adr)
8. [风险矩阵](#8-风险矩阵)
9. [验收总览](#9-验收总览)

---

## 1. 当前状态诊断

### 1.1 框架画像

| 指标 | 数值 |
|------|------|
| 总代码行数 | ~7,900 行 |
| Crate 数量 | 6（lrwf, lrwf-core, lrwf-http, lrwf-macros, lrwf-web, lrwf-openapi）+ 1 demo |
| 测试用例数 | 78 |
| 外部依赖 | tokio, hyper, serde, async-trait, inventory, lrdi, jsonwebtoken, rustls |
| CI 覆盖 | fmt + clippy + test + build (ubuntu-latest 单平台) |

### 1.2 生产阻断项（按严重度排序）

| ID | 严重度 | 问题 | 精确位置 | 影响 |
|----|--------|------|----------|------|
| **B1** | **P0 致命** | `CURRENT_USER` 全局可变状态导致并发身份混淆 | `crates/lrwf-http/src/endpoint.rs:12` | 权限提升 / 身份冒充 |
| **B2** | **P1 严重** | 全链路 `#[async_trait]` + `Box::pin` 动态分发 | `IRequestHandler` / `IMediator` / `IEndpoint` / `IMiddleware` | 性能落后成熟框架 ~3x |
| **B3** | **P2 中等** | Mediator dispatch 路径无单元测试 | `crates/lrwf-core/src/mediator_impl/mediator.rs` | 核心调度逻辑无回归保护 |
| **B4** | **P2 中等** | 端到端集成测试仅 3 个用例（404 / health / OpenAPI） | `crates/lrwf-http/tests/integration_test.rs` | 组合缺陷无法发现 |
| **B5** | **P2 中等** | CI 无覆盖率报告、无安全审计、无跨平台测试 | `.github/workflows/ci.yml` | CI 信号不完整 |
| **B6** | **P3 低** | README 架构图与实际 crate 结构不符 | `README.md` | 文档可发现性差 |
| **B7** | **P3 低** | `IHost` trait 与 `Host` 实现 API 不一致 | `crates/lrwf-core/src/app.rs` vs `crates/lrwf-http/src/server.rs` | 命名和语义混乱 |

### 1.3 功能完备性矩阵

#### 已完整实现（21 项）

| # | 功能 | 核心文件 | 行数 |
|---|------|----------|------|
| 1 | HTTP/HTTPS 服务器 + 多 URL 监听 | `server.rs` | 913 |
| 2 | Trie 路由匹配 + 路径参数提取 + 回溯 | `router.rs` | 159 |
| 3 | 中间件管道（forward + reverse after-hook + 短路） | `pipeline.rs` | 66 |
| 4 | CORS 中间件（含 preflight 处理） | `cors.rs` | 122 |
| 5 | JWT Bearer Token 认证（decode + claims） | `auth_jwt.rs` | 225 |
| 6 | 资源授权 RBAC（角色 + 权限，路由模式匹配） | `authz.rs` | 157 |
| 7 | Token Bucket 速率限制（per-IP） | `rate_limit.rs` | 163 |
| 8 | 请求体大小限制（413 响应） | `context.rs` | 296 |
| 9 | 统一 Error 枚举 + HTTP 状态码自动映射（8 变体） | `error.rs` | 55 |
| 10 | 编译时端点宏（`#[get]` / `#[post]` / `#[put]` / `#[delete]`） | `endpoint.rs` | 430 |
| 11 | `#[handler]` 编译时自动 DI 注册 | `handler.rs` | 83 |
| 12 | `#[controller]` 分组宏 | `controller.rs` | 52 |
| 13 | `#[authorize]` 声明式授权 | `endpoint.rs:383-403` | — |
| 14 | OpenAPI 3.0 规范生成 + 嵌入式 Swagger UI | `openapi.rs` + `apiui.rs` | 590 |
| 15 | SPA 静态文件中间件 + fallback | `spa.rs` | 222 |
| 16 | 配置系统（appsettings.json 多层合并 + `APP__` 环境变量覆盖） | `config.rs` | 294 |
| 17 | TLS / rustls 支持（PEM 证书） | `server.rs:862-913` | 52 |
| 18 | 健康检查端点（`/health` / `/healthz`） | `server.rs:302-309` | 8 |
| 19 | 开发/生产双模式结构化日志（tracing） | `server.rs:147-160` | 14 |
| 20 | 优雅关闭（Ctrl+C / SIGTERM + 30s drain） | `server.rs:344-572` | 229 |
| 21 | 生产安全警告（JWT 默认 secret、CORS wildcard 检测） | `server.rs:229-245` | 17 |

#### 部分实现 / 存根（6 项）

| # | 功能 | 当前状态 | Phase |
|---|------|----------|-------|
| S1 | `lrwf::request!` 宏 | Stub — 直接 `compile_error!` | Phase 3 |
| S2 | `#[from_body]` / `#[from_route]` / `#[from_query]` | Pass-through — 仅 metadata | Phase 3 |
| S3 | `IPipelineBehavior` 管道拦截器 | 骨架 — trait 定义完成，无实际链条 | Phase 3 |
| S4 | `IMediator::publish()` 并发发布 | 基础实现 — 顺序执行，无并发 | Phase 2 |
| S5 | Handler 通过 DI 注入 `IHttpContext` | 部分 — claims 转发，无完整上下文 | Phase 3 |
| S6 | 参数绑定自动反序列化 | 宏生成 — 路径参数 + body deserialize | Phase 3 |

#### 缺失

| # | 功能 | Phase |
|---|------|-------|
| M1 | 提取器模式（Extractor Pattern） | Phase 3 |
| M2 | 配置热重载 | Phase 3 |
| M3 | `lrwf-prometheus` / `lrwf-opentelemetry` | Phase 4 |
| M4 | musl / WASM / staticlib 跨平台支持 | Phase 4 |
| M5 | `sqlx` 事务示例 + Pipeline Behavior 事务传播 | Phase 4 |

---

## 2. 核心设计原则

| 原则 | 说明 | Rust 映射 |
|------|------|-----------|
| **零成本抽象优先** | 热路径避免 `dyn` 与堆分配 | 泛型单态化、原生 `async fn`、编译时路由分派 |
| **Fearless Concurrency** | 彻底消除全局可变状态 | 所有权转移、`Arc<不可变数据>`、per-request Context |
| **渐进式增强** | 外部 API 保持稳定，内部逐步替换 | Edition 2021 + 语义化版本控制 |
| **AI 原生友好** | 保留请求即端点的语义 | 宏生成代码保持人类可读 |
| **测试驱动修复** | 每个 P0/P1 修复必须先写回归测试 | `#[tokio::test]` + 并发压力测试 |

---

## 3. Phase 1：安全基线（第 1–2 周）

> **目标**：消除 P0/P2 缺陷，建立可信任的测试基线，达到"内部项目可用"。  
> **出口条件**：并发压力测试 100% 正确；代码覆盖率 >= 60%；Clippy 零警告；CI 全绿。

### 任务 1.1：修复 P0 并发身份混淆（B1）

**位置**：`crates/lrwf-http/src/endpoint.rs:12`

**问题**：`static CURRENT_USER: OnceLock<Mutex<Option<(String, String)>>>` 是全局可变状态。`tokio` 多任务调度下，请求 A 的 JWT 解析结果会被请求 B 覆盖。

**方案**：

1. **完全删除** `endpoint.rs` 中的 `CURRENT_USER`、`set_current_user`、`take_current_user`
2. 复用已存在的 `IClaimsExt` trait（`lrwf-core/src/http.rs:48-54`），`JwtAuth` 中间件已通过 `ctx.set_claims(...)` 写入
3. `StubEndpoint::handle` 中从 `ctx.claims()` 读取身份，显式传入 dispatch 函数

**代码变更示意**：

```rust
// BEFORE — 删除以下全部代码
pub(crate) fn set_current_user(id: &str, role: &str) { ... }
pub fn take_current_user() -> Option<(String, String)> { ... }

// AFTER — StubEndpoint::handle 中改用
let claims = ctx.claims().map(|c| c.clone_box());
// 显式传入 dispatch 函数，不再依赖全局状态
```

**验收标准**：

- [ ] 删除所有 `CURRENT_USER` 相关代码
- [ ] 并发压力测试：`wrk -t12 -c400 -d30s`，100% 请求的身份上下文与 JWT 一致
- [ ] 新增测试：`tests/concurrent_identity_test.rs` — 12 并发任务同时发带不同 JWT 的请求

**预计工时**：2 天

### 任务 1.2：移除 Router 运行时锁（B3 关联）

**位置**：`crates/lrwf-http/src/server.rs`

**说明**：代码中 router 已从 `Arc<RwLock<Router>>` 改为 `Arc<Router>`。本任务确认为已完成状态，并添加防御性测试。

**验收标准**：

- [ ] 确认 `Host` 中 `router` 字段类型为 `Arc<Router>`（非 `Arc<RwLock<Router>>`）
- [ ] 确认 `make_router_handler` 和 `serve_http` / `serve_https` 中无 `.read().await` 调用
- [ ] `Router` 文档化"启动后不可变"约束

**预计工时**：0.5 天

### 任务 1.3：补齐核心单元测试基线

**目标**：将覆盖率从当前 ~30% 提升至 >= 60%。

**测试矩阵**：

| 模块 | 测试场景 | 最低用例数 |
|------|----------|-----------|
| `Mediator::send` | 正常分派、handler 未注册、handler 返回错误、OnceLock 缓存命中 | 4 |
| `Mediator::publish` | 单 handler、多 handler、handler 返回错误、空 handler 列表 | 4 |
| `StubEndpoint` | 正常 dispatch、auth required + 合法 claims、auth required + 缺失 claims、auth required + 角色不匹配、动态 policy 检查 | 5 |
| `HttpContext` | body 读取、max_body_size 超限、claims set/get、RefCell 借用安全性 | 4 |
| `Config` | 加载不存在文件、环境变量覆盖、Development 合并、JSON 绑定 | 4 |
| `CorsMiddleware` | wildcard origin、精确 origin、无 origin header、credentials 设置 | 已有 7 |

**已覆盖的模块**（不需新增）：

- Router — 10 tests（exact match, params, multi-params, method distinction, 404, nested, duplicate, root）  
- Pipeline — 7 tests（empty, order, modify context, after hooks, reverse order, short-circuit, error skip after hooks）  
- JwtAuth — 8 tests（valid, missing, empty, invalid, wrong key, expired, case-sensitive prefix, whitespace）  
- Error — 15 tests（全变体 status_code + Display）  
- Authz — 8 tests  
- CORS — 7 tests  

**验收标准**：

- [ ] `cargo tarpaulin` 或 `cargo llvm-cov` 行覆盖率 >= 60%
- [ ] 所有新增测试在 CI 中通过
- [ ] CI 配置中添加覆盖率报告（`cargo tarpaulin --out Html --out Xml`）并上传到 Codecov / Coveralls

**预计工时**：5 天

### 任务 1.4：CI 管道增强

**方案**：

1. 添加覆盖率 job：`cargo tarpaulin --workspace --out Xml`
2. 添加安全审计：`cargo audit`（检查依赖漏洞）
3. 添加跨平台测试：在 `windows-latest`、`macos-latest` 上运行 `cargo test --workspace`

**验收标准**：

- [ ] `.github/workflows/ci.yml` 包含 coverage、audit、cross-platform 三个 job
- [ ] 所有 job 通过

**预计工时**：1 天

### Phase 1 交付清单

| 交付物 | 验收方式 |
|--------|----------|
| `CURRENT_USER` 完全移除 | code review |
| 并发身份测试通过 | `cargo test test_concurrent_identity` |
| 代码覆盖率 >= 60% | `cargo tarpaulin` CI 报告 |
| CI 全绿（含 clippy -D warnings） | GitHub Actions |
| 安全审计无严重漏洞 | `cargo audit` |

---

## 4. Phase 2：性能重构（第 3–5 周）

> **目标**：消除 P1 热路径开销，达到 Axum 同场景 >= 60% 吞吐量。  
> **原则**：外部 API 不变（`#[handler]` / `#[get]` 等宏保持稳定），仅改变内部实现和 trait 定义。

### 任务 2.1：`IRequestHandler` / `IEventHandler` 移除 `async_trait`

**技术决策（ADR-001）**：`IRequestHandler<T, R>` 和 `IEventHandler<T>` 不需要 object-safe。运行时通过泛型单态化调用。直接移除 `#[async_trait]`，使用 Rust 1.75+ 原生 `async fn`。

**代码变更**（`lrwf-core/src/handler.rs`）：

```rust
// BEFORE
#[async_trait::async_trait]
pub trait IRequestHandler<T, R>: Send + Sync
where
    T: IRequest<R> + Send + 'static,
    R: serde::Serialize + Send + 'static,
{
    async fn handle(&self, req: T) -> Result<R>;
    async fn handle_with_claims(&self, req: T, claims: Option<&dyn IClaims>) -> Result<R> {
        let _ = claims;
        self.handle(req).await
    }
}

// AFTER
pub trait IRequestHandler<T, R>: Send + Sync
where
    T: IRequest<R> + Send + 'static,
    R: serde::Serialize + Send + 'static,
{
    async fn handle(&self, req: T) -> Result<R>;
    async fn handle_with_claims(&self, req: T, claims: Option<&dyn IClaims>) -> Result<R> {
        let _ = claims;
        self.handle(req).await
    }
}
```

**连带修改**：

- `#[handler]` 宏生成的代码不再生成 `Box::pin` 包装
- `Register` 函数不再注册 `dyn IRequestHandler<T, R>`，改为注册具体类型
- `generate_dispatch_fn` 生成的 `static HANDLER: OnceLock<Arc<dyn IRequestHandler<...>>>` 改为 `OnceLock<Arc<ConcreteHandler>>`
- demo 和 examples 中所有 handler 移除 `#[async_trait]` 导入

**验收标准**：

- [ ] `IRequestHandler` 和 `IEventHandler` trait 不再有 `#[async_trait::async_trait]`
- [ ] `#[handler]` 宏生成的代码不再包含 `Box::pin` / `Pin<Box<dyn Future>>`
- [ ] demo 和所有 examples 编译通过
- [ ] `cargo test --workspace` 全部通过
- [ ] 基准测试：`mediator.send()` 延迟较基线降低 >= 50%

**预计工时**：5 天

### 任务 2.2：Mediator `TypeId` 快速分派（ADR-004 关联）

**当前问题**：`Mediator::send` 通过 `provider.get_service::<dyn IRequestHandler<T, R>>()` 进行 DI 查找，存在 dyn 解析开销。

**方案**：

1. 在 `HostBuilder::build` 阶段，将 `inventory` 收集的 handler 构建为 `HashMap<TypeId, Arc<dyn Any + Send + Sync>>` 本地缓存
2. `Mediator::send<T, R>` 通过 `TypeId::of::<T>()` 在本地缓存中 O(1) 查找
3. 查找成功后，通过 `downcast_ref` 获取具体 handler

**代码变更示意**：

```rust
// lrwf-http/src/server.rs: build() 中新增
let mut handler_cache: HashMap<TypeId, Arc<dyn Any + Send + Sync>> = HashMap::new();
for registration in inventory::iter::<HandlerRegistration> {
    let type_id = (registration.get_type_id)();
    let handler: Arc<dyn Any + Send + Sync> = (registration.get_or_default)();
    handler_cache.insert(type_id, handler);
}

// Mediator 使用
impl Mediator {
    pub fn new(handler_cache: HashMap<TypeId, Arc<dyn Any + Send + Sync>>) -> Self {
        Self { handler_cache }
    }

    async fn send<T, R>(&self, req: T) -> Result<R>
    where
        T: IRequest<R> + Send + 'static,
        R: serde::Serialize + Send + 'static,
    {
        let type_id = TypeId::of::<T>();
        let handler = self.handler_cache.get(&type_id)
            .and_then(|h| h.downcast_ref::<Arc<ConcreteHandler<T, R>>>())
            .ok_or_else(|| Error::Di(format!("No handler for {}", type_name::<T>())))?;
        handler.handle(req).await
    }
}
```

> **注意**：若 `TypeId` 方案因泛型类型擦除复杂度过高，回退方案为维持当前 `OnceLock` 缓存策略（dispatch 函数内已有），仅评估 Mediator 直接调用的优化空间。

**验收标准**：

- [ ] Mediator 分派延迟降低 >= 50%（对比 Phase 1 基线）
- [ ] DI 容器降级为启动期构建工具，运行时不参与热路径
- [ ] 所有 handler 仍可通过 `#[handler]` 自动注册

**预计工时**：5 天

### 任务 2.3：基准测试套件

**方案**：

1. 在 `crates/lrwf-http/benches/` 扩展已有 benchmark 文件
2. 使用 `criterion` 框架，`cargo bench --workspace`

**基准场景**：

| 基准 | 测量目标 | 最低目标 |
|------|----------|----------|
| `bench_router_match` | Trie 树匹配 10,000 次 | < 1 μs / 次 |
| `bench_mediator_send` | Mediator 分派 10,000 次 | < 500 ns / 次 |
| `bench_pipeline` | 5 层中间件链穿透 10,000 次 | < 10 μs / 次 |
| `bench_plaintext` | TechEmpower Plaintext 风格 | >= 50,000 req/s |
| `bench_json` | TechEmpower JSON Serialization 风格 | >= 80,000 req/s（即 Axum 的 >= 60%） |

**验收标准**：

- [ ] 5 个基准全部实现，`cargo bench` 可运行
- [ ] `bench_json` 达到 Axum ≥ 60%
- [ ] CI 中不运行 bench（仅本地手动），但编译检查通过

**预计工时**：3 天

### Phase 2 交付清单

| 交付物 | 验收方式 |
|--------|----------|
| `IRequestHandler` / `IEventHandler` 原生 async fn | code review + test pass |
| Mediator O(1) 分派 | benchmark 数据 |
| 基准测试套件 | `cargo bench` 可运行 |
| demo / examples 编译通过 | CI build |
| JSON 基准 >= Axum 60% | criterion 报告 |

---

## 5. Phase 3：API 精炼（第 6–7 周）

> **目标**：在性能提升的基础上，简化 API 并改善开发者体验。  
> **原则**：不破坏 Phase 1–2 中稳定的宏 API（`#[handler]` / `#[get]` 等）。

### 任务 3.1：HostBuilder API 统一

**当前问题**：

```rust
// IHost trait (lrwf-core)
pub trait IHost {
    async fn run(&self, addr: &str) -> Result<()>;
    async fn stop(&self) -> Result<()>;
}

// Host (lrwf-http) — 冲突
impl Host {
    pub async fn run(&self) -> Result<()> { /* reads AppOptions.app.urls */ }
    pub async fn run_at(&self, addr: &str) -> Result<()> { /* single addr */ }
    pub fn into_server(self) -> Server { ... }
    pub fn server_handle(&self) -> ServerHandle { ... }
}
```

**方案**：

1. `IHost` trait 重命名为 `IHostedService`，定义 `start(self) -> Result<ServerHandle>`
2. 移除 `IHost` 的 `stop(&self)`（无有效实现）
3. `HostBuilder` 提供统一接口：
   - `.run()` → 读 `AppOptions.app.urls` 启动
   - `.run_at(addr)` → 单地址快速启动
   - `.build()` → 返回 `Host` 用于测试
4. `ServerHandle::shutdown()` 替代 `IHost::stop()`

**验收标准**：

- [ ] `IHost` trait 重命名完成
- [ ] `HostBuilder` API 文档化
- [ ] demo 和 examples 无需修改即可编译

**预计工时**：2 天

### 任务 3.2：提取器模式基础支持

**目标**：提供类型安全的请求数据提取，不破坏现有 `IRequest<T>` 模型。

**方案**：

1. 新增 `lrwf-core/src/extract.rs`，定义提取器 trait：

```rust
pub trait FromRequest: Sized {
    type Rejection: Into<Error>;
    async fn from_request(ctx: &dyn IHttpContext) -> Result<Self, Self::Rejection>;
}

// 内置提取器
pub struct Path<T>(pub T);   // from route_params
pub struct Query<T>(pub T);  // from query_params
pub struct Json<T>(pub T);   // from body_bytes
pub struct Claims(pub JwtClaims); // from ctx.claims()
```

2. `#[handler]` 宏支持在 handler struct 的 `handle` 方法中自动调用提取器
3. 保留现有 `IRequest<T>` + `#[get]` 模型不变

**验收标准**：

- [ ] `FromRequest` trait 定义并可用
- [ ] `Path<T>` / `Json<T>` / `Query<T>` / `Claims` 提取器可用
- [ ] 新增 example：`examples/extractor_demo.rs`
- [ ] 现有所有 handler 代码无需修改

**预计工时**：5 天

### 任务 3.3：错误处理增强

**方案**：

1. `Error` 增加 `#[source]` 链支持，使错误链可通过 `std::error::Error::source()` 追溯
2. 为 `hyper::Error` 提供 `From` 自动转换
3. 中间件 `/` handler 支持通过 `Err(...)` 返回具体错误，异常中间件统一格式化
4. 增加 `Error::Unauthorized` 变体（独立于 `Error::Http`）映射 401

**代码变更**（`lrwf-core/src/error.rs`）：

```rust
#[derive(Error, Debug)]
pub enum Error {
    #[error("HTTP error: {0}")]
    Http(String),

    #[error("DI error: {0}")]
    Di(String),

    #[error("Routing error: {0}")]
    Routing(String),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("{0}")]
    Message(String),

    #[error("{0}")]
    Validation(String),

    #[error("{0}")]
    NotFound(String),

    // NEW
    #[error("Unauthorized: {0}")]
    Unauthorized(String),  // → 401

    // NEW: external error wrapping
    #[error("{0}")]
    External(#[from] Box<dyn std::error::Error + Send + Sync>),
}
```

**验收标准**：

- [ ] `error.source()` 可追溯错误链
- [ ] `hyper::Error` 可自动转换
- [ ] `Error::Unauthorized` 映射 401
- [ ] 新增测试覆盖新增变体

**预计工时**：3 天

### 任务 3.4：配置热重载（开发模式）

**方案**：

1. 引入 `notify` crate（仅 dev-dependencies）
2. 开发模式下监听 `appsettings.json` 变更
3. 通过 `tokio::sync::watch` 广播配置变更
4. `Host::options()` 返回最新配置

**验收标准**：

- [ ] 开发模式下修改 `appsettings.json` 后无需重启
- [ ] `AppMode::Production` 下不启用热重载

**预计工时**：3 天

### Phase 3 交付清单

| 交付物 | 验收方式 |
|--------|----------|
| HostBuilder API 统一 | 文档 + demo 编译通过 |
| 提取器模式原型 | `examples/extractor_demo.rs` 可运行 |
| 错误处理增强 | `cargo test error::` 通过 |
| 配置热重载 | 开发模式下手动验证 |
| 示例代码行数减少 >= 20% | 对比 `hello_request.rs` 前后 |

---

## 6. Phase 4：生态就绪（第 8 周及以后）

> **目标**：达到 v0.3.0 发布标准，具备社区推广基础。

### 任务 4.1：跨平台与 Fallback

**问题**：`inventory` 在 `staticlib` / `cdylib` / iOS / Android / WASM 目标上存在链接器段收集限制。

**方案**：

1. 在 `HostBuilder` 中添加 `.manual_route(path, method, handler_type)` 和 `.manual_handler(handler)` fallback API
2. 使用 `#[cfg(feature = "inventory")]` 编译条件
3. CI 中添加 `x86_64-unknown-linux-musl` 构建目标

**预计工时**：3 天

### 任务 4.2：`lrwf-prometheus` 扩展 crate

**方案**：

1. 新建 `crates/lrwf-prometheus/`
2. 实现 IMiddleware 适配 Prometheus metrics（请求计数、延迟直方图、错误计数）
3. 暴露 `/metrics` 端点

**预计工时**：3 天

### 任务 4.3：完整文档

**方案**：

1. 文档站（mdBook）包含：
   - 快速入门（同现有 `guides/quickstart.md`）
   - API 参考（同现有 `reference/api.md`）
   - 架构设计文档
   - 迁移指南（ASP.NET Core → LRWF）
   - 最佳实践（错误处理、认证授权、测试）
2. 新增 `sqlx` + `IPipelineBehavior` 事务示例

**预计工时**：5 天

### 任务 4.4：版本发布 v0.3.0

- [ ] 完成 Phase 1–3 所有任务
- [ ] `CHANGELOG.md` 记录所有变更
- [ ] 发布到 crates.io（如果之前已发布）
- [ ] 发布技术博客（Rust 中文社区 / This Week in Rust）

**预计工时**：1 天

---

## 7. 技术决策记录 (ADR)

### ADR-001：IRequestHandler 移除 async_trait

- **状态**：已决定
- **理由**：不需要 object-safe。原生 `async fn` 消除 `Box::pin` 开销（每次请求 20–30 ns）
- **风险**：`dyn IRequestHandler` 不再可用。LRWF 编译时收集 + 泛型分派模式无需 trait object

### ADR-002：认证信息通过 IHttpContext 显式传递

- **状态**：已决定
- **理由**：全局状态破坏 fearless concurrency。通过 `IClaimsExt` 在 per-request context 中存储
- **回滚策略**：无。全局 `CURRENT_USER` 在任何并发场景下都是错误设计

### ADR-003：Router 不可变 Arc

- **状态**：已决定
- **理由**：启动期注册、运行期只读是 Web 框架标准模式。`Arc<Router>` 消除 RwLock 开销
- **风险**：若需动态路由（插件热插拔），需引入 `arc-swap`。当前不在此定位

### ADR-004：保留 IMiddleware 的 async_trait（短期）

- **状态**：已决定
- **理由**：需要 `Vec<Arc<dyn IMiddleware>>`，必须 object-safe
- **演进路径**：Phase 4 中研究 `tower::Service` 风格泛型中间件

### ADR-005：Mediator TypeId 缓存策略

- **状态**：已决定
- **理由**：避免每次请求的 DI 容器 dyn 解析。构建期构建 `TypeId` → handler 映射，运行时 O(1) 查找
- **回滚策略**：若实现复杂度超出预期，维持当前 `OnceLock` 缓存策略

---

## 8. 风险矩阵

| 风险 | 概率 | 影响 | 缓解措施 |
|------|------|------|----------|
| 泛型化重构导致编译时间显著增加 | 高 | 中 | 使用 `cargo check` 快速验证；CI 启用 sccache |
| `lrdi` DI 容器不支持 `TypeId` 查找 | 中 | 高 | 在 `lrwf-http` 内部维护独立 `HashMap` 缓存；lrdi 仅作构建期工具 |
| `inventory` 在 musl/static 链接失效 | 中 | 中 | Phase 4 提供 `.manual_register()` fallback；CI 增加 musl 构建 |
| 外部贡献者与社区增长不足 | 中 | 高 | Phase 3 后发布技术博客；提供 Contributing Guide |
| API 频繁变动导致早期用户流失 | 低 | 高 | Phase 1–2 保持宏 API 稳定，变更仅限于内部实现 |
| 提取器模式与现有 IRequest 模型冲突 | 低 | 中 | 提取器作为独立模块，不与 IRequest 模型耦合 |

---

## 9. 验收总览

| 阶段 | 工期 | 关键交付物 | 硬性指标 |
|------|------|-----------|----------|
| **Phase 1** | 第 1–2 周 | 安全修复 + 测试基线 | 并发测试 100% 正确；覆盖率 >= 60%；Clippy 零警告 |
| **Phase 2** | 第 3–5 周 | 性能重构 + 基准套件 | 原生 async fn 完成；Mediator O(1)；JSON >= Axum 60% |
| **Phase 3** | 第 6–7 周 | API 精炼 + DX 提升 | HostBuilder 统一；提取器可用；示例代码减少 >= 20% |
| **Phase 4** | 第 8 周+ | 生态扩展 + 发布 | v0.3.0 发布；musl 构建；≥ 1 个外部中间件 crate |

### 各阶段版本号规划

| 版本 | 对应阶段 | 含义 |
|------|----------|------|
| v0.1.x | 当前 | 功能原型，不可用于生产 |
| v0.2.0 | Phase 1 完成 | 安全基线达标，内部项目可用 |
| v0.2.1 | Phase 2 完成 | 性能达标，正式项目可评估 |
| v0.3.0 | Phase 3–4 完成 | 首个"功能完整且性能可信"版本 |

---

## 附录 A：参考数据

- [Rust Async Working Group — async_trait benchmark](https://rust-lang.github.io/wg-async/vision/submitted_stories/status_quo/barbara_benchmarks_async_trait.html)：原生 async fn 639 ns vs async_trait boxed 1.82 µs（~3x 差距）
- [TechEmpower Benchmarks Round 23](https://www.techempower.com/benchmarks/#section=data-r23)：Axum/Actix JSON 14–20 万 req/s
- [eager to remove async_trait?](https://rymnc.com/posts/eager-to-remove-async-trait/)：`Box::pin` 单次 20–30 ns，百万次调用累计 30 ms

## 附录 B：版本号对照

| 本文档版本 | 对应里程碑 | 日期 |
|-----------|-----------|------|
| v1.0 | 初始迭代计划 | 2026-06-11 |

# LRWF 迭代计划：从可用到卓越

> **版本**：v1.0  
> **日期**：2026-06-10  
> **基线**：基于深度代码审计与安全/性能分析  
> **目标**：在保留 ASP.NET Core 风格开发者体验的同时，充分发挥 Rust 的零成本抽象与 fearless concurrency 优势，消除生产阻断性缺陷。

---

## 1. 核心设计原则

| 原则 | 说明 | Rust 映射 |
|------|------|-----------|
| **零成本抽象优先** | 热路径不使用 `dyn` 与堆分配替代方案 | 泛型单态化、原生 `async fn`、编译时路由分派 |
| **Fearless Concurrency** | 彻底消除全局可变状态 | 所有权转移、`Arc<不可变数据>`、per-request Context |
| **渐进式增强** | 外部 API 保持稳定，内部实现逐步替换 | Edition 2021 + 语义化版本控制 |
| **AI 原生友好** | 保留请求即端点的语义 | 宏生成代码保持人类可读，不引入隐式魔法 |

---

## 2. 当前状态诊断（生产阻断项）

### 2.1 致命：并发身份混淆（P0）
- **位置**：`crates/lrwf-http/src/endpoint.rs:12`
- **问题**：`static CURRENT_USER: OnceLock<Mutex<Option<(String, String)>>>` 是全局可变状态。在 `tokio` 多任务调度下，请求 A 的 JWT 解析结果会被请求 B 覆盖，导致权限提升或身份冒充。
- **修复紧迫性**：生产环境使用前的绝对前提。

### 2.2 严重：热路径动态分发（P1）
- **位置**：`IRequestHandler`、`IEventHandler`、`IMiddleware`、`IEndpoint` 全链路
- **问题**：`#[async_trait]` 将每个异步方法转换为 `Pin<Box<dyn Future + Send>>`，单次调用额外开销 20–30 ns（温和场景）至 1.2 µs（不利场景）。与原生 `async fn` 相比，Rust Async Working Group 实测差距约 3 倍。
- **影响**：在 TechEmpower JSON 序列化基准中，Axum/Actix 已达 14–20 万 req/s，LRWF 若不重构，预期差距在一个数量级。

### 2.3 中等：不必要的运行时同步（P2）
- **位置**：`crates/lrwf-http/src/server.rs`
- **问题**：`Arc<tokio::sync::RwLock<Router>>` 在运行期只读。Trie 树匹配发生在每次请求，RwLock 的 `read().await` 引入无意义的 cache coherency 开销。

### 2.4 中等：测试与基准缺失（P2）
- **问题**：集成测试仅覆盖 404/health/OpenAPI；无任何性能基准；CI 未集成覆盖率报告。

---

## 3. 迭代阶段

### Phase 1：安全与正确性基石（第 1–2 周）

**目标**：消除 P0/P2 缺陷，建立可信任的测试基线。

#### 任务 1.1：重构认证上下文传递（P0）
- **方案**：
  1. 删除 `endpoint.rs` 中的 `CURRENT_USER`、`set_current_user`、`take_current_user`。
  2. 复用 `IClaimsExt` trait（已存在于 `lrwf-core/src/http.rs`），要求 `JwtAuth` 中间件通过 `ctx.set_claims(...)` 写入。
  3. `StubEndpoint::handle` 中从 `ctx.claims()` 读取身份，显式传入 dispatch 函数。
- **代码变更示例**：
  ```rust
  // BEFORE（endpoint.rs）
  pub(crate) fn set_current_user(id: &str, role: &str) { ... }
  pub fn take_current_user() -> Option<(String, String)> { ... }

  // AFTER：完全删除。StubEndpoint::handle 改为：
  let claims = ctx.claims(); // 从 IHttpContext 读取
  ```
- **验收标准**：并发压力测试（`wrk -t12 -c400 -d30s`）下，100% 请求的身份上下文与请求携带的 JWT 一致。

#### 任务 1.2：移除 Router 运行期锁（P2）
- **方案**：
  1. `Host.router` 字段类型从 `Arc<tokio::sync::RwLock<Router>>` 改为 `Arc<Router>`。
  2. `make_router_handler` 和 `serve_http`/`serve_https` 移除 `.read().await`。
  3. `Router` 内部字段全部改为 `pub(crate)` 并文档化"启动后不可变"约束。
- **验收标准**：`cargo clippy --workspace --all-targets` 零警告；单元测试通过。

#### 任务 1.3：核心单元测试基线
- **测试矩阵**：
  | 模块 | 测试场景 | 用例数 |
  |------|----------|--------|
  | `Router` | Trie 树静态/动态匹配、参数提取、重复注册覆盖、404 | >= 8 |
  | `MiddlewarePipeline` | 顺序执行、短路行为、reverse pass、空管道 | >= 4 |
  | `JwtAuth` | 合法 token、过期 token、非法签名、缺失 header | >= 4 |
  | `Error` | 各变体 status_code 映射、Display 输出 | >= 2 |
- **验收标准**：`cargo tarpaulin` 或 `cargo llvm-cov` 行覆盖率 >= 60%；所有新增测试在 CI 中通过。

---

### Phase 2：性能重构 — 拥抱零成本抽象（第 3–5 周）

**目标**：消除 P1 热路径开销，将性能提升至主流框架的 60% 以上。

#### 任务 2.1：原生 `async fn` 迁移（非 dyn 边界）
- **范围**：`IRequestHandler<T, R>`、`IEventHandler<T>`
- **技术决策**：
  - 这两个 trait 在 LRWF 中**不需要 object-safe**。`#[handler]` 宏在编译时已通过 `inventory` 收集具体类型，运行时通过泛型单态化调用。
  - 因此可直接移除 `#[async_trait]`，改用 Rust 1.75+ 原生 `async fn`。
- **代码变更示例**：
  ```rust
  // BEFORE（lrwf-core/src/handler.rs）
  #[async_trait::async_trait]
  pub trait IRequestHandler<T, R>: Send + Sync { ... }

  // AFTER
  pub trait IRequestHandler<T, R>: Send + Sync {
      async fn handle(&self, req: T) -> Result<R>;
  }
  ```
- **兼容性处理**：`#[handler]` 宏生成的代码同步调整，不再生成 `Box::pin` 包装。

#### 任务 2.2：Mediator 泛型化与 DI 快速分派
- **当前问题**：`Mediator::send` 使用 `provider.get_service::<dyn IRequestHandler<T, R>>()`，运行时通过 `lrdi` 的字符串/类型名解析，存在 dyn 查找开销。
- **方案**：
  1. 在 `HostBuilder::build` 阶段，将 `inventory` 收集的 `HandlerRegistration` 构建为 `HashMap<TypeId, Arc<dyn Any + Send + Sync>>` 本地缓存。
  2. `Mediator::send<T, R>` 改为通过 `TypeId::of::<T>()` 在本地缓存中 O(1) 查找 handler，绕过 `lrdi` 的运行时反射。
  3. 查找成功后，通过 `downcast_ref` 转换为具体 `Arc<Handler>`，直接调用其原生 `async fn handle`。
- **收益**：消除每次请求的 DI 容器 dyn 解析 + `async_trait` Box::pin 双重开销。

#### 任务 2.3：宏生成代码零分配化
- **当前问题**：`RouteDispatch` 的函数指针类型返回 `Pin<Box<dyn Future + Send>>`。
- **方案**：
  1. 将 `RouteDispatch` 改为返回 `impl Future` 的泛型闭包，或直接使用 `std::future::ready`/`async move` 让编译器在栈上分配 future。
  2. 若必须保持函数指针（用于 inventory 收集），则让函数体内部直接 `.await` 具体 handler，避免多层 Box 嵌套。
- **代码变更示例**：
  ```rust
  // BEFORE
  pub dispatch: fn(...) -> Pin<Box<dyn Future<Output = ...> + Send>>

  // AFTER（伪代码，实际需配合 inventory 约束）
  pub dispatch: fn(...) -> impl Future<Output = ...> // 若不可行，则最小化 Box 层级
  ```

#### 任务 2.4：引入基准测试套件
- **方案**：
  1. 在 `benches/` 目录添加 `cargo bench` 套件，使用 `criterion`。
  2. 覆盖场景：
     - `bench_router_match`：Trie 树匹配 10,000 次
     - `bench_mediator_send`：泛型分派 vs dyn 分派对比
     - `bench_pipeline`：中间件链穿透
  3. 添加 TechEmpower 风格 HTTP 基准：`plaintext`、`json`。
- **验收标准**：
  - JSON 序列化场景达到 Axum 的 >= 60% 吞吐量（基于本地相同硬件）。
  - 每次 `mediator.send()` 的延迟较基线降低 >= 50%。

---

### Phase 3：API 精炼与开发者体验（第 6–7 周）

**目标**：在性能提升的同时，让 API 比 Phase 0 更简洁。

#### 任务 3.1：HostBuilder API 统一
- **当前问题**：`Host` 拥有 `run()`（无参，读配置）、`run_at(addr)`（显式地址），但 `IHost` trait 仅定义 `run(&self, addr: &str)`，存在命名冲突与不一致。
- **方案**：
  1. `IHost` trait 移除 `run(&self, addr: &str)`，改为 `start(self) -> Result<ServerHandle>`（类似 Axum 的 `Server` 模式）。
  2. `HostBuilder` 提供：
     - `.run()` → 读取 `AppOptions.app.urls`
     - `.run_at(addr: &str)` → 单地址快速启动
     - `.into_server()` → 返回可复用的 `Server` 结构，支持优雅关闭
  3. 移除 `IHost` 的 `stop(&self)`（无实现意义），改为 `ServerHandle` 的 `shutdown()`。

#### 任务 3.2：提取器模式（Extractor Pattern）
- **动机**：当前参数绑定依赖 `#[FromBody]`、`#[FromRoute]` 等宏，不够直观。
- **方案**：在保留现有宏的同时，新增基于泛型的提取器：
  ```rust
  use lrwf::extract::{Json, Path, Query, Claims};

  #[post("/users/{id}")]
  impl IRequest<UserModel> for UpdateUserRequest {}

  #[handler]
  impl IRequestHandler<UpdateUserRequest, UserModel> for UpdateUserHandler {
      async fn handle(&self, req: UpdateUserRequest) -> Result<UserModel> {
          // 未来可支持提取器自动解构：
          // async fn handle(&self, Path(id): Path<u64>, Json(body): Json<UpdateDto>) -> ...
          todo!()
      }
  }
  ```
  - 短期：在 `lrwf-macros` 中支持 `#[handler]` 自动生成提取器代码。
  - 长期（Phase 4）：完全支持函数式 handler，类似 Axum 的 `async fn handler(Path(id): Path<u64>) -> Json<T>`。

#### 任务 3.3：错误处理增强
- **方案**：
  1. `Error` 增加 `#[source]` 链支持，兼容 `std::error::Error::source()`。
  2. 为 `hyper::Error`、`lrdi::DiError`（若存在）提供 `#[from]` 自动转换。
  3. 中间件短路时，允许携带 `Error` 而非仅设置 status code，统一由异常中间件格式化。

#### 任务 3.4：配置系统简化
- **方案**：
  1. `appsettings.json` 支持环境变量插值：`"Urls": ["${LRWF_URLS:http://0.0.0.0:5000}"]`。
  2. 开发模式支持配置热重载（基于 `notify` crate 监听文件变更）。

---

### Phase 4：生态基础设施与长期演进（第 8 周及以后）

#### 任务 4.1：平台兼容性矩阵与 Fallback
- **问题**：`inventory` 在 `staticlib`、`cdylib`、iOS、Android、WASM 等目标上存在链接器段收集限制。
- **方案**：
  1. 在 `HostBuilder` 中提供 `.manual_register(...)` fallback API。
  2. 文档中明确声明支持矩阵，并提供 `cfg(not(inventory_supported))` 的编译分支。

#### 任务 4.2：数据库与事务示例
- 提供 `sqlx` + `IRequestHandler` 的完整示例，展示 Mediator Pipeline 中的事务传播（通过 `IPipelineBehavior` 包装 `BEGIN/COMMIT`）。

#### 任务 4.3：官方中间件扩展
- 发布 `lrwf-prometheus`、`lrwf-opentelemetry`、`lrwf-rate-limit` 等独立 crate，避免核心膨胀。

#### 任务 4.4：发布 v0.2.0
- **里程碑**：完成 Phase 1–3 后发布 v0.2.0，作为首个"功能完整且性能可信"的版本。

---

## 4. 技术决策记录（ADR）

### ADR-001：在 IRequestHandler 上移除 async_trait
- **状态**：已决定  
- **理由**：`IRequestHandler` 不需要 object-safe。原生 `async fn` 使编译器生成栈分配、可内联的 Future，消除每次请求的 `Box::pin`（20–30 ns）。  
- **风险**：`dyn IRequestHandler` 不再可用，但 LRWF 的编译时收集 + 泛型分派模式本就无需 trait object。

### ADR-002：认证信息通过 IHttpContext 显式传递
- **状态**：已决定  
- **理由**：全局状态破坏 Rust 的 fearless concurrency 承诺。`IHttpContext` 已是 per-request 的唯一上下文，在其内部通过 `Option<Box<dyn IClaims>>` 存储身份，数据流清晰且零成本。  
- **回滚策略**：无。全局 `CURRENT_USER` 在任何并发场景下都是错误设计，不存在可回滚的合理状态。

### ADR-003：Router 从 RwLock 改为不可变 Arc
- **状态**：已决定  
- **理由**：Web 框架的标准模式是"启动期注册、运行期只读"。Trie 树在 `build()` 后冻结，通过 `Arc<Router>` 共享即可。  
- **风险**：若未来需要动态路由（如插件热插拔），需重新引入锁或 `arc-swap`。当前框架定位不支持此场景。

### ADR-004：保留 IMiddleware 的 async_trait（短期）
- **状态**：已决定  
- **理由**：中间件链需要 `Vec<Arc<dyn IMiddleware>>`，必须保持 object-safe。直接移除会导致 API 不兼容。  
- **演进路径**：Phase 4 中研究迁移到 `tower::Service` 风格的泛型中间件，但需保证用户升级成本最小。

---

## 5. 风险与缓解

| 风险 | 概率 | 影响 | 缓解措施 |
|------|------|------|----------|
| 泛型化重构导致编译时间显著增加 | 高 | 中 | 开发环境使用 `cargo check`；评估 `cranelift` 后端；CI 中使用 `sccache` |
| `lrdi` DI 容器不支持 `TypeId` 快速查找 | 中 | 高 | 在 `lrwf-http` 内部维护独立的 `HashMap<TypeId, Arc<dyn Any>>` 缓存，仅将 `lrdi` 作为构建期收集工具 |
| `inventory` 在 musl/static 链接场景失效 | 中 | 中 | Phase 4 提供 `.manual_register()` fallback；CI 中增加 `x86_64-unknown-linux-musl` 构建目标 |
| 外部贡献者与社区增长不足 | 中 | 高 | Phase 3 完成后在 Rust 中文社区/RFC 发布技术博客；提供完善的 Contributing Guide |
| API 频繁变动导致早期用户流失 | 低 | 高 | Phase 1–2 保持 `#[handler]`/`#[get]` 等宏 API 稳定，变更仅限于内部实现 |

---

## 6. 验收总览

| 阶段 | 关键交付物 | 硬性指标 |
|------|-----------|----------|
| Phase 1 | 安全修复 + 测试基线 | 并发压力测试 100% 正确；代码覆盖率 >= 60%；Clippy 零警告 |
| Phase 2 | 性能重构 + 基准套件 | 原生 async fn 迁移完成；Mediator O(1) 分派；JSON 基准 >= Axum 60% |
| Phase 3 | API 精炼 + DX 提升 | HostBuilder API 统一；提取器模式可用；示例代码行数减少 >= 20% |
| Phase 4 | 生态扩展 + 正式发布 | v0.2.0 发布；支持 musl 构建；至少 1 个外部中间件扩展 |

---

## 附录：参考数据

- [Rust Async Working Group — Barbara benchmarks async_trait](https://rust-lang.github.io/wg-async/vision/submitted_stories/status_quo/barbara_benchmarks_async_trait.html)：原生 async fn 639 ns vs async_trait boxed 1.82 µs。
- [TechEmpower Framework Benchmarks Round 23](https://www.techempower.com/benchmarks/#section=data-r23)：Axum/Actix JSON 场景 14–20 万 req/s。
- [eager to remove async_trait? — Aaryamann Challani](https://rymnc.com/posts/eager-to-remove-async-trait/)：Box::pin 单次开销 20–30 ns，累积效应在百万级调用下达 30 ms。

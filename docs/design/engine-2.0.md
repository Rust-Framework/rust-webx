# rust-webx Engine 2.0 — 全面重写设计

> 状态：**已落地（2.0.0）**。内部执行引擎从零构建；应用侧公共 API 保持源码兼容。
> 本文取代按 1.5 `host` 逐步打补丁的路线。实测数字与遗留项见 §3、§6。

---

## 0. 范围与非目标

### 范围（从零重写）
- HTTP 执行引擎：worker/runtime 模型、accept/connection 生命周期、请求上下文、路由、dispatch、响应 finalize、中间件执行、停机/drain。
- 宏生成的编译期直达 dispatch 与 `TypeId` 注册表。
- `Host`/`HostBuilder` 与新引擎的装配层。
- 性能基准与 CI 性能门禁。

### 非目标（复用经过验证的协议工具，不做无意义重写）
- TLS 证书/私钥解析、rustls 配置。
- multipart 解析与落盘、Range/ETag/Last-Modified 计算、SSE 帧编码。
- `rust-dix` DI 容器、`rust-ef` 集成、AI/client 领域逻辑。

这些工具是**行为规范明确、测试充分**的协议实现；重写它们只会引入回归，不产生架构收益。它们会被移植到新模块并保持测试，而不是在旧 `host` 上继续修补。

---

## 1. 不可破坏的应用侧契约

重写期间以下内容必须保持**签名与语义**（详见 `api-contract` 审计）：

- 宏：`#[get]`/`#[post]`/`#[put]`/`#[delete]`/`#[endpoint]`、`#[handler]`/`#[handler(inject)]`、`#[authorize]`、`#[frompath]`/`#[fromquery]`/`#[frombody]`/`#[fromform]`/`#[fromfile]`/`#[fromheader]`/`#[fromclaim]`、`#[derive(WebxRequestMeta)]`、`#[webx::main]`。
- 核心类型：`IRequest<T>`、`IRequestHandler<T,R>`、`IEventRequest`/`IEventHandler`、`ActionResult`/`Error`/`Response`/`JsonResult`/`FileResult`/`ContentResult`/`NoContentResult`、`IdentityUser`/`RequestUser`、`PagedRequest`/`PagedResult`、`OperationResult`。
- HTTP 抽象：`IHttpContext`/`IHttpRequest`/`IHttpResponse`、`IMiddleware`/`MiddlewareSlot`、`IPipelineBehavior`/`Next`。
- Host：`Host::builder()`、`HostBuilder` 的现有方法（`register`、`use_middleware*`、`add_authentication`、`add_health_check*`、`use_cors`、`use_spa`、`add_memory_cache`、`add_options*`、`use_session_store`、`use_password_hasher`、`try_build`、`build`、`run*`）。
- DI：`IServiceCollectionExt`（`add_mediator`、`add_request_endpoints`、`add_typed_pipeline*`、`add_controllers`）。
- 不可删除的公共模块：auth、authz、cors、compression、rate_limit、health、memory_cache、spa、openapi、client、ai、ai-ef、ai-gateway。

**允许变化**：内部具体类型（`HttpContext` 内部布局、`StubEndpoint`、`RouteDispatch`、`HandlerCache`）可以替换；测试若依赖内部类型，迁移到等价的新测试，不把内部类型当作契约。

---

## 2. 新架构

```text
┌──────────────────────────────────────────────────────────────┐
│ webx (umbrella)  — re-exports only                           │
├──────────────────────────────────────────────────────────────┤
│ host  — HostBuilder / auth / cors / health / spa / openapi   │
│         装配层，构造 engine::Server                           │
├──────────────────────────────────────────────────────────────┤
│ engine (new crate)                                            │
│  worker pool → connection driver → request context            │
│  → pipeline (built-in stages + IMiddleware adapter)           │
│  → route table → direct dispatch / mediated fallback          │
│  → response finalize                                          │
├──────────────────────────────────────────────────────────────┤
│ core  — contracts / metadata / action / error / config        │
│ macros — route metadata + direct dispatch generation          │
└──────────────────────────────────────────────────────────────┘
```

### 2.1 Worker / runtime 模型（核心革新）

**原型已验证（关键决策）**：单 accept loop + N 个 `current_thread` worker 运行时 + 安全连接交接，不需要 `unsafe`：

```text
tokio listener (acceptor task)
  → accept tokio::TcpStream
  → stream.into_std()                  (safe, deregisters from acceptor runtime)
  → bounded channel to worker
  → tokio::net::TcpStream::from_std()  (safe, registers on worker runtime)
  → hyper auto::Builder (one per worker, into_owned)
```

本机 hyper-only 原型（32 连接、3 轮内取单轮，方向性）：

| workers | RPS@32 | p50 |
|---:|---:|---:|
| 4 | 125k | 239µs |
| 8 | 162k | 152µs |
| 16 | 182k | 72µs |
| 22 | 219k | 53µs |
| 单 multi-thread runtime 基线 | 114k | 249µs |

**结论**：per-worker current-thread runtime 是跨档的关键；`SO_REUSEPORT` 不是必需（actix 也没有用它），且 Windows 不支持。单 acceptor + 安全 `into_std/from_std` 交接可跨平台；代价是每连接两次注册，对 keep-alive 可忽略。

- 默认 `Workers = available_parallelism()`，clamp `1..=64`（Development 自动上限 8，避免测试主机线程膨胀）；显式值仍在 `1..=64`；配置键 `Server.Workers` / `APP__Server__Workers`。
- 每个 worker 独立 `current_thread` runtime；连接不迁移；请求 future 仍 `Send`（不引入 `!Send`/`Rc`，保持现有 `IMiddleware`/DI/`Arc` 兼容）。
- 单 acceptor 负责 round-robin + 容量感知；worker 用有界 channel；连接上限按 worker 分摊但总量仍以 `App.MaxConnections` 为准。
- TLS 在 worker 内握手（超时保护），HTTP/2 ALPN 不变。
- 优雅停机：signal → 取消 token（SSE）→ 停 hosted → acceptor 停止 → worker drain（`graceful_shutdown` + 超时）→ join 全部 worker → dispose DI。
- 全局连接上限、per-worker 限流、SSE active-stream 跟踪、hosted service 生命周期在新 `Server` 中重新设计。
- 不做：`SO_REUSEPORT`、每 worker 独立 bind、`!Send`/`Rc` 引擎（会破坏 DI/API，属于更远的阶段）。

### 2.2 编译期直达 dispatch

新增（内部/隐藏）`HandlerDispatch` inventory：由 `#[handler]` 生成，按 `(Req, Rsp)` 两个 `TypeId` 注册。

```text
RouteEntry(req TypeId, rsp TypeId, method, path)
  + HandlerDispatch(req TypeId, rsp TypeId, needs_scope,
                    fn(&mut dyn IHttpContext) -> Result<EndpointResult>)
  → Host 启动时 join，HTTP 路径直接：
      guard(#[authorize] / max_body_size)
      → bind::<T>(ctx) → construct H (默认或 scope) → H::handle(req) → EndpointResult
```

- `(Req, Rsp)` 双键避免同一请求类型实现多个 `IRequest<R>` 时 last-wins；重复 pair 在 `Host::build` 报错。
- 普通 JSON 路径不再经过 `Mediator::new`、`create_scope`、`get_all::<IPipelineBehavior>`、`Box<dyn Any>` 请求/响应、`JsonResult` 装箱、`Next` terminal 装箱。
- `#[handler(inject)]` 仍按请求创建一个 scope；`#[handler]` 默认构造不创建 scope。
- 任何 `IPipelineBehavior` 注册后进程级计数非零，Host 回退到 legacy mediated 路径，保证行为语义不变（保守但安全）。
- 用户代码仍用 `#[async_trait] impl IRequestHandler`，不改变 trait 形状。
- `IMediator::send`/`send_boxed` 保留给进程内调用，HTTP 快路径不再构造 Mediator。

### 2.3 零拷贝上下文与响应

- `RequestCtx` 持有 `http::request::Parts`/`Method`/`Uri`/`HeaderMap`/`Bytes`，不为 method/path/header 复制 `String`。
- 路由参数仅在实际存在时分配；静态路由零参数 map、零 pattern String。
- 响应使用 `HeaderMap` + 内部 body enum（Bytes/File/Stream），常见 JSON 直接产出 `Bytes`，不经 `JsonResult` box。
- `IHttpRequest`/`IHttpResponse` 的 &str/`Vec` 方法保持兼容；新引擎在内部用 `Bytes`/`HeaderMap` 实现。
- HEAD、Range、ETag、multipart、SSE 行为保持 1.5 语义；相应协议工具移植并保留测试。

### 2.4 洋葱中间件 / 内置快路径

- 每请求不再构造 `Vec<Arc<dyn IMiddleware>>`：`Pipeline` 只持有一次构建的 slice，按索引 forward/invoke、逆序 `after`。
- SecurityHeaders 与 JSON `content-type`/`cache-control` 由 engine finalizer 用静态 `HeaderName`/`HeaderValue` 写入，不再占一个中间件阶段和一次动态头解析；CORS 在 origins 为空时根本不注册阶段。
- RequestId、Metrics、Auth、RateLimit、SPA 仍是 `IMiddleware`（保持公共语义与槽位顺序），其 async-trait future 仍是剩余分配的一部分。
- 用户 `IMiddleware` 保持原 trait，通过 adapter 进入链；语义、槽位顺序、`after` 逆序执行保持不变。
- 预留真正的 `next` 洋葱语义（事务/超时/响应改写）作为内部能力，不在 2.0 破坏 `IMiddleware`。

### 2.5 DI 与 scope

- 一个请求最多一个 scope；binder/中间件/handler/behavior 共享（通过 `RequestContext`/context handle）。
- 无注入依赖的 handler 不创建 scope。
- `DbContext` 等 Scoped 服务仍按请求释放；严格作用域规则不变。

---

## 3. 性能目标（32 连接、JSON、本机同条件）

| 指标 | 1.5 实测 | 2.0 实测 |
|---|---:|---:|
| RPS@32 | 73k–95k（中位 ~88k） | **220k–238k**（低负载 burst）/ ~140k（同机 co-tenant 负载） |
| p50@32 | ~330µs | 45–80µs |
| allocations/req | 50.1 | **23.1** |
| 内存 | 10.5MB | ~12MB |
| 语义 | 926 tests | 999+ tests 全绿 + docbit/dmbit |

同机对照（同一 loadgen，32×keep-alive，1 KiB JSON）：axum 81k、hyper raw
114k、actix-web 233–284k。2.0 在低负载下进入 actix 同一档；高负载 / 同机
co-tenant 时绝对值下降，因此 CI 门禁只做**相对 1.5 基线**的回归判定，不把单轮
峰值当作承诺。基准脚本与原始 CSV 保留在仓库外的 `rw-bench`，避免污染发布包。

---

## 4. 迁移与交付

1. 新 crate `crates/engine` 从零实现；旧 `host` 保留公共类型，内部逐步切换到 engine，最终删除旧 `server/context/pipeline/router/endpoint` 实现。
2. 兼容性优先：先把现有 107 个测试套件与 docbit/dmbit 跑绿，再追性能。
3. 每阶段提交可运行分支；最终合并到 `rewrite/2.0`，通过 CI、发布 dry-run 后才考虑 2.0 发布。
4. 不在本分支继续 1.5 的发布修复；1.5.x 维护走 `main`。

---

## 5. 风险清单

| 风险 | 影响 | 缓解 |
|---|---|---|
| Windows 多 listener 语义 | 无法跨平台达到 actix 档 | 先做 Unix/Windows 原型；不达标则 Windows 回退单 runtime，文档标注 |
| TLS/HTTP2 与 per-worker 结合 | 握手/ALPN/停机回归 | 复用 TLS 工具，worker 级集成测试 |
| 双 dispatch 路径（direct/mediated） | 语义分叉 | 行为测试矩阵 + 默认只在无 behavior 时走 direct |
| 内部类型公开导致测试迁移 | 短期红灯 | API 审计清单 + 兼容测试 crate |
| 性能优化牺牲安全默认 | 安全回归 | 安全阶段保留，minimal profile 显式 opt-in |

---

## 6. 里程碑

- M0 ✅：设计/契约/骨架（本文 + `crates/engine` + 公共 API 审计）。
- M1 ✅：engine 跑通 HTTP/1.1，宿主现有路由/绑定/响应测试通过。
- M2 ✅：direct dispatch + 零拷贝上下文，JSON 路径分配 23.1（目标 ≤30）。
- M3 ✅：worker pool + TLS/h2 + shutdown，1.5 全量测试 + docbit/dmbit 全绿。
- M4 ✅：32 连接峰值 220–238k（actix 同档），clippy/fmt/doc/publish dry-run 门禁通过；CI 性能门禁以相对基线固化。

**遗留（非 2.0 阻塞）**：自定义 HTTP/1 编码器与 `!Send`/`Rc` 引擎可把持续吞吐再推
向 250k+，但会破坏现有 `Send` trait 与 DI 形状，留作独立产品决策；RequestId /
Metrics 仍是 `IMiddleware` boxing，后续可迁入 engine 快路径。

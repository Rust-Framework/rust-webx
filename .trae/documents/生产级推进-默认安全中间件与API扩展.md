# 生产级推进：默认安全中间件 + use\_middleware\_with API + 测试覆盖扩展

## Context

上一轮 plan（`生产级稳定性-修复404与集成测试扩展.md`）已写入 11 个集成测试与 CORS preflight 修复，但**尚未编译验证**；builder\_test.rs 也未创建。本轮在此基础上推进生产级：

1. **完成上轮验证**：确保 cors.rs、integration\_test.rs、builder\_test.rs 全部编译并测试通过
2. **默认安全中间件**：SecurityHeadersMiddleware + RequestIdMiddleware 默认启用（zero-config safe defaults，符合 ASP.NET Core 风格）
3. **use\_middleware\_with API**：扩展支持有参构造中间件（如 `RateLimitMiddleware::new(10.0, 20)`），保留 `use_middleware::<T: Default>` 用于简单场景
4. **集成测试扩展**：覆盖默认安全中间件、RateLimit、use\_middleware\_with API

不在本轮范围：CompressionMiddleware 实现（涉及 IHttpContext body 读取/重写，需单独评估）、RequestTracing 默认启用（日志策略应用差异大，应由应用显式启用）、JWT 端到端集成测试（复杂度高）、docbit 健康探针配置（应用层决策）。

## Current State Analysis

### 已有但未验证

* `crates/host/src/cors.rs` L92-107：OPTIONS preflight 改为 `ControlFlow::Break(())`，未编译验证

* `crates/host/tests/integration_test.rs`：11 个测试已写入，未编译/运行验证

* `crates/host/src/server.rs` L175-178：`no_spa()` 方法已添加

* `crates/host/src/server.rs` L238-243：`use_middleware::<T: Default>()` 已存在

### 待补

* `crates/host/tests/builder_test.rs`：不存在

* 默认安全中间件：`SecurityHeadersMiddleware`（`crates/host/src/security_headers.rs`）和 `RequestIdMiddleware`（`crates/host/src/request_id.rs`）已实现 `Default`，但 `server.rs` build() 未自动添加

* `use_middleware_with`：API 不存在，无法注册 `RateLimitMiddleware::new(rate, burst)` 等有参构造中间件

### 关键现状

* `crates/host/src/pipeline.rs` L43-75：MiddlewarePipeline 顺序执行 invoke，反向执行 after；支持 `ControlFlow::Break(())` 短路

* `crates/host/src/server.rs` L307-373：pipeline 构建顺序为「用户中间件 → CORS → SPA → Auth」

* `crates/host/src/lib.rs` L1-35：`pub use *` 导出所有模块，新增 API 自动可用

* `crates/host/src/rate_limit.rs` L115-119：`RateLimitMiddleware::new(rate, burst)` 不实现 Default，需要 use\_middleware\_with 才能注册

* `crates/host/src/security_headers.rs` L20-24：实现 `Default`

* `crates/host/src/request_id.rs` L18-22：实现 `Default`

## Proposed Changes

### 改动 1：编译验证上轮改动

**文件**：无修改，仅运行命令

**为什么**：上轮 cors.rs 和 integration\_test.rs 改动尚未验证，必须先确认基线稳定。

**命令**：

```powershell
cargo build -p rust-webapp-host
cargo test -p rust-webapp-host --test integration_test
```

**预期**：11 个集成测试全部通过。若失败，定位并修复（不扩大改动范围）。

### 改动 2：创建 builder\_test.rs

**文件**：新增 [crates/host/tests/builder\_test.rs](file:///e:/GitCode/RF/rust-webapp/crates/host/tests/builder_test.rs)

**为什么**：上轮 plan 改动 6 未完成，HostBuilder API（`use_middleware`、`no_spa`）无专门测试覆盖。

**实现**：

```rust
//! HostBuilder API tests — verifies middleware registration and pipeline wiring.

use std::net::TcpListener;
use std::ops::ControlFlow;
use std::sync::Arc;

use rust_webapp_core::http::IHttpContext;
use rust_webapp_core::middleware::IMiddleware;
use rust_webapp_host::server::Host;

fn find_free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

struct MarkerMiddleware;

impl Default for MarkerMiddleware {
    fn default() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl IMiddleware for MarkerMiddleware {
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> rust_webapp_core::error::Result<ControlFlow<()>> {
        ctx.response_mut().set_header("x-marker", "yes");
        Ok(ControlFlow::Continue(()))
    }
}

#[tokio::test]
async fn use_middleware_registers_into_pipeline() {
    let port = find_free_port();
    let addr = format!("127.0.0.1:{}", port);
    let host = Host::builder()
        .mode(rust_webapp_core::mode::AppMode::Development)
        .no_spa()
        .use_middleware::<MarkerMiddleware>()
        .build();
    tokio::spawn(async move { host.run_at(&addr).await.unwrap() });
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let resp = reqwest::get(format!("http://127.0.0.1:{}/health", port))
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(resp.headers().get("x-marker").unwrap(), "yes");
}
```

**验证**：`cargo test -p rust-webapp-host --test builder_test` 通过。

### 改动 3：默认启用 SecurityHeaders + RequestId

**文件**：[crates/host/src/server.rs](file:///e:/GitCode/RF/rust-webapp/crates/host/src/server.rs) L307 附近

**为什么**：符合 ASP.NET Core "zero-config safe defaults" 理念。每个生产级 Web 应用都需要安全头（防 MIME 嗅探、点击劫持）和请求 ID（可观测性）。默认启用避免每个新应用重复样板代码，符合用户偏好"框架自动化 + 微软极致易用"。RequestTracing 不默认启用（日志策略应用差异大）。

**实现**：

在 L307 `let mut pipeline = MiddlewarePipeline::new();` 之后、L308 `let middlewares: Vec<Arc<dyn IMiddleware>> = provider.get_all::<dyn IMiddleware>();` 之前插入：

```rust
let mut pipeline = MiddlewarePipeline::new();

// Default security & observability middleware (zero-config safe defaults).
// Order: SecurityHeaders → RequestId → user middleware → CORS → SPA → Auth
// - SecurityHeaders first so every response (including short-circuited) gets headers
// - RequestId first so every response carries a trace ID
pipeline.add_middleware(Arc::new(crate::security_headers::SecurityHeadersMiddleware::new()));
pipeline.add_middleware(Arc::new(crate::request_id::RequestIdMiddleware::new()));

let middlewares: Vec<Arc<dyn IMiddleware>> = provider.get_all::<dyn IMiddleware>();
for mw in middlewares {
    pipeline.add_middleware(mw);
}
```

**注意**：

* 不修改 `use_middleware` 注册逻辑——用户中间件仍通过 DI `get_all::<dyn IMiddleware>()` 收集

* SecurityHeaders/RequestId 直接 `Arc::new(...)` 添加，不通过 DI（避免被用户 `use_middleware` 重复注册）

* 顺序在用户中间件之前，确保短路响应也带安全头和请求 ID

### 改动 4：新增 use\_middleware\_with API

**文件**：[crates/host/src/server.rs](file:///e:/GitCode/RF/rust-webapp/crates/host/src/server.rs) L238-243 附近（紧邻 `use_middleware`）

**为什么**：当前 `use_middleware::<T: Default>` 无法注册 `RateLimitMiddleware::new(10.0, 20)` 等有参构造中间件。新增 `use_middleware_with(factory)` 接受闭包构造，API 对称清晰，符合用户选择。

**实现**：

在 `use_middleware` 方法之后插入：

````rust
/// Register a middleware with a custom factory function.
///
/// Use this when the middleware requires constructor parameters:
///
/// ```ignore
/// Host::builder()
///     .use_middleware_with(|| {
///         Arc::new(RateLimitMiddleware::new(10.0, 20)) as Arc<dyn IMiddleware>
///     })
///     .build()
/// ```
pub fn use_middleware_with<F>(self, factory: F) -> Self
where
    F: Fn() -> Arc<dyn IMiddleware> + Send + Sync + 'static,
{
    self.service_configs.push(Box::new(move |svc| {
        svc.singleton::<dyn IMiddleware>(move |_| factory())
    }));
    self
}
````

**注意**：

* 保留 `use_middleware::<T: Default>` 用于无参构造场景

* `factory: Fn() -> Arc<dyn IMiddleware>` 支持 `Send + Sync + 'static`，可跨线程调用

* 通过 DI `singleton` 注册，与 `use_middleware` 走相同收集路径（`get_all::<dyn IMiddleware>()`）

### 改动 5：扩展集成测试 — 默认安全中间件验证

**文件**：[crates/host/tests/integration\_test.rs](file:///e:/GitCode/RF/rust-webapp/crates/host/tests/integration_test.rs) 末尾追加

**为什么**：改动 3 默认启用了 SecurityHeaders + RequestId，必须有回归测试验证默认行为不被破坏。

**实现**：追加 2 个测试：

```rust
// ---------------------------------------------------------------------------
// Default security & observability middleware
// ---------------------------------------------------------------------------

#[tokio::test]
async fn integration_default_security_headers_present() {
    let port = find_free_port();
    spawn_test_host(port).await;

    let resp = reqwest::get(format!("http://127.0.0.1:{}/health", port))
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    let headers = resp.headers();
    assert_eq!(headers.get("x-content-type-options").unwrap(), "nosniff");
    assert_eq!(headers.get("x-frame-options").unwrap(), "DENY");
    assert_eq!(
        headers.get("referrer-policy").unwrap(),
        "strict-origin-when-cross-origin"
    );
}

#[tokio::test]
async fn integration_default_request_id_present() {
    let port = find_free_port();
    spawn_test_host(port).await;

    let resp = reqwest::get(format!("http://127.0.0.1:{}/health", port))
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    let request_id = resp
        .headers()
        .get("x-request-id")
        .expect("x-request-id header present by default");
    assert!(!request_id.is_empty());
}
```

### 改动 6：扩展集成测试 — RateLimit + use\_middleware\_with

**文件**：[crates/host/tests/integration\_test.rs](file:///e:/GitCode/RF/rust-webapp/crates/host/tests/integration_test.rs) 末尾追加

**为什么**：验证 `use_middleware_with` API 可注册有参构造的 RateLimitMiddleware，并验证 RateLimit 短路语义（429）。

**实现**：追加 2 个测试：

```rust
// ---------------------------------------------------------------------------
// use_middleware_with API + RateLimit short-circuit
// ---------------------------------------------------------------------------

#[tokio::test]
async fn integration_rate_limit_returns_429_when_exceeded() {
    let port = find_free_port();
    spawn_test_host_with(port, |b| {
        b.use_middleware_with(|| {
            Arc::new(rust_webapp_host::rate_limit::RateLimitMiddleware::new(1.0, 2))
                as Arc<dyn rust_webapp_core::middleware::IMiddleware>
        })
    })
    .await;

    let client = reqwest::Client::new();
    // First 2 requests: allowed (burst=2)
    let r1 = client
        .get(format!("http://127.0.0.1:{}/health", port))
        .send()
        .await
        .unwrap();
    let r2 = client
        .get(format!("http://127.0.0.1:{}/health", port))
        .send()
        .await
        .unwrap();
    assert_eq!(r1.status().as_u16(), 200);
    assert_eq!(r2.status().as_u16(), 200);

    // Third request immediately: should be rate-limited (429)
    let r3 = client
        .get(format!("http://127.0.0.1:{}/health", port))
        .send()
        .await
        .unwrap();
    assert_eq!(r3.status().as_u16(), 429);
}

#[tokio::test]
async fn integration_use_middleware_with_runs_in_pipeline() {
    use rust_webapp_core::http::IHttpContext;
    use rust_webapp_core::middleware::IMiddleware;
    use std::ops::ControlFlow;

    struct HeaderTagMiddleware;
    #[async_trait::async_trait]
    impl IMiddleware for HeaderTagMiddleware {
        async fn invoke(&self, ctx: &mut dyn IHttpContext) -> rust_webapp_core::error::Result<ControlFlow<()>> {
            ctx.response_mut().set_header("x-tagged", "true");
            Ok(ControlFlow::Continue(()))
        }
    }

    let port = find_free_port();
    spawn_test_host_with(port, |b| {
        b.use_middleware_with(|| Arc::new(HeaderTagMiddleware) as Arc<dyn IMiddleware>)
    })
    .await;

    let resp = reqwest::get(format!("http://127.0.0.1:{}/health", port))
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(resp.headers().get("x-tagged").unwrap(), "true");
}
```

### 改动 7：扩展 builder\_test.rs — use\_middleware\_with API

**文件**：[crates/host/tests/builder\_test.rs](file:///e:/GitCode/RF/rust-webapp/crates/host/tests/builder_test.rs) 追加

**为什么**：builder\_test.rs 应覆盖 `use_middleware_with` API 的注册路径（与 integration\_test 端到端验证互补）。

**实现**：在文件末尾追加：

```rust
#[tokio::test]
async fn use_middleware_with_registers_into_pipeline() {
    use rust_webapp_core::http::IHttpContext;
    use rust_webapp_core::middleware::IMiddleware;
    use std::ops::ControlFlow;

    struct TagMiddleware;
    #[async_trait::async_trait]
    impl IMiddleware for TagMiddleware {
        async fn invoke(&self, ctx: &mut dyn IHttpContext) -> rust_webapp_core::error::Result<ControlFlow<()>> {
            ctx.response_mut().set_header("x-via-with", "1");
            Ok(ControlFlow::Continue(()))
        }
    }

    let port = find_free_port();
    let addr = format!("127.0.0.1:{}", port);
    let host = Host::builder()
        .mode(rust_webapp_core::mode::AppMode::Development)
        .no_spa()
        .use_middleware_with(|| Arc::new(TagMiddleware) as Arc<dyn IMiddleware>)
        .build();
    tokio::spawn(async move { host.run_at(&addr).await.unwrap() });
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let resp = reqwest::get(format!("http://127.0.0.1:{}/health", port))
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(resp.headers().get("x-via-with").unwrap(), "1");
}
```

## Assumptions & Decisions

1. **默认中间件顺序**：SecurityHeaders → RequestId → 用户中间件 → CORS → SPA → Auth。理由：安全头和请求 ID 应在最早阶段注入，确保所有响应（含短路）都带这些头。
2. **默认中间件不通过 DI**：直接 `Arc::new(...)` 添加到 pipeline，避免被 `use_middleware` 重复注册。若用户想替换默认行为，可调用 `no_spa()` 类似的 opt-out API（本轮不实现，留作下轮）。
3. **`use_middleware_with`** **工厂签名**：`Fn() -> Arc<dyn IMiddleware> + Send + Sync + 'static`。理由：与 `singleton::<dyn IMiddleware>` 注册路径一致，工厂可在任意线程调用。
4. **不实现 CompressionMiddleware**：涉及 IHttpContext body 读取/重写，需评估 response API 是否支持读取已写入的 body。留作下轮专项。
5. **不默认启用 RequestTracing**：日志策略（info vs errors\_only、采样率）应用差异大，应由应用显式 `use_middleware::<RequestTracing>()` 启用。
6. **RateLimit 测试参数**：`new(1.0, 2)` 表示 1 req/s 持续速率 + 2 突发。前 2 个请求允许（突发），第 3 个立即请求应被限流。测试不 sleep 等待令牌补充，确保结果稳定。
7. **不修改 docbit**：本轮聚焦框架层。docbit 健康探针配置是应用层决策，留作下轮。

## Verification Steps

```powershell
# 1. 编译验证（改动 1）
cargo build -p rust-webapp-host

# 2. 集成测试（改动 5、6）
cargo test -p rust-webapp-host --test integration_test

# 3. builder_test（改动 2、7）
cargo test -p rust-webapp-host --test builder_test

# 4. 全量测试
cargo test --workspace

# 5. docbit 零破坏
cargo build -p docbit-host -p docbit-handlers -p docbit-contracts
```

## 实施顺序

1. **改动 1**：编译验证上轮改动 → 确保基线稳定
2. **改动 2**：创建 builder\_test.rs → 验证 use\_middleware API
3. **改动 3**：默认启用 SecurityHeaders + RequestId → 编译验证
4. **改动 4**：新增 use\_middleware\_with API → 编译验证
5. **改动 5**：扩展集成测试（默认安全中间件）→ 验证测试通过
6. **改动 6**：扩展集成测试（RateLimit + use\_middleware\_with）→ 验证测试通过
7. **改动 7**：扩展 builder\_test.rs（use\_middleware\_with）→ 验证测试通过
8. **全局验证**：`cargo test --workspace` + docbit 零破坏

## 完成标准

* ✅ 上轮 11 个集成测试全部通过

* ✅ builder\_test.rs 创建并测试通过

* ✅ 默认 SecurityHeaders + RequestId 启用，集成测试验证

* ✅ `use_middleware_with` API 可用，builder\_test + integration\_test 验证

* ✅ RateLimit 集成测试验证 429 短路

* ✅ `cargo test --workspace` 无新增失败

* ✅ docbit 应用层零破坏

## 不在本次范围

* CompressionMiddleware 实现（需评估 IHttpContext body API）

* RequestTracing 默认启用（日志策略应用差异大）

* JWT 端到端集成测试（需设计受保护端点 + token 生成）

* docbit 健康探针配置（应用层决策）

* 默认安全中间件的 opt-out API（如 `no_default_middleware()`）

* app\_base() 遍历逻辑重构（影响面大）


# 生产级推进：JWT 端到端测试 + CompressionMiddleware 实现

## Context

上轮完成默认安全中间件 + use_middleware_with API 后，框架生产级能力已基本就绪。本轮推进两个方向：

1. **JWT 端到端集成测试**：验证 `#[authorize]` 宏 + JWT 中间件 + 401/403 路径的端到端正确性。当前 auth_test.rs 仅单元测试 JwtAuth，未覆盖 HTTP 端到端路径。
2. **CompressionMiddleware 实现**：补齐生产级中间件矩阵。当前 compression.rs 仅有 `compress_gzip` 工具函数，未实现 `IMiddleware` trait。需扩展 `IHttpResponse` trait 加 `body_bytes()` 读方法（与 `IHttpRequest::body_bytes` 对称）。

**关键发现（驱动修复）**：
- `auth_jwt.rs` L163-164：token 解码失败返回 `Err(Error::Http(...))`，经 pipeline 错误处理映射为 400（`error.rs` L42）。生产级框架应返回 401——符合 ASP.NET Core 模式：无效 token 不报错，仅不设置 claims，让 StubEndpoint 触发 401。
- `init_jwt_secret`（L212-216）用 `OnceLock::set` + `.expect(...)`，同进程多个 auth 测试会 panic。需幂等化。

## Current State Analysis

### JWT 认证路径
- `crates/host/src/auth_jwt.rs` L150-168：`JwtAuth::authenticate` 解码失败返 `Err`
- `crates/host/src/auth_jwt.rs` L183-189：`AuthMiddleware::invoke` 用 `?` 传播 Err
- `crates/host/src/endpoint.rs` L88-130：StubEndpoint 检查 `auth_required_role` + `ctx.claims()`，无 claims → 401，角色不匹配 → 403
- `crates/host/tests/auth_test.rs` L32-60：`create_test_token`/`create_expired_token` 可复用模式
- `crates/host/tests/auth_test.rs` L112/130/145：3 个 `is_err()` 断言需同步调整

### Compression 路径
- `crates/core/src/http.rs` L104-123：`IHttpResponse` trait 有 `set_header` 但**无 `header()` 读方法**，有 `write_bytes` 但**无 `body_bytes()` 读方法**
- `crates/host/src/context.rs` L240：`HttpResponse.body: Option<Vec<u8>>`，replace 语义（write_bytes 覆盖）
- `crates/host/src/compression.rs` L12：`compress_gzip(data, level) -> Option<Vec<u8>>` 纯函数可复用
- `crates/host/src/pipeline.rs` L65-72：after hook 在 final_handler 后反向执行，可拿 `&mut dyn IHttpContext`

### 测试基础设施
- `crates/host/tests/integration_test.rs` L11-23：`spawn_test_host_with(port, |b| b)` 闭包模式
- `crates/host/Cargo.toml` L37-40：dev-deps 已有 `reqwest`/`criterion`/`rust-webx-macros`，**缺 `rust-webx` umbrella crate**（宏生成 `::rust_webx::RouteEntry` 路径需解析）
- `crates/webx/src/lib.rs`：umbrella crate re-export 所有类型 + 宏

## Proposed Changes

### 改动 1：AuthMiddleware 幂等化 + 401 修复

**文件**：[crates/host/src/auth_jwt.rs](file:///e:/GitCode/RF/rust-webx/crates/host/src/auth_jwt.rs)

**为什么**：
- `init_jwt_secret` 用 `OnceLock::set + expect`，同进程多测试会 panic（每个 `add_authentication()` 都调用一次）
- token 解码失败返 Err → 400，应返 Ok(None) 让 StubEndpoint 触发 401

**实现**：

L212-216 `init_jwt_secret` 改幂等：
```rust
pub fn init_jwt_secret(secret: &str) {
    let _ = JWT_ENCODING_SECRET.set(secret.to_owned());
}
```

L163-164 `authenticate` 解码失败改返 `Ok(None)`：
```rust
let token_data = match decode::<RawClaims>(&token, &self.decoding_key, &self.validation) {
    Ok(d) => d,
    Err(_) => return Ok(None),
};
```

### 改动 2：调整 auth_test.rs 断言

**文件**：[crates/host/tests/auth_test.rs](file:///e:/GitCode/RF/rust-webx/crates/host/tests/auth_test.rs)

**为什么**：改动 1 让解码失败返 `Ok(None)`，原 `is_err()` 断言会失败。

**3 处调整**（L112/130/145 附近）：
- `assert!(result.is_err(), "Invalid token should return an error")` → `assert!(result.unwrap().is_none(), "Invalid token should return None")`
- `assert!(result.is_err(), ...)` (wrong secret) → `assert!(result.unwrap().is_none(), ...)`
- `assert!(result.is_err(), "Expired token should be rejected")` → `assert!(result.unwrap().is_none(), "Expired token should return None")`

**验证**：`cargo test -p rust-webx-host --test auth_test` 全部通过。

### 改动 3：Cargo.toml 加 rust-webx dev-dep

**文件**：[crates/host/Cargo.toml](file:///e:/GitCode/RF/rust-webx/crates/host/Cargo.toml) L40 后

**为什么**：`#[get]` 宏生成 `::rust_webx::RouteEntry`/`::rust_webx::HttpMethod` 等路径，test crate 必须依赖 umbrella crate 才能解析。

**实现**：`[dev-dependencies]` 段加：
```toml
rust-webx.workspace = true
```

### 改动 4：新增 auth_integration_test.rs

**文件**：新增 [crates/host/tests/auth_integration_test.rs](file:///e:/GitCode/RF/rust-webx/crates/host/tests/auth_integration_test.rs)

**为什么**：验证 JWT 中间件 + `#[authorize]` 宏 + StubEndpoint 401/403 路径的端到端正确性。

**实现要点**：

1. 复用 `auth_test.rs` 的 token 构造模式（复制 `create_test_token`/`create_expired_token` + `TestClaims` 到本文件，避免跨 test 文件 mod 引用复杂度）
2. 在 test crate 内定义受保护 handler：
```rust
#[derive(serde::Serialize, serde::Deserialize)]
struct ProtectedReq;
#[rust_webx::get("/protected")]
#[rust_webx::authorize(role = "admin")]
struct ProtectedHandler;
#[rust_webx::handler]
impl ProtectedHandler {
    async fn handle(&self, _req: ProtectedReq) -> rust_webx::Result<String> {
        Ok("admin-area".to_string())
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct MeReq;
#[rust_webx::get("/me")]
#[rust_webx::authorize]
struct MeHandler;
#[rust_webx::handler]
impl MeHandler {
    async fn handle(&self, _req: MeReq) -> rust_webx::Result<String> {
        Ok("authenticated".to_string())
    }
}
```
3. `spawn(port)` 闭包：`std::env::set_var("APP__Jwt__Secret", "test-secret")` + `.add_authentication()` + `.no_spa()`
4. 5 个测试用例：
   - `auth_no_token_returns_401`：GET /protected 无 Authorization → 401
   - `auth_valid_admin_token_returns_200`：GET /protected + admin token → 200
   - `auth_wrong_role_returns_403`：GET /protected + user token → 403
   - `auth_expired_token_returns_401`：GET /protected + 过期 token → 401
   - `auth_authenticated_only_returns_200`：GET /me + 有效 token → 200

**验证**：`cargo test -p rust-webx-host --test auth_integration_test` 5 个测试通过。

### 改动 5：扩展 IHttpResponse trait

**文件**：[crates/core/src/http.rs](file:///e:/GitCode/RF/rust-webx/crates/core/src/http.rs) L104-123

**为什么**：CompressionMiddleware 需在 after hook 读取已写入的响应体 + content-type 头，trait 当前缺这两个读方法。与 `IHttpRequest::body_bytes`/`IHttpRequest::header` 对称扩展，非 hack。

**实现**：trait 新增两个默认方法（零破坏）：
```rust
pub trait IHttpResponse: Send {
    // ... existing methods ...

    /// Read the current response body bytes.
    ///
    /// Returns an empty Vec if no body has been written.
    /// Used by post-processing middleware (e.g. compression) in `after` hooks.
    fn body_bytes(&self) -> Vec<u8> {
        Vec::new()
    }

    /// Read a response header value by name (case-insensitive).
    fn header(&self, _key: &str) -> Option<&str> {
        None
    }
}
```

### 改动 6：HttpResponse 实现真实 body_bytes + header

**文件**：[crates/host/src/context.rs](file:///e:/GitCode/RF/rust-webx/crates/host/src/context.rs) HttpResponse impl 块

**实现**：
```rust
fn body_bytes(&self) -> Vec<u8> {
    self.body.clone().unwrap_or_default()
}

fn header(&self, key: &str) -> Option<&str> {
    self.headers.get(key).map(|s| s.as_str())
}
```

**注意**：若 headers 用 `HashMap<String, String>` 且 key 存储为小写，需在 `header()` 中做 `key.to_lowercase()` 查找。先按精确匹配实现，测试若失败再加容错。

### 改动 7：TestHttpResponse 实现 body_bytes + header

**文件**：[crates/host/tests/test_utils.rs](file:///e:/GitCode/RF/rust-webx/crates/host/tests/test_utils.rs) TestHttpResponse impl 块

**实现**：同改动 6，`body.clone().unwrap_or_default()` + headers 查找。

### 改动 8：实现 CompressionMiddleware

**文件**：[crates/host/src/compression.rs](file:///e:/GitCode/RF/rust-webx/crates/host/src/compression.rs) 末尾追加

**实现**：
```rust
use rust_webx_core::http::IHttpContext;
use rust_webx_core::middleware::IMiddleware;
use rust_webx_core::error::Result;
use std::ops::ControlFlow;

pub struct CompressionMiddleware {
    config: CompressionConfig,
}

impl Default for CompressionMiddleware {
    fn default() -> Self {
        Self { config: CompressionConfig::default() }
    }
}

impl CompressionMiddleware {
    pub fn new() -> Self { Self::default() }
    pub fn with_config(config: CompressionConfig) -> Self { Self { config } }
}

#[async_trait::async_trait]
impl IMiddleware for CompressionMiddleware {
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<ControlFlow<()>> {
        // Mark that this response may be compressed based on Accept-Encoding
        ctx.response_mut().set_header("vary", "accept-encoding");
        Ok(ControlFlow::Continue(()))
    }

    async fn after(&self, ctx: &mut dyn IHttpContext) -> Result<()> {
        // Skip if client doesn't accept gzip
        let accept = ctx.request().header("accept-encoding").unwrap_or("");
        if !accept.to_lowercase().contains("gzip") {
            return Ok(());
        }

        let body = ctx.response().body_bytes();
        if body.len() < self.config.min_size {
            return Ok(());
        }

        // Skip already-compressed content types
        let ct = ctx.response().header("content-type").unwrap_or("").to_lowercase();
        if ct.starts_with("image/") || ct.starts_with("video/") || ct.starts_with("audio/") {
            return Ok(());
        }

        if let Some(compressed) = compress_gzip(&body, self.config.level) {
            ctx.response_mut().set_header("content-encoding", "gzip");
            ctx.response_mut().write_bytes(compressed).await?;
        }
        Ok(())
    }
}
```

**注册方式**：`.use_middleware::<CompressionMiddleware>()`（默认配置）或 `.use_middleware_with(|| Arc::new(CompressionMiddleware::with_config(...)))`。

### 改动 9：扩展集成测试 — Compression

**文件**：[crates/host/tests/integration_test.rs](file:///e:/GitCode/RF/rust-webx/crates/host/tests/integration_test.rs) 末尾追加

**为什么**：验证 CompressionMiddleware 端到端行为：大响应压缩、小响应跳过、无 accept-encoding 跳过。

**实现**：3 个测试，复用 `/api/openapi.json` 端点（OpenAPI spec 通常 > 1024 字节）：

```rust
// ---------------------------------------------------------------------------
// Compression middleware
// ---------------------------------------------------------------------------

#[tokio::test]
async fn integration_compression_gzips_large_response() {
    let port = find_free_port();
    spawn_test_host_with(port, |b| {
        b.use_middleware::<rust_webx_host::compression::CompressionMiddleware>()
    })
    .await;

    let resp = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{}/api/openapi.json", port))
        .header("accept-encoding", "gzip")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    // openapi.json should be > 1024 bytes; verify compression kicked in
    assert_eq!(
        resp.headers().get("content-encoding").unwrap(),
        "gzip",
        "large response should be gzipped"
    );
    // Body should be valid JSON after decompression (reqwest auto-decompresses)
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body.get("openapi").is_some());
}

#[tokio::test]
async fn integration_compression_skips_small_response() {
    let port = find_free_port();
    spawn_test_host_with(port, |b| {
        b.use_middleware::<rust_webx_host::compression::CompressionMiddleware>()
    })
    .await;

    // /health/live returns {"status":"pass"} which is < 1024 bytes
    let resp = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{}/health/live", port))
        .header("accept-encoding", "gzip")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    assert!(
        resp.headers().get("content-encoding").is_none(),
        "small response should not be compressed"
    );
}

#[tokio::test]
async fn integration_compression_skips_without_accept_encoding() {
    let port = find_free_port();
    spawn_test_host_with(port, |b| {
        b.use_middleware::<rust_webx_host::compression::CompressionMiddleware>()
    })
    .await;

    let resp = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{}/api/openapi.json", port))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    assert!(
        resp.headers().get("content-encoding").is_none(),
        "should not compress without accept-encoding"
    );
}
```

**注意**：reqwest 默认启用 `gzip` feature 会自动解压。测试中 `resp.json()` 验证解压后仍是有效 JSON。若 reqwest 不自动解压，需用 `flate2::read::GzDecoder` 手动解压验证。

## Assumptions & Decisions

1. **AuthMiddleware 401 修复**：解码失败返 `Ok(None)` 而非 `Err`。理由：符合 ASP.NET Core 模式，无效 token 不报错仅不设置 claims，让 StubEndpoint 触发 401。这改变了 `authenticate` 的契约（从"错误"变为"无 claims"），但语义更清晰。
2. **`init_jwt_secret` 幂等化**：用 `let _ = ...set(...)` 忽略重复调用。理由：测试场景下同进程多次 build 必然多次调用，不应 panic。生产场景下多次调用也无害（secret 相同）。
3. **IHttpResponse 扩展用默认实现**：`body_bytes()` 和 `header()` 都提供默认实现返回空，破坏性为零。仅在生产 impl（HttpResponse）和测试 mock（TestHttpResponse）中添加真实实现。`MinimalResponse`（core/tests）用默认实现即可。
4. **CompressionMiddleware 注册顺序**：通过 `use_middleware` 注册，由 DI `get_all` 收集。after hook 反向执行，CompressionMiddleware 的 after 会先于 SecurityHeaders/RequestId 的 after 执行（它们无 after）。若测试发现压缩未生效，需在 `build()` 中显式排序。
5. **OpenAPI spec 大小假设**：假设 `/api/openapi.json` 返回 > 1024 字节。若实际 < 1024，`integration_compression_gzips_large_response` 测试会失败，届时改用 `/api/openapi.html`（APIUI_HTML 静态 HTML 通常更大）。
6. **header 大小写容错**：CompressionMiddleware 中 `accept-encoding` 和 `content-type` 读取都用 `to_lowercase().contains(...)` 容错。`HttpResponse.header()` 实现先按精确匹配，若测试失败再加 `to_lowercase()` 查找。
7. **test crate 内 `#[get]` 宏**：依赖 `rust-webx` umbrella crate（dev-dep）。inventory 在 test binary 内天然链接，同 binary 多个 `#[get]` 同路径会冲突，但 `auth_integration_test.rs` 与 `integration_test.rs` 是不同 binary，互不干扰。
8. **不修复 `JWT_SECRET` 文档误导**：代码实际识别 `APP__Jwt__Secret`，但文档提到 `JWT_SECRET`。这是预存在问题，不在本轮范围。

## Verification Steps

```powershell
# 1. AuthMiddleware 修复 + auth_test 调整
cargo test -p rust-webx-host --test auth_test

# 2. JWT 端到端集成测试
cargo test -p rust-webx-host --test auth_integration_test

# 3. IHttpResponse trait 扩展（确保现有测试不破坏）
cargo test -p rust-webx-core

# 4. Compression 集成测试
cargo test -p rust-webx-host --test integration_test

# 5. 全量测试
cargo test --workspace

# 6. docbit 零破坏
cargo build -p docbit-host -p docbit-handlers -p docbit-contracts
```

## 实施顺序

A、B 两个方向可并行，但内部有依赖：

**方向 A（JWT）**：
1. 改动 1：AuthMiddleware 幂等化 + 401 修复
2. 改动 2：调整 auth_test.rs 断言 → 验证 `cargo test --test auth_test` 通过
3. 改动 3：Cargo.toml 加 rust-webx dev-dep
4. 改动 4：新增 auth_integration_test.rs → 验证 5 个测试通过

**方向 B（Compression）**：
1. 改动 5：扩展 IHttpResponse trait（加默认方法）
2. 改动 6：HttpResponse 实现真实 body_bytes + header
3. 改动 7：TestHttpResponse 实现真实 body_bytes + header
4. 改动 8：实现 CompressionMiddleware → 编译验证
5. 改动 9：扩展集成测试 → 验证 3 个测试通过

**全局验证**：
- `cargo test --workspace` 无新增失败
- `cargo build -p docbit-host -p docbit-handlers -p docbit-contracts` 成功

## 完成标准

- ✅ `auth_test.rs` 全部通过（断言调整后）
- ✅ `auth_integration_test.rs` 5 个测试通过（401/200/403/401/200）
- ✅ `IHttpResponse` trait 扩展 `body_bytes()` + `header()` 默认方法
- ✅ `HttpResponse` + `TestHttpResponse` 实现真实 `body_bytes` + `header`
- ✅ `CompressionMiddleware` 实现 IMiddleware，支持 gzip 压缩
- ✅ 3 个 compression 集成测试通过（大响应压缩/小响应跳过/无 accept-encoding 跳过）
- ✅ `cargo test --workspace` 无新增失败
- ✅ docbit 应用层零破坏

## 不在本次范围

- `JWT_SECRET` 环境变量文档修复（预存在问题）
- CompressionMiddleware 的 deflate/brotli 支持（gzip 已够生产级）
- AuthMiddleware 的 WWW-Authenticate 响应头（401 时应返回，但不在本轮）
- DynamicAuthorizer 端到端测试（需设计资源/动作模型）
- docbit 健康探针配置（应用层决策）
- after hook 顺序显式排序（当前依赖 DI 收集顺序，若稳定则不改）

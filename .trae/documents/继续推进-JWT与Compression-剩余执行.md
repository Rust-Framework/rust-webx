# 继续推进：JWT 测试 + Compression 中间件（剩余执行）

## Summary

上轮已批准 plan `生产级推进-JWT测试与Compression中间件.md`，其中**改动 1（AuthMiddleware 幂等化 + 401 修复）已完成并编译通过**。本 plan 继续执行剩余 8 项改动（改动 2-9），覆盖 JWT 端到端集成测试与 CompressionMiddleware 实现。

## Current State（已验证）

### 已完成 ✅
- [crates/host/src/auth_jwt.rs](file:///e:/GitCode/RF/rust-webapp/crates/host/src/auth_jwt.rs)
  - L163-166：`authenticate` 解码失败返 `Ok(None)`（不再返 `Err`）
  - L213-215：`init_jwt_secret` 幂等化（`let _ = JWT_ENCODING_SECRET.set(...)`）
  - 文档注释已从 `# Panics` 改为 `Idempotent: subsequent calls are no-ops`

### 待执行（本 plan 范围）
- 改动 2：调整 `auth_test.rs` 3 处 `is_err()` 断言
- 改动 3：`Cargo.toml` 加 `rust-webapp` dev-dep
- 改动 4：新增 `auth_integration_test.rs`（5 个 JWT 端到端测试）
- 改动 5：扩展 `IHttpResponse` trait（`body_bytes()` + `header()` 默认方法）
- 改动 6：`HttpResponse` 实现真实 `body_bytes` + `header`
- 改动 7：`TestHttpResponse` 实现真实 `body_bytes` + `header`
- 改动 8：实现 `CompressionMiddleware`
- 改动 9：扩展集成测试 — Compression（3 个测试）

## Proposed Changes

### 改动 2：调整 auth_test.rs 断言

**文件**：[crates/host/tests/auth_test.rs](file:///e:/GitCode/RF/rust-webapp/crates/host/tests/auth_test.rs)

**为什么**：改动 1 让解码失败返 `Ok(None)`，原 `is_err()` 断言会失败。

**3 处调整**：

1. **L104-113** `auth_invalid_token_returns_error` 测试：
   - 测试名改为 `auth_invalid_token_returns_none`（语义对齐）
   - L112：`assert!(result.is_err(), "Invalid token should return an error")` → `assert!(result.unwrap().is_none(), "Invalid token should return None")`

2. **L115-133** `auth_wrong_secret_key_fails` 测试：
   - 测试名改为 `auth_wrong_secret_key_returns_none`
   - L128-132：`assert!(result.is_err(), "Token signed with different key should fail")` → `assert!(result.unwrap().is_none(), "Token signed with different key should return None")`

3. **L135-146** `auth_expired_token_fails` 测试：
   - 测试名改为 `auth_expired_token_returns_none`
   - L145：`assert!(result.is_err(), "Expired token should be rejected")` → `assert!(result.unwrap().is_none(), "Expired token should return None")`

**验证**：`cargo test -p rust-webapp-host --test auth_test` 全部通过。

---

### 改动 3：Cargo.toml 加 rust-webapp dev-dep

**文件**：[crates/host/Cargo.toml](file:///e:/GitCode/RF/rust-webapp/crates/host/Cargo.toml) L40 后

**为什么**：`#[get]`/`#[handler]` 宏生成 `::rust_webapp::RouteEntry`/`::rust_webapp::HttpMethod`/`::rust_webapp::HandlerCache` 等路径（已通过 Grep 确认 [crates/macros/src/endpoint.rs](file:///e:/GitCode/RF/rust-webapp/crates/macros/src/endpoint.rs) L130/131/225），test crate 必须依赖 umbrella crate 才能解析。

**实现**：`[dev-dependencies]` 段末尾追加：
```toml
rust-webapp.workspace = true
```

---

### 改动 4：新增 auth_integration_test.rs

**文件**：新增 [crates/host/tests/auth_integration_test.rs](file:///e:/GitCode/RF/rust-webapp/crates/host/tests/auth_integration_test.rs)

**为什么**：验证 JWT 中间件 + `#[authorize]` 宏 + StubEndpoint 401/403 路径的端到端正确性。当前 `auth_test.rs` 仅单元测试 `JwtAuth`，未覆盖 HTTP 端到端路径。

**实现要点**：

1. **token 构造**：复制 `auth_test.rs` 的 `TestClaims` + `create_test_token` + `create_expired_token` + `now_plus_seconds`/`now_minus_seconds` 到本文件（避免跨 test 文件 mod 引用复杂度）

2. **受保护 handler 定义**（test crate 内，inventory 同 binary 天然链接）：
```rust
#[derive(serde::Serialize, serde::Deserialize)]
struct ProtectedReq;

#[rust_webapp::get("/protected")]
#[rust_webapp::authorize(role = "admin")]
struct ProtectedHandler;

#[rust_webapp::handler]
impl ProtectedHandler {
    async fn handle(&self, _req: ProtectedReq) -> rust_webapp::Result<String> {
        Ok("admin-area".to_string())
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct MeReq;

#[rust_webapp::get("/me")]
#[rust_webapp::authorize]
struct MeHandler;

#[rust_webapp::handler]
impl MeHandler {
    async fn handle(&self, _req: MeReq) -> rust_webapp::Result<String> {
        Ok("authenticated".to_string())
    }
}
```

3. **spawn 闭包**：复用 `integration_test.rs` 的 `spawn_test_host_with` 模式，但需在本文件内重新定义（test binary 独立）：
```rust
async fn spawn_auth_host(port: u16) {
    let addr = format!("127.0.0.1:{}", port);
    // 双下划线分段配置：Jwt__Secret → APP__Jwt__Secret 环境变量
    std::env::set_var("APP__Jwt__Secret", "test-secret-key-for-integration");
    let builder = rust_webapp_host::server::Host::builder()
        .mode(rust_webapp_core::mode::AppMode::Development)
        .no_spa()
        .add_authentication();
    let host = builder.build();
    tokio::spawn(async move { host.run_at(&addr).await.unwrap() });
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
}
```

4. **5 个测试用例**：
   - `auth_no_token_returns_401`：GET /protected 无 Authorization → 401
   - `auth_valid_admin_token_returns_200`：GET /protected + admin token → 200
   - `auth_wrong_role_returns_403`：GET /protected + user token → 403
   - `auth_expired_token_returns_401`：GET /protected + 过期 token → 401
   - `auth_authenticated_only_returns_200`：GET /me + 有效 token（无角色要求）→ 200

**验证**：`cargo test -p rust-webapp-host --test auth_integration_test` 5 个测试通过。

---

### 改动 5：扩展 IHttpResponse trait

**文件**：[crates/core/src/http.rs](file:///e:/GitCode/RF/rust-webapp/crates/core/src/http.rs) L104-123

**为什么**：CompressionMiddleware 需在 after hook 读取已写入的响应体 + content-type 头。当前 trait 有 `set_header` 但无 `header()` 读方法，有 `write_bytes` 但无 `body_bytes()` 读方法。与 `IHttpRequest::body_bytes`/`IHttpRequest::header` 对称扩展。

**实现**：trait 新增两个默认方法（零破坏，现有 impl 不需改动）：
```rust
#[async_trait::async_trait]
pub trait IHttpResponse: Send {
    // ... existing methods (status, set_status, set_header, has_body, write_bytes, write_text) ...

    /// Read the current response body bytes.
    ///
    /// Returns an empty Vec if no body has been written.
    /// Used by post-processing middleware (e.g. compression) in `after` hooks.
    fn body_bytes(&self) -> Vec<u8> {
        Vec::new()
    }

    /// Read a response header value by name.
    ///
    /// Returns None if the header is not set.
    /// Used by post-processing middleware (e.g. compression) to inspect content-type.
    fn header(&self, _key: &str) -> Option<&str> {
        None
    }
}
```

---

### 改动 6：HttpResponse 实现真实 body_bytes + header

**文件**：[crates/host/src/context.rs](file:///e:/GitCode/RF/rust-webapp/crates/host/src/context.rs) L240-266 HttpResponse impl 块

**为什么**：生产 impl 需提供真实读取能力，供 CompressionMiddleware after hook 使用。

**实现**：在 `impl IHttpResponse for HttpResponse` 块中追加：
```rust
fn body_bytes(&self) -> Vec<u8> {
    self.body.clone().unwrap_or_default()
}

fn header(&self, key: &str) -> Option<&str> {
    self.headers.get(key).map(|s| s.as_str())
}
```

**注意**：`HttpResponse.headers` 是 `HashMap<String, String>`，key 存储时已是原始大小写（`set_header` 直接 insert）。CompressionMiddleware 读取时用 `to_lowercase().contains(...)` 容错。若测试发现 header 查找失败，再考虑 `to_lowercase()` 归一化。

---

### 改动 7：TestHttpResponse 实现真实 body_bytes + header

**文件**：[crates/host/tests/test_utils.rs](file:///e:/GitCode/RF/rust-webapp/crates/host/tests/test_utils.rs) L152-179 TestHttpResponse impl 块

**为什么**：测试 mock 需同步实现，保持与生产 impl 一致。

**实现**：在 `impl IHttpResponse for TestHttpResponse` 块中追加：
```rust
fn body_bytes(&self) -> Vec<u8> {
    self.body.clone().unwrap_or_default()
}

fn header(&self, key: &str) -> Option<&str> {
    self.headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.as_str())
}
```

**注意**：`TestHttpResponse.headers` 是 `Vec<(String, String)>`（因 set_header 用 push 而非 insert），需用 `iter().find()` + `eq_ignore_ascii_case` 容错查找。

---

### 改动 8：实现 CompressionMiddleware

**文件**：[crates/host/src/compression.rs](file:///e:/GitCode/RF/rust-webapp/crates/host/src/compression.rs) 末尾追加

**为什么**：补齐生产级中间件矩阵。当前仅有 `compress_gzip` 工具函数，未实现 `IMiddleware` trait。

**实现**：
```rust
use rust_webapp_core::error::Result;
use rust_webapp_core::http::IHttpContext;
use rust_webapp_core::middleware::IMiddleware;
use std::ops::ControlFlow;

pub struct CompressionMiddleware {
    config: CompressionConfig,
}

impl Default for CompressionMiddleware {
    fn default() -> Self {
        Self {
            config: CompressionConfig::default(),
        }
    }
}

impl CompressionMiddleware {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_config(config: CompressionConfig) -> Self {
        Self { config }
    }
}

#[async_trait::async_trait]
impl IMiddleware for CompressionMiddleware {
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<ControlFlow<()>> {
        ctx.response_mut().set_header("vary", "accept-encoding");
        Ok(ControlFlow::Continue(()))
    }

    async fn after(&self, ctx: &mut dyn IHttpContext) -> Result<()> {
        let accept = ctx.request().header("accept-encoding").unwrap_or("");
        if !accept.to_lowercase().contains("gzip") {
            return Ok(());
        }

        let body = ctx.response().body_bytes();
        if body.len() < self.config.min_size {
            return Ok(());
        }

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

---

### 改动 9：扩展集成测试 — Compression

**文件**：[crates/host/tests/integration_test.rs](file:///e:/GitCode/RF/rust-webapp/crates/host/tests/integration_test.rs) 末尾追加

**为什么**：验证 CompressionMiddleware 端到端行为：大响应压缩、小响应跳过、无 accept-encoding 跳过。

**实现**：3 个测试，复用 `/api/openapi.json`（OpenAPI spec 通常 > 1024 字节）和 `/health/live`（`{"status":"pass"}` < 1024 字节）：

```rust
// ---------------------------------------------------------------------------
// Compression middleware
// ---------------------------------------------------------------------------

#[tokio::test]
async fn integration_compression_gzips_large_response() {
    let port = find_free_port();
    spawn_test_host_with(port, |b| {
        b.use_middleware::<rust_webapp_host::compression::CompressionMiddleware>()
    })
    .await;

    let resp = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{}/api/openapi.json", port))
        .header("accept-encoding", "gzip")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(
        resp.headers().get("content-encoding").unwrap(),
        "gzip",
        "large response should be gzipped"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body.get("openapi").is_some());
}

#[tokio::test]
async fn integration_compression_skips_small_response() {
    let port = find_free_port();
    spawn_test_host_with(port, |b| {
        b.use_middleware::<rust_webapp_host::compression::CompressionMiddleware>()
    })
    .await;

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
        b.use_middleware::<rust_webapp_host::compression::CompressionMiddleware>()
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

**注意**：reqwest 默认启用 `gzip` feature 会自动解压。`resp.json()` 验证解压后仍是有效 JSON。若 reqwest 未自动解压，需用 `flate2::read::GzDecoder` 手动解压验证。

## Assumptions & Decisions

1. **AuthMiddleware 401 修复已完成**：改动 1 在上轮已实施并编译通过，本 plan 不再重复。
2. **`init_jwt_secret` 幂等化**：已改为 `let _ = ...set(...)`，多测试场景安全。
3. **IHttpResponse 扩展用默认实现**：`body_bytes()` 和 `header()` 都提供默认实现返回空，破坏性为零。仅 `HttpResponse`（生产）和 `TestHttpResponse`（测试）添加真实实现。
4. **CompressionMiddleware 注册顺序**：通过 `use_middleware` 注册，由 DI `get_all` 收集。after hook 反向执行，CompressionMiddleware 的 after 会先于 SecurityHeaders/RequestId 的 after 执行（它们无 after 实现，用默认 no-op）。
5. **OpenAPI spec 大小假设**：假设 `/api/openapi.json` 返回 > 1024 字节。若实际 < 1024，`integration_compression_gzips_large_response` 会失败，届时改用 `/api/openapi.html`。
6. **header 大小写容错**：CompressionMiddleware 中 `accept-encoding` 和 `content-type` 读取都用 `to_lowercase().contains(...)` 容错。`TestHttpResponse.header()` 用 `eq_ignore_ascii_case` 容错。
7. **test crate 内 `#[get]` 宏**：依赖 `rust-webapp` umbrella crate（dev-dep）。inventory 在 test binary 内天然链接，同 binary 多个 `#[get]` 同路径会冲突，但 `auth_integration_test.rs` 与 `integration_test.rs` 是不同 binary，互不干扰。
8. **`APP__Jwt__Secret` 环境变量**：双下划线分段配置注入（`Jwt:Secret` → `APP__Jwt__Secret`），非 `JWT_SECRET`。
9. **reqwest gzip 自动解压**：reqwest 默认启用 `gzip` feature，会自动解压 `content-encoding: gzip` 的响应体，`resp.json()` 直接拿到解压后的 JSON。

## Verification Steps

```powershell
# 1. AuthMiddleware 修复 + auth_test 调整
cargo test -p rust-webapp-host --test auth_test

# 2. JWT 端到端集成测试
cargo test -p rust-webapp-host --test auth_integration_test

# 3. IHttpResponse trait 扩展（确保现有测试不破坏）
cargo test -p rust-webapp-core

# 4. Compression 集成测试
cargo test -p rust-webapp-host --test integration_test

# 5. 全量测试
cargo test --workspace

# 6. docbit 零破坏
cargo build -p docbit-host -p docbit-handlers -p docbit-contracts
```

## 实施顺序

**方向 A（JWT）**：
1. 改动 2：调整 auth_test.rs 断言 → 验证 `cargo test --test auth_test` 通过
2. 改动 3：Cargo.toml 加 rust-webapp dev-dep
3. 改动 4：新增 auth_integration_test.rs → 验证 5 个测试通过

**方向 B（Compression）**：
4. 改动 5：扩展 IHttpResponse trait（加默认方法）
5. 改动 6：HttpResponse 实现真实 body_bytes + header
6. 改动 7：TestHttpResponse 实现真实 body_bytes + header
7. 改动 8：实现 CompressionMiddleware → 编译验证
8. 改动 9：扩展集成测试 → 验证 3 个测试通过

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

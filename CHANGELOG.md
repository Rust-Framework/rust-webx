# Changelog

All notable changes to **rust-webx** are documented in this file.

## [0.3.1] — 2026-08-16 — crates.io 再发布对齐 · Re-release alignment

> **English** · **简体中文**

### English

The `rust-webx` crate family has been re-published to crates.io and is now managed
under the `Rust-Framework` organization.

### Changed

- **crates.io re-publish**: `rust-webx` / `rust-webx-core` / `rust-webx-host` /
  `rust-webx-macros` / `rust-webx-spa` / `rust-webx-openapi` aligned; repository
  metadata (`license`, `repository`, `documentation`) aligned with the GitHub repo.
- **Automated publishing**: new GitHub Actions `publish.yml` publishes crates in
  dependency order on `v*` tag push.
- **Docs landing page**: added `docs/README.md` bilingual navigation.

> Maintenance release; the runtime API is unchanged.

### 简体中文

- **crates.io 再发布**：`rust-webx` / `rust-webx-core` / `rust-webx-host` / `rust-webx-macros`
  / `rust-webx-spa` / `rust-webx-openapi` 全系重新发布到 crates.io，归入 `Rust-Framework`
  组织统一管理；仓库元数据（`license`、`repository`、`documentation`）对齐 GitHub 仓库。
- **自动化发布**：新增 GitHub Actions `publish.yml`，推送 `v*` tag 时按依赖顺序自动发布。
- **文档落地页**：新增 `docs/README.md` 中英文档导航入口。

> 本版本为发布维护迭代，不改变运行时 API。

## [0.3.0] — 2026-07-09

### Changed (Breaking)

- **`HandlerRegistration.factory` / `HandlerEntry.factory`** signature: `fn(&dyn IServiceResolver) -> Box<dyn Any + Send>` → `fn(&dyn IServiceResolver) -> Result<Box<dyn Any + Send>>`. The `#[handler]` macro generates the new signature automatically; only manual `HandlerRegistration` constructions need updating.

### Fixed

- **Cache stampede**: `get_or_create` / `get_or_try_create` use per-key mutex + double-check to prevent thundering herd under high concurrency.
- **`MemoryCache` lock contention**: `get` / `exists` use read-lock-first (clone data → drop → short write lock for refresh) instead of write-lock for every read.
- **`MemoryCache` eviction**: FIFO via `VecDeque` replaces random `keys().next()` for predictable, fair eviction.
- **Macro panics**: `#[handler]`-generated factory/call functions return `Result` instead of `panic!`/`expect` on downcast failure.
- **`RequestIdMiddleware`**: propagates upstream `x-request-id` header instead of always generating a new UUID.
- **413 short-circuit**: `handle_request` skips the pipeline when `HttpContext::new` already set a 4xx response (prevents deserialization errors from overwriting 413 Payload Too Large).
- **`RequestTracing`**: propagates upstream `x-request-id` as `x-trace-id` (lock-free `AtomicU64` for sequence).
- Various clippy warnings resolved across macro-generated code and test scaffolding.

### Added

- Integration test suite (`request_path_test.rs`) covering 16 request-processing paths: GET/POST/PUT/DELETE, path params, JSON body, 400/404/405/413/422/500 status mapping, x-request-id propagation, unit-response 204.

## [0.2.1] — 2026-07-08

### Changed

- **Unified dispatch**: HTTP endpoints and `Mediator::send` share `dispatch::dispatch` (HandlerCache → scope → pipeline → handler).
- **Handler lookup**: `HandlerRegistration` adds `req_type_id: TypeId` for reliable in-process dispatch.
- **`add_mediator()`**: registers `Mediator` as transient (DI-injectable after host build).
- **`HandlerRegistry`**: type alias for `HandlerCache`.

### Fixed

- docbit `DocService` resolves monorepo docs at `<workspace>/docs` when `<app_base>/docs` is absent.
- docbit startup recreates SQLite schema on datatype mismatch; skips doc index when docs dir missing.

## [0.2.0] — 2026-07-08

### Changed

- **Rebrand**: crate series renamed from `rust-webapp` to `rust-webx` (`rust_webx` import path).
- **DI**: upgraded to `rust-dix 0.6` (formerly `rust-dicore 0.5`); `build()` returns `Arc<ServiceProvider>`; `get()` / `get_owned()` return `Result`.
- **ORM**: upgraded to `rust-ef 1.5.1` (+ `rust-ef-sqlite`, `rust-ef-mysql` from crates.io).
- Removed local `[patch.crates-io]` overrides for rust-ef; all ecosystem crates resolve from crates.io.

### Fixed

- `ScopeFactory` trait import for per-request DI scopes (`create_scope()`).
- Mediator / host tests adapted to rust-dix 0.6 `ServiceProvider` API.

### Migration — 0.1.x → 0.2.0

1. `Cargo.toml`: `rust-webapp = "0.1"` → `rust-webx = "0.2"`.
2. `use rust_webapp::*` → `use rust_webx::*`.
3. `rust_dicore` → `rust_dix`; `rust-dicore` → `rust-dix`.
4. Remove `Arc::new()` around `ServiceCollection::build()`; handle `Result` from `get()` / `get_owned()`.
5. `rust-ef = "1.5.1"` with provider crates on crates.io (no path patch).

## [0.2.0] — production readiness (docbit)

### Added

- `JWT_SECRET` environment variable support (overridden by `APP__Jwt__Secret`).
- Production fail-fast when JWT secret is missing, short, or a known placeholder.
- docbit production middleware stack: rate limit, compression, timing, request tracing.
- `docbit/Dockerfile`, `docbit/docker-compose.yml`, `docbit/.env.example`, `docbit/publish.sh`, `docbit/PRODUCTION.md`.
- CI job: Docker build for docbit.

### Framework production fixes (0.2.0)

- **Dynamic health checks**: `/health` and `/health/ready` evaluate probes per request; `fail` returns HTTP 503.
- **SIGTERM**: Unix graceful shutdown via tokio signal listener (alongside Ctrl+C).
- **`IHost::stop()`** and **`run_at()`** now trigger shutdown and run hosted service lifecycle.
- **Production fail-fast** for CORS wildcard `*`.
- **OpenAPI UI** registered only in Development mode.
- Tests: runtime health probe, production guard panics, `APP__Jwt__Secret` precedence.

### Framework production fixes (0.2.0) — continued

- **Unified error format**: 401/403/429/413 统一为 RFC 7807 `application/problem+json`（`problem_response` 模块）。
- **SIGTERM / shutdown tests** + **TLS HTTPS 集成测试**（rcgen 自签证书）。
- **docs/rust-webx** 批量更新：`rust_dix`、`add_memory_cache`、TLS/health API。

### Framework P2 (0.2.0)

- **RateLimit appsettings**：`RateLimit.Enabled/RequestsPerSecond/BurstSize/MaxTrackedIps`，build 时自动注册中间件。
- **Rate limit LRU**：超过 `MaxTrackedIps` 时淘汰最久未刷新的 IP bucket。
- **`GET /metrics`**：Prometheus text 格式（`Metrics.Enabled`）。
- docbit Production 改用 appsettings 配置 RateLimit/Metrics。
- 测试：openapi spec、spa 工具函数、metrics 集成、rate limit LRU。

### Changed

- `appsettings.Production.json`: JWT secret removed from file; must be supplied via env.

### Known limitations (0.2.0)

- **OpenTelemetry export**: not built-in; use structured JSON logs (Production), `RequestTracing` middleware, and optional `GET /metrics` (Prometheus). OTLP planned for a future minor release.
- **rust-ef insert ID**: `save_changes` does not backfill auto-increment IDs (1.5.1); docbit handlers re-query by natural keys (documented as `FIXME(upstream)`).

## [0.1.0] — 2026-06

Initial release as `rust-webapp` (superseded by 0.2.0 rebrand).

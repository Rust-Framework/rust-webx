# Changelog

All notable changes to **rust-webx** are documented in this file.


## [0.3.3] — 2026-08-31 — Docbit cross-compile and docs

> **English** · **简体中文**

### English

Maintenance release: docbit-host cross-compile tooling, ecosystem docs publishing, and documentation polish. Published crate APIs are unchanged from 0.3.2.

#### Changed

- **docbit-host**: use `native-tls` with vendored features instead of `openssl-sys` for Windows → linux-gnu cross builds.
- **Docbit publish**: copy ecosystem docs from source repos; PowerShell 5.1 UTF-8 BOM on scripts; Path wildcard fix for doc copy.
- **Docs**: standardized `INDEX.json` metadata; exhibition titles and descriptions in docs seed.

### 简体中文

维护版本：docbit-host 交叉编译、生态文档发布与文档润色。已发布 crate 的 API 与 0.3.2 一致。

#### 变更

- **docbit-host**：交叉编译改用 `native-tls` vendored。
- **Docbit 发布脚本**与**文档 INDEX/展览**更新。
## [0.3.2] 鈥?2026-08-31 鈥?Architecture remediation Phases 1鈥?

> **English** 路 **绠€浣撲腑鏂?*

### English

Breaking and behavioral changes from the architecture remediation (Phases 1鈥?). Migration guide: [docs/rust-webx/16-migration/global-state.md](docs/rust-webx/16-migration/global-state.md) and [docs/ARCHITECTURE_REMEDIATION.md](docs/ARCHITECTURE_REMEDIATION.md).

#### Breaking

- **Orphan routes/handlers fail at startup** 鈥?`HostBuilder::build()` panics when a route lacks `#[handler]`, a handler lacks a route, or duplicate `#[handler]` registrations exist. Run `cargo run -p <host> -- --doctor` before deploy.
- **SPA no longer serves `/api/*` unknown paths** 鈥?unmatched API routes return 404/501 from the router, not `index.html`.
- **`global_provider()` / `set_global_provider()` deprecated** 鈥?use `host.provider()` or `dispatch_provider()` inside `DispatchRuntime` scope. `Host::build()` no longer sets process-wide provider.
- **`register_handlers!` deprecated for HTTP** 鈥?inventory + `#[handler]` is the sole HTTP registration path; macro retained for Mediator-only scenarios.

#### Added

- **`DispatchRuntime`** on each `Host` 鈥?instance-scoped provider + `HandlerCache`; HTTP and `IHostedService::start` run inside `dispatch_runtime().run()`.
- **Query binding** 鈥?GET/DELETE merges route + query params via `Deserialize`.
- **`#[authorize(permission = "鈥?)]`** 鈥?parsed into route metadata; Resource Auth via `use_resource_authorization()`.
- **`#[derive(WebxRequestMeta)]`** 鈥?OpenAPI query/path/body param metadata from field attributes (`#[from_query]`, `#[from_route]`, `#[from_body]`).
- **Route diagnostics** 鈥?`--doctor` reports orphan routes/handlers and duplicate registrations with fix hints.

#### Changed

- **Middleware order** 鈥?CORS 鈫?JWT 鈫?SPA 鈫?Router; `SpaMiddleware` skips `/api/*`.
- **Stub endpoints** 鈥?without dispatch return **501** (RFC 7807), not silent 200 stubs.
- **Docbit/dmbit** 鈥?`DbInitService` uses `dispatch_provider()`; GET DTOs use `WebxRequestMeta`.

#### Documented (not changed)

- **`jwt_secret()`** remains a process-wide config shim (separate from DI); see security-best-practices.md.

### 绠€浣撲腑鏂?

鏋舵瀯鏁存敼 Phase 1鈥? 鐨勭牬鍧忔€у彉鏇翠笌琛屼负璋冩暣銆傝縼绉昏鍙傞槄 [鍏ㄥ眬鐘舵€佽縼绉籡(docs/rust-webx/16-migration/global-state.md) 涓?[ARCHITECTURE_REMEDIATION.md](docs/ARCHITECTURE_REMEDIATION.md)銆?

#### 鐮村潖鎬у彉鏇?

- **瀛ゅ効璺敱/Handler 鍚姩鍗?panic** 鈥?杩愯 `cargo run -p <host> -- --doctor` 鎺掓煡銆?
- **`/api/*` 鏈尮閰嶈矾寰勪笉鍐嶈繑鍥?SPA `index.html`**銆?
- **`global_provider()` 宸插純鐢?* 鈥?鏀圭敤 `dispatch_provider()` / `host.provider()`銆?
- **HTTP 涓嶅啀浠?`register_handlers!` 涓轰富璺緞** 鈥?浣跨敤 inventory + `#[handler]`銆?

#### 鏂板

- **`DispatchRuntime`**銆?*Query 缁戝畾**銆?*`#[authorize(permission)]`**銆?*`WebxRequestMeta`**銆?*`--doctor` 璺敱璇婃柇**銆?

#### 鍙樻洿

- 涓棿浠堕『搴忋€丼tub 501銆丏ocbit GET DTO OpenAPI 鍏冩暟鎹€?

## [0.3.1] 鈥?2026-08-16 鈥?crates.io 鍐嶅彂甯冨榻?路 Re-release alignment

> **English** 路 **绠€浣撲腑鏂?*

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

### 绠€浣撲腑鏂?

- **crates.io 鍐嶅彂甯?*锛歚rust-webx` / `rust-webx-core` / `rust-webx-host` / `rust-webx-macros`
  / `rust-webx-spa` / `rust-webx-openapi` 鍏ㄧ郴閲嶆柊鍙戝竷鍒?crates.io锛屽綊鍏?`Rust-Framework`
  缁勭粐缁熶竴绠＄悊锛涗粨搴撳厓鏁版嵁锛坄license`銆乣repository`銆乣documentation`锛夊榻?GitHub 浠撳簱銆?
- **鑷姩鍖栧彂甯?*锛氭柊澧?GitHub Actions `publish.yml`锛屾帹閫?`v*` tag 鏃舵寜渚濊禆椤哄簭鑷姩鍙戝竷銆?
- **鏂囨。钀藉湴椤?*锛氭柊澧?`docs/README.md` 涓嫳鏂囨。瀵艰埅鍏ュ彛銆?

> 鏈増鏈负鍙戝竷缁存姢杩唬锛屼笉鏀瑰彉杩愯鏃?API銆?

## [0.3.0] 鈥?2026-07-09

### Changed (Breaking)

- **`HandlerRegistration.factory` / `HandlerEntry.factory`** signature: `fn(&dyn IServiceResolver) -> Box<dyn Any + Send>` 鈫?`fn(&dyn IServiceResolver) -> Result<Box<dyn Any + Send>>`. The `#[handler]` macro generates the new signature automatically; only manual `HandlerRegistration` constructions need updating.

### Fixed

- **Cache stampede**: `get_or_create` / `get_or_try_create` use per-key mutex + double-check to prevent thundering herd under high concurrency.
- **`MemoryCache` lock contention**: `get` / `exists` use read-lock-first (clone data 鈫?drop 鈫?short write lock for refresh) instead of write-lock for every read.
- **`MemoryCache` eviction**: FIFO via `VecDeque` replaces random `keys().next()` for predictable, fair eviction.
- **Macro panics**: `#[handler]`-generated factory/call functions return `Result` instead of `panic!`/`expect` on downcast failure.
- **`RequestIdMiddleware`**: propagates upstream `x-request-id` header instead of always generating a new UUID.
- **413 short-circuit**: `handle_request` skips the pipeline when `HttpContext::new` already set a 4xx response (prevents deserialization errors from overwriting 413 Payload Too Large).
- **`RequestTracing`**: propagates upstream `x-request-id` as `x-trace-id` (lock-free `AtomicU64` for sequence).
- Various clippy warnings resolved across macro-generated code and test scaffolding.

### Added

- Integration test suite (`request_path_test.rs`) covering 16 request-processing paths: GET/POST/PUT/DELETE, path params, JSON body, 400/404/405/413/422/500 status mapping, x-request-id propagation, unit-response 204.

## [0.2.1] 鈥?2026-07-08

### Changed

- **Unified dispatch**: HTTP endpoints and `Mediator::send` share `dispatch::dispatch` (HandlerCache 鈫?scope 鈫?pipeline 鈫?handler).
- **Handler lookup**: `HandlerRegistration` adds `req_type_id: TypeId` for reliable in-process dispatch.
- **`add_mediator()`**: registers `Mediator` as transient (DI-injectable after host build).
- **`HandlerRegistry`**: type alias for `HandlerCache`.

### Fixed

- docbit `DocService` resolves monorepo docs at `<workspace>/docs` when `<app_base>/docs` is absent.
- docbit startup recreates SQLite schema on datatype mismatch; skips doc index when docs dir missing.

## [0.2.0] 鈥?2026-07-08

### Changed

- **Rebrand**: crate series renamed from `rust-webapp` to `rust-webx` (`rust_webx` import path).
- **DI**: upgraded to `rust-dix 0.6` (formerly `rust-dicore 0.5`); `build()` returns `Arc<ServiceProvider>`; `get()` / `get_owned()` return `Result`.
- **ORM**: upgraded to `rust-ef 1.5.1` (+ `rust-ef-sqlite`, `rust-ef-mysql` from crates.io).
- Removed local `[patch.crates-io]` overrides for rust-ef; all ecosystem crates resolve from crates.io.

### Fixed

- `ScopeFactory` trait import for per-request DI scopes (`create_scope()`).
- Mediator / host tests adapted to rust-dix 0.6 `ServiceProvider` API.

### Migration 鈥?0.1.x 鈫?0.2.0

1. `Cargo.toml`: `rust-webapp = "0.1"` 鈫?`rust-webx = "0.2"`.
2. `use rust_webapp::*` 鈫?`use rust_webx::*`.
3. `rust_dicore` 鈫?`rust_dix`; `rust-dicore` 鈫?`rust-dix`.
4. Remove `Arc::new()` around `ServiceCollection::build()`; handle `Result` from `get()` / `get_owned()`.
5. `rust-ef = "1.5.1"` with provider crates on crates.io (no path patch).

## [0.2.0] 鈥?production readiness (docbit)

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

### Framework production fixes (0.2.0) 鈥?continued

- **Unified error format**: 401/403/429/413 缁熶竴涓?RFC 7807 `application/problem+json`锛坄problem_response` 妯″潡锛夈€?
- **SIGTERM / shutdown tests** + **TLS HTTPS 闆嗘垚娴嬭瘯**锛坮cgen 鑷璇佷功锛夈€?
- **docs/rust-webx** 鎵归噺鏇存柊锛歚rust_dix`銆乣add_memory_cache`銆乀LS/health API銆?

### Framework P2 (0.2.0)

- **RateLimit appsettings**锛歚RateLimit.Enabled/RequestsPerSecond/BurstSize/MaxTrackedIps`锛宐uild 鏃惰嚜鍔ㄦ敞鍐屼腑闂翠欢銆?
- **Rate limit LRU**锛氳秴杩?`MaxTrackedIps` 鏃舵窐姹版渶涔呮湭鍒锋柊鐨?IP bucket銆?
- **`GET /metrics`**锛歅rometheus text 鏍煎紡锛坄Metrics.Enabled`锛夈€?
- docbit Production 鏀圭敤 appsettings 閰嶇疆 RateLimit/Metrics銆?
- 娴嬭瘯锛歰penapi spec銆乻pa 宸ュ叿鍑芥暟銆乵etrics 闆嗘垚銆乺ate limit LRU銆?

### Changed

- `appsettings.Production.json`: JWT secret removed from file; must be supplied via env.

### Known limitations (0.2.0)

- **OpenTelemetry export**: not built-in; use structured JSON logs (Production), `RequestTracing` middleware, and optional `GET /metrics` (Prometheus). OTLP planned for a future minor release.
- **rust-ef insert ID**: `save_changes` does not backfill auto-increment IDs (1.5.1); docbit handlers re-query by natural keys (documented as `FIXME(upstream)`).

## [0.1.0] 鈥?2026-06

Initial release as `rust-webapp` (superseded by 0.2.0 rebrand).

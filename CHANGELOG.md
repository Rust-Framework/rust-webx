# Changelog

All notable changes to **rust-webx** are documented in this file.


## [3.0.0] — unreleased — 契约类型与统一错误载体（3.0 前两批）

> **English** · **简体中文** · 迁移指南：[docs/rust-webx/16-migration/upgrade-to-3.0.md](docs/rust-webx/16-migration/upgrade-to-3.0.md)

The pagination, cursor and business-envelope contracts now come from the shared
**`rust-core`** crate instead of `rust-webx-core`. The 2.9.x definitions are deleted; the
historical `webx_core::pagination` / `::keyset` / `::operation` module paths keep working as
re-exports. The same 3.0 batch also replaces the error enum with the shared
`pub type Error = rust_core::AppError` (see below). The API-convergence batches remain separate.

#### English

##### Error = `AppError` (second batch)

`webx_core::Error` is now `pub type Error = rust_core::AppError`; the 2.9 enum is deleted. The
`ProblemDetails ↔ AppError` bridge preserves the pre-built problem wire shape, and the default
writer renders every error through it.

- **Constructors are snake_case**: `Validation` → `validation`, `Internal` → `internal`,
  `Status` → `status`, `Http` → `bad_request`, `Routing` → `route_not_found`,
  `Serialization` → `invalid_json`, `ResponseSerialization` → `response_serialization`,
  `TooManyRequests { .. }` → `too_many_requests(secs, detail)`, `ConflictRetryable` →
  `conflict_retryable`, `Message` → `internal`.
- **Pattern matches become accessors**: use `err.kind()`, `err.code()`, `err.status_code()`,
  `err.public_detail()`, `err.operator()`, `err.fields()`, `err.extensions()`. An old
  `Error::Problem(problem)` construction becomes `problem.into()`.
- **`Display` changed**: it prints the operator detail, else the client detail, else the code; the
  old `"HTTP error: "` / `"DI error: "` prefixes are gone. Client output must keep using
  `public_detail()`/the status title, never `Display`.
- **`Result` / `ActionResult`** still exist as `rust_core::Result` aliases; `RdiError` and
  `EFError` convert at their own boundary (`From` impls in rust-dix 1.1 / rust-ef 2.9.1).
- **`x-webx-*` extensions are reserved**: custom `IErrorResponseWriter` implementations must use
  `webx_core::problem::extensions_for_wire`; the default writer writes `Retry-After` from
  `retry_after_of`.
- **Verification**: `cargo test --workspace` (255 test result blocks after E2) and
  `cargo clippy --workspace --all-targets -- -D warnings` pass on this branch.

##### Structure

- **`webx_ef` no longer maps errors.** `map_ef_error`, `EfResultExt` and the `DETAIL_*`
  sanitization constants are removed; `rust-ef` 2.9.1 implements `From<EFError> for AppError`, so
  handlers use `?` (the workspace `.map_ef()?` call sites were removed). `webx_ef` remains a
  compatibility re-export for this release.
- **`EfIdempotencyStore` moved to `rust-webx-idempotency-ef`** (new workspace member
  `crates/idempotency/ef`). It owns the durable key store, fencing-token rows and
  `IdempotencyHousekeeping`; its test suite moved with it (25 tests). `webx_ef` re-exports the
  same items (including the historical `webx_ef::idempotency::*` path) for one release and is
  scheduled for removal in 3.0.
- **Root workspace is now ef-free**: the `*-ef` starters, the idempotency/outbox starters and the
  docbit/dmbit reference apps moved to `starters/Cargo.toml` (a separate workspace with its own
  lock file and local `[patch.crates-io]`). Root `cargo tree --workspace | rg rust-ef` is empty;
  CI runs both `cargo test --workspace` and
  `cargo test --workspace --manifest-path starters/Cargo.toml`. Acceptance: `rust-ef` only appears
  in the starter tree.

##### API convergence (A)

- **`OperationResult` constructors converged**: `ok`, `ok_with_status` and `success_with_status`
  are removed. Build a payload result with
  `OperationResult::success("created").with_status(7).with_result(value)`; the payload-less form is
  `OperationResult::success("deleted")`, and `failed(status, message)` is unchanged. The redundant
  constructors were only aliases of the `with_*` builders; `operation_contract` 6 tests and
  `operation_result_test` 3 tests cover the replacement.
- **`use_session_store_cached` removed**: wrap explicitly with
  `use_session_store(Arc::new(CachedSessionStore::new(store, ttl)))`. The cache is still a public
  decorator; there is no second registration entry.
- **`use_h3_alt_svc` removed**: `use_alt_svc("h3=\":443\"; ma=86400")` is the single entry (the raw
  value also covers non-H3 alt services). `AltSvcMiddleware::h3(port)` remains available when
  assembling the pipeline by hand.
- **Localization and memory cache have one entry each**: `add_localization()` / `_with(options)` are
  replaced by `use_localization(LocalizationOptions)`; `add_memory_cache()` / `_with(max_entries)` are
  replaced by `add_memory_cache(CacheOptions)` (`CacheOptions::default()` or
  `CacheOptions::default().max_entries(n)`). `max_entries(0)` is rejected at registration.

##### Nested request models (E2)

- **`#[fromquery(nested)] field: RequestModel`** binds a field through its own
  `RequestModel::bind_inline`. `Option<Nested>`, collections and body nesting are rejected at
  compile time; validation belongs on the nested model.
- **`impl RequestModel for PagedRequest`** lives in webx-core (the trait's crate): absent
  `page`/`page_size` use the shared defaults, `page < 1` / `page_size` outside `1..=100` are `400`
  field errors, and `page`/`page_size` are documented in OpenAPI.
- **Nested errors aggregate**: `Binder::nested` folds a nested `400` into the same `errors` map as
  the outer fields; a 401/403/5xx nested failure is kept as a fatal error that wins when the binder
  is consumed. `RequestParamEntry::params` is now `fn() -> &'static [ParamMeta]` so the derive can
  splice nested metadata through a `OnceLock` (OpenAPI A2); the hand-written fixtures use closures.
- **E2 acceptance**: `binding_test` 27 passed (nested bind/defaults/out-of-range/unparsable + OpenAPI
  meta splice); OpenAPI `spec_test` 13 passed.

##### Added

- **`ProblemDetails ↔ AppError` bridge** — `impl From<ProblemDetails> for rust_core::AppError` turns a
  pre-built problem into the shared carrier while storing `title`/`type`/`instance` in the reserved
  `x-webx-title` / `x-webx-type` / `x-webx-instance` extensions (verbatim mode);
  `ProblemDetails::from_app_error(&AppError, path, trace_id)` renders it back byte-for-byte
  (including an absent `type` and an unsanitized pre-built 5xx detail), or derives the title/type
  and sanitizes 5xx in the normal mode. `extensions_for_wire` filters the reserved keys and
  `retry_after_of` reads a typed 429 value; five wire-equivalence tests cover validation, 418, 500,
  derived 500 and 429. This is the prerequisite for the `Error = AppError` alias (next step).

##### Breaking

- **`PagedRequest` is the plain request contract**: fields `page` / `page_size` only; `new`,
  `from_parts`, `clamped`, `Default` and serde field defaults are gone. Defaults and range
  validation move to the binding layer (migration: use `#[serde(default = "..")]` +
  `#[validate(range(..))]`; out-of-range is `400`, not a silent clamp). `offset()` now returns
  `u64`; `limit()` returns `page_size`.
- **`PagedResult<T>` is the flat operation envelope**
  `{success,status,message,items,total}` (composed from `OperationResult<()>`; Rust has no
  inheritance). `PagedResult::new(items, total)` takes two arguments; `from_request` and the
  `page`/`page_size`/`total_pages` echo are removed. The page part still has no request echo;
  clients compute `total_pages = ceil(total / page_size)`.
- **`KeysetRequest` / `Cursor` / `CursorPage` / `CursorError` and `OperationResult<T>`** are
  re-exports from `rust-core`; their shapes are unchanged in this release (the keyset constructors
  and the `OperationResult` method convergence are later steps).
- **New dependency**: `rust-core` (0.1) must be published before `rust-webx` 3.0. `rust-webx`
  runtime crates still do not depend on `rust-ef`.

##### Migration

| 2.9.x | 3.0 |
|---|---|
| `PagedRequest::from_parts(page, size)` | `PagedRequest { page, page_size }` with binding defaults/validation |
| `PagedResult::from_request(items, total, &req)` | `PagedResult::new(items, total)` (or `to_page` in rust-ef) |
| `result.page` / `result.page_size` / `result.total_pages` | request fields / `ceil(total / page_size)` on the client |
| `#[serde(default)]` requires `Deserialize` | `serde` is a `WebxRequestMeta` helper attribute (2.9.1+) |

#### 简体中文

##### Error = `AppError`（第二批）

`webx_core::Error` 现在是 `pub type Error = rust_core::AppError`，2.9 的 enum 已删除；
`ProblemDetails ↔ AppError` 桥保证预构造 problem 的 wire 形状不变，默认 writer 统一走该桥。

- **构造器改为 snake_case**：`Validation` → `validation`、`Internal` → `internal`、`Status` →
  `status`、`Http` → `bad_request`、`Routing` → `route_not_found`、`Serialization` →
  `invalid_json`、`ResponseSerialization` → `response_serialization`、`TooManyRequests { .. }` →
  `too_many_requests(secs, detail)`、`ConflictRetryable` → `conflict_retryable`、`Message` →
  `internal`。
- **模式匹配改为访问器**：`err.kind()`、`err.code()`、`err.status_code()`、`err.public_detail()`、
  `err.operator()`、`err.fields()`、`err.extensions()`；旧的 `Error::Problem(problem)` 构造改为
  `problem.into()`。
- **`Display` 变化**：依次打印 operator detail、客户端 detail、code；不再有 `"HTTP error: "` /
  `"DI error: "` 前缀。客户端输出继续用 `public_detail()`/状态 title，绝不能用 `Display`。
- **`Result` / `ActionResult`** 仍是 `rust_core::Result` 别名；`RdiError`/`EFError` 在各自 crate
  的边界转换（rust-dix 1.1 / rust-ef 2.9.1 的 `From`）。
- **`x-webx-*` 是保留扩展键**：自定义 `IErrorResponseWriter` 必须用
  `webx_core::problem::extensions_for_wire`；默认 writer 用 `retry_after_of` 写 `Retry-After`。
- **验证**：本分支 `cargo test --workspace`（253 个 test binary）与
  `cargo clippy --workspace --all-targets -- -D warnings` 全过。

##### 结构

- **`webx_ef` 不再做错误映射**：删除 `map_ef_error`、`EfResultExt` 与 `DETAIL_*` 脱敏常量；
  rust-ef 2.9.1 已实现 `From<EFError> for AppError`，handler 只用 `?`（workspace 内
  `.map_ef()?` 已移除）。`webx_ef` 本版仅保留兼容重导出。
- **`EfIdempotencyStore` 迁到 `rust-webx-idempotency-ef`**（新 workspace member
  `crates/idempotency/ef`），负责 durable key 表、fencing token 与 `IdempotencyHousekeeping`；
  测试随迁（25 条）。`webx_ef` 重导出同名项（含历史 `webx_ef::idempotency::*` 路径）一个版本，
  3.0 删除。
- **根 workspace 已去 ef**：`*-ef` starters、idempotency/outbox starters 与 docbit/dmbit 参考应用
  移到独立的 `starters/Cargo.toml` workspace（自带 lock 与本地 `[patch.crates-io]`）。根
  `cargo tree --workspace | rg rust-ef` 为空；CI 同时跑
  `cargo test --workspace` 与 `cargo test --workspace --manifest-path starters/Cargo.toml`。
  验收：`rust-ef` 只出现在 starter tree。

##### API 收敛（A）

- **`OperationResult` 构造器收敛**：删除 `ok`、`ok_with_status`、`success_with_status`。带载荷用
  `OperationResult::success("created").with_status(7).with_result(value)`；无载荷用
  `OperationResult::success("deleted")`；`failed(status, message)` 不变。被删的只是 `with_*`
  的别名；`operation_contract` 6 条与 `operation_result_test` 3 条测试覆盖替代写法。
- **删除 `use_session_store_cached`**：显式用
  `use_session_store(Arc::new(CachedSessionStore::new(store, ttl)))` 装饰；缓存仍是公开装饰器，
  注册入口只保留一个。
- **删除 `use_h3_alt_svc`**：唯一入口是 `use_alt_svc("h3=\":443\"; ma=86400")`（原始值也覆盖
  非 H3 的 alt service）；手工组装 pipeline 时仍可用 `AltSvcMiddleware::h3(port)`。
- **localization 与 memory cache 各一个入口**：`add_localization()` / `_with(options)` 改为
  `use_localization(LocalizationOptions)`；`add_memory_cache()` / `_with(max_entries)` 改为
  `add_memory_cache(CacheOptions)`（`CacheOptions::default()` 或
  `CacheOptions::default().max_entries(n)`）；`max_entries(0)` 在注册期拒绝。

##### 嵌套请求模型（E2）

- **`#[fromquery(nested)] field: RequestModel`** 通过该模型自己的 `RequestModel::bind_inline`
  绑定；`Option<Nested>`、集合与 body 嵌套在编译期拒绝，校验写在嵌套模型自身。
- **`impl RequestModel for PagedRequest`** 放在 webx-core（trait 所在 crate）：缺省
  `page`/`page_size` 用共享默认值，`page < 1` / `page_size` 超出 `1..=100` 为 400 字段错误，
  `page`/`page_size` 会进入 OpenAPI。
- **嵌套错误聚合**：`Binder::nested` 把嵌套 400 合并进同一个 `errors` 映射；401/403/5xx 作为
  fatal error 保留，消费 binder 时优先返回。`RequestParamEntry::params` 改为
  `fn() -> &'static [ParamMeta]`，derive 用 `OnceLock` 懒拼接嵌套元数据（OpenAPI A2），手写
  fixture 改用闭包。
- **E2 验收**：`binding_test` 27 passed（嵌套绑定/默认值/越界/非法值 + OpenAPI 元数据拼接）；
  OpenAPI `spec_test` 13 passed。

##### 新增

- **`ProblemDetails ↔ AppError` 桥**：`impl From<ProblemDetails> for rust_core::AppError` 把预构造
  problem 转成共享载体，`title`/`type`/`instance` 存入保留扩展键 `x-webx-title` /
  `x-webx-type` / `x-webx-instance`（verbatim 模式）；
  `ProblemDetails::from_app_error(&AppError, path, trace_id)` 原样还原（包括"没有 type"与预构造
  5xx 的 detail 不脱敏），普通模式派生 title/type 并对 5xx 脱敏。`extensions_for_wire` 过滤保留键，
  `retry_after_of` 读取 typed 429 值；5 条 wire 等价测试覆盖 validation/418/500/派生 500/429。
  这是 `Error = AppError` 别名化的前置。

##### 破坏性变更

- **`PagedRequest` 变成纯请求契约**：只剩 `page` / `page_size` 字段，删除 `new`、`from_parts`、
  `clamped`、`Default` 与 serde 字段默认值；默认值与范围校验移到 binding 层（迁移：用
  `#[serde(default = "..")]` + `#[validate(range(..))]`；越界是 400，不再静默夹紧）。`offset()`
  改返回 `u64`；`limit()` 返回 `page_size`。
- **`PagedResult<T>` 变成扁平业务信封** `{success,status,message,items,total}`（组合
  `OperationResult<()>`，Rust 无继承）。`PagedResult::new(items, total)` 两个参数；删除
  `from_request` 与 `page`/`page_size`/`total_pages` 回声。页面部分仍无请求回声；客户端按
  `ceil(total / page_size)` 计算总页数。
- **`KeysetRequest` / `Cursor` / `CursorPage` / `CursorError` 与 `OperationResult<T>`** 改为从
  `rust-core` 重导出；本版形状不变（keyset 构造器与 `OperationResult` 方法收敛是后续步骤）。
- **新依赖**：`rust-webx` 3.0 依赖 `rust-core` 0.1，必须先发布 core；runtime crates 仍不依赖
  `rust-ef`。

##### 迁移

| 2.9.x | 3.0 |
|---|---|
| `PagedRequest::from_parts(page, size)` | `PagedRequest { page, page_size }` + binding 默认/校验 |
| `PagedResult::from_request(items, total, &req)` | `PagedResult::new(items, total)`（或 rust-ef `to_page`） |
| `result.page` / `result.page_size` / `result.total_pages` | 请求字段 / 客户端 `ceil(total / page_size)` |
| `#[serde(default)]` 需 `Deserialize` | 2.9.1 起 `serde` 是 `WebxRequestMeta` helper attribute |

## [2.9.1] — 2026-11-24

> **English** · **简体中文** · 迁移指南：[docs/rust-webx/16-migration/upgrade-to-3.0.md](docs/rust-webx/16-migration/upgrade-to-3.0.md)

### English

Downstream-feedback release: the fixes and additive APIs reported by RXStudio against 2.9.0
(`upstream-feedback.md`), plus the notification-port rename from the earlier 2.9.1 draft.
Dynamic authorizers can finally read `RequestContext::identity()` on the HTTP path, refresh-token
reuse reports the family that must be revoked, MFA recovery codes return the matched hash for a
database CAS, secret resolution maps to typed errors, and the rate limiter can express a
`(tenant, user)` budget on a templated route. The old notification names are deleted, not
deprecated, by maintainer decision (see Breaking).

#### Added

- **`webx_ef` classifies transient driver errors as a retryable `503`.** `rust-webx-ef` maps
  `EFError::Database` by its structured diagnostics: `SQLSTATE` class `08`/`40`/`57P0*`, SQLite
  `busy`/`locked` (extended result codes `5`/`6`, including `261`/`517`/`262`) and MySQL
  `1205`/`1213` now produce the same `503` + fixed retryable detail as `EFError::Connection`.
  Providers report these conditions as `Database` (only unique violations are promoted), so before
  this release a SQLite busy or a PostgreSQL deadlock reached the client as a non-retryable `500`.
  Applications no longer need a local pre-check for `code == 5/6` or `"database is locked"`.
- **`impl From<rust_dix::RdiError> for webx::Error`.** A failed runtime resolution now converts to
  `Error::Di` (500, `public_detail()` is `None`, the operator log keeps the container message), so
  `?` works in any function returning `webx::Error` and applications no longer need a local
  `map_di_error` helper.
- **`CursorPage::from_entities`** — build a keyset page from up to `page_size + 1` domain
  **entities** and map only the kept rows to the response items. The cursor is computed from the
  last kept entity through `sort_key`, so the entity type no longer needs `Serialize` and the
  response item does not need to expose the raw sort values; the probe row is dropped before
  `map` runs.
- **`CursorPage::map_items`** — transform the kept items while preserving `cursor` and `has_more`.
  Rebuilding a page by hand (`CursorPage::new(mapped, page.cursor)`) recomputes `has_more` from
  `cursor.is_some()`; this helper keeps the page envelope intact.
- **The request context now covers authorization.** `StubEndpoint`, `DirectRouteTarget` and
  `RequestEndpoint` establish `RequestContext::run_with_trace` **before** the authorization guard,
  so an `IDynamicAuthorizer` can read `RequestContext::identity()` (and `trace_id()`) exactly like
  the handler does. The handler/endpoint macros reuse the same `Arc<IdentityUser>`/trace id
  instead of allocating a second copy. Before this release the endpoint's authorization guard ran
  **outside** any request scope (only the later handler dispatch had one), so an authorizer read
  `None` and every authenticated dynamic authorization failed as a 401.
- **`IAdvisoryLock::acquire_with_timeout(key, ttl, wait)`** — the framework-owned waiting policy
  for advisory locks: one immediate attempt, then 10 ms/doubling/200 ms-capped backoff until
  `wait`; returns `Ok(None)` on contention, propagates backend errors unchanged, and treats
  `wait == Duration::ZERO` as one attempt. Business code no longer needs its own retry loop.
- **Rate-limit route templates, composite subjects and a token-bucket constructor** —
  `RateLimitRoute::pattern("/api/tenants/{id}/invitations")` matches one route shape without
  widening to a prefix, `RateLimitSubject::Composite(&[Tenant, User])` budgets the combination
  (each leaf keeps the anonymous-IP fallback, nested composites are flattened), and
  `webx_host::rate_limit::token_bucket(route, subject, rps, burst)` builds a rule over the same
  sharded `InMemoryRateLimiter` as the configuration path (re-exported as `webx::token_bucket`).
- **`EmailMessage` sender fields** — `from`, `reply_to` and `headers` (with `with_from`,
  `with_reply_to`, `with_header`) let a multi-tenant/branded sender choose the envelope instead of
  hiding it in the adapter; `validate()` now checks all addresses and rejects CRLF/control
  characters in header names and values (header-injection defense).
- **Typed `429`/`409` constructors** — `Error::too_many_requests(retry_after_secs, detail)` and
  `Error::conflict_retryable(detail)`. The engine's default problem writer emits `Retry-After`
  for the typed 429 exactly once; a bare `Error::Status(429, ..)` keeps its previous behavior.
- **`impl From<SecretError> for Error`** — `NotFound` becomes a hidden 500 (a missing secret is a
  deployment defect; the name goes to the operator log only) and `Unavailable` a retryable 503;
  handlers can use `?` instead of a local `map_protector_error`. `AesGcmSecretProtector::from_env_names`
  is the synchronous composition-root entry for `build_host(config)` (async/KMS paths keep using
  `from_secret_store`).
- **`RotateOutcome`** — `RefreshTokenService::rotate_outcome` returns
  `Rotated(IssuedRefreshToken)` / `Reused { family_id }` / `Expired` / `Revoked` / `Invalid`;
  reuse still revokes the family before returning, and the family id lets the caller revoke the
  paired session row instead of leaving the leaked access token valid until expiry. `rotate`
  keeps its 401 behavior.
- **MFA recovery codes return the matched record** — `RecoveryCodeSet::find` and
  `consume_matched` expose the matched `RecoveryCodeRecord` (including its hash) so a database
  adapter can run one `WHERE code_hash = ? AND used IS NULL` CAS instead of hashing twice;
  `verify_and_consume` is unchanged. `TotpSecret::generate` now uses the OS CSPRNG
  (`getrandom`, 160 bits) instead of two UUIDs.
- **`serde` is a `WebxRequestMeta` helper attribute** — a request struct no longer needs a separate
  `#[derive(Deserialize)]` just to use `#[serde(default)]` / `rename` / `alias`; the query binder
  honors the default when every source is absent. This is the binding prerequisite for the 3.0
  `PagedRequest` defaults (the `from_parts` normalization can then move into binding: absent
  `page_size` → default, out-of-range → `400`).
- **`webx_auth_ef::register_refresh_token_model(ctx)`** — explicit, idempotent registration of the
  refresh-token entity, shaped like `register_idempotency_model`. It matters for **keyed** contexts
  (a keyed model only discovers `#[context("key")]` entities) and for model-based DDL
  (`bootstrap` / diff); a default context already discovers the entity through `inventory`, and
  `migrate(dir)` reads SQL files and does not use the model.

#### Changed

- `HostBuilder::use_email_sender` → **`HostBuilder::use_email`**,
  `HostBuilder::use_sms_sender` → **`HostBuilder::use_sms`**. Same parameter types, same DI
  registration, same "no adapter registered by default" contract. Capture-sender rustdoc,
  `notifications_test` and the extensibility guide (`13-extensibility/third-party-integration.md`)
  use the new names.

#### Hardening

- **WebSocket**: each outbound frame now has a write deadline (`WebSocketLimits::with_write_timeout`,
  default 10 s; `None` waits forever), so a peer that stops reading cannot pin a writer task and
  its queue for the process lifetime. Browser sessions can be restricted to an Origin allowlist
  (`HostBuilder::use_websocket_origins` / `WebSocketLimits::with_origins`); an empty list keeps the
  previous unchecked behavior.
- **JWKS**: an unknown `kid` is unauthenticated input; refreshes triggered by it are now spaced by
  `JwksResolver::with_min_refresh_interval` (default 5 s), so guesses cannot amplify into
  identity-provider fetches while the single-flight mutex serializes authentication.
- **CSRF**: `IAuthenticationScheme::session_cookie_name` exposes the cookie a scheme reads; the host
  refuses to start when it differs from `CsrfConfig::session_cookie_name` (a drifted name would make
  the CSRF middleware miss every request).
- **Startup validation**: CORS `"*"` combined with credentials is rejected in every mode, and an
  enabled `RateLimit` with zero burst or a non-positive rate is a startup error instead of a host
  that rejects/blocks every request.
- **Health**: a probe panic that poisons the registry mutex no longer pins readiness to `503`; the
  poisoned lock is recovered and real probe state is reported again.
- **Binding**: JSON media-type detection no longer lowercases a fresh `String` on every request.
- **Build**: router/pipeline benches moved to `rust-webx-engine` (the host bencher measured the 1.5
  compatibility embedder), and the `testing/load-probe` harness is registered.

#### Breaking

- **`use_email_sender` / `use_sms_sender` are removed.** Rename the call site to `use_email` /
  `use_sms`; there is no forwarding shim and no `#[deprecated]` marker. `cargo-semver-checks`
  reports the removal through the documented `rust-webx-host` exception in
  [compatibility.md](docs/design/compatibility.md) §3, and the rename is recorded in
  [deprecations.md](docs/design/deprecations.md).
- **`EmailMessage` gains three public fields** (`from`, `reply_to`, `headers`): only
  struct-literal construction breaks. Build with `EmailMessage::new(..)` plus the `with_*`
  builders, as documented in [compatibility.md](docs/design/compatibility.md) §3.
- **`Error` gains `TooManyRequests { retry_after_secs, detail }` and `ConflictRetryable(String)`**:
  a downstream exhaustive `match` needs a wildcard arm. `Error::Status(429, ..)` and
  `Error::Conflict(..)` keep their previous status/code/retryable behavior; the semver exception is
  scoped to `rust-webx-core` in its `Cargo.toml` and registered in
  [compatibility.md](docs/design/compatibility.md) §3.
- **`RateLimitRoute` gains `Pattern(String)` and `RateLimitSubject` gains
  `Composite(&'static [RateLimitSubject])`**: exhaustive matches need a wildcard arm. Existing
  `Exact`/`Prefix`/`Any` and `Ip`/`User`/`Tenant`/`ApiKey` behavior is unchanged; the same
  `rust-webx-core` semver exception and compatibility entry cover these additions.

### 简体中文

命名收敛 + 下游反馈修复版本：在 `upstream-feedback.md` 对 2.9.0 的实测结论上，修复动态授权器
拿不到身份、refresh 重用不回报 family、恢复码无法做 DB CAS、秘密错误无映射、限流无法表达
`(tenant, user)` 等阻塞项；同时保留 2.9.1 草稿里的通知端口改名（旧名直接删除，见"破坏性变更"）。

#### 新增

- **`webx_ef` 把驱动层的瞬态错误归为可重试 `503`。** `rust-webx-ef` 按结构化诊断分类
  `EFError::Database`：`SQLSTATE` 类 `08`/`40`/`57P0*`、SQLite `busy`/`locked`（扩展码
  `5`/`6`，含 `261`/`517`/`262`）、MySQL `1205`/`1213` 现在与 `EFError::Connection` 一样输出
  `503` + 固定可重试文案。provider 只把唯一约束提升为 `UniqueViolation`，其余驱动错误都以
  `Database` 上报，因此本次之前 SQLite busy / PostgreSQL 死锁会以不可重试的 `500` 到达客户端；
  应用不再需要本地的 `code == 5/6` / `"database is locked"` 前置判断。
- **`impl From<rust_dix::RdiError> for webx::Error`。** 运行期解析失败转成 `Error::Di`
  （500、`public_detail()` 为 `None`、容器消息只进运维日志），任何返回 `webx::Error` 的函数都能
  直接 `?`，应用不再需要本地 `map_di_error` 助手。
- **`CursorPage::from_entities`** —— 用最多 `page_size + 1` 条领域**实体**构建游标页，只把留在
  本页的行映射成响应项。游标取自最后一条保留实体，因此实体不必实现 `Serialize`、响应项也不必
  暴露原始排序值；多取的探测行在 `map` 之前就被丢弃。
- **`CursorPage::map_items`** —— 只转换本页项，保留 `cursor` 与 `has_more`。手工
  `CursorPage::new(mapped, page.cursor)` 会按 `cursor.is_some()` 重算 `has_more`，容易丢状态。
- **请求上下文覆盖授权阶段**：`StubEndpoint` / `DirectRouteTarget` / `RequestEndpoint` 在
  授权守卫**之前**建立 `RequestContext::run_with_trace`，动态授权器可以像 Handler 一样读
  `RequestContext::identity()` / `trace_id()`；handler/endpoint 宏复用同一个
  `Arc<IdentityUser>`/trace id，不再二次分配。修复前真实 HTTP 路径上 `identity()` 恒为 `None`，
  所有已认证的动态授权都会变成 401。
- **`IAdvisoryLock::acquire_with_timeout(key, ttl, wait)`**：框架统一的等待策略——先立即尝试一次，
  之后 10ms 起、倍增、上限 200ms 退避，直到 `wait`；争用返回 `Ok(None)`，后端错误原样上抛，
  `wait = Duration::ZERO` 只尝试一次。业务不再手写 `loop + sleep`。
- **限流模板路由、组合主体与 token-bucket 构造器**：`RateLimitRoute::pattern(
  "/api/tenants/{id}/invitations")` 精确匹配一种路由形状而不放大到前缀；
  `RateLimitSubject::Composite(&[Tenant, User])` 按组合预算（叶子仍回退 IP，嵌套被拒绝）；
  `webx_host::rate_limit::token_bucket(route, subject, rps, burst)` 复用配置路径同一个分片
  `InMemoryRateLimiter`，不复制实现。
- **`EmailMessage` 发件人字段**：新增 `from` / `reply_to` / `headers` 与
  `with_from` / `with_reply_to` / `with_header`，多租户/品牌发件人不再藏进适配器字段；
  `validate()` 校验全部地址并拒绝头名/头值中的 CRLF/控制字符（防头注入）。
- **typed `429`/`409` 构造器**：`Error::too_many_requests(retry_after_secs, detail)` 与
  `Error::conflict_retryable(detail)`。引擎默认 problem writer 只为 typed 429 写一次
  `Retry-After`；裸 `Error::Status(429, ..)` 行为不变。
- **`impl From<SecretError> for Error`**：`NotFound` → 隐藏 500（部署缺密钥，名称只进运维日志），
  `Unavailable` → 可重试 503；Handler 直接 `?`，不再需要 `map_protector_error`。
  `AesGcmSecretProtector::from_env_names` 用于同步 `build_host(config)` 组合根；async/KMS 仍走
  `from_secret_store`。
- **`RotateOutcome`**：`RefreshTokenService::rotate_outcome` 返回
  `Rotated(IssuedRefreshToken)` / `Reused { family_id }` / `Expired` / `Revoked` / `Invalid`；
  reuse 仍先撤销 family，并返回 family id，调用方据此撤销配对 session，避免泄露的 access token
  活到自然过期。`rotate` 保持 401 语义。
- **MFA 恢复码返回命中记录**：`RecoveryCodeSet::find` / `consume_matched` 暴露命中的
  `RecoveryCodeRecord`（含 hash），数据库适配器可用一条 `WHERE code_hash = ? AND used IS NULL`
  条件更新完成单次使用，不再重复哈希；`verify_and_consume` 行为不变。`TotpSecret::generate`
  改用 OS CSPRNG（`getrandom`，160 位），不再用两个 UUID。
- **`serde` 成为 `WebxRequestMeta` 的 helper attribute**：请求结构体不必为了 `#[serde(default)]` /
  `rename` / `alias` 额外 `#[derive(Deserialize)]`；所有来源都缺失时查询绑定使用默认值。这是
  3.0 `PagedRequest` 默认值（`from_parts` 归一逻辑下沉到 binding：缺省 `page_size` → 默认值，
  越界 → 400）的前置能力。
- **`webx_auth_ef::register_refresh_token_model(ctx)`**：显式、幂等的 refresh token 实体注册入口，
  与 `register_idempotency_model` 同形。真正需要它的是 **keyed context**（keyed 模型只发现
  `#[context("key")]` 实体）与基于模型的 DDL（`bootstrap`/diff）；默认 context 已通过
  `inventory` 自动发现，`migrate(dir)` 读 SQL 文件、不读模型。

#### 变更

- `HostBuilder::use_email_sender` → **`HostBuilder::use_email`**，
  `HostBuilder::use_sms_sender` → **`HostBuilder::use_sms`**。参数类型、DI 注册方式、以及
  「默认不注册发送器，缺适配器即解析失败」的契约都不变。捕获发送器的 rustdoc、
  `notifications_test` 与扩展性指南（`13-extensibility/third-party-integration.md`）已改用新名。

#### 加固

- **WebSocket**：每帧写有截止时间（`WebSocketLimits::with_write_timeout`，默认 10 秒；`None` 永久
  等待），对端停止读取时不会永久占用 writer 任务与队列；浏览器会话可配置 Origin 白名单
  （`HostBuilder::use_websocket_origins` / `WebSocketLimits::with_origins`），空列表保持旧行为。
- **JWKS**：未知 `kid` 是未认证输入；由其触发的刷新用
  `JwksResolver::with_min_refresh_interval`（默认 5 秒）限速，避免猜测放大成对 IdP 的请求，
  同时 single-flight 锁不再被串行化认证拖住。
- **CSRF**：`IAuthenticationScheme::session_cookie_name` 暴露 scheme 读取的 cookie；与
  `CsrfConfig::session_cookie_name` 不一致时宿主拒绝启动（名字漂移会让 CSRF 中间件漏掉所有请求）。
- **启动校验**：CORS `"*"` + credentials 在任何模式都拒绝；`RateLimit` 启用但 burst 为 0 或速率
  非正时启动失败，而不是启动后把每个请求都拒掉/放行。
- **健康检查**：探针 panic 污染 registry mutex 后不再把 readiness 永久钉在 503；恢复锁并继续报告
  真实探针状态。
- **绑定**：JSON media-type 判断不再为每个请求分配小写 `String`。
- **构建**：router/pipeline bench 移到 `rust-webx-engine`（host bencher 测的是 1.5 兼容嵌入器），
  新增 `testing/load-probe` 压测 harness。

#### 破坏性变更

- **`use_email_sender` / `use_sms_sender` 已删除。** 调用点改成 `use_email` / `use_sms`；没有转发
  别名，也没有 `#[deprecated]` 标记。`cargo-semver-checks` 的移除报告按
  [compatibility.md](docs/design/compatibility.md) §3 的已登记例外处理，改名同时登记在
  [deprecations.md](docs/design/deprecations.md)。
- **`EmailMessage` 新增三个公开字段**（`from` / `reply_to` / `headers`）：只破坏结构体字面量构造。
  用 `EmailMessage::new(..)` + `with_*` 构建器，迁移见
  [compatibility.md](docs/design/compatibility.md) §3。
- **`Error` 新增 `TooManyRequests { retry_after_secs, detail }` 与 `ConflictRetryable(String)`**：
  下游穷举 `match` 需补通配分支。`Error::Status(429, ..)` 与 `Error::Conflict(..)` 的状态码/code/
  retryable 行为不变；semver 例外只限定在 `rust-webx-core` 的 `Cargo.toml`，并登记在
  [compatibility.md](docs/design/compatibility.md) §3。
- **`RateLimitRoute` 新增 `Pattern(String)`、`RateLimitSubject` 新增
  `Composite(&'static [RateLimitSubject])`**：下游穷举 `match` 需补通配分支。既有
  `Exact`/`Prefix`/`Any` 与 `Ip`/`User`/`Tenant`/`ApiKey` 的匹配/键派生不变；同一份
  `rust-webx-core` semver 例外与 compatibility 登记覆盖这两个新增。

## [2.9.0] — 2026-11-24

> **English** · **简体中文** · 迁移指南：[docs/rust-webx/16-migration/upgrade-to-3.0.md](docs/rust-webx/16-migration/upgrade-to-3.0.md)

### English

Transport-architecture release: the engine gains a **transport-neutral request/response port** and
a **listener-level `IHttpTransport` abstraction**, and the new optional `rust-webx-h3` crate serves
HTTP/3 (QUIC + rustls + h3) through the same router, middleware, binding, error writer, SSE
lifecycle, admission governor and DI as the hyper HTTP/1.1+2 path. Application code keeps using the
existing `webx` API; there are **no application-facing breaking changes**.

#### Added

- **Request-level transport port** (`webx_engine::transport`): `TransportRequest` /
  `TransportResponse`, neutral streaming request bodies (`Empty`/`Buffered`/`Streaming`
  with data/trailer frames) and response bodies (`None`/`Bytes`/`Stream`);
  `Server::serve_transport_request` runs the production pipeline for one decoded request,
  so a multiplexing transport (HTTP/3) needs no connection-level trait.
- **Listener-level transport port**: `IHttpTransport` + `TransportRuntime` +
  `Server::run_transport`, neutral `TransportUpgrade` (`IUpgradeSession` contract unchanged),
  `IHttpTransport::stop_budget()` for the host's bounded stop, and
  `HostBuilder::use_http_transport(...)` to start/stop a backend with the host.
- **Global admission for non-hyper transports**: `TransportRuntime::admit_connection()` shares the
  one `App.MaxConnections` governor and admitted/rejected/active counters with the TCP acceptors;
  h3 connections count as one connection each.
- **Per-transport metrics**: `/metrics` exposes
  `engine_transport_connections_total/active`, `engine_transport_requests_total`,
  `engine_transport_connections_rejected_total` and
  `engine_transport_handshake_failures_total` with a `transport="h3"` label.
- **`Alt-Svc` advertisement**: `HostBuilder::use_h3_alt_svc(udp_port)` (and the generic
  `use_alt_svc(value)`) advertises the HTTP/3 endpoint on HTTP/1.1+2 responses; nothing is
  advertised unless configured, and clients fall back to TCP when UDP is unreachable.
- **`rust-webx-h3` (new optional crate)**: QUIC endpoint + HTTP/3 listener with TLS 1.3-only /
  ALPN `h3`, mTLS client-CA verification, verified client-certificate SHA-256 injection,
  SSE/streaming responses, per-connection GOAWAY drain and engine admission. It is opt-in; default
  builds do not pull quinn/h3.
- **WebSocket over HTTP/3 (RFC 9220 extended CONNECT)**: the h3 adapter enables extended CONNECT,
  rewrites `:protocol: websocket` requests into the standard WebSocket upgrade form so the existing
  host middleware performs handshake/authentication/identity/limits, commits the successful
  extended CONNECT as HTTP `200`, and bridges the h3 request stream to `IUpgradeSession`.
  RFC-conformant clients that omit `Sec-WebSocket-Key` are accepted (the adapter supplies the
  middleware's placeholder; the h1 key/accept validation is not part of extended CONNECT), and a
  failed extended CONNECT never forwards a normal 2xx body. Real-client e2e covers text and binary
  echo with and without the key. This required upgrading the optional dependency from upstream
  `h3 0.0.8` to the pinned `trev-h3 0.3.0` fork.
- **WebTransport (adapter-level MVP)**: `H3Config::with_webtransport_handler` enables
  `:protocol: webtransport`, adopts RFC 9220 sessions and hands the application a concrete
  `WebTransportServerSession` (bidi/unidi streams + datagrams). The handler gets a pre-accept
  `authorize(&WebTransportRequestInfo)` hook (denial is `403` before any `200`), then
  `on_session(info, session)`; the hook is **fail-closed** (a handler that does not override it
  denies every session, because CONNECT bypasses the engine middleware pipeline). A real h3 client opens a WebTransport bidirectional stream and
  receives the echoed payload end to end. WebTransport CONNECT bypasses the engine HTTP
  middleware pipeline, so the handler authorizes the session itself; a transport-neutral
  WebTransport port for `webx`/`webx_core` remains future work.
- **QUIC keep-alive / idle / stream limits**: `H3Config` exposes
  `with_keep_alive_interval`, `with_max_idle_timeout` and `with_stream_limits` (defaults 15s /
  60s / 16+16) so long-lived realtime sessions survive quinn's idle timer and the buffered-body
  worst case stays bounded.
- **HTTP/3 interop example and evidence**: `crates/transport/h3/examples/interop_server.rs`
  serves the native `/health` probe and a JSON echo; it was exercised with `quiche-client`,
  `h3spec v0.1.15` (86 cases: 64 pass / 21 upstream-fork failures / 1 pending baseline) and
  `h3i 0.7.0` replay (GET/POST 200).
- **Parity and regression tests**: hyper-vs-neutral external parity matrix, fake/host transport
  end-to-end tests, and an HTTP/3 e2e suite (GET/POST/404/413, mTLS fingerprint, SSE, GOAWAY
  in-flight drain, admission rejection, shutdown, WebSocket extended CONNECT, WebTransport bidi
  stream echo).

#### Changed

- h3/h1/h2 share the same pipeline, error writer, response assembly and shutdown/drain lifecycle;
  the hyper path keeps its byte-identical output (alloc probe: Windows 10.42, Linux CI 12.42;
  the CI budget is 13, the Windows reference budget is 12).
- QUIC 0-RTT remains **disabled** (`max_early_data_size = 0`) until the idempotency replay story is
  designed end to end.
- WebSocket-over-HTTP/3 is delivered via the pinned `trev-h3 0.3.0` fork (upstream h3 0.0.8 cannot
  express `:protocol: websocket`). **WebTransport is delivered as an adapter-level MVP** on the
  same fork (`trev-h3-webtransport`): `H3Config::with_webtransport_handler` receives the concrete
  session with streams/datagrams and the e2e covers a bidi stream echo. A **transport-neutral**
  WebTransport port for `webx`/`webx_core` remains new-port work beyond 2.9.
- **Batched idempotency cleanup**: `EfIdempotencyStore::purge_expired` removes expired
  `webx_idempotency` rows in bounded batches (`PURGE_BATCH_SIZE`, 500 rows per statement) instead
  of one unbounded `DELETE`, so a large backlog can no longer become a single long write lock on
  the request hot path. Each batch re-checks `expires_at` inside the `DELETE`, so a row another
  instance reclaimed between the batch read and the delete keeps its new reservation. Behavior of
  `purge_expired(now, older_than)` and `IdempotencyHousekeeping` is otherwise unchanged.

#### Fixed

- **Graceful shutdown no longer truncates WebSocket close frames.** hyper resolves the connection
  future right after the `101` handoff, so a detached upgrade session could outlive the worker
  runtime that owned it and lose its `1001` close frame. `AdmissionStats` now counts live upgrade
  sessions and each worker waits (within the drain budget) for that count to reach zero before its
  runtime is dropped. Covered by the repeated `close_semantics_test` run.
- **Buffered request bodies honor the body limit on streaming reads.**
  `IHttpRequest::body_stream()` applies the same `413` check as `body_bytes()`, so a whole-body
  transport (HTTP/3, in-process backends) cannot bypass `App.MaxBodySize` or a route's
  `#[max_body_size]` by taking the streaming API.
- **The acceptor `waiting` gauge cannot leak on cancellation.** The TCP acceptor now uses an RAII
  guard around `AdmissionStats::begin_wait`/`end_wait`, so dropping the accept future
  (shutdown/task cancellation) no longer leaves the gauge non-zero and trips the shutdown
  invariant.

#### Breaking

- **None relative to 2.8.0.** `webx`, `webx_core`, `webx_host` and existing engine entry points
  keep their signatures and behavior. New transport APIs are additive; engine additions are
  documented above.
- **Upgrading from 2.7.0** (the last published release): 2.8.0 was never published separately;
  its documented Breaking changes ship in 2.9.0. Custom `IIdempotencyStore` implementations must
  apply the 2.8.0 migration (`IdempotentResponse::new(..)` + `.with_headers(..)`,
  `IdempotencyLease::new(..)`, wildcard arm for `IdempotencyBegin`) — see the [2.8.0] Breaking
  section below. All other 2.8.0 additions are source-compatible.
- **Behavior changes to apply when coming from 2.7.0:** `RateLimit.TrustProxy` /
  `RateLimit.TrustedProxies` are legacy no-ops (they were replaced by
  `ClientInfo.TrustForwardedHeaders` / `ClientInfo.TrustedProxies`); move the trust configuration
  to the `ClientInfo` section.

#### Notes

- MSRV stays **1.95**; `rust-webx-h3` pins `quinn = 0.11.12`, `trev-h3 = 0.3.0`,
  `trev-h3-quinn = 0.3.0` and `trev-h3-webtransport = 0.3.0` (the MIT h3 fork is isolated
  behind `H3Config`/`H3Transport`/`IWebTransportHandler`; exact pins keep fork changes contained).
- **`rust-webx-h3` is an opt-in preview transport in 2.9.** Default builds do not link it;
  the HTTP/1.1+2 engine is the stable path. Interop baseline: h3spec v0.1.15, 86 cases /
  64 pass / 21 failures in the pinned upstream `trev-h3 0.3.0` fork (H3/QPACK error handling
  and one quinn/rustls TLS case) / 1 pending. Those failures are tracked upstream and do not
  affect the default pipeline.
- Cross-repo pairing: rust-ef 2.9.0 first, then rust-webx 2.9.0 (unchanged ORM boundary).
- **Repository layout:** `crates/` is now the published surface only. The non-published
  harnesses moved to `testing/` (`alloc-probe`, `contract-tests`, `upgrade-smoke`), and the
  crate-layout gate rejects a `publish = false` package under `crates/`. No published API
  changed; only in-repo paths (CI commands, docs and the `contract-tests` dev-dependency path).
- **Composition root:** the host keeps exactly two assembly slots — `register` (DI object graph)
  and `configure` (`AppOptions`) — and the 2.9 transport/`Alt-Svc` helpers are sugar over the
  same builder state. `crates/host/tests/composition_slots_test.rs` pins the two-slot contract.

### 简体中文

传输架构版本：engine 新增**传输中立的请求/响应端口**与**监听级 `IHttpTransport` 抽象**；新增
可选 crate `rust-webx-h3`，通过同一 router、middleware、绑定、错误写入、SSE 生命周期、
准入 governor 与 DI 提供 HTTP/3（QUIC + rustls + h3）服务。应用代码继续使用原 `webx` API，
**对应用代码无破坏性变更**。

#### 新增

- 请求级传输端口：`TransportRequest`/`TransportResponse`，中立流式请求体
  （`Empty`/`Buffered`/`Streaming`，含 data/trailer 帧）与响应体（`None`/`Bytes`/`Stream`）；
  `Server::serve_transport_request` 复用生产 pipeline（多路复用传输按请求提交，无需连接级 trait）。
- 监听级传输端口：`IHttpTransport`/`TransportRuntime`/`Server::run_transport`、中立
  `TransportUpgrade`（`IUpgradeSession` 契约不变）、`IHttpTransport::stop_budget()` 供 Host
  有界停机、`HostBuilder::use_http_transport`。
- 非 hyper 传输共享全局准入：`TransportRuntime::admit_connection()` 与 TCP 共用
  `App.MaxConnections` governor 和 admitted/rejected/active 计数；一条 QUIC 连接占一个名额。
- per-transport 指标：`/metrics` 输出 `engine_transport_*{transport="h3"}` 系列。
- `Alt-Svc` 通告：`HostBuilder::use_h3_alt_svc(udp_port)`（以及通用 `use_alt_svc(value)`）
  在 HTTP/1.1+2 响应上显式通告 HTTP/3 端点；未显式配置时不发送，UDP 不可达时客户端回退 TCP。
- 新可选 crate `rust-webx-h3`：TLS1.3-only/ALPN h3、mTLS、客户端证书 SHA-256 注入、
  SSE/流式响应、每连接 GOAWAY 排空、engine 准入。默认构建不引入 quinn/h3 依赖。
- **WebSocket over HTTP/3（RFC 9220 extended CONNECT）**：h3 adapter 开启 extended CONNECT，
  把 `:protocol: websocket` 改写为标准 WebSocket upgrade，复用 host 的 handshake/鉴权/身份/
  限制中间件；成功响应按 extended CONNECT 要求返回 `200`，并把 h3 request stream 桥接到
  `IUpgradeSession`。RFC 合规客户端可省略 `Sec-WebSocket-Key`（adapter 为现有中间件补占位，
  extended CONNECT 不做 h1 的 key/accept 校验）；失败的 extended CONNECT 不会转发正常 2xx
  body。真实客户端 e2e 覆盖有/无 key 的文本/二进制回显。为此依赖从上游 `h3 0.0.8` 升级到
  精确钉版的 `trev-h3 0.3.0` fork。
- **WebTransport（adapter-level MVP）**：`H3Config::with_webtransport_handler` 启用
  `:protocol: webtransport`，接受 RFC 9220 session 并向应用暴露具体
  `WebTransportServerSession`（bidi/unidi streams + datagrams）；handler 先经过
  `authorize(&WebTransportRequestInfo)` 预授权（拒绝在 200 之前返回 403），再进入
  `on_session(info, session)`。该钩子**默认拒绝（fail-closed）**：WebTransport CONNECT 不经过
  engine middleware，未覆写 `authorize` 的 handler 不会收到未认证隧道。真实 h3 client 打开
  WebTransport bidi stream 并收到回显 payload 的 e2e 已覆盖；未覆写授权钩子返回 403 的 e2e
  也已覆盖。WebTransport CONNECT 不经过 engine HTTP middleware，handler 需自行认证授权。
  transport-neutral 的 WebTransport 端口仍是后续工作。
- **QUIC keep-alive / idle / stream limits**：`H3Config` 提供
  `with_keep_alive_interval`、`with_max_idle_timeout`、`with_stream_limits`（默认 15s/60s/16+16），
  避免长连接撞 quinn idle timer，并限制 buffered-body 最坏内存。
- **HTTP/3 互操作示例与证据**：`crates/transport/h3/examples/interop_server.rs` 提供
  native `/health` 与 JSON echo；已用 `quiche-client`、`h3spec v0.1.15`（86 cases：64 pass /
  21 个上游 fork 失败 / 1 pending 基线）和 `h3i 0.7.0` replay（GET/POST 200）验证。
- 测试：hyper/中立端口 parity 矩阵、fake/host e2e，以及 HTTP/3 e2e
  （GET/POST/404/413、mTLS 指纹、SSE、GOAWAY 在途请求、准入拒绝、停机、WebSocket 回显、
  WebTransport bidi stream echo）。

#### 变更

- h3/h1/h2 共用 pipeline、错误写入、响应组装与停机排空；hyper 路径保持字节一致
  （alloc 税：Windows 10.42、Linux CI 12.42；CI 预算 13，Windows 参考预算 12）。
- QUIC 0-RTT 保持关闭（`max_early_data_size = 0`），待幂等重放设计完成后再评估。
- WebSocket over HTTP/3 通过钉版 `trev-h3 0.3.0` fork 交付（上游 h3 0.0.8 无法表达
  `:protocol: websocket`）。WebTransport 以同一 fork 的 `trev-h3-webtransport` 交付
  **adapter-level MVP**（session + streams/datagrams，bidi stream echo e2e）；面向
  `webx`/`webx_core` 的 **transport-neutral** WebTransport 端口仍属后续新端口设计。
- 幂等清理分批：`EfIdempotencyStore::purge_expired` 改为按 `PURGE_BATCH_SIZE`（500 行/语句）
  分批删除过期 `webx_idempotency` 行，不再发一条无界 `DELETE`，积压再大也不会在请求热路径上
  形成一条长时间写锁。每批的 `DELETE` 内重复 `expires_at` 条件，因此另一实例在「读候选」与
  「删除」之间重新 claim 的行会保留新预约。`purge_expired(now, older_than)` 与
  `IdempotencyHousekeeping` 的对外行为不变。

#### 修复

- **优雅停机不再截断 WebSocket 关闭帧。** hyper 在 `101` 交棒后立刻结束连接 future，分离的升级
  会话可能比拥有它的 worker runtime 活得更久，从而丢失 `1001` 关闭帧。`AdmissionStats` 现在统计
  存活的升级会话，worker 在丢弃 runtime 之前、在排空预算内等待该计数归零。由反复运行的
  `close_semantics_test` 覆盖。
- **Buffered 请求体在流式读取路径同样受上限约束。** `IHttpRequest::body_stream()` 复用
  `body_bytes()` 的 `413` 检查，整包传输（HTTP/3、进程内后端）无法通过流式 API 绕过
  `App.MaxBodySize` 或路由 `#[max_body_size]`。
- **acceptor 的 `waiting` 计数不再在取消时泄漏。** TCP acceptor 用 RAII 守卫包裹
  `AdmissionStats::begin_wait`/`end_wait`，accept future 被 drop（停机/任务取消）不再留下非零
  计数并误触发停机不变量。

#### 破坏性变更

- **相对 2.8.0 无破坏性变更。** `webx`/`webx_core`/`webx_host` 与既有 engine 入口签名/行为不变；
  新传输 API 均为加性。
- **从 2.7.0 升级**（最近一个已发布版本）：2.8.0 未单独发布，其已文档化的 Breaking 变更随 2.9.0
  交付。自定义 `IIdempotencyStore` 实现需按下方 [2.8.0] Breaking 段迁移
  （`IdempotentResponse::new(..)` + `.with_headers(..)`、`IdempotencyLease::new(..)`、
  `IdempotencyBegin` 的 match 补通配分支）；2.8.0 的其余新增均源码兼容。

#### 备注

- MSRV 保持 **1.95**；`rust-webx-h3` 精确钉 `quinn = 0.11.12`、`trev-h3 = 0.3.0`、
  `trev-h3-quinn = 0.3.0`、`trev-h3-webtransport = 0.3.0`（MIT h3 fork 隔离在
  `H3Config`/`H3Transport`/`IWebTransportHandler` 后面，精确钉版限制 fork 变更的影响面）。
- **`rust-webx-h3` 在 2.9 是可选 preview 传输。** 默认构建不链接；HTTP/1.1+2 engine 仍是稳定
  路径。互操作基线：h3spec v0.1.15，86 cases / 64 pass / 21 个 pinned upstream
  `trev-h3 0.3.0` fork 失败（H3/QPACK 错误处理 + 1 个 quinn/rustls TLS case）/ 1 pending。
  失败按上游跟踪，不影响默认 pipeline。
- 跨仓配对：先 rust-ef 2.9.0，后 rust-webx 2.9.0（ORM 边界不变）。
- **仓库布局：** `crates/` 现在只放可发布 crate。非发布测试工具迁移到 `testing/`
  （`alloc-probe`、`contract-tests`、`upgrade-smoke`），布局门禁会拒绝 `crates/` 下的
  `publish = false` 包。公开 API 无变化，仅仓库内路径（CI 命令、文档与 `contract-tests`
  开发依赖路径）调整。
- **组合根：** Host 只有两个装配槽——`register`（DI 对象图）与 `configure`（`AppOptions`）；
  2.9 的传输/`Alt-Svc` helper 是同一 builder 状态的糖。
  `crates/host/tests/composition_slots_test.rs` 锁定两槽契约。

## [2.8.0] — unreleased

> This iteration was never tagged or published to crates.io; its changes ship in **2.9.0**.
> Kept here as the provenance of the 2.8 iteration.

> **English** · **简体中文** · 迁移指南：[docs/rust-webx/16-migration/upgrade-to-3.0.md](docs/rust-webx/16-migration/upgrade-to-3.0.md)

### English

Multi-instance hardening release: a durable `IIdempotencyStore` for `rust-webx-ef`, a complete
request-idempotency protocol (fencing tokens, replayable response headers, route allow-list, DI
auto-discovery), and a single source of truth for forwarded-header resolution (`ClientInfo`) that
rate limiting now consumes. Additive APIs plus documented breaking changes confined to the
idempotency value types and store protocol: `IdempotentResponse` gained a required `headers` field,
`IdempotentResponse`/`IdempotencyLease`/`IdempotencyBegin` are `#[non_exhaustive]`, and the lease
methods gained an explicit reservation-lease argument — see **Breaking**. The only other behavior
changes are that the legacy `RateLimit.TrustProxy` / `RateLimit.TrustedProxies` keys are ignored
(with a startup warning) and the idempotency scope encoding is now injective — see **Changed**.

#### Added

- **`Idempotency.AllowAnonymous`** (default `true`): opt out of idempotency for
  anonymous requests. Anonymous keys are scoped by client IP, so a body-derived
  client key on a public endpoint could replay another client's response from the
  same NAT/proxy; public deployments should set it to `false`.
- **`Idempotency.Paths` / `Idempotency.SkipPaths`** (default empty): restrict the
  `Idempotency-Key` middleware to path prefixes, matched on path-segment boundaries
  (`/api/orders` covers `/api/orders/123`, not `/api/orders-archive`; trailing `/`
  ignored; `SkipPaths` wins). Empty `Paths` keeps the pre-2.8 "every path" behavior;
  requests outside the list pass through before the key header is read, with no
  store round trip.
- **Idempotency fencing tokens and an explicit reservation lease** —
  `webx_core::idempotency::IdempotencyLease` plus three default-implemented
  `IIdempotencyStore` methods: `begin_lease(key, retention, lease)` (returns the
  attempt token with `Started`), `complete_lease(key, lease, response, retention)`
  and `release_lease(key, lease)` (pass the token back). The middleware calls the
  lease variants with `lease = min(Idempotency.LeaseSeconds, retention)`, so a
  reservation that expired and was reclaimed by a retry can no longer be
  completed/released by the old owner — the store's compare-and-set rejects the
  stale token even while the new lease is live. The default implementations
  delegate to `begin`/`complete`/`release` with the retention period and ignore
  the shorter lease, so stores written for 2.7 keep compiling and working
  unchanged (they keep the weaker "TTL must exceed the handler's worst case"
  guarantee).
- **`Idempotency.LeaseSeconds`** (default 60): the reservation lease is now a
  framework configuration knob, separate from `Idempotency.TtlSeconds` (the
  replay retention). A crashed handler becomes retryable after the lease instead
  of after the full retention; the middleware caps it at the retention. The host
  logs both effective durations at startup.
- **`IdempotentResponse::{new, with_headers}`** — constructors for the now
  `#[non_exhaustive]` idempotency value types, so custom stores can build the
  response without a struct literal (see **Breaking**).
- **`Idempotency` fail-open warning in `Production`**: with `Enabled=true`,
  `AllowAnonymous=true` (the default) and an empty `Paths` list the host logs a
  startup warning — every anonymous state-changing path is then replayed by
  client IP, so clients behind one NAT/proxy can replay each other's responses.
  Behavior is unchanged (unlike the CORS/legacy-RateLimit fail-closed guards):
  authenticated-only deployments are safe with the defaults.
- **`rust-webx-ef` real-database test coverage** — `idempotency_realdb_test.rs`
  (opt-in `postgres` / `mysql` features, `#[ignore]`d, driven by
  `RUST_EF_PG_URL` / `RUST_EF_MYSQL_URL`) exercises the store against a real
  PostgreSQL/MySQL server; a new CI `realdb` job runs them with `postgres:16` +
  `mysql:8` services. The two providers stay optional, so default builds/tests
  remain SQLite-only.
- **Replayable response headers** — `IdempotentResponse.headers` carries the
  `Location`, `ETag`, `Last-Modified` and `Cache-Control` headers of the cached
  response (case-insensitive whitelist, storage/replay order preserved); the
  middleware captures them and writes them back on a replay. Credential-bearing
  headers (`Set-Cookie`, `Authorization`, …) are never captured or replayed.
- **Host idempotency auto-discovery from DI** — with `Idempotency.Enabled = true`
  and no explicit `use_idempotency`, the host now uses the first
  `dyn IIdempotencyStore` registered in DI (e.g. `EfIdempotencyStore`) and falls back
  to the bounded in-memory store only when none is registered. An explicit
  `use_idempotency(store, ttl)` still wins over both.
- **`EfIdempotencyStore::with_max_body_bytes`**: the store caps response bodies
  to its column capacity (≈60 KiB on MySQL `BLOB`, unbounded on
  PostgreSQL/SQLite) and releases the reservation for larger bodies instead of
  failing. The reservation lease is no longer a store option: it is passed by
  the middleware (`Idempotency.LeaseSeconds`), so the store honours exactly the
  lease it is given and applies the retention to completed responses.
- **`rust-webx-ef::idempotency` — durable request idempotency over rust-ef.**
  `IdempotencyRecord` (table `webx_idempotency`, registered with
  `register_idempotency_model`), `EfIdempotencyStore` implementing the existing
  `IIdempotencyStore` port (`begin` / `complete` / `release` plus the lease variants),
  `purge_expired`, and `IdempotencyHousekeeping` (`PeriodicBackgroundService`) for TTL
  cleanup. The store owns the application's `Arc<DbContextOptions>` and opens a
  short-lived `DbContext` per operation, so it is constructed before
  `HostBuilder::build()` and wired with `use_idempotency(store, ttl)` — or registered
  in DI and picked up automatically by `Idempotency.Enabled`. Cross-instance
  correctness uses the primary-key uniqueness of the protocol key's SHA-256 digest
  (`key_hash`, fixed width; the full unbounded key is kept in a non-indexed `key`
  column) plus a compare-and-set lease claim
  (`UPDATE ... WHERE key_hash = ? AND expires_at <= ?`); a completed, unexpired key
  replays the stored status/content-type/content-encoding/replayable-headers/body
  (binary-safe), a live key returns `InProgress` (409), and an expired key is
  reclaimed by exactly one instance. Every claim writes a fresh `attempt` fencing
  token; `complete_lease`/`release_lease` add `attempt = ?` to the compare-and-set and
  clear it on success, so a late completion/release from a reclaimed attempt is
  discarded instead of overwriting the new owner. Completed responses persist the
  replayable headers as JSON in the `headers` column. Both columns are **nullable**
  (old rows keep working; upgrade with the official rust-ef migration tooling — no
  runtime/hand-written DDL).
  See `docs/rust-webx/11-production/idempotency.md` § 数据库持久化（多实例）.
- **`webx_core::client_info` — shared trusted-proxy resolution.**
  `IpCidr` (`parse` / `contains`), `client_from_forwarded_for` (right-most hop that is not
  a trusted proxy), and `resolve_client_info` (peer + forwarded chain + `X-Real-IP` under
  the trusted-proxy policy) now live in core, so every middleware resolves the client
  address the same way.
- **Aggregated startup configuration validation** (`webx_host::config_validation`):
  `HostBuilder::try_build` now collects every `AppOptions`/Production-guard violation and
  returns one sorted `Error::Di` listing all bad keys (`ConfigViolation { section, key,
  message }`, `validate_options`, `format_violations`) instead of aborting at the first
  invalid key. Existing rules and defaults are unchanged.
- **In-process wire test harness** (`TestHost::connect_wire`): serves an already-connected
  duplex stream through the real engine hyper/router/middleware/error/drain path without
  binding a port — HTTP/1.1 keep-alive, h2c prior knowledge, SSE bytes and malformed
  requests. The engine gains `Server::serve_stream` as the transport-neutral seam; tests
  live in `crates/webx/tests/wire_harness_test.rs`.
- **cargo-fuzz corpus** (`fuzz/`, a separate workspace that never enters the release):
  six targets — keyset cursor decode, config JSON binding, ClientInfo/`X-Forwarded-For`
  resolution, `ProblemDetails` JSON, file-name sanitization and event-wire framing — with
  committed seed corpora, stable-toolchain invariant tests, and the nightly
  `.github/workflows/fuzz.yml` job.
- **Stable observability contract** (`webx.http` / `webx.connection`): a DEBUG span tree
  (`http.server.request`, `auth.authenticate`, `http.route_match`, `http.handler`,
  `http.binding`, `http.response_write`) plus INFO `connection.opened`/`connection.closed`
  events; span/event names and fields are documented as a compatibility contract in
  `docs/design/observability-contract.md` (DEBUG keeps the default path allocation-free).
- **Unified close semantics**: `webx_core::realtime::CloseReason` (`ClientClosed`,
  `ServerShutdown`, `SessionCompleted`, `IdleTimeout`, `LifetimeExceeded`, `TokenExpired`,
  `ProtocolError`, `Internal`), `IConnection::close_reason()` and
  `ShutdownToken::is_draining()`; WebSocket close paths and SSE `StreamEnd` map onto it;
  the wire-code table is in `docs/design/close-codes.md`.
- **Error-code registry** (`webx_core::error_codes`): `ErrorCodeSpec`/`ERROR_CODES`/
  `FALLBACK_STATUS` register the 22 stable `problem+json` `code` values; a test ties every
  `Error` variant and `default_code_for_status` to the registry so an unregistered code
  fails CI. The `type` URI convention is in `docs/design/error-codes.md`.
- **Admission invariant metrics** (`/metrics`): `engine_admission_active_ratio`,
  `engine_admission_invariant_violations`, `engine_admission_invariant_violations_total`
  and `engine_admission_shutting_down`; the engine exposes `ServerStats` /
  `AdmissionStats::snapshot()` and warns once when permit/wait accounting invariants are
  violated. See `docs/rust-webx/11-production/metrics.md`.
- **Conformance gate** (`scripts/conformance.sh` and the `conformance` job in
  `stability.yml`): an external-client matrix over HTTP/1.1 + HTTP/2 h2c
  (200/400/401/413/415 and the problem+json envelope) plus optional bounded h2spec;
  h2-over-TLS ALPN stays covered by `http2_test.rs`.
- **Transport-port design** (`docs/design/transport-port.md`): an HTTP/3/WebTransport
  port sketch (`IHttpTransport`), the `EngineContext::from_parts` / `serve_stream` seam,
  quiche vs quinn trade-offs, and the explicit decision that a working H3 server is
  requirement-driven 3.0 work (2.x is additive-only).
- **Development-only TLS key logging**: setting `SSLKEYLOGFILE` with
  `APP_ENV=Development` attaches rustls' standard key logger so Wireshark can decrypt
  local TLS traffic (`webx_engine::tls::with_key_log` / `key_logging_allowed`).
  `Production` ignores the variable (one-time warning); the file is a credential and
  must not be shipped or collected. See `docs/rust-webx/11-production/cors-tls-health.md`.

#### Changed

- **WebSocket sessions end explicitly on host drain.** A WS session now selects on the
  host `ShutdownToken` (next to its idle/lifetime/token-expiry deadlines) and sends close
  code `1001` ("server shutting down") as soon as draining starts, instead of being
  aborted at the drain deadline; the session handler future is dropped at that point
  (matching SSE `on_server_shutdown`). `IConnection::close_reason()` reports why a session
  ended, and `ShutdownToken::is_draining()` lets handlers distinguish drain from
  cancellation. Every reason's wire code is in `docs/design/close-codes.md`.
- **Rate limiting keys on the resolved client address, and both configuration paths agree.**
  `RateLimitMiddleware` no longer parses `X-Forwarded-For` itself: `RateLimitSubject::Ip`
  (and the IP fallback for `User` / `Tenant` / `ApiKey`) reads
  `IHttpContext::client_info().client_ip`, i.e. the value produced by the `ClientInfo`
  middleware (or the peer address when that middleware is not installed). Consequently
  `RateLimitMiddleware::from_policy` and the `RateLimit` configuration section now behave
  identically; previously the policy path hard-coded "do not trust forwarded headers"
  while the config path could trust them.
- **`RateLimit.TrustProxy` / `RateLimit.TrustedProxies` are legacy (ignored).**
  The keys are still accepted by serde (old appsettings do not fail to start) but the rate
  limiter ignores them and the host logs a startup warning. Migrate to
  `ClientInfo.TrustForwardedHeaders` / `ClientInfo.TrustedProxies`. Per the repository 2.x
  policy no new `#[deprecated]` attributes are added (downstreams build with
  `-D warnings`); the items are registered in `docs/design/deprecations.md` for the 3.0
  window, and `RateLimiter::with_trust_proxy` remains as a no-op compatibility shell.
- **Idempotency scope now includes the tenant, the query string and the
  `ClientInfo` client address; the scope encoding is injective.** The scope
  segment is `tenant:<tenant>:user:<sub>` when the request carries a tenant claim
  (`user:<sub>` otherwise); anonymous requests use the resolved client IP (not
  the raw peer). The canonical query string (`k=v&...`, sorted) is part of the
  scope, so `POST /x?mode=a` and `?mode=b` no longer replay each other. Every
  dynamic component is now percent-escaped (`%`/`:`/`?`/`&`/`=`), so a subject,
  tenant, path or query value containing a separator can no longer alias another
  request's scope. Ordinary values (`u-1`, `/api/orders`, `mode=a`) produce the
  same key as before, so keys in flight during the upgrade keep matching;
  pathological values intentionally use a new key. The middleware reserves with
  the explicit reservation lease (`Idempotency.LeaseSeconds`) while a completed
  response is retained for the caller's TTL, so a killed instance is retried
  quickly.
- **In-memory idempotency eviction prefers completed keys.** At `MaxTrackedKeys`
  the bounded store now evicts a completed entry first (its handler already ran; a
  later retry simply re-executes), then an expired entry, and only as a last resort
  an in-progress reservation. Previously the entry with the earliest expiry was
  evicted regardless of state, so a burst of new keys could drop live reservations
  before completed ones. Single-process semantics are unchanged.
- **A replayed idempotent response now includes the whitelisted response headers**
  (`Location`, `ETag`, `Last-Modified`, `Cache-Control`) captured with the stored
  body; previously a replay restored only status/content-type/content-encoding/body
  and dropped `Location`/`ETag`. See **Added** for the whitelist.
- **Legacy `RateLimit.TrustProxy` / `RateLimit.TrustedProxies` fail the startup
  in `Production`** (they are still ignored with a warning in
  Development/Test). Silently ignoring them behind a reverse proxy would collapse
  every client into the proxy address's bucket, which an attacker can drain.
  Migrate to `ClientInfo.TrustForwardedHeaders` / `ClientInfo.TrustedProxies`.
- **Idempotency keys are tenant-scoped and use the `ClientInfo` client address.**
  The scope segment is now `tenant:<tenant>:user:<sub>` when the request carries a
  tenant claim (`user:<sub>` otherwise), and anonymous requests use the
  `ClientInfo`-resolved client IP instead of the raw peer address. This closes a
  cross-tenant replay window when the same subject exists in several tenants and
  makes proxied anonymous traffic scope by real client rather than by proxy. Keys
  stored by a previous 2.7 instance are not reused after the upgrade (they expire
  with the configured TTL); no configuration change is required.
- **`ClientInfoMiddleware` delegates to `webx_core::client_info::resolve_client_info`.**
  Behavior (trusted proxy gate, forwarded chain, fallback to the peer) is unchanged.
- **docbit production sample** now configures `ClientInfo.TrustForwardedHeaders` /
  `ClientInfo.TrustedProxies` (loopback Nginx) instead of `RateLimit.TrustProxy`.
- **Documentation toolchain**: CI and `docs/rust-webx/book.toml` moved to mdBook v0.5.4
  (the `multilingual` key was removed; it defaulted to `false`).

#### Fixed

- **A failed `complete` releases the reservation.** If storing the response fails
  (database hiccup, column limit), the middleware now attempts `release` so the
  client can retry immediately instead of receiving `409` until the TTL expires.
- **In-memory replays now filter stored response headers to the whitelist.** The
  middleware only ever stores `Location`/`ETag`/`Last-Modified`/`Cache-Control`,
  but a custom `complete` can supply anything; the in-memory store now filters
  and lower-cases on replay exactly like the durable store, so a hand-written
  `Set-Cookie`/`Authorization` can never reach a replayed response.
- **A completion that lost its reservation is now logged at `warn` (EF store)**
  instead of `debug`, with the `key_hash` and the attempt token. A pattern of
  these means the handler outlived `Idempotency.LeaseSeconds` (the handler may
  run twice for one key) or a clock skew shortened the lease; the documentation
  explains the log meaning.
- **File/stream responses are no longer cached as empty bodies.** A `200` with
  `FileResult`/streaming content reports `has_body() == true` but exposes no
  bytes; caching it would replay `200` + `0` bytes. Such responses now release
  the key (SSE was already excluded).
- **IPv4-mapped IPv6 peers/hops are normalized to IPv4 for trusted-proxy resolution.**
  A dual-stack listener reports IPv4 peers as `::ffff:a.b.c.d`; that form is now
  compared in its IPv4 shape, so an IPv4 `ClientInfo.TrustedProxies` range keeps
  matching (previously forwarded headers were ignored and every client behind the
  proxy shared the proxy's rate-limit bucket), and `ClientInfo::from_peer` reports
  the canonical IPv4 client address. Covered by client_info unit tests.

#### Breaking

Accepted under the documented 2.x policy (registered in the `cargo-semver-checks`
allowlist and `docs/design/compatibility.md`); all items are mechanical to migrate.

- **The idempotency value types are now `#[non_exhaustive]`.**
  `IdempotentResponse` gained a required `headers` field *and* is now
  `#[non_exhaustive]`; `IdempotencyLease` and `IdempotencyBegin` are
  `#[non_exhaustive]` too (the lease type is new in 2.8). Migrate:
  * construct a response with
    `IdempotentResponse::new(status, content_type, content_encoding, body)` and add
    replayable headers with `.with_headers(Vec<(String, String)>)` (previously a
    struct literal with `headers: Vec::new()`);
  * construct a fencing token with `IdempotencyLease::new(token)` (previously a
    literal);
  * add a wildcard arm (`_ =>`) to any `match` on `IdempotencyBegin`.

  Reading fields is unaffected and `IIdempotencyStore` stays object-safe with
  default methods. Custom store implementations should persist/round-trip
  `headers` so a replay is header-identical; ignoring it is valid but silently
  loses `Location`/`ETag`/`Last-Modified`/`Cache-Control` on replay. The
  `#[non_exhaustive]` markers are what let 2.x keep adding state/metadata without
  another breaking release.
- **`IIdempotencyStore` lease methods take an explicit reservation lease.**
  `begin_lease` is now
  `begin_lease(&self, key, retention: Duration, lease: Duration)` and
  `complete_lease` takes the response `retention` (parameter renames are not
  breaking; the added `lease` argument is). Stores written against the pre-2.8
  trait are unaffected: the provided defaults still delegate to
  `begin`/`complete`/`release` with the retention period. The middleware always
  calls the lease variants with `lease = min(Idempotency.LeaseSeconds, retention)`.
- **`EfIdempotencyStore::with_lease` / `EfIdempotencyStore::DEFAULT_LEASE` were
  removed** (both were introduced in this unreleased 2.8 cycle): the reservation
  lease now comes from the caller (`Idempotency.LeaseSeconds`), so the store's
  own option was redundant and could silently disagree with the middleware. Use
  the `Idempotency.LeaseSeconds` configuration key (the constant lives in
  `webx_core::idempotency::DEFAULT_LEASE`).
- **`rust-webx-ef::idempotency::IdempotencyRecord` gained two nullable columns**
  (`attempt: Option<String>`, `headers: Option<String>`) for fencing tokens and
  replayable headers. Struct-literal constructions add `attempt: None, headers: None`;
  existing databases upgrade with the official rust-ef migration tooling (generate a
  migration from the new snapshot and run `ctx.migrate`). Both columns are nullable,
  so the change is additive at the SQL level and pre-upgrade rows keep working.

#### Verification

- `cargo test --workspace` green; new host end-to-end tests assert (a) two forwarded
  clients behind a trusted proxy get independent rate-limit buckets, (b) a direct client
  forging `X-Forwarded-For` is still limited by its peer address, (c) `from_policy` and
  `from_config` produce the same decisions and bucket keys, (d) without the `ClientInfo`
  middleware the limiter falls back to the peer, (e) legacy keys only produce a warning.
- Idempotency: 14 host unit tests (protocol, replayable-header whitelist filtering,
  lease-vs-retention expiry, lease fencing/reclaim, eviction priority, route
  allow-list boundaries, scope isolation/injective-encoding collision
  counterexamples, canonical-query injectivity) + 9 host integration tests
  (whitelisted response-header capture/replay order, lease token and
  `(retention, lease)` passed to the store with the retention cap, route allow-list
  passthrough, DI auto-discovery and explicit-wins ordering, legacy token-less
  store fallback, Production scope warning not failing startup) + the 3 umbrella
  end-to-end tests (replay, concurrent 409, invalid key).
- `rust-webx-ef` idempotency: 8 unit + 24 SQLite integration tests (replay, in-progress
  conflict, TTL reclaim for both states, release, purge/grace, binary body round-trip,
  failed-INSERT recovery, over-244-byte protocol key by digest, late
  completion/release discarded after expiry, stale-lease fencing, explicit lease
  honoured without an internal `min(ttl)` correction, legacy `begin` reservation,
  replayable-header round-trip and whitelist filtering, pre-2.8 rows with NULL
  `attempt`/`headers`) + 2 `#[ignore]` real-database tests (PostgreSQL/MySQL).
- `cargo semver-checks --package {rust-webx-core,rust-webx-engine,rust-webx-host,rust-webx}
  --baseline-rev v2.7.0` passes locally; the CI `semver` job now baselines v2.7.0
  (previously v2.1.0) and the `docs/design/*` compatibility documents agree.
- Real databases: the CI `realdb` job runs
  `cargo test -p rust-webx-ef --features postgres,mysql --tests -- --ignored`
  against `postgres:16` + `mysql:8` service containers (unique-key INSERT race
  fallback, conditional UPDATE/DELETE, Unix-seconds TTL columns, binary bodies).
  The providers are optional features, so default builds stay SQLite-only.
- `cargo clippy --workspace --all-targets -D warnings`, rustdoc `-D warnings`, MSRV
  1.95 and both feature-matrix checks green; both mdBook sites build with mdBook v0.5.4.
- `cargo audit`: no vulnerabilities (the committed lockfile refreshed `url` 2.5.8 ->
  2.5.9 for RUSTSEC-2026-0592); the remaining advisories are the allowed unmaintained
  warnings (chrono, rustls-pemfile).
- **2.8 observability/quality gates (this batch).** `cargo fmt --all --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace --no-fail-fast`, `RUSTDOCFLAGS="-D warnings" cargo doc` for
  core/engine/host and `scripts/check-docs.sh` are green. New coverage: wire harness 4,
  aggregated config validation 7, observability 3, close semantics 4, error-code
  registry 6, admission invariant metrics 4, fuzz logic 8 (stable-toolchain
  `cargo test --manifest-path fuzz/Cargo.toml --lib`). Local `scripts/conformance.sh`
  HTTP/1.1 matrix passes 9/9; h2spec 2.6.0 against the same process records the
  documented one-known-deviation baseline (invalid connection preface closes TCP
  without GOAWAY). Nightly cargo-fuzz linkage and the Linux HTTP/2 h2c matrix remain
  CI-only (Windows libFuzzer has no entry point; local Windows curl lacks nghttp2).
- **Cross-repo pairing with rust-ef 2.8.0.** `scripts/check-pairing-rust-ef.sh` builds
  this workspace with `[patch.crates-io]` overrides that point `rust-ef` /
  `rust-ef-sqlite` at a local rust-ef checkout (`RUST_EF_PATH`), then runs
  `cargo test -p rust-webx-ef`; the optional CI `pairing` job runs the same script only
  when configured (`vars.RUST_EF_PATH` / `vars.RUST_EF_REPO`, or the `rust_ef_ref`
  dispatch input). Release order is **rust-ef 2.8.0 first, rust-webx 2.8.0 second**.
  Evidence boundary: this proves the workspace and the EF adapter compile — and the EF
  adapter's SQLite tests pass — against the provided rust-ef tree; it does not verify
  the crates.io artifacts, that tree's semver gate, or real PostgreSQL/MySQL (covered
  by the real-database job).

### 简体中文

多实例加固版本：`rust-webx-ef` 新增持久化 `IIdempotencyStore`；请求幂等协议补齐（fencing token、
可重放响应头、路由白名单、DI 自动装配、显式预约租约）；转发头解析收敛为 `ClientInfo` 唯一入口，
限流改为读取其解析结果。API 以加法为主，破坏性变更收敛在幂等值类型与 store 协议：
`IdempotentResponse` 新增必填 `headers` 字段、`IdempotentResponse`/`IdempotencyLease`/
`IdempotencyBegin` 标记 `#[non_exhaustive]`、租约方法新增显式 lease 参数（见「破坏性变更」）。
除此之外的行为变化是 `RateLimit.TrustProxy` / `RateLimit.TrustedProxies` 旧键被忽略（启动时 warn）
与幂等 scope 改为单射转义（见「变更」）。

#### 新增

- **`Idempotency.AllowAnonymous`**（默认 `true`）：按需关闭匿名请求的幂等。匿名键按客户端 IP
  分域，公开接口上由 body 确定性推导的客户端键可能在同一 NAT/反代 IP 下互相重放，对外部署
  建议设为 `false`。
- **`Idempotency.Paths` / `Idempotency.SkipPaths`**（默认空）：把幂等中间件收敛到路径前缀，
  按路径分段边界匹配（`/api/orders` 命中 `/api/orders/123`，不命中 `/api/orders-archive`；
  末尾 `/` 忽略；`SkipPaths` 优先）。`Paths` 为空保持 2.8 之前「所有路径」的行为；范围外的
  请求在读 `Idempotency-Key` 之前直接放行，不产生存储往返。
- **幂等 fencing token 与显式预约租约** —— `webx_core::idempotency::IdempotencyLease`，以及三个
  带默认实现的 `IIdempotencyStore` 方法：`begin_lease(key, retention, lease)`（`Started` 时返回
  本次尝试的 token）、`complete_lease(key, lease, response, retention)` / `release_lease(key, lease)`
  （把 token 传回）。中间件一律调用租约变体，并传
  `lease = min(Idempotency.LeaseSeconds, retention)`，因此「租约过期后被重试抢占」的旧 owner
  迟到 `complete`/`release` 会被存储的 CAS（`attempt = ?`）拒绝，即使新租约仍然 live 也不会互相
  覆盖。默认实现退回无 token 的 `begin`/`complete`/`release` 并用 retention 作预约期，
  2.7 时代的 store 不改代码即可继续编译与工作（保留「TTL 必须大于 Handler 最坏耗时」的较弱保证）。
- **`Idempotency.LeaseSeconds`**（默认 60）：预约租约成为框架配置项，与 `Idempotency.TtlSeconds`
  （已完成响应的重放保留期）分离。崩溃的 Handler 在租约到期后即可被重试抢占，不必等完整保留期；
  中间件下传时以保留期为上限。宿主启动日志会同时打印两个生效时长。
- **`IdempotentResponse::{new, with_headers}`** —— 为已标记 `#[non_exhaustive]` 的幂等值类型提供
  构造器，自定义 store 无需结构体字面量即可构造响应（见「破坏性变更」）。
- **`Production` 下的 `Idempotency` 启动 warn**：`Enabled=true` + `AllowAnonymous=true`（默认）
  + 空 `Paths` 时，宿主打 warn——这时所有匿名写路径都按客户端 IP 重放，同一 NAT/反代后的客户端
  可能互相重放响应。行为不变（与 CORS/旧限流键的 fail-closed 风格不同）：只启用已认证主体幂等
  的部署可安全使用默认值。
- **`rust-webx-ef` 真库测试覆盖** —— `idempotency_realdb_test.rs`（可选 `postgres` / `mysql`
  feature，`#[ignore]`，由 `RUST_EF_PG_URL` / `RUST_EF_MYSQL_URL` 驱动）在真实 PostgreSQL/MySQL
  上跑 store 协议；新增 CI `realdb` job 用 `postgres:16` + `mysql:8` service 执行。两个 provider
  为可选依赖，默认构建/测试仍只用 SQLite。
- **可重放响应头** —— `IdempotentResponse.headers` 保存缓存响应的 `Location`、`ETag`、
  `Last-Modified`、`Cache-Control`（大小写不敏感白名单，保存/重放顺序一致），中间件在首次响应
  时捕获、重放时写回。`Set-Cookie`、`Authorization` 等凭据头永不捕获、永不重放。
- **Host 从 DI 自动发现幂等存储** —— `Idempotency.Enabled = true` 且未显式 `use_idempotency`
  时，Host 使用 DI 中注册的第一个 `dyn IIdempotencyStore`（例如 `EfIdempotencyStore`），
  都没注册才退回有界内存存储；显式 `use_idempotency(store, ttl)` 仍然优先。
- **`EfIdempotencyStore::with_max_body_bytes`**：store 按列容量限制响应 body（MySQL `BLOB`
  约 60 KiB，PG/SQLite 由中间件 `MaxBodyBytes` 决定），超限时释放预约而不是失败。预约租约不再是
  store 选项，而由中间件（`Idempotency.LeaseSeconds`）下传：store 完全按收到的 lease 写预约、
  按 retention 写已完成响应。
- **`rust-webx-ef::idempotency` — 基于 rust-ef 的持久化请求幂等。**
  `IdempotencyRecord` 实体（表 `webx_idempotency`，用 `register_idempotency_model` 注册到
  应用模型，建表/迁移走官方 rust-ef 工具）；`EfIdempotencyStore` 实现既有
  `IIdempotencyStore` 端口（`begin`/`complete`/`release` 以及租约变体）；`purge_expired` 与
  `IdempotencyHousekeeping`（`PeriodicBackgroundService`）负责 TTL 清理。store 持有应用的
  `Arc<DbContextOptions>`，每次操作开短生命周期 `DbContext`，因此在 `HostBuilder::build()`
  之前即可构造并用 `use_idempotency(store, ttl)` 装配，也可注册到 DI 后由
  `Idempotency.Enabled` 自动发现。跨实例正确性靠主键唯一 + CAS 租约抢占
  （`UPDATE ... WHERE key_hash = ? AND expires_at <= ?`）：已完成且未过期的键重放
  status/content-type/content-encoding/可重放响应头/body（二进制安全）；进行中的键返回
  `InProgress`（409）；过期键恰好一个实例抢占成功。每次抢占写入新的 `attempt` fencing
  token；`complete_lease`/`release_lease` 的 CAS 增加 `attempt = ?` 并在成功后清空，
  因此被回收的旧尝试迟到完成/release 是 no-op，不会覆盖新 owner。已完成响应把可重放响应头
  以 JSON 存进 `headers` 列。两列都是**可空**列（旧行继续可用；升级走官方 rust-ef 迁移工具，
  无需手写 DDL）。
  详见 `docs/rust-webx/11-production/idempotency.md`「数据库持久化（多实例）」。
- **`webx_core::client_info` — 共享可信代理解析。**
  `IpCidr`（`parse`/`contains`）、`client_from_forwarded_for`（最右侧非可信代理 hop）、
  `resolve_client_info`（对端 + 转发链 + `X-Real-IP`，按可信代理策略）提升到 core，
  所有中间件用同一套解析。
- **启动期配置聚合校验**（`webx_host::config_validation`）：`HostBuilder::try_build`
  一次性收集全部 `AppOptions`/Production 守卫违规，按 section+key 排序后以一条
  `Error::Di` 返回（`ConfigViolation { section, key, message }`、`validate_options`、
  `format_violations`），不再“第一个非法键就退出”；既有规则与默认值不变。
- **进程内 wire 测试基座**（`TestHost::connect_wire`）：把一条已连接的双端流交给真实
  engine 的 hyper/路由/中间件/错误写入/排空路径，不绑定端口；覆盖 HTTP/1.1 keep-alive、
  h2c prior knowledge、SSE 字节与畸形请求。engine 新增 `Server::serve_stream` 作为
  传输无关接缝；验收见 `crates/webx/tests/wire_harness_test.rs`。
- **cargo-fuzz 语料**（`fuzz/`，独立 workspace，不进入发布）：6 个 target——keyset
  cursor 解码、配置 JSON 绑定、ClientInfo/`X-Forwarded-For` 解析、`ProblemDetails`
  JSON、文件名净化、事件线格式——提交种子语料、稳定工具链不变量测试，以及 nightly
  `.github/workflows/fuzz.yml`。
- **稳定的观测契约**（`webx.http` / `webx.connection`）：DEBUG span 树
  （`http.server.request`、`auth.authenticate`、`http.route_match`、`http.handler`、
  `http.binding`、`http.response_write`）与 INFO `connection.opened`/
  `connection.closed` 事件；span/事件名与字段作为兼容性契约写入
  `docs/design/observability-contract.md`（DEBUG 级保证默认路径零分配增量）。
- **统一关闭语义**：`webx_core::realtime::CloseReason`（`ClientClosed`/
  `ServerShutdown`/`SessionCompleted`/`IdleTimeout`/`LifetimeExceeded`/`TokenExpired`/
  `ProtocolError`/`Internal`）、`IConnection::close_reason()`、
  `ShutdownToken::is_draining()`；WebSocket 关闭路径与 SSE `StreamEnd` 统一映射，
  线协议码表见 `docs/design/close-codes.md`。
- **错误码注册表**（`webx_core::error_codes`）：`ErrorCodeSpec`/`ERROR_CODES`/
  `FALLBACK_STATUS` 登记 22 条稳定的 `problem+json` `code`；测试把每个 `Error`
  变体与 `default_code_for_status` 绑定到注册表，未登记的新 code 会让 CI 失败；
  `type` URI 约定见 `docs/design/error-codes.md`。
- **准入不变量指标**（`/metrics`）：新增 `engine_admission_active_ratio`、
  `engine_admission_invariant_violations`、`engine_admission_invariant_violations_total`、
  `engine_admission_shutting_down`；engine 暴露 `ServerStats`/
  `AdmissionStats::snapshot()`，permit/wait 记账不变量被违反时只告警一次。
  见 `docs/rust-webx/11-production/metrics.md`。
- **互操作门禁**（`scripts/conformance.sh` + `stability.yml` 的 `conformance` job）：
  外部客户端矩阵覆盖 HTTP/1.1 + HTTP/2 h2c（200/400/401/413/415 + problem+json
  信封）与可选、有界的 h2spec；h2-over-TLS ALPN 仍由 `http2_test.rs` 覆盖。
- **传输端口设计**（`docs/design/transport-port.md`）：HTTP/3/WebTransport 端口草案
  （`IHttpTransport`）、`EngineContext::from_parts`/`serve_stream` 接缝、quiche 与
  quinn 取舍，以及“可运行 H3 server 属需求驱动的 3.0 工作、2.x 只加性”的明确决策。
- **仅 Development 的 TLS 密钥日志**：`APP_ENV=Development` 且设置 `SSLKEYLOGFILE` 时，
  框架给 rustls 挂上标准 key logger，便于 Wireshark 解密本机 TLS 流量
  （`webx_engine::tls::with_key_log` / `key_logging_allowed`）。`Production` 忽略该变量
  （只告警一次）；该文件等同凭据，不得随镜像发布或进日志采集。见
  `docs/rust-webx/11-production/cors-tls-health.md`。

#### 变更

- **WebSocket 会话在主机排空时显式收尾。** WS 会话现在与自身 idle/lifetime/token 过期
  截止时间一起 `select` 主机 `ShutdownToken`：排空一开始立即发送 close code `1001`
  （"server shutting down"），而不是等到排空预算耗尽被 abort；此时 Handler future 会被
  drop（与 SSE `on_server_shutdown` 对齐）。`IConnection::close_reason()` 报告会话结束原因，
  `ShutdownToken::is_draining()` 让 Handler 区分“排空”与“取消”。每种原因的 wire code
  见 `docs/design/close-codes.md`。
- **限流按已解析客户端地址计桶，两条配置路径行为一致。**
  `RateLimitMiddleware` 不再自己解析 `X-Forwarded-For`：`RateLimitSubject::Ip`
  （以及 `User`/`Tenant`/`ApiKey` 的 IP 回退）读取
  `IHttpContext::client_info().client_ip`，即 `ClientInfo` 中间件的解析结果（未安装该
  中间件时即连接对端地址）。因此 `RateLimitMiddleware::from_policy` 与 `RateLimit`
  配置节现在行为一致；此前 policy 路径固定不信任转发头，而配置节路径可信任。
- **`RateLimit.TrustProxy` / `RateLimit.TrustedProxies` 变为 legacy（被忽略）。**
  serde 仍接受旧配置（旧 appsettings 不会启动失败），但限流器忽略并在宿主启动时
  打 warn。迁移到 `ClientInfo.TrustForwardedHeaders` / `ClientInfo.TrustedProxies`。
  按仓库 2.x 政策不新增 `#[deprecated]` 属性（下游常以 `-D warnings` 构建）；两项登记在
  `docs/design/deprecations.md` 的 3.0 窗口，`RateLimiter::with_trust_proxy` 保留为
  no-op 兼容壳。
- **幂等 scope 进入租户、查询串与 `ClientInfo` 客户端地址，且编码改为单射。**
  请求带 tenant claim 时 scope 为 `tenant:<tenant>:user:<sub>`（否则 `user:<sub>`）；匿名请求用
  解析后的客户端 IP（不再用原始对端地址）。规范化查询串（`k=v&...`，排序）进入 scope，
  `POST /x?mode=a` 与 `?mode=b` 不再互相重放。各动态段现在做百分号转义
  （`%`/`:`/`?`/`&`/`=`），主体/租户/路径/查询值里带分隔符也不再可能与另一个请求产生相同键；
  普通取值（`u-1`、`/api/orders`、`mode=a`）的键与升级前一致，在途键继续命中，异常取值会有意
  使用新键。预约使用显式租约（`Idempotency.LeaseSeconds`），完成写入使用调用方 TTL 作为保留期。
- **内存幂等存储在满额时优先淘汰已完成条目。** 达到 `MaxTrackedKeys` 时按优先级淘汰：先已
  完成条目（Handler 已执行过，后续重试重新执行即可），其次过期条目，最后才淘汰进行中的预约。
  此前的实现一律淘汰最早过期者，突发新键可能先丢掉仍 live 的预约。单进程语义不变。
- **幂等响应重放现在包含白名单响应头**（`Location`、`ETag`、`Last-Modified`、
  `Cache-Control`），此前只重放状态码/content-type/content-encoding/body 并丢弃
  `Location`/`ETag`。白名单见「新增」。
- **`RateLimit.TrustProxy` / `RateLimit.TrustedProxies` 在 Production 模式启动失败**
  （Development/Test 仍忽略并 warn）。反代后静默忽略会把所有客户端塌缩到代理地址的
  一个桶，攻击者可轻松打满；请迁移到 `ClientInfo.TrustForwardedHeaders` /
  `ClientInfo.TrustedProxies`。
- **幂等键按租户隔离，匿名主体使用 `ClientInfo` 解析后的客户端地址。**
  请求带 tenant claim 时 scope 段为 `tenant:<tenant>:user:<sub>`（否则 `user:<sub>`）；
  匿名请求用 `ClientInfo` 解析出的客户端 IP（替代原始对端地址）。这修掉了「同一用户
  存在于多个租户时按 sub 跨租户重放」的窗口，也让反代后的匿名流量按真实客户端而不是
  代理地址隔离。升级前 2.7 实例写入的键不会被复用（按配置 TTL 自然过期），无需改配置。
- **`ClientInfoMiddleware` 改为调用 `webx_core::client_info::resolve_client_info`。**
  行为（可信代理门控、转发链、回退对端）不变。
- **docbit 生产示例**改用 `ClientInfo.TrustForwardedHeaders` /
  `ClientInfo.TrustedProxies`（本机 Nginx），不再使用 `RateLimit.TrustProxy`。
- **文档工具链**：CI 与 `docs/rust-webx/book.toml` 升到 mdBook v0.5.4（移除
  `multilingual` 键，其原值即默认 false）。

#### 修复

- **`complete` 失败时释放预约。** 存储响应失败（DB 抖动/列上限）时，中间件会再尝试 `release`，
  客户端可立即重试，而不是在 TTL 内一直收到 409。
- **文件/流式响应不再被当作空 body 缓存。** `FileResult`/流式内容 `has_body() == true` 但
  `body_bytes()` 为空，缓存会重放 `200` + 0 字节；现在释放键（SSE 此前已排除）。
- **内存 store 重放时也按白名单过滤响应头。** 中间件只会存
  `Location`/`ETag`/`Last-Modified`/`Cache-Control`，但自定义 `complete` 可能写入任意头；
  内存 store 现在与持久化 store 一致，在重放时过滤并把名字规范为小写，
  `Set-Cookie`/`Authorization` 之类不会再出现在重放响应里。
- **完成写入丢失预约时改为 warn（EF store）**（之前是 debug），日志带 `key_hash` 与本次
  attempt token。频繁出现说明 Handler 超过 `Idempotency.LeaseSeconds`（同一键可能执行两次）
  或时钟偏差缩短了租约；文档写明了日志含义与处置方式。
- **IPv4-mapped IPv6 对端/hop 在可信代理解析中归一为 IPv4。** 双栈监听会把 IPv4 对端
  报告为 `::ffff:a.b.c.d`；现在按 IPv4 形态比较，IPv4 的 `ClientInfo.TrustedProxies`
  网段继续命中（此前会忽略转发头，导致代理后所有客户端共享代理地址的限流桶），
  `ClientInfo::from_peer` 也报告归一后的 IPv4 客户端地址。有 client_info 单元测试覆盖。

#### 破坏性变更

按仓库 2.x 已文档化破坏性变更政策接受（登记在 `cargo-semver-checks` allowlist 与
`docs/design/compatibility.md`）；以下各项都是机械迁移。

- **幂等值类型标记 `#[non_exhaustive]`。** `IdempotentResponse` 新增必填 `headers` 字段**并且**
  标记 `#[non_exhaustive]`；`IdempotencyLease`（2.8 新增）与 `IdempotencyBegin` 同样标记。
  迁移方式：
  * 用 `IdempotentResponse::new(status, content_type, content_encoding, body)` 构造响应，
    再用 `.with_headers(Vec<(String, String)>)` 附加可重放头（替代结构体字面量 +
    `headers: Vec::new()`）；
  * 用 `IdempotencyLease::new(token)` 构造围栏令牌（替代字面量）；
  * `match IdempotencyBegin` 补 `_` 通配分支。

  读取字段不受影响；`IIdempotencyStore` 仍对象安全且方法有默认实现。自定义 store 应持久化并
  原样返回 `headers`，否则重放会静默丢失 `Location`/`ETag`/`Last-Modified`/`Cache-Control`
  （仍可工作，只是不完整）。这些标记让 2.x 后续继续加状态/元数据时不再产生破坏性版本。
- **`IIdempotencyStore` 租约方法新增显式预约租约参数。** `begin_lease` 变为
  `begin_lease(&self, key, retention: Duration, lease: Duration)`，`complete_lease` 接收响应的
  `retention`（参数改名不破坏；新增 `lease` 参数是破坏性变更）。按 2.7 之前 trait 编写的 store
  不受影响：默认实现仍用 retention 委托 `begin`/`complete`/`release`。中间件始终以
  `lease = min(Idempotency.LeaseSeconds, retention)` 调用租约变体。
- **移除 `EfIdempotencyStore::with_lease` / `EfIdempotencyStore::DEFAULT_LEASE`**（二者都是本次
  未发布的 2.8 周期新增）：预约租约改由调用方（`Idempotency.LeaseSeconds`）决定，store 自己的
  选项既冗余又可能与中间件不一致。改用 `Idempotency.LeaseSeconds` 配置键；常量在
  `webx_core::idempotency::DEFAULT_LEASE`。
- **`rust-webx-ef::idempotency::IdempotencyRecord` 新增两个可空列**
  （`attempt: Option<String>`、`headers: Option<String>`），分别用于 fencing token 与可重放
  响应头。结构体字面量构造补 `attempt: None, headers: None`；已有数据库用官方 rust-ef 迁移
  工具升级（从新快照生成迁移并执行 `ctx.migrate`）。两列均可空，SQL 层是加性变更，升级前的
  行继续可用。

#### 验证

- `cargo test --workspace` 全绿；新增 host 端到端测试断言：(a) 可信代理后两个转发
  客户端各自独立限流桶；(b) 直连客户端伪造 `X-Forwarded-For` 仍按对端地址限流；
  (c) `from_policy` 与 `from_config` 决策与桶键一致；(d) 无 `ClientInfo` 中间件时回退
  对端；(e) 旧配置键只产生 warn。
- 幂等：14 个 host 单元测试（协议、重放头白名单过滤、租约/保留期过期、租约 fencing/抢占、
  淘汰优先级、路由白名单边界、scope 隔离与单射编码碰撞反例、查询串单射）+ 9 个 host 集成测试
  （白名单响应头捕获/重放顺序、传给 store 的 token 与 `(retention, lease)` 及 retention 截断、
  路由白名单直通、DI 自动发现与显式优先、旧 token-less store 默认实现回落、Production scope
  warn 不致启动失败）+ 3 个 umbrella 端到端测试（重放、并发 409、非法 key）。
- `rust-webx-ef` 幂等：8 个单元测试 + 24 个 SQLite 集成测试（重放、进行中冲突、两种状态的 TTL
  抢占、release、purge/grace、二进制 body 往返、失败 INSERT 恢复、>244 字节协议键走摘要、
  过期后的迟到 complete/release 被丢弃、旧 token fencing、显式 lease 不被内部 `min(ttl)` 纠正、
  旧 `begin` 预约期、可重放响应头往返与白名单过滤、2.8 之前 `attempt`/`headers` 为 NULL 的旧行）
  + 2 个 `#[ignore]` 真库测试（PostgreSQL/MySQL）。
- `cargo semver-checks --package {rust-webx-core,rust-webx-engine,rust-webx-host,rust-webx}
  --baseline-rev v2.7.0` 本地通过；CI `semver` job 的基线由 v2.1.0 升为 v2.7.0，
  `docs/design/*` 兼容性文档同步更新。
- 真库：CI `realdb` job 用 `postgres:16` + `mysql:8` service 运行
  `cargo test -p rust-webx-ef --features postgres,mysql --tests -- --ignored`
  （唯一键 INSERT 竞争回落、条件 UPDATE/DELETE、Unix 秒 TTL 列、二进制 body）。
  两个 provider 是可选 feature，默认构建仍只用 SQLite。
- `cargo clippy --workspace --all-targets -D warnings`、rustdoc `-D warnings`、MSRV
  1.95 与 feature 矩阵检查全绿；两个 mdBook 站点均可用 mdBook v0.5.4 构建。
- `cargo audit` 无漏洞（提交的 lockfile 把 `url` 2.5.8 升到 2.5.9，对应
  RUSTSEC-2026-0592）；剩余为允许的 unmaintained 告警（chrono、rustls-pemfile）。
- **2.8 观测/质量门禁（本批）**：`cargo fmt --all --check`、
  `cargo clippy --workspace --all-targets -- -D warnings`、
  `cargo test --workspace --no-fail-fast`、core/engine/host 的
  `RUSTDOCFLAGS="-D warnings" cargo doc` 与 `scripts/check-docs.sh` 全绿。新增覆盖：
  wire harness 4、配置聚合校验 7、观测量 3、关闭语义 4、错误码注册表 6、准入不变量
  指标 4、fuzz 逻辑 8（稳定工具链
  `cargo test --manifest-path fuzz/Cargo.toml --lib`）。本机 `scripts/conformance.sh`
  的 HTTP/1.1 矩阵 9/9 通过；h2spec 2.6.0 对同一进程测得已登记的单条已知偏差基线
  （非法 connection preface 时关闭 TCP 而未发 GOAWAY）。nightly cargo-fuzz 链接与
  Linux HTTP/2 h2c 矩阵仍只在 CI 验证（Windows libFuzzer 无入口点；本机 Windows curl
  无 nghttp2）。
- **与 rust-ef 2.8.0 的跨仓配对**：`scripts/check-pairing-rust-ef.sh` 用
  `[patch.crates-io]` 把 `rust-ef` / `rust-ef-sqlite` 指向本地 rust-ef checkout
  （`RUST_EF_PATH`）后编译本工作区，并执行 `cargo test -p rust-webx-ef`；可选 CI
  `pairing` job 仅在配置了 `vars.RUST_EF_PATH` / `vars.RUST_EF_REPO` 或
  `rust_ef_ref` dispatch 输入时运行同一脚本。发布顺序为**先 rust-ef 2.8.0、
  后 rust-webx 2.8.0**。证据边界：证明本工作区与 EF 适配层能针对给定 rust-ef
  源码树编译、且适配层的 SQLite 测试通过；不证明 crates.io 已发布制品、该源码树
  的 semver 门禁，也不覆盖真库 PostgreSQL/MySQL（由真库 job 覆盖）。

---

## [2.7.0] — 2026-11-22

> **English** · **简体中文** · 迁移指南：[docs/rust-webx/16-migration/upgrade-to-3.0.md](docs/rust-webx/16-migration/upgrade-to-3.0.md)

### English

Payment domain release. New opt-in `crates/payment/` group: one provider-neutral port for
Stripe, Alipay, WeChat Pay APIv3, PayPal, Adyen and Square, plus a rust-ef webhook inbox.
No framework crate or umbrella crate links a payment adapter; no provider SDK and no second
HTTP stack is introduced.

#### Added

- **`rust-webx-pay`** — `Money` (integer minor units + ISO 4217 exponent table),
  normalized statuses/refunds/events, `IPaymentProvider` + `IPaymentWebhookVerifier`,
  raw-body `PaymentWebhook`, the `Payments` runtime (TTL cache, single-flight,
  invalidation), `PaymentScope` (provider/tenant/account), `AccountSet` (multi-account),
  `IPaymentServiceFactory`/`IPaymentProviderFactory`/`IPaymentProviderBuilder` for
  runtime/DB-driven configuration, provider-specific ACKs and `PaymentError → webx::Error`
  mapping (`Unavailable` → 503).
- **Adapters** — `rust-webx-pay-stripe` (PaymentIntents/Checkout, refunds,
  `Stripe-Signature`), `-alipay` (page/wap/app/precreate, RSA2 signing, notify
  verification in public-key/certificate mode), `-wechatpay` (APIv3 request signing,
  platform-certificate/public-key verification, AEAD resource decryption), `-paypal`
  (Orders v2, OAuth2 token cache, verify-webhook-signature), `-adyen` (Checkout
  payments/captures/cancels/refunds, HMAC notifications), `-square` (Payments/Refunds,
  Payment Links, HMAC notifications). Each exposes `add_*()` (runtime config from its
  `IOptionsMonitor` section) and `add_*_with(options)` (static), plus a provider
  factory/builder; there is no `add_payments()` or `add_*_from_config()`.
- **`rust-webx-pay-ef`** — durable webhook inbox on rust-ef: unique `(provider,
  dedupe_key)`, lease-based claim, exponential backoff and dead-letter, drained by a
  `PeriodicBackgroundService`.
- **Multi-account / service-provider mode** — several merchant accounts per adapter,
  selection through `PaymentScope::account`, tenant through `PaymentScope::tenant`, and an
  account-scoped webhook route using the same `PaymentWebhook` type.
- **`payment_webhook!`** — generates the application-local request type Rust's orphan
  rule requires for `IRequest<Response>`, binds the raw body and declares the route; the
  handler stays application code. One type/macro handles both plain and account-scoped
  callbacks.
- **Frontend payment catalog** — `PaymentOption`/`PaymentFlowKind` plus
  `Payments::payment_options(_for)` list the currently enabled provider/account/method/flow
  combinations with display metadata (`provider_name`, `display_name`, `description`, `icon`
  key, `brand_color`, `sort_order`) and public client config (Stripe publishable key, Adyen
  client key, PayPal client id, Square application id, Alipay/WeChat app id). Secrets are
  never included.
- **`IHttpRequest::header_pairs()`** — object-safe, zero-copy header enumeration in
  `rust-webx-core`, implemented by the engine; signature schemes can read headers whose
  names are only known to the adapter.
- **`WebSocketLimits` lifetime bounds** — `with_max_connection_lifetime` closes a session
  at an absolute deadline (`1001`, traffic never extends it) and `close_at_token_expiry()`
  closes it when the upgrade token's `exp` passes (`1008`); the earlier deadline wins.
- **Realtime resume** — `IRealtimeEventStore` contract (`RealtimeEvent`,
  `ResumeRequest`, `ResumeFrame`) with an in-memory default (bounded per-subscription
  retention), registered for WebSocket hosts and replaceable with
  `HostBuilder::use_realtime_event_store`; `IConnectionRegistry` gains
  `user_connection_count`/`room_connection_count`/`tenant_connection_count`.
- **Presence transitions** — `IPresenceObserver` + `PresenceTransition`
  (`PresenceDirection`, `is_first`/`is_last`) delivered atomically from
  `register_observed`/`unregister_observed` and wired with
  `HostBuilder::use_presence_observer`, so online/offline announcements never need
  app-side connection counting.
- **`rust-webx-ef`** — new opt-in ORM-boundary crate (`crates/ef/webx`):
  sanitized `EFError → webx::Error` mapping (`map_ef_error`, `EfResultExt::map_ef`)
  with the 404/400/409/503/500 contract; core/umbrella still never link `rust-ef`,
  and the reference apps now re-export the shared mapper instead of duplicating it.
- **Notification ports** — `IEmailSender`/`ISmsSender` with `EmailMessage`/`SmsMessage`
  shape validation, `HostBuilder::use_email_sender`/`use_sms_sender`, and bounded
  `InMemoryEmailSender`/`InMemorySmsSender` capture senders for tests and local
  development. No sender is registered by default: a missing adapter fails at DI
  resolution instead of silently dropping messages.
- **At-rest secret protection** — `ISecretProtector` (`protect`/`unprotect`,
  `active_key_id`) with `AesGcmSecretProtector` (AES-256-GCM, self-describing
  `webx1:<key-id>:…` ciphertext, rotatable key ring loaded from `ISecretStore`,
  fail-closed on tamper/unknown key) and `HostBuilder::use_secret_protector`.
- **Advisory locks** — `IAdvisoryLock` (`AdvisoryLease`, `try_acquire` / `release` /
  `renew`, contention as `None`, expired leases reclaimable) with
  `HostBuilder::use_advisory_lock`, the process-local `InMemoryAdvisoryLock`, and
  `rust-webx-redis`'s `RedisAdvisoryLock` (`add_redis_advisory_lock`, `SET NX PX`
  plus compare-and-delete/extend Lua). No lock is registered by default.
- **Rate-limit policies** — `RateLimitPolicy` / `RateLimitRule` / `RateLimitRoute`
  / `RateLimitSubject` (exact > longest prefix > any; IP, user, tenant and API-key
  subjects with an IP fallback for anonymous traffic), installed after
  authentication with `HostBuilder::use_rate_limit_policy`; `use_rate_limiter`
  registers an explicit `IRateLimiter` for the pre-auth middleware. The 429
  contract (`Retry-After` + `problem+json`) is unchanged.
- **Request idempotency** — `Idempotency-Key` middleware over `IIdempotencyStore`
  (`begin` / `complete` / `release`), the bounded process-local
  `InMemoryIdempotencyStore`, `HostBuilder::use_idempotency` and an `Idempotency`
  configuration section. Retries replay the stored 2xx response
  (`Idempotency-Replayed: true`), concurrent duplicates get `409` + `Retry-After`,
  and non-2xx/oversized/streaming responses release the key.
- **Business metrics on `/metrics`** — `IMeter::snapshot()` (default empty) with
  `MetricSample`/`MetricKind`; `MemoryMeter` now stores structured series and
  enforces a cardinality cap (`Metrics.MaxSeries`, `with_max_series`,
  `dropped_series`). A metrics-enabled host registers a bounded `MemoryMeter` by
  default (an application `use_meter` wins) and `/metrics` renders application
  counters/gauges/histograms (histograms as `_count`/`_sum` summaries).
- **Transactional outbox** — new opt-in starter crate `rust-webx-outbox-ef`
  (`crates/outbox/ef`): the `OutboxMessage` entity + `register_outbox_model`,
  same-transaction `enqueue_outbox`/`enqueue_outbox_with_id` (a duplicate id is a
  `UniqueViolation`), the `IOutbox` lease-claim port, and an `OutboxDispatcher`
  (`PeriodicBackgroundService`) that dispatches rows through the process's
  `EventDispatcher` with exponential backoff, dead-lettering (including events
  with no handler) and retention cleanup. Registered with `add_outbox`.
- **`ClientInfo` request binding** — `#[fromclient] client: ClientInfo` binds the
  listener peer address plus trusted `X-Forwarded-For` resolution
  (`peer_addr` / `client_ip` / `forwarded_for`; the chain stays empty unless
  forwarding is trusted). A `ClientInfo` config section
  (`TrustForwardedHeaders`, `TrustedProxies`) drives a host middleware that
  resolves the right-most untrusted hop before authentication and binding.
- **Keyset pagination** — `KeysetRequest` (`cursor` + `page_size`, defaults and
  clamping like offset pagination, `decoded()`), the opaque `Cursor` codec
  (base64url JSON sort-key components, 4 KiB limit, typed `CursorError` → 400),
  and `CursorPage<T>` (`items` / `cursor` / `has_more`; `from_rows` derives an
  exact `has_more` from one extra fetched row). Offset `PagedRequest` /
  `PagedResult` remain unchanged.
- **Declarative request validation** — `#[validate(...)]` on `WebxRequestMeta`
  fields (`length`, `range`, `email`, `custom`) is expanded at compile time and
  runs after binding; failures land in the same `400 problem+json` `errors` map
  as binding errors. The same rules emit `ParamMeta::constraints`, so generated
  OpenAPI schemas carry `minLength` / `maxLength` / `minItems` / `maxItems` /
  `minimum` / `maximum` / `format: "email"`.
- **Instance-level authorization helpers** — `RequestUser::require_owner(owner_id)`
  and `require_same_tenant(tenant_id)` turn the authenticated identity into the
  401/403 decision (anonymous → 401; mismatch → 403 with a message that does not
  disclose the owner/tenant), while the resource lookup and ownership check stay
  in the handler.
- **MFA login-flow recipe** — `crates/auth/mfa/tests/login_flow_test.rs` pins the
  documented two-step issuance: a password-only token cannot satisfy a step-up,
  a replayed TOTP step is rejected, and recovery codes are single use.
- **Multi-instance presence recipe** — the WebSocket handbook now spells out the
  two supported patterns (shared presence store for authoritative counts, or
  `PresenceTransition` events for eventually-consistent counts) without
  introducing a second pub/sub.

#### Changed

- Workspace version `2.6.0` → `2.7.0` (all internal `=2.6.0` pins moved together).

### 简体中文

支付领域发布。新增按需引入的 `crates/payment/` 组：Stripe、支付宝、微信支付 APIv3、PayPal、
Adyen、Square 共用一套与供应商无关的端口，并提供基于 rust-ef 的回调 inbox。框架 crate 与
umbrella 均不链接支付适配器；不引入服务商 SDK，也不引入第二套 HTTP 栈。

#### 新增

- **`rust-webx-pay`** — `Money`（最小单位整数 + ISO 4217 指数表）、规范化的状态/退款/事件、
  `IPaymentProvider` + `IPaymentWebhookVerifier`、原始 body 的 `PaymentWebhook`、
  `Payments` 运行时（TTL 缓存、单飞、失效）、`PaymentScope`（provider/tenant/account）、
  多账户 `AccountSet`、`IPaymentServiceFactory`/`IPaymentProviderFactory`/
  `IPaymentProviderBuilder`（运行时/DB 配置）、各供应商 ACK，以及
  `PaymentError → webx::Error` 映射（`Unavailable` → 503）。
- **适配器** — `rust-webx-pay-stripe`（PaymentIntents/Checkout、退款、`Stripe-Signature`）、
  `-alipay`（page/wap/app/precreate、RSA2 签名、公钥/证书模式异步通知验签）、
  `-wechatpay`（APIv3 请求签名、平台证书/公钥验签、AEAD 资源解密）、`-paypal`
  （Orders v2、OAuth2 令牌缓存、verify-webhook-signature）、`-adyen`（Checkout
  payments/captures/cancels/refunds、HMAC 回调）、`-square`（Payments/Refunds、
  Payment Links、HMAC 回调）。每个适配器提供 `add_*()`（运行时读取配置节）与
  `add_*_with(options)`（静态），并注册 provider factory/builder；不再有
  `add_payments()`/`add_*_from_config()`。
- **`rust-webx-pay-ef`** — rust-ef 持久化回调 inbox：`(provider, dedupe_key)` 唯一、
  租约领取、指数退避与死信，由 `PeriodicBackgroundService` 消费。
- **多账户/服务商模式** — 一个适配器多商户账户；账户用 `PaymentScope::account` 选择，
  租户用 `PaymentScope::tenant`；账户级回调复用同一个 `PaymentWebhook` 类型。
- **`payment_webhook!`** — 生成应用本地请求类型（满足 Rust 孤儿规则）、绑定原始 body
  并声明路由；业务 Handler 仍写在应用里。单一类型/宏同时覆盖普通与账户级回调。
- **前端支付方式目录** — `PaymentOption`/`PaymentFlowKind` 与
  `Payments::payment_options(_for)` 返回当前启用的 provider/account/method/flow，包含
  展示元数据（`provider_name`、`display_name`、`description`、`icon` key、`brand_color`、
  `sort_order`）及公开客户端配置（Stripe publishable key、Adyen client key、PayPal
  client id、Square application id、支付宝/微信 app id）；绝不包含密钥。
- **`IHttpRequest::header_pairs()`** — `rust-webx-core` 新增对象安全、零拷贝的请求头枚举，
  由 engine 实现；验签代码可读取只有适配器才知道名字的请求头。
- **`WebSocketLimits` 生命周期上限** — `with_max_connection_lifetime` 在绝对期限关闭会话
  （`1001`，入站流量不续期）；`close_at_token_expiry()` 让会话在升级令牌 `exp` 到期时关闭
  （`1008`）；两者取较早者。
- **实时续传** — `IRealtimeEventStore` 契约（`RealtimeEvent`、`ResumeRequest`、`ResumeFrame`）
  与进程内存默认实现（按 subscription 有界保留）；WebSocket 宿主自动注册，可用
  `HostBuilder::use_realtime_event_store` 换成共享实现；`IConnectionRegistry` 新增
  `user_connection_count`/`room_connection_count`/`tenant_connection_count`。
- **presence 原子转移** — `IPresenceObserver` + `PresenceTransition`（`PresenceDirection`、
  `is_first`/`is_last`）由 `register_observed`/`unregister_observed` 在注册表锁内计算并交付，
  用 `HostBuilder::use_presence_observer` 接入；上线/下线广播不再需要业务侧数连接。
- **`rust-webx-ef`** — 新增按需 ORM 边界 crate（`crates/ef/webx`）：脱敏的
  `EFError → webx::Error` 映射（`map_ef_error`、`EfResultExt::map_ef`，覆盖
  404/400/409/503/500 契约）；core/umbrella 仍不链接 `rust-ef`，参考应用改为
  re-export 共享映射，不再各自复制。
- **通知端口** — `IEmailSender`/`ISmsSender` 与 `EmailMessage`/`SmsMessage` 形状校验；
  `HostBuilder::use_email_sender`/`use_sms_sender` 注册适配器；测试与本地开发用有界
  `InMemoryEmailSender`/`InMemorySmsSender` 捕获实现。默认不注册发送器：缺适配器在 DI
  解析阶段失败，而不是静默吞信。
- **静态密钥保护** — `ISecretProtector`（`protect`/`unprotect`、`active_key_id`）与
  `AesGcmSecretProtector`（AES-256-GCM、自描述 `webx1:<key-id>:…` 密文、可轮换 key ring
  从 `ISecretStore` 加载、篡改/未知 key 失败关闭），用 `HostBuilder::use_secret_protector`
  注册；默认不注册，避免明文静默落库。
- **分布式锁** — `IAdvisoryLock`（`AdvisoryLease`、`try_acquire`/`release`/`renew`，冲突返回
  `None`、过期租约可回收）与 `HostBuilder::use_advisory_lock`；进程内 `InMemoryAdvisoryLock`
  用于测试/单节点，`rust-webx-redis` 提供 `RedisAdvisoryLock`（`add_redis_advisory_lock`，
  `SET NX PX` + compare-and-delete/extend Lua）。默认不注册任何锁。
- **限流策略层** — `RateLimitPolicy`/`RateLimitRule`/`RateLimitRoute`/`RateLimitSubject`
  （exact > 最长 prefix > Any；Ip/User/Tenant/ApiKey 主体，匿名流量回退 IP），在鉴权之后
  用 `HostBuilder::use_rate_limit_policy` 安装；`use_rate_limiter` 为鉴权前的中间件注册
  显式 `IRateLimiter`。429 契约（`Retry-After` + `problem+json`）不变。
- **请求级幂等** — `Idempotency-Key` 中间件 + `IIdempotencyStore`（`begin`/`complete`/`release`）
  与有界进程内 `InMemoryIdempotencyStore`；`HostBuilder::use_idempotency` 与 `Idempotency`
  配置节。重试重放已存的 2xx 响应（`Idempotency-Replayed: true`），并发重复请求返回
  `409` + `Retry-After`；非 2xx/超大/流式响应会释放键。
- **`/metrics` 业务指标导出** — `IMeter::snapshot()`（默认空）与 `MetricSample`/`MetricKind`；
  `MemoryMeter` 改为结构化存储并加基数上限（`Metrics.MaxSeries`、`with_max_series`、
  `dropped_series`）。启用 metrics 时 host 默认注册有界 `MemoryMeter`（应用 `use_meter` 优先），
  `/metrics` 渲染业务 counter/gauge/histogram（histogram 为 `_count`/`_sum` summary）。
- **事务性 outbox** — 新增按需 starter crate `rust-webx-outbox-ef`（`crates/outbox/ef`）：
  `OutboxMessage` 实体 + `register_outbox_model`；同事务 `enqueue_outbox`/
  `enqueue_outbox_with_id`（重复 id 为 `UniqueViolation`）；`IOutbox` 租约领取端口；
  `OutboxDispatcher`（`PeriodicBackgroundService`）通过 `EventDispatcher` 派发，
  指数退避、死信（含无 handler 事件）与保留期清理；用 `add_outbox` 注册。
- **`ClientInfo` 请求绑定** — `#[fromclient] client: ClientInfo` 绑定监听器对端地址与
  受信 `X-Forwarded-For` 解析（`peer_addr`/`client_ip`/`forwarded_for`；未启用信任时转发链为空）。
  `ClientInfo` 配置节（`TrustForwardedHeaders`、`TrustedProxies`）驱动 host 中间件在鉴权与绑定前
  解析最右侧非可信 hop。
- **Keyset 分页** — `KeysetRequest`（`cursor` + `page_size`，默认/夹紧与 offset 一致，
  `decoded()`）与不透明 `Cursor` 编解码（base64url JSON 排序键分量、4 KiB 上限、
  `CursorError` → 400），以及 `CursorPage<T>`（`items`/`cursor`/`has_more`；`from_rows` 用多取的
  一行给出精确 `has_more`）。offset `PagedRequest`/`PagedResult` 保持不变。
- **声明式校验** — `WebxRequestMeta` 字段上的 `#[validate(...)]`（`length`、`range`、`email`、
  `custom`）在编译期展开并在绑定后执行；失败进入与绑定错误相同的 `400 problem+json`
  `errors`。同一份规则写入 `ParamMeta::constraints`，生成的 OpenAPI schema 带
  `minLength`/`maxLength`/`minItems`/`maxItems`/`minimum`/`maximum`/`format: "email"`。
- **实例级授权 helper** — `RequestUser::require_owner(owner_id)` 与
  `require_same_tenant(tenant_id)` 把身份收敛成 401/403 判定（匿名 401；不匹配 403，
  文案不泄露 owner/tenant）；资源加载与归属校验仍留在 Handler。
- **MFA 登录流配方** — `crates/auth/mfa/tests/login_flow_test.rs` 固化两步签发：仅密码的 token
  不能满足 step-up、TOTP 重放被拒、恢复码单次使用。
- **多实例 presence 配方** — WebSocket 手册明确两种支持形态（共享 presence store 作为权威计数，
  或 `PresenceTransition` 事件做最终一致计数），不引入第二套 pub/sub。

#### 变更

- workspace 版本 `2.6.0` → `2.7.0`（所有内部 `=2.6.0` 钉版同步）。

## [2.6.0] — 2026-11-21

> **English** · **简体中文** · 迁移指南：[docs/rust-webx/16-migration/upgrade-to-3.0.md](docs/rust-webx/16-migration/upgrade-to-3.0.md)

### English

Dependency-alignment release. The workspace `rust-ef` dependency moves to 2.6 (the
ORM removed its `webx` feature and no longer depends on a web framework). There is
no public API change and no source migration: the `EFError -> webx::Error`
translation stays at the application boundary, and the reference apps now
implement the full sanitized contract.

#### Changed

- **rust-ef 2.6**: workspace dependency `2.5` -> `2.6`. rust-ef 2.6 removed its
  `webx` feature; this repository keeps **no** EF bridge at all (no core/umbrella
  feature, no shim, no `[patch.crates-io]`).

#### Fixed

- **Reference app mapping (docbit/dmbit `handlers/src/db.rs`)**: `UniqueViolation`
  maps to sanitized 409 (no constraint/table), `Connection`/`Transaction` to
  retryable 503 with fixed text, `Query`/`TypeConversion` to 400 with fixed text,
  and 5xx keeps the EF message plus its source chain for the operator log only.

### 简体中文

> 依赖对齐发布：workspace `rust-ef` 升到 2.6（ORM 移除了 `webx` feature，不再依赖
> Web 框架）。**无公开 API 变更、无源码迁移**：`EFError -> webx::Error` 映射仍在
> 应用边界，参考应用补齐完整脱敏契约。

#### 变更

- **rust-ef 2.6**：workspace 依赖 `2.5` → `2.6`。rust-ef 2.6 移除了 `webx`
  feature；本仓保持**零 ef 桥接**（core/umbrella 不加 feature、不恢复 shim 或
  `[patch.crates-io]`）。

#### 修复

- **参考应用映射（docbit/dmbit `handlers/src/db.rs`）**：`UniqueViolation` → 409
  （脱敏，不回显 constraint/table）；`Connection`/`Transaction` → 可重试 503 固定
  文案；`Query`/`TypeConversion` → 400 固定文案；5xx 原文与 source 链只进日志。

## [2.5.0] — 2026-11-21

> 2.5.0 是 2.x 的加性发布，汇总运行信任、实时执行模型、认证体系、事件/消息适配器、
> AI 网关与测试基座。详细说明见 [`docs/design/release-notes-2.5.0.md`](docs/design/release-notes-2.5.0.md)。

### Added

#### 运行信任

- **Strict configuration.** `try_load_appsettings`, `try_bind_root` and
  `try_bind_config` reject wrong value types, unknown keys inside framework
  sections and unreadable files at startup (`ConfigError` / `ConfigSources`);
  the host logs the effective sources with values redacted. Live option reload
  keeps the last known good snapshot on a bad file.
- **Inbound deadline.** `Server.RequestTimeoutSeconds` defaults to **30s**;
  Production refuses `0` (a hung handler would pin a connection and permit).
- **Bounded admission.** A connection that cannot get an admission slot within
  `Server.AdmissionTimeoutMs` (default 1000ms) receives `503` + `Retry-After`
  instead of waiting unbounded. New counters expose admitted/rejected/waiting.
- **RED observability.** `/metrics` exposes per-route request/status counters,
  a fixed-bucket latency histogram, inflight gauge, timeout counter and bounded
  label overflow; cancelled requests are counted.
- **Machine-readable errors.** `problem+json` (RFC 9457) carries `code`,
  `retryable`, `instance` and `traceId` (same value as `x-trace-id`);
  `Error::code()` / `Error::retryable()` expose the same mapping in Rust.
- **Rate-limit port.** `IRateLimiter` / `RateLimitDecision` plus a sharded
  in-memory implementation; `RateLimitMiddleware::with_limiter` accepts any
  store (e.g. Redis-backed).
- **Compile-time route diagnostics.** Route macros require
  `impl IRequest<Response>` and fail at compile time; startup sorts routes
  deterministically and rejects conflicts; compile-fail snapshots live in
  `crates/webx/tests/ui/`.
- **Allocation budget.** `crates/alloc-probe` compares the framework against a
  bare-hyper floor in the same window; CI gates the framework tax (≤12).

#### 实时执行模型

- **WebSocket.** Engine upgrade support, `IConnection` message abstraction,
  host `use_websocket`, connection scope shared by session state, and
  message-as-request dispatch through the same handler pipeline.
- **Fan-out and presence.** In-process connection registry/Hub with rooms and
  presence TTL; cross-node fan-out reuses `IEventPublisher` via
  `RealtimeFanoutEvent` (no second pub/sub port).
- **Backpressure.** Bounded per-connection outbound queue, inbound sliding-window
  limits, frame size cap, idle timeout and connection statistics.
- **Realtime metrics.** Connection gauges, message in/out counters, drops,
  rate limits, close codes and dispatch latency, mirrored into `IMeter`.
- **Media/object-store ports.** `IMediaPlaneAdapter` and `IObjectStore` with
  in-memory implementations and reusable contract suites.

#### 认证

- **JWT hardening:** configurable `iss`/`aud`/algorithm allow-list, clock skew,
  max token size, empty-`sub` rejection and `InvalidTokenBehavior::Reject`.
- **JWKS + asymmetric JWT:** RS256/ES256/PS256, `kid` rotation, cached
  single-flight fetch, SSRF guard, JWKS-only startup.
- **Cookie sessions + CSRF:** opaque 256-bit session ids, `ISessionRepository`,
  `__Host-` cookies, double-submit CSRF and `Sec-Fetch-Site` checks.
- **Login hardening:** NIST password policy, locked throttle/backoff, dummy-hash
  verification for unknown accounts, bounded Argon2 concurrency (503 on
  saturation) and the `IPasswordHasher` port with Argon2id default.
- **Revocation:** TTL-aware `ISessionStore::revoke_until`, negative cache and
  fail-closed backend errors.
- **Refresh tokens:** SHA-256 hashed storage, atomic rotation, reuse detection
  with family revocation and the `rust-webx-auth-ef` durable store.
- **Federated identity:** OIDC (discovery, PKCE, id_token validation, private-CA
  HTTPS) and OAuth2 social login with provider presets.
- **MFA and strong auth:** RFC 6238 TOTP, recovery codes, `amr`/`acr`, WebAuthn
  passkey verification, mTLS, DPoP (RFC 9449) and HTTP Basic.
- **Protocols:** RFC 7662 introspection, RFC 7009 revocation, per-path scheme
  policy, auth metrics and the `JwksHealthCheck` readiness probe.

#### 事件与适配器

- **Asynchronous events.** `IMediator::publish` encodes an `EventMessage` and
  hands it to `IEventPublisher`; the default in-memory transport is a bounded
  queue drained by a background service, and a full queue returns retryable 503.
  `#[handler]` / `#[handler(inject)]` on `impl IEventHandler<T>` emit event
  registrations, so no explicit DI registration is needed.
- **Wire envelope.** `EventMessage::to_wire` / `from_wire` (version byte + name +
  payload) is the single broker-neutral framing.
- **Delivery semantics.** Per-adapter reconnect/backoff, bounded retries,
  dead-letter outlets (Redis stream, Rabbit DLX), manual ack and readiness probes.
- **`rust-webx-redis`:** Redis Streams consumer-group transport (at-least-once,
  `XAUTOCLAIM` before delivery, trimmed-PEL cleanup, `max_deliveries` →
  dead-letter, `MAXLEN ~` bound) plus a Redis `IDistributedCache`.
- **`rust-webx-nats`:** core NATS publisher/subscriber with optional queue group.
- **`rust-webx-rabbitmq`:** quorum topic queue, `x-delivery-limit`, optional DLX,
  publisher confirms and requeue-on-failure.
- **`rust-webx-kafka`:** pure-Rust single-partition follower (no librdkafka;
  plaintext brokers, no consumer-group offset commits).
- **Event metrics:** published/failed/dispatch-failed/dropped/dead-lettered and
  reconnect counters. Adapters run the shared `EventTransportContract`.

#### AI 网关与测试基座

- **`rust-webx-ai`:** model catalog/routing, chat IR + provider port, OpenAI
  adapter, quota pre-charge/settlement, usage metering, resilience (retry,
  circuit breaker, bulkhead), caching/single-flight, secrets and gateway cache.
- **`rust-webx-ai-ef`:** durable usage ledger, outbox and rollup workers.
- **`rust-webx-ai-gateway`:** OpenAI-compatible `/v1/chat/completions`
  (JSON + SSE) and `/v1/models`.
- **`TestHost` + `IClock`/`TestClock` + chaos hooks:** in-process dispatch
  without binding a port, deterministic time and failure injection.
- **Reference apps:** `docbit`, `dmbit` and the new `imbit` real-time reference.

### Changed

- `IMediator::publish` is now fire-and-forget and transport-agnostic; it no
  longer awaits handlers or returns their errors.
- The direct HTTP fast path is decided per host from a DI marker instead of a
  process-global counter, so unrelated registrations cannot downgrade a host.
- Options reload keeps the last known good snapshot on invalid input.
- The in-memory rate limiter shards token buckets and removes the global mutex.
- Internal `rust-webx*` crates are pinned with exact `=x.y.z` versions.
- Optional capabilities are grouped under `crates/ai`, `crates/auth`,
  `crates/cache` and `crates/queue`; package names and import paths are unchanged.

### Fixed

- HTTPS through the rustls connector no longer fails with
  `invalid URL, scheme is not http`.
- Config binding failures, disabled deadlines and unbounded admission no longer
  degrade silently.
- Problem details now carry stable codes; cancelled requests are counted.
- Route macros no longer fall back to a response type of `()` on a wrong impl.
- Redis PEL growth and poison-message requeue loops are bounded; RabbitMQ
  quorum queue arguments are consistent between publisher and consumer; NATS
  connect errors redact credentials.

### Removed

- The duplicate `IPubSub` port: cross-node real-time fan-out now uses
  `IEventPublisher` / `RealtimeFanoutEvent`.
- The process-global direct-dispatch switch and the legacy name-keyed dispatch
  fallback for macro-produced routes.

### Security

- Upgraded `rsa` to 0.10, clearing RUSTSEC-2023-0071 (Marvin timing side
  channel). `cargo audit` now reports **0 vulnerabilities**.

### Performance & verification

- Allocation tax: **10.42 allocs/request** (hyper floor 6.00; CI upper bound 12).
- HS256 validation: p50 **2.3µs** / p99 **3.9µs** (release, 20k).
- Authentication layer allocation delta: **0**.
- Wire p99: **165µs** in the nightly budget test.
- Soak: 30min and 1h Linux runs with zero failures, RSS stable (~17.7MB).
- Chaos 3/3, fuzz-lite zero panics, broker contract tests in nightly CI.
- Full workspace `fmt` / `clippy -D warnings` / `test` green on three platforms.

## [2.1.0] — 2026-11-20

Production-stability pass over the 2.0 engine. No feature expansion; the focus
is admission, lifecycle, error semantics and wire-level verification.

> **English** · **简体中文** · 迁移指南：[docs/rust-webx/16-migration/upgrade-to-3.0.md](docs/rust-webx/16-migration/upgrade-to-3.0.md)

### English

- **Performance:** the direct JSON path remains allocation-free; same-host
  `/allocs` measurement is **22.06** allocs/request vs **23.06–23.07** in 2.0
  (about 4.4% fewer allocations). Median throughput is within co-tenant noise
  (152k vs 158k RPS in the 5-round 4-worker run; 2.0/2.1 latency effectively
  unchanged). Allocation counts are now tracked by the allocation gate (see `docs/design/alloc-budget.md`).
- **Global connection admission.** One `Semaphore(App.MaxConnections)` per
  `Server`, shared by every http/https listener and worker. Removes the
  per-worker floor that could exceed the documented global cap and the
  per-listener multiplication of workers/connections.
- **Pre-request deadline.** `Server.FirstRequestTimeoutSeconds` (default 10;
  zero uses the secure default) closes TCP connections that send no first
  request/partial h2 preface; HTTP/2 keepalive is enabled. Silent sockets can no
  longer pin admission permits forever.
- **Single-use lifecycle guards.** Host states Built/Running/Disposed/Failed;
  concurrent/repeated runs return `Error::State`. Dropping/aborting a run future
  triggers immediate shutdown; a join timeout no longer disposes DI while a
  blocking worker can still resolve it. Engine/Worker/Host cleanup is guarded
  against cancellation.
- **Error before after-hooks.** `Pipeline::execute_with_error` writes the error
  response before the reverse `after` pass, so metrics, tracing and compression
  observe the real 4xx/5xx status. The writer strips stale entity headers
  (`cache-control`, `content-encoding`, validators, ranges) and preserves
  `Allow`/CORS/`Vary`; a panicking custom writer falls back to a fixed 500.
- **File/cache/204/304 semantics.** Files with validators revalidate; unknown
  streams are `no-store`; 204/1xx never carry `Content-Length`; 304 keeps cache
  metadata instead of gaining `no-store`.
- **SSE and drain.** Shutdown cancellation is polled as a waker (quiet streams
  emit `event:server_shutdown`); drain budget is per worker, not process-global.
- **Request contracts.** JSON body binding returns 415 for non-JSON media types;
  route `#[max_body_size]` can tighten **or raise** the global cap and works for
  declared `Content-Length`; `#[max_body_size(64 * 1024 * 1024)]` accepts
  compile-time integer expressions.
- **No handler annotation for mediator-only code.** `#[handler]` now generates
  its direct bridge without naming the response type: the **route** supplies a
  route-owned response encoder. A handler whose Request has no route therefore
  compiles with any `Rsp` and is treated as `IMediator`-only; an actual HTTP
  route with a non-`Serialize` response still fails at the route encoder. This
  removes the 2.0 source break without a new `#[handler(in_process)]` syntax,
  while keeping the direct path allocation-free. The obsolete concrete
  `webx_host::HttpContext`/`HttpRequest`/`HttpResponse` runtime types were
  removed (use `webx_engine::EngineContext`; `IHttpContext` remains the
  application contract). `Router::try_insert` lets Host fail startup on route
  conflicts.
- **Verification.** New wire-level tests cover silent-connection admission,
  cross-listener global MaxConnections, TLS handshake + HTTP, h2c, Host
  single-use lifecycle, error-writer panic, metrics error status, media/body
  contracts and quiet SSE shutdown.

### 简体中文

- 连接准入改为每个 `Server` 一个全局 `MaxConnections` Semaphore，所有
  http/https listener 和 worker 共享；删除会被放大上限的 per-worker floor。
- 新增首请求超时（默认 10s，0 回退默认）和 h2 keepalive，静默连接不再永久
  占用 admission permit。
- Host 单次运行状态机 + run drop guard；并发/重复运行返回 `Error::State`；
  join 超时不再提前 dispose DI。
- 错误响应在 after hooks 之前写入，metrics/tracing/compression 看到真实状态；
  错误 writer 清理表示相关 headers，writer panic 回退 500。
- 文件缓存策略、204/304、SSE shutdown waker、per-worker drain 修正。
- JSON 非 JSON Content-Type 返回 415；route body limit 可收紧/放宽；
  无路由的 `#[handler]` 自动作为 mediator handler，无需额外标注。
- 删除旧具体 `HttpContext` 运行时类型；`IHttpContext` 应用契约不变。
- 新增 admission/TLS/h2/lifecycle/错误写入/media/SSE 等 wire-level 测试。

## [2.0.0] — 2026-11-20

The execution engine is rewritten from first principles on the new
`rust-webx-engine` crate. The application-facing API (macros, `Host`,
`IRequest`/`IRequestHandler`, `IMiddleware`, results, auth/config/DI) is kept;
the old single-runtime hyper loop is gone.

> **English** · **简体中文** · 迁移指南：[docs/rust-webx/16-migration/upgrade-to-3.0.md](docs/rust-webx/16-migration/upgrade-to-3.0.md)

### English

#### Execution engine

- **One acceptor, N current-thread worker runtimes.** `rust-webx-engine` binds a
  listener, accepts on the caller runtime, and hands each `std::net::TcpStream`
  to a worker thread through a bounded channel (`into_std`/`from_std`, no
  `unsafe`). Each worker owns a `current_thread` tokio runtime and one hyper
  `auto::Builder`; connections never migrate cores. `Server.Workers`
  (or `APP__Server__Workers`) selects the count; `0` = available parallelism,
  clamped to 1..=64.
- **Compile-time direct dispatch.** `#[handler]` now emits a `HandlerDispatch`
  bridge keyed by `(Req, Rsp)`; the host joins it with the route at build time
  and bypasses the mediator, `Box<dyn Any>` request/response and DI scope when no
  typed pipeline behavior is registered. `#[handler(inject)]` still creates one
  request scope; authorization from `#[authorize]` runs before the direct call.
- **Zero-copy request context.** hyper's request `Parts` are moved, method/URI
  borrowed, query parsed lazily, body kept as `Incoming` with a single-frame
  `Bytes` fast path, and JSON results stored as `Bytes`. The response uses one
  `HeaderMap`; static framework headers use `HeaderName::from_static` /
  `HeaderValue::from_static`.
- **Onion pipeline preserved.** `IMiddleware::invoke`/`after` order,
  short-circuiting, slot ordering and first-error-wins semantics are unchanged;
  the loop no longer allocates a per-request `Vec<Arc>`.
- **Graceful lifecycle.** Accept stop → SSE token cancel → connection
  `graceful_shutdown` → drain budget (`stream_drain_timeout` while streams are
  active) → worker join → hosted-service stop → DI dispose. TLS termination and
  HTTP/2 (ALPN) run inside the worker; a stalled TLS handshake is bounded.
- **Security headers and `cache-control` defaults** are applied by the engine
  finalizer with static header values, so they stay correct on 4xx/5xx and cost
  no per-request parse/allocation.

#### Performance (same host, 22 logical cores, 1 KiB JSON, 32 keep-alive conns)

| Version | RPS | Allocations/request |
|---|---:|---:|
| 1.5.0 (single multi-thread runtime) | ~88k | ~50.1 |
| 2.0.0 (worker pool + direct dispatch) | **~220–238k** burst / ~140k sustained under co-tenant load | **~23.1** |

The worker model alone reached ~219k in the hyper-only prototype; 2.0 keeps the
same architecture while preserving middleware/auth/DI semantics. Benchmarks are
sensitive to co-tenant CPU load, so the CI performance gate uses relative
thresholds against a pinned baseline rather than a single absolute number.

#### Compatibility / migration

- All existing route/handler macros, request binding attributes,
  `WebxRequestMeta`, `HostBuilder`, `IRequest`, `IRequestHandler`,
  `IMiddleware`, `IPipelineBehavior`, `IHttpContext`, result types,
  auth/session/password ports, paging, `OperationResult`, health, SPA, OpenAPI
  and config keys remain source-compatible.
- `EndpointResult` is a new additive internal type (re-exported by `webx`) used
  by direct dispatch; custom `IRequest<Response>` endpoints are unchanged.
- `RouteEntry` gained an additive `rsp_type_id` field (constructors default it
  to `None`). Hand-written struct literals should use `RouteEntry::new` /
  `with_response_type_id`.
- `#![forbid(unsafe_code)]` is preserved; the engine contains no `unsafe`.
- The old host-internal `serve_http`/`serve_https`, `HttpContext` runtime loop
  and name-keyed dispatch are no longer used by `Host`. The public
  `webx_host::HttpContext` type remains for downstream test/extension code.
- `rust-webx-engine` is a new published crate, published before
  `rust-webx-host`; the workspace is 2.0.0.

#### Verification

- `cargo test --workspace`: 999+ tests, 0 failures (1.5 host/webx/core suites +
  docbit/dmbit e2e), including routing, binding, auth, CORS, security headers,
  SSE, TLS/HTTP2, shutdown/drain, files/Range and request timeouts.
- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check`.
- `cargo doc --workspace --no-deps` with `-D warnings`.

### 简体中文

- 执行引擎全面重写到新 crate `rust-webx-engine`：单 acceptor + N 个
  `current_thread` worker 运行时、安全连接交接（无 `unsafe`）、每 worker 一个
  hyper `auto::Builder`，连接不再跨核迁移。
- `#[handler]` 生成按 `(Req, Rsp)` 键控的 `HandlerDispatch`，无 typed
  pipeline 时绕过 mediator / `Box<dyn Any>` / 请求级 DI scope；`inject` 仍按需
  开一个 scope，`#[authorize]` 鉴权在直达调用前执行。
- 请求上下文零拷贝：移动 hyper `Parts`，方法/URI 借用，query 惰性解析，body
  保持 `Incoming`，JSON 结果直接存 `Bytes`；框架静态响应头零分配。
- 中间件洋葱语义、槽位顺序、短路、错误传播完全保留，但去掉了每请求
  `Vec<Arc>` 分配。
- 生命周期：停止 accept → 取消 SSE token → `graceful_shutdown` → 按预算
  drain → join worker → 停 hosted → dispose DI；TLS/HTTP2 在 worker 内完成。
- 同机 32 连接 1 KiB JSON 基准：1.5 约 88k RPS / 50.1 allocs，2.0 约
  **220–238k** 峰值、~140k 持续（受同机负载影响）/ **23.1** allocs。
- 应用侧公共 API 保持兼容；新增 `EndpointResult` 与
  `RouteEntry::rsp_type_id`（增量），`rust-webx-engine` 为新发布 crate。
- `cargo test --workspace` 999+ 通过、0 失败；clippy/fmt/doc 全绿。

## [1.5.0] — 2026-11-20

Security/correctness hardening plus the 1.5 capability line (SSE, outbound client,
API-key auth, secrets, metrics, options hot reload, request-body streaming,
error writers, HTTP/2, AI gateway crates).

> **English** · **简体中文** · 迁移指南：[docs/rust-webx/16-migration/upgrade-to-3.0.md](docs/rust-webx/16-migration/upgrade-to-3.0.md)

### English

#### Security / correctness (behaviour changes)

- **Route dispatch is keyed by `TypeId`, not by request-type name.** Two request
  types that share a bare name in different modules previously collided in the
  dispatch map, so one route could execute another module's handler. The endpoint
  macros now also submit `RouteDispatchById`; `Host::build` keys dispatch by
  `TypeId` and falls back to the name only for hand-written fixtures. Duplicate
  `(method, path)` routes, duplicate dispatch bridges and duplicate `#[handler]`
  registrations now fail startup instead of being silently dropped.
- **`#[authorize]` is order-independent.** It is rewritten to `__webx_authorize`,
  the same mechanism `#[display]` already used, so writing it above `#[get]` no
  longer produces a public endpoint.
- **Dynamic authorization fails closed.** A route declaring
  `#[authorize(dynamic)]` without a registered `IDynamicAuthorizer` now fails
  `Host::build` (previously it silently degraded to "authenticated only").
- **`resource_auth_middleware` is deprecated and fails closed.** It runs before
  routing and can never see the matched pattern; it now returns `403` with a
  pointer to `#[authorize]` / `IDynamicAuthorizer` instead of allowing every
  request.
- **CORS is opt-in.** `Cors.Origins` defaults to an empty list (no CORS headers)
  instead of `"*"`, so a default Production host starts. The Production wildcard
  guard now validates the effective `CorsConfig` (including `use_cors`) and
  responses add `Vary: Origin`.
- **Header read timeout is enforced.** The hyper `auto` builders install a
  `TokioTimer` and bound the header buffer; previously hyper's 30s default was
  silently disabled, leaving the server open to slowloris. The accept loop also
  observes shutdown while waiting for a connection slot.
- **Panic isolation.** `#[handler(inject)]` uses the fallible rust-dix
  constructor and maps `RdiError` to `Error::Di`; the host catches middleware or
  handler panics at the request boundary and returns `500` (the workspace release
  profile no longer uses `panic = "abort"`).
- **Multi-valued response headers.** `IHttpResponse::append_header` and a
  `HeaderMap`-backed host response allow two `Set-Cookie` headers and correct
  `Vary` composition.

#### Performance

- Request and response headers use `http::HeaderMap` directly: no per-header
  `String` copies and no second copy into hyper.
- Request bodies are buffered as `Bytes`; `body_bytes_shared` lets JSON binding
  use the first read without copying the whole body again.
- Development access logging moved from `info` to `debug` (one synchronous line
  per request previously dominated local throughput by ~12x).
- Rate-limiter eviction is FIFO O(1) instead of an O(tracked IPs) scan;
  `MemoryCache` is bounded at 100,000 entries by default.
- AI quota leases are sized from the request deadline and released on
  cancellation; client-caused 4xx errors no longer trip shared model circuits.

#### Added

- `rust-webx-client` (pooled HTTP/1.1+HTTP/2 client, SSE, SSRF guard),
  `rust-webx-ai`, `rust-webx-ai-ef`, `rust-webx-ai-gateway`.
- `SseResult`/`SseEvent`, `#[max_body_size]`, `IErrorResponseWriter`,
  `IAuthenticationScheme`/API-key auth, `ISecretStore`, `IMeter`,
  `IOptionsMonitor` + `watch_options_file`, `IHealthCheck` async checks,
  `PagedRequest::from_parts`, `OperationResult`.
- Inbound/outbound HTTP/2 and request-level `Server.RequestTimeoutSeconds`.

#### Fixed

- AI gateway billing ids are server-generated UUIDs; the client `x-request-id`
  is correlation metadata only. Provider responses without a usage block settle
  with the reserved estimate instead of zero tokens.
- Gateway readiness fails on an empty model catalog.

### 简体中文

#### 安全 / 正确性（行为变更）

- **路由派发改为按 `TypeId` 键控**，不同模块中同名的请求类型不会再互相覆盖；
  重复路由、重复派发桥、重复 `#[handler]` 会在启动期失败。
- **`#[authorize]` 与书写顺序无关**；写在 `#[get]` 上方也不会再静默变成公开端点。
- **动态授权 fail-closed**：有 `#[authorize(dynamic)]` 路由但未注册
  `IDynamicAuthorizer` 时启动失败，不再降级为“仅已认证”。
- **`resource_auth_middleware` 弃用并 fail-closed**：它在路由前运行、看不到匹配
  模式，现在返回 403 并提示改用 `#[authorize]` / `IDynamicAuthorizer`。
- **CORS 默认关闭**：`Cors.Origins` 默认空列表，默认 Production host 可以启动；
  wildcard 守卫校验实际生效的 `CorsConfig`（含 `use_cors`），并发送
  `Vary: Origin`。
- **启用 header 读超时**（hyper `TokioTimer` + 有界 header 缓冲），修复 slowloris；
  accept 循环在等待连接槽时也能观察到停机。
- **panic 隔离**：`#[handler(inject)]` 改用可失败构造器；host 在请求边界捕获
  panic 并返回 500；workspace release 不再 `panic = "abort"`。
- **支持多值响应头**：`append_header` + `HeaderMap`，可发送多个 `Set-Cookie`
  并正确组合 `Vary`。

#### 性能

- 请求/响应头直接使用 `http::HeaderMap`，去掉每头两次 `String` 复制与二次拷贝。
- 请求体以 `Bytes` 缓冲；`body_bytes_shared` 避免 JSON 绑定时的整段复制。
- 开发模式访问日志从 `info` 降到 `debug`（此前默认每请求同步一行，实测拖慢约 12 倍）。
- 限流器改为 O(1) FIFO 淘汰；`MemoryCache` 默认上限 10 万条。
- AI 配额租约按请求 deadline 计算并在取消时释放；客户端 4xx 不再触发共享模型熔断。

#### 新增 / 修复

- 新增 `client`、`ai`、`ai-ef`、`ai-gateway` 四个 crate；SSE、出站客户端、
  API Key、密钥、指标、热配置、请求体流式、错误写入器、HTTP/2 等能力。
- AI 计费 id 改为服务端生成；provider 未返回 usage 时按预估值结算；
  空模型目录时 readiness 失败。

## [1.4.0] — 2026-11-14

Additive release on the 1.3 line: every existing API keeps working (`register`, `add_health_check`,
the synchronous health registry view, the default `ITokenIssuer`). It fixes the two upstream defects
found by the Travel production verification.

> **English** · **简体中文** · 迁移指南：[docs/rust-webx/16-migration/upgrade-to-3.0.md](docs/rust-webx/16-migration/upgrade-to-3.0.md)

### English

#### Fixed

- **WEBX-01 — a custom `ITokenIssuer` reaches the final JWT signing secret.** `Jwt.SecretFile`
  used to be resolved after the host's own authentication setup, so an issuer built during
  application DI registration could see an empty `Jwt.Secret` in a `SecretFile`-only deployment and
  mint tokens the same host's `JwtAuth` could not verify. The host now resolves the secret and
  registers `webx::JwtSecret` as a DI singleton **before** application service configurations run,
  and adds `HostBuilder::register_with_options(|svc, options| ...)`, which passes the final
  `AppOptions` (`options.jwt.secret` already resolved) to the registration closure. `register` keeps
  its signature and behaviour. The default issuer, `JwtAuth` and the Production weak-secret guard
  still read the same resolved value. `JwtSecret` is registered only when a non-empty secret was
  resolved: resolving it in an unconfigured deployment fails start-up instead of signing with `""`.
- **WEBX-02 — asynchronous health checks with per-check timeouts.** New `webx::IHealthCheck` port
  (`name()` + `async check()`) and `HostBuilder::add_health_check_async` /
  `add_health_check_async_with_timeout` (default `DEFAULT_HEALTH_CHECK_TIMEOUT` = 3 s).
  `HealthCheckRegistry::snapshot_async` / `overall_status_async` / `all_healthy_async` /
  `build_health_response_async` evaluate all checks concurrently (`futures_util::future::join_all`),
  preserve registration order, and apply an independent timeout to every async check. A timeout
  becomes `HealthStatus::fail("timed out after ...")`, so `/health` and `/health/ready` answer 503;
  `fail` dominates `warn`, and `warn` keeps 200.

#### Compatibility

- Existing synchronous API (`register`, `snapshot`, `overall_status`, `all_healthy`,
  `build_health_response`, `add_health_check`) is source- and behaviour-compatible. The sync view
  evaluates sync checks and reports each async check as
  `warn("async health check '<name>' requires async evaluation")` — never `fail` — so existing call
  sites compile unchanged and registering an async probe does not break readiness.
- `register`'s signature is unchanged; `register_with_options` is additive. No item was removed or
  renamed; the workspace version is 1.4.0.

### 简体中文

#### 修复

- **WEBX-01 — 自定义 `ITokenIssuer` 能拿到最终的 JWT 签名密钥。** 此前 `Jwt.SecretFile` 在 host
  自身认证装配之后才解析，应用在 DI 注册期构造 issuer 时可能看到空的 `Jwt.Secret`，在只配置
  `SecretFile` 的部署里签出的 token 无法被同一 host 的 `JwtAuth` 验证。现在 host 在**应用 service
  配置之前**完成密钥解析并把 `webx::JwtSecret` 以 DI 单例注册；新增
  `HostBuilder::register_with_options(|svc, options| ...)`，闭包直接拿到已解析的最终 `AppOptions`
  （`options.jwt.secret`）。`register` 的签名与行为保持不变；默认 issuer、`JwtAuth` 与 Production
  弱密钥守卫读取的仍是同一个最终值。仅在解析出非空密钥时注册 `JwtSecret`：未配置密钥时工厂解析它会
  启动失败，而不是用空密钥签名。
- **WEBX-02 — 异步健康检查 + 每项独立超时。** 新增 `webx::IHealthCheck` 端口（`name()` +
  `async check()`）与 `HostBuilder::add_health_check_async` /
  `add_health_check_async_with_timeout`（默认 `DEFAULT_HEALTH_CHECK_TIMEOUT` = 3 秒）。
  `HealthCheckRegistry::snapshot_async` / `overall_status_async` / `all_healthy_async` /
  `build_health_response_async` 用 `futures_util::future::join_all` 并发执行所有检查、保持注册顺序，
  并对每项异步检查独立计时。超时记为 `HealthStatus::fail("timed out after ...")`，`/health` 与
  `/health/ready` 返回 503；`fail` 优先于 `warn`，`warn` 仍为 200。

#### 兼容性

- 既有同步 API（`register`、`snapshot`、`overall_status`、`all_healthy`、`build_health_response`、
  `add_health_check`）源码与语义兼容。同步视图只执行同步检查，遇到异步检查以
  `warn("async health check '<name>' requires async evaluation")` 计入——不会 `fail`——既有调用点无需
  改动，注册异步探针也不会破坏 readiness。
- `register` 签名不变，`register_with_options` 为新增；没有删除或改名，工作区版本升级到 1.4.0。

## [1.3.0] — 2026-11-13

Additive release for the 1.2 line: every existing 1.2 API keeps working
(`IDynamicAuthorizer::authorize`/`authorize_with_rules`, `DeclaredRules`, static
`#[authorize]` enforcement, `PagedRequest::new/clamped`, the `Jwt.Secret` flow).
Two public structs gained a field (`RouteEntry::authorization_dynamic`,
`StubEndpoint::authorization_dynamic`), so hand-written struct literals must add the
field; the official constructors (`RouteEntry::new`/`with_type_id`) are unchanged.

### Added

- **WEBX-18 — `#[authorize(dynamic)]`: declarative rules can be delegated to
  `IDynamicAuthorizer`.** Combines with `role`/`permission` in one attribute
  (`#[authorize(dynamic, permission = "order.delete")]`); when an endpoint has several
  `#[authorize]` attributes, one `dynamic` makes the whole endpoint dynamic. In dynamic
  mode the static gate only requires an authenticated principal (no claims → 401) and the
  declared rules are passed to the registered authorizer, which decides (denial → 403).
  With **no** authorizer registered the endpoint is authenticated-only pass-through —
  documented behaviour, not a safe default. Static endpoints behave exactly as in 1.2.
  New `RouteEntry.authorization_dynamic: bool` and the `const`
  `RouteEntry::with_dynamic_authorization()` builder (usable from `inventory::submit!`).
- **WEBX-19 — `IDynamicAuthorizer::authorize_with_declared_rules(&self, claims,
  route_pattern, method, declared: &'static [AuthorizeRule])`.** Provided default method;
  the host now calls it with the endpoint's own `&'static [AuthorizeRule]` slice
  (zero allocation, no `DeclaredRules` wrapper on the hot path). The default chain is new
  method → `authorize_with_rules` → `authorize`, so 1.1/1.2 implementations compile and
  work unchanged. `AuthorizerSet::authorize_with_declared_rules` mirrors it host-side.
- **XR-4 — `PagedRequest::from_parts(page: Option<u32>, page_size: Option<u32>)`.**
  `None` → 1/20, then clamped to 1..=100; equivalent to `new(..).clamped()`.
- **XR-2 — `Jwt.SecretFile` configuration source.** `JwtSection.secret_file:
  Option<String>` (JSON `SecretFile`, env `APP__Jwt__SecretFile`), resolved at host build
  time by `webx::apply_jwt_secret_file`; precedence `APP__Jwt__Secret`/`JWT_SECRET` >
  `Jwt.Secret` (JSON or `useOptions`) > secret file contents. Missing/unreadable/empty/
  shorter-than-`MIN_JWT_SECRET_LEN` (32 bytes) or (Unix) non-`0600` files fail startup with
  a typed error instead of panicking; the resolved value feeds the default issuer, the
  `JwtAuth` middleware and the Production weak-secret guard. Windows ACLs are not checked;
  DPAPI remains a future item.
- **XR-3 — timing-safe comparison and single-use secrets.** `webx::security::
  constant_time_eq` (no early return; length leakage documented),
  `OneTimeSecret`/`OneTimeSecretState` (TTL + atomic compare-and-consume, `issue`/
  `consume`/`matches`/`state`/`revoke`, redacted `Debug`), exported from `webx::security`
  and the `webx` facade.
- Docs: authorize macro chapter (dynamic delegation, "database is the source of truth"
  pattern, static-vs-dynamic table), resource authorization, route macros, new pagination
  page, security best practices, appsettings/JWT page.

### Changed

- The host now calls `IDynamicAuthorizer::authorize_with_declared_rules` instead of
  building a `DeclaredRules` wrapper per request; the `DeclaredRules` type and
  `authorize_with_rules` stay as legacy aliases (removal scheduled for 2.0, no
  `#[deprecated]` attribute so 1.2 code keeps compiling without warnings).
- `StubEndpoint`/`RouteEntry` gained `authorization_dynamic`; the framework's own fixtures
  and the host constructor were updated. No other signature changed.

## [1.2.0] —2026-11-13

Additive release: every existing 1.1 API keeps working (`IDynamicAuthorizer::authorize`,
`add_hosted_service<T: Default>`, `RequestContext::run`, the `x-trace-id` response header).
New extension points are default methods or new types.

### Added

- **Declared authorization rules reach dynamic authorizers (WEBX-1).**
  `webx::DeclaredRules { roles, permissions }` plus
  `IDynamicAuthorizer::authorize_with_rules(claims, route_pattern, method, &DeclaredRules)`
  (default forwards to `authorize`, so 1.1 implementations are source-compatible). The
  endpoint call site now passes the role/permission literals declared by `#[authorize]`, so a
  DB-backed authorizer no longer has to re-derive the requirement from route + method.
- **Token issuance and session revocation ports (WEBX-2).** `webx::security` exposes
  `ITokenIssuer`, `TokenClaims`, `IssuedToken`, `ISessionStore` and `InMemorySessionStore`;
  `IClaims::jti()` reads the token id. `rust-webx-host` adds a default `JwtTokenIssuer`
  (registered by `add_authentication()` when `Jwt.Secret` is configured, overridable via DI),
  `jwt_middleware_with_revocation` and `HostBuilder::use_session_store()`. A revoked `jti`
  is rejected at the middleware layer with 401 `problem+json` + `WWW-Authenticate`.
- **Password hashing port with an Argon2id default (WEBX-3).** `IPasswordHasher`
  (`hash`/`verify`/`needs_rehash`, PHC strings, `spawn_blocking` inside) lives in
  `webx_core::security`; `rust-webx-host` ships `Argon2PasswordHasher` + `Argon2Params`
  behind the default `password-argon2` feature. Unknown algorithm/PHC input returns a typed
  error instead of panicking.
- **Trace id is visible to handlers (WEBX-4).** `IHttpContext::trace_id()/set_trace_id()`
  (default no-op), `RequestContext::run_with_trace()`/`RequestContext::trace_id()`, and the
  endpoint macro seeds the request scope, so a handler reads the same id that is written to
  the `x-trace-id` response header. The header behaviour is unchanged; an inbound
  `x-request-id` still wins.
- **Background services (WEBX-5).** `BackgroundService::execute(ct)`,
  `PeriodicBackgroundService` (interval + interruptible tick + stop/join),
  `BackgroundServiceHost`, `CancellationToken` and `delay(&ct, dur)`. Registration:
  `add_hosted_service_with` (factory, no `Default` bound), `_ordered`, `_if`,
  `add_background_service(_with/_with_if)`; `IHostedService::order()` provides stable ordering.
  A startup failure is returned as a typed error (with rollback of already-started services)
  instead of panicking.

### Changed

- **Host shutdown order (WEBX-5).** Signal →hosted services stop in reverse registration
  order (workers observe cancellation first) →connection drain →`provider.dispose()`.
  Previously hosted services stopped after the drain; code that relied on workers running
  during draining should read the shutdown token instead.
- `rust-dix` requirement raised to `1.0` (trait-object `instance<T: ?Sized>`, fallible typed
  factory resolution, macro crate-name resolution).

### Docs

- Handler-visible trace ids and the `#[fromheader("x-trace-id")]` caveat
  (`user-identity.md`), background/hosted-service registration and shutdown semantics
  (`hosted-services.md`, `graceful-shutdown.md`).

## [1.1.1] — 2026-11-12

### Fixed

- **Response header names are case-insensitive again.** `IHttpResponse::set_header` /
  `remove_header` / `header` normalize the name, so `.header("Content-Type", ...)` overrides the
  framework's `content-type` instead of emitting a duplicate header (the backing map used to be
  case-sensitive).
- **OpenAPI response detection matches the runtime.** Qualified spellings such as
  `IRequest<webx::Response>` and `std::string::String` now produce the same opaque/string schemas as
  their bare spellings, instead of a placeholder `$ref` to the wrapper type.
- **`publish.sh` no longer hides gate failures.** It runs under bash with `set -euo pipefail`, so a
  failing `cargo clippy` / `check` / `test` in the release preflight aborts instead of being masked
  by the `tail` at the end of the pipeline.
- **`publish.yml` fails on a version/tag mismatch** instead of skipping crates with a warning; a
  mistyped tag used to leave the workflow green with nothing published.

### Added

- **`rust-version = "1.95"`** in `[workspace.package]`, inherited by every published crate (the
  MSRV claim in the docs and README is now machine-readable); CI gained an MSRV job that builds the
  workspace with Rust 1.95.
- **`scripts/check-docs.sh`** and a CI gate: relative Markdown links, `INDEX.json` ↔ handbook files,
  removed API names in the live handbook and Cargo/CHANGELOG/INDEX version agreement are checked on
  every push.

### Docs

- README (EN/zh): the file-response feature list uses the 1.1 `FileResult` / `ContentResult` API;
  the error-mapping table covers all 17 `Error` variants and no longer claims 401/403 come from
  `Error::Http`; the rust-ef handbook link points at crates.io / docs.rs; a 1.0.x → 1.1 upgrade
  pointer was added.
- New [upgrade to 1.0.2](docs/rust-webx/16-migration/upgrade-to-1.0.2.md) page (`ActionResult`
  rename, `PagedResult`), so every breaking release has a migration page again.
- Handbook: raw-response type spelling, framework-managed file headers, header-name
  case-insensitivity, OpenAPI placeholder/opaque behaviour and a custom-`IActionResult` redirect
  example are documented.
- `docs/design/api.md`: the unit `()` → 204 rule is part of the frozen contract, the "CI 门禁"
  heading is corrected to "compiler + review", and the stale "3.x" reference is gone.

## [1.1.0] — 2026-11-12

> This release contains breaking changes (`ResponseData` removal). The project ships the
> work on the 1.x line as **1.1.0** instead of opening a 2.0 major; the former 2.0
> development branch is now `main`. Migration steps:
> `docs/rust-webx/16-migration/upgrade-to-1.1.md`.

### Breaking

- **`ResponseData` is replaced by the ASP.NET Core `IActionResult` family.** Endpoints that
  control the HTTP response now declare `IRequest<Response>` (where `Response` is
  `Box<dyn IActionResult>`) and return a result:
  - `ResponseData::file(p)` → `FileResult::path(p)`
  - `ResponseData::file_stream(r, ct)` → `FileResult::stream(r).content_type(ct)`
  - `ResponseData::file_seekable_stream(r, len, ct)` →
    `FileResult::seekable_stream(r, len).content_type(ct)`
  - `ResponseData::with_file(body)` → `FileResult::with_body(body)`
  - `ResponseData::bytes(v)` / `::text(s)` / `::html(s)` → `ContentResult::bytes(v, ct)` /
    `::text(s)` / `::html(s)`
  - `ResponseData::no_content()` → `NoContentResult`
  - `ResponseData::json(v)` → `JsonResult::new(v)` (ordinary DTO responses do this
    automatically)
  - `status` / `header` / `download_name` / `inline_name` / `enable_range_processing` /
    `entity_tag` / `last_modified` keep their names on the result types
  e.g. `Ok(Box::new(FileResult::path(path).download_name("report.csv")))`.

### Added

- `IActionResult` trait, `Response` (`Box<dyn IActionResult>`), and the result types
  `JsonResult<T>`, `ContentResult`, `FileResult`, `NoContentResult`, `StatusCodeResult` —
  the ASP.NET Core result vocabulary, so an action reports the same shapes as
  `File(...)`, `Content(...)`, `StatusCode(...)`, `NoContent()`.

### Removed

- The unused `webx::Json<T>` alias (use `JsonResult<T>`); no deprecated shims remain.

### Changed

- OpenAPI now emits placeholder `components.schemas` entries for every referenced response type
  (`$ref` no longer dangles) and marks `IRequest<Response>` endpoints opaque.
- Internal plans/reviews moved out of the shipped docs tree; `publish.sh` and the publish workflow
  share the crates.io index helpers (`scripts/crates-index.sh`); the phantom 1.0.1 changelog entry
  is merged into 1.0.2.

## [1.0.2] — 2026-11-12

### Breaking

- **`IRequestHandler::handle` returns `ActionResult<R>`** — the framework-level name for
  ASP.NET Core's `ActionResult<T>`: `Ok(value)` is the response payload, `Err(Error)` is
  mapped to the HTTP status and `problem+json` by the framework. It is the same type as
  `Result<R, Error>` (`T` defaults to `()`), so `?` and `Err(...)` keep working; `Result<T>`
  remains the general fallible alias for infrastructure code. `IPipelineBehavior::handle`
  and `Next::run` follow the same name.
- **`PagedResponse<T>` is removed in favour of `PagedResult<T>`** — the list envelope now
  pairs with `OperationResult`.

### Added

- **`OperationResult` / `OperationResult<T>`** — a framework-provided business result DTO:
  `success`, business `status`, `message` and an optional `result` (`OperationResult` has no
  `result` member). It saves every module from defining its own `{ success, status, message }`
  envelope. It is an ordinary DTO, not a transport shape: the HTTP status stays the framework's
  (200 with the serialized body, `problem+json` for `Err(webx::Error)`), and `status` is
  whatever the business assigns. Constructors: `ok`, `ok_with_status`, `failed`, `success`,
  `success_with_status`.
- **`OperationResult` can be adjusted while the operation runs**: `with_status`,
  `with_message` and `with_result` (unit envelope → payload) sit next to the public fields,
  so an outcome that different branches decide is set in place instead of rebuilt.

## [1.0.0] — 2026-11-11

### Breaking

- **rust-dix 0.10 alignment.** The DI container is now rust-dix 0.10 (rust-dix-macros 0.10,
  rust-ef 2.1 for the examples); MSRV is 1.95. User-visible semantics:
  - strict scope validation is the **default** — resolving a Scoped service from the root
    provider returns `Err(ScopedFromRootProvider)`; `provider.scope()` is the supported path,
    and `get_owned` / `try_get_owned` remain callable from the root (caller-owned, not cached);
  - single-service resolution is MEDI **last-registration-wins**; framework defaults are
    registered with `try_add*` so an earlier user registration still wins;
  - `get_all` returns **non-keyed** registrations only; use `get_all_keyed::<T>()` for keyed;
  - `get_owned` on a Singleton returns `Err(OwnedNotSupported)` instead of panicking;
  - `RdiError` is `#[non_exhaustive]` with new `ScopedFromRootProvider` / `ProviderDisposed` /
    `ScopeDisposed` variants.
- **Ambient DI removed.** `dispatch_provider()`, `dispatch_handler_cache()`,
  `DispatchRuntime::run()` and `DispatchRuntime::run_in_scope()` are gone; there is no
  task-local provider. `DispatchRuntime` is a plain per-host handle (`provider()`,
  `handler_cache()`, `scope()`). Handlers use `#[handler(inject)]` constructor injection;
  background work injects rust-dix's built-in `Arc<dyn ScopeFactory>` and opens scopes
  explicitly; `IHttpContext::dispatch_runtime()` gives endpoint adapters the host handles.
- **`Mediator::new` takes explicit handles**: `Mediator::new(provider, Arc<HandlerCache>)`.
  `add_mediator()` still registers a DI-injectable mediator (the host registers the handler
  registry in DI).
- **Typed pipeline only.** `IErasedPipelineBehavior`, `BoxedNextFn`, `BoxedPipelineFuture`,
  `add_pipeline` / `add_pipeline_with` and `build_pipeline_chain` are removed; dispatch runs a
  single `IPipelineBehavior<Req, Rsp>` chain (short-circuit values are compiler-checked).
- **Deprecated APIs removed**: `register_handlers!` (since 0.2) and
  `build_resource_policy_from_routes()` (since 0.6).
- **Host shutdown disposes the provider**: after hosted services stop and connections drain,
  `provider.dispose()` releases singletons in reverse creation order and marks the provider
  disposed (later resolution returns `ProviderDisposed`; the call is idempotent).

### Added

- **`HostBuilder::try_build_async()`** for async singletons (rust-ef `add_dbcontext_async`):
  it awaits rust-dix `build_async_with`, and `try_build()` reports an actionable error when
  async registrations are present. Host assembly is shared between the sync and async paths.
- **`HostBuilder::lenient_scope_validation()`**: migration-only opt-out that restores the
  pre-1.0 root-scope behavior (`ServiceProviderOptions::lenient()`).
- `DispatchRuntime::scope()` and `IHttpContext::dispatch_runtime()` as explicit DI handles.
- Migration guide `16-migration/upgrade-to-1.0.md` (Chinese + English summary).

### Changed

- Framework-provided defaults (`HealthCheckRegistry`, `IEndpointCatalog`, `IStringLocalizer`)
  use `try_add*`; rust-dix 0.10 resolves the last registration, so user overrides keep winning.
- docbit/dmbit examples migrated: `DbInitService` injects `Arc<dyn ScopeFactory>`, the SEO
  middleware opens its own scope, and the old ambient calls are gone.
- README / handbook version references moved to 1.0.

### Fixed

- Hosted services no longer depend on an implicit task-local scope: scoped dependencies are
  bound to the scope the service opens and are released with it.
- Endpoint dispatch no longer relies on a global provider lookup; a missing runtime is reported
  as a clear `Error::Internal` instead of a panic in a macro-generated dispatch.
## [0.10.0] — 2026-11-11

### Breaking

- **`IPipelineBehavior` is typed.** The pipeline trait is now
  `IPipelineBehavior<Req, Rsp>` with
  `async fn handle(&self, req: Req, next: Next<Req, Rsp>) -> Result<Rsp>`: the request and
  response are real types, and a behavior that short-circuits returns the handler's response
  type at compile time instead of being downcast from `Box<dyn Any + Send>` at runtime. The old
  type-erased trait is renamed `IErasedPipelineBehavior` and deprecated; it keeps running for
  one release, with its chain wrapped outermost around the typed chain (1.0 removes it).
- **Typed pipeline registration.** `IServiceCollectionExt::add_typed_pipeline::<B, Req, Rsp>()`
  (Default-constructed) and `add_typed_pipeline_with::<F, Req, Rsp>(factory)` (constructor
  injection) register typed behaviors. `add_pipeline` / `add_pipeline_with` still compile but
  now register `IErasedPipelineBehavior` and are deprecated.
- **`HostBuilder::use_resource_authorization()` is removed.** It had been a deprecated no-op
  since 0.6; authorization is enforced from endpoint metadata and `#[authorize]`.
- **The process-global provider shim is removed.** `set_global_provider`,
  `try_global_provider` and the `global_provider` fallback are gone. `dispatch_provider()`
  resolves only from the active `DispatchRuntime` and panics with the fix in the message when
  none is active; `Host::provider()` is the single source of truth for the host instance.

### Added

- **`Arc<dyn IMediator>` dispatch via `send_boxed`.** The mediator trait is now
  dyn-compatible for type-erased callers: `send_boxed(Box<dyn Any + Send>) -> Result<Box<dyn Any + Send>>`
  looks the request up by `TypeId` (one boxed dispatch per `#[handler]`), runs the same typed
  pipeline + handler path, and returns a boxed response to downcast. The generic `send`/`publish`
  gain `where Self: Sized`; `publish` stays generic (event handlers are resolved by concrete
  `IEventHandler<T>`), so boxed publish is not part of 0.10.
- **`HostBuilder::try_build() -> Result<Host, Error>`.** Production guards (weak or placeholder
  JWT secret, CORS `*`) now return an error instead of panicking, so a deployment can exit with
  a readable message. `build()` stays fail-fast: it delegates to `try_build()` and panics on
  the same conditions as before.




## [0.9.0] — 2026-11-10

### Breaking

- **`APP_ENV` unset now means Production.** The safe default protects deployments
  that forget the variable: weak/placeholder JWT secrets and CORS `*` are rejected
  at build time instead of warned about. Local development must set
  `APP_ENV=Development` explicitly (the warning says so).
- **Anonymous identity must be declared explicitly.** `Option<IdentityUser>` now
  requires `#[fromclaim(optional)]`; a plain `#[fromclaim]` on an optional identity
  is a compile error that names the fix. A request may carry only one identity
  field (`user: IdentityUser` plus `viewer: Option<IdentityUser>` is rejected).
  Rationale: an "optional login" endpoint used to be completely public by
  accident, so forgetting `require_user()` silently exposed it.
- **`RequestContext::run` takes an identity.** The signature is now
  `run(Option<Arc<IdentityUser>>, future)`; `operator_id()` keeps working and is
  derived from the identity. The HTTP adapter builds the identity before binding,
  so the binder, handler and audit helpers all observe the same `Arc`.

### Added

- **Identity accessors**: `IdentityUser::{tenant, require_tenant}` (empty
  `tenant_id` means “no tenant claim”, not a tenant called `""`) and
  `RequestUser::{permissions, has_permission, require_role, require_tenant}`, so
  generic helpers can distinguish 401 (anonymous) from 403 (authenticated but
  missing the role/tenant).
- **`RequestContext::identity()`** — the request-scoped `Arc<IdentityUser>`, for
  audit columns and persistence callbacks outside the handler.

- **`IHttpRequest::remote_addr()`** — a defaulted accessor for the client peer
  address (`None` by default so test contexts and alternative hosts keep
  compiling). The host records it at accept time; rate limiting and audit code
  use it instead of guessing a constant.
- **`Error::public_detail()`** — the client-facing message of a 4xx variant,
  without the operator-facing `Display` prefix (`"Unauthorized: …"`); 5xx
  variants return `None`, which is what makes detail redaction enforceable.
- **`Retry-After` on 429** — derived from the bucket refill rate.
- **`IHttpContext::take_early_error()`** — a defaulted hook for rejections the
  host detects from the request line (currently an oversized declared
  `Content-Length`). The context records them instead of writing a response, so
  the pipeline can still see the request.
- **`SpaMiddleware::try_index_fallback()`** — the SPA shell fallback, called by
  the host after a router miss.
- **Hosted-service start/stop timeouts** — `start()` and `stop()` are bounded
  (30s each) so one stuck service cannot block startup or shutdown forever.

### Changed

- **Invalid or expired tokens no longer fail the request.** A bad `Authorization`
  header is treated as "unauthenticated": the request continues anonymously and
  protected endpoints answer 401 through `#[authorize]` or a required
  `#[fromclaim] user: IdentityUser`. Public endpoints (login, health, config) keep
  working when a client sends a stale token — previously such a request was
  rejected everywhere, which could lock users out of the login endpoint. The 401
  detail is the stable `authentication required`; decoder reasons go to
  `tracing::debug!`.
- **`405` carries `Allow`, and plain `OPTIONS` reports methods.** Only a real CORS
  preflight (`Origin` + `Access-Control-Request-Method`) is short-circuited by the
  CORS middleware; a plain `OPTIONS` answers 204 + `Allow` for a known path and 404
  for an unknown one, instead of 204 for every path.
- **OpenAPI parameter metadata is keyed by `TypeId`.** Qualified request paths
  (`impl IRequest<T> for contracts::ListOrdersRequest`) and same-named request types
  in different modules no longer lose or mix up their parameter documentation;
  string keys remain only as a fallback for hand-written `RouteEntry::new` entries.
- **User middleware defaults to the after-authentication slot.** `use_middleware`
  / `use_middleware_with` (and DI-registered `IMiddleware`) now run after JWT, so a
  middleware that reads `ctx.claims()` sees the principal — the framework's own
  `RequestTracing` previously logged `anon` for every production request. Explicit
  positions use `use_middleware_at` / `use_middleware_with_at` with the new
  `MiddlewareSlot::{BeforeCors, AfterAuth, AfterSpa}`. Pipeline order is now
  SecurityHeaders → RequestId → user(BeforeCors) → CORS → Metrics → RateLimit →
  auth → user(AfterAuth) → SPA → user(AfterSpa) → Router, so 429/413 responses
  carry CORS headers and rate-limited requests are counted.
- **Hosted services run inside a DI scope.** Each `IHostedService::start`/`stop`
  gets a fresh scope: `dispatch_provider().get_owned::<DbContext>()` resolves
  scoped dependencies against that scope and they are disposed when the call
  returns, instead of being cached on the root provider for the process lifetime.
- **`add_pipeline_with` gives pipeline behaviors real constructor injection.**
  `add_pipeline` always required `Default` and discarded the DI resolver, so the
  documented "behaviors resolve dependencies via constructor injection" was a
  no-op. The new
  `add_pipeline_with(|resolver| Arc::new(MyBehavior::new(...)) as Arc<dyn IPipelineBehavior>)`
  receives the resolver at construction time (capture an `Arc` in the closure for
  the simple case; during DI build-time validation the portable resolver call is
  `resolver.get_any(type_name::<T>())` + downcast). A behavior that short-circuits
  with a different response type now produces an error naming the expected type
  and the fix instead of a generic "response type mismatch".
- **`after` hooks run on error paths.** The middleware pipeline now unwinds the
  middlewares that entered when a later `invoke` or the final handler returns an
  error, then returns the first error. Metrics, tracing, compression and cleanup
  finally observe failed requests (previously a `?` skipped the reverse pass).
- **One 401 write path.** Declarative authorization no longer writes its own
  401; it returns `Error::Unauthorized` and the host writes every 401 through
  `problem_response::write_unauthorized`, which sets `application/problem+json`
  and `WWW-Authenticate: Bearer realm="rust-webx"` (RFC 7235). Missing
  credentials and a rejected token keep distinct `detail` text.
- **Rate limiting keys on the real peer address.** Previously every request was
  bucketed under a constant `127.0.0.1`, silently turning per-client limits into
  one global bucket. When the host cannot report a peer address the limiter now
  passes the request through instead of bucketing everyone together.
- **`run()`/`run_at()` bind listeners before anything starts.** A port conflict
  now returns an error (non-zero exit) instead of being logged and followed by a
  successful shutdown; a failed startup also stops the hosted services that
  already started, in reverse order.
- **Shutdown signals are stored, not broadcast.** `ServerHandle::shutdown()` and
  `IHost::stop()` use a `watch` channel, so a signal sent before the accept loops
  start is not lost (the documented `spawn(run); handle.shutdown()` pattern used
  to hang). Connections are asked to close gracefully (`Connection: close`)
  during the drain window instead of being aborted after it.
- **Hosted services stop in reverse start order**, with rollback on a failed
  start. Docs previously disagreed with the implementation on this.
- **SPA index fallback runs after routing.** It is applied only when the router
  has no match and the client explicitly asked for HTML (`Accept: text/html`);
  unmatched API paths and data clients get a real 404. A handler response can no
  longer inherit the SPA file cache policy (`Cache-Control: public`), which used
  to make private JSON publicly cacheable.
- **Oversized declared bodies go through the middleware pipeline.** The eager
  413 response previously bypassed CORS, security headers, access logs, metrics
  and rate limiting; it is now a normal error raised inside the pipeline.
- **`Error::Serialization` only describes request parsing.** Response
  serialization failures are `Error::ResponseSerialization` (500) and body
  state-machine errors are `Error::State` (500), so a server-side bug can no
  longer be reported to the client as a 400/415.
- **`AppMode::from_env()` warns when `APP_ENV` is unset.** The default stays
  `Development` in this release, but a once-per-process warning now tells the
  operator to set `APP_ENV=Production` explicitly.
- **Embedded-asset compression defaults to brotli quality 9.** Quality 11 made a
  clean build of a ~27 MB `wwwroot` take over an hour (the build script is
  single-threaded), which blocked development and CI. Quality 9 is a few percent
  larger and an order of magnitude faster; `WEBX_BROTLI_QUALITY=11` restores the
  old behaviour for size-critical release builds, and changing it now retriggers
  the build script.

### Fixed

- **`rust-ef` upgraded to 2.0.1 (`libsqlite3-sys` 0.39).** The example apps pinned
  rust-ef 1.8.3, which resolved `libsqlite3-sys` 0.28 — affected by
  RUSTSEC-2026-0459 (CVE-2026-119375). Moving to the documented 2.0.1 baseline
  updates rusqlite 0.41 / libsqlite3-sys 0.39 and makes `cargo audit` pass.
- **5xx detail no longer leaks internals.** `Error::Internal` / `Di` / response
  serialization text and file-system paths were served verbatim; they are logged
  (`tracing::error!`) and replaced with a stable detail. File-download and
  ranged-response failures are redacted the same way.
- **`claims` is an ordinary field name again.** The derive special-cased a field
  named `claims` and silently skipped it — even when the user wrote
  `#[frombody] claims: Vec<Claim>` — which also meant a request whose only field
  was `claims` never read the body. That leftover from the removed `#[claims]`
  injection is gone.
- **`r#` identifiers use their unraw wire name.** `#[frombody] r#type: String`
  now binds the JSON key `type` (previously `r#type`), and query/header names
  match serde's behaviour.
- **`#[fromclaim]` reads the raw claim map first.** A token that literally
  carries `user_id` or `tenant` is no longer silently replaced by `sub` /
  `tenant_id`; the dedicated accessors are only a fallback.
- **`Option<composite>` fields on body-less methods are a 400, not a silent
  `None`.** The complex-type guard only covered required fields, so
  `Option<Filter>` on a GET never bound and never failed.
- **`?ids=` (present but empty) binds as `Some(vec![])`.** `Option<Vec<T>>` could
  not express an explicitly empty list: the empty string became one empty element
  and failed to parse.
- **JWT failures no longer echo the decoder's error text** (`InvalidToken`,
  signature/base64 details) into the 401 body.

## [0.8.0] — 2026-11-10

### Breaking

- **`CurrentUser` is now `IdentityUser`** — same fields, same binding
  (`#[fromclaim] user: IdentityUser`, `Option<IdentityUser>`), clearer name: it is a
  read-only projection of `IClaims`, not a persistence entity. Replace `CurrentUser`
  with `IdentityUser` in request structs; the `RequestUser` accessors (`user_id()`,
  `is_in_role()`, `require_user()`) are unchanged.
- **`#[claims]` and `IClaimsCarrier` removed** — identity now comes from
  `#[fromclaim] user: IdentityUser` (or `#[fromclaim("key")]` for a single claim).
  The dispatcher no longer injects a `claims` field; middleware/infrastructure code
  that needs the raw map reads it through `IClaimsExt`.

## [0.7.2] — 2026-11-10

### Added

- **`CurrentUser` in requests** — `#[fromclaim] user: CurrentUser` binds the authenticated
  identity (`id`, `name`, `email`, `tenant_id`, `roles`, `permissions`) as an ordinary
  field; `Option<CurrentUser>` keeps an endpoint usable by anonymous callers. The derive
  also implements `RequestUser` (`user_id()`, `user_name()`, `tenant_id()`,
  `is_in_role("admin")`, `require_user()`), so handlers no longer reach into
  `IHttpContext` or parse claim strings by hand.

## [0.7.1] — 2026-11-10

### Fixed

- **Consumer builds no longer need a direct `inventory` dependency** — `#[handler]`
  and the route macros now submit their registrations through `::webx::inventory`,
  the re-export `rust-webx` already publishes. A project created from the
  quickstart (only `rust-webx`, `serde`, `tokio`) compiles without adding
  `inventory` itself.

## [0.7.0] — 2026-11-10

### Breaking

- **Every request type must derive `WebxRequestMeta`** — the endpoint macros
  bind through `RequestModel`, which the derive implements. A missing derive is
  a compile error with an `add #[derive(WebxRequestMeta)]` hint.
- **Binding failures are 400s** — a value that cannot be parsed, a required
  field that is missing, or an unknown key under `deny_unknown_fields` returns
  `application/problem+json` with `errors: { "field": ["message"] }` keyed by
  **field name**.
- **A form endpoint rejects a non-multipart body with 415.**
- **Field markers are written without underscores**: `#[frompath]`,
  `#[fromquery]`, `#[frombody]`, `#[fromform]`, `#[fromfile]`,
  `#[fromheader]`, `#[fromclaim]`, and `#[request(queryall)]`. The previous
  snake_case spellings are gone.

### Added

- **`#[fromfile("avatar")]`** — binds a file part, and every marker except
  `#[frombody]` now accepts an optional source key (`#[fromquery("q")]`,
  `#[frompath("id")]`, `#[fromform("title")]`).
- **`#[fromheader("X-Tenant")]`** — binds a header, case-insensitively; the bare
  form derives the name from the field (`x_request_id` → `x-request-id`).
- **`#[fromclaim("tenant_id")]`** — binds a claim. `sub`, `roles` and
  `permissions` use their dedicated slots; a required claim on an
  unauthenticated request is 401.
- **`Vec<T>` query fields** — repeated keys (`?id=1&id=2`) and comma lists
  (`?id=1,2`) both bind; the host keeps every query value.
- **Type coercion for every non-body source** — each scalar is parsed with
  `FromStr`, so `page: u32` binds from `?page=2`.
- **One declaration for binding and OpenAPI** — `#[derive(WebxRequestMeta)]`
  emits both `RequestModel::bind` and `param_meta()`, so the spec and the runtime
  cannot disagree; `required` flags mirror the runtime exactly.
- **UTF-8 query decoding** — `%E4%B8%AD` decodes to `中`.

### Changed

- Unmarked fields follow the **request method**: `GET`/`HEAD`/`DELETE`/`OPTIONS`
  bind from the query string, then the route; `POST`/`PUT`/`PATCH` bind from the
  JSON body, then the form, then the query string. Header and claim values are
  never implicit.
- **Body fields bind one at a time** from the JSON object: presence is exact,
  so an absent key falls through to the next source while an explicitly `null`
  or mistyped value is a 400 for that field. `rename`, `rename_all`, `alias`,
  `default`, `flatten`, `deny_unknown_fields` and custom deserializers keep
  their serde meaning; defaults apply only after the fallback chain finds
  nothing.
- Composite fields (`ComponentModel`, `Option<ComponentModel>`,
  `Vec<ComponentModel>`, `#[serde(flatten)]`) bind from the JSON body; custom
  scalars (`Uuid`, enums) bind from query/path/header/claim only when marked
  explicitly.
- `Option<Vec<T>>` distinguishes "absent" (`None`) from "present, empty"
  (`Some([])`).
- `Error::Problem` carries a complete `ProblemDetails` through the pipeline so
  the `errors` extension reaches the client instead of a stringified `detail`.

### Removed

- `FromHttpContext`, `bind_form_request`, `try_deserialize_from_params`,
  `FormDeserializer` and `FormError` — the old single-pass binding helpers.
- `FormFile` no longer implements `Deserialize`. A file handle can only come
  from a parsed multipart body, so upload request structs derive
  `WebxRequestMeta` only (no `Deserialize`).

## [0.6.1] — 2026-11-04

### Fixed

- **CI on Rust 1.99** — `async-trait` updated to 0.1.92, which no longer emits the
  `#[must_use]` attribute that triggered `clippy::double_must_use`. No API change.
- **Security audit** — updated `rustls` 0.23.46, `rustls-webpki` 0.103.16,
  `webpki-roots` 1.0.10, `quinn-proto` 0.11.18, and `zip` 9.3.0. `cargo audit`
  now reports 0 vulnerabilities. The four allowed warnings (unmaintained
  `rustls-pemfile`, unsound `anyhow`/`tokio`, yanked `chacha20`) are dependency
  advisories, not vulnerabilities.
- **Docbit zip extraction on zip 9** — `ZipFile::name()` now returns a `Result`;
  the extractor maps the error to a validation error instead of unwrapping.
- **Coverage job** — replaced `cargo-tarpaulin` (segfaulted on the upload/download
  test binary) with `cargo-llvm-cov`, which produces `coverage/lcov.info`.

## [0.6.0] — 2026-11-04

### Added

- **Global `Culture` configuration** — `Culture.Default` in `appsettings.json`
  (or `APP__Culture__Default`) is the host-wide UI culture. Endpoint names,
  OpenAPI, and the endpoint catalog use it; request headers do not change
  administrative output, so permission/menu records cannot drift with the
  caller's browser language. Default: `"en"`.

  ```json
  { "Culture": { "Default": "zh-CN" } }
  ```

- **`#[display]` / `#[description]` take resource references, not translations** —
  `#[display("literal")]`, `#[display(key = "...")]`, or
  `#[display(key = "...", text = "literal")]`. Translations live in per-culture
  resource files; the active culture resolves them. When no `#[display]` is
  present, the first line of a `///` doc comment is used, and the request type
  name is the last fallback for the name.

  ```rust
  #[post("/api/blog")]
  #[display(key = "endpoint.blog.create.name", text = "博客模块-创建博客")]
  #[description(key = "endpoint.blog.create.description")]
  #[category("博客模块")]
  #[authorize(permission = "blog:create")]
  impl IRequest<BlogPostModel> for CreateBlogPostRequest {}
  ```

- **`#[category]` — optional grouping label** — accepted by the route macros and
  carried by `RouteEntry.category` / `EndpointDescriptor.category` /
  `EndpointInfo.category` (resolved like name/description). It is display-only
  metadata: it does not affect routing or authorization, and OpenAPI uses it as
  the operation tag when set. Localization/resource-key forms match
  `#[display]`.

- **`IStringLocalizer` / `StringLocalizer` / `LocalizationOptions`** — the .NET
  model in Rust: resources are JSON objects, either read from
  `locales/<culture>.json` at startup or embedded with `include_str!`; lookup
  falls back from the exact culture to parent cultures, the default culture, the
  literal fallback, then the key itself. `LocalizedString.resource_not_found`
  marks missing keys.

  ```rust
  // Conventional layout: locales/<culture>.json
  Host::builder().add_localization().build();

  // Single-file deployment: compile resources into the binary
  Host::builder().add_localization_with(
      LocalizationOptions::none()
          .resources("zh-CN", include_str!("../locales/zh-CN.json"))
          .resources("en", include_str!("../locales/en.json")),
  )
  ```

- **`IEndpointCatalog` — the runtime endpoint metadata service** — built once in
  `Host::build()`, registered in DI as `Arc<dyn IEndpointCatalog>`, and exposed
  as `Host::endpoints()` (the `EndpointDataSource` analogue). `all()` returns the
  language-neutral `&[EndpointDescriptor]` (also `Serialize`), `get(method, path)`
  looks one up, and `localized()` / `localized_for(culture)` produce
  `EndpointInfo` rows sorted by `path` then `method`.

  ```rust
  #[derive(Inject)]
  struct ListEndpointsHandler {
      #[inject]
      catalog: Arc<dyn IEndpointCatalog>,
  }

  // In handle:
  let all = self.catalog.all(); // &[EndpointDescriptor]
  ```

  See [`docs/rust-webx/05-request-pattern/route-macros.md`](docs/rust-webx/05-request-pattern/route-macros.md#在-handler-中返回所有路由).

### Changed

- **`#[display]` / `#[description]` / `#[category]` may be declared before or
  after the route macro.** The text macro carries its arguments on an internal
  attribute so the route macro reads them in either order; there is no ordering
  constraint.

- **`RouteEntry.summary` is now `RouteEntry.name`**, and both `name` and
  `description` are `TextRef` resource references instead of strings. OpenAPI
  still publishes the resolved name as `summary`, resolved through the host
  localizer.

- **`generate_openapi_spec(title, version)` is now
  `generate_openapi_spec(title, version, &dyn IStringLocalizer)`**. The document
  uses the localizer's default culture (the configured `Culture.Default`), not a
  per-request locale.

- **`RouteEntry.required_role` / `required_permission` are now
  `RouteEntry.authorization: &[AuthorizeRule]`**. Every `#[authorize]` is kept as
  one rule (no longer collapsed into two scalars), which is what makes multiple
  declarations representable:

  ```rust
  pub struct AuthorizeRule {
      pub roles: &'static [&'static str],        // any-of; empty = unconstrained
      pub permissions: &'static [&'static str],  // any-of; empty = unconstrained
  }
  ```

  All rules must be satisfied (AND); within a rule the two dimensions are ANDed,
  and `roles` / `permissions` accept arrays (`#[authorize(roles = ["admin",
  "auditor"])]`). The 403 problem response now reports `required_roles` /
  `required_permissions` arrays instead of `required_role` / `required_permission`
  strings.

### Deprecated

- **`HostBuilder::use_resource_authorization()`** — endpoints enforce their
  `#[authorize]` rules directly after routing, so there is no policy switch. The
  method is a no-op and will be removed in a future major release.

- **`build_resource_policy_from_routes()`** — a `ResourceAuthorization` policy
  stores one role/permission set per path and cannot faithfully represent several
  rules on one endpoint. Use an `IDynamicAuthorizer` for custom policies.

### Breaking

- Route metadata field renames and type changes above; `RouteEntry::new` now
  takes `name` / `description` (`TextRef`), `category`, and `authorization`
  instead of `summary` / `description` strings and required role/permission
  scalars. Code that read `RouteEntry.summary` / `required_role` must migrate to
  `RouteEntry.name` / `authorization`.
- **`generate_openapi_spec(title, version)` is now
  `generate_openapi_spec(title, version, &dyn IStringLocalizer)`** — pass
  `host.localizer()` or a custom localizer.
- **`StubEndpoint` fields** (host crate) — `auth_required_role`,
  `auth_required_permission`, and `resource_policy` are replaced by
  `authorization: &'static [AuthorizeRule]`. Custom endpoint construction must
  read the rule list instead of two scalars.

See [`docs/rust-webx/16-migration/upgrade-to-0.6.md`](docs/rust-webx/16-migration/upgrade-to-0.6.md)
for the step-by-step migration.


## [0.5.1] — 2026-10-07

### Added

- **`webx::assets` — read embedded files by path**: `webx::assets::read(path)`
  and `webx::assets::read_str(path)` return a file's original contents from the
  table linked by `#[webx::main(embed)]`, anywhere and without a `Host` —
  `std::fs::read` for the binary itself. Verbatim entries come back without a
  copy; brotli entries are decoded once and cached for the life of the process
  (the cache is shared with the SPA middleware). A leading `/` is accepted.
  Only the compiled-in copy is read — the disk overlay and `WEBX_EMBED` play no
  part. `None` when the file is not embedded, or nothing is.

  ```rust
  let config: Option<&'static str> = webx::assets::read_str("config/default.json");
  let logo: Option<&'static [u8]> = webx::assets::read("images/logo.png");
  ```

  The table-level form is `EmbeddedAssets::registered()` (`&'static`) with
  `read` / `read_str` on it, for code that needs the table itself.

  See [`docs/rust-webx/11-production/openapi-spa.md`](docs/rust-webx/11-production/openapi-spa.md#在代码里读取内嵌文件).

### Changed

- `Host::build` looks the table up through `EmbeddedAssets::registered()`, so
  the host and application code share one lookup (including the panic when
  more than one table is registered).


## [0.5.0] — 2026-09-20

> Upgrading from 0.4 or earlier: see
> [`docs/rust-webx/16-migration/upgrade-to-0.5.md`](docs/rust-webx/16-migration/upgrade-to-0.5.md),
> which walks through every breaking change below with verification steps.

### Breaking

- **Import path**: the crate **library names** are now `webx` / `webx_core` /
  `webx_host` / `webx_macros` / `webx_spa` / `webx_openapi`. Package names are
  unchanged (`rust-webx`, `rust-webx-core`, …), so `Cargo.toml` needs no edit —
  only `use` paths and macro paths do.

  ```bash
  # Only the underscore form is a code path; dashed package names are unaffected.
  grep -rl 'rust_webx' --include='*.rs' . | xargs sed -i 's/rust_webx_build/webx/g; s/rust_webx/webx/g'
  grep -rl 'RUST_WEBX' --include='*.rs' . | xargs sed -i 's/RUST_WEBX/WEBX/g'
  ```

  | Item | Before | After |
  |------|--------|-------|
  | Umbrella import | `use rust_webx::*;` | `use webx::*;` |
  | Sub-crate import | `rust_webx_core::…` | `webx_core::…` |
  | Entry macro | `#[rust_webx::main]` | `#[webx::main]` |
  | Embed macro | `rust_webx::spa::embed_assets!()` | `webx::spa::embed_assets!()` |
  | Build-time crate | `rust_webx_build::embed_assets("wwwroot")` | `webx::builder().web_root("wwwroot").build()` |
  | Generated file | `rust_webx_embedded_assets.rs` | `webx_embedded_assets.rs` |

- **Environment variables**: `RUST_WEBX_APP_BASE` → `WEBX_APP_BASE`;
  `RUST_WEBX_EMBED` → `WEBX_EMBED`.
- **SPA entry**: `HostBuilder::embed()` removed. `build.rs` writes the table with
  `webx::builder().web_root(...).build()` and the entry point links it with
  `#[webx::main(embed)]`; plain `#[webx::main]` never bakes assets. Integration
  tests that never run `fn main` register the same table with
  `#[webx::embed_assets]`.
- **Embedded assets are stored compressed** (see Added). Code that reads
  `EmbeddedAsset` fields directly must account for `raw_len` and `encoding`:
  `bytes` is the stored payload, which is only the file itself when `encoding`
  is `Identity`.

### Added

- **Compressed embedded assets**: `build.rs` brotli-encodes every compressible
  file (quality 11) and the SPA middleware hands the stored stream to clients
  that accept `Content-Encoding: br`, decoding once and caching it for clients
  that do not. Media that is already compressed (PNG, WOFF2, ZIP, …) and files
  under 256 bytes stay verbatim. On docbit's 27 MiB `wwwroot` this removes ~80%
  of the embedded footprint — and because static files were never compressed on
  the wire, they are now smaller in transit too.
- **Per-representation `ETag`s**: an embedded file served as brotli carries
  `<etag>-br`, and only responses that can vary by encoding send
  `Vary: Accept-Encoding`.
- **Migration guide**: [`docs/rust-webx/16-migration/upgrade-to-0.5.md`](docs/rust-webx/16-migration/upgrade-to-0.5.md)
  documents the import rename, the environment variables, the SPA embed model and
  the new payload encoding, with a verification checklist and a troubleshooting
  table.

- **Docbit — sixth work (`rust-agent-flow`)**: the `rust-flow` repository is now
  part of the portfolio. Its documentation slug is `rust-agent-flow`, so the
  wiring names it explicitly everywhere it appears: `DocService::WORK_SLUGS`,
  `sibling_doc_relative` (`rust-flow/docs/rust-agent-flow`), the seed exhibition
  row, and both `scripts/copy-ecosystem-docs` variants — the publish bundle now
  carries six doc trees instead of five.
- **Docbit — documentation bundle upload**: `POST /api/works/{slug}/docs`
  (admin-only) installs an uploaded zip as `<app_base>/docs/{slug}` and re-syncs
  the catalog in the same request. Extraction is sandboxed against path
  traversal, symlink entries and zip bombs; the install is an atomic swap with
  rollback, so the served documentation is never left half-replaced. Works with
  no seed template are created from their `INDEX.json`.
- **Docbit — blog media**: `POST /api/media` (authenticated) stores images and
  attachments under `<app_base>/uploads/`, served back by `GET /api/media/{*path}`.
  Raster images are inline; everything else — notably SVG — is sent as a download
  so it cannot execute on this origin. Vditor's `upload`/`insert` toolbar entries
  are wired to it.
- **Docbit — admin UI**: a "上传文档" action on each 作品管理 row, with progress
  and a post-upload summary.

### Changed

- **SPA embed**: `Host::build` layers the compiled-in table under `.use_spa(...)`
  automatically once the table is linked; `.use_spa` is the runtime disk overlay.
- **Build script API**: `webx::builder().web_root(...).build()` (the
  `rust-webx-build` package exposes the `webx` library name).
- **`EmbeddedAsset`** gained `raw_len` and `encoding`
  (`ContentEncoding::Identity` / `Brotli`). `EmbeddedAssets::total_bytes()` is now
  the stored payload size and `total_raw_bytes()` the uncompressed size; the host
  startup log reports both.

### Fixed

- **Docbit admin password**: the built-in `admin@docbit.local` / `admin123`
  account was created on every boot with no environment gate, and the password
  was written to the log. Production now provisions the admin only from
  `DOCBIT_ADMIN_PASSWORD` and never falls back to a built-in password.
- **`rust-webx-build` is now published.** The documented
  `[build-dependencies] rust-webx-build = "…"` dependency resolves; the crate was
  missing from the release lists (`publish.sh` and `.github/workflows/publish.yml`).
- **`cargo doc --workspace` no longer collides.** `rust-webx-build` shares the
  library name `webx` with the umbrella crate, so it is marked `doc = false`
  (it is a build-script helper with no runtime API to browse).
- **Crate-level rustdoc**: the crate roots now use `//!` docs (previously `//`
  comments, which left every docs.rs landing page without a description).
- **`publish.sh --check`** reports a dependent crate whose internal dependency is
  not yet on crates.io as "packages OK; dry-run skipped" instead of a hard
  failure, and its pre-flight now mirrors the CI gates (`fmt`, `clippy -D warnings`,
  workspace tests).


## [0.4.0] — 2026-09-16 — 文件上传与下载基础设施

> **English** · **简体中文** · 迁移指南：[docs/rust-webx/16-migration/upgrade-to-3.0.md](docs/rust-webx/16-migration/upgrade-to-3.0.md)

### English

#### Added

- **Uploads**: `multipart/form-data` binding into typed request structs. A request
  field declared as `FormFile` is filled from the matching file part; text fields,
  `bool`/numeric/`Option`/`Vec` and unit enums bind as usual. No extra derive and no
  custom middleware — `#[derive(Deserialize)]` is all a request struct needs.
- **Uploads**: file parts stream into a `FormFileBuilder` that buffers in memory up to
  `Form.MemoryThreshold` and then spools to `Form.TempDir`. Peak memory is independent
  of upload size, and spooled files are deleted when the request struct is dropped.
- **Uploads**: `FormFile` API — `file_name()` (sanitised), `original_file_name()`,
  `content_type()`, `size()`, `extension()`, `path()`, `open()`, `read_bytes()`,
  `copy_to()`, `save_as()` (atomic write).
- **Downloads**: `ResponseData` as a response type gives a handler full control of
  status, headers and body: `json` / `text` / `html` / `bytes` / `file` / `no_content`,
  plus `status()` / `content_type()` / `header()` / `download_name()` / `inline_name()`.
- **Downloads**: the `File(...)` family, mirroring ASP.NET Core. A response can come
  from a path (`ResponseData::file`), an async reader of unknown length
  (`ResponseData::file_stream`, sent chunked), or a seekable reader of known length
  (`ResponseData::file_seekable_stream`, which keeps exact `Content-Length` and
  `Range`). `ResponseData::with_file(FileBody)` exposes the file-level knobs:
  `enable_range_processing`, `entity_tag`, `last_modified`.
- **Downloads**: `ResponseData::file` is streamed and automatically carries
  `Content-Length`, `ETag`, `Last-Modified`, `Accept-Ranges` and a RFC 6266/5987
  `Content-Disposition`, and honours `Range` (`206`/`416`), `If-Range`,
  `If-None-Match`/`If-Modified-Since` (`304`) and `HEAD`.
- **Resumable downloads**: `Range` handling is RFC 7233-correct end to end. Ranges are
  coalesced — overlapping, adjacent and duplicate members merge — so a range request
  can never amplify traffic beyond the file size; up to eight disjoint parts are
  answered as `multipart/byteranges` with an exactly computed `Content-Length`; more
  than that falls back to the full representation. Unsatisfiable members are dropped,
  an entirely unsatisfiable set is `416` with `Content-Range: bytes */<len>`, and a
  syntactically invalid member invalidates the whole header (§2.1).
- **Resumable downloads**: correct validator semantics. `If-Range` uses a **strong**
  comparison as RFC 7233 §3.2 requires (a weak tag never matches), while
  `If-None-Match` uses the weak comparison RFC 7232 §3.2 requires. `ETag`s carry full
  mtime precision, so an in-place edit that preserves the file size is still detected;
  `Last-Modified` is suppressed when the mtime is in the future (RFC 7232 §2.2.2); and
  `Accept-Ranges: none` is sent when ranges are unavailable, so download managers stop
  probing instead of guessing.
- **Performance**: one shared 64 KiB `STREAM_BUF_SIZE` for streamed responses and
  spooled uploads (the previous `ReaderStream` default of 4 KiB cost ~16x more
  reads and socket writes per gigabyte); file responses `open()` once and take
  length/mtime from the same handle instead of `metadata()` + `open()`; spooled
  uploads write through a 64 KiB `BufWriter` so `tokio::fs` no longer dispatches
  to the blocking pool once per transport chunk; memory-backed uploads are held in
  a reference-counted `Bytes` so `FormFile::open` no longer copies them;
  `FormFile::copy_to` uses the 64 KiB buffer instead of `tokio::io::copy`'s 8 KiB.
- **Routing**: `HEAD` now falls back to the `GET` route for the same path, so download
  endpoints answer `HEAD` with the same headers and no body.
- **Static files**: `SpaMiddleware` hands files to the host as file bodies, so
  `wwwroot` assets are streamed with the same validators and range support, get MIME
  types from a full extension table, and no longer buffer into memory. It also
  overrides the default `cache-control: no-store` with a caching policy.
- **Config**: new `Form` section (`MaxRequestSize`, `MaxFileSize`, `MaxFieldSize`,
  `MemoryThreshold`, `TempDir`). Multipart requests are measured against it; other
  requests keep using `App.MaxBodySize`. A declared `Content-Length` above the limit
  is refused with `413` before the upload is read.
- **Errors**: `Error::PayloadTooLarge` → `413` and `Error::UnsupportedMediaType` → `415`.
- **Docs**: new chapter [文件上传与下载](docs/rust-webx/05-request-pattern/file-upload-download.md).

#### Changed

- **(breaking)** `IHttpRequest::body_bytes` / `body_text` take `&mut self`: reading the
  body consumes it, and reading it twice now reports an error instead of returning an
  empty body. `read_json_body` takes `&mut dyn IHttpRequest`.
- **(breaking)** `IHttpRequest` gained `content_type()`, `is_multipart()` and
  `multipart()`; `IHttpResponse` gained `write_body(ResponseBody)`. Both have defaults,
  so existing implementations keep compiling.
- **(breaking)** `ResponseData.body` is now a `ResponseBody` (`Bytes` or `File`) and
  `ResponseData` gained a `headers` field; use the constructors instead of a struct
  literal.
- **(breaking)** `IRequest<R>` / `IRequestHandler<T, R>` / `IMediator::send` no longer
  require `R: Serialize`; the bound is enforced where the value is serialized. This is
  what allows `IRequest<ResponseData>`.
- The route dispatch function now receives `&mut dyn IHttpContext` instead of the
  pre-read body, route/query maps and claims, so endpoints can stream the body.
- Request bodies are no longer read eagerly; they stay a live stream until the endpoint
  asks for them.
- `IHttpResponse` no longer defaults to `Content-Type: application/json`; writers set it
  explicitly (all built-in endpoints already did).
- `Cache-Control` defaulting moved from `SecurityHeadersMiddleware` into response
  finalisation, so it can depend on what the response is: `no-store` for ordinary
  responses, `public, max-age=0, must-revalidate` for file responses (an `ETag` paired
  with `no-store` could never be used). Either way an explicit `cache-control` set by
  the application always wins.
- Streamed responses are boxed as `UnsyncBoxBody`, so a body only has to be `Send` —
  requiring `Sync` would rule out most streaming readers.
- `HttpStatus` gained `PARTIAL_CONTENT`, `NOT_MODIFIED`, `PAYLOAD_TOO_LARGE`,
  `UNSUPPORTED_MEDIA_TYPE` and `RANGE_NOT_SATISFIABLE`.

#### Fixed

- **security**: `rustls` bumped to `0.23.45` and `rustls-webpki` to `0.103.15` for
  RUSTSEC-2026-0285 (TLS 1.3 handshake messages could be accepted across encryption
  level boundaries).
- `crates/spa` MIME lookup replaced with the full extension table (`.csv`, `.md`,
  `.webp`, `.mp4`, … no longer fell through to `application/octet-stream`).
- Static assets are no longer forced to `cache-control: no-store`.
- `docs/rust-webx/13-extensibility/custom-endpoints.md` no longer claims a custom
  `IEndpoint` can be registered through `register()`; it documents the two supported
  escape hatches.

#### Reference apps

- **dmbit**: inventory export is a real `text/csv` download with a
  `Content-Disposition` filename; inventory import is a real `multipart/form-data`
  upload (`file` + `confirm_update`) instead of a JSON-encoded CSV string.

### 简体中文

#### 新增

- **上传**：`multipart/form-data` 绑定到类型化请求结构体。字段声明为 `FormFile`
  即可接收对应文件部分；文本、布尔、数值、`Option`、`Vec` 与单元枚举照常绑定。
  请求结构体只需 `#[derive(Deserialize)]`，无需额外派生或中间件。
- **上传**：文件部分以流式写入 `FormFileBuilder`，超过 `Form.MemoryThreshold`
  即落盘到 `Form.TempDir`。峰值内存与文件大小无关，临时文件随请求结构体释放自动删除。
- **上传**：`FormFile` 提供 `file_name()`（已净化）、`original_file_name()`、
  `content_type()`、`size()`、`extension()`、`path()`、`open()`、`read_bytes()`、
  `copy_to()`、`save_as()`（原子写入）。
- **下载**：响应类型写 `ResponseData` 即可完全掌控状态码、响应头与响应体：
  `json` / `text` / `html` / `bytes` / `file` / `no_content`，配合
  `status()` / `content_type()` / `header()` / `download_name()` / `inline_name()`。
- **下载**：对齐 ASP.NET Core 的 `File(...)` 家族。响应内容可以来自磁盘路径
  （`ResponseData::file`）、未知长度的异步读取器（`ResponseData::file_stream`，
  以 chunked 发送）、或已知长度且可 seek 的读取器
  （`ResponseData::file_seekable_stream`，保留精确 `Content-Length` 与 `Range`）。
  `ResponseData::with_file(FileBody)` 暴露文件级开关：
  `enable_range_processing`、`entity_tag`、`last_modified`。
- **下载**：`ResponseData::file` 流式发送，自动带 `Content-Length`、`ETag`、
  `Last-Modified`、`Accept-Ranges` 与符合 RFC 6266/5987 的 `Content-Disposition`，
  并支持 `Range`（`206`/`416`）、`If-Range`、`If-None-Match`/`If-Modified-Since`
  （`304`）与 `HEAD`。
- **断点续传**：`Range` 处理端到端符合 RFC 7233。区间先做合并——重叠、相邻、
  重复的成员都会合并——因此范围请求不可能放大流量到超过文件本身；最多 8 个不相交
  区间会以 `multipart/byteranges` 应答，`Content-Length` 精确计算；超过上限则退回
  完整响应。不可满足的成员被丢弃，全部不可满足时返回 `416` 与
  `Content-Range: bytes */<len>`；语法非法的成员会使整个头失效（§2.1）。
- **断点续传**：校验器语义正确。`If-Range` 按 RFC 7233 §3.2 使用**强**比较
  （弱标签永不匹配），`If-None-Match` 按 RFC 7232 §3.2 使用弱比较。`ETag` 保留完整
  mtime 精度，因此原地修改但大小不变也能被识别；mtime 在未来时不发送
  `Last-Modified`（RFC 7232 §2.2.2）；不支持范围请求时显式发送
  `Accept-Ranges: none`，下载工具不必再试探。
- **性能**：流式响应与落盘上传共用 64 KiB 的 `STREAM_BUF_SIZE`（`ReaderStream`
  原先默认 4 KiB，每 GiB 会多出约 16 倍的 read 与 socket 写入）；文件响应只
  `open` 一次并从同一句柄取长度与 mtime，不再 `metadata()` 后再 `open()`；
  落盘上传走 64 KiB `BufWriter`，`tokio::fs` 不再为每个传输分片调度一次阻塞线程池；
  内存态上传改用引用计数的 `Bytes` 承载，`FormFile::open` 不再复制；
  `FormFile::copy_to` 改用 64 KiB 缓冲，而非 `tokio::io::copy` 的 8 KiB。
- **路由**：`HEAD` 自动回退到同路径的 `GET` 路由，下载端点的 HEAD 头部与 GET 一致、无响应体。
- **静态文件**：`SpaMiddleware` 改为交给宿主流式发送文件，`wwwroot` 资源同样获得
  校验器与范围请求支持，MIME 改用完整扩展名表，不再整份读入内存；并覆盖了默认的
  `cache-control: no-store`。
- **配置**：新增 `Form` 配置节（`MaxRequestSize`、`MaxFileSize`、`MaxFieldSize`、
  `MemoryThreshold`、`TempDir`）。multipart 请求按此度量，其余请求仍用
  `App.MaxBodySize`；声明超限的 `Content-Length` 会在读取上传内容前直接返回 `413`。
- **错误**：新增 `Error::PayloadTooLarge` → `413`、`Error::UnsupportedMediaType` → `415`。
- **文档**：新增[文件上传与下载](docs/rust-webx/05-request-pattern/file-upload-download.md)章节。

#### 变更

- **（破坏性）** `IHttpRequest::body_bytes` / `body_text` 改为 `&mut self`：读取即消费，
  重复读取会返回明确错误而不是空 body；`read_json_body` 改为接收 `&mut dyn IHttpRequest`。
- **（破坏性）** `IHttpRequest` 新增 `content_type()`、`is_multipart()`、`multipart()`；
  `IHttpResponse` 新增 `write_body(ResponseBody)`。两者都有默认实现，现有实现仍可编译。
- **（破坏性）** `ResponseData.body` 改为 `ResponseBody`（`Bytes` 或 `File`）并新增
  `headers` 字段；请改用构造方法而不是结构体字面量。
- **（破坏性）** `IRequest<R>` / `IRequestHandler<T, R>` / `IMediator::send` 不再要求
  `R: Serialize`，约束移到真正序列化的位置。这是 `IRequest<ResponseData>` 能成立的前提。
- 路由 dispatch 函数改为接收 `&mut dyn IHttpContext`，端点因此可以流式读取请求体。
- 请求体不再预先读入内存，在使用之前一直是活的流。
- `IHttpResponse` 不再默认 `Content-Type: application/json`，由写入方显式设置。
- `Cache-Control` 的默认值从 `SecurityHeadersMiddleware` 移到响应收尾阶段，
  以便按响应类型区分：普通响应 `no-store`，文件响应
  `public, max-age=0, must-revalidate`（`ETag` 配 `no-store` 永远用不上）。
  两种情况都只在应用未显式设置 `cache-control` 时生效，显式设置始终优先。
- 流式响应的 body 改用 `UnsyncBoxBody` 装箱，只要求 `Send`——要求 `Sync` 会
  排除掉大多数流式读取器。
- `HttpStatus` 新增 `PARTIAL_CONTENT`、`NOT_MODIFIED`、`PAYLOAD_TOO_LARGE`、
  `UNSUPPORTED_MEDIA_TYPE`、`RANGE_NOT_SATISFIABLE`。

#### 修复

- **安全**：`rustls` 升级到 `0.23.45`、`rustls-webpki` 升级到 `0.103.15`，修复
  RUSTSEC-2026-0285（TLS 1.3 握手消息曾可跨加密层级被接受）。
- `crates/spa` 的 MIME 推断改用完整扩展名表（`.csv`、`.md`、`.webp`、`.mp4` 等
  不再落到 `application/octet-stream`）。
- 静态资源不再被强制 `cache-control: no-store`。
- 修正 `custom-endpoints.md` 中「可在 `register()` 里注册自定义 `IEndpoint`」的错误说明。

#### 参考应用

- **dmbit**：库存导出改为带 `Content-Disposition` 的 `text/csv` 真下载；
  库存导入改为真正的 `multipart/form-data` 上传（`file` + `confirm_update`），
  不再把 CSV 编码进 JSON 字符串。

## [0.3.7] — 2026-09-01 — rust-ef 1.8.3 + docbit DbContext

> **English** · **简体中文** · 迁移指南：[docs/rust-webx/16-migration/upgrade-to-3.0.md](docs/rust-webx/16-migration/upgrade-to-3.0.md)

### English

#### Fixed

- **docbit**: migrate host seed / handler writes to `DbContext` add/update mutations required by rust-ef 1.8.
- **deps**: bump workspace `rust-ef` / `rust-ef-sqlite` / `rust-ef-mysql` to **1.8.3** (String FK `EntityType` fix).

### 简体中文

#### 修复

- **docbit**：主机种子与 handler 写入改为 rust-ef 1.8 所需的 `DbContext` add/update。
- **依赖**：工作区 `rust-ef` 系列升级到 **1.8.3**。

## [0.3.6] — 2026-09-01 — rust-dix 0.7 / rust-ef 1.8.2

> **English** · **简体中文** · 迁移指南：[docs/rust-webx/16-migration/upgrade-to-3.0.md](docs/rust-webx/16-migration/upgrade-to-3.0.md)

### English

#### Changed

- **DI / ORM**: `rust-dix 0.7` + `rust-ef 1.8.2` (single ServiceProvider graph; inject diagnostics; built-in `ScopeFactory`).

### 简体中文

#### 变更

- **DI / ORM**：升级到 `rust-dix 0.7` 与 `rust-ef 1.8.2`。

## [0.3.5] — 2026-08-31 — Cors appsettings bind + docbit Production

> **English** · **简体中文** · 迁移指南：[docs/rust-webx/16-migration/upgrade-to-3.0.md](docs/rust-webx/16-migration/upgrade-to-3.0.md)

### English

#### Fixed

- **CorsSection**: bind PascalCase `Origins` / `Methods` / `Headers` from `appsettings*.json` (previously ignored → default `*`, Production fail-fast panic).
- **docbit Production**: CORS whitelist for `https://www.lusida.net` (+ apex); `TrustProxy` for reverse proxy; SiteUrl `www.lusida.net`.
- **run.sh**: require `JWT_SECRET` / `APP__Jwt__Secret` before start; document lusida.net overrides.

### 简体中文

#### 修复

- **CorsSection**：正确绑定 appsettings 中的 PascalCase CORS 字段（此前回退为 `*`，生产启动直接 panic）。
- **docbit 生产配置**：CORS 含 `www.lusida.net`；反代 `TrustProxy`；站点 URL。
- **run.sh**：启动前校验 JWT 环境变量。

## [0.3.4] — 2026-08-31 — Docbit SQLite-only and rustls

> **English** · **简体中文** · 迁移指南：[docs/rust-webx/16-migration/upgrade-to-3.0.md](docs/rust-webx/16-migration/upgrade-to-3.0.md)

### English

Maintenance release focused on simpler docbit production packaging and OpenSSL-free TLS on Unix.

#### Changed

- **docbit**: SQLite-only runtime (MySQL path removed from production docs, compose, and host wiring).
- **rust-webx-host / docbit**: `reqwest` uses `rustls-tls` (no OpenSSL on Unix); `testing` feature not enabled for docbit production builds.
- **Docbit publish**: `publish.ps1 -Linux` cross-compiles `docbit-host` for `x86_64-unknown-linux-gnu` (Zig linker wrappers under `.tools/`).

### 简体中文

维护版本：docbit 仅 SQLite 部署，以及 Unix 上免 OpenSSL 的 TLS。

#### 变更

- **docbit**：生产环境仅 SQLite（移除 MySQL 相关说明与编排）。
- **rust-webx-host / docbit**：`reqwest` 使用 `rustls-tls`；生产构建不启用 `testing`。
- **Docbit 发布**：`publish.ps1 -Linux` 支持交叉编译。
## [0.3.3] — 2026-08-31 — Docbit cross-compile and docs

> **English** · **简体中文** · 迁移指南：[docs/rust-webx/16-migration/upgrade-to-3.0.md](docs/rust-webx/16-migration/upgrade-to-3.0.md)

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
## [0.3.2] — 2026-08-31 — Architecture remediation Phases 1–4

> **English** · **简体中文** · 迁移指南：[docs/rust-webx/16-migration/upgrade-to-3.0.md](docs/rust-webx/16-migration/upgrade-to-3.0.md)

### English

Breaking and behavioral changes from the architecture remediation (Phases 1–4). Migration guide: [docs/ARCHITECTURE_REMEDIATION.md](docs/ARCHITECTURE_REMEDIATION.md).

#### Breaking

- **Orphan routes/handlers fail at startup** — `HostBuilder::build()` panics when a route lacks `#[handler]`, a handler lacks a route, or duplicate `#[handler]` registrations exist. Run `cargo run -p <host> -- --doctor` before deploy.
- **SPA no longer serves `/api/*` unknown paths** — unmatched API routes return 404/501 from the router, not `index.html`.
- **`global_provider()` / `set_global_provider()` deprecated** — use `host.provider()` or `dispatch_provider()` inside `DispatchRuntime` scope. `Host::build()` no longer sets process-wide provider.
- **`register_handlers!` deprecated for HTTP** — inventory + `#[handler]` is the sole HTTP registration path; macro retained for Mediator-only scenarios.

#### Added

- **`DispatchRuntime`** on each `Host` — instance-scoped provider + `HandlerCache`; HTTP and `IHostedService::start` run inside `dispatch_runtime().run()`.
- **Query binding** — GET/DELETE merges route + query params via `Deserialize`.
- **`#[authorize(permission = "…")]`** — parsed into route metadata; Resource Auth via `use_resource_authorization()`.
- **`#[derive(WebxRequestMeta)]`** — OpenAPI query/path/body param metadata from field attributes (`#[from_query]`, `#[from_route]`, `#[from_body]`).
- **Route diagnostics** — `--doctor` reports orphan routes/handlers and duplicate registrations with fix hints.

#### Changed

- **Middleware order** — CORS → JWT → SPA → Router; `SpaMiddleware` skips `/api/*`.
- **Stub endpoints** — without dispatch return **501** (RFC 7807), not silent 200 stubs.
- **Docbit/dmbit** — `DbInitService` uses `dispatch_provider()`; GET DTOs use `WebxRequestMeta`.

#### Documented (not changed)

- **`jwt_secret()`** remains a process-wide config shim (separate from DI); see security-best-practices.md.

### 简体中文

架构整改 Phase 1–4 的破坏性变更与行为调整。迁移请参阅 [ARCHITECTURE_REMEDIATION.md](docs/ARCHITECTURE_REMEDIATION.md)。

#### 破坏性变更

- **孤儿路由/Handler 启动即 panic** — 运行 `cargo run -p <host> -- --doctor` 排查。
- **`/api/*` 未匹配路径不再返回 SPA `index.html`**。
- **`global_provider()` 已弃用** — 改用 `dispatch_provider()` / `host.provider()`。
- **HTTP 不再以 `register_handlers!` 为主路径** — 使用 inventory + `#[handler]`。

#### 新增

- **`DispatchRuntime`**、**Query 绑定**、**`#[authorize(permission)]`**、**`WebxRequestMeta`**、**`--doctor` 路由诊断**。

#### 变更

- 中间件顺序、Stub 501、Docbit GET DTO OpenAPI 元数据。

## [0.3.1] — 2026-08-16 — crates.io 再发布对齐 · Re-release alignment

> **English** · **简体中文** · 迁移指南：[docs/rust-webx/16-migration/upgrade-to-3.0.md](docs/rust-webx/16-migration/upgrade-to-3.0.md)

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

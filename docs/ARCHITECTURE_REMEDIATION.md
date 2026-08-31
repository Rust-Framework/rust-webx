# Architecture Remediation Plan

Phased plan to address findings from the rust-webx architectural review. **Phases 1–4 are complete** (2026-08). Remaining items below are optional future work only.

## Status Summary

| Priority | Item | Status | Notes |
|----------|------|--------|-------|
| P1 | Middleware ordering (JWT before SPA; `/api/*` guard) | **Done** | `server.rs`, `SpaMiddleware` |
| P1 | Eliminate silent stub failures | **Done** | `StubEndpoint` → 501; `assert_route_configuration_valid()` at build |
| P2 | Sync docs with code | **Done** | Handbook sweep; `inject_attr`/`rust-dicore` → `#[inject]`/`rust-dix` |
| P2 | Deprecate `register_handlers!` for HTTP | **Done** | `#[deprecated]` on macro; docs updated |
| P2 | Doc accuracy: authorize / query / Resource Auth | **Done** | Query binding, permission parse, Resource Auth wiring documented |
| P3 | Unify handler registration model | **Done** | Inventory canonical; `register_handlers!` deprecated for HTTP |
| P3 | Query binding / `authorize(permission)` / Resource Auth wiring | **Done** | Query binding, permission parse, `use_resource_authorization()` |
| P4 | Reduce global state (`ServiceProvider` / `HandlerCache`) | **Done** | `DispatchRuntime` on `Host`; globals deprecated shim only |
| P4 | OpenAPI query param metadata | **Done** | `#[derive(WebxRequestMeta)]` + merge in `generate_openapi_spec` |
| P4 | Duplicate handler diagnostics | **Done** | `duplicate_handlers()` in `--doctor` and startup warnings |

---

## Phase 1 — Safe runtime fixes (implemented)

### 1.1 Middleware ordering

**Problem:** SPA middleware ran before JWT and before the router. Unmatched `GET /api/*` requests received `index.html` with HTTP 200 instead of 404.

**Fix:**
- Pipeline order is now: `… → CORS → JWT → SPA → Router`
- `SpaMiddleware` skips `/api/*` paths entirely (pass-through to router)

**Breaking change:** None for correctly configured apps. Clients that relied on `/api/unknown` returning `index.html` will now get 404/501 from the router.

### 1.2 Fail-fast route configuration

**Problem:** Orphan routes returned HTTP 200 stub text, masking misconfiguration.

**Fix:**
- `HostBuilder::build()` calls `assert_route_configuration_valid()` and **panics** when:
  - a route has no matching `#[handler]`
  - a `#[handler]` has no matching route
  - a route has a handler but no `RouteDispatch` bridge
- `StubEndpoint` without dispatch returns **501 Not Implemented** (RFC 7807 problem response)

**Breaking change:** Builds that previously started with orphan routes now fail at startup. Run `cargo run -p <host> -- --doctor` to inspect the route table.

### 1.3 Documentation sync (partial)

Updated:
- `07-middleware/ordering-strategy.md` — matches implementation
- `05-request-pattern/handler-registration.md` — inventory as canonical path; `#[inject]` from rust-dix
- `FOREWORD.md`, `06-di-lifecycle/injection-patterns.md` — `rust-dicore` → `rust-dix`

Remaining doc drift (addressed in Phase 2 follow-up):
- ~~Full sweep of `inject_attr` → `#[inject]` / `#[derive(Inject)]`~~ — handbook sweep done
- ~~`authorize(permission = "...")` — not yet parsed by macros~~ — **Done** (Phase 3)
- ~~`add_authentication()` — registers JWT only, not `ResourceAuthorization` middleware~~ — documented in authorize-macro, built-in-middleware, resource-authorization
- Docbit architecture doc — multi-crate layout (`contracts/`, `handlers/`, `host/`, `wwwroot/`) — partial in case-study docs

---

## Phase 2 — Handler registration unification (complete)

**Current state:** HTTP dispatch uses **inventory + `HandlerCache`** exclusively:

```
#[get]/#[post] → RouteEntry (inventory)
#[handler]     → HandlerRegistration (inventory)
endpoint macro → RouteDispatch (inventory)
Host build     → StubEndpoint → Mediator → HandlerCache
```

Docs previously described `dyn IRequestHandler` DI lookup as the primary path. That path exists via `register_handlers!` but is **not** used by HTTP `RouteDispatch`.

**Done:**
1. Document inventory as the sole HTTP registration mechanism
2. Deprecate `register_handlers!` for HTTP (`#[deprecated]` on macro in `crates/webx/src/lib.rs`)

**Optional future work:**
3. Compile-time orphan detection in a macro pass (inventory is link-time only; today: runtime panic at build + `--doctor`)

**Migration guide outline:**
- Remove manual `.singleton::<dyn IRequestHandler<…>>()` for HTTP handlers
- Ensure every route DTO has `#[handler]` or `#[handler(inject)]` on its handler impl
- Run `--doctor` after migration

---

## Phase 3 — Unfinished features (implemented)

| Feature | Status | Notes |
|---------|--------|-------|
| Query binding | **Done** | GET/DELETE merges route + query params via `Deserialize` |
| `#[authorize(permission = "…")]` | **Done** | Parsed into `RouteEntry.required_permission` |
| Resource Auth wiring | **Done** | `HostBuilder::use_resource_authorization()` builds policy at endpoint layer |

---

## Phase 4 — Global state reduction (implemented)

**Done:**
- `DispatchRuntime` on each `Host` (provider + `HandlerCache`)
- HTTP requests and `IHostedService::start` run inside `DispatchRuntime::run()` (task-local)
- Macro-generated dispatch uses `dispatch_provider()` (runtime-first, deprecated global shim fallback)
- `Mediator::send` / `dispatch()` use `dispatch_handler_cache()` (runtime-first)
- `Host::provider()` and `Host::dispatch_runtime()` for tests
- `global_provider()` / `set_global_provider()` / `HandlerCache::get_or_init()` **deprecated**; `Host::build()` no longer sets process-wide provider
- Docbit `DbInitService` uses `dispatch_provider()` instead of `global_provider()`
- `#[derive(WebxRequestMeta)]` on docbit GET request DTOs with route-bound fields

**Optional future work:**
- Macro pass that fails `cargo build` on orphan routes (inventory link-time only; runtime panic + `--doctor` instead)
- `jwt_secret()` process-wide shim → per-Host injection (documented in [security-best-practices.md](rust-webx/09-auth-security/security-best-practices.md); acceptable for single-Host deployments)

**Migration:** See [docs/rust-webx/16-migration/global-state.md](rust-webx/16-migration/global-state.md). Use `host.provider()` or `dispatch_provider()` inside `host.dispatch_runtime().run()`. Replace `global_provider()` in hosted services with `dispatch_provider()` (scoped automatically during `Host::run()`).

### 4.2 Compile-time orphan detection

**Done (minimum):** `duplicate_handlers()` in `format_route_diagnostics()` / `--doctor`; startup `tracing::warn!` for duplicate `#[handler]` registrations; actionable fix hints listing route path/method for orphans.

**Deferred (optional):** Macro pass that fails `cargo build` on orphan routes (inventory link-time only).

### 4.3 OpenAPI query param metadata

**Done:**
- `#[derive(WebxRequestMeta)]` on request structs collects `#[from_query]` / `#[from_route]` / `#[from_body]` fields
- `#[webx_request(query_all)]` treats unmarked fields as query params (except `claims` / `#[serde(skip)]`)
- `generate_openapi_spec` merges route-level path/body params with struct metadata
- Docbit: `GetBlogPostRequest`, `GetExhibitionRequest`, docs GET requests, `ListCommentsRequest`

---

## Phase 4 (previous draft) — superseded

Process-wide singletons are deprecated shims only. Instance-scoped `DispatchRuntime` is the canonical path for HTTP and hosted services.

---

## Breaking-change checklist (Phases 1–4)

When upgrading to a release containing these fixes:

- [x] Run `cargo run -p <host-crate> -- --doctor` and fix all orphan routes/handlers
- [x] Verify no client depends on `/api/*` returning SPA `index.html`
- [x] Replace `global_provider()` with `dispatch_provider()` / `host.provider()` — see [global-state.md](rust-webx/16-migration/global-state.md)
- [x] Replace `inject_attr` references with `#[inject]` / `#[derive(Inject)]` (rust-dix)

---

## Testing

After applying Phase 1:

```bash
cd rust-webx
cargo test --workspace
cargo run -p docbit-host -- --doctor
```

---

## Related files

| Area | Files |
|------|-------|
| Middleware order | `crates/host/src/server.rs` |
| SPA `/api` guard | `crates/spa/src/spa.rs` |
| Stub / orphan | `crates/host/src/endpoint.rs`, `crates/host/src/diagnostics.rs` |
| Route diagnostics | `crates/core/src/route/diagnostics.rs` |
| Docbit reference app | `docbit/` |

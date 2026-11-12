# Changelog

All notable changes to **rust-webx** are documented in this file.


## [1.0.2] — 2026-11-12

### Breaking

- **`IRequestHandler::handle` returns `ActionResult<R>`** — the framework-level name for
  ASP.NET Core's `ActionResult<T>`: `Ok(value)` is the response payload, `Err(Error)` is
  mapped to the HTTP status and `problem+json` by the framework. It is the same type as
  `Result<R, Error>` (`T` defaults to `()`), so `?` and `Err(...)` keep working; `Result<T>`
  remains the general fallible alias for infrastructure code. `IPipelineBehavior::handle`
  and `Next::run` follow the same name.

### Added

- **`OperationResult` can be adjusted while the operation runs**: `with_status`,
  `with_message` and `with_result` (unit envelope → payload) sit next to the public fields,
  so an outcome that different branches decide is set in place instead of rebuilt.

## [1.0.1] — 2026-11-12

### Added

- **`OperationResult` / `OperationResult<T>`** — a framework-provided business result DTO:
  `success`, business `status`, `message` and an optional `result` (`OperationResult` has no
  `result` member). It saves every module from defining its own `{ success, status, message }`
  envelope. It is an ordinary DTO, not a transport shape: the HTTP status stays the framework's
  (200 with the serialized body, `problem+json` for `Err(webx::Error)`), and `status` is
  whatever the business assigns. Constructors: `ok`, `ok_with_status`, `failed`, `success`,
  `success_with_status`.

### Breaking

- **`PagedResponse<T>` is removed in favour of `PagedResult<T>`** — the list envelope now
  pairs with `OperationResult`.

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

> **English** · **简体中文**

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

> **English** · **简体中文**

### English

#### Fixed

- **docbit**: migrate host seed / handler writes to `DbContext` add/update mutations required by rust-ef 1.8.
- **deps**: bump workspace `rust-ef` / `rust-ef-sqlite` / `rust-ef-mysql` to **1.8.3** (String FK `EntityType` fix).

### 简体中文

#### 修复

- **docbit**：主机种子与 handler 写入改为 rust-ef 1.8 所需的 `DbContext` add/update。
- **依赖**：工作区 `rust-ef` 系列升级到 **1.8.3**。

## [0.3.6] — 2026-09-01 — rust-dix 0.7 / rust-ef 1.8.2

> **English** · **简体中文**

### English

#### Changed

- **DI / ORM**: `rust-dix 0.7` + `rust-ef 1.8.2` (single ServiceProvider graph; inject diagnostics; built-in `ScopeFactory`).

### 简体中文

#### 变更

- **DI / ORM**：升级到 `rust-dix 0.7` 与 `rust-ef 1.8.2`。

## [0.3.5] — 2026-08-31 — Cors appsettings bind + docbit Production

> **English** · **简体中文**

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

> **English** · **简体中文**

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
## [0.3.2] — 2026-08-31 — Architecture remediation Phases 1–4

> **English** · **简体中文**

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

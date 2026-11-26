**English** | [简体中文](README.zh-CN.md)

<div align="center">

# rust-webx

**An ASP.NET Core-inspired Web API framework for Rust, built on DI + Mediator.**

A type-safe, convention-over-configuration platform where *requests are endpoints* — declare a route with one attribute, implement a handler, and the framework takes care of routing, dependency injection, middleware, and error mapping.

[![Crates.io Version](https://img.shields.io/crates/v/rust-webx)](https://crates.io/crates/rust-webx)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Documentation](https://img.shields.io/badge/docs-mdBook-blue.svg)](https://rust-framework.github.io/rust-webx/)
[![CI](https://github.com/Rust-Framework/rust-webx/actions/workflows/ci.yml/badge.svg)](https://github.com/Rust-Framework/rust-webx/actions/workflows/ci.yml)
![Rust](https://img.shields.io/badge/rust-%3E%3D%201.95-orange.svg)

</div>

---

## Table of Contents

- [What is rust-webx](#what-is-rust-webx)
- [Key Features](#key-features)
- [Why rust-webx](#why-rust-webx)
- [Architecture](#architecture)
- [Request-as-Endpoint: Hello World](#request-as-endpoint-hello-world)
- [Core Concepts](#core-concepts)
- [Broker & cache adapters](#broker--cache-adapters)
- [Optional transport adapters](#optional-transport-adapters)
- [Example Applications](#example-applications)
- [Quick Start](#quick-start)
- [Development](#development)
- [Production Readiness](#production-readiness)
- [Environment Variables](#environment-variables)
- [Error Mapping](#error-mapping)
- [Documentation](#documentation)
- [License](#license)

---

## What is rust-webx

> **If you come from ASP.NET Core and want to keep its DI + Mediator engineering model in Rust, rust-webx is built for you.**

**rust-webx** is a Web API framework for Rust inspired by **ASP.NET Core**. It uses **DI (Dependency Injection) + Mediator** as its dual core. Through the *request-as-endpoint* pattern, developers define complete HTTP APIs with type-safe Rust code — no hand-written route tables, no manual handler registration, no scattered error handling.

Instead of assembling routing libraries, DI containers, middleware and error handling yourself:

```
choose a router → hand-write route tables → design your own DI → wire middleware → unify errors → repeat
```

rust-webx folds these cross-cutting concerns into the framework layer:

| Pain point | How rust-webx solves it |
|------------|--------------------------|
| Routing divorced from handlers | `IRequest<T>` carries route metadata; `#[get("/path")]` registers at compile time |
| Handler registration boilerplate | `#[handler]` auto-registers the handler with the DI container |
| Tight coupling between modules | `IMediator::send()` dispatches requests; `publish()` publishes events |
| Scattered error → HTTP mapping | Unified `Error` type + engine error pipeline (error status is visible to metrics/after hooks before the response is finalized) |
| Ad-hoc auth/authorization | `use_authentication()` + declarative `#[authorize]` |

rust-webx is **not** a full-stack UI framework (pair it with any frontend), **not** an ORM (bring your own or use [`rust-ef`](https://crates.io/crates/rust-ef)), and **not** a microservice-governance platform. The optional `rust-webx-ai*` crates build an OpenAI-compatible gateway on top of the framework; they are a separate, opt-in product surface, not a dependency of the core Web API story.

### Who it is for

- **.NET teams moving to Rust.** The words you already know carry over: `IRequest<T>` / `IRequestHandler<T, R>` for a request and its handler, `IMediator::send()` for MediatR-style dispatch, `#[fromclaim] user: IdentityUser` where `HttpContext.User` used to be, `problem+json` instead of hand-rolled exception middleware, and `HostBuilder` where `Program.cs` used to be.
- **Layered business services.** One operation = one request + one handler + one file; contract, domain, application and host crates make the dependency direction explicit instead of growing a 2,000-line `routes.rs`.
- **API backends that need production plumbing on day one.** Authentication and sessions, validation, OpenAPI, health checks, the transactional outbox, realtime fan-out and metrics are framework ports, not a hand-picked crate stack you have to glue together.

### How it compares

| | Composable HTTP toolkits (`axum`, `actix-web`, `poem`, `rocket`) | rust-webx |
|---|---|---|
| Unit of code | A route handler function plus extractors | A request type that *is* the endpoint (`IRequest<R>`) plus its handler |
| Wiring you still write | Router tree, state/DI choice, error → response conversion, validation and OpenAPI crates | One convention for all of it: `#[handler]`, `#[fromclaim]`, `#[validate]`, `#[authorize]`, and `?` on boundary errors (`From<EFError>` / `From<RdiError>` live in the owning crates) |
| Registration | Register routes by hand, or add your own macro layer | Route and handler metadata collected at compile time (`inventory`), joined once in `Host::build()` |
| Application services | Bring your own | Auth/session/passkeys, rate limiting, idempotency, outbox, realtime, payments, OpenAPI and health checks as ports |
| Escape hatch | — | Handlers are plain Rust, and middleware/after hooks receive `&mut dyn IHttpContext`, so raw HTTP stays reachable |

rust-webx is opinionated on purpose: it does not try to be the smallest router. If you want a minimal HTTP toolkit and prefer to choose every layer yourself, an `axum`-style framework is a better fit. If you are porting a layered ASP.NET Core service and want the same application model on the other side, the mapping is direct.

## Key Features

- **DI + compile-time dispatch core** — `IRequest<T>` + `IRequestHandler<T, R>` as the carrier; the route/handler registry is built at compile time and resolved by `TypeId`, while `IMediator` is the optional in-process façade over the same path.
- **Compile-time route shortcuts** — `#[get("/path")]`, `#[post("/path")]`, `#[put("/path")]`, `#[delete("/path")]` define a full endpoint in one line.
- **Compile-time scanning** — route metadata is collected at compile time via `inventory`; registered automatically in `Host::build()`.
- **Zero-config handler registration** — the `#[handler]` attribute macro registers a handler with the DI container automatically.
- **Authentication & authorization** — JWT Bearer (HS256/RS256/ES256 + JWKS/kid), OIDC, cookie sessions + CSRF, refresh-token rotation, TOTP/WebAuthn passkeys, mTLS, DPoP, HTTP Basic, RFC 7662/7009, OAuth 2.0 social login (GitHub/Google/Microsoft/GitLab/Gitee/DingTalk/Feishu/WeChat/Weibo/QQ/Douyin/Alipay/Facebook), and declarative `#[authorize]`. Start with the [10-minute auth quickstart](docs/rust-webx/09-auth-security/quickstart.md).
- **Identity on the request** — `#[fromclaim] user: IdentityUser` binds the authenticated identity (id, name, email, tenant, roles) as a plain request field; handlers stop parsing tokens. `#[fromclaim(optional)] viewer: Option<IdentityUser>` covers anonymous endpoints (anonymous access must be declared explicitly).
- **Declarative validation** — `#[validate(length/range/email/custom)]` on request fields runs after binding, merges failures into the same `400 problem+json` `errors` map, and reflects the constraints into OpenAPI (`minLength`/`minItems`/`minimum`/`format`).
- **Keyset pagination, client info, instance checks** — `KeysetRequest`/`CursorPage` complement offset `PagedRequest`/`PagedResult`; `#[fromclient] ClientInfo` binds the listener peer plus trusted `X-Forwarded-For` resolution; `RequestUser::require_owner`/`require_same_tenant` centralize instance-level 401/403 decisions.
- **Transactional outbox** — `rust-webx-outbox` ships the port, the lease-based `OutboxDispatcher` (through `EventDispatcher`: backoff, dead-letter, retention) and an `InMemoryOutbox` default, registered with `add_outbox`. Same-transaction enqueue is data adaptation: a non-published EF reference lives in `examples/adapters/outbox/ef` (`add_outbox_ef`).
- **Realtime at IM scale** — one event model with two delivery classes (`EventDelivery::Competing` vs `Broadcast`): `RealtimeFanoutEvent` / `RealtimeUserFanoutEvent` fan out across nodes through the same `IEventPublisher` (Redis Pub/Sub, a NATS broadcast subject, a RabbitMQ fanout exchange), `RedisRealtimeEventStore` shares the replay log, `IOnlinePresence` (+ Redis implementation and a host heartbeat bridge) shares online state, and `WebSocketLimits::with_session_revalidation` closes a live socket when its JWT/cookie session is revoked (close `1008`). See the [multi-instance realtime guide](docs/rust-webx/11-production/realtime-cluster.md).
- **Mobile push** — `IPushSender` with a three-way outcome (`Accepted`/`TokenInvalid`/`Retryable`) so dead device tokens are not mistaken for server errors; `HostBuilder::use_push` wires APNs (`rust-webx-apns`) or FCM (`rust-webx-fcm`), and `InMemoryPushSender` captures pushes in tests. See the [push guide](docs/rust-webx/13-extensibility/push-notifications.md).
- **Ports, not starters** — every infrastructure capability is a port (`IIdempotencyStore`, `IOutbox`, `ISessionStore`, `IObjectStore`, `IMeter`, `IAdvisoryLock`, `IEmailSender`, …) that either ships a zero-config in-memory default (`IIdempotencyStore`, `IOutbox`, `IObjectStore`, `IMeter`, `IAdvisoryLock`) or deliberately has no default implementation (the notification and push senders), so a missing adapter fails at startup as a DI error instead of silently no-op'ing. Entities, tables and the data/logic adaptation belong to the downstream service: implement the port in your crate, or copy a non-published reference adapter from `examples/adapters/` and run the port contracts in `testing/contract-tests`. See the [ports & adapters guide](docs/rust-webx/13-extensibility/integrations.md).
- **Endpoint metadata service** — `IEndpointCatalog` exposes every endpoint at runtime (`all()` / `localized()`), so permission sync, admin navigation and OpenAPI share one source.
- **Localization** — `#[display]` / `#[description]` / `#[category]` store resource keys; a host-wide `Culture.Default` selects the language, with `.json` resources or `include_str!` embedding.
- **Production capabilities out of the box** — graceful shutdown, live health checks, security headers, request IDs, CORS, TLS, rate limiting, compression, OpenAPI + SPA hosting.
- **Aggregated startup validation** — `HostBuilder::try_build` reports every invalid configuration key once, as a sorted list, with the same Production fail-closed guards as before (no more "first error only").
- **Wire-level test harness & fuzz corpus** — `TestHost::connect_wire()` serves real HTTP/1.1 keep-alive, h2c, SSE bytes and malformed requests over an in-process duplex pipe (no port, no socket); a separate `fuzz/` cargo-fuzz workspace covers cursor/config/ClientInfo/ProblemDetails/protector parsing. See [test-host.md](docs/design/test-host.md) and [stability-gates.md](docs/design/stability-gates.md).
- **Stable observability contract** — a `webx.http` span tree (request/auth/route/handler/binding/response) plus `webx.connection` opened/closed events, with span/event names and fields documented as a compatibility contract in [observability-contract.md](docs/design/observability-contract.md).
- **Close reasons & error registry** — `CloseReason`, `IConnection::close_reason()` and `ShutdownToken::is_draining()` unify WebSocket/SSE shutdown semantics; 23 registered error codes (`ERROR_CODES`) and a printed close-code table keep `problem+json` `code` values and close frames from drifting. See [close-codes.md](docs/design/close-codes.md) and [error-codes.md](docs/design/error-codes.md).
- **Admission invariants on `/metrics`** — active/capacity ratio, invariant-violation gauges and a shutdown gauge make permit/wait accounting leaks visible without a debugger.

- **File upload & download** — typed `FormFile` binding for `multipart/form-data` (streamed, spooled to disk, size-limited) and ASP.NET Core-shaped file responses (`FileResult::path` / `::stream` / `::seekable_stream`, `ContentResult`) with `Content-Disposition`, `ETag`, `Last-Modified` and `Range`.
- **ORM-agnostic** — the framework does not depend on `rust-ef`; the `docbit` reference app wires it in with minimal glue.
- **Payments as a domain group** — opt-in `rust-webx-pay*` crates: one `IPaymentProvider` port for Stripe, Alipay, WeChat Pay v3, PayPal, Adyen and Square, with runtime `IPaymentServiceFactory`/`PaymentScope` configuration, a frontend payment-method catalog (`PaymentOption`), multi-account routing, raw-body webhook verification and provider-specific ACKs. A durable webhook inbox is data adaptation: the non-published reference adapter lives in `examples/adapters/payment/ef`. See the [payments chapter](docs/rust-webx/13-extensibility/payments.md).
- **Mediator without an annotation tax** — a `#[handler]` whose Request has no HTTP route is automatically an `IMediator` handler; its response does not need `Serialize`. HTTP capability belongs to the route, not to a handler mode flag.

## Why rust-webx

| Advantage | What it means in practice |
|---|---|
| **ASP.NET Core-shaped model** | Requests are endpoints: `#[get]`/`#[post]` on `impl IRequest<R>`, `#[handler]` on the implementation, DI and middleware configured once in `Host`. |
| **Compile-time dispatch, no stringly route tables** | Route metadata and direct handler bridges are collected by `inventory`; `Host::build()` joins them and fails fast on inconsistent configuration. |
| **Low-allocation hot path** | The direct JSON path binds, constructs and encodes without a mediator round trip or request/response `Box<dyn Any>`. The framework tax is **10.42 allocations/request** above the bare-hyper floor (6.00) on the Windows reference `/hello` benchmark (Linux CI: 12.42); CI gates the tax at ≤13 (Windows reference ≤12). |
| **Performance without giving up correctness** | Allocation counts, wire-level correctness and stability suites are release gates. See the [allocation budget](docs/design/alloc-budget.md) and [stability gates](docs/design/stability-gates.md). |
| **Production admission and lifecycle** | One global connection governor across all listeners, first-request deadline, HTTP/2 keepalive, graceful/immediate shutdown guards, Host single-use state, and per-worker SSE drain. |
| **Errors that downstream systems can trust** | Error responses are written before reverse `after` hooks, so metrics/tracing/compression see the real status; representation headers from a failed success path are stripped and `problem+json` is consistent. |
| **Native-first integrations** | `rust-dix` 1.0 DI scopes, `rust-ef` 2.6+ ORM (verified paired with rust-ef 2.9.1), JWT/session/password ports, and `rust-webx-client` are framework capabilities rather than ad-hoc app glue. |
| **Explicit HTTP vs in-process capabilities** | `#[handler]` without a route is mediator-only; a route adds the response encoder. No `#[handler(in_process)]`-style mode flag and no hidden HTTP endpoint for a command handler. |

The numbers above are a local same-window comparison, not a capacity guarantee.
Allocation counts and the wire-level correctness suite are the release
regression signals; the full method and budget live in the
[allocation budget](docs/design/alloc-budget.md).

## Architecture

The framework is split into a small set of focused crates, re-exported through the `rust-webx` umbrella crate:

```
┌──────────────────────────────────────────────────────────┐
│              rust-webx  (umbrella crate)                  │
│  re-exports core / host / macros / spa / openapi + rust_dix│
├──────────┬──────────┬──────────┬─────────────────────────┤
│rust-webx-│rust-webx-│rust-webx-│ rust-webx-macros         │
│host      │core      │engine    │ #[get] #[post] #[handler]│
│Host +    │traits +  │worker    │ #[authorize] #[fromclaim]│
│pipeline  │config    │pool +    │                          │
│          │          │dispatch  │                          │
├──────────┴──────────┴──────────┴─────────────────────────┤
│                    rust-webx-core                        │
│  IHost / IHttpContext / IMiddleware / IRequestHandler    │
│  IMediator / AppOptions / Error / AppMode                │
└──────────────────────┬───────────────────────────────────┘
                       │
          ┌────────────┼────────────┐
          ▼            ▼            ▼
   ┌──────────┐ ┌──────────┐ ┌──────────┐
   │ rust-dix │ │  hyper   │ │inventory │
   │ (DI)     │ │ (HTTP)   │ │ (route   │
   └──────────┘ └──────────┘ └────╌─────┘
                                   collection
```

### Crate layout

```
rust-webx/
├── Cargo.toml                 # workspace root — published runtime crates
├── crates/
│   ├── core/                  # rust-webx-core  — traits, configuration, rust-core re-exports
│   ├── engine/                # rust-webx-engine — worker pool, direct dispatch
│   │                          #   zero-copy context, pipeline, TLS/HTTP2
│   ├── host/                  # rust-webx-host  — Host builder, middleware
│   │                          #   routing/health/SPA adapters over the engine
│   ├── macros/                # rust-webx-macros — procedural macros
│   ├── spa/                   # rust-webx-spa   — static-file / SPA middleware
│   ├── openapi/               # rust-webx-openapi — OpenAPI spec generation + UI
│   ├── client/                # rust-webx-client — outbound HTTP: pool, streaming, SSE
│   ├── webx/                  # rust-webx       — umbrella crate (re-exports)
│   ├── ai/                    # AI domain group
│   │   ├── core/              #   rust-webx-ai         — gateway domain kit
│   │   └── gateway/           #   rust-webx-ai-gateway — OpenAI-compatible /v1
│   ├── auth/                  # authentication domain group
│   │   ├── oidc/              #   rust-webx-oidc
│   │   ├── oauth2/            #   rust-webx-oauth2
│   │   ├── mfa/               #   rust-webx-mfa
│   │   └── webauthn/          #   rust-webx-webauthn
│   ├── cache/                 # third-party cache adapters
│   │   └── redis/             #   rust-webx-redis — IDistributedCache + event transport
│   ├── queue/                 # third-party message-queue adapters
│   │   ├── nats/              #   rust-webx-nats
│   │   ├── rabbitmq/          #   rust-webx-rabbitmq
│   │   └── kafka/             #   rust-webx-kafka
│   ├── push/                  # push provider adapters
│   │   ├── apns/              #   rust-webx-apns — APNs (HTTP/2 token auth)
│   │   └── fcm/               #   rust-webx-fcm — Firebase Cloud Messaging HTTP v1
│   ├── transport/             # optional non-hyper listener backends
│   │   └── h3/                #   rust-webx-h3 — HTTP/3 (QUIC + rustls + h3)
│   ├── payment/               # payment domain group (opt-in)
│   │   ├── core/              #   rust-webx-pay — ports, models, Payments runtime
│   │   ├── stripe/            #   rust-webx-pay-stripe
│   │   ├── alipay/            #   rust-webx-pay-alipay
│   │   ├── wechatpay/         #   rust-webx-pay-wechatpay
│   │   ├── paypal/            #   rust-webx-pay-paypal
│   │   ├── adyen/             #   rust-webx-pay-adyen
│   │   └── square/            #   rust-webx-pay-square
│   └── outbox/                #   rust-webx-outbox — outbox port, dispatcher, InMemoryOutbox
├── examples/                  # reference apps & adapters (one cargo workspace each, own lockfile)
│   ├── docbit/                #   portfolio + blog + RBAC + docs site
│   ├── dmbit/                 #   device & inventory management
│   ├── imbit/                 #   real-time IM (WS + signaling)
│   └── adapters/              #   non-published reference adapters (publish = false)
│       └── ai|auth|idempotency|outbox|payment/ef/   # ports implemented on rust-ef (copy, never depend)
├── testing/                   # non-published test harnesses (publish = false)
│                              #   alloc-probe / contract-tests / upgrade-smoke / load-probe
└── fuzz/                      # standalone cargo-fuzz workspace (nightly CI)
```

> **Package vs import name**: the dependency is `rust-webx`, the code is
> `use webx::*;`. Same for the sub-crates — `rust-webx-core` → `webx_core`,
> `rust-webx-engine` → `webx_engine`, `rust-webx-host` → `webx_host`,
> `rust-webx-macros` → `webx_macros`,
> `rust-webx-spa` → `webx_spa`, `rust-webx-openapi` → `webx_openapi`,
> `rust-webx-ai` → `webx_ai`, `rust-webx-client` → `webx_client`,
> `rust-webx-ai-gateway` → `webx_ai_gateway`,
> `rust-webx-oidc` → `webx_oidc`,
> `rust-webx-oauth2` → `webx_oauth2`, `rust-webx-mfa` → `webx_mfa`,
> `rust-webx-webauthn` → `webx_webauthn`, `rust-webx-redis` → `webx_redis`,
> `rust-webx-nats` → `webx_nats`, `rust-webx-rabbitmq` → `webx_rabbitmq`,
> `rust-webx-kafka` → `webx_kafka`, `rust-webx-outbox` → `webx_outbox`,
> `rust-webx-pay` → `webx_pay`,
> `rust-webx-pay-stripe` → `webx_pay_stripe`, `rust-webx-pay-alipay` → `webx_pay_alipay`,
> `rust-webx-pay-wechatpay` → `webx_pay_wechatpay`, `rust-webx-pay-paypal` → `webx_pay_paypal`,
> `rust-webx-pay-adyen` → `webx_pay_adyen`, `rust-webx-pay-square` → `webx_pay_square`,
> `rust-webx-h3` → `webx_h3`.
> `rust-webx-apns` → `webx_apns`, `rust-webx-fcm` → `webx_fcm`.
>
> The `*-ef` reference adapters under `examples/adapters/` are **not dependencies**:
> they are `publish = false`, so they have no crates.io package and no import name to rely on.

## Request-as-Endpoint: Hello World

Declare a request, map a route onto it, implement its handler — no route table to maintain:

```rust
use webx::*;

struct HelloRequest;

#[get("/hello")]
impl IRequest<String> for HelloRequest {}

#[derive(Default)]
struct HelloHandler;

#[handler] // compile-time registration into the DI container
#[async_trait]
impl IRequestHandler<HelloRequest, String> for HelloHandler {
    async fn handle(&mut self, _req: HelloRequest) -> ActionResult<String> {
        Ok("Hello, World! Welcome to Rust WebX.".to_string())
    }
}

#[tokio::main]
async fn main() {
    Host::builder()
        .build()
        .run()
        .await
        .expect("Server failed");
}
```

Routing discovery, handler resolution, middleware orchestration, and graceful shutdown are all handled automatically by `Host::build()`.

### Composition root: assembly slots

Every application is assembled through the same few slots on the builder:

| Slot | What belongs there | Example |
|---|---|---|
| `register` | DI services, ports and adapters — the object graph | `.register(\|svc\| svc.add_mediator())` |
| `configure` | `AppOptions` runtime knobs for this host | `.configure(\|app\| app.options(\|o\| o.metrics.enabled = false))` |
| `add_options` / `add_options_with` | one configuration section bound to one `T`; `_with` applies a code-level override after binding | `.add_options_with::<SiteConfig>("Site", \|s\| s.public_url = site_url)` |

Typed helpers (`use_spa`, `use_authentication`, `use_http_transport`, `use_alt_svc`, ...) are
sugar over the same builder state and never introduce a third assembly channel. Business code
consumes interfaces; only the composition root knows concrete implementations.

The full verb table (`add_*` / `use_*` / `register` / `configure` / `add_options*` /
`watch_options_file`) lives in [api-convergence.md](docs/design/api-convergence.md).

## Core Concepts

| Concept | Description |
|---------|-------------|
| `IRequest<TResponse>` | Generic request marker carrying the response type. `IRequest<()>` returns `204 No Content`. |
| `IRequestHandler<T, R>` | Two-type-parameter handler; `T` is the request type, `R` the response type. |
| `#[get("/path")]` | Route shortcut annotated on an `impl IRequest<T>` block; registered at compile time. |
| `#[post("/path")]` / `#[put("/path")]` / `#[delete("/path")]` | HTTP-method route shortcuts. |
| `IMediator` | Mediator; `send()` dispatches requests, `publish()` hands events to the event transport (non-blocking, no handler result). |
| `IMiddleware` | Middleware; sequential pipeline that can short-circuit requests. |
| `IPipelineBehavior` | Mediator pipeline interceptor that wraps the request-handling chain. |
| `IEventHandler<T>` | Event handler; `publish()` delivers asynchronously through `IEventPublisher` (default: in-memory queue). |
| `Error` | Unified error type automatically mapped to HTTP status codes. |
| `IClaims` / `IAuthenticationHandler` | JWT auth interfaces that extract identity from the Bearer token. |
| `RequestUser` | Instance-level ownership checks in handlers: `require_owner` / `require_same_tenant` (401 vs 403); route-level rules stay on `#[authorize]`. |
| `IDynamicAuthorizer` | Route-level runtime authorization (DB/cache/feature flags) after static `#[authorize]` rules. |
| `IEndpointCatalog` | Runtime endpoint metadata (`all()` / `localized()`); DI-injected or `host.endpoints()`. |
| `IStringLocalizer` | Resource-key lookup for endpoint text and OpenAPI; default culture from `Culture.Default`. |

### Request lifecycle

```
HTTP Request
    │
    ▼
EngineContext::new(req)                ← move hyper Parts, lazy body
    │
    ▼
engine Pipeline::execute()             ← IMiddleware in registration order;
    │                                      short-circuit supported
    ▼
matchit Router::match_route            ← method + path; {param} → route_params
    │
    ▼
Route matched?  ──No──▶  405/OPTIONS/SPA fallback or 404
    │ Yes
    ▼
RouteTarget                            ← #[authorize] guard + max_body_size
    │
    ├─ no typed behavior ─▶ handler direct dispatch → EndpointResult
    └─ typed behavior ────▶ mediated HandlerCache path
    ▼
after hooks (real error status) → EngineContext::finalize()
    ▼
hyper::Response                        ← problem+json for errors; bytes/file/SSE for success
```

## Broker & cache adapters

Event transport and the distributed cache are opt-in adapter crates. One `add_*` call registers an
`IEventPublisher` implementation plus a background consumer; `publish()` and `#[handler]` event
handlers stay unchanged.

| Crate | Entry point | Delivery semantics |
|-------|-------------|--------------------|
| [`rust-webx-redis`](https://crates.io/crates/rust-webx-redis) | `add_redis(RedisOptions::new(url))` — also registers `IDistributedCache` | at-least-once (Redis Streams consumer group; `XACK` after handlers, pending reclaimed, bounded retries → dead-letter stream) |
| [`rust-webx-nats`](https://crates.io/crates/rust-webx-nats) | `add_nats(NatsOptions::new(url))` | at-most-once (core NATS; optional queue group) |
| [`rust-webx-rabbitmq`](https://crates.io/crates/rust-webx-rabbitmq) | `add_rabbitmq(RabbitMqOptions::new(url))` | at-least-once (quorum topic queue, publisher confirms, manual ack + requeue, `x-delivery-limit` + optional DLX) |
| [`rust-webx-kafka`](https://crates.io/crates/rust-webx-kafka) | `add_kafka(KafkaOptions::new(brokers, topic))` | single-partition follower; per-partition ordering; no consumer-group offset commits (restart uses the configured start offset); reference adapter, published |

Cache-only: `RedisOptions::cache_only()`. Publisher-only: `.publisher_only()` on NATS/RabbitMQ/Kafka;
Redis uses `.without_cache()` + `RedisEventsOptions { consumer_enabled: false, .. }`. See
[event pub/sub](docs/rust-webx/08-mediator-events/event-pub-sub.md) and
[caching](docs/rust-webx/11-production/caching-rate-limit.md).

## Optional transport adapters

Beyond the built-in hyper HTTP/1.1 + HTTP/2 listeners, the engine has a neutral transport port
(`IHttpTransport` / `TransportRuntime`) that serves the same router, middleware, binding, error
writer, DI, admission governor and metrics. HTTP/3 is available as an opt-in crate; default builds
do not pull QUIC dependencies.

| Crate | Entry point | Notes |
|-------|-------------|-------|
| [`rust-webx-h3`](https://crates.io/crates/rust-webx-h3) | `H3Transport` + `HostBuilder::use_http_transport(Arc::new(transport))`, optional `use_alt_svc("h3=\":<udp_port>\"; ma=86400")`, optional `H3Config::with_webtransport_handler` | QUIC + rustls + h3 fork (`trev-h3`), TLS 1.3 / ALPN `h3`, mTLS, SSE/streaming, per-connection GOAWAY drain, shared `App.MaxConnections` admission, `/metrics` transport labels, explicit `Alt-Svc`, **WebSocket over HTTP/3 via RFC 9220 extended CONNECT**, and **WebTransport adapter-level MVP** (session + bidi stream echo; transport-neutral port is future work). |

## Example Applications

Three full reference applications ship in this repository, demonstrating the framework, layered project structure (`contracts` / `domain` / `handlers` / `host`), JWT auth, RBAC, real-time channels, and SPA hosting. Each one is its own cargo workspace under [`examples/`](examples/README.md):

| Application | Description | Dev URL |
|-------------|-------------|---------|
| [`docbit`](examples/docbit/) | Portfolio + blog + RBAC + a full docs site (`docs/`). Uses `rust-ef` (SQLite in dev and production). | <http://localhost:5000> |
| [`dmbit`](examples/dmbit/) | Device & inventory management for a data-center rig (device/product/spec/stock management). SQLite-based. | <http://localhost:5100> |
| [`imbit`](examples/imbit/) | Real-time IM: HTTP + WebSocket in one host, room messaging with replay, signaling, TURN credentials, recording. | <http://localhost:8090> |

Run the reference apps in development (one workspace per app, so pass `--manifest-path`):

```bash
cargo run --manifest-path examples/docbit/Cargo.toml -p docbit-host   # → http://localhost:5000
cargo run --manifest-path examples/dmbit/Cargo.toml -p dmbit-host     # → http://localhost:5100
cargo run --manifest-path examples/imbit/Cargo.toml -p imbit-host     # → http://localhost:8090 (bin name: imbit)
```

For production deployment details, see [`examples/docbit/PRODUCTION.md`](examples/docbit/PRODUCTION.md).

## Quick Start

### Build

```bash
# release build of a specific application
cargo build --release --manifest-path examples/docbit/Cargo.toml -p docbit-host
cargo build --release --manifest-path examples/dmbit/Cargo.toml -p dmbit-host
```

### Run a reference app

```bash
cargo run --manifest-path examples/docbit/Cargo.toml -p docbit-host     # development (SQLite), http://localhost:5000
```

### Publish (bare metal)

```bash
# Linux / macOS
chmod +x examples/docbit/publish.sh
./examples/docbit/publish.sh /opt/docbit --production

# Windows
.\examples\docbit\publish.ps1 -Destination D:\deploy\docbit -Production
```

Set `DATABASE_URL` and `JWT_SECRET` before starting, or edit the generated `run.sh` / `run.cmd`.

### Docker (optional)

Primary deployment is **Linux binary + static assets** via `examples/docbit/publish.sh` (see [docbit/PRODUCTION.md](examples/docbit/PRODUCTION.md)). Docker files are reference-only and not maintained in CI:

```bash
# standalone image (verify locally before use)
docker build -f examples/docbit/Dockerfile .

# local stack (docbit + SQLite volume)
cp examples/docbit/.env.example examples/docbit/.env     # edit JWT_SECRET
docker compose -f examples/docbit/docker-compose.yml --env-file examples/docbit/.env up --build
# → http://localhost:8100
```

## Development

Run the full suite from the workspace root:

```bash
cargo test --workspace --no-fail-fast
```

### Disk usage (large Windows checkouts)

`target/` is disposable, but Cargo never deletes artifacts that belong to an old
fingerprint, so every full rebuild (toolchain bump, `Cargo.lock` update,
`RUSTFLAGS` change, switching feature sets) leaves its whole universe behind.
The dev/test profiles in [`Cargo.toml`](Cargo.toml) keep *new* builds small
(no debug info for dependencies, line tables only for workspace crates, no
incremental sessions). If `target/` has already grown, report and reclaim it:

```powershell
.\scripts\clean-target.ps1              # report sizes, change nothing
.\scripts\clean-target.ps1 -Incremental # drop the incremental compile cache only
.\scripts\clean-target.ps1 -All         # cargo clean (everything is rebuildable)
```

For a debugging or fast-edit session, override the committed profile for that
shell only:

```powershell
$env:CARGO_PROFILE_DEV_DEBUG = '2'             # full debug info for workspace crates
$env:CARGO_PROFILE_DEV_INCREMENTAL = 'true'    # incremental compilation back on
```

## Production Readiness

| Capability | Status |
|------------|--------|
| Graceful shutdown (Ctrl+C / SIGTERM) | ✅ |
| Connection drain (30s in Production) | ✅ |
| Header read timeout (slowloris) | ✅ 30s, bounded header buffer |
| Health checks `/health` `/health/ready` (runtime probes, fail → 503) | ✅ |
| Security response headers + Request ID | ✅ on by default |
| JWT production fail-fast | ✅ |
| CORS `*` production fail-fast | ✅ |
| Panic isolation (handler → 500, process survives) | ✅ |
| OpenAPI UI | Development mode only |
| Rate limiting / compression | app-layer opt-in (`use_middleware`) |
| File uploads (streamed, disk-spooled, size-limited) | ✅ |
| File downloads (`FileResult`, streamed, `Content-Disposition`, `ETag`, `Range`, `304`, `HEAD`) | ✅ |
| TLS | ✅ via `App.Urls` + `Tls.CertPath/KeyPath` |

## Environment Variables

- `APP_ENV` — application environment (`Development` / `Production`).
- `WEBX_APP_BASE` — application base directory.
- `JWT_SECRET` — JWT signing secret. Read in every mode; `APP_ENV=Production` additionally rejects secrets shorter than 32 bytes or known placeholders.
- `APP__Jwt__Secret` — overrides `JWT_SECRET`.
- `DATABASE_URL` — optional DB override (docbit defaults to SQLite `app.db` beside the binary).
- `APP__*` — inline JSON override of any `appsettings.json` key (e.g. `APP__App__Urls`, `APP__Cors__Origins`).

See the configuration chapter under `docs/rust-webx/` for the full variable reference.

## Error Mapping

`webx::Error` is `rust_core::AppError` (2.9.1: a kind + private fields, built through constructors,
read through accessors — the 2.9 enum is gone). `kind` decides the default status/code/retryable;
constructors can tighten it:

| Constructor | HTTP status | `code` | Retryable | Meaning |
|-------------|-------------|--------|-----------|---------|
| `Error::validation(msg)` | 400 | `validation_failed` | no | Validation failure (with `with_field`) |
| `Error::bad_request(msg)` | 400 | `bad_request` | no | Invalid input |
| `Error::invalid_json(err)` | 400 | `invalid_json` | no | Request (de)serialization error |
| `Error::unauthorized(msg)` | 401 | `unauthorized` | no | Authentication failed / missing token |
| `Error::forbidden(msg)` | 403 | `forbidden` | no | Insufficient permissions |
| `Error::not_found(msg)` | 404 | `not_found` | no | Resource not found |
| `Error::route_not_found(msg)` | 404 | `not_found` | no | Routing miss |
| `Error::conflict(msg)` | 409 | `conflict` | no | Optimistic concurrency / state conflict |
| `Error::conflict_retryable(msg)` | 409 | `conflict` | yes | Conflict that resolves by retrying |
| `Error::payload_too_large(msg)` | 413 | `payload_too_large` | no | Request body or uploaded file too large |
| `Error::unsupported_media_type(msg)` | 415 | `unsupported_media_type` | no | Unsupported request content type |
| `Error::too_many_requests(secs, msg)` | 429 | `too_many_requests` | yes | Rate limited (`Retry-After`) |
| `Error::unavailable(msg)` | 503 | `service_unavailable` | yes | Upstream unavailable / timeout |
| `Error::internal(operator)` / `Error::state(operator)` | 500 | `internal_error` | no | Server-side defect; `public_detail() = None` |
| `Error::di(operator)` | 500 | `dependency_injection` | no | DI container / composition-root defect |
| `Error::response_serialization(err)` | 500 | `internal_error` | no | Response encoding failed (server-side DTO defect) |
| `Error::status(code, msg)` | `code` | derived from status | derived | Explicit status override |
| `ProblemDetails::validation(..).to_error()` | 400 | `validation_failed` | no | Field-level binding/validation problem carrying `errors` |

Boundary errors need no adapter: `EFError` and `RdiError` implement `From` in their own crates, so
handlers just use `?`. The classification authority is
[`app-error-migration.md`](docs/design/app-error-migration.md); the code registry is
`rust_core::error_codes::ERROR_CODES`.

## Documentation

- **Online handbook** — the full book is published at <https://rust-framework.github.io/rust-webx/> (mdBook, Chinese; built from `docs/rust-webx/` by `.github/workflows/docs.yml`).
- **rust-webx developer's manual** — a progressive-disclosure book under [`docs/rust-webx/`](docs/rust-webx/INDEX.md) (16 chapters, Chinese): introduction, quick start, architecture, DI & lifecycle, middleware, mediator & events, auth & security, configuration, production, project structure, extensibility, best practices, case study and migration guides.
- **Upgrading** — 2.9.1 is a breaking patch (the planned 3.0 contract migration landed there); 2.9.2 is additive IM hardening (no migration), and 2.9.3 is a boundary-closure patch whose registered behavior changes (`#[handler]` on reserved fan-out events fails host build — never register them manually; Redis `start_id` default `"0"`; Redis presence key layout) are listed in [compatibility.md](docs/design/compatibility.md) §3, with rollout constraints in [known-boundaries.md](docs/design/known-boundaries.md) C4. See the [2.9.1 upgrade guide](docs/rust-webx/16-migration/upgrade-to-2.9.1.md) and, for the 2.x additive line, the [2.5 upgrade guide](docs/rust-webx/16-migration/upgrade-to-2.5.md).
- **rust-ef reference** — the ORM handbook lives with the [`rust-ef`](https://crates.io/crates/rust-ef) crate (see its [docs.rs](https://docs.rs/rust-ef) documentation), used by the `docbit` example.
- The `docbit` reference app also serves these docs as a live website (`GET /api/docs/rust-webx/...`).

## License

[MIT](LICENSE)

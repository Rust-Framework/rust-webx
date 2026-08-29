**English** | [简体中文](README.zh-CN.md)

<div align="center">

# rust-webx

**An ASP.NET Core-inspired Web API framework for Rust, built on DI + Mediator.**

A type-safe, convention-over-configuration platform where *requests are endpoints* — declare a route with one attribute, implement a handler, and the framework takes care of routing, dependency injection, middleware, and error mapping.

[![Crates.io Version](https://img.shields.io/crates/v/rust-webx)](https://crates.io/crates/rust-webx)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![CI](https://github.com/Rust-Framework/rust-webx/actions/workflows/ci.yml/badge.svg)](https://github.com/Rust-Framework/rust-webx/actions/workflows/ci.yml)
![Rust](https://img.shields.io/badge/rust-%3E%3D%201.81-orange.svg)

</div>

---

## Table of Contents

- [What is rust-webx](#what-is-rust-webx)
- [Key Features](#key-features)
- [Architecture](#architecture)
- [Request-as-Endpoint: Hello World](#request-as-endpoint-hello-world)
- [Core Concepts](#core-concepts)
- [Example Applications](#example-applications)
- [Quick Start](#quick-start)
- [Production Readiness](#production-readiness)
- [Environment Variables](#environment-variables)
- [Error Mapping](#error-mapping)
- [Documentation](#documentation)
- [License](#license)

---

## What is rust-webx

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
| Scattered error → HTTP mapping | Unified `Error` type + built-in exception middleware |
| Ad-hoc auth/authorization | `add_authentication()` + declarative `#[authorize]` |

rust-webx is **not** a full-stack UI framework (pair it with any frontend), **not** an ORM (bring your own or use [`rust-ef`](https://crates.io/crates/rust-ef)), and **not** a microservice-governance platform.

## Key Features

- **DI + Mediator dual core** — `IRequest<T>` + `IRequestHandler<T, R>` as the carrier; the framework handles route mapping and DI resolution automatically.
- **Compile-time route shortcuts** — `#[get("/path")]`, `#[post("/path")]`, `#[put("/path")]`, `#[delete("/path")]` define a full endpoint in one line.
- **Compile-time scanning** — route metadata is collected at compile time via `inventory`; registered automatically in `Host::build()`.
- **Zero-config handler registration** — the `#[handler]` attribute macro registers a handler with the DI container automatically.
- **Authentication & authorization** — JWT Bearer auth + route-pattern-based resource authorization (`#[authorize]`, `#[claims]`).
- **Production capabilities out of the box** — graceful shutdown, live health checks, security headers, request IDs, CORS, TLS, rate limiting, compression, OpenAPI + SPA hosting.
- **ORM-agnostic** — the framework does not depend on `rust-ef`; the `docbit` reference app wires it in with minimal glue.

## Architecture

The framework is split into a small set of focused crates, re-exported through the `rust-webx` umbrella crate:

```
┌──────────────────────────────────────────────────────────┐
│              rust-webx  (umbrella crate)                  │
│  re-exports core / host / macros / spa / openapi + rust_dix│
├──────────┬──────────┬──────────┬─────────────────────────┤
│rust-webx-│rust-webx-│rust-webx-│ rust-webx-macros         │
│host      │core      │openapi   │ #[get] #[post] #[handler]│
│Host +    │traits +  │OpenAPI   │ #[authorize] #[claims]   │
│pipeline  │config    │generation│                          │
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
├── Cargo.toml                 # workspace root (v0.3.0)
├── crates/
│   ├── core/                  # rust-webx-core  — traits, configuration
│   ├── host/                  # rust-webx-host  — Host builder, middleware
│   │                          #   pipeline, Trie-based router, hyper integration
│   ├── macros/                # rust-webx-macros — procedural macros
│   ├── spa/                   # rust-webx-spa   — static-file / SPA middleware
│   ├── openapi/               # rust-webx-openapi — OpenAPI spec generation + UI
│   └── webx/                  # rust-webx       — umbrella crate (re-exports)
├── docbit/                    # reference app: portfolio + blog + RBAC + docs
└── dmbit/                     # reference app: device & inventory management
```

## Request-as-Endpoint: Hello World

Declare a request, map a route onto it, implement its handler — no route table to maintain:

```rust
use rust_webx::*;

struct HelloRequest;

#[get("/hello")]
impl IRequest<String> for HelloRequest {}

#[derive(Default)]
struct HelloHandler;

#[handler] // compile-time registration into the DI container
#[async_trait]
impl IRequestHandler<HelloRequest, String> for HelloHandler {
    async fn handle(&self, _req: HelloRequest) -> Result<String> {
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

## Core Concepts

| Concept | Description |
|---------|-------------|
| `IRequest<TResponse>` | Generic request marker carrying the response type. `IRequest<()>` returns `204 No Content`. |
| `IRequestHandler<T, R>` | Two-type-parameter handler; `T` is the request type, `R` the response type. |
| `#[get("/path")]` | Route shortcut annotated on an `impl IRequest<T>` block; registered at compile time. |
| `#[post("/path")]` / `#[put("/path")]` / `#[delete("/path")]` | HTTP-method route shortcuts. |
| `IMediator` | Mediator; `send()` dispatches requests, `publish()` publishes events. |
| `IMiddleware` | Middleware; sequential pipeline that can short-circuit requests. |
| `IPipelineBehavior` | Mediator pipeline interceptor that wraps the request-handling chain. |
| `IEventHandler<T>` | Event handler; broadcast to all registered handlers via `publish()`. |
| `Error` | Unified error type automatically mapped to HTTP status codes. |
| `IClaims` / `IAuthenticationHandler` | JWT auth interfaces that extract identity from the Bearer token. |
| `IAuthorizationPolicy` | Authorization-policy interface; checks roles/permissions against route patterns. |

### Request lifecycle

```
HTTP Request
    │
    ▼
HttpContext::new(req).await            ← read body bytes
    │
    ▼
MiddlewarePipeline::execute()          ← middleware run in registration order;
    │                                      each may short-circuit the response
    ▼
Router::match_route(ctx)               ← Trie match on method + path;
    │                                      {param} values → route_params
    ▼
Route matched?  ──No──▶  404 "Not Found"
    │ Yes
    ▼
IEndpoint::handle(ctx)                 ← invoke handler, serialize response
    ▼
HttpResponse → hyper::Response         ← on Err, built-in exception middleware maps status
    ▼
JSON: RFC 7807 application/problem+json (4xx / 5xx)
```

## Example Applications

Two full reference applications ship in this repository, demonstrating the framework, layered project structure (`contracts` / `handlers` / `domain`), JWT auth, RBAC, and SPA hosting:

| Application | Description | Dev URL |
|-------------|-------------|---------|
| [`docbit`](docbit/) | Portfolio + blog + RBAC + a full docs site (`docs/`). Uses `rust-ef` (SQLite in dev, MySQL in production). | <http://localhost:5000> |
| [`dmbit`](dmbit/) | Device & inventory management for a data-center rig (device/product/spec/stock management). SQLite-based. | <http://localhost:5100> |

Run the reference apps in development:

```bash
cargo run -p docbit-host     # → http://localhost:5000
cargo run -p dmbit-host      # → http://localhost:5100
```

For production deployment details, see [`docbit/PRODUCTION.md`](docbit/PRODUCTION.md).

## Quick Start

### Build

```bash
# release build of a specific application
cargo build --release -p docbit-host
cargo build --release -p dmbit-host
```

### Run a reference app

```bash
cargo run -p docbit-host     # development (SQLite), http://localhost:5000
```

### Publish (bare metal)

```bash
# Linux / macOS
chmod +x docbit/publish.sh
./docbit/publish.sh /opt/docbit --production

# Windows
.\docbit\publish.ps1 -Destination D:\deploy\docbit -Production
```

Set `DATABASE_URL` and `JWT_SECRET` before starting, or edit the generated `run.sh` / `run.cmd`.

### Docker

```bash
# standalone image
docker build -f docbit/Dockerfile .

# full local stack (docbit + MySQL)
cp docbit/.env.example docbit/.env     # edit JWT_SECRET and MYSQL_ROOT_PASSWORD
docker compose -f docbit/docker-compose.yml --env-file docbit/.env up --build
# → http://localhost:8100
```

## Production Readiness

| Capability | Status |
|------------|--------|
| Graceful shutdown (Ctrl+C / SIGTERM) | ✅ |
| Connection drain (30s in Production) | ✅ |
| Health checks `/health` `/health/ready` (runtime probes, fail → 503) | ✅ |
| Security response headers + Request ID | ✅ on by default |
| JWT production fail-fast | ✅ |
| CORS `*` production fail-fast | ✅ |
| OpenAPI UI | Development mode only |
| Rate limiting / compression | app-layer opt-in (`use_middleware`) |
| TLS | ✅ via `App.Urls` + `Tls.CertPath/KeyPath` |

## Environment Variables

- `APP_ENV` — application environment (`Development` / `Production`).
- `RUST_WEBX_APP_BASE` — application base directory.
- `JWT_SECRET` — JWT signing secret (≥32 chars; requires `APP_ENV=Production`).
- `APP__Jwt__Secret` — overrides `JWT_SECRET`.
- `DATABASE_URL` — database connection string (docbit production, e.g. `mysql://user:pass@host:3306/docbit`).
- `APP__*` — inline JSON override of any `appsettings.json` key (e.g. `APP__App__Urls`, `APP__Cors__Origins`).

See the configuration chapter under `docs/rust-webx/` for the full variable reference.

## Error Mapping

| `Error` variant | HTTP status | Meaning |
|-----------------|-------------|---------|
| `Error::NotFound(msg)` | 404 | Resource not found |
| `Error::Validation(msg)` | 400 | Validation failure |
| `Error::Serialization(e)` | 400 | (De)serialization error |
| `Error::Http(msg)` | 400 | HTTP protocol error (incl. 401 Unauthorized, 403 Forbidden) |
| `Error::Di(msg)` | 500 | DI container error |
| `Error::Internal(msg)` | 500 | Internal error |
| `Error::Message(msg)` | 500 | Generic error message |
| `Error::Routing(msg)` | 404 | Routing error |

## Documentation

- **rust-webx developer's manual** — a progressive-disclosure book under [`docs/rust-webx/`](docs/rust-webx/INDEX.md) (16 chapters, Chinese): introduction, quick start, architecture, DI & lifecycle, middleware, mediator & events, auth & security, configuration, production, project structure, extensibility, best practices, case study and migration guides.
- **rust-ef reference** — the ORM handbook under [`docs/rust-ef/`](docs/rust-ef/INDEX.md), used by the `docbit` example.
- The `docbit` reference app also serves these docs as a live website (`GET /api/docs/rust-webx/...`).

## License

[MIT](LICENSE)
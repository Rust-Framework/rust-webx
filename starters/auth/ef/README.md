# rust-webx-auth-ef

Durable `rust-ef` stores for `rust-webx` authentication state:

- **Refresh tokens** — `auth_refresh_tokens`, implementing
  `webx_core::security::IRefreshTokenStore` with atomic compare-and-set
  rotation, family revocation, and reuse detection semantics.
- **Browser sessions** — `auth_sessions`, implementing
  `webx_core::security::ISessionRepository`; cookie sessions survive restarts
  and are shared by every instance (`purge_expired` for cleanup jobs).
- A `RefreshTokenEntity` ready for the official migration tooling
  (`ensure_created` is test-only; production uses `rust-ef migration`), exposed
  through the same `register_refresh_token_model` starter entry point as
  `rust-webx-ef`'s `register_idempotency_model`.

```rust
use std::sync::Arc;
use rust_ef::prelude::*;
use webx_auth_ef::{EfRefreshTokenStore, EfSessionRepository};
use webx_host::refresh::RefreshTokenService;

let store = Arc::new(EfRefreshTokenStore::from_options(Arc::clone(&db_options)));
let refresh = RefreshTokenService::new(
    store,
    Duration::from_secs(30 * 24 * 3600),
    Duration::from_secs(90 * 24 * 3600),
);

// Cookie sessions (multi-instance): register as dyn ISessionRepository.
let sessions = Arc::new(EfSessionRepository::from_options(Arc::clone(&db_options)));
```

## Model registration

`RefreshTokenEntity` derives `EntityType`, so a **default** `DbContext` discovers it from the
compile-time `inventory` registry without any call. Call `register_refresh_token_model` when the
context is **keyed** (a keyed model only discovers entities tagged `#[context("key")]`), or when a
starter wants an explicit, idempotent registration before model-based schema creation, exactly like
`rust-webx-ef`'s `register_idempotency_model`:

```rust,ignore
use rust_ef::db_context::DbContext;
use webx_auth_ef::register_refresh_token_model;
use webx_ef::idempotency::register_idempotency_model;

// 1. One options instance for the whole process.
let options = Arc::new(build_db_options());

// 2. Explicit model registration before model-based DDL (bootstrap / diff).
let mut ctx = DbContext::from_options(&options)?;
register_refresh_token_model(&mut ctx); // auth_refresh_tokens
register_idempotency_model(&mut ctx);   // webx_idempotency, same starter step
ctx.bootstrap().await?;                 // fresh database / test; model-based

// 3. Stores over the same options (shared provider/pool).
let store = Arc::new(EfRefreshTokenStore::from_options(Arc::clone(&options)));
```

`ctx.migrate("Migrations")` reads versioned **SQL** files and does not use the model, so
registration is irrelevant to that path. Both starter crates expose the same contract; the relative
order of the two `register_*` calls does not matter, and registration is idempotent.

The store is intentionally thin: callers provide configured `DbContextOptions`
and own migrations. It never stores the refresh secret — only its SHA-256
digest, which the host service computes.

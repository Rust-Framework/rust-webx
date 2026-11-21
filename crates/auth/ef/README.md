# rust-webx-auth-ef

Durable `rust-ef` stores for `rust-webx` authentication state:

- **Refresh tokens** — `auth_refresh_tokens`, implementing
  `webx_core::security::IRefreshTokenStore` with atomic compare-and-set
  rotation, family revocation, and reuse detection semantics.
- **Browser sessions** — `auth_sessions`, implementing
  `webx_core::security::ISessionRepository`; cookie sessions survive restarts
  and are shared by every instance (`purge_expired` for cleanup jobs).
- A `RefreshTokenEntity` ready for the official migration tooling
  (`ensure_created` is test-only; production uses `rust-ef migration`).

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

The store is intentionally thin: callers provide configured `DbContextOptions`
and own migrations. It never stores the refresh secret — only its SHA-256
digest, which the host service computes.

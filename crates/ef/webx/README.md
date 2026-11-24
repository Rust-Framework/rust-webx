# rust-webx-ef

Compatibility package for the extracted durable idempotency store.

`rust-ef` is Web-agnostic (since 2.6 it does not have a `webx` feature), and since 2.9.1 it implements
`From<EFError> for rust_core::AppError` itself. Handlers therefore use `?` directly; this package no
longer translates errors and no longer exposes `EfResultExt` / `map_ef_error`.

The idempotency store now lives in [`rust-webx-idempotency-ef`](../idempotency/ef/README.md):

```rust,ignore
use rust_webx_idempotency_ef::{register_idempotency_model, EfIdempotencyStore};

register_idempotency_model(&mut ctx);
let store = Arc::new(EfIdempotencyStore::new(db_options));
host.use_idempotency(store, ttl);
```

This package re-exports those items (including `webx_ef::idempotency::*`) for one release and is
scheduled for removal in 3.0.

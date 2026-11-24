# rust-webx-idempotency-ef

Durable `Idempotency-Key` store for [rust-webx](https://github.com/Rust-Framework/rust-webx),
backed by [rust-ef](https://github.com/Rust-Framework/rust-ef).

`EfIdempotencyStore` implements `webx_core::idempotency::IIdempotencyStore` on a database table, so
multiple application instances share reservations and replay cached responses. Register
`IdempotencyRecord` once (`register_idempotency_model`), hand the application's
`Arc<DbContextOptions>` to the store, and enable the `Idempotency` configuration section.

```rust,ignore
let store = Arc::new(EfIdempotencyStore::new(Arc::clone(&db_options)));
host.use_idempotency(store, ttl);
```

The store uses a fencing token (`attempt`) and conditional updates: a reclaimed lease's late
completion/release cannot overwrite the new owner; `IdempotencyHousekeeping` purges expired rows in
bounded batches.

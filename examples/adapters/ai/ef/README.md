# rust-webx-ai-ef

Durable `rust-ef` usage ledger for `rust-webx-ai`.

`EfUsageSink` implements `IUsageSink` on top of one `DbContext` per write. It is
idempotent on `UsageEvent::request_id`: a duplicate settlement is a no-op (or a
unique-constraint race that is also treated as success).

```rust
use std::sync::Arc;
use rust_ef::prelude::*;
use webx_ai_ef::EfUsageSink;

let options = Arc::clone(&db_options);
let sink = EfUsageSink::new(Arc::new(move || DbContext::from_options(&options)));

let executor = GatewayExecutor::new(catalog, router)
    .with_usage(Arc::new(sink));
```

The entity is `ai_usage_events`; `DbContext::ensure_created()` / migrations create
the table. `EfUsageSink` also writes `ai_usage_outbox` in the same `save_changes`
(disable with `without_outbox()`); `EfUsageOutbox` reads/acks/backs off rows, and
`EfUsageRollup` writes idempotent `ai_usage_rollups` windows. `UsageOutboxWorker`
and `UsageRollupWorker` implement `PeriodicBackgroundService` (register with
`add_background_service_with`) for the scheduled drain/rollup.

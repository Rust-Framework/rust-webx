# rust-webx-ai

AI gateway domain kit for [`rust-webx`](https://crates.io/crates/rust-webx).

This crate holds the AI-specific half of the gateway design: the canonical
request/stream IR, the `ChatProvider` port and OpenAI-compatible adapter, the
model catalog/router (with `CatalogHandle` hot reload), the request executor,
an exact response cache with single-flight, resilience primitives
(retry/backoff, circuit breaker, provider bulkhead), usage metering and ledger
sinks, windowed rollups, cost calculation (including dry-run estimates), token
accounting, API-key parsing/HMAC helpers with an `IApiKeyStore` port and
in-memory store, quota reserve/settle, and the OpenAI error envelope
(`OpenAiErrorWriter`). HTTP data-plane endpoints and non-OpenAI adapters live in
separate crates; the executor emits structured `tracing` events (request id,
model, provider, latency, tokens, cost, trace id; no prompt/response text).

```toml
[dependencies]
rust-webx-ai = "1.3"
```

The version always matches `rust-webx` — `version.workspace = true`, no
independent 0.x line.

Scope: no HTTP engine, no provider SDKs, no database code. Provider-specific
fields stay in adapters, never in the shared types.

See `docs/design/ai-gateway.md` in the `rust-webx` repository for the full plan.

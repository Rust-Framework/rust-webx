# rust-webx-ai-gateway

OpenAI-compatible HTTP surface for a [`rust-webx-ai`](https://crates.io/crates/rust-webx-ai)
model gateway.

It provides the `/v1/chat/completions` endpoint: the request DTO, conversion to
the canonical IR, JSON/SSE responses, and the OpenAI error envelope.

```rust
use std::sync::Arc;
use webx::Host;
use webx_ai::GatewayExecutor;

webx_ai_gateway::install(); // force the route inventory into the binary

// Register the executor built from your catalog/router/quota/usage wiring.
Host::builder()
    .use_authentication()          // management-plane JWT
    // or: .add_authentication_scheme(Arc::new(ApiKeyScheme::new(validator)))
    .add_error_writer(Arc::new(webx_ai::OpenAiErrorWriter::new()))
    .register(move |svc| svc.instance(Arc::new(executor)))
    .build()
    .run()
    .await?;
```

The crate deliberately contains no provider/routing/quota logic: those live in
`rust-webx-ai`. This crate is the HTTP adapter that makes the data plane
OpenAI-compatible.

Scope:

- `POST /v1/chat/completions` (JSON + SSE);
- `GET /v1/models` (public aliases);
- `dry_run: true` token/cost estimate without calling the provider;
- request conversion to `CanonicalRequest`, response conversion from
  `CanonicalResponse`;
- `OpenAiErrorWriter`-compatible errors;
- no embeddings, images or audio yet;
- no dashboard/admin UI.

# OPTIONAL — not the primary deployment path for docbit.
# Recommended: Linux binary publish via docbit/publish.sh (see docbit/PRODUCTION.md).
# This Dockerfile is kept for reference only and is not exercised in CI.
#
#   docker build -f docbit/Dockerfile .

FROM rust:1-bookworm AS builder
WORKDIR /build

# Cache dependencies: copy manifests first
COPY Cargo.toml Cargo.lock ./
COPY crates/core/Cargo.toml crates/core/
COPY crates/host/Cargo.toml crates/host/
COPY crates/macros/Cargo.toml crates/macros/
COPY crates/webx/Cargo.toml crates/webx/
COPY crates/spa/Cargo.toml crates/spa/
COPY crates/openapi/Cargo.toml crates/openapi/
COPY docbit/contracts/Cargo.toml docbit/contracts/
COPY docbit/domain/Cargo.toml docbit/domain/
COPY docbit/handlers/Cargo.toml docbit/handlers/
COPY docbit/host/Cargo.toml docbit/host/

RUN mkdir -p crates/core/src crates/host/src crates/macros/src crates/webx/src \
    crates/spa/src crates/openapi/src \
    docbit/contracts/src docbit/domain/src docbit/handlers/src docbit/host/src \
    && echo "fn main(){}" > docbit/host/src/main.rs \
    && cargo build --release -p docbit-host 2>/dev/null || true

COPY . .
RUN cargo build --release -p docbit-host

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /build/target/release/docbit-host /app/docbit-host
COPY docbit/appsettings.json docbit/appsettings.Production.json ./
COPY docbit/wwwroot ./wwwroot
# Ecosystem docs mirror (run scripts/sync-docs.ps1 before docker build if missing)
COPY docs ./docs

ENV APP_ENV=Production
EXPOSE 8100

HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
    CMD curl -f http://127.0.0.1:8100/health || exit 1

CMD ["./docbit-host"]

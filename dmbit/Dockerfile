# dmbit production image — multi-stage build from workspace root:
#   docker build -f dmbit/Dockerfile .

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
COPY dmbit/contracts/Cargo.toml dmbit/contracts/
COPY dmbit/domain/Cargo.toml dmbit/domain/
COPY dmbit/handlers/Cargo.toml dmbit/handlers/
COPY dmbit/host/Cargo.toml dmbit/host/

RUN mkdir -p crates/core/src crates/host/src crates/macros/src crates/webx/src \
    crates/spa/src crates/openapi/src \
    dmbit/contracts/src dmbit/domain/src dmbit/handlers/src dmbit/host/src \
    && echo "fn main(){}" > dmbit/host/src/main.rs \
    && cargo build --release -p dmbit-host 2>/dev/null || true

COPY . .
RUN cargo build --release -p dmbit-host

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /build/target/release/dmbit-host /app/dmbit-host
COPY dmbit/appsettings.json dmbit/appsettings.Production.json ./
COPY dmbit/wwwroot ./wwwroot

ENV APP_ENV=Production
EXPOSE 5100

HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
    CMD curl -f http://127.0.0.1:5100/health || exit 1

CMD ["./dmbit-host"]

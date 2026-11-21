#!/usr/bin/env bash
# Run the env-gated adapter suite against a local broker stack.
# Requires Docker with Compose v2; Java/Kafka tooling is inside the image.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
compose="$root/docker-compose.brokers.yml"
timeout_seconds="${BROKER_TIMEOUT_SECONDS:-240}"

if [[ "${BROKER_VALIDATE_ONLY:-0}" == "1" ]]; then
  docker compose -f "$compose" config >/dev/null
  echo "docker-compose.brokers.yml: OK"
  exit 0
fi

cleanup() {
  docker compose -f "$compose" down -v >/dev/null 2>&1 || true
}
trap cleanup EXIT

docker compose -f "$compose" up -d --wait

wait_port() {
  local port="$1"
  local deadline=$((SECONDS + timeout_seconds))
  while (( SECONDS < deadline )); do
    if (echo > "/dev/tcp/127.0.0.1/$port") >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  echo "broker on port $port did not open within ${timeout_seconds}s" >&2
  return 1
}

# Redis/RabbitMQ/Kafka have healthchecks; NATS is TCP-gated here.
for port in 6379 4222 5672 9092; do
  wait_port "$port"
done

WEBX_REDIS_URL=redis://127.0.0.1:6379 \
WEBX_NATS_URL=nats://127.0.0.1:4222 \
WEBX_RABBITMQ_URL='amqp://guest:guest@127.0.0.1:5672/%2f' \
WEBX_KAFKA_BROKERS=127.0.0.1:9092 \
  cargo test -p rust-webx-redis -p rust-webx-nats \
    -p rust-webx-rabbitmq -p rust-webx-kafka --tests -- --nocapture

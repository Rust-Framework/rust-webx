#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# HTTP interop / conformance gate for rust-webx.
#
# Starts a real engine listener (crates/webx/examples/auth_quickstart.rs) and
# asserts an external-client matrix with curl:
#
#   protocol   HTTP/1.1 and HTTP/2 (h2c prior knowledge)
#   status     200, 400, 401, 413, 415
#   envelope   application/problem+json with `status`, `code` and `retryable`
#
# TLS ALPN is intentionally not exercised here: the example has no TLS
# listener. crates/host/tests/http2_test.rs covers real h2-over-TLS ALPN, and
# the conformance CI job runs that test next to this script.
#
# h2spec (RFC 7540/7541 wire conformance) is optional: CI installs it, but when
# it is missing this script prints a SKIP and the curl matrix remains the
# enforced gate. h2spec failures are bounded by H2SPEC_MAX_FAILURES (default 5;
# baseline is 1 known deviation — the server closes the TCP connection instead
# of sending GOAWAY on an invalid connection preface) so the job is not red on
# day one. Lower it once the baseline is recorded in
# docs/design/stability-gates.md and the known deviation is fixed.
#
# Local Windows curl builds usually have no HTTP/2 support (no nghttp2): the
# HTTP/2 half of the matrix then prints a SKIP and is only enforced on the CI
# runner, whose curl advertises HTTP2. h2spec itself speaks HTTP/2 and runs on
# Windows too.
#
# Usage: bash scripts/conformance.sh
# Env:
#   H2SPEC_MAX_FAILURES   maximum tolerated h2spec failures (default 5; the
#                         current baseline is 1 known deviation)
#   CONFORMANCE_LOG_DIR   optional directory receiving h2spec/server logs (CI
#                         uploads it as an artifact on success and failure)
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail
cd "$(dirname "$0")/.."

# Fixed by the example's `Urls`; the script fails clearly if the port is busy.
PORT=5000
BASE_URL="http://127.0.0.1:${PORT}"
H2SPEC_MAX_FAILURES="${H2SPEC_MAX_FAILURES:-5}"
LOG_DIR="${CONFORMANCE_LOG_DIR:-}"
WORK_DIR="$(mktemp -d)"
SERVER_PID=""
CHECKS=0
FAILURES=0

cleanup() {
    trap - EXIT
    if [ -n "${SERVER_PID}" ] && kill -0 "${SERVER_PID}" 2>/dev/null; then
        kill "${SERVER_PID}" 2>/dev/null || true
        for _ in $(seq 1 50); do
            kill -0 "${SERVER_PID}" 2>/dev/null || break
            sleep 0.1
        done
        kill -9 "${SERVER_PID}" 2>/dev/null || true
    fi
    if [ -n "${LOG_DIR}" ]; then
        mkdir -p "${LOG_DIR}"
        for file in "${WORK_DIR}"/*; do
            if [ -e "${file}" ]; then
                cp -f "${file}" "${LOG_DIR}/" 2>/dev/null || true
            fi
        done
    fi
    rm -rf "${WORK_DIR}"
}
trap cleanup EXIT

pass() {
    CHECKS=$((CHECKS + 1))
    printf 'PASS  %s\n' "$1"
}

fail() {
    CHECKS=$((CHECKS + 1))
    FAILURES=$((FAILURES + 1))
    printf 'FAIL  %s\n' "$1"
}

# One request; status on stdout, headers/body in $WORK_DIR. Never aborts the
# script on a transport error — that is reported as `curl-error` instead.
request() {
    local output
    if ! output="$(curl -sS --max-time 30 -D "$WORK_DIR/headers" -o "$WORK_DIR/body" \
        -w '%{http_code}' "$@")"; then
        printf 'curl-error'
        return 0
    fi
    printf '%s' "${output}"
}

# Same, for HTTP/2: echoes "<status> <http_version>".
request_h2() {
    local output
    if ! output="$(curl -sS --max-time 30 --http2-prior-knowledge \
        -D "$WORK_DIR/headers" -o "$WORK_DIR/body" \
        -w '%{http_code} %{http_version}' "$@")"; then
        printf 'curl-error'
        return 0
    fi
    printf '%s' "${output}"
}

# Assert the saved response is an RFC 9457 problem+json document.
assert_problem() {
    local label="$1" expected="$2"
    if ! grep -qiE '^content-type:[[:space:]]*application/problem\+json' "$WORK_DIR/headers"; then
        fail "${label}: content-type is not application/problem+json"
        return 1
    fi
    if ! grep -Eq "\"status\"[[:space:]]*:[[:space:]]*${expected}([^0-9]|$)" "$WORK_DIR/body"; then
        fail "${label}: problem body has no \"status\": ${expected}"
        return 1
    fi
    if ! grep -q '"code"' "$WORK_DIR/body"; then
        fail "${label}: problem body has no stable \"code\" member"
        return 1
    fi
    if ! grep -q '"retryable"' "$WORK_DIR/body"; then
        fail "${label}: problem body has no \"retryable\" member"
        return 1
    fi
    return 0
}

expect_status() {
    local label="$1" expected="$2"
    shift 2
    local actual
    actual="$(request "$@")"
    if [ "${actual}" = "${expected}" ]; then
        pass "${label} -> HTTP ${actual}"
    else
        fail "${label}: expected HTTP ${expected}, got ${actual}"
    fi
}

expect_problem() {
    local label="$1" expected="$2"
    shift 2
    local actual
    actual="$(request "$@")"
    if [ "${actual}" != "${expected}" ]; then
        fail "${label}: expected HTTP ${expected}, got ${actual}"
        return
    fi
    if assert_problem "${label}" "${expected}"; then
        pass "${label} -> HTTP ${actual} problem+json"
    fi
}

expect_h2() {
    local label="$1" expected="$2"
    shift 2
    local result status version
    result="$(request_h2 "$@")"
    status="${result%% *}"
    version="${result##* }"
    if [ "${status}" = "${expected}" ] && [ "${version}" = "2" ]; then
        pass "${label} -> HTTP/2 ${status}"
    else
        fail "${label}: expected HTTP/2 ${expected}, got '${result}'"
    fi
}

expect_h2_problem() {
    local label="$1" expected="$2"
    shift 2
    local result status version
    result="$(request_h2 "$@")"
    status="${result%% *}"
    version="${result##* }"
    if [ "${status}" != "${expected}" ] || [ "${version}" != "2" ]; then
        fail "${label}: expected HTTP/2 ${expected}, got '${result}'"
        return
    fi
    if assert_problem "${label}" "${expected}"; then
        pass "${label} -> HTTP/2 ${status} problem+json"
    fi
}

for tool in curl cargo; do
    if ! command -v "${tool}" >/dev/null 2>&1; then
        echo "conformance: '${tool}' is required" >&2
        exit 1
    fi
done

# Windows curl builds ship without nghttp2; degrade that half of the matrix
# loudly instead of failing on an unsupported curl option.
CURL_HTTP2=0
if curl --version 2>/dev/null | grep -q 'HTTP2'; then
    CURL_HTTP2=1
fi

echo "== building the scenario (cargo build -p rust-webx --example auth_quickstart) =="
cargo build -q -p rust-webx --example auth_quickstart

BIN=""
for candidate in target/debug/examples/auth_quickstart target/debug/examples/auth_quickstart.exe; do
    if [ -x "${candidate}" ]; then
        BIN="${candidate}"
        break
    fi
done
if [ -z "${BIN}" ]; then
    echo "conformance: built example binary not found under target/debug/examples/" >&2
    exit 1
fi

echo "== starting ${BIN} on ${BASE_URL} (APP_ENV=Development) =="
APP_ENV=Development "${BIN}" >"${WORK_DIR}/server.log" 2>&1 &
SERVER_PID=$!

ready=0
for _ in $(seq 1 300); do
    if ! kill -0 "${SERVER_PID}" 2>/dev/null; then
        echo "conformance: scenario server exited during startup; log:" >&2
        cat "${WORK_DIR}/server.log" >&2 || true
        exit 1
    fi
    if curl -fsS -o /dev/null --max-time 1 "${BASE_URL}/health/live" 2>/dev/null; then
        ready=1
        break
    fi
    sleep 0.1
done
if [ "${ready}" -ne 1 ]; then
    echo "conformance: scenario server did not become ready on ${BASE_URL}" >&2
    cat "${WORK_DIR}/server.log" >&2 || true
    exit 1
fi
echo "== server ready; running the HTTP matrix =="

# ── HTTP/1.1 ─────────────────────────────────────────────────────────────────
expect_status "HTTP/1.1 GET /health/live" 200 --http1.1 "${BASE_URL}/health/live"

expect_status "HTTP/1.1 POST /auth/login (valid JSON)" 200 --http1.1 \
    -X POST "${BASE_URL}/auth/login" \
    -H 'Content-Type: application/json' \
    --data '{"email":"conformance@example.com","password":"admin123"}'

TOKEN="$(sed -n 's/.*"token":"\([^"]*\)".*/\1/p' "${WORK_DIR}/body")"
if [ -n "${TOKEN}" ]; then
    pass "login response carries a bearer token"
else
    fail "login response carries no \"token\" member"
fi

expect_status "HTTP/1.1 GET /me (valid bearer)" 200 --http1.1 \
    -H "Authorization: Bearer ${TOKEN}" "${BASE_URL}/me"

expect_problem "HTTP/1.1 GET /me (no credentials)" 401 --http1.1 "${BASE_URL}/me"
if grep -qi '^www-authenticate:[[:space:]]*Bearer' "${WORK_DIR}/headers"; then
    pass "401 carries a WWW-Authenticate: Bearer challenge"
else
    fail "401 is missing the WWW-Authenticate: Bearer challenge"
fi

expect_problem "HTTP/1.1 POST /auth/login (wrong media type)" 415 --http1.1 \
    -X POST "${BASE_URL}/auth/login" \
    -H 'Content-Type: text/plain' \
    --data '{"email":"conformance@example.com","password":"admin123"}'

expect_problem "HTTP/1.1 POST /auth/login (malformed JSON)" 400 --http1.1 \
    -X POST "${BASE_URL}/auth/login" \
    -H 'Content-Type: application/json' \
    --data '{"email":'

# Default `App.MaxBodySize` is 10 MiB; a declared Content-Length above it is
# rejected (413) before the body is read or bound.
head -c 10485761 /dev/zero | tr '\0' 'x' >"${WORK_DIR}/oversized.json"
expect_problem "HTTP/1.1 POST /auth/login (body over App.MaxBodySize)" 413 --http1.1 \
    -X POST "${BASE_URL}/auth/login" \
    -H 'Content-Type: application/json' \
    --data-binary "@${WORK_DIR}/oversized.json"

# ── HTTP/2 (h2c prior knowledge; h2-over-TLS ALPN is http2_test.rs) ──────────
if [ "${CURL_HTTP2}" -eq 1 ]; then
    expect_h2 "HTTP/2 GET /health/live" 200 "${BASE_URL}/health/live"

    expect_h2 "HTTP/2 POST /auth/login (valid JSON)" 200 \
        -X POST "${BASE_URL}/auth/login" \
        -H 'Content-Type: application/json' \
        --data '{"email":"conformance@example.com","password":"admin123"}'

    expect_h2_problem "HTTP/2 GET /me (no credentials)" 401 "${BASE_URL}/me"
else
    echo "SKIP  curl has no HTTP/2 support (no nghttp2); HTTP/2 matrix is CI-only."
fi

# ── h2spec (optional, bounded) ───────────────────────────────────────────────
if command -v h2spec >/dev/null 2>&1; then
    echo "== h2spec: RFC 7540/7541 wire conformance (plain h2c) =="
    h2spec_status=0
    h2spec -h 127.0.0.1 -p "${PORT}" -P /health/live -o 2 \
        -j "${WORK_DIR}/h2spec.xml" http2 hpack generic \
        >"${WORK_DIR}/h2spec.log" 2>&1 || h2spec_status=$?

    summary="$(grep -E '[0-9]+ tests, [0-9]+ passed, [0-9]+ skipped, [0-9]+ failed' \
        "${WORK_DIR}/h2spec.log" | tail -n 1 || true)"
    if [ -z "${summary}" ]; then
        fail "h2spec produced no summary (exit ${h2spec_status}); last output:"
        tail -n 20 "${WORK_DIR}/h2spec.log" || true
    else
        h2spec_failed="$(printf '%s' "${summary}" | sed -E 's/^.* ([0-9]+) failed$/\1/')"
        if [ "${h2spec_failed}" -gt "${H2SPEC_MAX_FAILURES}" ]; then
            fail "h2spec: ${summary} exceeds H2SPEC_MAX_FAILURES=${H2SPEC_MAX_FAILURES}"
            sed -n '/^Failures:/,$p' "${WORK_DIR}/h2spec.log" | head -n 120 || true
        else
            pass "h2spec: ${summary} (tolerated failures: ${H2SPEC_MAX_FAILURES})"
        fi
    fi
else
    echo "SKIP  h2spec not installed; the curl matrix above is the enforced gate."
fi

echo
echo "== conformance summary: ${CHECKS} checks, ${FAILURES} failure(s) =="
if [ "${FAILURES}" -ne 0 ]; then
    exit 1
fi

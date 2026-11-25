#!/usr/bin/env bash
# -----------------------------------------------------------------------------
# Architecture gate: the runtime framework must not depend on rust-ef.
#
# `rust-webx` (umbrella), `rust-webx-core`, `rust-webx-host` and
# `rust-webx-engine` are the runtime surface. The ORM adapter crates
# (`rust-webx-idempotency-ef`, `-auth-ef`, `-outbox-ef`, `-pay-ef`, `-ai-ef`) are
# opt-in reference adapters (examples/adapters, unpublished) and are allowed to
# depend on both sides; they must never leak into the core feature graph.
# Feedback §37 defines the target: no `webx ↔ ef` edge; only reference adapters
# may link both.
#
# This check is cheap (metadata only) and runs in CI before a build, so a stray
# `rust-ef` dependency fails the PR that introduces it instead of a release.
# -----------------------------------------------------------------------------
set -euo pipefail
cd "$(dirname "$0")/.."

fail=0
for package in rust-webx rust-webx-core rust-webx-host rust-webx-engine; do
    if cargo tree -p "$package" -e normal --prefix none 2>/dev/null \
        | grep -Eq '^rust-ef( |$)'; then
        echo "error: $package depends on rust-ef; move the ORM adapter to an opt-in integration crate" >&2
        fail=1
    fi
done

if [ "$fail" -ne 0 ]; then
    exit 1
fi

echo "ok: rust-webx runtime crates have no rust-ef dependency"

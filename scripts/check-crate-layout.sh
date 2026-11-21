#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# Crate ownership gate for rust-webx (CI: Lint job).
#
# crates/ must not grow one top-level directory per vendor. The closed
# taxonomy and the dependency-direction rules live in
# scripts/check-crate-layout.py (single source; also runnable directly).
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail
cd "$(dirname "$0")/.."

PY=""
for candidate in python3 python; do
    # The Windows Store stub exits 1 without running Python; only accept an
    # interpreter that actually executes and has the stdlib TOML parser.
    if command -v "$candidate" >/dev/null 2>&1 && \
       "$candidate" -c "import sys, tomllib" >/dev/null 2>&1; then
        PY="$candidate"
        break
    fi
done
if [ -z "$PY" ]; then
    echo "check-crate-layout: python3 (3.11+) is required" >&2
    exit 1
fi

exec "$PY" scripts/check-crate-layout.py

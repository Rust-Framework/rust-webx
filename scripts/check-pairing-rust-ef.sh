#!/usr/bin/env bash
# -----------------------------------------------------------------------------
# Cross-repo pairing gate: rust-webx x local rust-ef checkout.
#
# Why: rust-ef is released first (2.9.1) and rust-webx 2.9.1 consumes it from
# crates.io. This script proves that the webx workspace and the
# rust-webx-idempotency-ef adapter compile -- and that the adapter's SQLite tests
# pass -- against a *local*
# rust-ef tree before that tree is published. It is intentionally not a
# substitute for CI: it does not check rust-ef's semver gate or the published
# crates.io artifacts, and it does not run Postgres/MySQL (see the real-database
# CI job). The evidence boundary is documented in docs/design/compatibility.md.
#
# Usage:
#   RUST_EF_PATH=/path/to/rust-ef bash scripts/check-pairing-rust-ef.sh
#   bash scripts/check-pairing-rust-ef.sh /path/to/rust-ef
#
# What it does:
#   1. resolves the `rust-ef` / `rust-ef-sqlite` manifests from the checkout;
#   2. writes a throwaway `[patch.crates-io]` config outside the repository and
#      reconciles Cargo.lock so the local tree is actually used;
#   3. runs `cargo check --workspace --locked` on the root workspace and
#      `cargo test --manifest-path examples/adapters/Cargo.toml -p rust-webx-idempotency-ef`
#      on the reference-adapter workspace (`rust-webx-idempotency-ef` lives
#      there; the adapters are unpublished);
#   4. always restores the original Cargo.lock on exit (trap), leaving the
#      working tree byte-identical.
#
# Exit codes: 0 = pairing verified, 2 = usage/precondition error, other = the
# underlying cargo command failed.
# -----------------------------------------------------------------------------
set -euo pipefail
cd "$(dirname "$0")/.."

usage() {
    awk 'NR > 2 && /^#/ { line = $0; sub(/^# ?/, "", line); print line; next } NR > 2 { exit }' "$0"
}

die() {
    echo "check-pairing-rust-ef: $*" >&2
    exit 2
}

case "${1:-}" in
    -h | --help)
        usage
        exit 0
        ;;
esac

RUST_EF_PATH="${1:-${RUST_EF_PATH:-}}"
[ -n "$RUST_EF_PATH" ] || die "set RUST_EF_PATH to the rust-ef repository root (or pass it as the first argument)"
command -v cargo >/dev/null 2>&1 || die "cargo is not on PATH"

# Git Bash hands over Windows-style paths (`D:\...`); convert for `cd` and later
# back to a TOML-friendly form via `cygpath -m`.
if command -v cygpath >/dev/null 2>&1; then
    RUST_EF_PATH="$(cygpath -u "$RUST_EF_PATH")"
fi
[ -d "$RUST_EF_PATH" ] || die "rust-ef checkout not found: $RUST_EF_PATH"
[ -f "$RUST_EF_PATH/Cargo.toml" ] || die "$RUST_EF_PATH has no Cargo.toml -- is it the rust-ef repository root?"

PY=""
for candidate in python3 python; do
    if command -v "$candidate" >/dev/null 2>&1 && "$candidate" -c "import sys, json, tomllib" >/dev/null 2>&1; then
        PY="$candidate"
        break
    fi
done
[ -n "$PY" ] || die "python3 (3.11+, for tomllib) is required to read cargo metadata and Cargo.lock"

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/rust-webx-pairing.XXXXXX")" || die "cannot create a temporary directory"
LOCK_BACKUP=""
ADAPTERS_LOCK_BACKUP=""
cleanup() {
    if [ -n "$LOCK_BACKUP" ] && [ -f "$LOCK_BACKUP" ]; then
        cp "$LOCK_BACKUP" Cargo.lock
        echo "check-pairing-rust-ef: restored the original Cargo.lock"
    elif [ -f Cargo.lock ]; then
        # The workspace had no lockfile before this run; do not leave the
        # reconciliation behind.
        rm -f Cargo.lock
    fi
    if [ -n "$ADAPTERS_LOCK_BACKUP" ] && [ -f "$ADAPTERS_LOCK_BACKUP" ]; then
        cp "$ADAPTERS_LOCK_BACKUP" examples/adapters/Cargo.lock
        echo "check-pairing-rust-ef: restored the original examples/adapters/Cargo.lock"
    fi
    rm -rf "$TMP_DIR"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

if [ -f Cargo.lock ]; then
    cp Cargo.lock "$TMP_DIR/Cargo.lock.original"
    LOCK_BACKUP="$TMP_DIR/Cargo.lock.original"
fi
# The reference adapters live in their own workspace (`examples/adapters/`), so
# its lockfile must be restored too when the local patch re-resolves it.
if [ -f examples/adapters/Cargo.lock ]; then
    cp examples/adapters/Cargo.lock "$TMP_DIR/adapters.lock.original"
    ADAPTERS_LOCK_BACKUP="$TMP_DIR/adapters.lock.original"
fi

WEBX_VERSION="$(sed -n -E 's/^version = "(.*)"/\1/p' Cargo.toml | head -1 | tr -d '\r')"
[ -n "$WEBX_VERSION" ] || die "cannot read the workspace version from rust-webx/Cargo.toml"

# -- 1. Locate the rust-ef manifests -----------------------------------------
if ! cargo metadata --manifest-path "$RUST_EF_PATH/Cargo.toml" --no-deps --format-version 1 \
    > "$TMP_DIR/ef-metadata.json" 2> "$TMP_DIR/ef-metadata.err"; then
    cat "$TMP_DIR/ef-metadata.err" >&2
    die "cargo metadata failed for $RUST_EF_PATH"
fi
if ! "$PY" - "$TMP_DIR/ef-metadata.json" > "$TMP_DIR/ef-paths.txt" <<'PY'; then
import json
import sys
from pathlib import Path

meta = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
packages = {package["name"]: package for package in meta.get("packages", [])}
missing = [name for name in ("rust-ef", "rust-ef-sqlite") if name not in packages]
if missing:
    sys.exit("missing workspace package(s): " + ", ".join(missing))
print(packages["rust-ef"]["version"])
print(Path(packages["rust-ef"]["manifest_path"]).parent)
print(Path(packages["rust-ef-sqlite"]["manifest_path"]).parent)
PY
    die "$RUST_EF_PATH does not look like the rust-ef workspace (need packages rust-ef and rust-ef-sqlite)"
fi
{
    IFS= read -r EF_VERSION
    IFS= read -r EF_CORE
    IFS= read -r EF_SQLITE
} < "$TMP_DIR/ef-paths.txt"
# Native Windows tooling ends redirected output with CRLF; strip the CR so the
# paths are usable by `cd`/TOML/grep on every platform.
EF_VERSION="${EF_VERSION%$'\r'}"
EF_CORE="${EF_CORE%$'\r'}"
EF_SQLITE="${EF_SQLITE%$'\r'}"
[ -n "$EF_VERSION" ] && [ -n "$EF_CORE" ] && [ -n "$EF_SQLITE" ] \
    || die "could not resolve rust-ef package paths from $RUST_EF_PATH"

# -- 2. Throwaway [patch.crates-io] config (never written into the repo) -----
toml_path() {
    local path="$1"
    if command -v cygpath >/dev/null 2>&1; then
        path="$(cygpath -m "$path")"
    fi
    printf '%s' "$path" | sed -e 's|\\|/|g' -e 's|"|\\"|g'
}
{
    echo "[patch.crates-io]"
    echo "rust-ef = { path = \"$(toml_path "$EF_CORE")\" }"
    echo "rust-ef-sqlite = { path = \"$(toml_path "$EF_SQLITE")\" }"
} > "$TMP_DIR/patch.toml"

PATCH_CONFIG="$TMP_DIR/patch.toml"
cat <<EOF
check-pairing-rust-ef: pairing rust-ef $EF_VERSION (local) with rust-webx $WEBX_VERSION
  rust-ef        -> $EF_CORE
  rust-ef-sqlite -> $EF_SQLITE
  patch config   : $PATCH_CONFIG (temporary; Cargo.lock is restored on exit)
EOF
if [ "$EF_VERSION" != "$WEBX_VERSION" ]; then
    echo "check-pairing-rust-ef: note: version skew (rust-ef $EF_VERSION vs rust-webx $WEBX_VERSION); releases are paired 1:1 (rust-ef 2.9.1 -> rust-webx 2.9.1)."
fi

# -- 3. Reconcile Cargo.lock, then require the patch to be in the graph ------
# Cargo.lock is shared by every cargo invocation in this working tree (other
# builds included). Snapshot the reconciled lock and restore it before each
# `--locked` step so a concurrent re-resolve cannot invalidate the run; the trap
# still restores the caller's original lock on exit.
PAIRED_LOCK="$TMP_DIR/Cargo.lock.paired"

# Succeeds when Cargo.lock resolves rust-ef to the local path version.
lock_is_paired() {
    "$PY" - "$1" "$EF_VERSION" <<'PY'
import sys
import tomllib
from pathlib import Path

lock = tomllib.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
for package in lock.get("package", []):
    if (
        package.get("name") == "rust-ef"
        and package.get("version") == sys.argv[2]
        and "source" not in package
    ):
        raise SystemExit(0)
raise SystemExit(1)
PY
}

seed_paired_lock() {
    local attempt
    for attempt in 1 2 3; do
        if cargo --config "$PATCH_CONFIG" update -p rust-ef -p rust-ef-sqlite \
            > "$TMP_DIR/update.log" 2>&1 && lock_is_paired Cargo.lock; then
            cp Cargo.lock "$PAIRED_LOCK"
            return 0
        fi
        sleep 1
    done
    return 1
}

# Run a cargo command that must see the reconciled lock. A concurrent cargo
# invocation can rewrite Cargo.lock between steps, so restore the snapshot and
# retry a few times before treating the failure as real.
run_locked() {
    local attempt
    for attempt in 1 2 3; do
        cp "$PAIRED_LOCK" Cargo.lock
        if "$@"; then
            return 0
        fi
        echo "check-pairing-rust-ef: cargo failed; Cargo.lock may have been re-resolved concurrently -- retrying (${attempt}/3)" >&2
        sleep 2
    done
    return 1
}

echo "check-pairing-rust-ef: [1/3] reconciling Cargo.lock with the local patch"
if ! seed_paired_lock; then
    if [ -f "$TMP_DIR/update.log" ]; then
        cat "$TMP_DIR/update.log" >&2
    fi
    die "cargo update failed or the [patch.crates-io] entries were not used; an offline machine must prime the registry cache first (run 'cargo fetch' once)"
fi

echo "check-pairing-rust-ef: [2/3] cargo check --workspace --locked"
if ! run_locked cargo --config "$PATCH_CONFIG" check --workspace --locked; then
    die "cargo check --workspace --locked failed (see the cargo output above)"
fi

# `rust-webx-idempotency-ef` lives in the reference-adapter workspace
# (`examples/adapters/Cargo.toml`), not in the root one; resolve the patch there.
if ! run_locked cargo --config "$PATCH_CONFIG" tree \
    --manifest-path examples/adapters/Cargo.toml -p rust-webx-idempotency-ef --depth 1 \
    > "$TMP_DIR/tree.txt" 2> "$TMP_DIR/tree.err"; then
    cat "$TMP_DIR/tree.err" >&2
    die "cargo tree failed for rust-webx-idempotency-ef"
fi
if grep -q "was not used in the crate graph" "$TMP_DIR/tree.err"; then
    cat "$TMP_DIR/tree.err" >&2
    die "the [patch.crates-io] entries were not used -- rust-webx is not actually built against the local rust-ef tree"
fi
if ! grep -Fq "rust-ef v$EF_VERSION (" "$TMP_DIR/tree.txt"; then
    cat "$TMP_DIR/tree.txt" >&2
    die "rust-webx-idempotency-ef does not resolve the local rust-ef@$EF_VERSION (patch not applied)"
fi

# -- 4. Adapter tests against the patched ORM -------------------------------
echo "check-pairing-rust-ef: [3/3] cargo test --manifest-path examples/adapters/Cargo.toml -p rust-webx-idempotency-ef"
if ! run_locked cargo --config "$PATCH_CONFIG" test --manifest-path examples/adapters/Cargo.toml -p rust-webx-idempotency-ef; then
    die "cargo test --manifest-path examples/adapters/Cargo.toml -p rust-webx-idempotency-ef failed (see the cargo output above)"
fi

echo "check-pairing-rust-ef: pairing verified -- rust-webx $WEBX_VERSION compiles against rust-ef $EF_VERSION and rust-webx-idempotency-ef tests pass."
echo "check-pairing-rust-ef: evidence boundary: local rust-ef source tree only; crates.io artifacts, rust-ef's semver gate and real Postgres/MySQL are covered by other CI jobs."

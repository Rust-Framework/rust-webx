#!/bin/sh
# ─────────────────────────────────────────────────────────────────
# lrwf crate publish script
#
# Publishes all LRWF crates to crates.io in dependency order.
#
# Usage:
#   ./publish.sh              # Dry-run: verify all crates are publishable
#   ./publish.sh --do         # Publish all crates
#   ./publish.sh --do --ver X # Bump version to X and publish
#   ./publish.sh --check      # Check only (cargo publish --dry-run)
#
# Order (must respect dependency graph):
#   1. lrwf-core    (no internal deps)
#   2. lrwf-macros  (no internal deps)
#   3. lrwf-web     (depends on lrwf-core)
#   4. lrwf-openapi (depends on lrwf-core)
#   5. lrwf-http    (depends on lrwf-core, lrwf-web, lrwf-openapi)
#   6. lrwf         (depends on all above, umbrella crate)
# ─────────────────────────────────────────────────────────────────

set -e

DO_PUBLISH=false
DRY_RUN=true
NEW_VERSION=""

while [ $# -gt 0 ]; do
    case "$1" in
        --do)    DO_PUBLISH=true; DRY_RUN=false ;;
        --check) DO_PUBLISH=false; DRY_RUN=true ;;
        --ver)   NEW_VERSION="$2"; shift ;;
        *)       echo "Unknown flag: $1"; exit 1 ;;
    esac
    shift
done

# ── Bump version if requested ──
bump_version() {
    if [ -n "$NEW_VERSION" ]; then
        echo "  → Bumping workspace version to $NEW_VERSION"
        sed -i "s/^version = \".*\"/version = \"$NEW_VERSION\"/" Cargo.toml
    fi
}

# ── Publish a single crate ──
publish_crate() {
    local crate="$1"
    local path="crates/$crate"
    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "  Publishing: $crate"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

    # Verify with dry-run
    cargo publish -p "$crate" --dry-run --allow-dirty 2>&1 | tail -5
    if [ $? -ne 0 ]; then
        echo "  ✗ Dry-run failed for $crate"
        exit 1
    fi

    if [ "$DRY_RUN" = true ]; then
        echo "  ✓ $crate — dry-run OK"
    else
        echo "  → Publishing $crate..."
        cargo publish -p "$crate"
        if [ $? -eq 0 ]; then
            echo "  ✓ $crate published successfully"
        else
            echo "  ✗ Failed to publish $crate"
            exit 1
        fi
        # Wait for crates.io index to update before publishing dependents
        sleep 5
    fi
}

# ── Main ──
echo ""
echo "┌─────────────────────────────────────────────────┐"
echo "│          LRWF — Crate Publish Script            │"
if [ "$DO_PUBLISH" = true ]; then
    echo "│          MODE: PUBLISH (live)                   │"
else
    echo "│          MODE: CHECK (dry-run only)             │"
fi
if [ -n "$NEW_VERSION" ]; then
    echo "│          VERSION: $NEW_VERSION                  │"
fi
echo "└─────────────────────────────────────────────────┘"

# Pre-check
echo ""
echo "▸ Verifying workspace..."
cargo check --workspace 2>&1 | tail -1

echo "▸ Running tests..."
cargo test -p lrwf-core -p lrwf-http --quiet 2>&1 | tail -1

bump_version

# Publish in dependency order
publish_crate "lrwf-core"
publish_crate "lrwf-macros"
publish_crate "lrwf-web"
publish_crate "lrwf-openapi"
publish_crate "lrwf-http"
publish_crate "lrwf"

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
if [ "$DO_PUBLISH" = true ]; then
    echo "  ✓ All crates published to crates.io"
else
    echo "  ✓ All crates verified (dry-run). Use --do to publish."
fi
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

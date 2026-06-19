#!/bin/sh
# ─────────────────────────────────────────────────────────────────
# rust-webapp crate publish script
#
# Publishes all rust-webapp crates to crates.io in dependency order.
#
# Usage:
#   ./publish.sh              # Dry-run: verify all crates are publishable
#   ./publish.sh --do         # Publish all crates
#   ./publish.sh --do --ver X # Bump version to X and publish
#   ./publish.sh --check      # Check only (cargo publish --dry-run)
#
# Order (must respect dependency graph):
#   1. rust-webapp-core     (no internal deps)
#   2. rust-webapp-macros   (no internal deps)
#   3. rust-webapp-spa      (depends on rust-webapp-core)
#   4. rust-webapp-openapi  (depends on rust-webapp-core)
#   5. rust-webapp-host     (depends on rust-webapp-core, rust-webapp-spa, rust-webapp-openapi)
#   6. rust-webapp          (depends on all above, umbrella crate)
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
    cargo publish -p "$crate" --dry-run --allow-dirty --registry crates-io 2>&1 | tail -5
    if [ $? -ne 0 ]; then
        echo "  ✗ Dry-run failed for $crate"
        exit 1
    fi

    if [ "$DRY_RUN" = true ]; then
        echo "  ✓ $crate — dry-run OK"
    else
        echo "  → Publishing $crate..."
        cargo publish -p "$crate" --registry crates-io
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
echo "│      rust-webapp — Crate Publish Script         │"
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
cargo test -p rust-webapp-core -p rust-webapp-host --quiet 2>&1 | tail -1

bump_version

# Publish in dependency order
publish_crate "rust-webapp-core"
publish_crate "rust-webapp-macros"
publish_crate "rust-webapp-spa"
publish_crate "rust-webapp-openapi"
publish_crate "rust-webapp-host"
publish_crate "rust-webapp"

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
if [ "$DO_PUBLISH" = true ]; then
    echo "  ✓ All crates published to crates.io"
else
    echo "  ✓ All crates verified (dry-run). Use --do to publish."
fi
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

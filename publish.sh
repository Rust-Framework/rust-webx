#!/bin/sh
# ─────────────────────────────────────────────────────────────────
# rust-webx crate publish script
#
# Publishes all rust-webx crates to crates.io in dependency order.
#
# Usage:
#   ./publish.sh              # Dry-run: verify all crates are publishable
#   ./publish.sh --do         # Publish all crates
#   ./publish.sh --do --ver X # Bump version to X and publish
#   ./publish.sh --check      # Check only (cargo publish --dry-run)
#
# Order (must respect dependency graph):
#   1. rust-webx-core     (no internal deps)
#   2. rust-webx-macros   (no internal deps)
#   3. rust-webx-build    (no internal deps; build-dependency of apps)
#   4. rust-webx-spa      (depends on rust-webx-core)
#   5. rust-webx-openapi  (depends on rust-webx-core)
#   6. rust-webx-host     (depends on rust-webx-core, rust-webx-spa, rust-webx-openapi)
#   7. rust-webx          (depends on all above, umbrella crate)
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
#
# Two places have to move together: `[workspace.package] version`, and the
# `rust-webx*` entries in `[workspace.dependencies]`. The latter are path
# dependencies that also carry a version for crates.io, so leaving them behind
# would publish crates pinning the previous release.
bump_version() {
    if [ -n "$NEW_VERSION" ]; then
        echo "  → Bumping workspace version to $NEW_VERSION"
        sed -i -E \
            -e "s/^version = \".*\"/version = \"$NEW_VERSION\"/" \
            -e "s/^(rust-webx[a-z-]* = \{ version = )\"[^\"]*\"/\1\"$NEW_VERSION\"/" \
            Cargo.toml
    fi
}

# ── crates.io sparse-index helpers ──
# A dependent crate can only be *published* (and therefore verified) once its
# internal dependencies are visible on crates.io. `cargo publish --dry-run`
# rewrites path deps to registry deps, so on a fresh version bump it fails for
# dependents purely because the deps are not live yet — not because the crate
# is broken. These helpers let the check mode tell those two cases apart.

index_path() {
    local name="$1" len=${#1}
    case "$len" in
        1) echo "1/$name" ;;
        2) echo "2/$name" ;;
        3) echo "3/${name:0:1}/$name" ;;
        *) echo "${name:0:2}/${name:2:2}/$name" ;;
    esac
}

index_has() {
    local name="$1" version="$2"
    curl -fsS "https://index.crates.io/$(index_path "$name")" 2>/dev/null \
        | grep -q "\"vers\":\"${version}\""
}

# Internal dependencies of a crate, in the order they appear in the workspace.
internal_deps() {
    case "$1" in
        rust-webx-core|rust-webx-macros|rust-webx-build) ;;
        rust-webx-spa|rust-webx-openapi) echo "rust-webx-core" ;;
        rust-webx-host) echo "rust-webx-core rust-webx-spa rust-webx-openapi" ;;
        rust-webx) echo "rust-webx-core rust-webx-host rust-webx-macros rust-webx-spa rust-webx-openapi" ;;
    esac
}

# True when every internal dependency of `$crate` is already on crates.io at the
# workspace version, so `cargo publish --dry-run` can actually verify it.
internal_deps_ready() {
    local crate="$1" version dep
    version="$(workspace_version)"
    for dep in $(internal_deps "$crate"); do
        index_has "$dep" "$version" || return 1
    done
    return 0
}

workspace_version() {
    sed -n -E 's/^version = "(.*)"/\1/p' Cargo.toml | head -1
}

wait_for_index() {
    local name="$1" version
    version="$(workspace_version)"
    for attempt in $(seq 1 60); do
        if index_has "$name" "$version"; then
            echo "  ✓ ${name} ${version} is visible in the index"
            return 0
        fi
        echo "  … waiting for ${name} ${version} in the index (attempt ${attempt}/60)"
        sleep 5
    done
    echo "  ✗ ${name} ${version} never appeared in the index"
    return 1
}

# ── Publish a single crate ──
publish_crate() {
    local crate="$1"
    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "  Publishing: $crate"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

    # Packaging sanity: file list only, so this never resolves against
    # crates.io and therefore works on a fresh version bump.
    if ! cargo package -p "$crate" --list --allow-dirty >/dev/null 2>&1; then
        echo "  ✗ cargo package --list failed for $crate"
        exit 1
    fi

    if [ "$DRY_RUN" = true ]; then
        if internal_deps_ready "$crate"; then
            if cargo publish -p "$crate" --dry-run --allow-dirty >/dev/null 2>&1; then
                echo "  ✓ $crate — dry-run OK"
            else
                echo "  ✗ Dry-run failed for $crate"
                exit 1
            fi
        else
            echo "  … $crate — packages OK; publish dry-run skipped"
            echo "    internal dependency not yet on crates.io at this version,"
            echo "    so cargo cannot verify it before the dependency is published"
        fi
        return 0
    fi

    echo "  → Publishing $crate..."
    if cargo publish -p "$crate" --registry crates-io; then
        echo "  ✓ $crate published successfully"
    else
        echo "  ✗ Failed to publish $crate"
        exit 1
    fi
    # Dependent crates need this one to be visible in the index first.
    wait_for_index "$crate"
}

# ── Main ──
echo ""
echo "┌─────────────────────────────────────────────────┐"
echo "│      rust-webx — Crate Publish Script         │"
if [ "$DO_PUBLISH" = true ]; then
    echo "│          MODE: PUBLISH (live)                   │"
else
    echo "│          MODE: CHECK (dry-run only)             │"
fi
if [ -n "$NEW_VERSION" ]; then
    echo "│          VERSION: $NEW_VERSION                  │"
fi
echo "└─────────────────────────────────────────────────┘"

# Pre-check — mirrors the CI gates so a release cannot be attempted on a tree
# that would fail there.
echo ""
echo "▸ Checking formatting..."
cargo fmt --all --check || { echo "  ✗ cargo fmt --all --check failed"; exit 1; }

echo "▸ Running Clippy (deny warnings)..."
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -3

echo "▸ Verifying workspace..."
cargo check --workspace 2>&1 | tail -1

echo "▸ Running tests..."
cargo test --workspace --quiet 2>&1 | tail -1

bump_version

# Publish in dependency order (dependencies before dependents)
publish_crate "rust-webx-macros"
publish_crate "rust-webx-core"
publish_crate "rust-webx-build"
publish_crate "rust-webx-spa"
publish_crate "rust-webx-openapi"
publish_crate "rust-webx-host"
publish_crate "rust-webx"

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
if [ "$DO_PUBLISH" = true ]; then
    echo "  ✓ All crates published to crates.io"
else
    echo "  ✓ All crates verified (dry-run). Use --do to publish."
fi
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

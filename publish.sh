#!/usr/bin/env bash
# =============================================================================
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
# Crates are published from the root workspace (`crates/**`) only. The `rust-ef`
# reference adapters live in a separate, unpublished workspace
# (`examples/adapters/`, every member `publish = false`): they are ecosystem
# reference implementations, not release artifacts, so nothing under
# `examples/` is packaged or published here. `publish.sh` and
# `.github/workflows/publish.yml` must list exactly the same crates in order;
# `scripts/check-publish-order.py` enforces the order and the coverage of the
# root workspace.
#
# Order (dependencies first):
#   1. rust-webx-macros   (no internal deps)
#   2. rust-webx-core     (no internal deps)
#   3. rust-webx-engine   (depends on rust-webx-core)
#   4. rust-webx-client   (depends on rust-webx-core; outbound HTTP)
#   5. rust-webx-ai       (AI domain kit; depends on core/client)
#   6. rust-webx-build    (no internal deps; build-dependency of apps)
#   7. rust-webx-spa      (depends on rust-webx-core)
#   8. rust-webx-openapi  (depends on rust-webx-core)
#   9. rust-webx-host     (depends on core/engine/client/macros/spa/openapi)
#  10. rust-webx-outbox   (outbox port/dispatcher/in-memory store; depends on core/host)
#  11. rust-webx-mfa      (TOTP/recovery codes; depends on core)
#  12. rust-webx-webauthn (passkey verification; depends on core)
#  13. rust-webx          (umbrella crate; depends on all framework crates above)
#  14. rust-webx-oauth2   (social login; depends on core/client; dev-dep on webx)
#  15. rust-webx-oidc     (OIDC client; depends on core/host/client)
#  16. rust-webx-redis    (event transport + distributed cache; depends on core/host)
#  17. rust-webx-nats     (event transport; depends on core/host)
#  18. rust-webx-rabbitmq (event transport; depends on core/host)
#  19. rust-webx-kafka    (event transport; depends on core/host)
#  20. rust-webx-ai-gateway (OpenAI-compatible surface; depends on webx + webx-ai)
#  21. rust-webx-pay      (payment ports/models; depends on core/host/client)
#  22. rust-webx-pay-stripe
#  23. rust-webx-pay-alipay
#  24. rust-webx-pay-wechatpay
#  25. rust-webx-pay-paypal
#  26. rust-webx-pay-adyen
#  27. rust-webx-pay-square
#  28. rust-webx-h3      (optional HTTP/3 transport; depends on engine/core; dev-dep on webx)
# =============================================================================

# `pipefail` is required: the gate commands below pipe into `tail`, and without
# it a failing `cargo clippy`/`test` would only be observed as `tail`'s status
# 0, so the release preflight would report success on a red tree.
set -euo pipefail

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

# -- crates.io sparse-index helpers -------------------------------------------
# A dependent crate can only be *published* (and therefore verified) once its
# internal dependencies are visible on crates.io. `cargo publish --dry-run`
# rewrites path deps to registry deps, so on a fresh version bump it fails for
# dependents purely because the deps are not live yet, not because the crate is
# broken. These helpers let the check mode tell those two cases apart.
# `index_path` / `index_has` / `workspace_version` / `wait_for_index` are shared
# with the release workflow (`scripts/crates-index.sh`) so the two paths cannot
# drift apart.

. "$(dirname "$0")/scripts/crates-index.sh"

# `workspace_version` comes from `scripts/crates-index.sh`. Fail before the
# (expensive) preflight when the root workspace version is unreadable, so a
# broken manifest cannot reach `cargo publish`.
check_workspace_version() {
    local root_version
    root_version="$(workspace_version)"
    if [ -z "$root_version" ]; then
        echo "  [FAIL] cannot read the workspace version from Cargo.toml"
        exit 1
    fi
    echo "  [OK] root workspace is at $root_version"
}

# -- Bump version if requested ------------------------------------------------
#
# Every place that carries the release version has to move together:
#   1. `[workspace.package] version` of the root workspace (`Cargo.toml`);
#   2. the internal `rust-webx*` entries of `[workspace.dependencies]`, which
#      carry a crates.io requirement next to the path (`=X.Y.Z`).
# The `rust-ef` reference adapters under `examples/adapters/` are unpublished:
# they pin the same version for the shared build, but they are not release
# artifacts and are deliberately left out of the bump.
# Dependency lines are anchored at the start of the line and on the `rust-webx`
# name prefix, so external requirements (`rust-dix = "1.0"`, `rust-ef = "2.6"`,
# `tokio`, ...) are never rewritten. `Cargo.lock` files are left alone: the
# normal cargo invocations that follow refresh them and they are committed with
# the release.
bump_version() {
    if [ -n "$NEW_VERSION" ]; then
        echo "  -> Bumping workspace version to $NEW_VERSION"
        sed -i -E \
            -e "s/^version = \".*\"/version = \"$NEW_VERSION\"/" \
            -e "s/^(rust-webx[a-z0-9-]* = \{ version = )\"=?[^\"]*\"/\1\"=$NEW_VERSION\"/" \
            Cargo.toml
    fi
}

# Internal dependencies of a crate, in the order they appear in the workspace.
internal_deps() {
    case "$1" in
        rust-webx-core|rust-webx-macros|rust-webx-build) ;;
        rust-webx-engine) echo "rust-webx-core" ;;
        rust-webx-client) echo "rust-webx-core" ;;
        rust-webx-ai) echo "rust-webx-core rust-webx-client" ;;
        rust-webx-ai-gateway) echo "rust-webx rust-webx-ai" ;;
        rust-webx-spa|rust-webx-openapi) echo "rust-webx-core" ;;
        rust-webx-host) echo "rust-webx-core rust-webx-engine rust-webx-client rust-webx-macros rust-webx-spa rust-webx-openapi" ;;
        rust-webx-outbox) echo "rust-webx-core rust-webx-host" ;;
        rust-webx-mfa|rust-webx-webauthn) echo "rust-webx-core" ;;
        rust-webx-oauth2) echo "rust-webx-core rust-webx-client rust-webx" ;;
        rust-webx-oidc) echo "rust-webx-core rust-webx-host rust-webx-client" ;;
        rust-webx-redis|rust-webx-nats|rust-webx-rabbitmq|rust-webx-kafka) echo "rust-webx-core rust-webx-host" ;;
        rust-webx-pay) echo "rust-webx-core rust-webx-host rust-webx-client" ;;
        rust-webx-pay-stripe|rust-webx-pay-alipay|rust-webx-pay-wechatpay|rust-webx-pay-paypal|rust-webx-pay-adyen|rust-webx-pay-square) echo "rust-webx-pay" ;;
        # `rust-webx` is a dev-dependency (the e2e tests drive a real host); it is
        # published before this crate, so the dry run can resolve it.
        rust-webx-h3) echo "rust-webx-core rust-webx-engine rust-webx" ;;
        rust-webx) echo "rust-webx-core rust-webx-engine rust-webx-host rust-webx-client rust-webx-macros rust-webx-spa rust-webx-openapi rust-webx-outbox" ;;
    esac
}

# True when every internal dependency of `$crate` is already on crates.io at the
# root workspace version, so `cargo publish --dry-run` can actually verify it.
internal_deps_ready() {
    local crate="$1" version dep
    version="$(workspace_version)"
    for dep in $(internal_deps "$crate"); do
        index_has "$dep" "$version" || return 1
    done
    return 0
}

# -- Publish a single crate ---------------------------------------------------
publish_crate() {
    local crate="$1" version
    version="$(workspace_version)"
    echo ""
    echo "================================================================================"
    echo "  Publishing: $crate"
    echo "================================================================================"

    # Resumable: a re-run after a partial failure must not fail on crates that
    # are already visible in the index (same contract as the tag workflow).
    if index_has "$crate" "$version"; then
        echo "  [SKIP] $crate $version is already published"
        return 0
    fi

    # Packaging sanity: file list only, so this never resolves against
    # crates.io and therefore works on a fresh version bump.
    if ! cargo package -p "$crate" --list --allow-dirty >/dev/null 2>&1; then
        echo "  [FAIL] cargo package --list failed for $crate"
        exit 1
    fi

    if [ "$DRY_RUN" = true ]; then
        if internal_deps_ready "$crate"; then
            if cargo publish -p "$crate" --dry-run --allow-dirty >/dev/null 2>&1; then
                echo "  [OK] $crate - dry-run OK"
            else
                echo "  [FAIL] Dry-run failed for $crate"
                exit 1
            fi
        else
            echo "  [SKIP] $crate - packages OK; publish dry-run skipped"
            echo "         internal dependency not yet on crates.io at $version,"
            echo "         so cargo cannot verify it before the dependency is published"
        fi
        return 0
    fi

    echo "  -> Publishing $crate..."
    if cargo publish -p "$crate" --registry crates-io; then
        echo "  [OK] $crate published successfully"
    else
        echo "  [FAIL] Failed to publish $crate"
        exit 1
    fi
    # Dependent crates need this one to be visible in the index first.
    wait_for_index "$crate" "$version"
}

# -- Main ---------------------------------------------------------------------
echo ""
echo "================================================================================"
echo "     rust-webx - Crate Publish Script"
if [ "$DO_PUBLISH" = true ]; then
    echo "         MODE: PUBLISH (live)"
else
    echo "         MODE: CHECK (dry-run only)"
fi
if [ -n "$NEW_VERSION" ]; then
    echo "         VERSION: $NEW_VERSION"
fi
echo "================================================================================"

# Fail fast: an unreadable workspace version would make every publish command
# address the wrong release.
check_workspace_version

# Pre-check: mirrors the CI gates so a release cannot be attempted on a tree
# that would fail there. Only the root workspace is published; the unpublished
# `rust-ef` reference adapters under `examples/adapters/` are compiled by CI
# (they pin the same version) but are not release artifacts.
echo ""
echo ">> Checking formatting..."
cargo fmt --all --check || { echo "  [FAIL] cargo fmt --all --check failed"; exit 1; }

echo ">> Running Clippy (deny warnings)..."
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -3

echo ">> Verifying workspace..."
cargo check --workspace 2>&1 | tail -1

echo ">> Running tests..."
cargo test --workspace --quiet 2>&1 | tail -1

bump_version
check_workspace_version

# Publish in dependency order (dependencies before dependents)
publish_crate "rust-webx-macros"
publish_crate "rust-webx-core"
publish_crate "rust-webx-engine"
publish_crate "rust-webx-client"
publish_crate "rust-webx-ai"
publish_crate "rust-webx-build"
publish_crate "rust-webx-spa"
publish_crate "rust-webx-openapi"
publish_crate "rust-webx-host"
publish_crate "rust-webx-outbox"
publish_crate "rust-webx-mfa"
publish_crate "rust-webx-webauthn"
publish_crate "rust-webx"
publish_crate "rust-webx-oauth2"
publish_crate "rust-webx-oidc"
publish_crate "rust-webx-redis"
publish_crate "rust-webx-nats"
publish_crate "rust-webx-rabbitmq"
publish_crate "rust-webx-kafka"
publish_crate "rust-webx-ai-gateway"
publish_crate "rust-webx-pay"
publish_crate "rust-webx-pay-stripe"
publish_crate "rust-webx-pay-alipay"
publish_crate "rust-webx-pay-wechatpay"
publish_crate "rust-webx-pay-paypal"
publish_crate "rust-webx-pay-adyen"
publish_crate "rust-webx-pay-square"
publish_crate "rust-webx-h3"

echo ""
echo "================================================================================"
if [ "$DO_PUBLISH" = true ]; then
    echo "  [OK] All crates published to crates.io"
else
    echo "  [OK] All crates verified (dry-run). Use --do to publish."
fi
echo "================================================================================"

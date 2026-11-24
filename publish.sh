#!/usr/bin/env bash
# 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€
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
#   1. rust-webx-macros   (no internal deps)
#   2. rust-webx-core     (no internal deps)
#   3. rust-webx-engine   (depends on rust-webx-core)
#   4. rust-webx-client   (depends on rust-webx-core; outbound HTTP)
#   5. rust-webx-ef       (EFError -> webx::Error boundary; depends on core)
#   6. rust-webx-ai       (AI domain kit; depends on core/client)
#   7. rust-webx-ai-ef    (rust-ef usage ledger)
#   8. rust-webx-build    (no internal deps; build-dependency of apps)
#   9. rust-webx-spa      (depends on rust-webx-core)
#  10. rust-webx-openapi  (depends on rust-webx-core)
#  11. rust-webx-host     (depends on core/engine/client/macros/spa/openapi)
#  12. rust-webx-auth-ef  (durable auth stores; depends on core)
#  13. rust-webx-mfa      (TOTP/recovery codes; depends on core)
#  14. rust-webx-webauthn (passkey verification; depends on core)
#  15. rust-webx          (umbrella crate; depends on all framework crates above)
#  16. rust-webx-outbox-ef (transactional outbox starter; depends on core/host)
#  17. rust-webx-oauth2   (social login; depends on core/client; dev-dep on webx)
#  18. rust-webx-oidc     (OIDC client; depends on core/host/client)
#  19. rust-webx-redis    (event transport + distributed cache; depends on core/host)
#  20. rust-webx-nats     (event transport; depends on core/host)
#  21. rust-webx-rabbitmq (event transport; depends on core/host)
#  22. rust-webx-kafka    (event transport; depends on core/host)
#  23. rust-webx-ai-gateway (OpenAI-compatible surface; depends on webx + webx-ai)
#  24. rust-webx-pay      (payment ports/models; depends on core/host/client)
#  25. rust-webx-pay-stripe
#  26. rust-webx-pay-alipay
#  27. rust-webx-pay-wechatpay
#  28. rust-webx-pay-paypal
#  29. rust-webx-pay-adyen
#  30. rust-webx-pay-square
#  31. rust-webx-pay-ef   (durable webhook inbox; depends on rust-webx-pay + rust-ef)
#  32. rust-webx-h3      (optional HTTP/3 transport; depends on engine/core; dev-dep on webx)
# 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€

# `pipefail` is required: the gate commands below pipe into `tail`, and without
# it a failing `cargo clippy`/`test` would only be observed as `tail`'s status
# 0 鈥?the release preflight would report success on a red tree.
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

# 鈹€鈹€ Bump version if requested 鈹€鈹€
#
# Two places have to move together: `[workspace.package] version`, and the
# `rust-webx*` entries in `[workspace.dependencies]`. The latter are path
# dependencies that also carry a version for crates.io, so leaving them behind
# would publish crates pinning the previous release.
bump_version() {
    if [ -n "$NEW_VERSION" ]; then
        echo "  鈫?Bumping workspace version to $NEW_VERSION"
        sed -i -E \
            -e "s/^version = \".*\"/version = \"$NEW_VERSION\"/" \
            -e "s/^(rust-webx[a-z-]* = \{ version = )\"[^\"]*\"/\1\"$NEW_VERSION\"/" \
            Cargo.toml
    fi
}

# 鈹€鈹€ crates.io sparse-index helpers 鈹€鈹€
# A dependent crate can only be *published* (and therefore verified) once its
# internal dependencies are visible on crates.io. `cargo publish --dry-run`
# rewrites path deps to registry deps, so on a fresh version bump it fails for
# dependents purely because the deps are not live yet 鈥?not because the crate
# is broken. These helpers let the check mode tell those two cases apart.
# `index_path` / `index_has` / `workspace_version` / `wait_for_index` are shared
# with the release workflow (`scripts/crates-index.sh`) so the two paths cannot
# drift apart.

. "$(dirname "$0")/scripts/crates-index.sh"

# Internal dependencies of a crate, in the order they appear in the workspace.
internal_deps() {
    case "$1" in
        rust-webx-core|rust-webx-macros|rust-webx-build) ;;
        rust-webx-engine) echo "rust-webx-core" ;;
        rust-webx-client) echo "rust-webx-core" ;;
        rust-webx-ef) echo "rust-webx-core" ;;
        rust-webx-ai) echo "rust-webx-core rust-webx-client" ;;
        rust-webx-ai-ef) echo "rust-webx-ai rust-webx-core" ;;
        rust-webx-ai-gateway) echo "rust-webx rust-webx-ai" ;;
        rust-webx-spa|rust-webx-openapi) echo "rust-webx-core" ;;
        rust-webx-host) echo "rust-webx-core rust-webx-engine rust-webx-client rust-webx-macros rust-webx-spa rust-webx-openapi" ;;
        rust-webx-auth-ef|rust-webx-mfa|rust-webx-webauthn) echo "rust-webx-core" ;;
        rust-webx-outbox-ef) echo "rust-webx-core rust-webx-host" ;;
        rust-webx-oauth2) echo "rust-webx-core rust-webx-client rust-webx" ;;
        rust-webx-oidc) echo "rust-webx-core rust-webx-host rust-webx-client" ;;
        rust-webx-redis|rust-webx-nats|rust-webx-rabbitmq|rust-webx-kafka) echo "rust-webx-core rust-webx-host" ;;
        rust-webx-pay) echo "rust-webx-core rust-webx-host rust-webx-client" ;;
        rust-webx-pay-stripe|rust-webx-pay-alipay|rust-webx-pay-wechatpay|rust-webx-pay-paypal|rust-webx-pay-adyen|rust-webx-pay-square|rust-webx-pay-ef) echo "rust-webx-pay" ;;
        # `rust-webx` is a dev-dependency (the e2e tests drive a real host); it is
        # published before this crate, so the dry run can resolve it.
        rust-webx-h3) echo "rust-webx-core rust-webx-engine rust-webx" ;;
        rust-webx) echo "rust-webx-core rust-webx-engine rust-webx-host rust-webx-client rust-webx-macros rust-webx-spa rust-webx-openapi" ;;
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

# 鈹€鈹€ Publish a single crate 鈹€鈹€
publish_crate() {
    local crate="$1"
    echo ""
    echo "鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹?
    echo "  Publishing: $crate"
    echo "鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹?

    # Resumable: a re-run after a partial failure must not fail on crates that
    # are already visible in the index (same contract as the tag workflow).
    if index_has "$crate" "$(workspace_version)"; then
        echo "  鈥?$crate $(workspace_version) is already published 鈥?skipping"
        return 0
    fi

    # Packaging sanity: file list only, so this never resolves against
    # crates.io and therefore works on a fresh version bump.
    if ! cargo package -p "$crate" --list --allow-dirty >/dev/null 2>&1; then
        echo "  鉁?cargo package --list failed for $crate"
        exit 1
    fi

    if [ "$DRY_RUN" = true ]; then
        if internal_deps_ready "$crate"; then
            if cargo publish -p "$crate" --dry-run --allow-dirty >/dev/null 2>&1; then
                echo "  鉁?$crate 鈥?dry-run OK"
            else
                echo "  鉁?Dry-run failed for $crate"
                exit 1
            fi
        else
            echo "  鈥?$crate 鈥?packages OK; publish dry-run skipped"
            echo "    internal dependency not yet on crates.io at this version,"
            echo "    so cargo cannot verify it before the dependency is published"
        fi
        return 0
    fi

    echo "  鈫?Publishing $crate..."
    if cargo publish -p "$crate" --registry crates-io; then
        echo "  鉁?$crate published successfully"
    else
        echo "  鉁?Failed to publish $crate"
        exit 1
    fi
    # Dependent crates need this one to be visible in the index first.
    wait_for_index "$crate"
}

# 鈹€鈹€ Main 鈹€鈹€
echo ""
echo "鈹屸攢鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹?
echo "鈹?     rust-webx 鈥?Crate Publish Script         鈹?
if [ "$DO_PUBLISH" = true ]; then
    echo "鈹?         MODE: PUBLISH (live)                   鈹?
else
    echo "鈹?         MODE: CHECK (dry-run only)             鈹?
fi
if [ -n "$NEW_VERSION" ]; then
    echo "鈹?         VERSION: $NEW_VERSION                  鈹?
fi
echo "鈹斺攢鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹?

# Pre-check 鈥?mirrors the CI gates so a release cannot be attempted on a tree
# that would fail there.
echo ""
echo "鈻?Checking formatting..."
cargo fmt --all --check || { echo "  鉁?cargo fmt --all --check failed"; exit 1; }

echo "鈻?Running Clippy (deny warnings)..."
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -3

echo "鈻?Verifying workspace..."
cargo check --workspace 2>&1 | tail -1

echo "鈻?Running tests..."
cargo test --workspace --quiet 2>&1 | tail -1

bump_version

# Publish in dependency order (dependencies before dependents)
publish_crate "rust-webx-macros"
publish_crate "rust-webx-core"
publish_crate "rust-webx-engine"
publish_crate "rust-webx-client"
publish_crate "rust-webx-ef"
publish_crate "rust-webx-ai"
publish_crate "rust-webx-ai-ef"
publish_crate "rust-webx-build"
publish_crate "rust-webx-spa"
publish_crate "rust-webx-openapi"
publish_crate "rust-webx-host"
publish_crate "rust-webx-auth-ef"
publish_crate "rust-webx-mfa"
publish_crate "rust-webx-webauthn"
publish_crate "rust-webx"
publish_crate "rust-webx-outbox-ef"
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
publish_crate "rust-webx-pay-ef"
publish_crate "rust-webx-h3"

echo ""
echo "鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹?
if [ "$DO_PUBLISH" = true ]; then
    echo "  鉁?All crates published to crates.io"
else
    echo "  鉁?All crates verified (dry-run). Use --do to publish."
fi
echo "鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹佲攣鈹?

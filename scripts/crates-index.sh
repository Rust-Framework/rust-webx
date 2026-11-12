#!/bin/sh
# crates.io sparse-index helpers, shared by `publish.sh` and the release workflow.
#
# Publication is asynchronous: a dependent crate can only be packaged once its
# internal dependencies are visible in the index, so callers poll with
# `wait_for_index` instead of sleeping a fixed amount.

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

# The workspace version from the root `Cargo.toml` (`[workspace.package]`).
workspace_version() {
    sed -n -E 's/^version = "(.*)"/\1/p' Cargo.toml | head -1
}

# Poll the index until `name@version` appears. `version` defaults to the
# workspace version so `publish.sh` can call it with the crate name only.
wait_for_index() {
    local name="$1" version="${2:-}"
    if [ -z "$version" ]; then
        version="$(workspace_version)"
    fi
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

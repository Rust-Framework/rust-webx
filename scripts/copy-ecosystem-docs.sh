#!/usr/bin/env bash
# Shared ecosystem docs copy logic for publish.sh and sync-docs.sh.
# Source this file; do not execute directly.

resolve_framework_root() {
    local workspace_root="$1"
    if [[ -n "${RUST_FRAMEWORK_ROOT:-}" && -d "$RUST_FRAMEWORK_ROOT" ]]; then
        (cd "$RUST_FRAMEWORK_ROOT" && pwd)
        return
    fi
    (cd "$workspace_root/.." && pwd)
}

copy_doc_tree() {
    local src="$1"
    local dest="$2"
    local label="$3"
    if [[ ! -d "$src" ]]; then
        echo "WARN: skip $label — source not found: $src" >&2
        return 0
    fi
    rm -rf "$dest"
    mkdir -p "$dest"
    cp -a "$src/." "$dest/"

    # serde_json rejects UTF-8 BOM; strip from INDEX.json if an editor saved one.
    local index_path="$dest/INDEX.json"
    if [[ -f "$index_path" ]] && [[ "$(head -c 3 "$index_path" | wc -c)" -eq 3 ]]; then
        local bom
        bom="$(od -An -tx1 -N3 "$index_path" | tr -d ' \n')"
        if [[ "$bom" == "efbbbf" ]]; then
            tail -c +4 "$index_path" > "$index_path.tmp"
            mv "$index_path.tmp" "$index_path"
            echo "Stripped UTF-8 BOM from $index_path"
        fi
    fi

    echo "Copied $label -> $dest"
}

# Usage: copy_ecosystem_docs <workspace_root> <docs_dest> [framework_root]
copy_ecosystem_docs() {
    local workspace_root="$1"
    local docs_dest="$2"
    local framework_root="${3:-}"

    workspace_root="$(cd "$workspace_root" && pwd)"
    if [[ -z "$framework_root" ]]; then
        framework_root="$(resolve_framework_root "$workspace_root")"
    else
        framework_root="$(cd "$framework_root" && pwd)"
    fi

    mkdir -p "$docs_dest"

    echo "Framework root: $framework_root"
    echo "Workspace root: $workspace_root"
    echo "Docs dest:      $docs_dest"

    copy_doc_tree "$framework_root/rust-dix/docs/rust-dix" "$docs_dest/rust-dix" "rust-dix"
    copy_doc_tree "$framework_root/rust-ef/docs/rust-ef" "$docs_dest/rust-ef" "rust-ef"
    copy_doc_tree "$framework_root/rust-agent-framework/docs" "$docs_dest/rust-agent-framework" "rust-agent-framework"
    copy_doc_tree "$framework_root/rust-gpui-rml/docs" "$docs_dest/rust-gpui-rml" "rust-gpui-rml"
    copy_doc_tree "$workspace_root/docs/rust-webx" "$docs_dest/rust-webx" "rust-webx"

    if [[ -f "$framework_root/rust-dix/assets/logo.svg" ]]; then
        mkdir -p "$docs_dest/rust-dix"
        cp -f "$framework_root/rust-dix/assets/logo.svg" "$docs_dest/rust-dix/logo.svg"
        echo "Copied logo -> $docs_dest/rust-dix/logo.svg"
    fi
    if [[ -f "$framework_root/rust-gpui-rml/demo/assets/logo.svg" ]]; then
        mkdir -p "$docs_dest/rust-gpui-rml"
        cp -f "$framework_root/rust-gpui-rml/demo/assets/logo.svg" "$docs_dest/rust-gpui-rml/logo.svg"
        echo "Copied logo -> $docs_dest/rust-gpui-rml/logo.svg"
    fi
}

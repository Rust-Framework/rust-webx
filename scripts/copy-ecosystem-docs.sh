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
    local src_full dest_full
    src_full="$(cd "$src" && pwd)"
    if [[ -d "$dest" ]]; then
        dest_full="$(cd "$dest" && pwd)"
        if [[ "$src_full" == "$dest_full" ]]; then
            echo "WARN: skip $label — refusing to copy a tree onto itself: $src_full" >&2
            return 0
        fi
    fi
    rm -rf "$dest"
    mkdir -p "$dest"
    cp -a "$src/." "$dest/"

    # serde_json / some parsers reject UTF-8 BOM; strip from all copied .json/.md.
    local stripped=0
    while IFS= read -r -d '' f; do
        if [[ "$(od -An -tx1 -N3 "$f" 2>/dev/null | tr -d ' \n')" == "efbbbf" ]]; then
            tail -c +4 "$f" > "$f.tmp"
            mv "$f.tmp" "$f"
            stripped=$((stripped + 1))
        fi
    done < <(find "$dest" -type f \( -name '*.json' -o -name '*.md' \) -print0 2>/dev/null)
    if [[ "$stripped" -gt 0 ]]; then
        echo "Stripped UTF-8 BOM from $stripped json/md file(s) under $dest"
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

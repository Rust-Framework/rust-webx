#!/usr/bin/env bash
# Copy ecosystem docs into rust-webx/docs/ for standalone deploy bundles.
#
# OPTIONAL — only needed when publishing a bundle without the full monorepo.
# During monorepo dev, DocService resolves sibling repo docs at runtime.
# Run before docbit/publish.sh for self-contained production layouts.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WEBX_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
FRAMEWORK_ROOT="$(cd "$WEBX_ROOT/.." && pwd)"
DOCS_DEST="$WEBX_ROOT/docs"

sync_tree() {
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
    echo "Synced $label -> $dest"
}

echo "Framework root: $FRAMEWORK_ROOT"
echo "Docs dest:      $DOCS_DEST"

sync_tree "$FRAMEWORK_ROOT/rust-dix/docs/rust-dix" "$DOCS_DEST/rust-dix" "rust-dix"
sync_tree "$FRAMEWORK_ROOT/rust-ef/docs/rust-ef" "$DOCS_DEST/rust-ef" "rust-ef"
sync_tree "$FRAMEWORK_ROOT/rust-agent-framework/docs" "$DOCS_DEST/rust-agent-framework" "rust-agent-framework"
sync_tree "$FRAMEWORK_ROOT/rust-gpui-rml/docs" "$DOCS_DEST/rust-gpui-rml" "rust-gpui-rml"

if [[ -d "$DOCS_DEST/rust-webx" ]]; then
    echo "rust-webx docs already present at $DOCS_DEST/rust-webx"
else
    echo "WARN: rust-webx docs missing at $DOCS_DEST/rust-webx" >&2
fi

echo "Done. Run: cargo run -p docbit-host"

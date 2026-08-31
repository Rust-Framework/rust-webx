#!/usr/bin/env bash
# Optional: stage ecosystem docs under rust-webx/docs/ for local preview.
# Production publish copies directly from source repos — see docbit/publish.sh.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WEBX_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
DOCS_DEST="${DOCS_DEST:-$WEBX_ROOT/docs}"

# shellcheck source=copy-ecosystem-docs.sh
source "$SCRIPT_DIR/copy-ecosystem-docs.sh"

copy_ecosystem_docs "$WEBX_ROOT" "$DOCS_DEST" "${RUST_FRAMEWORK_ROOT:-}"
echo "Done. Run: cargo run -p docbit-host"

#!/usr/bin/env bash
# Publish docbit to a target directory (Linux/macOS).
#
# Usage:
#   ./docbit/publish.sh /opt/docbit
#   ./docbit/publish.sh /opt/docbit --production
#   ./docbit/publish.sh /opt/docbit --skip-build --clean
#
set -euo pipefail

DEST=""
SKIP_BUILD=0
CLEAN=0
PRODUCTION=0
WORKSPACE_ROOT=""

usage() {
    sed -n '2,12p' "$0"
    exit 1
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --skip-build) SKIP_BUILD=1; shift ;;
        --clean) CLEAN=1; shift ;;
        --production) PRODUCTION=1; shift ;;
        --workspace-root) WORKSPACE_ROOT="$2"; shift 2 ;;
        -h|--help) usage ;;
        *)
            if [[ -z "$DEST" ]]; then
                DEST="$1"
            else
                echo "Unknown argument: $1" >&2
                usage
            fi
            shift
            ;;
    esac
done

[[ -n "$DEST" ]] || usage

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DOCBIT_DIR="$SCRIPT_DIR"
if [[ -z "$WORKSPACE_ROOT" ]]; then
    WORKSPACE_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
fi

EXE="$WORKSPACE_ROOT/target/release/docbit-host"
WWWROOT_SRC="$DOCBIT_DIR/wwwroot"
APPS_BASE="$DOCBIT_DIR/appsettings.json"
APPS_PROD="$DOCBIT_DIR/appsettings.Production.json"

[[ -d "$WWWROOT_SRC" ]] || { echo "wwwroot not found: $WWWROOT_SRC" >&2; exit 1; }
[[ -f "$APPS_BASE" ]] || { echo "appsettings.json not found" >&2; exit 1; }
[[ -f "$APPS_PROD" ]] || { echo "appsettings.Production.json not found" >&2; exit 1; }

if [[ "$CLEAN" -eq 1 && -d "$DEST" ]]; then
    echo "[Clean] removing $DEST"
    rm -rf "$DEST"
fi
mkdir -p "$DEST"
DEST="$(cd "$DEST" && pwd)"

echo "=== docbit publish ==="
echo "Destination: $DEST"

if [[ "$SKIP_BUILD" -eq 0 ]]; then
    echo "[1/6] cargo build --release -p docbit-host"
    (cd "$WORKSPACE_ROOT" && cargo build --release -p docbit-host)
else
    echo "[1/6] skip build"
fi

[[ -f "$EXE" ]] || { echo "binary not found: $EXE" >&2; exit 1; }

echo "[2/6] copy docbit-host"
cp "$EXE" "$DEST/docbit-host"
chmod +x "$DEST/docbit-host"

echo "[3/6] sync wwwroot/"
rm -rf "$DEST/wwwroot"
cp -a "$WWWROOT_SRC" "$DEST/wwwroot"

echo "[4/6] copy appsettings"
cp "$APPS_BASE" "$APPS_PROD" "$DEST/"

echo "[5/6] copy docs/ (from source repos)"
# shellcheck source=../scripts/copy-ecosystem-docs.sh
source "$WORKSPACE_ROOT/scripts/copy-ecosystem-docs.sh"
DOCS_DEST="$DEST/docs"
rm -rf "$DOCS_DEST"
copy_ecosystem_docs "$WORKSPACE_ROOT" "$DOCS_DEST"

if [[ "$PRODUCTION" -eq 1 ]]; then
    echo "[6/6] create run.sh"
    cat > "$DEST/run.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
export APP_ENV=Production
# export DATABASE_URL=mysql://user:password@host:3306/docbit
# export JWT_SECRET=your-strong-secret-min-32-chars
exec ./docbit-host
EOF
    chmod +x "$DEST/run.sh"
    echo "Created run.sh (set DATABASE_URL and JWT_SECRET before starting)"
else
    echo "[6/6] skip run.sh (-Production not set)"
fi

echo "=== done ==="
ls -la "$DEST"

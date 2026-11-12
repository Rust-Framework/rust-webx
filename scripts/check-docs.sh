#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────
# Documentation consistency gate for rust-webx.
#
# Fails when the shipped docs drift from the code/release:
#   1. a relative Markdown link in README/docs does not resolve;
#   2. docs/rust-webx/INDEX.json and the .md files disagree (both ways);
#   3. an API name removed in a previous release leaks into the live handbook;
#   4. the workspace version, docs INDEX.json version and CHANGELOG top entry
#      disagree.
#
# Run locally with `bash scripts/check-docs.sh`; CI runs it in the Lint job.
# Needs python3 (available on ubuntu-latest).
# ─────────────────────────────────────────────────────────────────
set -euo pipefail
cd "$(dirname "$0")/.."

PY=""
for candidate in python3 python; do
    # `command -v python3` can point at the Windows Store stub, which exits 1
    # without running the script; only accept interpreters that actually run.
    if command -v "$candidate" >/dev/null 2>&1 && "$candidate" -c "import sys" >/dev/null 2>&1; then
        PY="$candidate"
        break
    fi
done
if [ -z "$PY" ]; then
    echo "check-docs: python3 is required" >&2
    exit 1
fi

"$PY" - <<'PY'
import json
import re
import sys
from pathlib import Path

root = Path(".").resolve()
errors = []

# ── 1. Relative Markdown links resolve ─────────────────────────────
LINK = re.compile(r"\[[^\]]*\]\(\s*([^)\s]+)\s*\)")
SKIP_PREFIXES = ("http://", "https://", "mailto:", "#", "//")
md_files = sorted(set((root / "docs").rglob("*.md")))
md_files += [root / "README.md", root / "README.zh-CN.md"]
for md in md_files:
    if not md.exists():
        errors.append(f"missing Markdown file: {md.relative_to(root)}")
        continue
    text = md.read_text(encoding="utf-8")
    for match in LINK.finditer(text):
        target = match.group(1)
        if target.startswith(SKIP_PREFIXES):
            continue
        target = target.split("#", 1)[0]
        if not target:
            continue
        candidate = (root / target) if target.startswith("/") else (md.parent / target)
        if not candidate.exists():
            errors.append(
                f"{md.relative_to(root)}: broken link -> {match.group(1)}"
            )

# ── 2. INDEX.json <-> docs/rust-webx/*.md (bidirectional) ─────────
docs_root = root / "docs" / "rust-webx"
index_path = docs_root / "INDEX.json"
indexed = set()
try:
    index = json.loads(index_path.read_text(encoding="utf-8"))
except Exception as exc:  # noqa: BLE001 - report and fail
    errors.append(f"docs/rust-webx/INDEX.json is not valid JSON: {exc}")
    index = {"parts": []}
for part in index.get("parts", []):
    for chapter in part.get("chapters", []):
        for section in chapter.get("sections", []):
            rel = f"{chapter['id']}/{section['id']}.md"
            indexed.add(rel)
            if not (docs_root / rel).is_file():
                errors.append(f"INDEX.json references a missing file: docs/rust-webx/{rel}")
actual = {
    str(path.relative_to(docs_root)).replace("\\", "/")
    for path in docs_root.rglob("*.md")
    if path.name not in {"INDEX.md", "FOREWORD.md"}
}
for rel in sorted(actual - indexed):
    errors.append(f"docs/rust-webx/{rel} is not listed in INDEX.json")

# ── 3. Removed API names must not leak into the live handbook ─────
# 16-migration/upgrade-to-*.md documents the removed names on purpose, and
# CHANGELOG keeps the history; only the "current" handbook must stay clean.
STALE = ("ResponseData", "PagedResponse", "file_seekable_stream", "file_stream")
for md in sorted((docs_root).rglob("*.md")):
    if "16-migration" in md.parts:
        continue
    text = md.read_text(encoding="utf-8")
    for token in STALE:
        if token in text:
            errors.append(
                f"stale API name `{token}` in {md.relative_to(root)}"
                " (removed APIs belong in 16-migration only)"
            )

# ── 4. Version agreement: Cargo.toml == INDEX.json == CHANGELOG ───
version = None
in_workspace_package = False
for line in (root / "Cargo.toml").read_text(encoding="utf-8").splitlines():
    if line.strip() == "[workspace.package]":
        in_workspace_package = True
        continue
    if line.startswith("["):
        in_workspace_package = False
    if in_workspace_package:
        match = re.match(r'\s*version\s*=\s*"([^"]+)"', line)
        if match:
            version = match.group(1)
            break
if version is None:
    errors.append("cannot read [workspace.package] version from Cargo.toml")
else:
    docs_version = index.get("meta", {}).get("version")
    if docs_version != version:
        errors.append(
            f"docs INDEX.json version is {docs_version}, workspace version is {version}"
        )
    changelog = (root / "CHANGELOG.md").read_text(encoding="utf-8")
    heading = re.search(r"^## \[([^\]]+)\]", changelog, re.MULTILINE)
    if not heading:
        errors.append("CHANGELOG.md has no `## [version]` heading")
    elif heading.group(1) != version:
        errors.append(
            f"CHANGELOG.md top entry is {heading.group(1)}, workspace version is {version}"
        )

if errors:
    print("check-docs: documentation consistency failed")
    for error in errors:
        print(f"  ✗ {error}")
    sys.exit(1)

print(
    f"check-docs: ok ({len(md_files)} Markdown files, "
    f"{len(indexed)} indexed handbook sections, version {version})"
)
PY

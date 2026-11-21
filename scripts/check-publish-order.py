#!/usr/bin/env python3
"""Publish-order gate for rust-webx.

Two release paths publish the workspace (`publish.sh` and the tag-triggered
`publish.yml`). They duplicate the order on purpose (a workflow must not depend
on a shell function), but they must not drift or silently drop a crate: a
missing crate means the tag run stays green while nothing is published.

This script checks, for **every publishable workspace crate**:

1. exactly one entry in `publish.sh` (`publish_crate "<name>"`);
2. exactly one entry in `publish.yml` (`CRATES="..."`);
3. the two lists are identical;
4. internal dependencies come earlier in the order (dependencies first).

Development-only crates (`publish = false`) are ignored. Run it from CI Lint
and from `publish.sh` before a release.
"""

import re
import sys
import tomllib
from pathlib import Path


def load(path: Path):
    with path.open("rb") as handle:
        return tomllib.load(handle)


def internal_deps(name, manifests):
    """Workspace dependencies of `name` (path or exact-pinned workspace deps)."""
    data = manifests[name]
    deps = set()
    for section in ("dependencies", "build-dependencies"):
        for dep, spec in data.get(section, {}).items():
            if dep in manifests and dep != name:
                deps.add(dep)
            elif isinstance(spec, dict) and spec.get("workspace") and dep in manifests:
                deps.add(dep)
    return deps


def main() -> int:
    root = Path(__file__).resolve().parent.parent
    workspace = load(root / "Cargo.toml")
    members = workspace["workspace"]["members"]

    manifests = {}
    publishable = set()
    for member in members:
        manifest = root / member / "Cargo.toml"
        if not manifest.is_file():
            continue
        data = load(manifest)
        name = data["package"]["name"]
        manifests[name] = data
        if data["package"].get("publish", True) is not False:
            publishable.add(name)

    script = (root / "publish.sh").read_text(encoding="utf-8")
    script_order = re.findall(r'publish_crate "([^"]+)"', script)

    workflow = (root / ".github" / "workflows" / "publish.yml").read_text(encoding="utf-8")
    match = re.search(r'^(\s*)CRATES="([^"]+)"', workflow, re.MULTILINE)
    workflow_order = match.group(2).split() if match else []

    errors = []

    for label, order in (("publish.sh", script_order), ("publish.yml", workflow_order)):
        if not order:
            errors.append(f"{label}: no publish list found")
            continue
        duplicates = {name for name in order if order.count(name) > 1}
        if duplicates:
            errors.append(f"{label}: duplicated crate(s) {sorted(duplicates)}")
        missing = sorted(publishable - set(order))
        extra = sorted(set(order) - publishable)
        if missing:
            errors.append(f"{label}: missing publishable crate(s) {missing}")
        if extra:
            errors.append(f"{label}: unknown or unpublished crate(s) {extra}")

    if script_order and workflow_order and script_order != workflow_order:
        errors.append("publish.sh and publish.yml publish orders differ")

    # Dependency order: a crate may only appear after every internal dependency.
    seen = set()
    for name in script_order:
        for dep in sorted(internal_deps(name, manifests)):
            if dep in publishable and dep not in seen:
                errors.append(f"{name} is published before its dependency {dep}")
        seen.add(name)

    if errors:
        print("publish order check failed:", file=sys.stderr)
        for error in errors:
            print(f"  - {error}", file=sys.stderr)
        return 1

    print(f"publish order OK: {len(script_order)} crates, dependencies first")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

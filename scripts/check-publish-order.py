#!/usr/bin/env python3
"""Publish-order gate for rust-webx.

The project publishes from **one workspace**: the root workspace (`crates/**`).
The `rust-ef` reference adapters (`examples/adapters/`: ai-ef, auth-ef,
idempotency-ef, outbox-ef, pay-ef) are ecosystem reference implementations, not
release artifacts: every member sets `publish = false`, so nothing under
`examples/` is published by `publish.sh` or the tag-triggered `publish.yml`.

Those two release paths duplicate the order on purpose (a workflow must not
depend on a shell function), but they must not drift or silently drop a crate:
a missing crate means the tag run stays green while nothing is published.

This script checks, over the member manifests of the published workspace:

1. exactly one entry in `publish.sh` (`publish_crate "<name>"`);
2. exactly one entry in `publish.yml` (`CRATES="..."`);
3. the two lists are identical and cover every publishable crate;
4. internal dependencies come earlier in the order (dependencies first).

Crates that are not published (`publish = false` in their own manifest, or
inherited through `[workspace.package]`) are ignored: they are neither
required in the lists nor allowed to appear in them. Dependency names without
a published workspace manifest (`rust-ef`, `rust-dix`, ...) are external
crates from crates.io and impose no ordering here, so a missing manifest is
tolerated.

Run it from CI (the `Check publish order` step) and before any manual release.
Exit code: 0 = order is consistent, 1 = at least one violation.
"""

import re
import sys
import tomllib
from pathlib import Path

# Only the published root workspace is checked; paths are relative to the
# repository root. `examples/adapters/` is unpublished (`publish = false`) and
# therefore never part of the release lists.
WORKSPACES = ("Cargo.toml",)
DEPENDENCY_SECTIONS = ("dependencies", "build-dependencies", "dev-dependencies")


def load(path: Path):
    with path.open("rb") as handle:
        return tomllib.load(handle)


def is_publishable(package, workspace_package):
    """`publish` may be `false`, a registry list, or inherited from the workspace."""
    publish = package.get("publish", True)
    if isinstance(publish, dict):
        publish = workspace_package.get("publish", True) if publish.get("workspace") else True
    # `publish = false` and `publish = []` both mean "never publish this crate".
    return publish is not False and publish != []


def collect(root, workspace_manifest, manifests, publishable, errors):
    """Load every member manifest of one workspace into the shared tables."""
    workspace = load(root / workspace_manifest)["workspace"]
    base = (root / workspace_manifest).parent
    workspace_package = workspace.get("package", {})
    for member in workspace["members"]:
        manifest = base / member / "Cargo.toml"
        if not manifest.is_file():
            errors.append(f"{workspace_manifest}: member {member} has no Cargo.toml")
            continue
        data = load(manifest)
        name = data["package"]["name"]
        if name in manifests:
            errors.append(f"{workspace_manifest}: package {name} is already declared in another workspace")
            continue
        manifests[name] = data
        if is_publishable(data["package"], workspace_package):
            publishable.add(name)


def internal_deps(name, manifests):
    """Published workspace crates that `name` depends on.

    Dependencies without a manifest in the published workspace are external
    crates.io packages (`rust-ef`, `rust-dix`, ...): they are not an ordering
    constraint here, so they are skipped instead of failing the lookup.
    """
    deps = set()
    for section in DEPENDENCY_SECTIONS:
        for dep in manifests[name].get(section, {}):
            if dep != name and dep in manifests:
                deps.add(dep)
    return deps


def main() -> int:
    root = Path(__file__).resolve().parent.parent
    manifests = {}
    publishable = set()
    errors = []

    for workspace_manifest in WORKSPACES:
        collect(root, workspace_manifest, manifests, publishable, errors)

    script = (root / "publish.sh").read_text(encoding="utf-8")
    script_order = re.findall(r'publish_crate "([^"]+)"', script)

    workflow = (root / ".github" / "workflows" / "publish.yml").read_text(encoding="utf-8")
    match = re.search(r'^(\s*)CRATES="([^"]+)"', workflow, re.MULTILINE)
    workflow_order = match.group(2).split() if match else []

    for label, order in (("publish.sh", script_order), ("publish.yml", workflow_order)):
        if not order:
            errors.append(f"{label}: no publish list found")
            continue
        duplicates = sorted({name for name in order if order.count(name) > 1})
        if duplicates:
            errors.append(f"{label}: duplicated crate(s) {duplicates}")
        missing = sorted(publishable - set(order))
        extra = sorted(set(order) - publishable)
        if missing:
            errors.append(f"{label}: missing publishable crate(s) {missing}")
        if extra:
            errors.append(f"{label}: unknown or unpublished crate(s) {extra}")

    if script_order and workflow_order and script_order != workflow_order:
        errors.append("publish.sh and publish.yml publish orders differ")

    # Dependency order: a crate may only appear after every internal dependency.
    # The `name in manifests` guard keeps a stale list entry (a crate that moved
    # out of the published workspace) reportable as "unknown" instead of
    # crashing here.
    seen = set()
    for name in script_order:
        if name in manifests:
            for dep in sorted(internal_deps(name, manifests)):
                if dep in publishable and dep not in seen:
                    errors.append(f"{name} is published before its dependency {dep}")
        seen.add(name)

    if errors:
        print("publish order check failed:", file=sys.stderr)
        for error in errors:
            print(f"  - {error}", file=sys.stderr)
        return 1

    workspace_word = "workspace" if len(WORKSPACES) == 1 else "workspaces"
    print(
        f"publish order OK: {len(script_order)} crates from "
        f"{len(WORKSPACES)} {workspace_word}, dependencies first"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

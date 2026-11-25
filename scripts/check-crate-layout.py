#!/usr/bin/env python3
"""Crate ownership gate for rust-webx.

crates/ must not grow one top-level directory per vendor. Ownership is a closed
taxonomy:

    crates/<framework>      fixed framework set (core, engine, host, ..., outbox)
    crates/ai/<leaf>        AI domain (core/ef/gateway)
    crates/auth/<leaf>      authentication (ef/oidc/oauth2/mfa/webauthn)
    crates/cache/<leaf>     third-party cache adapters (redis)
    crates/queue/<leaf>     third-party message-queue adapters (nats/rabbitmq/kafka)
    crates/payment/<leaf>   payment domain (core/stripe/alipay/wechatpay/paypal/adyen/square/ef)
    crates/ef/<leaf>        ORM boundary adapters (webx)
    crates/transport/<leaf> optional non-hyper listener backends (h3)
    examples/<app>/<crate>  non-published reference apps (docbit/dmbit/imbit); each
                            app is its own cargo workspace with its own lockfile
    testing/<leaf>          non-published test/dev harnesses (alloc-probe,
                            contract-tests, upgrade-smoke)

Rules enforced here (the dependency direction is the constitution's P10
"small core, large ecosystem"):

1. every workspace member under crates/ matches the taxonomy;
2. every crate directory under crates/ is a workspace member (no orphans);
3. framework crates never depend on a domain-group crate;
4. the `rust-webx` umbrella never links ai/auth/cache/queue adapters;
5. `crates/` is the published surface: every member there must be publishable
   (`publish != false`), and every non-published package must live outside it
   (reference apps under `examples/` or `testing/`);
6. `testing/` packages are never published.

Run locally with `python3 scripts/check-crate-layout.py`; CI runs it in Lint
through `scripts/check-crate-layout.sh`.
"""

import sys
import tomllib
from pathlib import Path

# Framework crates live directly under crates/. This list may only shrink by a
# deliberate layout change; new capabilities go into a domain group.
FRAMEWORK = {
    "core",
    "engine",
    "host",
    "macros",
    "spa",
    "openapi",
    "client",
    "webx",
    "build",
    # Outbox contract/dispatcher/in-memory store: framework-side, ORM-free. The
    # durable store and producer enqueue helper live in
    # `examples/adapters/outbox/ef` (the unpublished `rust-webx-outbox-ef`
    # reference adapter, outside this workspace).
    "outbox",
}
GROUPS = {"ai", "auth", "cache", "queue", "payment", "ef", "transport"}
# Non-published test/dev harnesses: allowed as workspace members, never in
# `crates/` (the published surface).
TESTING = {"testing"}
# Non-published reference applications. Each app lives in its own cargo
# workspace under `examples/` (own lockfile), so `examples/<app>` and
# `examples/<app>/<crate>` are the accepted member paths; the bare `<app>` form
# is still recognised for the pre-`examples/` layout.
REFERENCE_APPS = {"docbit", "dmbit", "imbit"}
REFERENCE_APPS_DIR = "examples"


def is_reference_app(parts: tuple) -> bool:
    """`examples/<app>`, `examples/<app>/<crate>`, or legacy bare `<app>`."""
    if len(parts) == 1:
        return parts[0] in REFERENCE_APPS
    if len(parts) in (2, 3) and parts[0] == REFERENCE_APPS_DIR:
        return parts[1] in REFERENCE_APPS
    return False


def load(path: Path):
    with path.open("rb") as handle:
        return tomllib.load(handle)


def main() -> int:
    root = Path(__file__).resolve().parent.parent
    errors = []
    group_leaves = {group: set() for group in GROUPS}

    workspace = load(root / "Cargo.toml")
    members = workspace["workspace"]["members"]
    member_set = {Path(member).as_posix() for member in members}

    # Validate the self-pinned internal dependency paths: a wrong path would
    # only surface as a confusing resolver error after someone adds a crate.
    internal = workspace["workspace"]["dependencies"]
    for name, spec in internal.items():
        if not isinstance(spec, dict) or "path" not in spec:
            continue
        path = Path(spec["path"]).as_posix()
        # Sibling RF workspaces (rust-dix/rust-ef) are deliberately outside this
        # repository; the runtime crates must not depend on them, but the shared
        # contract paths are pinned by the workspace itself.
        if path.startswith("../"):
            continue
        if path not in member_set:
            errors.append(
                f"workspace.dependencies.{name} points at '{path}', "
                "which is not a workspace member"
            )

    for member in members:
        path = Path(member)
        parts = path.parts
        if parts[0] == "crates":
            if len(parts) == 2:
                leaf = parts[1]
                if leaf not in FRAMEWORK:
                    errors.append(
                        f"{member}: crates/<name> is reserved for the framework "
                        "set; put the crate in crates/{ai,auth,cache,queue,payment}/<leaf>"
                    )
            elif len(parts) == 3:
                group, leaf = parts[1], parts[2]
                if group not in GROUPS:
                    errors.append(
                        f"{member}: unknown crate group '{group}' "
                        f"(allowed: {sorted(GROUPS)})"
                    )
                else:
                    group_leaves[group].add(leaf)
            else:
                errors.append(
                    f"{member}: crates/ allows exactly one framework level or "
                    "crates/<group>/<leaf>"
                )
        elif parts[0] in TESTING:
            if len(parts) != 2:
                errors.append(
                    f"{member}: testing/ allows exactly testing/<leaf>"
                )
        elif is_reference_app(parts):
            # examples/<app> or examples/<app>/<crate>; each app is its own
            # cargo workspace, so a member here only appears if the root
            # workspace deliberately pulls one in.
            pass
        else:
            errors.append(f"{member}: unexpected workspace member root '{parts[0]}'")
        if not (root / member / "Cargo.toml").is_file():
            errors.append(f"{member}: member directory has no Cargo.toml")

    # Orphans: a crate manifest on disk that the workspace does not own.
    # `crates/` is scanned recursively because groups add a level, but only
    # workspace members may appear there. `testing/` is scanned one level deep:
    # harnesses such as `testing/upgrade-smoke` are standalone cargo workspaces
    # (their nested `stable/`/`current/` manifests are intentionally not
    # members), while `testing/<leaf>` packages must be members.
    for manifest in sorted((root / "crates").rglob("Cargo.toml")):
        rel = manifest.parent.relative_to(root).as_posix()
        if rel not in member_set:
            errors.append(f"{rel}: crate directory is not a workspace member")
    for entry in sorted((root / "testing").iterdir()):
        if not entry.is_dir() or not (entry / "Cargo.toml").is_file():
            continue
        rel = entry.relative_to(root).as_posix()
        if rel not in member_set:
            errors.append(f"{rel}: crate directory is not a workspace member")

    # Top-level crate directories must belong to the taxonomy (a stray
    # directory with no Cargo.toml still signals ownership drift).
    for entry in sorted((root / "crates").iterdir()):
        if not entry.is_dir():
            continue
        if entry.name in FRAMEWORK or entry.name in GROUPS:
            continue
        errors.append(
            f"crates/{entry.name}: not an allowed top-level owner "
            f"(framework: {sorted(FRAMEWORK)}; groups: {sorted(GROUPS)})"
        )

    # Build package-name -> manifest for the dependency-direction checks.
    manifests = {}
    for member in members:
        manifest = root / member / "Cargo.toml"
        if manifest.is_file():
            data = load(manifest)
            manifests[data["package"]["name"]] = (member, data)

    group_packages = set()
    for name, (member, _data) in manifests.items():
        parts = Path(member).parts
        if len(parts) == 3 and parts[0] == "crates" and parts[1] in GROUPS:
            group_packages.add(name)

    framework_packages = {
        name
        for name, (member, _data) in manifests.items()
        if len(Path(member).parts) == 2 and Path(member).parts[0] == "crates"
    }

    testing_packages = {
        name
        for name, (member, _data) in manifests.items()
        if Path(member).parts[0] in TESTING
    }

    # `crates/` is the published surface. A `publish = false` package there is
    # release pollution: it would be shipped in the repository's framework
    # layout but can never appear on crates.io (and publish.sh ignores it).
    for name, (member, data) in sorted(manifests.items()):
        publishable = data["package"].get("publish", True) is not False
        root_segment = Path(member).parts[0]
        if root_segment == "crates" and not publishable:
            errors.append(
                f"{name} ({member}): non-published crate under crates/; "
                "crates/ is the published surface — move it to testing/"
            )
        if root_segment in TESTING and publishable:
            errors.append(
                f"{name} ({member}): testing/ packages must set publish = false"
            )

    if not group_packages:
        errors.append(
            "no domain-group crates discovered; the taxonomy check is not effective"
        )

    # P10: the framework core must not depend on a domain group, otherwise the
    # "opt-in starter" promise is gone (the umbrella would link broker SDKs).
    for name in sorted(framework_packages):
        member, data = manifests[name]
        leaked = sorted(set(data.get("dependencies", {})) & group_packages)
        if leaked:
            errors.append(
                f"{name} ({member}) depends on domain crate(s) {leaked}; "
                "framework core must stay free of ai/auth/cache/queue/payment"
            )

    # The umbrella is the one crate users opt into; it must not pull adapters.
    umbrella = manifests.get("rust-webx")
    if umbrella is None:
        errors.append("the rust-webx umbrella crate is missing")
    else:
        member, data = umbrella
        leaked = sorted(set(data.get("dependencies", {})) & group_packages)
        if leaked:
            errors.append(
                f"rust-webx ({member}) links adapter crate(s) {leaked}; "
                "adapters must stay opt-in"
            )

    if errors:
        print("crate layout check failed:", file=sys.stderr)
        for error in errors:
            print(f"  - {error}", file=sys.stderr)
        return 1

    summary = " ".join(
        f"{group}={sorted(leaves)}" for group, leaves in group_leaves.items()
    )
    print(
        f"crate layout OK: framework={len(framework_packages)} published crates, "
        f"testing={len(testing_packages)} harnesses, {summary}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

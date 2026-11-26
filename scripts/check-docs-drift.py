#!/usr/bin/env python3
"""Semantic documentation drift gate for rust-webx.

This complements scripts/check-docs.sh (links, INDEX, version agreement) with
the facts that cannot be inferred from links alone:

1. the generated crate inventory is current and covers every published crate;
2. live docs do not mention crates that are not published (historical pages are
   explicitly allowlisted);
3. removed/renamed APIs listed in docs/design/deprecations.md do not leak into
   the current handbook;
4. contract types owned by rust-core are not attributed to webx-core;
5. "current version" claims do not point at older releases;
6. generated/manual duplicate inventories and the authority map stay in place.

Run from the repository root: python scripts/check-docs-drift.py
"""

from __future__ import annotations

import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
HANDBOOK = ROOT / "docs" / "rust-webx"
DESIGN = ROOT / "docs" / "design"
README = [ROOT / "README.md", ROOT / "README.zh-CN.md"]

# Files that intentionally document history/removals. They are allowed to
# mention removed crates/APIs; every other live page must stay clean.
HISTORICAL_PATHS = (
    "docs/rust-webx/16-migration/",
    "docs/rust-webx/CHANGELOG.md",
    "docs/design/deprecations.md",
    "docs/design/compatibility.md",
    "docs/design/upstream-gaps.md",
    "docs/design/api-convergence.md",
    "docs/design/pagination-model.md",
    "docs/design/im-hardening.md",
    "docs/design/release-notes-2.5.0.md",
)
ALLOW_STALE_MARKER = "<!-- docs:allow-stale-api -->"

# Removed/renamed APIs that predate docs/design/deprecations.md or are recorded
# there only as a crate-level note. Keep this list small; the authority is the
# deprecations table, and new removals belong there first.
BASE_STALE_TOKENS = {
    "ResponseData",
    "PagedResponse",
    "file_seekable_stream",
    "file_stream",
    "map_ef_error",
    "EfResultExt",
    "add_health_check_async",
    "add_health_check_with_timeout",
    "PagedRequest::from_parts",
    "PagedResult::from_request",
    # 2.9.1 replaced the Error enum with rust_core::AppError constructors.
    "Error::Http",
    "Error::Validation",
    "Error::Serialization",
    "Error::NotFound",
    "Error::Routing",
    "Error::Conflict",
    "Error::Di",
    "Error::Internal",
    "Error::Message",
    "Error::State",
    "Error::ResponseSerialization",
    "Error::Status",
    # 2.9.x metrics use Prometheus snake_case names.
    "http.requests",
}

# Removed crates (publish = false / deleted) that may only appear in history.
REMOVED_CRATES = {"rust-webx-ef", "rust-webx-stack"}
# Valid names that are not workspace-published crates: reference adapters,
# test harnesses, planned adapters, and family names used as prose.
REFERENCE_ADAPTER_CRATES = {
    "rust-webx-ai-ef",
    "rust-webx-auth-ef",
    "rust-webx-idempotency-ef",
    "rust-webx-outbox-ef",
    "rust-webx-pay-ef",
}
TEST_CRATES = {"rust-webx-contract-tests"}
PLANNED_CRATES = {"rust-webx-redis-cluster", "rust-webx-nats-jetstream"}
FAMILY_NAMES = {"rust-webx-auth", "rust-webx-push"}
ALLOWED_CRATE_NAMES = (
    REFERENCE_ADAPTER_CRATES | TEST_CRATES | PLANNED_CRATES | FAMILY_NAMES
)

AUTHORITY_PAGES = [
    "docs/design/doc-authority.md",
    "docs/design/known-boundaries.md",
    "docs/design/broker-capability-matrix.md",
    "docs/design/error-codes.md",
    "docs/design/close-codes.md",
    "docs/design/compatibility.md",
    "docs/design/deprecations.md",
    "docs/rust-webx/04-architecture/crate-layout.md",
    "docs/rust-webx/05-request-pattern/pagination.md",
]

# Contract types that are defined in rust-core, not rust-webx-core.
RUST_CORE_TYPES = [
    "PagedRequest",
    "PagedResult",
    "KeysetRequest",
    "CursorPage",
    "OperationResult",
    "IPatchable",
    "AppError",
    "Cursor",
]


def rel(path: Path) -> str:
    return str(path.relative_to(ROOT)).replace("\\", "/")


def is_historical(path: Path) -> bool:
    text = rel(path)
    return any(text.startswith(prefix) for prefix in HISTORICAL_PATHS)


META_FILES = {"docs/design/doc-authority.md"}


def live_docs() -> list[Path]:
    files = sorted(HANDBOOK.rglob("*.md")) + sorted(DESIGN.glob("*.md")) + [p for p in README if p.exists()]
    return [
        p
        for p in files
        if not is_historical(p) and rel(p) not in META_FILES
    ]


def read(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def cargo_metadata() -> dict:
    raw = subprocess.check_output(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        cwd=ROOT,
        text=True,
        encoding="utf-8",
    )
    return json.loads(raw)


def published_crates(metadata: dict) -> dict[str, dict]:
    crates: dict[str, dict] = {}
    for pkg in metadata["packages"]:
        if not pkg["name"].startswith("rust-webx"):
            continue
        if pkg.get("publish") == []:
            continue
        lib = next(
            (t["name"] for t in pkg["targets"] if "lib" in t["kind"] or "proc-macro" in t["kind"]),
            pkg["name"].replace("-", "_"),
        )
        crates[pkg["name"]] = {"lib": lib, "manifest": pkg["manifest_path"]}
    return crates


def check_inventory() -> list[str]:
    errors: list[str] = []
    result = subprocess.run(
        [sys.executable, str(ROOT / "scripts" / "gen-crate-inventory.py"), "--check"],
        cwd=ROOT,
        text=True,
        capture_output=True,
    )
    if result.returncode != 0:
        errors.append(
            "generated crate inventory is stale: run `python scripts/gen-crate-inventory.py`\n"
            + (result.stderr.strip() or result.stdout.strip())
        )
    inc = HANDBOOK / "04-architecture" / "crate-inventory.inc"
    if not inc.exists():
        errors.append("missing docs/rust-webx/04-architecture/crate-inventory.inc")
        return errors
    text = read(inc)
    metadata = cargo_metadata()
    for name in published_crates(metadata):
        if f"`{name}`" not in text:
            errors.append(f"generated inventory is missing published crate `{name}`")
    return errors


def check_crate_names(metadata: dict) -> list[str]:
    errors: list[str] = []
    published = published_crates(metadata)
    token = re.compile(r"rust-webx(?:-[a-z0-9]+)+")
    for path in live_docs():
        text = read(path)
        lines = text.splitlines()
        for match in token.finditer(text):
            name = match.group(0)
            if name in published or name in ALLOWED_CRATE_NAMES:
                continue
            # `rust-webx-auth/ef` is a path shorthand, not a crate mention.
            if match.end() < len(text) and text[match.end()] == "/":
                continue
            line = text.count("\n", 0, match.start()) + 1
            if line <= len(lines) and ALLOW_STALE_MARKER in lines[line - 1]:
                continue
            if name in REMOVED_CRATES:
                errors.append(
                    f"{rel(path)}:{line}: removed crate `{name}` in live docs"
                    " (only a marked historical footnote may mention it)"
                )
                continue
            errors.append(
                f"{rel(path)}:{line}: unknown crate `{name}` in live docs"
                " (see docs/design/doc-authority.md)"
            )
    return errors


def parse_deprecations() -> set[str]:
    path = DESIGN / "deprecations.md"
    tokens: set[str] = set()
    for line in read(path).splitlines():
        if not line.startswith("|"):
            continue
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if len(cells) < 2 or "`removed`" not in line:
            continue
        # The first cell may explain the removal in parentheses (e.g.
        # "`authz::resource_auth_middleware` ...（路由前看不到 `route_pattern`）").
        # Only the symbols before that parenthesis are removed API names.
        first = cells[0].split("（", 1)[0]
        for token in re.findall(r"`([^`]+)`", first):
            token = token.strip()
            if (
                not token
                or token.startswith(("#", "examples/", "integrations/"))
                or token.startswith("rust-webx")
                or "/" in token
            ):
                continue
            tokens.add(token)
    return BASE_STALE_TOKENS | tokens


def stale_token_pattern(token: str) -> re.Pattern[str]:
    # Match the registered symbol, including a trailing call/signature prefix
    # such as `order_by(&str)` -> `order_by(`. Avoid matching substrings inside
    # longer identifiers.
    escaped = re.escape(token)
    if token.endswith("(&str)"):
        escaped = re.escape(token[: -len("(&str)")]) + r"\("
    return re.compile(r"(?<![A-Za-z0-9_:])" + escaped + r"(?![A-Za-z0-9_])")


def check_stale_apis() -> list[str]:
    errors: list[str] = []
    tokens = parse_deprecations()
    for path in live_docs():
        text = read(path)
        lines = text.splitlines()
        for token in tokens:
            match = stale_token_pattern(token).search(text)
            if match:
                line = text.count("\n", 0, match.start()) + 1
                if line <= len(lines) and ALLOW_STALE_MARKER in lines[line - 1]:
                    continue
                errors.append(
                    f"{rel(path)}:{line}: stale API `{token}` in live docs"
                    " (mark a historical footnote or update the page)"
                )
    return errors


def check_contract_ownership() -> list[str]:
    errors: list[str] = []
    type_group = "|".join(re.escape(t) for t in RUST_CORE_TYPES)
    bad_owner = re.compile(
        r"rust-webx-core[^\n]{0,80}(?:定义|提供|包含)[^\n]{0,80}(?:" + type_group + r")"
    )
    bad_module = re.compile(
        r"webx_core::(?:pagination|keyset|operation|persistence|error)"
        r"[^\n]{0,50}(?:定义|提供|是[^\n]{0,10}定义)"
    )
    for path in live_docs():
        text = read(path)
        for pattern in (bad_owner, bad_module):
            match = pattern.search(text)
            if match:
                line = text.count("\n", 0, match.start()) + 1
                errors.append(
                    f"{rel(path)}:{line}: shared contract attributed to webx-core"
                    " (definitions live in rust-core; see docs/design/doc-authority.md)"
                )
    return errors


def check_current_versions() -> list[str]:
    errors: list[str] = []
    old_current = re.compile(
        r"当前(?:发布|基线|版本)\s*\**\s*2\.9\.[0-2](?![0-9])"
    )
    for path in live_docs():
        text = read(path)
        match = old_current.search(text)
        if match:
            line = text.count("\n", 0, match.start()) + 1
            errors.append(
                f"{rel(path)}:{line}: stale current-version claim `{match.group(0).strip()}`;"
                " current release claims belong in docs/design/compatibility.md"
            )
    return errors


def check_known_versions() -> list[str]:
    errors: list[str] = []
    patterns = [
        re.compile(r"v2\.7\.0"),
        re.compile(r"N-2[^\n]{0,40}2\.7\.[0-9]"),
        re.compile(r"rust-dix 1\.0(?![.0-9])"),
        re.compile(r"rust-ef 2\.9\.0(?![.0-9])"),
    ]
    for path in live_docs():
        text = read(path)
        for pattern in patterns:
            match = pattern.search(text)
            if match:
                line = text.count("\n", 0, match.start()) + 1
                errors.append(
                    f"{rel(path)}:{line}: stale baseline/dependency claim `{match.group(0)}`;"
                    " update to the current compatibility baseline"
                )
    return errors


def check_authority_pages() -> list[str]:
    errors: list[str] = []
    for path in AUTHORITY_PAGES:
        if not (ROOT / path).exists():
            errors.append(f"missing authority page: {path}")
    # The full published inventory must only be the generated include; the
    # handbook must not grow a second hand-written "package name -> import name"
    # table again.
    header = "| 包名（`Cargo.toml`） | 导入名（`use`） |"
    for path in sorted(HANDBOOK.rglob("*.md")):
        if header in read(path):
            errors.append(
                f"{rel(path)}: hand-written package/import inventory table;"
                " use the generated crate-inventory.inc (docs/design/doc-authority.md)"
            )
    return errors


def main() -> int:
    metadata = cargo_metadata()
    errors: list[str] = []
    errors += check_inventory()
    errors += check_crate_names(metadata)
    errors += check_stale_apis()
    errors += check_contract_ownership()
    errors += check_current_versions()
    errors += check_known_versions()
    errors += check_authority_pages()
    if errors:
        print("check-docs-drift: failed")
        for error in errors:
            print(f"  - {error}")
        return 1
    print(
        f"check-docs-drift: ok ({len(published_crates(metadata))} published crates, "
        f"{len(live_docs())} live docs checked)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

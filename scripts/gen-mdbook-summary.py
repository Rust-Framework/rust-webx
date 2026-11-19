#!/usr/bin/env python3
"""Generate the mdBook `SUMMARY.md` from the handbook's `INDEX.json`.

`docs/<slug>/INDEX.json` is the single source of truth for the handbook
navigation: Docbit renders it directly, and the GitHub Pages site (mdBook)
derives its table of contents from it through this script. Adding or
renaming a chapter/section means editing `INDEX.json` only, then running:

    python scripts/gen-mdbook-summary.py

The handbook slug defaults to the repository directory name (`rust-webx`,
`rust-dix`); pass it explicitly when the checkout directory is named
differently. The Docs workflow regenerates the file before `mdbook build`
and fails when the committed copy is stale, so the published site never
drifts from `INDEX.json`.

Exit code is non-zero when `INDEX.json` is missing or references a file
that does not exist.
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_SLUG = REPO_ROOT.name

CN_DIGITS = "零一二三四五六七八九"
CHAPTER_NUMBER = re.compile(r"^第[零一二三四五六七八九十百]+章")


def cn_number(n: int) -> str:
    """Return the Chinese numeral for 1..99 (e.g. 1 -> 一, 16 -> 十六)."""
    if not 0 < n < 100:
        raise ValueError(f"chapter number out of range: {n}")
    if n < 10:
        return CN_DIGITS[n]
    if n < 20:
        return "十" + (CN_DIGITS[n % 10] if n % 10 else "")
    tens, ones = divmod(n, 10)
    return CN_DIGITS[tens] + "十" + (CN_DIGITS[ones] if ones else "")


def chapter_title(chapter: dict) -> str:
    """Prefix the chapter title with its Chinese number from the id (01 -> 第一章).

    Titles that already carry their number (rust-dix writes "第一章 …" in
    INDEX.json) are left untouched so the site never shows it twice.
    """
    title = chapter["title"]
    prefix = str(chapter["id"]).split("-", 1)[0]
    if not prefix.isdigit() or CHAPTER_NUMBER.match(title):
        return title
    return f"第{cn_number(int(prefix))}章 {title}"


def main() -> int:
    slug = sys.argv[1] if len(sys.argv) > 1 else DEFAULT_SLUG
    src = REPO_ROOT / "docs" / slug
    index_path = src / "INDEX.json"
    if not index_path.is_file():
        print(f"gen-mdbook-summary: missing {index_path.relative_to(REPO_ROOT)}", file=sys.stderr)
        print(
            "usage: python scripts/gen-mdbook-summary.py [handbook-slug]"
            "  (default: repository directory name)",
            file=sys.stderr,
        )
        return 1

    index = json.loads(index_path.read_text(encoding="utf-8"))
    meta = index["meta"]

    lines: list[str] = ["# Summary", ""]
    missing: list[str] = []

    foreword = meta.get("foreword")
    if foreword:
        lines.append(f"[前言](./{foreword})")
        lines.append("")

    for part in index.get("parts", []):
        chapters = part.get("chapters") or []
        if not chapters:
            continue
        lines.append("---")
        lines.append("")
        lines.append(f"# {part['title']}")
        lines.append("")
        for chapter in chapters:
            chapter_id = chapter["id"]
            chapter_path = f"{chapter_id}/INDEX.md"
            if not (src / chapter_path).is_file():
                missing.append(chapter_path)
            lines.append(f"- [{chapter_title(chapter)}](./{chapter_path})")
            for section in chapter.get("sections") or []:
                section_path = f"{chapter_id}/{section['id']}.md"
                if not (src / section_path).is_file():
                    missing.append(section_path)
                lines.append(f"  - [{section['title']}](./{section_path})")
            lines.append("")

    if missing:
        for path in missing:
            print(f"gen-mdbook-summary: INDEX.json references missing file {path}", file=sys.stderr)
        return 1

    target = src / "SUMMARY.md"
    target.write_text("\n".join(lines).rstrip("\n") + "\n", encoding="utf-8", newline="\n")
    print(f"wrote {target.relative_to(REPO_ROOT)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
"""Check local Markdown links without external network access."""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
LINK_RE = re.compile(r"\[[^\]]+\]\(([^)]+)\)")


def iter_markdown() -> list[Path]:
    return sorted(
        p
        for p in ROOT.rglob("*.md")
        if ".git" not in p.parts and "target" not in p.parts
    )


def local_target(raw: str) -> str | None:
    target = raw.strip()
    if not target or target.startswith(("#", "http://", "https://", "mailto:")):
        return None
    if "://" in target:
        return None
    return target.split("#", 1)[0]


def main() -> int:
    failures: list[str] = []
    for path in iter_markdown():
        text = path.read_text(encoding="utf-8")
        for match in LINK_RE.finditer(text):
            target = local_target(match.group(1))
            if not target:
                continue
            resolved = (path.parent / target).resolve()
            try:
                resolved.relative_to(ROOT)
            except ValueError:
                failures.append(f"{path.relative_to(ROOT)}: link escapes repo: {target}")
                continue
            if not resolved.exists():
                failures.append(f"{path.relative_to(ROOT)}: missing link target: {target}")

    for failure in failures:
        print(failure, file=sys.stderr)
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
"""Conservatively verify that Markdown formatting did not change text content."""

from __future__ import annotations

import argparse
import json
import re
import sys
import unicodedata
from pathlib import Path


STRUCTURAL_LINE = re.compile(
    r"^(?:#{1,6}\s+|>\s?|[-+*]\s+(?:\[[ xX]\]\s+)?|\d+[.)]\s+)"
)
TABLE_SEPARATOR = re.compile(r"^\|?\s*:?-{3,}:?\s*(?:\|\s*:?-{3,}:?\s*)+\|?$")
FENCE = re.compile(r"^\s*(```+|~~~+)(?:[^`]*)$")
HORIZONTAL_RULE = re.compile(r"^\s*(?:-{3,}|\*{3,}|_{3,})\s*$")


def visible_text(markdown: str) -> str:
    """Remove structural Markdown while retaining user-authored characters."""
    pieces: list[str] = []
    for raw_line in unicodedata.normalize("NFC", markdown).splitlines():
        line = raw_line.strip()
        if not line or FENCE.match(line) or TABLE_SEPARATOR.match(line) or HORIZONTAL_RULE.match(line):
            continue
        line = STRUCTURAL_LINE.sub("", line)
        # Table pipes are formatting delimiters. Escaped pipes remain content.
        line = re.sub(r"(?<!\\)\|", "", line)
        # Paired inline markers may be added or normalized by a format-only edit.
        previous = None
        while previous != line:
            previous = line
            line = re.sub(r"(\*\*|__|~~|`)(.+?)\1", r"\2", line)
            line = re.sub(r"(?<!\*)\*([^*\n]+?)\*(?!\*)", r"\1", line)
            line = re.sub(r"(?<!_)_([^_\n]+?)_(?!_)", r"\1", line)
        pieces.append(re.sub(r"\s+", "", line))
    return "".join(pieces)


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Check that a format-only Markdown edit preserved visible text and order."
    )
    parser.add_argument("original", type=Path)
    parser.add_argument("formatted", type=Path)
    args = parser.parse_args()

    original = visible_text(args.original.read_text(encoding="utf-8"))
    formatted = visible_text(args.formatted.read_text(encoding="utf-8"))
    ok = original == formatted
    print(json.dumps({"ok": ok, "originalLength": len(original), "formattedLength": len(formatted)}, ensure_ascii=False))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())

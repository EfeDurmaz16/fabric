#!/usr/bin/env python3
"""Fails when a page makes a claim the evidence on main does not back
without the qualifier that makes it true. Each rule names a phrase and the
words one of which the same paragraph must carry. Evidence logs and ADRs
are records of their day and are not checked.

Rules, and when each can go:
- "real Mac": the only move between two real Macs ran on 2026-10-21, on
  the binary of that day, and its log is not in the repository. A
  paragraph that names it says it was before M2 to M4, or that it is
  pending or never ran. Drop the rule when gate G1 is recorded on main.

Usage: scripts/docs/check-claims.py [file ...]
Standard library only.
"""

import os
import re
import subprocess
import sys

RULES = [
    (re.compile(r"real Macs?\b"), ("before M2", "pending", "never", "not been run")),
]


def paragraphs(text):
    """Blank-line separated blocks; a table row or list item is its own."""
    block = []
    for line in text.split("\n"):
        if not line.strip():
            if block:
                yield " ".join(block)
                block = []
        elif line.lstrip().startswith(("|", "- ", "* ")):
            if block:
                yield " ".join(block)
                block = []
            yield line
        else:
            block.append(line)
    if block:
        yield " ".join(block)


def main():
    root = subprocess.run(
        ["git", "rev-parse", "--show-toplevel"], check=True, capture_output=True, text=True
    ).stdout.strip()
    files = sys.argv[1:]
    if not files:
        listed = subprocess.run(
            ["git", "ls-files", "-z", "--", "*.md"], cwd=root, check=True, capture_output=True, text=True
        ).stdout.split("\0")
        files = [
            os.path.join(root, f)
            for f in listed
            if f and not f.startswith(("docs/evidence/", "docs/adr/"))
        ]
    bad = 0
    for path in files:
        with open(path, encoding="utf-8") as f:
            text = f.read()
        for para in paragraphs(text):
            for phrase, qualifiers in RULES:
                if phrase.search(para) and not any(q in para for q in qualifiers):
                    print("%s: '%s' without one of: %s" % (
                        os.path.relpath(path, root), phrase.pattern, ", ".join(qualifiers)))
                    bad += 1
    if bad:
        sys.exit("error: %d unqualified claim(s); see scripts/docs/check-claims.py" % bad)


if __name__ == "__main__":
    main()

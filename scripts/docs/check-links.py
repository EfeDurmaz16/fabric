#!/usr/bin/env python3
"""Fails when a relative link in a tracked Markdown file names a file or
directory the repository does not have. Evidence logs under docs/evidence
are records of past runs and are not checked; their own index is.

Usage: scripts/docs/check-links.py [file ...]
       scripts/docs/check-links.py --site <dir>
Without files, every tracked *.md outside docs/evidence is checked. With
--site, every relative href in the built site's pages must name a file of
the site; a link that starts with / is the site URL and is skipped.
A link to a page that only includes a file from outside docs/ (such as
docs/contributing.md) fails too: on GitHub it lands on the stub, so link
the included file instead, which the site maps to the chapter. Only
docs/SUMMARY.md, the site's table of contents, names such a page.
"""

import os
import re
import subprocess
import sys
from urllib.parse import unquote

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import links  # noqa: E402

INCLUDE = re.compile(r"^\{\{#include\s+\.\./\S+\s*\}\}$", re.M)


def is_stub(path):
    """A page under docs/ whose content is an include of an outside file."""
    if not path.endswith(".md") or not os.path.isfile(path):
        return False
    with open(path, encoding="utf-8") as f:
        return bool(INCLUDE.search(f.read()))


def check_site(site):
    broken = 0
    for folder, _, names in os.walk(site):
        for name in names:
            if not name.endswith(".html"):
                continue
            path = os.path.join(folder, name)
            with open(path, encoding="utf-8") as f:
                hrefs = re.findall(r'href="([^"]+)"', f.read())
            for href in hrefs:
                target = unquote(href.split("#", 1)[0])
                if not target or target.startswith("/") or not links.is_relative(target):
                    continue
                if not os.path.exists(os.path.normpath(os.path.join(folder, target))):
                    print("%s: broken link %s" % (os.path.relpath(path, site), href))
                    broken += 1
    if broken:
        sys.exit("error: %d broken link(s) in the site" % broken)


def main():
    if sys.argv[1:2] == ["--site"] and len(sys.argv) == 3:
        return check_site(sys.argv[2])
    root = subprocess.run(
        ["git", "rev-parse", "--show-toplevel"], check=True, capture_output=True, text=True
    ).stdout.strip()
    files = sys.argv[1:]
    if not files:
        listed = subprocess.run(
            ["git", "ls-files", "-z", "--", "*.md"], cwd=root, check=True, capture_output=True, text=True
        ).stdout.split("\0")
        files = [os.path.join(root, f) for f in listed if f and not f.startswith("docs/evidence/")]
    broken = 0
    for path in files:
        with open(path, encoding="utf-8") as f:
            text = f.read()
        for target in links.targets(text):
            target = target.strip("<>")
            if not links.is_relative(target):
                continue
            name = unquote(target.split("#", 1)[0])
            if not name:
                continue
            resolved = os.path.normpath(os.path.join(os.path.dirname(path), name))
            if not os.path.exists(resolved):
                print("%s: broken link %s" % (os.path.relpath(path, root), target))
                broken += 1
            elif os.path.basename(path) != "SUMMARY.md" and is_stub(resolved):
                print("%s: link %s lands on an include stub on GitHub; link the file it includes"
                      % (os.path.relpath(path, root), target))
                broken += 1
    if broken:
        sys.exit("error: %d broken relative link(s)" % broken)


if __name__ == "__main__":
    main()

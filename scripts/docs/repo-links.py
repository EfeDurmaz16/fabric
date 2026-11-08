#!/usr/bin/env python3
"""mdBook preprocessor: keeps the docs' relative links working on the site.

A link to another chapter stays relative, and mdBook makes it .html (a
README.md chapter is index.html). A
link to anything else in the repository (source, fixtures, evidence, a
Markdown file that is not a chapter) becomes a link to that path on the
repository's main branch, so the same Markdown reads right on GitHub and
on the site. A line that is only {{#include <file>}} of a file outside
docs/ is inlined here, with its links rebased on the included file, so
CONTRIBUTING.md is one file and still reads right on both. A link to such
an included file goes to the chapter that includes it, and the lines
between <!-- github-only:begin --> and <!-- github-only:end --> (a pointer
for readers of the stub on GitHub) leave the site.

The repository URL comes from scripts/docs/names.env. Standard library only.
"""

import json
import os
import re
import sys

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import links  # noqa: E402

INCLUDE = re.compile(r"^\{\{#include\s+(\S+)\s*\}\}$")
GITHUB_ONLY = re.compile(r"<!-- github-only:begin -->.*?<!-- github-only:end -->\n?", re.S)


def names(root):
    values = {}
    with open(os.path.join(root, "scripts/docs/names.env"), encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if line and not line.startswith("#") and "=" in line:
                key, value = line.split("=", 1)
                values[key] = value
    return values


def chapters(items, found):
    for item in items:
        if isinstance(item, dict) and "Chapter" in item:
            chapter = item["Chapter"]
            if chapter.get("source_path"):
                found.append(chapter)
            chapters(chapter.get("sub_items", []), found)
    return found


def main():
    if len(sys.argv) > 1 and sys.argv[1] == "supports":
        sys.exit(0)
    context, book = json.load(sys.stdin)
    root = context["root"]
    src = os.path.normpath(os.path.join(root, context["config"]["book"].get("src", "src")))
    repo = names(root)["REPO_URL"].rstrip("/")
    every = chapters(book.get("items", book.get("sections", [])), [])
    pages = {os.path.normpath(os.path.join(src, c["source_path"])) for c in every}
    # An outside file a chapter includes whole is read on the site as that chapter.
    included_by = {}
    for chapter in every:
        chapter_dir = os.path.dirname(os.path.join(src, chapter["source_path"]))
        for line in chapter["content"].split("\n"):
            m = INCLUDE.match(line.strip())
            if m:
                path = os.path.normpath(os.path.join(chapter_dir, m.group(1)))
                if os.path.commonpath([path, src]) != src:
                    included_by[path] = os.path.normpath(os.path.join(src, chapter["source_path"]))

    def fixer(base, chapter_dir):
        def fix(target):
            bare = target.strip("<>")
            if not links.is_relative(bare):
                return target
            name, _, anchor = bare.partition("#")
            if not name:
                return target
            path = os.path.normpath(os.path.join(base, name))
            path = included_by.get(path, path)
            if path in pages:
                rel = os.path.relpath(path, chapter_dir)
                # mdBook renders a README.md chapter as index.html.
                if os.path.basename(rel) == "README.md":
                    rel = os.path.join(os.path.dirname(rel), "index.md")
                return rel + ("#" + anchor if anchor else "")
            if os.path.commonpath([path, root]) != root or not os.path.exists(path):
                return target
            kind = "tree" if os.path.isdir(path) else "blob"
            return "%s/%s/main/%s%s" % (repo, kind, os.path.relpath(path, root), "#" + anchor if anchor else "")

        return fix

    for chapter in every:
        chapter_dir = os.path.dirname(os.path.join(src, chapter["source_path"]))
        lines = []
        for line in GITHUB_ONLY.sub("", chapter["content"]).split("\n"):
            m = INCLUDE.match(line.strip())
            included = os.path.normpath(os.path.join(chapter_dir, m.group(1))) if m else None
            if included and os.path.commonpath([included, src]) != src and os.path.isfile(included):
                with open(included, encoding="utf-8") as f:
                    lines.append(links.rewrite(f.read(), fixer(os.path.dirname(included), chapter_dir)))
            else:
                lines.append(line)
        chapter["content"] = links.rewrite("\n".join(lines), fixer(chapter_dir, chapter_dir))
    json.dump(book, sys.stdout)


if __name__ == "__main__":
    main()

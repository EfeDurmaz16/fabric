#!/usr/bin/env python3
"""Writes the generated parts of the docs from the files they describe.

- docs/adr/README.md, the ADR index, from the title and status line of
  every docs/adr/NNNN-*.md, and the ADR list in docs/SUMMARY.md.
- docs/reference/evidence.md, the evidence index, from docs/evidence.
- The "what moves" matrix in README.md and docs/README.md, copied from
  docs/start/what-moves.md, where it is written once.
- The fault points in docs/concepts/ledger.md, from the node's fault
  module and the end-to-end tests that arm them, so the page never counts
  them by hand.

Usage: scripts/docs/gen.py [--check]
With --check nothing is written, and the script fails when a generated
part differs from what is committed. Standard library only.
"""

import os
import re
import subprocess
import sys

ROOT = subprocess.run(
    ["git", "rev-parse", "--show-toplevel"], check=True, capture_output=True, text=True
).stdout.strip()

# Where the fault points are defined and where the end-to-end test arms them.
FAULTS = "crates/fabricd/src/faults.rs"
E2E = "crates/fabric/tests/e2e"

GENERATED = "<!-- This page is written by scripts/docs/gen.py. Do not edit it; run the script. -->"


def read(rel):
    with open(os.path.join(ROOT, rel), encoding="utf-8") as f:
        return f.read()


def block(text, name, rel):
    """Returns the text between the begin and end markers of `name`."""
    begin, end = "<!-- %s:begin -->" % name, "<!-- %s:end -->" % name
    if text.count(begin) != 1 or text.count(end) != 1:
        sys.exit("error: %s needs exactly one %s and one %s" % (rel, begin, end))
    return text.split(begin, 1)[1].split(end, 1)[0]


def replace_block(text, name, body, rel):
    old = block(text, name, rel)
    begin = "<!-- %s:begin -->" % name
    return text.replace(begin + old, begin + body, 1)


def adrs():
    """(number, file, title, status) for every ADR, in number order."""
    out = []
    for name in sorted(os.listdir(os.path.join(ROOT, "docs/adr"))):
        m = re.match(r"^(\d{4})-.*\.md$", name)
        if not m:
            continue
        text = read("docs/adr/" + name)
        title = re.search(r"^# ADR \d{4}: (.+)$", text, re.M)
        status = re.search(r"^Status: (.+)$", text, re.M)
        if not title or not status:
            sys.exit("error: docs/adr/%s needs a '# ADR NNNN: title' line and a 'Status:' line" % name)
        out.append((m.group(1), name, title.group(1).strip(), status.group(1).strip()))
    return out


def adr_index(entries):
    numbers = {int(n) for n, _, _, _ in entries}
    missing = [
        "%04d" % n for n in range(1, max(numbers) + 1) if n not in numbers
    ] if numbers else []
    lines = [
        GENERATED,
        "",
        "# Decisions",
        "",
        "Every architecture decision record on `main`, with the status line the ADR itself carries. "
        "An ADR that says `proposed` and `as built on main` records code that is on `main` "
        "while the decision is still open for amendment; the status line says which parts.",
        "",
    ]
    if missing:
        lines += ["No ADR on `main` has the number %s." % ", ".join(missing), ""]
    lines += [
        "How to write one is in [CONTRIBUTING, Decisions](../../CONTRIBUTING.md#decisions).",
        "",
        "| ADR | Title | Status |",
        "|---|---|---|",
    ]
    for number, name, title, status in entries:
        lines.append("| [%s](%s) | %s | %s |" % (number, name, title, status.replace("|", "\\|")))
    return "\n".join(lines) + "\n"


def summary_block(entries):
    return "\n" + "".join(
        "  - [ADR %s: %s](adr/%s)\n" % (n, t, f) for n, f, t, _ in entries
    )


def first_heading(rel):
    for line in read(rel).splitlines():
        if line.startswith("# "):
            return line[2:].strip()
    return None


def evidence_index():
    base = os.path.join(ROOT, "docs/evidence")
    lines = [
        GENERATED,
        "",
        "# Evidence",
        "",
        "What was run to prove a claim, kept in the repository: gate logs, A/B "
        "performance runs, walks of `main`, interop runs. One row per entry of "
        "`docs/evidence`, with the first heading of its main page. Everything "
        "here is on `main`; work proved only on a branch keeps its evidence on that branch.",
        "",
        "| Entry | What it holds |",
        "|---|---|",
    ]
    for name in sorted(os.listdir(base)):
        path = os.path.join(base, name)
        if name.startswith("."):
            continue
        if os.path.isdir(path):
            mds = sorted(
                f for f in os.listdir(path) if f.endswith(".md") and os.path.isfile(os.path.join(path, f))
            )
            page = "README.md" if "README.md" in mds else (mds[0] if mds else None)
            if page:
                title = first_heading("docs/evidence/%s/%s" % (name, page)) or page
                lines.append("| [%s](../evidence/%s/%s) | %s |" % (name, name, page, title))
            else:
                lines.append("| [%s](../evidence/%s) | Raw run output, no summary page |" % (name, name))
        elif name.endswith(".md"):
            title = first_heading("docs/evidence/" + name) or name
            lines.append("| [%s](../evidence/%s) | %s |" % (name, name, title))
    return "\n".join(lines) + "\n"


def fault_points():
    """A Markdown list of every fault point the end-to-end test arms, in
    the order the node defines them, each with its doc comment."""
    text = read(FAULTS)
    enum = re.search(r"pub enum Point \{(.*?)\n\}", text, re.S)
    names = dict(re.findall(r'Point::(\w+) => "(\w+)"', text))
    if not enum or not names:
        sys.exit("error: %s: no `pub enum Point` or no name for its variants" % FAULTS)
    armed = set()
    base = os.path.join(ROOT, E2E)
    for name in sorted(os.listdir(base)):
        if name.endswith(".rs"):
            armed |= set(re.findall(r'"(\w+)(?::\w+)?"', read(E2E + "/" + name)))
    lines, doc = [], []
    for line in enum.group(1).split("\n"):
        line = line.strip()
        if line.startswith("///"):
            doc.append(line[3:].strip())
        elif re.match(r"^\w+,$", line):
            variant = line[:-1]
            if variant not in names:
                sys.exit("error: %s: Point::%s has no name" % (FAULTS, variant))
            if names[variant] in armed:
                lines.append("- `%s`: %s" % (names[variant], " ".join(doc)))
            doc = []
    if not lines:
        sys.exit("error: no fault point of %s is armed in %s" % (FAULTS, E2E))
    return "\n" + "\n".join(lines) + "\n"


def main():
    check = sys.argv[1:] == ["--check"]
    if sys.argv[1:] not in ([], ["--check"]):
        sys.exit(__doc__)
    entries = adrs()
    matrix = block(read("docs/start/what-moves.md"), "matrix", "docs/start/what-moves.md")
    wanted = {
        "docs/adr/README.md": adr_index(entries),
        "docs/SUMMARY.md": replace_block(read("docs/SUMMARY.md"), "adr", summary_block(entries), "docs/SUMMARY.md"),
        "docs/reference/evidence.md": evidence_index(),
        "README.md": replace_block(read("README.md"), "matrix", matrix, "README.md"),
        "docs/README.md": replace_block(read("docs/README.md"), "matrix", matrix, "docs/README.md"),
        "docs/concepts/ledger.md": replace_block(
            read("docs/concepts/ledger.md"), "faults", fault_points(), "docs/concepts/ledger.md"
        ),
    }
    stale = []
    for rel, text in wanted.items():
        path = os.path.join(ROOT, rel)
        current = read(rel) if os.path.exists(path) else None
        if current == text:
            continue
        if check:
            stale.append(rel)
        else:
            with open(path, "w", encoding="utf-8") as f:
                f.write(text)
    if stale:
        sys.exit("error: out of date, run scripts/docs/gen.py: " + ", ".join(stale))


if __name__ == "__main__":
    main()

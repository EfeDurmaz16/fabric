"""Finds the relative links of a Markdown text, outside code. Shared by
check-links.py and the repo-links preprocessor."""

import re

INLINE = re.compile(r"\]\(\s*(<[^>]*>|[^)\s]+)((?:\s+\"[^\"]*\")?\s*)\)")
REFERENCE = re.compile(r"^(\s{0,3}\[[^\]]+\]:\s*)(\S+)(.*)$")
EXTERNAL = re.compile(r"^(?:[a-z][a-z0-9+.-]*:|#|//)", re.I)


def is_relative(target):
    return not EXTERNAL.match(target)


def rewrite(text, fn):
    """Calls fn(target) for every link target outside code and puts back
    what it returns. Fenced blocks and inline code spans are left alone."""
    out, fence = [], None
    for line in text.split("\n"):
        stripped = line.lstrip()
        marker = re.match(r"^(`{3,}|~{3,})", stripped)
        if fence:
            if marker and marker.group(1)[0] == fence[0] and len(marker.group(1)) >= len(fence):
                fence = None
            out.append(line)
            continue
        if marker:
            fence = marker.group(1)
            out.append(line)
            continue
        ref = REFERENCE.match(line)
        if ref:
            out.append(ref.group(1) + fn(ref.group(2)) + ref.group(3))
            continue
        parts = line.split("`")
        for i in range(0, len(parts), 2):
            parts[i] = INLINE.sub(lambda m: "](" + fn(m.group(1)) + m.group(2) + ")", parts[i])
        out.append("`".join(parts))
    return "\n".join(out)


def targets(text):
    found = []

    def keep(target):
        found.append(target)
        return target

    rewrite(text, keep)
    return found

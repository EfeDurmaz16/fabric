#!/usr/bin/env python3
"""A Portable Session Format v1 reader in the Python standard library.

This is the independence check of docs/portable-session-format.md,
section 13: it shares no code with the Rust crate (crates/fabric-psf) or
the TypeScript reader (sdk/typescript/src/psf.ts), and all three must give
the same verdict, code and field for every fixture in protocol/psf/v1.
It follows crates/fabric-psf/src/check.rs phase by phase.

Usage, with python3 -I so nothing in the working directory is imported:

  psf_check.py <manifest.json | container dir>
      prints the digest, the offered fidelity and the detached attachments,
      or exits 1 with "<code> <field>".
  psf_check.py --fixtures protocol/psf/v1
      checks every case and container against its expect; exits 1 on the
      first disagreement.
  psf_check.py --verdicts protocol/psf/v1
      prints one verdict line per fixture, the lines the other readers print.
  psf_check.py --lines <file>
      reads one JSON string per line, the bytes of a manifest, and prints
      one verdict line each, named lines/<index>; the readers' tests feed
      it generated documents.

Python 3.9, standard library only: json, hashlib, re, os, unicodedata.
"""

import hashlib
import json
import math
import os
import re
import shutil
import sys
import tempfile
import unicodedata

MANIFEST_MAX_BYTES = 1 << 20
TEXT_MAX_BYTES = 16 * 1024
PACKAGE_MAX_BYTES = 256 * 1024
NAME_MAX_BYTES = 128
HARNESS_STATE_FORMAT = "fabric.harness_state.v1"
U64_MAX = 18446744073709551615


class Refusal(Exception):
    """A refused manifest: the code of section 10 and the field."""

    def __init__(self, code, field):
        Exception.__init__(self, code, field)
        self.code = code
        self.field = field


class ContainerRefusal(Exception):
    """A refused container: the code, the path inside it, and for a refused
    manifest its field."""

    def __init__(self, code, path, field=""):
        Exception.__init__(self, code, path, field)
        self.code = code
        self.path = path
        self.field = field


class _NotJson(Exception):
    pass


class _NoForm(Exception):
    def __init__(self, path):
        Exception.__init__(self, path)
        self.path = path


def _child(path, key):
    return key if path == "" else path + "." + key


def _element(path, index):
    return "%s[%d]" % (path, index)


def _sorted_keys(obj):
    return sorted(obj.keys(), key=lambda k: k.encode("utf-8"))


_KEY = re.compile(r"[a-z0-9_]+\Z")


def _write_string(text):
    out = ['"']
    for c in text:
        if c == '"':
            out.append('\\"')
        elif c == "\\":
            out.append("\\\\")
        elif c == "\n":
            out.append("\\n")
        elif c == "\r":
            out.append("\\r")
        elif c == "\t":
            out.append("\\t")
        elif ord(c) < 0x20:
            out.append("\\u%04x" % ord(c))
        else:
            out.append(c)
    out.append('"')
    return "".join(out)


def canonical(value, path=""):
    """The canonical text of a parsed value (section 4); raises _NoForm with
    the path of the first JSON number or bad key in canonical order."""
    if value is None:
        return "null"
    if value is True:
        return "true"
    if value is False:
        return "false"
    if isinstance(value, (int, float)):
        raise _NoForm(path)
    if isinstance(value, str):
        return _write_string(value)
    if isinstance(value, list):
        return "[" + ",".join(canonical(v, _element(path, i)) for i, v in enumerate(value)) + "]"
    parts = []
    for key in _sorted_keys(value):
        at = _child(path, key)
        if not _KEY.match(key):
            raise _NoForm(at)
        parts.append(_write_string(key) + ":" + canonical(value[key], at))
    return "{" + ",".join(parts) + "}"


def _reject_constant(name):
    raise _NotJson(name)


# serde_json, the Rust reader's parser, refuses a number past the range of
# a binary64; one that rounds to zero it reads.
def _float(text):
    value = float(text)
    if math.isinf(value):
        raise _NotJson(text)
    return value


def _int(text):
    value = int(text)
    try:
        float(value)
    except OverflowError:
        raise _NotJson(text)
    return value


def _has_surrogate(text):
    return any(0xD800 <= ord(c) <= 0xDFFF for c in text)


def _strings_ok(value):
    if isinstance(value, str):
        return not _has_surrogate(value)
    if isinstance(value, list):
        return all(_strings_ok(v) for v in value)
    if isinstance(value, dict):
        return all(not _has_surrogate(k) and _strings_ok(v) for k, v in value.items())
    return True


def _depth(value):
    if isinstance(value, list):
        return 1 + max([_depth(v) for v in value] or [0])
    if isinstance(value, dict):
        return 1 + max([_depth(v) for v in value.values()] or [0])
    return 0


def _parse(data):
    try:
        text = data.decode("utf-8")
        value = json.loads(text, parse_constant=_reject_constant, parse_float=_float,
                           parse_int=_int)
    except (UnicodeDecodeError, ValueError, _NotJson, RecursionError):
        return None
    # serde_json, the Rust reader's parser, refuses a 128th level of nesting.
    if not isinstance(value, dict) or not _strings_ok(value) or _depth(value) > 127:
        return None
    return value


# The schema of section 5 for missing_field, keys in canonical order.
STR, STR_OR_NULL, NAME, STRINGS, SOURCE, KEY, PACKAGE = (
    "str", "str_or_null", "name", "strings", "source", "key", "package")


def _obj(fields):
    return ("obj", fields)


def _obj_or_null(fields):
    return ("obj_or_null", fields)


def _list(fields):
    return ("list", fields)


BOUND = [("home", STR), ("root", STR)]
ATTACHMENT = [("algorithm", STR), ("bound", _obj_or_null(BOUND)), ("bytes", STR), ("digest", STR),
              ("fidelity", STR), ("format", NAME), ("harness", STR), ("name", STR),
              ("session", STR_OR_NULL)]
CACHE = [("carry", STR), ("class", STR), ("digest", STR_OR_NULL), ("key", KEY), ("path", STR)]
EVIDENCE = [("attempt", STR), ("digest", STR), ("kind", STR)]
FIDELITY = [("level", STR), ("reason", STR_OR_NULL)]
HARNESS = [("args", STRINGS), ("kind", STR), ("program", STR_OR_NULL), ("version", STR_OR_NULL)]
JOB = [("created_by", STR), ("goal", STR), ("id", STR)]
PRODUCED = [("at_ms", STR), ("by", STR), ("writer", NAME)]
SECRETS = [("allowlist", STRINGS), ("left_behind", STRINGS), ("policy", STR)]
PLATFORM = [("arch", STR), ("os", STR)]
WORKSPACE = [("bundle_head", STR), ("epoch", STR), ("ledger_head", STR),
             ("platform", _obj(PLATFORM)), ("root", STR)]
HEADS = [("bundle_head", STR), ("ledger_head", STR)]
PACKAGE_FIELDS = [("job", STR), ("version", STR), ("workspace", _obj(HEADS))]
TOP = [("attachments", _list(ATTACHMENT)), ("caches", _list(CACHE)),
       ("evidence", _list(EVIDENCE)), ("fidelity", _obj(FIDELITY)),
       ("harness", _obj(HARNESS)), ("job", _obj(JOB)), ("package", PACKAGE),
       ("produced", _obj(PRODUCED)), ("psf", STR), ("requires", STRINGS),
       ("secrets", _obj(SECRETS)), ("source", SOURCE), ("workspace", _obj(WORKSPACE))]

_ABSENT = object()


def _fields(obj, fields, path):
    for key, want in fields:
        _node(obj.get(key, _ABSENT), want, _child(path, key))


def _node(value, want, path):
    def missing():
        raise Refusal("missing_field", path)

    if value is None and (want in (STR_OR_NULL, KEY, PACKAGE)
                          or (isinstance(want, tuple) and want[0] == "obj_or_null")):
        return
    if want in (STR, STR_OR_NULL):
        if not isinstance(value, str):
            missing()
    elif want == NAME:
        if not isinstance(value, str) or value == "":
            missing()
    elif want == STRINGS:
        if not isinstance(value, list):
            missing()
        for i, item in enumerate(value):
            _node(item, STR, _element(path, i))
    elif want == SOURCE:
        if not isinstance(value, dict):
            missing()
        _node(value.get("kind", _ABSENT), STR, _child(path, "kind"))
        required = {"git": ["commit", "uri"], "snapshot": ["snapshot"]}.get(value["kind"], [])
        for key in required:
            _node(value.get(key, _ABSENT), STR, _child(path, key))
    elif want == KEY:
        if not isinstance(value, dict):
            missing()
        _node(value.get("kind", _ABSENT), STR, _child(path, "kind"))
        for key in _sorted_keys(value):
            _node(value[key], STR, _child(path, key))
    elif want == PACKAGE:
        if not isinstance(value, dict):
            missing()
        _fields(value, PACKAGE_FIELDS, path)
    elif want[0] in ("obj", "obj_or_null"):
        if not isinstance(value, dict):
            missing()
        _fields(value, want[1], path)
    elif want[0] == "list":
        if not isinstance(value, list):
            missing()
        for i, item in enumerate(value):
            _node(item, _obj(want[1]), _element(path, i))


def _walk(value, path, pattern, rule):
    """The first path, in canonical order and a node before its children,
    whose value breaks rule(pattern, value); None when none does."""
    if rule(pattern, value):
        return path
    if isinstance(value, list):
        for i, item in enumerate(value):
            hit = _walk(item, _element(path, i), pattern + "[]", rule)
            if hit is not None:
                return hit
    elif isinstance(value, dict):
        for key in _sorted_keys(value):
            hit = _walk(value[key], _child(path, key), _child(pattern, key), rule)
            if hit is not None:
                return hit
    return None


def _first(value, code, rule):
    hit = _walk(value, "", "", rule)
    if hit is not None:
        raise Refusal(code, hit)


def _under_package(pattern):
    return pattern.startswith("package.") or pattern.startswith("package[")


def _utf8_len(text):
    return len(text.encode("utf-8"))


_ARRAY_CAPS = {"requires": 16, "harness.args": 64, "attachments": 64, "caches": 256,
               "evidence": 1024, "secrets.left_behind": 1024, "secrets.allowlist": 1024}


def _too_large(pattern, value):
    if isinstance(value, list):
        cap = _ARRAY_CAPS.get(pattern)
        return cap is not None and len(value) > cap
    if isinstance(value, str):
        if pattern in ("produced.writer", "attachments[].name", "attachments[].format"):
            return _utf8_len(value) > NAME_MAX_BYTES
        if pattern in ("job.goal", "fidelity.reason") or _under_package(pattern):
            return _utf8_len(value) > TEXT_MAX_BYTES
        return False
    if isinstance(value, dict) and pattern == "package":
        return _utf8_len(canonical(value)) > PACKAGE_MAX_BYTES
    return False


_ULID = re.compile(r"[0-7][0-9A-HJKMNP-TV-Z]{25}\Z")
_NODE = re.compile(r"fab1[a-z2-7]{51}[aq]\Z")
_SESSION = re.compile(r"[A-Za-z0-9_:][A-Za-z0-9._:-]{0,127}\Z")
_NAME = re.compile(r"[A-Za-z0-9._-]{1,128}\Z")
_HEX64 = re.compile(r"[0-9a-f]{64}\Z")
_COMMIT = re.compile(r"(?:[0-9a-f]{40}|[0-9a-f]{64})\Z")
_UINT = re.compile(r"(?:0|[1-9][0-9]*)\Z")
_HARNESS = re.compile(r"[a-z][a-z0-9_]{0,31}\Z")


def _uint(text):
    if not _UINT.match(text):
        return None
    n = int(text)
    return n if n <= U64_MAX else None


def _bad_id(pattern, value):
    if not isinstance(value, str):
        return False
    if pattern in ("job.id", "evidence[].attempt"):
        return not _ULID.match(value)
    if pattern in ("job.created_by", "produced.by"):
        return not _NODE.match(value)
    if pattern == "attachments[].session":
        return not _SESSION.match(value)
    if pattern == "attachments[].name":
        return not _NAME.match(value)
    if pattern in ("attachments[].digest", "workspace.bundle_head", "workspace.ledger_head",
                   "source.snapshot", "caches[].digest", "evidence[].digest"):
        return not _HEX64.match(value)
    if pattern == "source.commit":
        return not _COMMIT.match(value)
    if pattern in ("produced.at_ms", "attachments[].bytes"):
        return _uint(value) is None
    if pattern == "workspace.epoch":
        n = _uint(value)
        return n is None or n == 0
    return False


def _components_ok(text):
    if "\0" in text:
        return False
    for c in text.split("/"):
        if c in ("", ".", "..") or c.lower() == ".git":
            return False
    return unicodedata.is_normalized("NFC", text)


def _abs_path(text):
    if not text.startswith("/"):
        return False
    rest = text[1:]
    return rest == "" or _components_ok(rest)


def _rel_path(text):
    return text != "" and not text.startswith("/") and _components_ok(text)


def _bad_path(pattern, value):
    if not isinstance(value, str):
        return False
    if pattern in ("workspace.root", "attachments[].bound.root", "attachments[].bound.home"):
        return not _abs_path(value)
    if pattern in ("caches[].path", "secrets.left_behind[]", "secrets.allowlist[]"):
        return not _rel_path(value)
    return False


_ENUMS = {
    "source.kind": ("empty", "git", "snapshot"),
    "caches[].class": ("build", "deps", "tool_cache"),
    "caches[].carry": ("carried", "not_carried", "partial"),
    "attachments[].fidelity": ("native_session", "new_native_session_with_package"),
    "attachments[].algorithm": ("sha256",),
    "evidence[].kind": ("checkpoint", "exit", "receipt"),
    "fidelity.level": ("native_session", "new_native_session_with_package", "unsupported",
                       "workspace_only"),
}


def _bad_enum(pattern, value):
    if not isinstance(value, str):
        return False
    if pattern in ("harness.kind", "attachments[].harness"):
        return not _HARNESS.match(value)
    allowed = _ENUMS.get(pattern)
    return allowed is not None and value not in allowed


def _duplicates(top):
    for name, key in (("attachments", "name"), ("caches", "path")):
        items = top[name]
        for i in range(1, len(items)):
            if items[i - 1][key].encode("utf-8") >= items[i][key].encode("utf-8"):
                raise Refusal("duplicate", _child(_element(name, i), key))


# fabric_core::credential::looks_like_credential, shape for shape.
def _alnum(b):
    return 0x30 <= b <= 0x39 or 0x41 <= b <= 0x5A or 0x61 <= b <= 0x7A


def _alnum_dash_underscore(b):
    return _alnum(b) or b in (0x2D, 0x5F)


def _upper_alnum(b):
    return 0x30 <= b <= 0x39 or 0x41 <= b <= 0x5A


TOKEN_SHAPES = [
    ("ghp_", 36, _alnum), ("gho_", 36, _alnum), ("ghu_", 36, _alnum), ("ghs_", 36, _alnum),
    ("ghr_", 36, _alnum), ("github_pat_", 22, _alnum_dash_underscore),
    ("glpat-", 20, _alnum_dash_underscore), ("npm_", 36, _alnum),
    ("xoxb-", 10, _alnum_dash_underscore), ("xoxp-", 10, _alnum_dash_underscore),
    ("sk-", 20, _alnum_dash_underscore), ("AKIA", 16, _upper_alnum),
    ("sk_live_", 24, _alnum), ("sk_test_", 24, _alnum), ("rk_live_", 24, _alnum),
    ("rk_test_", 24, _alnum), ("AIza", 35, _alnum_dash_underscore), ("hf_", 34, _alnum),
    ("pypi-", 50, _alnum_dash_underscore), ("glptt-", 20, _alnum_dash_underscore),
]


def _has_token_shape(text):
    data = text.encode("utf-8")
    for prefix, min_len, allowed in TOKEN_SHAPES:
        p = prefix.encode("utf-8")
        start = data.find(p)
        while start != -1:
            starts_a_word = start == 0 or not _alnum_dash_underscore(data[start - 1])
            n = 0
            i = start + len(p)
            while i < len(data) and allowed(data[i]):
                n += 1
                i += 1
            if starts_a_word and n >= min_len:
                return True
            start = data.find(p, start + 1)
    return False


def _url_has_password(text):
    start = text.find("://")
    while start != -1:
        rest = text[start + 3:]
        authority = re.split(r"[/?#]", rest, maxsplit=1)[0]
        at = authority.rfind("@")
        if at != -1 and ":" in authority[:at]:
            return True
        start = text.find("://", start + 1)
    return False


def looks_like_credential(text):
    return "-----BEGIN" in text or _url_has_password(text) or _has_token_shape(text)


def _credential(value, path):
    if isinstance(value, str):
        if looks_like_credential(value):
            raise Refusal("credential_like", path)
    elif isinstance(value, list):
        for i, item in enumerate(value):
            _credential(item, _element(path, i))
    elif isinstance(value, dict):
        for key in _sorted_keys(value):
            at = _child(path, key)
            if looks_like_credential(key):
                raise Refusal("credential_like", at)
            _credential(value[key], at)


_RANK = {"native_session": 2, "new_native_session_with_package": 1}
_DERIVED = {2: "native_session", 1: "new_native_session_with_package", 0: "workspace_only"}


def _fidelity(top):
    kind = top["harness"]["kind"]
    attachments = top["attachments"]
    for i, a in enumerate(attachments):
        if a["fidelity"] == "native_session" and a["harness"] != kind:
            raise Refusal("fidelity_mismatch", _child(_element("attachments", i), "harness"))
    level = top["fidelity"]["level"]
    has_reason = top["fidelity"]["reason"] is not None
    if level == "unsupported":
        if attachments:
            raise Refusal("fidelity_mismatch", "fidelity.level")
        if not has_reason:
            raise Refusal("fidelity_mismatch", "fidelity.reason")
    else:
        if has_reason:
            raise Refusal("fidelity_mismatch", "fidelity.reason")
        best = max([_RANK.get(a["fidelity"], 0) for a in attachments] or [0])
        if level != _DERIVED[best]:
            raise Refusal("fidelity_mismatch", "fidelity.level")
    if level == "new_native_session_with_package" and top["package"] is None:
        raise Refusal("fidelity_mismatch", "package")


def _package(top):
    package = top["package"]
    if package is None:
        return
    pairs = [(package["version"], "1", "package.version"),
             (package["job"], top["job"]["id"], "package.job"),
             (package["workspace"]["bundle_head"], top["workspace"]["bundle_head"],
              "package.workspace.bundle_head"),
             (package["workspace"]["ledger_head"], top["workspace"]["ledger_head"],
              "package.workspace.ledger_head")]
    for got, want, field in pairs:
        if got != want:
            raise Refusal("package_mismatch", field)


def read_manifest(data):
    """Checks one manifest's bytes by section 10 in order; returns the
    parsed value, or raises Refusal(code, field)."""
    if len(data) > MANIFEST_MAX_BYTES:
        raise Refusal("too_large", "")
    value = _parse(data)
    if value is None:
        raise Refusal("not_json", "")
    if "psf" in value and value["psf"] != "1":
        raise Refusal("unsupported_version", "psf")
    try:
        text = canonical(value)
    except _NoForm as e:
        raise Refusal("not_canonical", e.path)
    if text.encode("utf-8") != data:
        raise Refusal("not_canonical", "")
    _fields(value, TOP, "")
    if value["requires"]:
        raise Refusal("unknown_requirement", "requires[0]")
    _first(value, "too_large", _too_large)
    _first(value, "bad_id", _bad_id)
    _first(value, "bad_path", _bad_path)
    _first(value, "bad_enum", _bad_enum)
    _duplicates(value)
    if value["secrets"]["policy"] != "never_inside":
        raise Refusal("secret_policy", "secrets.policy")
    _credential(value, "")
    _fidelity(value)
    _package(value)
    return value


def digest(data):
    return hashlib.sha256(data).hexdigest()


def _entries(root):
    out = []
    for dirpath, dirnames, filenames in os.walk(root):
        rel_dir = os.path.relpath(dirpath, root)
        for name in dirnames + filenames:
            rel = name if rel_dir == "." else rel_dir.replace(os.sep, "/") + "/" + name
            out.append(rel)
    return sorted(out, key=lambda p: p.encode("utf-8", "surrogateescape"))


def read_container(path):
    """Reads a container directory; returns (manifest, present, detached) or
    raises ContainerRefusal(code, path, field)."""
    manifest_path = os.path.join(path, "manifest.json")
    if not (os.path.isfile(manifest_path) and not os.path.islink(manifest_path)):
        raise ContainerRefusal("manifest_missing", "manifest.json")
    with open(manifest_path, "rb") as f:
        data = f.read(MANIFEST_MAX_BYTES + 1)
    try:
        manifest = read_manifest(data)
    except Refusal as r:
        raise ContainerRefusal(r.code, "manifest.json", r.field)
    attachments = manifest["attachments"]
    found = set()
    for rel in _entries(path):
        full = os.path.join(path, rel)
        if rel == "manifest.json" or (rel == "blobs" and os.path.isdir(full)
                                      and not os.path.islink(full)):
            continue
        name = rel[len("blobs/"):] if rel.startswith("blobs/") else None
        is_file = os.path.isfile(full) and not os.path.islink(full)
        named = [a for a in attachments if a["digest"] == name] if (
            name is not None and "/" not in name and is_file) else []
        if not named:
            raise ContainerRefusal("blob_unlisted", rel)
        with open(full, "rb") as f:
            blob = f.read()
        if digest(blob) != name or any(int(a["bytes"]) != len(blob) for a in named):
            raise ContainerRefusal("blob_mismatch", rel)
        found.add(name)
    present = [a["name"] for a in attachments if a["digest"] in found]
    detached = [a["name"] for a in attachments if a["digest"] not in found]
    return manifest, present, detached


def _version(text):
    """(core, pre) of `core[-pre][+build]`, or None; achieve.rs Version."""
    text = text.split("+", 1)[0]
    core, sep, pre = text.partition("-")
    def number(p):
        if not re.match(r"[0-9]+\Z", p) or int(p) > U64_MAX:
            raise ValueError(p)
        return int(p)
    try:
        c = [number(p) for p in core.split(".")]
        idents = []
        for p in (pre.split(".") if sep else []):
            if not re.match(r"[0-9A-Za-z-]+\Z", p):
                return None
            # Digits before letters, as semver orders them.
            idents.append((0, number(p), b"") if re.match(r"[0-9]+\Z", p) else (1, 0, p.encode()))
    except ValueError:
        return None
    return c, idents


def _not_older(have, want):
    h, w = _version(have), _version(want)
    if h is None or w is None:
        return None
    n = max(len(h[0]), len(w[0]))
    hc, wc = h[0] + [0] * (n - len(h[0])), w[0] + [0] * (n - len(w[0]))
    if hc != wc:
        return hc > wc
    if not h[1] or not w[1]:
        # A release is newer than any of its pre-releases.
        return not h[1]
    return h[1] >= w[1]


def achievable(manifest, target, detached=()):
    """(offered, achieved, why) on target {"root", "home", "harnesses":
    [(kind, version or None)]}; crates/fabric-psf/src/achieve.rs."""
    offered = manifest["fidelity"]["level"]
    why = []
    if offered == "unsupported":
        return offered, offered, ["the writer offers nothing: " + manifest["fidelity"]["reason"]]
    installed = dict(target["harnesses"])
    want = manifest["harness"]["version"]
    for a in manifest["attachments"]:
        if a["fidelity"] != "native_session":
            continue
        kind = a["harness"]
        if a["name"] in detached:
            why.append("attachment %s is detached" % a["name"])
        elif kind not in installed:
            why.append("%s is not installed here" % kind)
        elif want is not None and not (installed[kind] is not None
                                       and _not_older(installed[kind], want)):
            why.append("%s here is older than %s or not comparable" % (kind, want))
        elif a["format"] != HARNESS_STATE_FORMAT:
            why.append("attachment %s is %s, not read natively here" % (a["name"], a["format"]))
        elif a["bound"] is not None and (a["bound"]["root"] != target["root"]
                                         or a["bound"]["home"] != target["home"]):
            why.append("attachment %s is bound to another root or home" % a["name"])
        else:
            return offered, "native_session", why
    if manifest["package"] is not None:
        for a in manifest["attachments"]:
            if a["name"] not in detached and a["harness"] in installed:
                return offered, "new_native_session_with_package", why
        if manifest["attachments"]:
            why.append("no present attachment is for a harness installed here")
    elif manifest["attachments"]:
        why.append("the document has no work package to start a new session from")
    return offered, "workspace_only", why


FIXTURE_TARGET = {
    "root": "/Users/me/fabric/jobs/01JABCDEFGHJKMNPQRSTVWXYZ0/work",
    "home": "/Users/me",
    "harnesses": [("claude_code", "2.1.294"), ("codex", "0.163.0")],
}


def _line(obj):
    return json.dumps(obj, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def case_verdict(name, data):
    try:
        m = read_manifest(data)
    except Refusal as r:
        return {"code": r.code, "field": r.field, "name": name, "outcome": "refuse"}
    return {"achieved": achievable(m, FIXTURE_TARGET)[1], "digest": digest(data),
            "fidelity": m["fidelity"]["level"], "name": name, "outcome": "accept",
            "package": m["package"] is not None}


def container_verdict(name, path):
    try:
        m, _present, detached = read_container(path)
    except ContainerRefusal as r:
        return {"code": r.code, "field": r.field, "name": name, "outcome": "refuse",
                "path": r.path}
    with open(os.path.join(path, "manifest.json"), "rb") as f:
        d = digest(f.read())
    return {"achieved": achievable(m, FIXTURE_TARGET, detached)[1], "detached": detached,
            "digest": d, "name": name, "outcome": "accept"}


def fixture_verdicts(root):
    """(verdict, expect) per fixture: cases by file name, then containers,
    each read from a copy without its expect.json."""
    out = []
    cases = os.path.join(root, "cases")
    for file in sorted(os.listdir(cases)):
        if not file.endswith(".json"):
            continue
        with open(os.path.join(cases, file), "rb") as f:
            case = json.loads(f.read().decode("utf-8"))
        name = "cases/" + file[:-len(".json")]
        out.append((case_verdict(name, case["input"].encode("utf-8")), case["expect"]))
    containers = os.path.join(root, "containers")
    for d in sorted(os.listdir(containers)):
        src = os.path.join(containers, d)
        with open(os.path.join(src, "expect.json"), "rb") as f:
            expect = json.loads(f.read().decode("utf-8"))["expect"]
        scratch = tempfile.mkdtemp()
        try:
            copy = os.path.join(scratch, d)
            shutil.copytree(src, copy, ignore=lambda at, names: (
                ["expect.json"] if at == src else []))
            out.append((container_verdict("containers/" + d, copy), expect))
        finally:
            shutil.rmtree(scratch)
    return out


def _agrees(verdict, expect):
    keys = (["digest", "fidelity", "package"] if verdict["name"].startswith("cases/")
            else ["digest", "detached"]) if expect["outcome"] == "accept" else (
        ["code", "field"] if verdict["name"].startswith("cases/") else ["code", "path"])
    return verdict["outcome"] == expect["outcome"] and all(verdict.get(k) == expect[k] for k in keys)


def main(argv):
    if len(argv) == 3 and argv[1] in ("--fixtures", "--verdicts"):
        for verdict, expect in fixture_verdicts(argv[2]):
            if argv[1] == "--verdicts":
                print(_line(verdict))
            elif not _agrees(verdict, expect):
                print("disagree: %s %s, expected %s" % (verdict["name"], _line(verdict),
                                                         _line(expect)))
                return 1
        if argv[1] == "--fixtures":
            print("ok")
        return 0
    if len(argv) == 3 and argv[1] == "--lines":
        with open(argv[2], "rb") as f:
            inputs = [json.loads(line) for line in f.read().decode("utf-8").splitlines() if line]
        for i, text in enumerate(inputs):
            data = text.encode("utf-8", "surrogatepass")
            print(_line(case_verdict("lines/%d" % i, data)))
        return 0
    if len(argv) != 2:
        sys.stderr.write(__doc__)
        return 2
    path = argv[1]
    try:
        if os.path.isdir(path):
            m, _present, detached = read_container(path)
            with open(os.path.join(path, "manifest.json"), "rb") as f:
                d = digest(f.read())
        else:
            with open(path, "rb") as f:
                data = f.read(MANIFEST_MAX_BYTES + 1)
            m, detached, d = read_manifest(data), [], digest(data)
    except Refusal as r:
        print("%s %s" % (r.code, r.field))
        return 1
    except ContainerRefusal as r:
        print(" ".join(x for x in (r.code, r.path, r.field) if x))
        return 1
    print("digest %s" % d)
    print("offered %s" % m["fidelity"]["level"])
    print("detached %s" % (" ".join(detached) if detached else "none"))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

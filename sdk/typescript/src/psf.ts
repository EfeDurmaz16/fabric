// A Portable Session Format v1 reader (ADR 0029,
// docs/portable-session-format.md). It shares no code with the Rust crate
// (crates/fabric-psf) or the Python reader (scripts/psf_check.py), and all
// three give the same verdict, code and field for every document: this file
// follows crates/fabric-psf/src/check.rs phase by phase, and
// test/psf.test.ts diffs the three over the fixtures in protocol/psf/v1 and
// over documents generated from them.
//
//   const { manifest, detached } = await readContainer("/tmp/job.psf");
//   const { offered, achieved, why } = achievable(manifest, {
//     root: "/Users/me/work", home: "/Users/me", harnesses: [["claude_code", "2.1.294"]],
//   }, detached);

import { createHash } from "node:crypto";
import { lstat, open, readdir, readFile } from "node:fs/promises";
import { join } from "node:path";

import { z } from "zod";

import { FabricError } from "./errors.ts";

/** The manifest cap: the client API's request parse cap. */
export const PSF_MANIFEST_MAX_BYTES = 1 << 20;
/** The cap on a text field. */
export const PSF_TEXT_MAX_BYTES = 16 * 1024;
/** The cap on the canonical JSON of `package`. */
export const PSF_PACKAGE_MAX_BYTES = 256 * 1024;
/** The cap on a name. */
export const PSF_NAME_MAX_BYTES = 128;
/** The attachment format Fabric's harness adapters read natively. */
export const HARNESS_STATE_FORMAT = "fabric.harness_state.v1";

/** The manifest refusal codes of section 10, in the order a reader checks them; closed in v1. */
export const PSF_CODES = [
  "too_large",
  "not_json",
  "not_canonical",
  "unsupported_version",
  "missing_field",
  "unknown_requirement",
  "bad_id",
  "bad_path",
  "bad_enum",
  "duplicate",
  "secret_policy",
  "credential_like",
  "fidelity_mismatch",
  "package_mismatch",
] as const;
export type PsfCode = (typeof PSF_CODES)[number];

/** The container refusal codes of section 10; closed in v1. */
export const PSF_CONTAINER_CODES = ["manifest_missing", "blob_mismatch", "blob_unlisted"] as const;
export type PsfContainerCode = (typeof PSF_CONTAINER_CODES)[number];

/** A refused manifest: the first rule it breaks and where. */
export class PsfError extends FabricError {
  override name = "PsfError";
  readonly code: PsfCode;
  /** The dotted path of the value, `[i]` for an index; empty for the whole document. */
  readonly field: string;

  constructor(code: PsfCode, field: string) {
    super(field === "" ? `psf refused: ${code}` : `psf refused: ${code} at ${field}`);
    this.code = code;
    this.field = field;
  }
}

/**
 * A refused container: a container code with the path inside it, or the
 * manifest's own code with `path` `manifest.json` and the manifest field.
 */
export class PsfContainerError extends FabricError {
  override name = "PsfContainerError";
  readonly code: PsfContainerCode | PsfCode;
  readonly path: string;
  readonly field: string;

  constructor(code: PsfContainerCode | PsfCode, path: string, field = "") {
    super(`psf container refused: ${code} ${path}${field === "" ? "" : ` ${field}`}`);
    this.code = code;
    this.path = path;
    this.field = field;
  }
}

/** The fidelity ladder of ADR 0024. */
export const Fidelity = z.enum(["native_session", "new_native_session_with_package", "workspace_only", "unsupported"]);
export type Fidelity = z.infer<typeof Fidelity>;

const AttachmentFidelity = z.enum(["native_session", "new_native_session_with_package"]);

// The typed view of a manifest the rules accepted. The rules decide what is
// valid; this schema only gives the accepted value its type, and keeps keys
// a later v1 writer adds.
export const PsfManifest = z.looseObject({
  psf: z.literal("1"),
  requires: z.array(z.string()),
  job: z.looseObject({ id: z.string(), goal: z.string(), created_by: z.string() }),
  produced: z.looseObject({ at_ms: z.string(), by: z.string(), writer: z.string() }),
  workspace: z.looseObject({
    root: z.string(),
    bundle_head: z.string(),
    ledger_head: z.string(),
    epoch: z.string(),
    platform: z.looseObject({ os: z.string(), arch: z.string() }),
  }),
  source: z.looseObject({
    kind: z.enum(["git", "snapshot", "empty"]),
    uri: z.string().optional(),
    commit: z.string().optional(),
    snapshot: z.string().optional(),
  }),
  harness: z.looseObject({
    kind: z.string(),
    program: z.string().nullable(),
    args: z.array(z.string()),
    version: z.string().nullable(),
  }),
  fidelity: z.looseObject({ level: Fidelity, reason: z.string().nullable() }),
  attachments: z.array(
    z.looseObject({
      name: z.string(),
      harness: z.string(),
      format: z.string(),
      fidelity: AttachmentFidelity,
      session: z.string().nullable(),
      bound: z.looseObject({ root: z.string(), home: z.string() }).nullable(),
      algorithm: z.literal("sha256"),
      digest: z.string(),
      bytes: z.string(),
    }),
  ),
  caches: z.array(
    z.looseObject({
      path: z.string(),
      class: z.enum(["deps", "build", "tool_cache"]),
      carry: z.enum(["carried", "partial", "not_carried"]),
      key: z.record(z.string(), z.string()).nullable(),
      digest: z.string().nullable(),
    }),
  ),
  evidence: z.array(
    z.looseObject({ attempt: z.string(), digest: z.string(), kind: z.enum(["checkpoint", "exit", "receipt"]) }),
  ),
  package: z.looseObject({ version: z.string(), job: z.string() }).nullable(),
  secrets: z.looseObject({
    policy: z.literal("never_inside"),
    left_behind: z.array(z.string()),
    allowlist: z.array(z.string()),
  }),
});
export type PsfManifest = z.infer<typeof PsfManifest>;

/** A manifest the rules accepted, with its document digest. */
export interface Manifest {
  /** The document. */
  readonly value: PsfManifest;
  /** sha256 of the bytes read, lowercase hex. */
  readonly digest: string;
}

/** A container the rules accepted. */
export interface Container {
  readonly manifest: Manifest;
  /** Attachments whose blob is present and matches, in manifest order. */
  readonly present: string[];
  /** Attachments whose blob is absent, in manifest order. */
  readonly detached: string[];
}

/** Where a document would be continued. */
export interface Target {
  readonly root: string;
  readonly home: string;
  /** Installed harnesses by kind, with the version each prints, or null when unknown. */
  readonly harnesses: ReadonlyArray<readonly [string, string | null]>;
}

/** The offered level, the one achieved on a target, and why it fell. */
export interface Achieved {
  readonly offered: Fidelity;
  readonly achieved: Fidelity;
  readonly why: string[];
}

type Json = null | boolean | number | string | Json[] | { [key: string]: Json };
type JsonObject = { [key: string]: Json };

const encoder = new TextEncoder();
const utf8Len = (text: string): number => encoder.encode(text).length;
const has = (obj: JsonObject, key: string): boolean => Object.hasOwn(obj, key);
const get = (obj: JsonObject, key: string): Json | undefined => (has(obj, key) ? obj[key] : undefined);
const isObject = (value: Json | undefined): value is JsonObject =>
  typeof value === "object" && value !== null && !Array.isArray(value);
const compareBytes = (a: string, b: string): number => Buffer.compare(Buffer.from(a, "utf8"), Buffer.from(b, "utf8"));
const sortedKeys = (obj: JsonObject): string[] => Object.keys(obj).sort(compareBytes);
const child = (path: string, key: string): string => (path === "" ? key : `${path}.${key}`);
const element = (path: string, index: number): string => `${path}[${index}]`;

class NoForm {
  readonly path: string;

  constructor(path: string) {
    this.path = path;
  }
}

function writeString(text: string): string {
  let out = '"';
  for (const c of text) {
    const n = c.codePointAt(0) ?? 0;
    if (c === '"') out += '\\"';
    else if (c === "\\") out += "\\\\";
    else if (c === "\n") out += "\\n";
    else if (c === "\r") out += "\\r";
    else if (c === "\t") out += "\\t";
    else if (n < 0x20) out += `\\u${n.toString(16).padStart(4, "0")}`;
    else out += c;
  }
  return `${out}"`;
}

const KEY = /^[a-z0-9_]+$/;

/** The canonical text of a value (section 4); throws NoForm at the first JSON number or bad key. */
function canonical(value: Json, path = ""): string {
  if (value === null) return "null";
  if (value === true) return "true";
  if (value === false) return "false";
  if (typeof value === "number") throw new NoForm(path);
  if (typeof value === "string") return writeString(value);
  if (Array.isArray(value)) return `[${value.map((v, i) => canonical(v, element(path, i))).join(",")}]`;
  const parts: string[] = [];
  for (const key of sortedKeys(value)) {
    const at = child(path, key);
    if (!KEY.test(key)) throw new NoForm(at);
    parts.push(`${writeString(key)}:${canonical(value[key] as Json, at)}`);
  }
  return `{${parts.join(",")}}`;
}

const LONE_SURROGATE = /[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/;

/** False when a string or key holds a lone surrogate, or nesting reaches 128 levels, which serde_json refuses. */
function wellFormed(root: Json): boolean {
  const stack: Array<[Json, number]> = [[root, 1]];
  while (stack.length > 0) {
    const [value, depth] = stack.pop() as [Json, number];
    if (typeof value === "string") {
      if (LONE_SURROGATE.test(value)) return false;
    } else if (Array.isArray(value)) {
      if (depth > 127) return false;
      for (const item of value) stack.push([item, depth + 1]);
    } else if (isObject(value)) {
      if (depth > 127) return false;
      for (const key of Object.keys(value)) {
        if (LONE_SURROGATE.test(key)) return false;
        stack.push([value[key] as Json, depth + 1]);
      }
    }
  }
  return true;
}

function parse(bytes: Uint8Array): JsonObject | undefined {
  let value: Json;
  try {
    const text = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(bytes);
    value = JSON.parse(text) as Json;
  } catch {
    return undefined;
  }
  return isObject(value) && wellFormed(value) ? value : undefined;
}

// The schema of section 5 for missing_field, keys in canonical order.
type Want =
  | "str"
  | "strOrNull"
  | "name"
  | "strings"
  | "source"
  | "key"
  | "package"
  | { obj: Fields; nullable: boolean }
  | { list: Fields };
type Fields = ReadonlyArray<readonly [string, Want]>;
const obj = (fields: Fields): Want => ({ obj: fields, nullable: false });
const BOUND: Fields = [
  ["home", "str"],
  ["root", "str"],
];
const ATTACHMENT: Fields = [
  ["algorithm", "str"],
  ["bound", { obj: BOUND, nullable: true }],
  ["bytes", "str"],
  ["digest", "str"],
  ["fidelity", "str"],
  ["format", "name"],
  ["harness", "str"],
  ["name", "str"],
  ["session", "strOrNull"],
];
const CACHE: Fields = [
  ["carry", "str"],
  ["class", "str"],
  ["digest", "strOrNull"],
  ["key", "key"],
  ["path", "str"],
];
const EVIDENCE: Fields = [
  ["attempt", "str"],
  ["digest", "str"],
  ["kind", "str"],
];
const HEADS: Fields = [
  ["bundle_head", "str"],
  ["ledger_head", "str"],
];
const PACKAGE: Fields = [
  ["job", "str"],
  ["version", "str"],
  ["workspace", obj(HEADS)],
];
const TOP: Fields = [
  ["attachments", { list: ATTACHMENT }],
  ["caches", { list: CACHE }],
  ["evidence", { list: EVIDENCE }],
  [
    "fidelity",
    obj([
      ["level", "str"],
      ["reason", "strOrNull"],
    ]),
  ],
  [
    "harness",
    obj([
      ["args", "strings"],
      ["kind", "str"],
      ["program", "strOrNull"],
      ["version", "strOrNull"],
    ]),
  ],
  [
    "job",
    obj([
      ["created_by", "str"],
      ["goal", "str"],
      ["id", "str"],
    ]),
  ],
  ["package", "package"],
  [
    "produced",
    obj([
      ["at_ms", "str"],
      ["by", "str"],
      ["writer", "name"],
    ]),
  ],
  ["psf", "str"],
  ["requires", "strings"],
  [
    "secrets",
    obj([
      ["allowlist", "strings"],
      ["left_behind", "strings"],
      ["policy", "str"],
    ]),
  ],
  ["source", "source"],
  [
    "workspace",
    obj([
      ["bundle_head", "str"],
      ["epoch", "str"],
      ["ledger_head", "str"],
      [
        "platform",
        obj([
          ["arch", "str"],
          ["os", "str"],
        ]),
      ],
      ["root", "str"],
    ]),
  ],
];

function fields(map: JsonObject, want: Fields, path: string): void {
  for (const [key, w] of want) node(get(map, key), w, child(path, key));
}

function node(value: Json | undefined, want: Want, path: string): void {
  const missing = (): never => {
    throw new PsfError("missing_field", path);
  };
  if (value === null) {
    if (want === "strOrNull" || want === "key" || want === "package") return;
    if (typeof want === "object" && "obj" in want && want.nullable) return;
  }
  if (want === "str" || want === "strOrNull") {
    if (typeof value !== "string") missing();
  } else if (want === "name") {
    if (typeof value !== "string" || value === "") missing();
  } else if (want === "strings") {
    if (!Array.isArray(value)) return missing();
    value.forEach((item, i) => node(item, "str", element(path, i)));
  } else if (want === "source") {
    if (!isObject(value)) return missing();
    node(get(value, "kind"), "str", child(path, "kind"));
    const kind = get(value, "kind");
    const required = kind === "git" ? ["commit", "uri"] : kind === "snapshot" ? ["snapshot"] : [];
    for (const key of required) node(get(value, key), "str", child(path, key));
  } else if (want === "key") {
    if (!isObject(value)) return missing();
    node(get(value, "kind"), "str", child(path, "kind"));
    for (const key of sortedKeys(value)) node(value[key], "str", child(path, key));
  } else if (want === "package") {
    if (!isObject(value)) return missing();
    fields(value, PACKAGE, path);
  } else if ("obj" in want) {
    if (!isObject(value)) return missing();
    fields(value, want.obj, path);
  } else {
    if (!Array.isArray(value)) return missing();
    value.forEach((item, i) => node(item, obj(want.list), element(path, i)));
  }
}

type Rule = (pattern: string, value: Json) => boolean;

/** The first path, in canonical order and a node before its children, whose value breaks `rule`. */
function walk(value: Json, path: string, pattern: string, rule: Rule): string | undefined {
  if (rule(pattern, value)) return path;
  if (Array.isArray(value)) {
    for (let i = 0; i < value.length; i++) {
      const hit = walk(value[i] as Json, element(path, i), `${pattern}[]`, rule);
      if (hit !== undefined) return hit;
    }
  } else if (isObject(value)) {
    for (const key of sortedKeys(value)) {
      const hit = walk(value[key] as Json, child(path, key), child(pattern, key), rule);
      if (hit !== undefined) return hit;
    }
  }
  return undefined;
}

function first(value: Json, code: PsfCode, rule: Rule): void {
  const hit = walk(value, "", "", rule);
  if (hit !== undefined) throw new PsfError(code, hit);
}

const underPackage = (pattern: string): boolean => pattern.startsWith("package.") || pattern.startsWith("package[");

const ARRAY_CAPS = new Map<string, number>([
  ["requires", 16],
  ["harness.args", 64],
  ["attachments", 64],
  ["caches", 256],
  ["evidence", 1024],
  ["secrets.left_behind", 1024],
  ["secrets.allowlist", 1024],
]);

const tooLarge: Rule = (pattern, value) => {
  if (Array.isArray(value)) {
    const cap = ARRAY_CAPS.get(pattern);
    return cap !== undefined && value.length > cap;
  }
  if (typeof value === "string") {
    if (pattern === "produced.writer" || pattern === "attachments[].name" || pattern === "attachments[].format") {
      return utf8Len(value) > PSF_NAME_MAX_BYTES;
    }
    if (pattern === "job.goal" || pattern === "fidelity.reason" || underPackage(pattern)) {
      return utf8Len(value) > PSF_TEXT_MAX_BYTES;
    }
    return false;
  }
  return isObject(value) && pattern === "package" && utf8Len(canonical(value)) > PSF_PACKAGE_MAX_BYTES;
};

const ULID = /^[0-7][0-9A-HJKMNP-TV-Z]{25}$/;
const NODE = /^fab1[a-z2-7]{51}[aq]$/;
const SESSION = /^[A-Za-z0-9_:][A-Za-z0-9._:-]{0,127}$/;
const NAME = /^[A-Za-z0-9._-]{1,128}$/;
const HEX64 = /^[0-9a-f]{64}$/;
const COMMIT = /^(?:[0-9a-f]{40}|[0-9a-f]{64})$/;
const UINT = /^(?:0|[1-9][0-9]*)$/;
const HARNESS_KIND = /^[a-z][a-z0-9_]{0,31}$/;
const U64_MAX = 18446744073709551615n;

function uint(text: string): bigint | undefined {
  if (!UINT.test(text)) return undefined;
  const n = BigInt(text);
  return n <= U64_MAX ? n : undefined;
}

const HEX_FIELDS = new Set([
  "attachments[].digest",
  "workspace.bundle_head",
  "workspace.ledger_head",
  "source.snapshot",
  "caches[].digest",
  "evidence[].digest",
]);

const badId: Rule = (pattern, value) => {
  if (typeof value !== "string") return false;
  switch (pattern) {
    case "job.id":
    case "evidence[].attempt":
      return !ULID.test(value);
    case "job.created_by":
    case "produced.by":
      return !NODE.test(value);
    case "attachments[].session":
      return !SESSION.test(value);
    case "attachments[].name":
      return !NAME.test(value);
    case "source.commit":
      return !COMMIT.test(value);
    case "produced.at_ms":
    case "attachments[].bytes":
      return uint(value) === undefined;
    case "workspace.epoch": {
      const n = uint(value);
      return n === undefined || n === 0n;
    }
    default:
      return HEX_FIELDS.has(pattern) && !HEX64.test(value);
  }
};

function componentsOk(text: string): boolean {
  if (text.includes("\0")) return false;
  for (const c of text.split("/")) {
    if (c === "" || c === "." || c === ".." || c.toLowerCase() === ".git") return false;
  }
  return text.normalize("NFC") === text;
}

const absPath = (text: string): boolean => text.startsWith("/") && (text === "/" || componentsOk(text.slice(1)));
const relPath = (text: string): boolean => text !== "" && !text.startsWith("/") && componentsOk(text);

const badPath: Rule = (pattern, value) => {
  if (typeof value !== "string") return false;
  if (pattern === "workspace.root" || pattern === "attachments[].bound.root" || pattern === "attachments[].bound.home") {
    return !absPath(value);
  }
  if (pattern === "caches[].path" || pattern === "secrets.left_behind[]" || pattern === "secrets.allowlist[]") {
    return !relPath(value);
  }
  return false;
};

const ENUMS = new Map<string, readonly string[]>([
  ["source.kind", ["empty", "git", "snapshot"]],
  ["caches[].class", ["build", "deps", "tool_cache"]],
  ["caches[].carry", ["carried", "not_carried", "partial"]],
  ["attachments[].fidelity", ["native_session", "new_native_session_with_package"]],
  ["attachments[].algorithm", ["sha256"]],
  ["evidence[].kind", ["checkpoint", "exit", "receipt"]],
  ["fidelity.level", ["native_session", "new_native_session_with_package", "unsupported", "workspace_only"]],
]);

const badEnum: Rule = (pattern, value) => {
  if (typeof value !== "string") return false;
  if (pattern === "harness.kind" || pattern === "attachments[].harness") return !HARNESS_KIND.test(value);
  const allowed = ENUMS.get(pattern);
  return allowed !== undefined && !allowed.includes(value);
};

// fabric_core::credential::looks_like_credential, shape for shape, on UTF-8 bytes.
const isAlnum = (b: number): boolean => (b >= 0x30 && b <= 0x39) || (b >= 0x41 && b <= 0x5a) || (b >= 0x61 && b <= 0x7a);
const isAlnumDashUnderscore = (b: number): boolean => isAlnum(b) || b === 0x2d || b === 0x5f;
const isUpperAlnum = (b: number): boolean => (b >= 0x30 && b <= 0x39) || (b >= 0x41 && b <= 0x5a);
const TOKEN_SHAPES: ReadonlyArray<readonly [string, number, (b: number) => boolean]> = [
  ["ghp_", 36, isAlnum],
  ["gho_", 36, isAlnum],
  ["ghu_", 36, isAlnum],
  ["ghs_", 36, isAlnum],
  ["ghr_", 36, isAlnum],
  ["github_pat_", 22, isAlnumDashUnderscore],
  ["glpat-", 20, isAlnumDashUnderscore],
  ["npm_", 36, isAlnum],
  ["xoxb-", 10, isAlnumDashUnderscore],
  ["xoxp-", 10, isAlnumDashUnderscore],
  ["sk-", 20, isAlnumDashUnderscore],
  ["AKIA", 16, isUpperAlnum],
  ["sk_live_", 24, isAlnum],
  ["sk_test_", 24, isAlnum],
  ["rk_live_", 24, isAlnum],
  ["rk_test_", 24, isAlnum],
  ["AIza", 35, isAlnumDashUnderscore],
  ["hf_", 34, isAlnum],
  ["pypi-", 50, isAlnumDashUnderscore],
  ["glptt-", 20, isAlnumDashUnderscore],
];

function hasTokenShape(text: string): boolean {
  const data = Buffer.from(text, "utf8");
  for (const [prefix, minLen, allowed] of TOKEN_SHAPES) {
    let start = data.indexOf(prefix);
    while (start !== -1) {
      const startsAWord = start === 0 || !isAlnumDashUnderscore(data[start - 1] ?? 0);
      let n = 0;
      for (let i = start + prefix.length; i < data.length && allowed(data[i] ?? 0); i++) n++;
      if (startsAWord && n >= minLen) return true;
      start = data.indexOf(prefix, start + 1);
    }
  }
  return false;
}

function urlHasPassword(text: string): boolean {
  let start = text.indexOf("://");
  while (start !== -1) {
    const authority = text.slice(start + 3).split(/[/?#]/, 1)[0] ?? "";
    const at = authority.lastIndexOf("@");
    if (at !== -1 && authority.slice(0, at).includes(":")) return true;
    start = text.indexOf("://", start + 1);
  }
  return false;
}

/** The shared token shapes of section 8: `fabric_core::credential::looks_like_credential`. */
export function looksLikeCredential(text: string): boolean {
  return text.includes("-----BEGIN") || urlHasPassword(text) || hasTokenShape(text);
}

function credential(value: Json, path: string): void {
  if (typeof value === "string") {
    if (looksLikeCredential(value)) throw new PsfError("credential_like", path);
  } else if (Array.isArray(value)) {
    value.forEach((item, i) => credential(item, element(path, i)));
  } else if (isObject(value)) {
    for (const key of sortedKeys(value)) {
      const at = child(path, key);
      if (looksLikeCredential(key)) throw new PsfError("credential_like", at);
      credential(value[key] as Json, at);
    }
  }
}

const str = (value: Json | undefined): string => (typeof value === "string" ? value : "");
const objAt = (value: Json | undefined): JsonObject => (isObject(value) ? value : {});

function checkRules(top: JsonObject): void {
  // duplicate: attachments by name, then caches by path, strictly ascending by bytes.
  for (const [list, key] of [
    ["attachments", "name"],
    ["caches", "path"],
  ] as const) {
    const items = get(top, list) as JsonObject[];
    for (let i = 1; i < items.length; i++) {
      if (compareBytes(str(get(items[i - 1] ?? {}, key)), str(get(items[i] ?? {}, key))) >= 0) {
        throw new PsfError("duplicate", child(element(list, i), key));
      }
    }
  }
  if (get(objAt(get(top, "secrets")), "policy") !== "never_inside") {
    throw new PsfError("secret_policy", "secrets.policy");
  }
  credential(top, "");
  // fidelity_mismatch: section 5.6.
  const kind = str(get(objAt(get(top, "harness")), "kind"));
  const attachments = get(top, "attachments") as JsonObject[];
  attachments.forEach((a, i) => {
    if (get(a, "fidelity") === "native_session" && get(a, "harness") !== kind) {
      throw new PsfError("fidelity_mismatch", child(element("attachments", i), "harness"));
    }
  });
  const fidelity = objAt(get(top, "fidelity"));
  const level = str(get(fidelity, "level"));
  const hasReason = get(fidelity, "reason") !== null;
  if (level === "unsupported") {
    if (attachments.length > 0) throw new PsfError("fidelity_mismatch", "fidelity.level");
    if (!hasReason) throw new PsfError("fidelity_mismatch", "fidelity.reason");
  } else {
    if (hasReason) throw new PsfError("fidelity_mismatch", "fidelity.reason");
    const rank = (f: Json | undefined): number =>
      f === "native_session" ? 2 : f === "new_native_session_with_package" ? 1 : 0;
    const best = Math.max(0, ...attachments.map((a) => rank(get(a, "fidelity"))));
    const derived = ["workspace_only", "new_native_session_with_package", "native_session"][best];
    if (level !== derived) throw new PsfError("fidelity_mismatch", "fidelity.level");
  }
  const pkg = get(top, "package");
  if (level === "new_native_session_with_package" && pkg === null) {
    throw new PsfError("fidelity_mismatch", "package");
  }
  // package_mismatch: section 5.6.
  if (isObject(pkg)) {
    const workspace = objAt(get(top, "workspace"));
    const heads = objAt(get(pkg, "workspace"));
    const pairs: Array<[Json | undefined, Json | undefined, string]> = [
      [get(pkg, "version"), "1", "package.version"],
      [get(pkg, "job"), get(objAt(get(top, "job")), "id"), "package.job"],
      [get(heads, "bundle_head"), get(workspace, "bundle_head"), "package.workspace.bundle_head"],
      [get(heads, "ledger_head"), get(workspace, "ledger_head"), "package.workspace.ledger_head"],
    ];
    for (const [got, want, field] of pairs) {
      if (got !== want) throw new PsfError("package_mismatch", field);
    }
  }
}

/** The document digest: sha256 of the bytes, lowercase hex. */
export function manifestDigest(bytes: Uint8Array): string {
  return createHash("sha256").update(bytes).digest("hex");
}

/** Reads one manifest, checked by section 10 in order; throws {@link PsfError}. */
export function readManifest(bytes: Uint8Array): Manifest {
  if (bytes.length > PSF_MANIFEST_MAX_BYTES) throw new PsfError("too_large", "");
  const top = parse(bytes);
  if (top === undefined) throw new PsfError("not_json", "");
  let text: string;
  try {
    text = canonical(top);
  } catch (e) {
    if (e instanceof NoForm) throw new PsfError("not_canonical", e.path);
    throw e;
  }
  if (Buffer.compare(encoder.encode(text), bytes) !== 0) throw new PsfError("not_canonical", "");
  if (has(top, "psf") && top.psf !== "1") throw new PsfError("unsupported_version", "psf");
  fields(top, TOP, "");
  if ((get(top, "requires") as Json[]).length > 0) throw new PsfError("unknown_requirement", "requires[0]");
  first(top, "too_large", tooLarge);
  first(top, "bad_id", badId);
  first(top, "bad_path", badPath);
  first(top, "bad_enum", badEnum);
  checkRules(top);
  return { value: PsfManifest.parse(top), digest: manifestDigest(bytes) };
}

async function entries(dir: string): Promise<Array<[string, "dir" | "file" | "other"]>> {
  const out: Array<[string, "dir" | "file" | "other"]> = [];
  const pending: string[] = [""];
  while (pending.length > 0) {
    const rel = pending.pop() as string;
    for (const entry of await readdir(join(dir, rel), { withFileTypes: true })) {
      const at = rel === "" ? entry.name : `${rel}/${entry.name}`;
      const kind = entry.isDirectory() ? "dir" : entry.isFile() ? "file" : "other";
      if (kind === "dir") pending.push(at);
      out.push([at, kind]);
    }
  }
  return out.sort((a, b) => compareBytes(a[0], b[0]));
}

/** Reads a container directory (section 3); throws {@link PsfContainerError}. */
export async function readContainer(dir: string): Promise<Container> {
  const manifestPath = join(dir, "manifest.json");
  const stat = await lstat(manifestPath).catch(() => undefined);
  if (stat === undefined || !stat.isFile()) throw new PsfContainerError("manifest_missing", "manifest.json");
  const handle = await open(manifestPath, "r");
  let bytes: Uint8Array;
  try {
    const buffer = Buffer.alloc(PSF_MANIFEST_MAX_BYTES + 1);
    const { bytesRead } = await handle.read(buffer, 0, buffer.length, 0);
    bytes = buffer.subarray(0, bytesRead);
  } finally {
    await handle.close();
  }
  let manifest: Manifest;
  try {
    manifest = readManifest(bytes);
  } catch (e) {
    if (e instanceof PsfError) throw new PsfContainerError(e.code, "manifest.json", e.field);
    throw e;
  }
  const attachments = manifest.value.attachments;
  const found = new Set<string>();
  for (const [rel, kind] of await entries(dir)) {
    if (rel === "manifest.json" || (rel === "blobs" && kind === "dir")) continue;
    const name = rel.startsWith("blobs/") ? rel.slice("blobs/".length) : undefined;
    const named =
      name !== undefined && !name.includes("/") && kind === "file" ? attachments.filter((a) => a.digest === name) : [];
    if (name === undefined || named.length === 0) throw new PsfContainerError("blob_unlisted", rel);
    const blob = await readFile(join(dir, rel));
    if (manifestDigest(blob) !== name || named.some((a) => BigInt(a.bytes) !== BigInt(blob.length))) {
      throw new PsfContainerError("blob_mismatch", rel);
    }
    found.add(name);
  }
  return {
    manifest,
    present: attachments.filter((a) => found.has(a.digest)).map((a) => a.name),
    detached: attachments.filter((a) => !found.has(a.digest)).map((a) => a.name),
  };
}

function notOlder(have: string, want: string): boolean | undefined {
  const parts = (text: string): bigint[] | undefined => {
    const out: bigint[] = [];
    for (const p of text.split(".")) {
      if (!/^[0-9]+$/.test(p)) return undefined;
      const n = BigInt(p);
      if (n > U64_MAX) return undefined;
      out.push(n);
    }
    return out;
  };
  const h = parts(have);
  const w = parts(want);
  if (h === undefined || w === undefined) return undefined;
  for (let i = 0; i < Math.max(h.length, w.length); i++) {
    const a = h[i] ?? 0n;
    const b = w[i] ?? 0n;
    if (a !== b) return a > b;
  }
  return true;
}

/** What `target` achieves with `manifest` when the attachments in `detached` are absent (section 6). */
export function achievable(manifest: Manifest, target: Target, detached: readonly string[] = []): Achieved {
  const m = manifest.value;
  const offered = m.fidelity.level;
  const why: string[] = [];
  if (offered === "unsupported") {
    return { offered, achieved: offered, why: [`the writer offers nothing: ${m.fidelity.reason ?? ""}`] };
  }
  const installed = new Map(target.harnesses.map(([kind, version]) => [kind, version] as const));
  const want = m.harness.version;
  for (const a of m.attachments) {
    if (a.fidelity !== "native_session") continue;
    const have = installed.get(a.harness);
    if (detached.includes(a.name)) why.push(`attachment ${a.name} is detached`);
    else if (!installed.has(a.harness)) why.push(`${a.harness} is not installed here`);
    else if (want !== null && !(have !== null && have !== undefined && notOlder(have, want) === true)) {
      why.push(`${a.harness} here is older than ${want} or not comparable`);
    } else if (a.format !== HARNESS_STATE_FORMAT) why.push(`attachment ${a.name} is ${a.format}, not read natively here`);
    else if (a.bound !== null && (a.bound.root !== target.root || a.bound.home !== target.home)) {
      why.push(`attachment ${a.name} is bound to another root or home`);
    } else return { offered, achieved: "native_session", why };
  }
  if (m.package !== null) {
    if (m.attachments.some((a) => !detached.includes(a.name) && installed.has(a.harness))) {
      return { offered, achieved: "new_native_session_with_package", why };
    }
    if (m.attachments.length > 0) why.push("no present attachment is for a harness installed here");
  } else if (m.attachments.length > 0) {
    why.push("the document has no work package to start a new session from");
  }
  return { offered, achieved: "workspace_only", why };
}

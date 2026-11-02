// The TypeScript half of the facade protocol v1 fixture pin (ADR 0027,
// docs/facade-protocol.md): the same checks as
// crates/fabric-client/tests/facade_fixtures.rs on the same files in
// protocol/facade/v1. Every file matches manifest.sha256, none is missing
// or extra, the manifest's own digest is the one pinned here and in the
// Rust test, every case names itself and says what to expect, counters
// travel as decimal strings, and the accepted message types are exactly
// the catalogue. The parser checks come with the M5 facade code.

import { describe, expect, test } from "bun:test";

import { FacadeRefusal, ProtocolViolation, readAnswer, readClientMessage, writeAnswer, writeClientMessage } from "../src/facade.ts";
import { decodeJson } from "../src/frame.ts";
import { createHash } from "node:crypto";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";

/** The sha256 of protocol/facade/v1/manifest.sha256; change it with the Rust pin. */
const MANIFEST_SHA256 = "2a149a1b080e97fb4efe5ecbedd1eb0f45e0c2fff4a73b5184a39444f0e78a24";

const ROOT = fileURLToPath(new URL("../../../protocol/facade/v1/", import.meta.url));

const CLIENT_TYPES = ["submit", "inspect", "watch", "cancel", "checkpoint", "continue"] as const;
const ANSWER_TYPES = ["accepted", "inspection", "events", "refused"] as const;
const REFUSAL_CODES = [
  "unsupported_version",
  "bad_request",
  "unauthorized",
  "scope",
  "unknown_job",
  "conflict",
  "revision_mismatch",
  "refused",
  "unavailable",
] as const;
const COUNTER_FIELDS = new Set([
  "revision",
  "expected_revision",
  "current_revision",
  "generation",
  "seq",
  "next",
  "after",
]);
const REQUIRED_CASES = [
  "submit_duplicate_key",
  "submit_conflict",
  "submit_revoked_token",
  "submit_expired_token",
  "inspect_scope_refused",
  "cancel_scope_refused",
  "submit_pin_outside_resource_scope",
  "watch_lagged",
  "watch_revoked_token",
  "cancel_revision_mismatch",
  "submit_repository_source_rejected",
];

type Json = null | boolean | number | string | Json[] | { [key: string]: Json };

function sha256(bytes: Uint8Array): string {
  return createHash("sha256").update(bytes).digest("hex");
}

function files(dir: string, out: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) {
      files(path, out);
    } else {
      out.push(relative(ROOT, path).split("\\").join("/"));
    }
  }
  return out;
}

function manifest(): Map<string, string> {
  const entries = new Map<string, string>();
  let previous = "";
  for (const line of readFileSync(join(ROOT, "manifest.sha256"), "utf8").split("\n")) {
    if (line === "") continue;
    const at = line.indexOf("  ");
    expect(at, `manifest line without two spaces: ${line}`).toBeGreaterThan(0);
    const digest = line.slice(0, at);
    const path = line.slice(at + 2);
    expect(digest).toMatch(/^[0-9a-f]{64}$/);
    expect(previous < path, `manifest not sorted at ${path}`).toBe(true);
    previous = path;
    entries.set(path, digest);
  }
  return entries;
}

function isObject(value: Json | undefined): value is { [key: string]: Json } {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isCanonicalDecimal(text: string): boolean {
  return /^(0|[1-9][0-9]{0,19})$/.test(text);
}

function countersAreStrings(name: string, value: Json): void {
  if (Array.isArray(value)) {
    for (const item of value) countersAreStrings(name, item);
    return;
  }
  if (!isObject(value)) return;
  for (const [key, inner] of Object.entries(value)) {
    if (COUNTER_FIELDS.has(key) && inner !== null) {
      expect(typeof inner, `${name}: ${key} is a decimal string`).toBe("string");
      expect(isCanonicalDecimal(inner as string), `${name}: ${key} = ${JSON.stringify(inner)}`).toBe(true);
    }
    countersAreStrings(name, inner);
  }
}

function cases(): Array<[string, { [key: string]: Json }]> {
  return readdirSync(join(ROOT, "cases"))
    .filter((name) => name.endsWith(".json"))
    .sort()
    .map((name) => {
      // Read as a frame's body is, so an integer written as a float is a symbol no reader takes.
      const parsed = decodeJson(readFileSync(join(ROOT, "cases", name))) as Json;
      if (!isObject(parsed)) throw new Error(`${name}: not an object`);
      return [name.slice(0, -".json".length), parsed];
    });
}

describe("facade protocol v1 fixtures", () => {
  test("the manifest is the pinned one", () => {
    const bytes = readFileSync(join(ROOT, "manifest.sha256"));
    expect(sha256(bytes), "protocol/facade/v1/manifest.sha256 changed; update the pin here and in crates/fabric-client/tests/facade_fixtures.rs together").toBe(MANIFEST_SHA256);
  });

  test("every fixture matches the manifest and none is missing or extra", () => {
    const listed = manifest();
    const found = files(ROOT)
      .filter((path) => path !== "manifest.sha256")
      .sort();
    expect(found).toEqual([...listed.keys()]);
    for (const [path, digest] of listed) {
      const bytes = readFileSync(join(ROOT, path));
      expect(sha256(bytes), `${path} does not match the manifest`).toBe(digest);
      expect(bytes.includes(13), `${path} has a carriage return`).toBe(false);
      expect(path.startsWith("cases/") && path.endsWith(".json"), `${path}: v1 holds cases only`).toBe(true);
    }
  });

  test("every case names itself and says what to expect", () => {
    const acceptedClient = new Set<string>();
    const acceptedAnswers = new Set<string>();
    let seen = 0;
    for (const [stem, c] of cases()) {
      const name = c.case;
      expect(name, `${stem}: a case is named after its file`).toBe(stem);
      const direction = c.direction;
      const message = c.message;
      const expectation = c.expect;
      if (!isObject(message) || !isObject(expectation)) throw new Error(`${stem}: message and expect are objects`);
      const kind = message.type;
      const outcome = expectation.outcome;
      if (typeof kind !== "string") throw new Error(`${stem}: type is a string`);
      if (direction === "client_to_authority") {
        const given = c.given;
        if (!isObject(given)) throw new Error(`${stem}: given`);
        expect(typeof given.now_ms).toBe("number");
        expect(Array.isArray(given.tokens)).toBe(true);
        for (const token of given.tokens as Json[]) {
          if (!isObject(token)) throw new Error(`${stem}: token`);
          expect(token.bearer).toMatch(/^[0-9a-f]{64}$/);
          expect(typeof token.token_id).toBe("string");
        }
        expect(Array.isArray(given.jobs)).toBe(true);
        expect(Array.isArray(given.operations)).toBe(true);
        if (outcome === "accept") {
          expect(CLIENT_TYPES as readonly string[]).toContain(kind);
          expect(message.facade).toBe(1);
          expect(typeof message.token).toBe("string");
          acceptedClient.add(kind as string);
          const answer = expectation.answer;
          if (!isObject(answer) || typeof answer.type !== "string") throw new Error(`${stem}: answer`);
          expect(ANSWER_TYPES as readonly string[]).toContain(answer.type);
          expect(answer.type).not.toBe("refused");
        }
      } else if (direction === "authority_to_client") {
        expect("given" in c, `${stem}: an answer has no given`).toBe(false);
        if (outcome === "accept") {
          expect(ANSWER_TYPES as readonly string[]).toContain(kind);
          expect(message.facade).toBe(1);
          acceptedAnswers.add(kind as string);
          if (kind === "refused") {
            if (typeof message.code !== "string") throw new Error(`${stem}: code`);
            expect(REFUSAL_CODES as readonly string[]).toContain(message.code);
            expect(typeof message.reason).toBe("string");
            expect("current_revision" in message).toBe(message.code === "revision_mismatch");
          }
        }
      } else {
        throw new Error(`${stem}: unknown direction ${String(direction)}`);
      }
      if (outcome === "accept") {
        countersAreStrings(stem, message);
      } else if (outcome === "reject") {
        expect(typeof expectation.reason).toBe("string");
        if (typeof expectation.code !== "string") throw new Error(`${stem}: code`);
        if (direction === "client_to_authority") {
          expect(REFUSAL_CODES as readonly string[]).toContain(expectation.code);
          expect(expectation.code).not.toBe("unavailable");
        } else {
          expect(expectation.code).toBe("protocol_violation");
        }
      } else {
        throw new Error(`${stem}: unknown outcome ${String(outcome)}`);
      }
      seen += 1;
    }
    expect([...acceptedClient].sort()).toEqual([...CLIENT_TYPES].sort());
    expect([...acceptedAnswers].sort()).toEqual([...ANSWER_TYPES].sort());
    expect(seen).toBeGreaterThanOrEqual(40);
  });

  test("the named failure modes have their cases", () => {
    const names = new Set(cases().map(([stem]) => stem));
    for (const required of REQUIRED_CASES) {
      expect(names.has(required), `missing case ${required}`).toBe(true);
    }
  });

  // The parse alone decides the schema and the version; every other code needs the authority's journal.
  const parseDecides = (code: string): boolean => code === "bad_request" || code === "unsupported_version";

  test("every client message reads as the authority reads it", () => {
    let checked = 0;
    for (const [name, c] of cases()) {
      if (c["direction"] !== "client_to_authority") continue;
      const expected = c["expect"];
      if (!isObject(expected)) throw new Error(`${name}: expect`);
      let message: ReturnType<typeof readClientMessage> | undefined;
      let refusal: FacadeRefusal | undefined;
      try {
        message = readClientMessage(c["message"]);
      } catch (e) {
        if (!(e instanceof FacadeRefusal)) throw e;
        refusal = e;
      }
      if (expected["outcome"] === "accept") {
        expect(refusal?.message, `${name}: refused`).toBeUndefined();
        if (message === undefined) throw new Error(name);
        expect(writeClientMessage(message) as Json, `${name}: writes back`).toStrictEqual(c["message"] as Json);
      } else if (parseDecides(String(expected["code"]))) {
        expect(refusal?.refusal.code, `${name}: code`).toBe(String(expected["code"]) as never);
        expect(refusal?.refusal.reason, `${name}: reason`).toBe(String(expected["reason"]));
      } else {
        expect(refusal?.message, `${name}: the parse refused what the authority decides`).toBeUndefined();
      }
      checked += 1;
    }
    expect(checked).toBeGreaterThanOrEqual(30);
  });

  test("every answer reads as a client reads it", () => {
    let checked = 0;
    for (const [name, c] of cases()) {
      if (c["direction"] !== "authority_to_client") continue;
      const expected = c["expect"];
      if (!isObject(expected)) throw new Error(`${name}: expect`);
      let answer: ReturnType<typeof readAnswer> | undefined;
      let violation: ProtocolViolation | undefined;
      try {
        answer = readAnswer(c["message"]);
      } catch (e) {
        if (!(e instanceof ProtocolViolation)) throw e;
        violation = e;
      }
      if (expected["outcome"] === "accept") {
        expect(violation?.message, `${name}: refused`).toBeUndefined();
        if (answer === undefined) throw new Error(name);
        expect(writeAnswer(answer) as Json, `${name}: writes back`).toStrictEqual(c["message"] as Json);
      } else {
        expect(expected["code"], name).toBe("protocol_violation");
        expect(violation?.reason, `${name}: reason`).toBe(String(expected["reason"]));
      }
      checked += 1;
    }
    expect(checked).toBeGreaterThanOrEqual(10);
  });
});

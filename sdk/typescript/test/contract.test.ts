// The TypeScript half of the client API's contract test: the same checks as
// crates/fabric-client/tests/contract.rs on the same files in
// protocol/fixtures. Every fixture parses and writes back to the same JSON,
// every op, event kind and refusal reason has a fixture, the messages under
// `invalid` are refused, and the messages under `tolerated` read as the
// compatibility rule of ADR 0018 says.

import { describe, expect, test } from "bun:test";
import { readdirSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import type { z } from "zod";

import {
  API_VERSION,
  Answer,
  createFrameDecoder,
  decodeJson,
  encodeFrame,
  Event,
  EVENT_KINDS,
  Hello,
  HelloAnswer,
  OPS,
  REASON_CODES,
  REASON_KIND,
  Refusal,
  Reply,
  Request,
  tagsOf,
} from "../src/index.ts";

const FIXTURES = fileURLToPath(new URL("../../../protocol/fixtures/", import.meta.url));

/** The schema each fixture directory, and each `schema` field, names. */
const SCHEMAS: Readonly<Record<string, z.ZodType>> = {
  hello: Hello,
  hello_answer: HelloAnswer,
  answer: Answer,
  request: Request,
  reply: Reply,
  event: Event,
  refusal: Refusal,
};

function schemaNamed(name: unknown): z.ZodType {
  const schema = typeof name === "string" ? SCHEMAS[name] : undefined;
  if (schema === undefined) {
    throw new Error(`no schema named ${String(name)}`);
  }
  return schema;
}

function readJson(path: string): unknown {
  return JSON.parse(readFileSync(path, "utf8"));
}

/** Every fixture in `dir`, by file name, with its JSON. */
function fixtures(dir: string): ReadonlyArray<readonly [string, unknown]> {
  const names = readdirSync(`${FIXTURES}${dir}`)
    .filter((name) => name.endsWith(".json"))
    .sort();
  expect(names.length).toBeGreaterThan(0);
  return names.map((name) => [name, readJson(`${FIXTURES}${dir}/${name}`)] as const);
}

/** A fixture file that wraps a message: `{ schema, value, reads_as? }` or `{ value, hex }`. */
function field(fixture: unknown, key: string): unknown {
  if (typeof fixture !== "object" || fixture === null || !Object.hasOwn(fixture, key)) {
    throw new Error(`the fixture has no ${key}`);
  }
  return Object.getOwnPropertyDescriptor(fixture, key)?.value;
}

/** The JSON a parsed value writes as: absent optional fields stay absent. */
function written(value: unknown): unknown {
  return JSON.parse(JSON.stringify(value));
}

/** Parses every fixture in `dir`, checks it writes back to the same JSON, and returns the values. */
function roundTrip<T extends z.ZodType>(dir: string, schema: T): Array<z.output<T>> {
  return fixtures(dir).map(([name, json]) => {
    const parsed = schema.safeParse(json);
    if (!parsed.success) {
      throw new Error(`${dir}/${name} does not parse: ${parsed.error.message}`);
    }
    expect(written(parsed.data), `${dir}/${name}`).toStrictEqual(json);
    return parsed.data;
  });
}

describe("the shared fixtures", () => {
  test("every fixture parses and writes back to the same JSON", () => {
    for (const [dir, schema] of Object.entries(SCHEMAS)) {
      roundTrip(dir, schema);
    }
  });

  test("the hello fixture pins the api version", () => {
    for (const hello of roundTrip("hello", Hello)) {
      expect(hello.api).toBe(API_VERSION);
    }
  });

  test("the welcome fixture lists every op, in order", () => {
    const welcome = roundTrip("hello_answer", HelloAnswer).find((answer) => answer.type === "welcome");
    expect(welcome?.type).toBe("welcome");
    expect(welcome?.type === "welcome" ? welcome.capabilities : []).toStrictEqual([...OPS]);
  });

  test("every op has a request and a reply fixture tagged with its name", () => {
    const requested = new Set(roundTrip("request", Request).map((request) => request.op));
    const replied = new Set(roundTrip("reply", Reply).map((reply) => reply.kind));
    expect([...requested].sort()).toStrictEqual([...OPS].sort());
    expect([...replied].sort()).toStrictEqual([...OPS].sort());
  });

  test("the schemas know exactly the ops of this api version", () => {
    expect(tagsOf("op", Request.options)).toStrictEqual([...OPS]);
    expect(tagsOf("kind", Reply.options)).toStrictEqual([...OPS]);
  });

  test("every event kind has a fixture", () => {
    const events = roundTrip("event", Event);
    const seen = new Set<string>(events.map((event) => event.kind));
    expect(seen.has("unrecognized")).toBe(false);
    expect([...seen].sort()).toStrictEqual([...EVENT_KINDS].sort());
    // The list the check runs on is the schema's own, so a new kind fails here until it has a fixture.
    expect(EVENT_KINDS.length).toBe(Event.out.options.length - 1);
  });

  test("every refusal reason has a fixture with its kind", () => {
    const refusals = roundTrip("refusal", Refusal);
    const seen = new Set<string>();
    for (const { kind, reason } of refusals) {
      expect(reason.code).not.toBe("unrecognized");
      if (reason.code !== "unrecognized") {
        expect(REASON_KIND[reason.code]).toBe(kind);
      }
      seen.add(reason.code);
    }
    expect([...seen].sort()).toStrictEqual([...REASON_CODES].sort());
    expect(REASON_CODES.length).toBe(Object.keys(REASON_KIND).length);
  });

  test("every invalid message is refused at the boundary", () => {
    for (const [name, fixture] of fixtures("invalid")) {
      const parsed = schemaNamed(field(fixture, "schema")).safeParse(field(fixture, "value"));
      expect(parsed.success, `invalid/${name}`).toBe(false);
    }
  });

  test("a message from a newer node reads as the compatibility rule says", () => {
    for (const [name, fixture] of fixtures("tolerated")) {
      const parsed = schemaNamed(field(fixture, "schema")).safeParse(field(fixture, "value"));
      if (!parsed.success) {
        throw new Error(`tolerated/${name} does not parse: ${parsed.error.message}`);
      }
      expect(written(parsed.data), `tolerated/${name}`).toStrictEqual(field(fixture, "reads_as"));
    }
  });

  test("a hello frames to the fixture bytes and back", () => {
    const fixture = readJson(`${FIXTURES}frame/hello.json`);
    const hello = Hello.parse(field(fixture, "value"));
    expect(Buffer.from(encodeFrame(hello)).toString("hex")).toBe(String(field(fixture, "hex")));
    const decoder = createFrameDecoder();
    decoder.push(Buffer.from(String(field(fixture, "hex")), "hex"));
    const body = decoder.next();
    expect(body).toBeDefined();
    expect(Hello.parse(decodeJson(body ?? new Uint8Array()))).toStrictEqual(hello);
    expect(decoder.buffered()).toBe(0);
  });
});

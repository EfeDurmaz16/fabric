// The TypeScript half of the client API's contract test: the same checks as
// crates/fabric-client/tests/contract.rs on the same files in
// protocol/fixtures. Every fixture parses and writes back to the same JSON,
// every op, event kind, refusal reason and variant of every nested enum has
// a fixture, the messages under `invalid` are refused, and the messages
// under `tolerated` read as the compatibility rule of ADR 0018 says.

import { describe, expect, test } from "bun:test";
import { readdirSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import type { z } from "zod";

import {
  API_VERSION,
  Answer,
  createFrameDecoder,
  Decision,
  decodeJson,
  DoctorReport,
  encodeFrame,
  Event,
  EVENT_KINDS,
  Harness,
  Hello,
  HelloAnswer,
  LEDGER_EVENT_KINDS,
  LedgerEvent,
  Marker,
  OPS,
  Outcome,
  type PreflightRow,
  Process,
  REASON_CODES,
  REASON_KIND,
  Refusal,
  RefusalKind,
  Reply,
  Request,
  ROOT_CAPABILITIES,
  Standing,
  Step,
  tagsOf,
  View,
  Wire,
  WorkspaceState,
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

/**
 * The enums whose set of variants is closed, with the tags the schemas know: a reader refuses a variant it does
 * not know, so a new one breaks every reader of the api version (ADR 0018, Compatibility rule 3). Ops, event
 * kinds, refusal reasons and ledger record kinds are open and are not here.
 */
const CLOSED: Readonly<Record<string, readonly string[]>> = {
  Answer: tagsOf("type", Answer.options),
  Decision: Decision.options,
  DoctorReport: tagsOf("kind", DoctorReport.options),
  Harness: tagsOf("kind", Harness.options),
  HelloAnswer: tagsOf("type", HelloAnswer.options),
  Marker: tagsOf("kind", Marker.options),
  Outcome: Outcome.options,
  Process: tagsOf("kind", Process.options),
  RefusalKind: RefusalKind.options,
  Standing: Standing.options,
  Step: Step.options,
  View: tagsOf("kind", View.options),
  Wire: Wire.options,
  WorkspaceState: tagsOf("kind", WorkspaceState.options),
};

/** `DoctorReport` as `doctor_report`, the name of its file under `closed`. */
function snakeCase(name: string): string {
  return name.replace(/(?<=[a-z])(?=[A-Z])/g, "_").toLowerCase();
}

/** Every key of `value`, at any depth, as a JSON pointer below `at`. */
function pointers(value: unknown, at = ""): string[] {
  if (Array.isArray(value)) {
    return value.flatMap((item, index) => pointers(item, `${at}/${index}`));
  }
  if (typeof value === "object" && value !== null) {
    return Object.entries(value).flatMap(([key, field]) => {
      const here = `${at}/${key.replaceAll("~", "~0").replaceAll("/", "~1")}`;
      return [here, ...pointers(field, here)];
    });
  }
  return [];
}

/** `value` without the key `pointer` names. */
function without(value: unknown, pointer: string): unknown {
  const copy: unknown = structuredClone(value);
  const steps = pointer.split("/").slice(1).map((step) => step.replaceAll("~1", "/").replaceAll("~0", "~"));
  const key = steps.pop() ?? "";
  let parent: unknown = copy;
  for (const step of steps) {
    parent = Object.getOwnPropertyDescriptor(parent, step)?.value;
  }
  if (typeof parent !== "object" || parent === null) {
    throw new Error(`${pointer} names no key`);
  }
  Reflect.deleteProperty(parent, key);
  return copy;
}

/** The directories of fixtures that are messages of the api. */
const MESSAGES = ["hello", "hello_answer", "answer", "request", "reply", "event", "refusal"] as const;

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

  test("the welcome fixture lists every op, in order, then every root", () => {
    const welcome = roundTrip("hello_answer", HelloAnswer).find((answer) => answer.type === "welcome");
    expect(welcome?.type).toBe("welcome");
    expect(welcome?.type === "welcome" ? welcome.capabilities : []).toStrictEqual([...OPS, ...ROOT_CAPABILITIES]);
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

  // ADR 0018 says the two clients cannot drift apart without a test failing. Ops, event kinds and refusal
  // reasons each have a test above; this one holds every other enum a message carries, ledger records
  // included, to a fixture for each variant, listed from the schemas' own options (review finding on PR #13).
  test("every variant of every nested enum has a fixture", () => {
    const seen = new Map<string, Set<string>>();
    const mark = (name: string, variant: string): void => {
      const variants = seen.get(name) ?? new Set<string>();
      variants.add(variant);
      seen.set(name, variants);
    };
    const harness = (value: Harness): void => mark("Harness", value.kind);
    const rows = (values: readonly PreflightRow[]): void => {
      for (const row of values) {
        mark("Outcome", row.outcome);
      }
    };
    const refusal = (value: Refusal): void => {
      mark("RefusalKind", value.kind);
      if (value.reason.code === "unfinished") {
        mark("Step", value.reason.step);
      }
    };
    const ledger = (value: LedgerEvent): void => {
      mark("LedgerEvent", value.kind);
      if (value.kind === "created" || value.kind === "resumed") {
        harness(value.harness);
      }
    };
    const event = (value: Event): void => {
      switch (value.kind) {
        case "preflight":
          rows(value.rows);
          break;
        case "session_captured":
        case "session_restored":
        case "continuing_session":
          harness(value.harness);
          break;
        case "new_session":
          harness(value.harness);
          if (value.previous !== undefined) {
            harness(value.previous.harness);
          }
          break;
        case "decision_left":
          mark("Standing", value.standing);
          break;
        case "not_replicated":
        case "no_answer":
        case "move_unanswered":
        case "holder_not_told":
        case "owner_not_told":
        case "not_caught_up":
        case "left_as_it_was":
          refusal(value.error);
          break;
        default:
          break;
      }
    };
    const reply = (value: Reply): void => {
      switch (value.kind) {
        case "node_ping":
          mark("Wire", value.wire);
          break;
        case "job_list":
          for (const job of value.jobs) {
            mark("Standing", job.standing);
          }
          break;
        case "job_status":
          harness(value.harness);
          mark("View", value.view.kind);
          if (value.workspace !== undefined) {
            mark("WorkspaceState", value.workspace.state.kind);
            if (value.workspace.state.kind === "current") {
              mark("Marker", value.workspace.state.marker.kind);
            }
          }
          mark("Process", value.process.kind);
          break;
        case "job_log":
          for (const record of value.records) {
            ledger(record.event);
          }
          break;
        case "doctor":
          mark("DoctorReport", value.report.kind);
          if (value.report.kind === "compared") {
            rows(value.report.rows);
          }
          break;
        case "job_new":
        case "job_run":
        case "job_resume":
        case "harness_started":
        case "harness_session":
          harness(value.harness);
          break;
        default:
          break;
      }
    };
    for (const value of roundTrip("hello_answer", HelloAnswer)) {
      mark("HelloAnswer", value.type);
    }
    for (const value of roundTrip("answer", Answer)) {
      mark("Answer", value.type);
      if (value.type === "event") {
        event(value.event);
      } else if (value.type === "done") {
        reply(value.reply);
      } else {
        refusal(value.refusal);
      }
    }
    for (const value of roundTrip("request", Request)) {
      switch (value.op) {
        case "job_new":
        case "harness_started":
        case "harness_session":
          harness(value.harness);
          break;
        case "job_resume":
          if (value.harness !== undefined) {
            harness(value.harness);
          }
          break;
        case "job_decide":
          mark("Decision", value.decision);
          break;
        default:
          break;
      }
    }
    roundTrip("reply", Reply).forEach(reply);
    roundTrip("event", Event).forEach(event);
    roundTrip("refusal", Refusal).forEach(refusal);

    const variants = { ...CLOSED, LedgerEvent: LEDGER_EVENT_KINDS };
    expect([...seen.keys()].sort()).toStrictEqual(Object.keys(variants).sort());
    for (const [name, known] of Object.entries(variants)) {
      expect([...(seen.get(name) ?? [])].sort(), name).toStrictEqual([...known].sort());
    }
    // The ledger list the check runs on is the schema's own, so a new kind fails here until it has a fixture.
    expect(LEDGER_EVENT_KINDS.length).toBe(LedgerEvent.out.options.length - 1);
  });

  // Review finding on PR #13: a new variant of a closed enum came with a fixture of its own, which the fixture
  // check let through. scripts/check-fixtures.sh refuses an edit of a pinned file without an api bump.
  test("every closed enum has exactly the variants its api version pins", () => {
    const pinned = Object.fromEntries(
      fixtures("closed").map(([name, tags]) => [name.replace(/\.json$/, ""), tags] as const),
    );
    const known: Readonly<Record<string, unknown>> = Object.fromEntries(
      Object.entries(CLOSED).map(([name, tags]) => [snakeCase(name), [...tags].sort()] as const),
    );
    expect(known).toStrictEqual(pinned);
  });

  // Review finding on PR #13: a field made optional passes every round trip, yet an older reader that needs it
  // breaks once a node leaves it out. scripts/check-fixtures.sh refuses a line of protocol/required.txt removed,
  // or added for a fixture the base had, without an api bump.
  test("every message needs exactly the fields its api version pins", () => {
    const required: string[] = [];
    for (const dir of MESSAGES) {
      const schema = schemaNamed(dir);
      for (const [name, json] of fixtures(dir)) {
        for (const key of pointers(json)) {
          if (!schema.safeParse(without(json, key)).success) {
            required.push(`${dir}/${name} ${key}`);
          }
        }
      }
    }
    const pinned = readFileSync(`${FIXTURES}../required.txt`, "utf8").split("\n").filter((line) => line !== "");
    const unpinned = required.filter((line) => !pinned.includes(line));
    const optional = pinned.filter((line) => !required.includes(line));
    expect({ unpinned, optional }).toStrictEqual({ unpinned: [], optional: [] });
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

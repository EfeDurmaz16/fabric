// Facade protocol v1 (ADR 0027, docs/facade-protocol.md): the messages a
// client sends a managed job's authority through any registered node, and
// the answers it gets back, mirroring crates/fabric-client/src/facade.rs.
//
// Both ends are here so the fixtures in protocol/facade/v1 pin the same
// outcomes on both sides: `readClientMessage` reads a message as the
// authority does (schema, then version; the token and the scope are the
// authority's own checks) and names each refusal with the `code` and
// `reason` of the `refused` answer; `readAnswer` reads an answer as a
// client does, skipping fields it does not know and refusing an unknown
// message type or refusal code as a protocol violation. Counters cross as
// canonical decimal strings; a JSON number where a counter belongs is
// `bad_integer` on either side.

import { z } from "zod";

import {
  Counter,
  Fidelity,
  FIDELITIES,
  Harness,
  Inspection,
  JobId,
  type ManagedPolicy,
  type ManagedSource,
  NodeId,
  OperationId,
  OperationKey,
  SnapshotId,
  TaskId,
} from "./schema.ts";

/** The protocol version this module speaks; every message carries it as `facade`. */
export const FACADE_VERSION = 1;
/** The longest a watch may wait for a first event, in milliseconds. */
export const MAX_WAIT_MS = 25_000;
/** How long a watch waits when the client gave no `wait_ms`. */
export const DEFAULT_WAIT_MS = 10_000;
/** The most events one `events` answer carries. */
export const WATCH_WINDOW = 256;
/** The most bytes one free-text field may hold. */
export const TEXT_MAX_BYTES = 16 * 1024;

const BEARER = /^[0-9a-f]{64}$/;
const MAX_SAFE = Number.MAX_SAFE_INTEGER;

/** The `code` of a `refused` answer, in the protocol's order. */
export const REFUSAL_CODES = [
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
export type RefusalCode = (typeof REFUSAL_CODES)[number];

/** The message types a client sends. */
export const CLIENT_TYPES = ["submit", "inspect", "watch", "cancel", "checkpoint", "continue"] as const;
/** The answer types. */
export const ANSWER_TYPES = ["accepted", "inspection", "events", "refused"] as const;

type Json = null | boolean | number | string | Json[] | { [key: string]: Json };
type JsonObject = { [key: string]: Json };

/** A `refused` answer: the code and reason a client switches on. */
export type Refusal = {
  code: RefusalCode;
  reason: string;
  message: string;
  current_revision?: Counter;
};

/** The authority refused a message; the refusal is the answer to send. */
export class FacadeRefusal extends Error {
  readonly refusal: Refusal;
  constructor(code: RefusalCode, reason: string, message?: string) {
    const text = message ?? `${code}: ${reason}`;
    super(text);
    this.name = "FacadeRefusal";
    this.refusal = { code, reason, message: text };
  }
}

/** A frame was not an answer of this protocol; `reason` is the fixtures' name for what was wrong. */
export class ProtocolViolation extends Error {
  readonly reason: string;
  constructor(reason: string, detail: string) {
    super(`protocol violation (${reason}): ${detail}`);
    this.name = "ProtocolViolation";
    this.reason = reason;
  }
}

/** What a `submit` asks for. */
export type Submit = {
  goal: string;
  harness: Harness;
  acceptance: string[];
  source: ManagedSource;
  policy?: ManagedPolicy;
};

/** A message from a client to the authority. */
export type ClientMessage =
  | { type: "submit"; token: string; key: OperationKey; submit: Submit }
  | { type: "inspect"; token: string; job: JobId }
  | { type: "watch"; token: string; job: JobId; after: Counter | null; wait_ms?: number }
  | {
      type: "cancel";
      token: string;
      key: OperationKey;
      job: JobId;
      expected_revision: Counter | null;
      task: TaskId | null;
      reason: string;
    }
  | { type: "checkpoint"; token: string; key: OperationKey; job: JobId; expected_revision: Counter | null }
  | {
      type: "continue";
      token: string;
      key: OperationKey;
      job: JobId;
      expected_revision: Counter | null;
      fidelity?: Fidelity[];
      pin: NodeId | null;
    };

/** One event in an `events` answer; the event keeps its JSON, with its `kind` read. */
export type FacadeEvent = { seq: Counter; revision: Counter; at_ms: number; event: JsonObject & { kind: string } };

/** An answer from the authority. */
export type FacadeAnswer =
  | { type: "accepted"; operation: OperationId; key: OperationKey; replayed: boolean; job: JobId; revision: Counter }
  | { type: "inspection"; inspection: Inspection }
  | {
      type: "events";
      job: JobId;
      events: FacadeEvent[];
      next: Counter | null;
      lagged: boolean;
      closed: string | null;
    }
  | { type: "refused"; refusal: Refusal };

function isObject(value: unknown): value is JsonObject {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function bad(reason: string, detail: string): FacadeRefusal {
  return new FacadeRefusal("bad_request", reason, `bad_request: ${reason} (${detail})`);
}

function isText(value: unknown): value is string {
  return (
    typeof value === "string" &&
    new TextEncoder().encode(value).length <= TEXT_MAX_BYTES &&
    // eslint-disable-next-line no-control-regex
    !/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f-\u009f]/.test(value)
  );
}

function isInteger(value: unknown): value is number {
  return typeof value === "number" && Number.isInteger(value) && value >= 0 && value <= MAX_SAFE;
}

/** One JSON object being read field by field; what is left at the end is unknown. */
class Fields {
  private readonly seen = new Set<string>();
  private readonly map: JsonObject;
  private readonly where: string;
  constructor(map: JsonObject, where: string) {
    this.map = map;
    this.where = where;
  }

  optional(name: string): Json | undefined {
    this.seen.add(name);
    return this.map[name];
  }

  required(name: string): Json {
    const value = this.optional(name);
    if (value === undefined) throw bad("missing_field", `${this.where}${name} is required`);
    return value;
  }

  string(name: string): string {
    const value = this.required(name);
    if (typeof value !== "string") throw bad("bad_string", `${this.where}${name}: a string`);
    return value;
  }

  text(name: string): string {
    const value = this.required(name);
    if (!isText(value)) throw bad("bad_string", `${this.where}${name}: free text`);
    return value;
  }

  integer(name: string): number {
    const value = this.required(name);
    if (!isInteger(value)) throw bad("bad_integer", `${this.where}${name}: an integer`);
    return value;
  }

  id<T extends z.ZodType>(name: string, schema: T): z.output<T> {
    const parsed = schema.safeParse(this.string(name));
    if (!parsed.success) throw bad("bad_id", `${this.where}${name}: not an id`);
    return parsed.data;
  }

  idNullable<T extends z.ZodType>(name: string, schema: T): z.output<T> | null {
    const value = this.required(name);
    if (value === null) return null;
    const parsed = typeof value === "string" ? schema.safeParse(value) : undefined;
    if (parsed === undefined || !parsed.success) throw bad("bad_id", `${this.where}${name}: an id or null`);
    return parsed.data;
  }

  key(): OperationKey {
    const parsed = OperationKey.safeParse(this.string("key"));
    if (!parsed.success) throw bad("bad_string", `${this.where}key: not an operation key`);
    return parsed.data;
  }

  counterNullable(name: string): Counter | null {
    const value = this.required(name);
    if (value === null) return null;
    const parsed = typeof value === "string" ? Counter.safeParse(value) : undefined;
    if (parsed === undefined || !parsed.success) {
      throw bad("bad_integer", `${this.where}${name}: a canonical decimal string or null`);
    }
    return parsed.data;
  }

  object(name: string): Fields {
    const value = this.required(name);
    if (!isObject(value)) throw bad("bad_string", `${this.where}${name}: an object`);
    return new Fields(value, `${this.where}${name}.`);
  }

  finish(): void {
    for (const key of Object.keys(this.map)) {
      if (!this.seen.has(key)) throw bad("unknown_field", `${this.where}${key} is not a field of this message`);
    }
  }
}

function fidelities(value: Json, where: string): Fidelity[] {
  if (!Array.isArray(value)) throw bad("bad_string", `${where}: a list`);
  if (value.length === 0) throw bad("empty_fidelity", `${where}: at least one fidelity`);
  return value.map((item) => {
    if (typeof item !== "string") throw bad("bad_string", `${where}: names`);
    if (!(FIDELITIES as readonly string[]).includes(item)) throw bad("unknown_variant", `${where}: ${item}`);
    return item as Fidelity;
  });
}

function readHarness(value: Json): Harness {
  const parsed = Harness.safeParse(value);
  if (parsed.success) return parsed.data;
  const text = parsed.error.issues.map((issue) => issue.code).join(",");
  const reason = text.includes("unrecognized_keys")
    ? "unknown_field"
    : text.includes("invalid_union") || text.includes("invalid_value")
      ? "unknown_variant"
      : "bad_string";
  // An absent `kind` is a missing field, which zod reports as an invalid union.
  if (isObject(value) && value["kind"] === undefined) throw bad("missing_field", "harness.kind");
  if (isObject(value) && typeof value["kind"] === "string" && !["claude_code", "codex", "acp"].includes(value["kind"])) {
    throw bad("unknown_variant", `harness.kind ${value["kind"]}`);
  }
  if (isObject(value) && value["kind"] === "acp" && (value["program"] === undefined || value["args"] === undefined)) {
    throw bad("missing_field", "harness.program and harness.args");
  }
  throw bad(reason, `harness: ${parsed.error.message}`);
}

function readSubmit(obj: Fields): Submit {
  const intent = obj.object("intent");
  const goal = intent.text("goal");
  const harness = readHarness(intent.required("harness"));
  const acceptanceValue = intent.optional("acceptance");
  let acceptance: string[] = [];
  // The facade writes `acceptance` always, the local socket only when it is not empty.
  if (acceptanceValue !== undefined) {
    if (!Array.isArray(acceptanceValue)) throw bad("bad_string", "acceptance: a list");
    acceptance = acceptanceValue.map((item) => {
      if (!isText(item)) throw bad("bad_string", "acceptance: free text");
      return item;
    });
  }
  intent.finish();

  const sourceFields = obj.object("source");
  const kind = sourceFields.string("kind");
  let source: ManagedSource;
  switch (kind) {
    case "git": {
      const uri = sourceFields.string("uri");
      // eslint-disable-next-line no-control-regex
      if (new TextEncoder().encode(uri).length > 2048 || /[\u0000-\u001f\u007f]/.test(uri)) {
        throw bad("bad_string", "source.uri");
      }
      const commit = sourceFields.string("commit");
      sourceFields.finish();
      source = { kind: "git", uri, commit };
      break;
    }
    case "snapshot": {
      const snapshot = sourceFields.id("snapshot", SnapshotId);
      sourceFields.finish();
      source = { kind: "snapshot", snapshot };
      break;
    }
    case "empty":
      sourceFields.finish();
      source = { kind: "empty" };
      break;
    case "repository":
    case "directory":
      throw bad("source_local", "a repository or directory is a path on one node; the facade takes git, snapshot or empty");
    default:
      throw bad("unknown_variant", `source.kind ${kind}: git, snapshot or empty`);
  }

  const policyValue = obj.optional("policy");
  const policy = policyValue === undefined ? undefined : readPolicy(policyValue);
  return policy === undefined ? { goal, harness, acceptance, source } : { goal, harness, acceptance, source, policy };
}

function readPolicy(value: Json): ManagedPolicy {
  if (!isObject(value)) throw bad("bad_string", "policy: an object");
  const obj = new Fields(value, "policy.");
  const policy: ManagedPolicy = { pin: null };
  const drain = obj.optional("drain_budget_ms");
  if (drain !== undefined) {
    if (!isInteger(drain) || drain > 0xffff_ffff) throw bad("bad_integer", "policy.drain_budget_ms");
    policy.drain_budget_ms = drain;
  }
  const allowed = obj.optional("allowed_fidelity");
  if (allowed !== undefined) policy.allowed_fidelity = fidelities(allowed, "policy.allowed_fidelity");
  const secrets = obj.optional("secrets");
  if (secrets !== undefined) {
    if (!Array.isArray(secrets)) throw bad("bad_string", "policy.secrets: a list");
    policy.secrets = secrets.map((item) => {
      if (!isObject(item)) throw bad("bad_string", "policy.secrets: objects");
      const secret = new Fields(item, "policy.secrets.");
      const name = secret.string("name");
      if (!/^[A-Z0-9_]{1,64}$/.test(name)) throw bad("bad_string", "a secret name is 1 to 64 characters of [A-Z0-9_]");
      const attempt = secret.string("attempt");
      if (attempt !== "any" && attempt !== "current") throw bad("unknown_variant", `secret attempt scope ${attempt}`);
      const kinds = secret.required("resource_kinds");
      if (!Array.isArray(kinds)) throw bad("bad_string", "resource_kinds");
      const resource_kinds = kinds.map((kindName) => {
        if (typeof kindName !== "string") throw bad("bad_string", "resource_kinds");
        if (kindName !== "node") throw bad("unknown_variant", `resource kind ${kindName}`);
        return kindName;
      });
      secret.finish();
      return { name, attempt, resource_kinds };
    });
  }
  const allowlist = obj.optional("export_allowlist");
  if (allowlist !== undefined) {
    if (!Array.isArray(allowlist)) throw bad("bad_string", "policy.export_allowlist");
    policy.export_allowlist = allowlist.map((item) => {
      if (!isText(item)) throw bad("bad_string", "policy.export_allowlist: paths");
      return item;
    });
  }
  const disk = obj.optional("disk_bytes");
  if (disk !== undefined) {
    if (!isInteger(disk)) throw bad("bad_integer", "policy.disk_bytes");
    policy.disk_bytes = disk;
  }
  policy.pin = obj.idNullable("pin", NodeId);
  obj.finish();
  return policy;
}

/**
 * Reads one message as the authority does: schema first, then the
 * version. Throws `FacadeRefusal` with the `refused` answer to send.
 */
export function readClientMessage(value: unknown): ClientMessage {
  if (!isObject(value)) throw bad("bad_string", "a facade message is a JSON object");
  const obj = new Fields(value, "");
  const type = obj.string("type");
  if (!(CLIENT_TYPES as readonly string[]).includes(type)) {
    throw bad("unknown_variant", `${type} is not a facade message`);
  }
  const version = obj.integer("facade");
  const token = obj.string("token");
  if (!BEARER.test(token)) throw bad("bad_string", "token: 64 lowercase hex");
  let message: ClientMessage;
  switch (type) {
    case "submit":
      message = { type, token, key: obj.key(), submit: readSubmit(obj) };
      break;
    case "inspect":
      message = { type, token, job: obj.id("job", JobId) };
      break;
    case "watch": {
      const job = obj.id("job", JobId);
      const after = obj.counterNullable("after");
      const wait = obj.optional("wait_ms");
      if (wait === undefined) {
        message = { type, token, job, after };
      } else {
        if (!isInteger(wait)) throw bad("bad_integer", "wait_ms");
        if (wait > MAX_WAIT_MS) throw bad("wait_above_limit", `wait_ms is at most ${MAX_WAIT_MS}`);
        message = { type, token, job, after, wait_ms: wait };
      }
      break;
    }
    case "cancel":
      message = {
        type,
        token,
        key: obj.key(),
        job: obj.id("job", JobId),
        expected_revision: obj.counterNullable("expected_revision"),
        task: obj.idNullable("task", TaskId),
        reason: obj.text("reason"),
      };
      break;
    case "checkpoint":
      message = {
        type,
        token,
        key: obj.key(),
        job: obj.id("job", JobId),
        expected_revision: obj.counterNullable("expected_revision"),
      };
      break;
    default: {
      const key = obj.key();
      const job = obj.id("job", JobId);
      const expected_revision = obj.counterNullable("expected_revision");
      const fidelityValue = obj.optional("fidelity");
      const pin = obj.idNullable("pin", NodeId);
      message =
        fidelityValue === undefined
          ? { type: "continue", token, key, job, expected_revision, pin }
          : { type: "continue", token, key, job, expected_revision, fidelity: fidelities(fidelityValue, "fidelity"), pin };
    }
  }
  obj.finish();
  if (version !== FACADE_VERSION) {
    throw new FacadeRefusal(
      "unsupported_version",
      "facade",
      `unsupported_version: facade (this side speaks facade ${FACADE_VERSION}, the message says ${version})`,
    );
  }
  return message;
}

/** The message as JSON, the bytes a client sends; the inverse of `readClientMessage`. */
export function writeClientMessage(message: ClientMessage): JsonObject {
  const out: JsonObject = { type: message.type, facade: FACADE_VERSION, token: message.token };
  switch (message.type) {
    case "submit": {
      out["key"] = message.key;
      const { goal, harness, acceptance, source, policy } = message.submit;
      out["intent"] = { goal, harness: harness as unknown as Json, acceptance };
      out["source"] = source as unknown as Json;
      if (policy !== undefined) out["policy"] = JSON.parse(JSON.stringify(policy)) as Json;
      break;
    }
    case "inspect":
      out["job"] = message.job;
      break;
    case "watch":
      out["job"] = message.job;
      out["after"] = message.after;
      if (message.wait_ms !== undefined) out["wait_ms"] = message.wait_ms;
      break;
    case "cancel":
      out["key"] = message.key;
      out["job"] = message.job;
      out["expected_revision"] = message.expected_revision;
      out["task"] = message.task;
      out["reason"] = message.reason;
      break;
    case "checkpoint":
      out["key"] = message.key;
      out["job"] = message.job;
      out["expected_revision"] = message.expected_revision;
      break;
    case "continue":
      out["key"] = message.key;
      out["job"] = message.job;
      out["expected_revision"] = message.expected_revision;
      if (message.fidelity !== undefined) out["fidelity"] = [...message.fidelity];
      out["pin"] = message.pin;
      break;
  }
  return out;
}

const COUNTER_FIELDS = new Set(["revision", "expected_revision", "current_revision", "generation", "seq", "next", "after"]);

function countersAreStrings(value: Json): void {
  if (Array.isArray(value)) {
    for (const item of value) countersAreStrings(item);
    return;
  }
  if (!isObject(value)) return;
  for (const [key, inner] of Object.entries(value)) {
    if (COUNTER_FIELDS.has(key) && inner !== null && typeof inner !== "string") {
      throw new ProtocolViolation("bad_integer", `${key} = ${JSON.stringify(inner)} is a counter and travels as a decimal string`);
    }
    countersAreStrings(inner);
  }
}

const Accepted = z.object({
  operation: OperationId,
  key: OperationKey,
  replayed: z.boolean(),
  job: JobId,
  revision: Counter,
});

const Event = z.object({
  seq: Counter,
  revision: Counter,
  at_ms: z.number().int().min(0).max(MAX_SAFE),
  event: z.object({ kind: z.string() }).passthrough(),
});

const Events = z.object({
  job: JobId,
  events: z.array(Event),
  next: Counter.nullable(),
  lagged: z.boolean(),
  closed: z.string().nullable(),
});

function violation(error: z.ZodError): ProtocolViolation {
  const codes = error.issues.map((issue) => issue.code);
  const reason = codes.includes("invalid_type") && error.issues.some((issue) => issue.message.includes("undefined"))
    ? "missing_field"
    : "bad_string";
  return new ProtocolViolation(reason, error.message);
}

/**
 * Reads an answer as a client does: an unknown type or refusal code is a
 * protocol violation, a counter sent as a number is one, and a field this
 * client does not know is skipped.
 */
export function readAnswer(value: unknown): FacadeAnswer {
  if (!isObject(value)) throw new ProtocolViolation("bad_string", "an answer is a JSON object");
  const type = value["type"];
  if (typeof type !== "string") throw new ProtocolViolation("missing_field", "type");
  if (!(ANSWER_TYPES as readonly string[]).includes(type)) {
    throw new ProtocolViolation("unknown_variant", `${type} is not a facade answer`);
  }
  const version = value["facade"];
  if (!isInteger(version)) throw new ProtocolViolation("bad_integer", "facade");
  if (version !== FACADE_VERSION) throw new ProtocolViolation("unknown_variant", `facade ${version}`);
  countersAreStrings(value);
  switch (type) {
    case "accepted": {
      const parsed = Accepted.safeParse(value);
      if (!parsed.success) throw violation(parsed.error);
      return { type, ...parsed.data };
    }
    case "inspection": {
      const parsed = Inspection.safeParse(value);
      if (!parsed.success) throw violation(parsed.error);
      return { type, inspection: parsed.data };
    }
    case "events": {
      const parsed = Events.safeParse(value);
      if (!parsed.success) throw violation(parsed.error);
      return { type, ...parsed.data } as FacadeAnswer;
    }
    default: {
      const code = value["code"];
      if (typeof code !== "string") throw new ProtocolViolation("missing_field", "code");
      if (!(REFUSAL_CODES as readonly string[]).includes(code)) {
        throw new ProtocolViolation("unknown_variant", `refusal code ${code}`);
      }
      const reason = value["reason"];
      if (typeof reason !== "string") throw new ProtocolViolation("missing_field", "reason");
      const messageText = typeof value["message"] === "string" ? value["message"] : `${code}: ${reason}`;
      const refusal: Refusal = { code: code as RefusalCode, reason, message: messageText };
      const current = value["current_revision"];
      if (current !== undefined && current !== null) {
        const parsed = Counter.safeParse(current);
        if (!parsed.success) throw new ProtocolViolation("bad_integer", "current_revision");
        refusal.current_revision = parsed.data;
      }
      return { type: "refused", refusal };
    }
  }
}

/** The answer as JSON, the bytes the authority sends; the inverse of `readAnswer`. */
export function writeAnswer(answer: FacadeAnswer): JsonObject {
  switch (answer.type) {
    case "accepted":
      return {
        type: "accepted",
        facade: FACADE_VERSION,
        operation: answer.operation,
        key: answer.key,
        replayed: answer.replayed,
        job: answer.job,
        revision: answer.revision,
      };
    case "inspection":
      return { type: "inspection", facade: FACADE_VERSION, ...(JSON.parse(JSON.stringify(answer.inspection)) as JsonObject) };
    case "events":
      return {
        type: "events",
        facade: FACADE_VERSION,
        job: answer.job,
        events: JSON.parse(JSON.stringify(answer.events)) as Json,
        next: answer.next,
        lagged: answer.lagged,
        closed: answer.closed,
      };
    case "refused": {
      const out: JsonObject = {
        type: "refused",
        facade: FACADE_VERSION,
        code: answer.refusal.code,
        reason: answer.refusal.reason,
        message: answer.refusal.message,
      };
      if (answer.refusal.current_revision !== undefined) out["current_revision"] = answer.refusal.current_revision;
      return out;
    }
  }
}

// Every message of the client API (ADR 0018) as a zod schema, mirroring
// crates/fabric-client/src/api.rs field for field. protocol/fixtures pins
// both, and test/contract.test.ts reads the same files as the Rust test.
//
// Wire rules kept here: ids parse exactly as the Rust parsers do, so a
// value either side accepts the other accepts too; unknown object fields
// are dropped (zod's default); an optional field is absent when it has no
// value and `null` reads as absent; an event, a ledger event or a refusal
// reason of a kind this client does not know reads as `unrecognized`,
// while a known kind with a bad field is an error.

import { isIPv4, isIPv6 } from "node:net";
import { z } from "zod";

import { EnvError } from "./errors.ts";

/** The version of the client API this SDK speaks; a node answers only the same number. */
export const API_VERSION = 1;

/** Every op of this api version, in the order of `Op::ALL` in the Rust crate. */
export const OPS = [
  "node_id",
  "node_add",
  "node_list",
  "node_ping",
  "job_new",
  "job_list",
  "job_status",
  "job_log",
  "job_run",
  "job_resume",
  "job_suspend",
  "job_move",
  "job_steal",
  "job_decide",
  "doctor",
  "harness_started",
  "harness_session",
  "harness_exited",
] as const;

/** The name of a request, which is also the kind of its reply. */
export type Op = (typeof OPS)[number];

// Numbers. Every integer on the wire is below 2^53, so a JS number holds it
// exactly; anything larger is refused rather than rounded.

const U32_MAX = 0xffff_ffff;
const count = z.number().int().min(0).max(Number.MAX_SAFE_INTEGER);
const signedCount = z.number().int().min(Number.MIN_SAFE_INTEGER).max(Number.MAX_SAFE_INTEGER);

/** A lease epoch; zero does not exist. */
export const Epoch = z.number().int().min(1).max(Number.MAX_SAFE_INTEGER).brand<"Epoch">();
export type Epoch = z.infer<typeof Epoch>;

/** A process id, as Rust's `NonZeroU32`. */
export const Pid = z.number().int().min(1).max(U32_MAX).brand<"Pid">();
export type Pid = z.infer<typeof Pid>;

// Text. The node's JSON reader refuses a lone UTF-16 surrogate, such as
// half an emoji cut off by `.slice()`, which a JavaScript string may hold
// but a Rust string may not. Every string in a message is checked, so a
// request holding one never leaves the client, as Rust refuses it too.

const LONE_SURROGATE = /\p{Cs}/u;

/** Whether `text` is Unicode text, with no UTF-16 surrogate apart from its pair. */
export function isUnicodeText(text: string): boolean {
  return !LONE_SURROGATE.test(text);
}

const UnicodeText = z.string().refine(isUnicodeText, "holds a lone UTF-16 surrogate, which is not Unicode text");

// Ids, each in its one text form.

// 26 uppercase Crockford base32 characters; the first carries 3 bits, so 0 to 7.
const ULID = /^[0-7][0-9A-HJKMNP-TV-Z]{25}$/;
// 32 bytes in 52 lowercase base32 characters; the last character's 4 spare bits are zero.
const NODE_ID = /^fab1[a-z2-7]{51}[aq]$/;
const HEX_32 = /^[0-9a-f]{64}$/;
const SESSION_ID = /^(?![-.])[A-Za-z0-9._:-]{1,128}$/;
// No control character, C0, DEL or C1 (`\p{Cc}`, which holds NUL), as Rust's `char::is_control`.
const PROGRAM_NAME = /^(?!-)[^/\p{Cc}]+$/u;

export const JobId = z.string().regex(ULID, "not a ULID in its text form").brand<"JobId">();
export type JobId = z.infer<typeof JobId>;

export const MoveId = z.string().regex(ULID, "not a ULID in its text form").brand<"MoveId">();
export type MoveId = z.infer<typeof MoveId>;

export const EventId = z.string().regex(ULID, "not a ULID in its text form").brand<"EventId">();
export type EventId = z.infer<typeof EventId>;

export const NodeId = z.string().regex(NODE_ID, "not a node id in its text form").brand<"NodeId">();
export type NodeId = z.infer<typeof NodeId>;

export const SnapshotId = z.string().regex(HEX_32, "not 64 lowercase hex digits").brand<"SnapshotId">();
export type SnapshotId = z.infer<typeof SnapshotId>;

export const EnvSpecId = z.string().regex(HEX_32, "not 64 lowercase hex digits").brand<"EnvSpecId">();
export type EnvSpecId = z.infer<typeof EnvSpecId>;

/** A session id a harness minted: safe in a file name and on a command line. */
export const SessionId = z.string().regex(SESSION_ID, "not a session id").brand<"SessionId">();
export type SessionId = z.infer<typeof SessionId>;

/** A program to look up on `PATH`: never a path, never an option, never a control character. */
export const ProgramName = UnicodeText.regex(PROGRAM_NAME, "not a bare program name").brand<"ProgramName">();
export type ProgramName = z.infer<typeof ProgramName>;

/** A path on the node. Only UTF-8 paths cross the socket, so a path is Unicode text. */
const Path = UnicodeText;

/** `ip:port` or `[ipv6]:port`, as Rust's `SocketAddr` reads it. */
const SocketAddr = z.string().refine(isSocketAddr, "not an ip:port socket address");

function isSocketAddr(text: string): boolean {
  const match = /^(?:\[([^\]]+)\]|([0-9.]+)):([0-9]{1,5})$/.exec(text);
  if (match === null || Number(match[3]) > 0xffff) {
    return false;
  }
  const [, v6, v4] = match;
  if (v4 !== undefined) {
    return isIPv4(v4);
  }
  if (v6 === undefined) {
    return false;
  }
  // Rust accepts a numeric scope id after `%`, as in `[fe80::1%2]:7777`.
  const [address = "", scope, ...rest] = v6.split("%");
  return rest.length === 0 && isIPv6(address) && (scope === undefined || /^[0-9]+$/.test(scope));
}

/** A field that may be absent; `null` reads as absent, as serde reads it. */
function optional<T extends z.ZodType>(schema: T) {
  return z.preprocess((value) => (value === null ? undefined : value), schema.optional());
}

function isRecord(value: unknown): value is Readonly<Record<string, unknown>> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

type Tagged<K extends string> = {
  readonly shape: { readonly [P in K]: { readonly values: ReadonlySet<unknown> } };
};

/** The tag values of a discriminated union's options, read from the schemas themselves. */
export function tagsOf<K extends string>(tag: K, options: readonly Tagged<K>[]): readonly string[] {
  return options.flatMap((option) =>
    [...option.shape[tag].values].filter((value): value is string => typeof value === "string"),
  );
}

/**
 * Rewrites an object whose `tag` is a string this client does not know to
 * `{ [tag]: "unrecognized" }`, as serde's `#[serde(other)]` does. A known
 * tag passes through untouched, so its fields are still checked.
 */
function unknownTagAsUnrecognized(tag: string, known: readonly string[]) {
  const knownTags = new Set(known);
  return (value: unknown): unknown => {
    if (!isRecord(value) || !Object.hasOwn(value, tag)) {
      return value;
    }
    const name = value[tag];
    return typeof name === "string" && !knownTags.has(name) ? { [tag]: "unrecognized" } : value;
  };
}

// Enums written as bare strings.

export const Decision = z.enum(["approve", "reject"]);
export type Decision = z.infer<typeof Decision>;

export const Wire = z.enum(["quic", "tcp_tls"]);
export type Wire = z.infer<typeof Wire>;

export const Standing = z.enum(["owner", "departing", "departed", "replica", "frozen", "unreadable"]);
export type Standing = z.infer<typeof Standing>;

export const Outcome = z.enum(["equal", "fallback", "warn", "refuse", "refuse_always"]);
export type Outcome = z.infer<typeof Outcome>;

export const Step = z.enum(["suspend", "restore"]);
export type Step = z.infer<typeof Step>;

/** The class of a refusal, which decides the CLI's exit code; a closed set. */
export const RefusalKind = z.enum(["not_owner", "frozen", "unreachable", "needs_force", "job_state", "failed"]);
export type RefusalKind = z.infer<typeof RefusalKind>;

// Records shared by several messages.

/** Which agent drives a job. An unknown kind is an error: a node cannot launch it. */
export const Harness = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("claude_code") }),
  z.object({ kind: z.literal("codex") }),
  z.object({ kind: z.literal("acp"), program: ProgramName, args: z.array(UnicodeText) }),
]);
export type Harness = z.infer<typeof Harness>;

export const HarnessSession = z.object({ harness: Harness, session: SessionId });
export type HarnessSession = z.infer<typeof HarnessSession>;

export const PreflightRow = z.object({
  fact: UnicodeText,
  here: optional(UnicodeText),
  peer: optional(UnicodeText),
  outcome: Outcome,
  note: UnicodeText,
});
export type PreflightRow = z.infer<typeof PreflightRow>;

export const StealWarning = z.object({
  owner: NodeId,
  snapshot: SnapshotId,
  owner_stopped_at_snapshot: z.boolean(),
  last_record_at: count,
  age_ms: count,
});
export type StealWarning = z.infer<typeof StealWarning>;

// Refusals.

const reasonOptions = [
  z.object({ code: z.literal("unknown_job"), job: JobId }),
  z.object({ code: z.literal("not_owner"), job: JobId, owner: NodeId, epoch: Epoch }),
  z.object({ code: z.literal("departed"), job: JobId, owner: NodeId, epoch: Epoch }),
  z.object({ code: z.literal("stale_epoch"), yours: Epoch, current: Epoch }),
  z.object({ code: z.literal("epoch_ahead"), yours: Epoch, current: Epoch }),
  z.object({ code: z.literal("wrong_job"), token: JobId, job: JobId }),
  z.object({ code: z.literal("departing"), job: JobId, move_id: MoveId, target: NodeId }),
  z.object({ code: z.literal("forked"), job: JobId, peer: NodeId, epoch: Epoch }),
  z.object({ code: z.literal("fork_behind"), job: JobId, peer: NodeId, epoch: Epoch }),
  z.object({ code: z.literal("unreachable"), peer: NodeId }),
  z.object({ code: z.literal("unknown_peer"), node: NodeId }),
  z.object({ code: z.literal("preflight"), facts: z.array(UnicodeText) }),
  z.object({ code: z.literal("steal_needs_force"), warning: StealWarning }),
  z.object({ code: z.literal("harness_running"), job: JobId, pid: Pid }),
  z.object({ code: z.literal("harness_gone"), pid: Pid }),
  z.object({ code: z.literal("harness_not_recorded"), job: JobId, pid: Pid, session: SessionId }),
  z.object({ code: z.literal("ran_during_move"), job: JobId, target: NodeId }),
  z.object({ code: z.literal("changed_during_move"), job: JobId, root: Path, target: NodeId }),
  z.object({ code: z.literal("stale_workspace"), job: JobId, root: Path, left_at: Epoch }),
  z.object({ code: z.literal("no_workspace"), job: JobId }),
  z.object({ code: z.literal("unfinished"), job: JobId, step: Step }),
  z.object({ code: z.literal("no_snapshot"), job: JobId }),
  z.object({ code: z.literal("workspace_occupied"), root: Path }),
  z.object({ code: z.literal("bad_address"), text: UnicodeText }),
  z.object({ code: z.literal("peer"), peer: NodeId }),
  z.object({ code: z.literal("fetch"), snapshot: SnapshotId, peer: NodeId }),
  z.object({ code: z.literal("bad_request") }),
  z.object({ code: z.literal("internal") }),
] as const;

/** Every refusal reason this client knows, read from the schema. */
export const REASON_CODES = tagsOf("code", reasonOptions);

/** The fact behind a refusal; a code this client does not know reads as `unrecognized`. */
export const Reason = z.preprocess(
  unknownTagAsUnrecognized("code", REASON_CODES),
  z.discriminatedUnion("code", [...reasonOptions, z.object({ code: z.literal("unrecognized") })]),
);
export type Reason = z.infer<typeof Reason>;
export type KnownReason = Exclude<Reason, { code: "unrecognized" }>;

/** The class each known reason belongs to, as `Reason::kind()` in api.rs. */
export const REASON_KIND: Readonly<Record<KnownReason["code"], RefusalKind>> = {
  unknown_job: "not_owner",
  not_owner: "not_owner",
  departed: "not_owner",
  stale_epoch: "not_owner",
  epoch_ahead: "not_owner",
  wrong_job: "not_owner",
  departing: "frozen",
  forked: "frozen",
  unreachable: "unreachable",
  unknown_peer: "unreachable",
  preflight: "needs_force",
  steal_needs_force: "needs_force",
  harness_running: "job_state",
  harness_gone: "job_state",
  harness_not_recorded: "job_state",
  ran_during_move: "job_state",
  changed_during_move: "job_state",
  stale_workspace: "job_state",
  no_workspace: "job_state",
  unfinished: "job_state",
  no_snapshot: "job_state",
  workspace_occupied: "job_state",
  fork_behind: "failed",
  bad_address: "failed",
  peer: "failed",
  fetch: "failed",
  bad_request: "failed",
  internal: "failed",
};

/**
 * Why a node refused or failed a command. A known reason must agree with
 * the kind; an unrecognized one keeps the kind the node sent, so the exit
 * code and the sentence still work.
 */
export const Refusal = z
  .object({ kind: RefusalKind, reason: Reason, message: UnicodeText })
  .refine(({ kind, reason }) => reason.code === "unrecognized" || REASON_KIND[reason.code] === kind, {
    message: "the refusal's kind does not agree with its reason",
    path: ["kind"],
  });
export type Refusal = z.infer<typeof Refusal>;

// Progress events.

const eventOptions = [
  z.object({ kind: z.literal("catching_up") }),
  z.object({ kind: z.literal("preflight"), peer: NodeId, rows: z.array(PreflightRow) }),
  z.object({ kind: z.literal("stale_workspace_kept"), job: JobId, root: Path, left_at: Epoch }),
  z.object({ kind: z.literal("already_suspended"), snapshot: SnapshotId }),
  z.object({ kind: z.literal("workspace_changed"), snapshot: SnapshotId }),
  z.object({ kind: z.literal("stopping_harness"), pid: Pid }),
  z.object({ kind: z.literal("finishing_suspend"), snapshot: SnapshotId }),
  z.object({ kind: z.literal("session_captured"), harness: Harness, session: SessionId, files: count }),
  z.object({
    kind: z.literal("snapshot_published"),
    snapshot: SnapshotId,
    files: count,
    bytes: count,
    new_objects: count,
  }),
  z.object({ kind: z.literal("left_behind"), path: UnicodeText }),
  z.object({ kind: z.literal("no_departure_record"), session: SessionId }),
  z.object({ kind: z.literal("suspended"), snapshot: SnapshotId, epoch: Epoch }),
  z.object({ kind: z.literal("replicated"), peer: NodeId }),
  z.object({ kind: z.literal("not_replicated"), peer: NodeId, error: Refusal }),
  z.object({ kind: z.literal("move_reoffered"), move_id: MoveId, target: NodeId }),
  z.object({ kind: z.literal("already_moved"), owner: NodeId, epoch: Epoch }),
  z.object({ kind: z.literal("target_holds_copy"), target: NodeId }),
  z.object({ kind: z.literal("move_started"), move_id: MoveId, target: NodeId }),
  z.object({ kind: z.literal("no_answer"), target: NodeId, error: Refusal }),
  z.object({ kind: z.literal("move_gave_up"), detail: UnicodeText }),
  z.object({ kind: z.literal("imported"), owner: NodeId, epoch: Epoch }),
  z.object({ kind: z.literal("declined"), target: NodeId, epoch: Epoch }),
  z.object({ kind: z.literal("decision_left"), standing: Standing }),
  z.object({ kind: z.literal("move_undecided"), move_id: MoveId, target: NodeId }),
  z.object({ kind: z.literal("move_unanswered"), move_id: MoveId, target: NodeId, error: Refusal }),
  z.object({ kind: z.literal("move_waits"), target: NodeId }),
  z.object({ kind: z.literal("fenced"), epoch: Epoch, pid: Pid }),
  z.object({ kind: z.literal("copy_moved_aside"), to: Path }),
  z.object({ kind: z.literal("pulling"), job: JobId, holder: NodeId }),
  z.object({ kind: z.literal("holder_started_move"), holder: NodeId, move_id: MoveId }),
  z.object({ kind: z.literal("imported_here"), move_id: MoveId, epoch: Epoch }),
  z.object({ kind: z.literal("holder_not_told"), holder: NodeId, error: Refusal }),
  z.object({ kind: z.literal("workspace_gone"), root: Path }),
  z.object({ kind: z.literal("workspace_in_place"), root: Path }),
  z.object({ kind: z.literal("root_chosen"), root: Path, recorded: optional(Path) }),
  z.object({ kind: z.literal("cache_carried"), path: UnicodeText }),
  z.object({ kind: z.literal("cache_rebuilt"), path: UnicodeText, why: UnicodeText }),
  z.object({ kind: z.literal("restored"), snapshot: SnapshotId, files: count, bytes: count }),
  z.object({ kind: z.literal("orphan_set_aside"), root: Path, left_at: Epoch, epoch: Epoch, to: Path }),
  z.object({ kind: z.literal("session_restored"), harness: Harness, session: SessionId }),
  z.object({ kind: z.literal("steal_warning"), warning: StealWarning }),
  z.object({ kind: z.literal("stole"), job: JobId, epoch: Epoch }),
  z.object({ kind: z.literal("owner_not_told"), owner: NodeId, error: Refusal }),
  z.object({ kind: z.literal("continuing_session"), harness: Harness, session: SessionId }),
  z.object({ kind: z.literal("new_session"), harness: Harness, previous: optional(HarnessSession) }),
  z.object({ kind: z.literal("not_caught_up"), peer: NodeId, error: Refusal }),
  z.object({ kind: z.literal("peer_silent"), peer: NodeId, waited_s: count }),
  z.object({ kind: z.literal("left_as_it_was"), error: Refusal }),
  z.object({ kind: z.literal("frozen"), peer: NodeId, epoch: Epoch }),
] as const;

/** Every event kind this client knows, read from the schema. */
export const EVENT_KINDS = tagsOf("kind", eventOptions);

/**
 * One step of a command, reported as it happens. Events never carry a
 * command's result, so a kind this client does not know reads as
 * `unrecognized` and the client skips it.
 */
export const Event = z.preprocess(
  unknownTagAsUnrecognized("kind", EVENT_KINDS),
  z.discriminatedUnion("kind", [...eventOptions, z.object({ kind: z.literal("unrecognized") })]),
);
export type Event = z.infer<typeof Event>;
export type KnownEvent = Exclude<Event, { kind: "unrecognized" }>;

// What a job's status and ledger say.

export const View = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("owner") }),
  z.object({ kind: z.literal("departing"), move_id: MoveId, target: NodeId }),
  z.object({ kind: z.literal("departed"), owner: NodeId, epoch: Epoch }),
  z.object({ kind: z.literal("replica"), owner: NodeId, epoch: Epoch }),
  z.object({ kind: z.literal("frozen"), peer: NodeId, epoch: Epoch }),
]);
export type View = z.infer<typeof View>;

export const Marker = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("live") }),
  z.object({ kind: z.literal("suspending") }),
  z.object({ kind: z.literal("clean"), snapshot: SnapshotId }),
  z.object({ kind: z.literal("restoring"), snapshot: SnapshotId }),
]);
export type Marker = z.infer<typeof Marker>;

export const WorkspaceState = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("current"), marker: Marker }),
  z.object({ kind: z.literal("stale"), left_at: Epoch }),
  z.object({ kind: z.literal("awaiting_decision"), target: NodeId }),
  z.object({ kind: z.literal("clean_copy") }),
  z.object({ kind: z.literal("orphan") }),
]);
export type WorkspaceState = z.infer<typeof WorkspaceState>;

export const Workspace = z.object({ root: Path, state: WorkspaceState });
export type Workspace = z.infer<typeof Workspace>;

export const SnapshotState = z.object({ id: SnapshotId, suspended: z.boolean() });
export type SnapshotState = z.infer<typeof SnapshotState>;

export const Process = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("not_running") }),
  z.object({ kind: z.literal("running"), pid: Pid, epoch: Epoch }),
  z.object({ kind: z.literal("group_remains"), pid: Pid }),
]);
export type Process = z.infer<typeof Process>;

const ledgerEventOptions = [
  z.object({ kind: z.literal("created"), created_by: NodeId, harness: Harness, goal: UnicodeText }),
  z.object({ kind: z.literal("move_started"), move_id: MoveId, target: NodeId }),
  z.object({ kind: z.literal("move_imported"), move_id: MoveId, from: NodeId }),
  z.object({ kind: z.literal("move_aborted"), move_id: MoveId }),
  z.object({ kind: z.literal("stolen"), by: NodeId, from: NodeId, reason: UnicodeText }),
  z.object({ kind: z.literal("snapshot_published"), snapshot: SnapshotId, parent: optional(SnapshotId) }),
  z.object({ kind: z.literal("suspended"), snapshot: SnapshotId }),
  z.object({ kind: z.literal("resumed"), node: NodeId, snapshot: optional(SnapshotId), harness: Harness }),
  z.object({ kind: z.literal("env_spec_changed"), env_spec: EnvSpecId }),
  z.object({ kind: z.literal("decision_recorded"), text: UnicodeText }),
  z.object({ kind: z.literal("note"), text: UnicodeText }),
] as const;

/** Every ledger record kind this client knows, read from the schema. */
export const LEDGER_EVENT_KINDS = tagsOf("kind", ledgerEventOptions);

/** What a ledger record says happened; a kind a later ledger format wrote reads as `unrecognized`. */
export const LedgerEvent = z.preprocess(
  unknownTagAsUnrecognized("kind", LEDGER_EVENT_KINDS),
  z.discriminatedUnion("kind", [...ledgerEventOptions, z.object({ kind: z.literal("unrecognized") })]),
);
export type LedgerEvent = z.infer<typeof LedgerEvent>;

export const LogRecord = z.object({ id: EventId, epoch: Epoch, at: count, event: LedgerEvent });
export type LogRecord = z.infer<typeof LogRecord>;

export const Peer = z.object({ node: NodeId, addrs: z.array(UnicodeText), connected: z.boolean() });
export type Peer = z.infer<typeof Peer>;

export const JobSummary = z.object({ job: JobId, standing: Standing, epoch: Epoch, goal: UnicodeText });
export type JobSummary = z.infer<typeof JobSummary>;

export const ClosedJob = z.object({ job: JobId, error: UnicodeText });
export type ClosedJob = z.infer<typeof ClosedJob>;

export const Fact = z.object({ name: UnicodeText, value: UnicodeText });
export type Fact = z.infer<typeof Fact>;

export const DoctorReport = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("facts"), facts: z.array(Fact) }),
  z.object({ kind: z.literal("compared"), peer: NodeId, rows: z.array(PreflightRow) }),
]);
export type DoctorReport = z.infer<typeof DoctorReport>;

// The client's environment (ADR 0018, The client's environment).

/**
 * An absolute path as the operating system resolves it: text that starts
 * with `/` and holds no NUL, as Rust's `SystemPath`. Unlike the core's one
 * text form, it keeps `//`, `.`, `..`, a trailing `/`, a `.git` component
 * and any Unicode form, because the kernel reads it: a tool root or a
 * `PATH` directory names the directory the client's shell names. A
 * relative path would name a directory of the node's choosing.
 */
export const SystemPath = UnicodeText
  .refine((text) => text.startsWith("/") && !text.includes("\u0000"), "not an absolute path")
  .brand<"SystemPath">();
export type SystemPath = z.infer<typeof SystemPath>;

/** One directory of a `PATH`: a `SystemPath` without a `:`, which would split it in two. */
export const PathDir = UnicodeText
  .refine(
    (text) => !text.includes(":") && SystemPath.safeParse(text).success,
    "not an absolute PATH directory without a `:`",
  )
  .brand<"PathDir">();
export type PathDir = z.infer<typeof PathDir>;

/** The tool roots a node takes from a client; a root that is absent is unset for the request. */
export const Roots = z.object({
  cargo_home: optional(SystemPath),
  rustup_home: optional(SystemPath),
  pnpm_home: optional(SystemPath),
  xdg_data_home: optional(SystemPath),
});
export type Roots = z.infer<typeof Roots>;

/**
 * The environment a request runs in: the node looks programs up on `path`
 * and runs each tool with `roots`, instead of its own. `roots` reads as
 * `{}` when absent and is always written.
 */
export const ClientEnv = z.object({
  path: z.array(PathDir),
  roots: Roots.default(() => ({})),
});
export type ClientEnv = z.infer<typeof ClientEnv>;

/** The environment variable each root stands for. */
const ROOT_VARS: Readonly<Record<keyof Roots, string>> = {
  cargo_home: "CARGO_HOME",
  rustup_home: "RUSTUP_HOME",
  pnpm_home: "PNPM_HOME",
  xdg_data_home: "XDG_DATA_HOME",
};

/** The capability a node lists for a root it applies, as Rust's `Roots::capabilities()`. */
function rootCapability(root: keyof Roots): string {
  return `env.roots.${root}`;
}

/**
 * The capability a node lists for each root it applies, in the order a
 * welcome lists them after the ops. A client sends an environment only to
 * a node that lists every root the client knows, so an older node never
 * measures with its own value in place of the client's.
 */
export const ROOT_CAPABILITIES: readonly string[] = Roots.keyof().options.map(rootCapability);

/**
 * The environment of this process, as Rust's `ClientEnv::from_vars`: its
 * `PATH` without the entries that are not absolute, such as `.`, an empty
 * one or a relative one, which would name a directory of the node's
 * choosing, and each root it sets to an absolute path. Every entry and
 * root it keeps is kept as written, so the node finds the tools the
 * client's shell finds.
 *
 * Throws `EnvError` for an absolute entry or root that holds U+FFFD. Node
 * and Bun hand a variable that is not UTF-8 to JavaScript with U+FFFD in
 * place of each byte they cannot decode, so `/opt/caf\xe9/bin` arrives as
 * `/opt/caf\ufffd/bin`, a directory that does not exist. Sending it would
 * have the node measure without the client's directory, which Rust
 * prevents by failing the command; JavaScript cannot tell such a value
 * from a path that holds U+FFFD itself, so it refuses both.
 */
export function clientEnvFromProcess(vars: Readonly<Record<string, string | undefined>> = process.env): ClientEnv {
  const path = (vars["PATH"] ?? "").split(":").flatMap((dir) => {
    const parsed = PathDir.safeParse(utf8Path("PATH", dir));
    return parsed.success ? [parsed.data] : [];
  });
  const roots: Roots = {};
  for (const field of Roots.keyof().options) {
    const variable = ROOT_VARS[field];
    const value = vars[variable];
    const parsed = value === undefined ? undefined : SystemPath.safeParse(utf8Path(variable, value));
    if (parsed?.success) {
      roots[field] = parsed.data;
    }
  }
  return { path, roots };
}

/** `value` of `variable`, refused when it is absolute and holds U+FFFD, as `clientEnvFromProcess` says. */
function utf8Path(variable: string, value: string): string {
  if (value.startsWith("/") && value.includes("\ufffd")) {
    throw new EnvError(variable, value);
  }
  return value;
}

// Requests and replies.

const env = optional(ClientEnv);

/** What a client asks a node; an unknown op is an error. */
export const Request = z.discriminatedUnion("op", [
  z.object({ op: z.literal("node_id") }),
  z.object({ op: z.literal("node_add"), node: NodeId, addr: optional(UnicodeText) }),
  z.object({ op: z.literal("node_list") }),
  z.object({ op: z.literal("node_ping"), node: NodeId }),
  z.object({ op: z.literal("job_new"), repo: SystemPath, goal: UnicodeText, harness: Harness }),
  z.object({ op: z.literal("job_list") }),
  z.object({ op: z.literal("job_status"), job: JobId }),
  z.object({ op: z.literal("job_log"), job: JobId }),
  z.object({ op: z.literal("job_run"), job: JobId }),
  z.object({
    op: z.literal("job_resume"),
    job: JobId,
    on: optional(NodeId),
    harness: optional(Harness),
    force: z.boolean(),
    env,
  }),
  z.object({ op: z.literal("job_suspend"), job: JobId, env }),
  z.object({ op: z.literal("job_move"), job: JobId, to: NodeId, force: z.boolean(), env }),
  z.object({ op: z.literal("job_steal"), job: JobId, reason: UnicodeText, force: z.boolean() }),
  z.object({ op: z.literal("job_decide"), job: JobId, decision: Decision, note: UnicodeText }),
  z.object({ op: z.literal("doctor"), peer: optional(NodeId), job: optional(JobId), env }),
  z.object({
    op: z.literal("harness_started"),
    job: JobId,
    epoch: Epoch,
    pid: Pid,
    harness: Harness,
    session: optional(SessionId),
  }),
  z.object({
    op: z.literal("harness_session"),
    job: JobId,
    epoch: Epoch,
    pid: Pid,
    harness: Harness,
    session: SessionId,
  }),
  z.object({ op: z.literal("harness_exited"), job: JobId, pid: Pid }),
]);
export type Request = z.infer<typeof Request>;
export type RequestOf<K extends Op> = Extract<Request, { op: K }>;

/** One ACP agent's settings in the node's `harness.toml`. */
export const AcpSettings = z.object({ state_dir: Path });
export type AcpSettings = z.infer<typeof AcpSettings>;

/** The node's harness configuration, which a launch from a run plan must use. */
export const HarnessConfig = z.object({
  acp: z.record(UnicodeText, AcpSettings).default(() => ({})),
});
export type HarnessConfig = z.infer<typeof HarnessConfig>;

const runPlan = {
  job: JobId,
  epoch: Epoch,
  root: Path,
  harness: Harness,
  session: optional(SessionId),
  goal: UnicodeText,
  harness_config: HarnessConfig.default(() => ({ acp: {} })),
};
const lease = { owner: NodeId, epoch: Epoch };

/** The result of a request; its kind is the request's op, and an unknown kind is an error. */
export const Reply = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("node_id"), node: NodeId, listen: SocketAddr }),
  z.object({ kind: z.literal("node_add"), node: NodeId, addr: optional(UnicodeText) }),
  z.object({ kind: z.literal("node_list"), peers: z.array(Peer) }),
  z.object({ kind: z.literal("node_ping"), node: NodeId, rtt_ms: count, wire: Wire, clock_skew_ms: signedCount }),
  z.object({ kind: z.literal("job_new"), job: JobId, root: Path, repo: Path, harness: Harness, epoch: Epoch }),
  z.object({ kind: z.literal("job_list"), jobs: z.array(JobSummary), closed: z.array(ClosedJob) }),
  z.object({
    kind: z.literal("job_status"),
    job: JobId,
    node: NodeId,
    goal: UnicodeText,
    harness: Harness,
    owner: NodeId,
    epoch: Epoch,
    view: View,
    snapshot: optional(SnapshotState),
    workspace: optional(Workspace),
    process: Process,
    records: count,
  }),
  z.object({ kind: z.literal("job_log"), left_out: count.default(0), records: z.array(LogRecord) }),
  z.object({ kind: z.literal("job_run"), ...runPlan }),
  z.object({ kind: z.literal("job_resume"), ...runPlan }),
  z.object({ kind: z.literal("job_suspend"), snapshot: SnapshotId }),
  z.object({ kind: z.literal("job_move"), ...lease }),
  z.object({ kind: z.literal("job_steal"), ...lease }),
  z.object({ kind: z.literal("job_decide"), text: UnicodeText }),
  z.object({ kind: z.literal("doctor"), report: DoctorReport }),
  z.object({ kind: z.literal("harness_started"), epoch: Epoch, harness: Harness, pid: Pid }),
  z.object({ kind: z.literal("harness_session"), harness: Harness, session: SessionId }),
  z.object({ kind: z.literal("harness_exited") }),
]);
export type Reply = z.infer<typeof Reply>;
export type ReplyTo<K extends Op> = Extract<Reply, { kind: K }>;

export type NodeIdentity = ReplyTo<"node_id">;
export type Paired = ReplyTo<"node_add">;
export type PeerList = ReplyTo<"node_list">;
export type Pong = ReplyTo<"node_ping">;
export type JobCreated = ReplyTo<"job_new">;
export type JobList = ReplyTo<"job_list">;
export type JobStatus = ReplyTo<"job_status">;
export type JobLog = ReplyTo<"job_log">;
export type RunPlan = ReplyTo<"job_run" | "job_resume">;
export type Suspended = ReplyTo<"job_suspend">;
export type Lease = ReplyTo<"job_move" | "job_steal">;
export type Registered = ReplyTo<"harness_started">;
export type SessionRecorded = ReplyTo<"harness_session">;

// The exchange.

/** The client's first frame on a connection. */
export const Hello = z.object({ api: z.number().int().min(0).max(U32_MAX), client: UnicodeText });
export type Hello = z.infer<typeof Hello>;

/** What a node says about itself when it accepts a client. */
export const Welcome = z.object({
  type: z.literal("welcome"),
  api: z.number().int().min(0).max(U32_MAX),
  node: NodeId,
  node_version: UnicodeText,
  capabilities: z.array(UnicodeText),
});
export type Welcome = z.infer<typeof Welcome>;

/** The node's answer to a hello; after `incompatible` the node closes. */
export const HelloAnswer = z.discriminatedUnion("type", [
  Welcome,
  z.object({
    type: z.literal("incompatible"),
    api: z.number().int().min(0).max(U32_MAX),
    node_version: UnicodeText,
  }),
]);
export type HelloAnswer = z.infer<typeof HelloAnswer>;

/** A frame from the node once it welcomed the client: events, then one final frame. */
export const Answer = z.discriminatedUnion("type", [
  z.object({ type: z.literal("event"), event: Event }),
  z.object({ type: z.literal("done"), reply: Reply }),
  z.object({ type: z.literal("refused"), refusal: Refusal }),
]);
export type Answer = z.infer<typeof Answer>;

/** Whether `reply` answers `op`. */
export function isReplyTo<K extends Op>(reply: Reply, op: K): reply is ReplyTo<K> {
  return reply.kind === op;
}

/**
 * The capabilities, beyond its op, a node must list for `request` to mean
 * what it says, as Rust's `Request::needs()`: with an environment, one for
 * every root this client knows, set or not. A root left unset must be
 * removed from the tool's environment, never the daemon's own, and a node
 * that does not know the root would leave the daemon's value in place.
 */
export function capabilitiesNeeded(request: Request): readonly string[] {
  const env = "env" in request ? request.env : undefined;
  return env === undefined ? [] : ROOT_CAPABILITIES;
}

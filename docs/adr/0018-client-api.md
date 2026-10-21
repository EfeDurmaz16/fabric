# ADR 0018: The client API

Status: accepted, 2026-10-21

Supersedes the paragraph of ADR 0016 on the control socket's encoding (one request and one response, each a `u32` length and postcard bytes) and its Alternative 3, which rejected a JSON control protocol. Everything else in ADR 0016 stands: the socket's place and its owner-only trust, the exit codes, and the split of work between the node and the CLI.

## Context

The thesis says a job can be created, watched, moved, resumed, approved or stopped from anywhere. Fabric ships as a library, an SDK and a CLI, not as an MCP server. Today the only client is the CLI. It talks to the node over the control socket with postcard frames, and most answers are lines of text the node already rendered for a terminal. That serves the CLI and nobody else:

- postcard has no maintained implementation outside Rust, and it encodes an enum variant by its index, so a reader in another language would have to mirror the Rust declaration order exactly. ADR 0016 already had to append `HarnessSession` at the end of the request enum to keep the other indices.
- A program cannot read a job's owner, epoch or workspace from `["owner: fab1... at epoch 3", ...]` without parsing English.
- A refusal is a kind and a sentence. The kind decides the exit code; the facts behind it (which node owns the job, at which epoch, which move is open) are only in the sentence.
- A long command (`move`, `resume`, `run`) reports its steps as lines collected until it ends. Nothing typed comes out while it runs, so a phone cannot show a move in progress.

This ADR fixes the contract that any program, in any language, uses against a local node. The first two clients are the Rust crate `fabric-client`, which the CLI now uses for every request, and the TypeScript SDK in `sdk/typescript`.

## Decision

### Wire format: length-prefixed JSON on the same socket

The client API runs on `<state>/fabric.sock`, the control socket of ADR 0016. Each frame is a `u32` little-endian length, 1 to 16 MiB, then that many bytes of one UTF-8 JSON object. A reader checks the length against the cap before it allocates. A node parses a hello or a request only up to 1 MiB and refuses a larger one unparsed, a request with `bad_request` and a hello with `incompatible`: parsing a frame takes up to about thirty times its size, for a frame of small values such as a long array of zeros in a field the node does not know, so a 16 MiB request took over 500 MiB of the node's memory, and a client never needs to send more than the free text of a goal or a note. Node-to-node traffic (`fabric-net`, ADR 0013), the ledger and the manifests stay postcard.

JSON wins on this socket for four reasons:

- Every language a client is likely to be written in reads it without a dependency: `JSON.parse` in JavaScript, `json` in Python, `encoding/json` in Go, `serde_json` in Rust, which the node already uses for its local files.
- It is self-describing. A tagged object names its variant, so adding a variant or a field never shifts what an old reader sees, and a reader can skip what it does not know (see Compatibility).
- A person can read it with `socat` while debugging a client.
- The messages are small and local: an answer is a few hundred bytes, and the largest, a job's log, is bounded by the frame cap. Its size and parse cost do not matter here as they do for chunks on the network, once the node bounds what it parses from a client.

The length prefix, rather than one JSON object per line, lets a reader refuse an oversized frame before it reads it, and it is the framing every other socket in Fabric already uses.

Encoding rules, the same in every message:

- Enums are objects tagged by one field: `op` for a request, `type` for a frame from the node, `kind` for a reply, an event and most nested enums, `code` for a refusal reason. Tags and field names are `snake_case` strings. A variant without data, such as a decision, a standing or a refusal kind, is its name as a string. A reader refuses a number where a tag or a name belongs, at any depth, so no client comes to depend on the order in which the Rust types declare their variants, and an object where a name belongs, such as `{"approve": null}` for `"approve"`, which serde reads as the same variant, so no client comes to depend on a second encoding one reader takes and the other refuses.
- A record is a JSON object with named fields, never an array of its values in field order. A reader refuses an array where a record belongs, at any depth, so no client comes to depend on the order in which the Rust types declare their fields.
- Ids travel in their text form: a job, move or event id as its 26-character ULID, a node id as `fab1` and 52 base32 characters, a snapshot or env spec id as 64 lowercase hex digits, a session id as the harness wrote it. Each is parsed on arrival, as the core parses it; an id that does not parse refuses the whole message.
- An epoch is a JSON integer of at least 1. Counts, sizes, pids and times in Unix milliseconds are integers below 2^53, so a JavaScript number holds them exactly. A reader refuses an integer beyond 2^53 - 1 or below its negation, wherever it stands, and a writer writes such an integer as that bound. Only a value a peer reported can be that large, such as a ledger record's time from a broken or forged clock, a fork's epoch or a peer's clock in `node_ping`; the node relays those, and refusing the frame instead would fail every later read of the job.
- A string is Unicode text. A lone UTF-16 surrogate, which a JavaScript string may hold, such as half an emoji cut off by `.slice()`, is not, and every reader and writer refuses one, in a value or a key.
- Text a request names that a client later passes to a process as an argument, the goal of `job_new` and the arguments of an ACP harness in any request, holds no NUL, which no process argument can; a reader refuses such a request, so the node never records a job it cannot launch. A reply is not checked: it carries what a node already recorded, which a client must still be able to read.
- An optional field is absent when it has no value, never `null`. Readers accept `null` as absent.
- Paths are strings. A path that is not UTF-8 cannot cross the socket and fails the request that names it. An answer that holds one, such as a job's root under a home that is not UTF-8, becomes an `internal` refusal that says the command ran, so a client does not ask for it again, and an event that holds one is dropped. A path a client sends for the node to use, the repository of `job_new`, a `PATH` directory or a tool root, must be absolute and is refused otherwise, since the node would read a relative one against the directory it was started in.

### The exchange

One connection carries one exchange:

```text
client: Hello      { api: 1, client: "<name>/<version>" }
node:   Welcome    { type: "welcome", api: 1, node, node_version, capabilities: [op, ..., field, ...] }
      | Incompatible { type: "incompatible", api, node_version }        and the node closes
client: Request    { op: ..., ... }
node:   Event      { type: "event", event: { kind: ..., ... } }         zero or more, as they happen
node:   Done       { type: "done", reply: { kind: <the request's op>, ... } }
      | Refused    { type: "refused", refusal: { kind, reason: { code, ... }, message } }
                                                                         and the node closes
```

The node streams each event the moment the step it reports happens, and the final frame follows the last event. A client that disconnects does not stop the command, and a client that stays connected but stops reading, such as a CLI stopped with Ctrl-Z or writing to a paused terminal, does not pause it: the command runs as its own task, and its events wait in the node's memory until the client reads them or leaves. A move or a suspend runs to its end on the node, as it did before, and never waits on a socket while it holds the job's lock, because stopping halfway is what the ledger protocol already guards against. The node gives a connection 30 seconds to send its hello and its request.

A request that does not parse, or names an op the node does not serve, is answered `Refused` with the reason `bad_request`. A hello that does not parse is answered `Incompatible`.

### The message set (api 1)

```text
Request (op)   = node_id | node_add { node, addr? } | node_list | node_ping { node }
               | job_new { repo, goal, harness } | job_list | job_status { job } | job_log { job }
               | job_run { job, env? } | job_resume { job, on?, harness?, force, env? }
               | job_suspend { job, env? } | job_move { job, to, force, env? }
               | job_steal { job, reason, force } | job_decide { job, decision, note }
               | doctor { peer?, job?, env? }
               | harness_started { job, epoch, pid, harness, session? }
               | harness_session { job, epoch, pid, harness, session }
               | harness_exited { job, pid }
Harness (kind) = claude_code | codex | acp { program, args }
ClientEnv      = { path: [dir], roots: { cargo_home?, rustup_home?, pnpm_home?, xdg_data_home? } }
```

### The client's environment

A node runs tools on a client's behalf: it measures the facts `doctor` compares and the carry keys a snapshot records (ADR 0012) by running `rustc`, `pnpm`, `node` and the rest, and it finds the harness a capture or a restore needs. Those tools answer by the environment they run in. The node used to run them with its own environment and only the caller's `PATH`, so a capture recorded the daemon's `CARGO_HOME` as the key of `target/`, not the one of the shell that asked (review finding on PR #8), and `pnpm store path` and the `rustc` a rustup proxy picks followed the daemon's variables in the same way.

`job_run`, `job_resume`, `job_suspend`, `job_move` and `doctor` take `env`, the caller's environment as data:

- `path` is the caller's `PATH` as a list of directories, each an absolute path without a `:`, kept as the shell has it: `//`, `..`, a trailing `/` or a decomposed accent stay as written, because the kernel resolves them and the node must find the tools the shell finds. This is not the core's one text form of a path, which is for paths Fabric itself writes. A relative entry or `.` would name a directory of the node's choosing, so both clients leave such entries out and the node refuses a request that holds one.
- `roots` is a closed set of tool roots, one field each: `cargo_home`, `rustup_home`, `pnpm_home` and `xdg_data_home`, each an absolute path kept as written, as `path` is. They are the variables that decide where the measured tools keep their state: cargo and rustup by their homes, pnpm its store by `PNPM_HOME`, then `XDG_DATA_HOME`.

For that request the node looks programs up on the client's `path` and runs each tool with every root set to the client's value or removed when the client did not set it, never the daemon's own. When `env` is absent, and for work no client asked for, such as the capture a peer's pull asks for or the facts a peer's preflight compares, the node uses its own environment: the one the last local client sent, which it keeps in `<state>/client-env` across restarts, or the one the daemon started with until a client sends one. A daemon is rarely started from the user's shell, so measuring a peer's side in the daemon's environment while the CLI's side is measured in the shell's compared two `CARGO_HOME` values of one user and rebuilt `target/` where it could be carried (review finding on PR #13). `job_run` takes `env` for this alone: the node measures nothing for it, but a node that only ever ran a job would otherwise keep the daemon's environment. The daemon's own environment never crosses the socket, so the node keeps it as the system gave it: a `PATH` entry or a root of the daemon's own that is not UTF-8 is used as it is and does not stop `fabric serve` from starting. A node lists each root it applies in `Welcome.capabilities` as `env.roots.<root>`, such as `env.roots.cargo_home`, and a client sends `env` only to a node that lists every root the client knows, set or not, failing with `UnsupportedField` otherwise. A root the client sets must reach the tool and one it leaves unset must be removed from the tool's environment; a node that does not know the root does neither, so it would measure with the daemon's value and nobody would know. A new root is therefore an additive change: a newer client does not send `env` to an older node and names the root that node lacks, instead of letting it measure with its own value. A client that does not know a newer root cannot send it, so the node runs the tool with that root unset, as for a shell that does not set it. `ClientEnv::current()` reads the calling process's environment, and the CLI sends it with each of these requests. An absolute entry or root that is not UTF-8 cannot cross the socket, so reading it fails the command rather than leaving the directory out. The TypeScript SDK's `clientEnvFromProcess()` reads the variables as JavaScript strings, in which Node and Bun put U+FFFD for each byte they cannot decode, so it refuses an absolute entry or root that holds U+FFFD; it cannot tell one from a path that holds U+FFFD itself.

A reply's `kind` is its request's `op`, so a client checks that the answer is the one its request takes and refuses any other as a protocol error. The replies are typed records: `node_id { node, listen }`, `node_list { peers: [{ node, addrs, connected }] }`, `node_ping { node, rtt_ms, wire, clock_skew_ms }`, `job_new { job, root, repo, harness, epoch }`, `job_list { jobs: [{ job, standing, epoch, goal }], closed: [{ job, error }] }`, `job_status { job, node, goal, harness, owner, epoch, view, snapshot?, workspace?, process, records }`, `job_log { records: [{ id, epoch, at, event }] }`, `job_run` and `job_resume` with the fields of a run plan, `job_suspend { snapshot }`, `job_move { owner, epoch }`, `job_steal { owner, epoch }`, `job_decide { text }`, `doctor { report }`, `harness_started { epoch, harness, pid }`, `harness_session { harness, session }`, `harness_exited {}`. `crates/fabric-client/src/api.rs` holds every field with its meaning, and `protocol/fixtures` holds one example of each.

`job_status` says what `fabric job status` printed, as data: the node's `view` of the job (`owner`, `departing { move_id, target }`, `departed { owner, epoch }`, `replica { owner, epoch }` or `frozen { peer, epoch }`), what the workspace holds (`current { marker }`, `stale { left_at }`, `awaiting_decision { target }`, `clean_copy` or `orphan`), and whether a harness runs (`not_running`, `running { pid, epoch }` or `group_remains { pid }`). A local record the node cannot read is reported in its place, so the rest of the status still arrives: `view` is `unreadable { error }` for the fork record, `workspace_unreadable` holds the error for the workspace marker, and `process` is `unreadable { error }` for the harness record. The CLI prints the lines before the first of them and then fails with its error, as it did when the node stopped there.

`doctor` answers a report, `facts { facts }` or `compared { peer, rows }`, and never refuses because a row would stop a move: a diagnostic reports. The CLI keeps exit code 84 for `fabric doctor --peer` when a row refuses, computed from the rows.

### Progress as typed events

Every line the node used to print is now an event with typed fields: `stopping_harness { pid }`, `snapshot_published { snapshot, files, bytes, new_objects }`, `left_behind { path }`, `preflight { peer, rows }`, `move_started { move_id, target }`, `imported { owner, epoch }`, `restored { snapshot, files, bytes }`, `continuing_session { harness, session }` and the rest of the list in `api.rs`. An event that reports a failure the command survived, such as a peer the ledger could not be copied to, carries that failure as a `Refusal`. Three events carry a sentence where the node's explanation has no stable shape yet: `cache_rebuilt { path, why }`, `move_gave_up { detail }` and `not_replicated`'s refusal message.

The CLI renders each event and each reply to the exact line the node printed before, so `fabric`'s output and exit codes do not change; the lines now appear as the steps happen instead of all at the end. Work no client asked for, such as start-up recovery, catch-up, a peer's move request or a fork, keeps its events, and the node logs each as the same line, so the log of `fabric serve` reads as it did.

### Refusals

```text
Refusal = { kind, reason: { code, ... }, message }
kind    = not_owner | frozen | unreachable | needs_force | job_state | failed
```

`kind` is the class ADR 0016's exit codes are picked from, unchanged: 81 to 85 and 1. `reason` names the fact behind it with its data, for example `not_owner { job, owner, epoch }`, `departing { job, move_id, target }`, `forked { job, peer, epoch }`, `unfinished { job, step }`, `preflight { facts }` or `harness_running { job, pid }`; a failure with no structure worth typing, a file or the store, is `internal`. A peer's answer that names a fact is typed the same way: `peer_ahead { job, peer, epoch }` when the peer knows a newer epoch, `peer_behind { job, peer, epoch }` when it must catch up first, `peer_forked { job, peer, head }` when its ledger does not continue this node's, `move_unlinked { job, peer, head }` and `move_stale { job, peer, epoch }` when a pulled move does not fit this node's ledger, and `fetch { snapshot, peer }` when a snapshot did not arrive whole. Only an answer the protocol does not allow at that point, or a transport failure, is `peer { peer }`. All of these keep the kind `failed`, so the exit code is 1 as before. `message` is the node's sentence, with the command that resolves it, which the CLI prints after `error:`. A reader checks that a known reason belongs to the kind it came with and refuses the frame otherwise.

Errors that happen before the node answers are the client's own, typed in each client: no daemon at the socket (the CLI's exit 80), a state directory or socket that is not this user's alone (`Untrusted`), an incompatible api, an op the node does not list, a frame over the cap, a frame that does not parse, and a connection closed before the final frame.

### Compatibility

1. The hello carries the client's `api`. The node answers `Welcome` when it equals its own and `Incompatible` with its own number otherwise, and closes. The client also checks the number in `Welcome`. There is no range: a node speaks one api version.
2. Within one api version, a change is additive:
   - a new op, listed in `Welcome.capabilities`; a client sends an op only when the node lists it and otherwise fails with `Unsupported` before sending;
   - a new optional field in a request, which a node that does not know it ignores, so the node lists it in `Welcome.capabilities` as `<op>.<field>`, or `env.roots.<root>` for a root of the client's environment, and a client sends it only to a node that lists it, failing with `UnsupportedField` otherwise. A root counts as sent whenever `env` is, since leaving it unset asks the node to remove it. A node of api 1 lists its ops and then the four roots;
   - a new field in a reply, an event or a refusal reason, which readers ignore, except in what a client launches: a run plan, its `harness_config` and a harness anywhere it appears. Both clients refuse a field they do not know there, because a client applies every setting of a plan, and one that skipped a new field would launch a harness configured otherwise than the node captures and restores it, then report the harness without the field in `harness_started`, which the node records as the job's. A new field there falls under rule 3;
   - a new event kind, which a client skips: events report progress and never carry the result;
   - a new refusal reason, which a client reads as `unrecognized` with its `kind` and `message` intact, so the exit code and the sentence still work;
   - a new kind of ledger record in `job_log`, which a client reads as `unrecognized`, since a later ledger format may write one.
3. Anything else bumps `api`: removing or renaming an op, a field or a kind, changing a field's type or meaning, a new refusal `kind` (exit codes depend on the closed set), a new reply shape for an existing op, or a change to the exchange after the hello.
4. Three things never change, bump or not, because they are how a client learns that a node speaks another api: the frame header (a `u32` little-endian length, then one JSON object), the `incompatible` answer `{ type, api, node_version }`, and the two hello answer types, `welcome` and `incompatible`. A client of every earlier api must still read them to tell its user to upgrade; changed at a bump, they would turn that message into a frame the older client cannot parse. The hello may change with a bump, since a node answers a hello it cannot read with `incompatible`. A range of versions (Alternative 8) would therefore come as a new field of `incompatible`, which an older client ignores, never as a new shape of it.

The Rust crate and the TypeScript SDK both declare `API_VERSION = 1`, and a contract test pins it. The fixtures in `protocol/fixtures` follow the same rule: once a fixture is on the base branch, CI refuses a change that edits, renames or removes it unless `API_VERSION` changes too (`scripts/check-fixtures.sh`), so a new shape gets a new fixture and an older client's example stays what that client must still read. The fixtures of rule 4, `frame/`, `hello_answer/incompatible.json` and `closed/hello_answer.json`, are refused any change even with a bump.

A new fixture alone cannot carry every change that rule 3 names, so two more pins sit beside the examples, held by the same script and checked by both contract suites:

- `protocol/fixtures/closed` lists, one file per closed enum, the tags of its variants: the harness kinds, standings, views, workspace states, markers, processes, outcomes, wires, steps, decisions, refusal kinds, doctor reports and the frame and hello answer types. A new variant of one of them fails both suites until its file lists it, and editing that file needs a bump. Ops, event kinds, refusal reasons and ledger record kinds are open and have no such file.
- `protocol/required.txt` lists, for each message fixture, every key whose removal makes the message refused. A field made optional fails both suites until its lines go, and a field made required until its lines come; the script refuses a removed line, or a line added for a fixture the base had, without a bump. A new fixture brings its own lines, which `FABRIC_PIN_REQUIRED=1 cargo test -p fabric-client --test contract` writes.

A CLI from before this ADR that reaches a node after it sends postcard, which does not parse as a hello; the node answers `Incompatible` and the old CLI fails to decode it. A new client that reaches an old node gets the connection closed without a welcome and says that the node may run an older fabric. Neither side misreads a frame.

### Security

Unchanged from ADR 0016. The socket lives in the state directory, mode 0700. The node refuses a connection whose peer user id is not its own. The client refuses a state directory or a socket another user owns, a symlink, or a directory others may enter, before it sends anything, because a run plan read there decides what it launches; `fabric-client` and the TypeScript SDK both make these checks. There is no network listener for the client API in this ADR. Remote clients, a phone included, come later with capability tokens (C4): a token names what its holder may do to which job, and the local socket stays the owner's alone.

Some fields name the node's own machine, not a job, and are safe only because whoever reaches the socket is the node's owner:

- `env` decides which binaries the node runs and with which tool roots, for the request and, since the node keeps the last one, for later work a peer asks for; `job_new`'s `repo` decides which directory it clones. A caller who could send `env.path` pointing into a workspace, whose files come from a repository or a peer's snapshot, could have the node run any program in it as the owner, now and on a peer's next pull.
- `harness_started`'s `pid` names a process group the node later stops: a suspend, a move or a fence sends that group `SIGTERM`, then `SIGKILL`. The node checks that the pid leads a group, not that the caller launched it, so a caller could have the node kill any of the owner's process groups.
- A harness, in `job_new`, `job_resume`, `harness_started` and `harness_session`, decides what the owner's CLI launches: `harness_started` records it as `Resumed`, the job's harness follows that record, and the next `fabric job run` executes an `acp` harness's `program` and `args` as the owner.

A token scoped to one op on one job cannot express or prevent any of these. So C4 must not pass these fields from a remote caller as they are: its listener refuses `harness_started`, `harness_session` and `harness_exited` from a remote caller, since only the client that launched the harness on this machine can report it, and refuses or drops `env`, `repo` and a harness in `job_new` or `job_resume` unless the token grants each by a scope of its own. Even a granted `env` from a remote caller applies to its request only: the node never keeps it as its own environment, which later work for peers runs in. A request without `env` measures with the node's environment, as it does today.

### The harness stays with the client that launches it

`job_run` and `job_resume` answer a launch plan: the job, the epoch it is fenced on, the root, the harness, the session to continue and the goal. The client that launches the harness, interactive or headless, reports it with `harness_started`, a session the harness names late with `harness_session`, and its end with `harness_exited`, as ADR 0016 describes. The API carries no terminal. Handing a terminal to the harness's process group (`crates/fabric/src/terminal.rs`) stays in the CLI, which owns the terminal; a headless run is the same plan launched without one.

### The seam for C2

C2 adds a live event stream and approvals from a phone. It builds on two things this ADR puts in place. Events are typed and are made in one place on the node, the per-request sink `Out` in `fabricd`, which today forwards to the one connection that asked. C2 turns that sink into a broadcast per job, and a new op, `watch { job? }`, listed as a capability, streams the same `Event` values to any client until it disconnects. Each watcher reads from its own queue, as the client that asked does today, so a slow watcher cannot hold a command. Approvals already exist as `job_decide`; C2 adds no new kind of refusal.

### Where the code lives

- `crates/fabric-client`: `api` (every type above, with serde), `frame` (the length-prefixed codec, both directions), `sentences` (the line each event reads as, which the CLI prints and the node logs), and `Client` (connect, the trust checks, the hello, one typed method per op). It depends on `fabric-core`, `serde`, `serde_json`, `thiserror`, `tokio` and `rustix`, all already in the workspace. The node uses `api` and `frame` to serve.
- `SessionId` moves from `fabric-harness` to `fabric-core`, so a client does not depend on the harness adapters.
- `crates/fabric/src/render.rs`: the human text for every reply, and each event's line from `sentences`.
- `sdk/typescript`: the TypeScript client for Node and Bun. Strict mode, named exports, zod schemas at the boundary and no other runtime dependency.
- `protocol/fixtures`: one JSON example of every hello, request, reply, event and refusal reason, some messages both clients must refuse, and one framed hello as hex. The Rust tests and the TypeScript tests both parse every fixture and write it back to the same JSON, so the two clients cannot drift apart without a test failing.

## Alternatives considered

1. **Keep postcard on the socket.** The CLI already speaks it, and it is compact. A client in another language would need a postcard implementation that follows the Rust declaration order of every enum, and there is none we could depend on. Rejected for the client API; kept where only Rust reads it.
2. **CBOR or MessagePack.** Self-describing and smaller than JSON, but each needs a library in every client language, and the size does not matter on a local socket. Rejected.
3. **Protocol Buffers or gRPC.** A schema and code generation in every language, an HTTP/2 stack, and a build step for the SDK. Too heavy for a local socket and against the minimal dependency rule. Rejected; worth a look again for C4 if remote clients need generated stubs.
4. **HTTP with JSON over the Unix socket.** Familiar to clients, but it needs an HTTP server in the node and gives nothing a length prefix does not give here. Rejected for now; C4's network listener may well be HTTP.
5. **One JSON object per line.** As easy to read in every language, but a reader learns a frame is too large only after it has read it. Rejected.
6. **Keep the node's text and add a parallel typed API.** Two answers to keep in step for every command. Rejected: the CLI is a client like any other.
7. **Batch events into the final answer.** Simpler, but it is the behavior C2 cannot build on and that leaves a user staring at a silent move. Rejected.
8. **An api version range with negotiation.** Nothing older than api 1 exists, and capabilities already cover additive changes. Rejected until a second version exists; then it is a new field of `incompatible` (Compatibility, rule 4).

## Consequences

- A program in any language can drive a local node with a socket and a JSON parser, and read typed answers.
- The CLI and the SDKs share one code path on the node; the CLI has no private protocol.
- The CLI prints each step as it happens.
- A change to the API shows up as a fixture change in review, and a client that does not follow fails its contract tests.
- ADR 0016's paragraph on postcard frames and its Alternative 3 are history.

## Open questions

- Should the node offer a headless launch for clients that cannot launch a harness themselves, such as a phone, or should that always go through a node-side client with the user's environment? Decide with C2.
- Variables that are not roots also change what a tool answers: `RUSTUP_TOOLCHAIN` picks a toolchain, `PYENV_VERSION` a Python, and `git` and the store run with the node's environment. Add them to the client's environment when a measured fact turns out to depend on one, each as a root with its capability.
- Should a client be able to keep one connection for several requests? Not needed while every request is local and cheap; reconsider with C4.
- Does C4's network listener speak this same JSON over TLS, or HTTP? Decide with C4. The message set can stay, but not what each field means to a remote caller: `env`, `job_new`'s `repo` and a harness need a scope of their own in the token, and the `harness_*` ops stay local (see Security), so a request a phone may send is not every request the local socket takes.

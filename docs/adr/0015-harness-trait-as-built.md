# ADR 0015: Harness trait as built

Status: accepted, 2026-10-20

Amended 2026-10-21 (gate finding F10): a resume reads the model and the other settings from the session and passes them, `Running::settings` says what was passed and what was not, and a harness older than the newest one that wrote the session is refused before it starts (`HarnessOlderThanSession`, `HarnessVersionUnknown`), under "Launch" and in ADR 0006, Settings travel with the session.

Amended 2026-10-21 (gate finding F5 and headless runs): `Running` can start held, before its exec, until its group owns the terminal (ADR 0006, Launching in the user's terminal), and the Claude and Codex adapters read what a headless turn prints into a `TurnReport`, under "Launch".

Supersedes the section "The `Harness` trait" of ADR 0006. The rest of ADR 0006 stands: which harnesses get adapters, the credential rules, opaque state, no path rewrite, and cross-harness resume as an experiment.

Written after the first implementation (`crates/fabric-harness`, branch `feat/harness`, PR #6) and two review rounds. ADR 0009 says an ADR is superseded, not edited, when a decision changes. Moving the trait out of the core and changing its signatures are decisions, so they are recorded here.

## Context

ADR 0006 put the trait in `fabric-core` with `stop`, a session-level `detect(workspace)`, `capture(workspace, out) -> Captured` and `restore(state, from_root, to_root)` with a rewrite. Building it showed four things.

- `launch` returns a handle that owns a child process, which is a `tokio` type. The core is pure: no IO, no clock, no process.
- `stop`, the session-level `detect` and the `Daemon` case need the supervisor of ADR 0003, which does not exist yet.
- ADR 0011 fixed the root to the same absolute path on every node, so a restore never rewrites a path and never needs `from_root` and `to_root`.
- The review found that "equal or byte prefix", the restore rule of ADR 0006, only fits files that grow. Claude rewrites memory, task files and sidecar metadata in place.

## Decision

### The trait

`fabric_harness::Harness` lives in `fabric-harness` and has six methods.

```text
trait Harness: Send + Sync {
    fn kind(&self) -> HarnessKind;
    fn detect(&self) -> BoxFuture<Result<Option<Installed>, HarnessError>>;
        Is the binary installed, at which path, with which version.
    fn capture(&self, workspace, session: &SessionId, out) -> BoxFuture<Result<StateManifest, HarnessError>>;
        Copy what `session` needs into `out`, redacted, and write `out/manifest.json` last.
    fn confirm_departure(&self, manifest: &StateManifest) -> BoxFuture<Result<(), HarnessError>>;
        The snapshot holding this capture is published; its departure record now counts.
    fn restore(&self, workspace, state) -> BoxFuture<Result<Restored, HarnessError>>;
        Place a captured tree on this node, all or nothing.
    fn launch(&self, workspace, Launch, env) -> BoxFuture<Result<Running, HarnessError>>;
        Start the harness in a new process group with `env` minus parent-session markers.
}
```

The session is an explicit argument, so an adapter never guesses which session a job continues. `ClaudeCode::latest_session` and `Codex::latest_session` exist for `fabric job init` to adopt a session the user started by hand. When ADR 0003's `RunningHarness` lands, the trait can move down into the core and return that handle; the method list does not change for that. `stop`, the session-level `detect` and the `Daemon` case come with the supervisor.

`capture` and `restore` copy files on a blocking thread. Dropping their future does not stop the copy, so the caller awaits them to completion and does not `launch` the same session until they return. Each adapter takes its home as an `AbsPath` and the `PATH` value to search in its constructor, and parses the workspace once into its physical `AbsPath`, which feeds the manifest, the Claude project key, the Codex trust key and the ACP `cwd`. The library never reads its own process environment except to probe a version, which runs with `PATH` set to the adapter's value; `PATH` lookup skips relative and empty entries, so the file probed is the file launched; a probe that exits with a failure is an error, never a version; `ps`, `pgrep` and `kill` run from `/bin` and `/usr/bin`, and a script path given to `pgrep -f` is escaped so it matches only itself. `out` belongs to the capture: a file in it that the new manifest does not list is removed before the manifest is written.

Every adapter captures and restores through `pipeline.rs`, so the order of the steps and the checks below are written once. An adapter supplies its sources and, at restore, where each tree path lands and how it treats that path.

`Launch` says where the process's input and output go and carries the first prompt: `Interactive { resume: Option<SessionId>, prompt: Option<String> }` runs in the user's terminal, starting held when standard input is a terminal (`Running::is_held`, `Running::release`, amended 2026-10-21), and `Headless { resume: Option<SessionId>, prompt: String }` runs without one. `claude -p` and `codex exec` read their first message from the command line, so a headless launch without a prompt cannot be built. An ACP agent speaks its protocol on its stdio and has no terminal mode, so its trait `launch` refuses `Interactive` (`NeedsClient`); `AcpHarness::launch_acp` returns the protocol client for a caller that wants to drive the session.

### The manifest and what restore may do

`capture` writes the tree under one prefix, `home/`, and then `manifest.json`. A `StateManifest` holds the harness kind, the session, the physical root, the home, the harness version, the redaction counts, and every file with its content kind, its length, its BLAKE3 digest and its mutability. Its fields are private. A manifest comes from `StateManifest::new` or from a file, and both check the same rules: format 1, every path under `home/`, no temporary file, strictly sorted. Staging removes an earlier `manifest.json` from `out` and fsyncs the directory before it writes anything, so a capture killed in a reused directory never leaves an old manifest naming files it has partly replaced.

A restore accepts only the root and home of the capture (`RootMismatch`, `HomeMismatch`, ADR 0011), only the destinations its adapter captures from (`UnexpectedFile`), only a manifest whose content kind and mutability for a path match the adapter's own rule (`EntryMismatch`), no destination that is a symlink or lies below one under the home (`SymlinkInState`), because a rename or a new directory there would write through it, and no two destinations that differ only in letter case (`DuplicateTreePath`), because APFS opens them as one file; capture refuses such a pair too. It checks every staged digest, then plans every destination, and writes only when all of them pass. A destination passes when:

- it is missing, equal to the incoming bytes, or a byte prefix of them;
- it is this node's own copy, and the incoming bytes extend what capture's redaction makes of it; the local bytes stay, secrets and signed blocks included, and only the incoming suffix is added;
- it is `Replaceable` and still holds the bytes this node recorded for the last capture known to have left.

Anything else is `StateDiverged { path }`, and nothing is written.

Mutability is part of the data. `AppendOnly` covers transcripts and rollouts, which only grow. `Immutable` covers tool results, file-history backups and uploads, which are written once. `Replaceable` covers everything Claude rewrites in place (memory, task files, sidecar metadata) and all ACP state, which Fabric cannot see into. At capture the adapter records the digest of each `Replaceable` file on this node, taken in the same read that staged it, so an edit made after staging is never recorded as having left, as a pending record, `~/.fabric/harness/departed/<harness>/<session>.pending.json`, keyed by the digest of the manifest it writes next. For ACP, `<harness>` is `acp-<program>-` plus a digest of the program and its arguments, so two agents started through one program never share a record. Restore ignores a pending record. Once the snapshot that holds the capture is published, the core calls `confirm_departure` with the manifest, and the pending record becomes `<session>.json`, the one restore reads. A publish that fails, or a kill between capture and publish, therefore never moves the record forward: a restore of the older snapshot finds the file edited since and refuses (`StateDiverged`). Confirming a capture that a later one replaced, or one made on another node, is `DepartureNotPending`. Records never travel. A job that went from the Pro to the Air and comes back replaces the Pro's `MEMORY.md` with the Air's only if the Pro's file was not edited after the job left.

Every write goes through a temporary sibling, created exclusively so it never writes through a symlink, an fsync and a rename, keeps the mode of the file it replaces (0600 for a new one), and fsyncs the directory, and the parent of every directory it creates on the way. Crash tests send `SIGKILL` at named steps (the test-only `crashpoints` feature): a Claude capture after each staged file and after its pending departure record, a Claude restore after each placed file, and a Codex restore after its first placed file. They check that no manifest a fresh node would accept is left, that every placed file is whole, that a departure that never left moves nothing forward, and that the next run converges. ACP capture and restore share the same pipeline but have no crash test of their own. A process killed between a rename and the directory fsync loses nothing, because the page cache outlives it, so that window needs a power cut to test, not a kill.

### What each adapter carries

- Claude: `projects/*/<sid>.jsonl` and `projects/*/<sid>/` across every project key (sidecars of one session do spread across keys), the project memory under the workspace's key, `file-history/<sid>/` (filtered, below), `tasks/session-<first 8 of sid>/` and `uploads/<sid>/`. The project key is `slug::slug` of the physical path: every UTF-16 code unit outside ASCII letters and digits becomes `-`, as the JavaScript regex in Claude's bundle does, so a character outside the Basic Multilingual Plane becomes two; a key over 200 characters is refused (`ProjectKeyTooLong`) because Claude's hash suffix is undocumented. Files inside the workspace (`.claude/`, `CLAUDE.local.md`) are the workspace snapshot's to carry (ADR 0004). A symlink in session state, at `~/.claude` itself or above it under the home, fails the capture (`SymlinkInState`); a symlinked Codex store fails capture and restore the same way.
- Codex: the thread's rollouts, found by file name under `sessions/` and `archived_sessions/` at any depth, as Codex's own scan does, following `session_meta.history_base.thread_id` through plain and `.zst` files. A missing prefix is `LineageBroken`. Two files with one rollout id are accepted only when one is a byte prefix of the other, and the longer one is used; otherwise `LineageAmbiguous`. Only the ids the thread's lineage reaches are resolved, so a forked rollout of another thread blocks nothing. Restore derives the lineage again from the staged files and refuses any rollout off it, places each rollout over the file that already holds it on this node, so a thread that comes back does not fork, and puts a rollout this node has never seen under `sessions/fabric/<thread_id>/`.
- ACP: the files under the declared state directory that are named for the session (below). An undeclared state directory is reported when capture or restore is asked for (`StateDirNotDeclared`), not at `detect`, because `detect` here only answers whether the binary is installed.

### Running harnesses

Capture and restore refuse while the harness may be writing the session (`HarnessRunning`, `ActiveWriter`).

- Claude: the session registry `~/.claude/sessions/<pid>.json` answers only when it exists, every record in it parses, and the installed Claude is at least 2.1.294, the version it was read on. Otherwise any running `claude` process refuses the capture or restore. The Air runs 2.1.199 today, so there it means closing every Claude session first. A `ps` answer other than found or not found is an error, never "dead".
- Codex: Codex's own thread writer lock, held for the whole copy, taken as `codex-rs/rollout/src/writer_lock.rs` takes it. The lock is trusted only when the installed Codex is at least 0.155.1, the version it was read on. With an older or unknown version, any running `codex` refuses the capture or restore, because a Codex that takes no lock keeps appending to a rollout that restore renamed away.
- ACP: the adapter cannot see whether a generic agent writes a session. The caller owns that guard, through the handle `launch_acp` returns.

### Redaction as built

At capture every Claude file is redacted; Codex and ACP state travels byte for byte and is refused when a secret shape is found in it (`SecretInState { path, line }`), on the raw line or in any JSON string literal of it once decoded. Inside a JSON string a newline is `\n` and a quote is `\"`, which hide an env line or a quoted value from a match on raw bytes. A settings or MCP overlay is checked the same way.

- `.jsonl` and `.json` files go through the JSON scanner: every string outside a signed block is decoded, redacted and re-encoded. A signed block is a Claude content block whose signature the API checks: `type` `thinking` with a `signature`, or `type` `redacted_thinking`. Any other object with a `signature` key is ordinary JSON. Whether a signed block holds a secret is decided on its raw text and on each of its strings decoded. A signed block that holds a secret is dropped whole when it is an array element and counted as `signed-block-dropped`; a block nested in a dropped one goes with it; otherwise the capture fails (`SecretInSignedBlock`).
- Every other Claude file is redacted as text, line by line. A line that is not UTF-8 is redacted as bytes and otherwise kept. A PEM private key that spans lines is redacted as one block: the BEGIN line from its header on, every line up to the END line, and to the end of the file when no END line comes.
- Rules: Anthropic, OpenAI, GitHub, AWS, Slack, Google and Stripe key shapes (Stripe webhook secrets included), JWTs, PEM private key blocks, `Authorization` headers, bearer tokens whose value looks like a token, credentials in URLs (`postgres://user:password@host`), and the value of an upper-case variable whose name says secret, token, password, API key, private key, access key, credentials, `DATABASE_URL` or `DSN`. Every key rule starts on an ASCII word boundary, so `sk-` inside `disk-usage-analyzer-service` is not a key. A rule that does not compile fails every capture.
- `file-history/<sid>/` holds Claude's verbatim backups of edited files, named by a hash of the path. The transcripts' `file-history-snapshot` records map each backup to the path it tracks. A backup whose original is on the denylist stays behind, and so does a backup no record names; both are counted. The original is checked as recorded and as the tree path it resolves to under the home, so `/Users/u/.claude/history.jsonl` or `../.codex/config.toml` meets the host-state rules.
- Not done: the account email and the organization UUID in `session_context` and `credential_org` attachments, which ADR 0006 lists. No transcript read so far showed where those fields sit, and a guessed field path would redact the wrong thing or nothing. They land with the first live capture on the gate, when the shape can be read from a real transcript. Until then the account email can travel inside the job's own state, between the user's own two machines.

### The denylist

`denylist::check` refuses credential names (`.credentials.json`, `auth.json`, `.claude.json`, `.netrc`, `.npmrc`, `.pypirc`, `.git-credentials`, `.pgpass`, `credentials.tfrc.json`, `id_rsa`, `id_dsa`, `id_ecdsa`, `id_ed25519`, `.env*` except examples), key and database suffixes, credential directories (`.ssh`, `.aws`, `.gnupg`, `.kube`, `.docker`, `.config/gh`), the host-state directories and files of both harnesses, and `~/.fabric`, Fabric's own node state. Names are compared after ASCII case folding, because APFS opens `AUTH.JSON` as `auth.json`. It runs inside staging before anything is copied, again on every manifest entry at restore, and on the original path of every file-history backup.

### Launch

- Claude: `claude --resume <sid> [--model <name>] <flags>` in the terminal, or `claude -p --output-format json --resume <sid> [--model <name>] <flags> -- <prompt>` headless. A launch with `resume: None` passes `--session-id <uuid4>`, so the id is known before the process starts. The flags are a typed `LaunchFlags`: `--settings`, `--mcp-config`, `--add-dir` and `--plugin-dir` with absolute paths, `--permission-mode` from a closed set, `--fallback-model` with a parsed `ModelName`. `--bg`, a second `--resume`, a bare `--` and `bypassPermissions` cannot be expressed; adding the last one is a decision, not a missing case. A settings or MCP overlay that holds a literal secret refuses the launch (`SecretInOverlay`). `ClaudeCode::preseed_trust` takes the launch environment, refuses one that moves Claude's config, reads `~/.claude.json` and returns `AlreadyTrusted { key }` when the key Claude reads for project settings (the main checkout of the repository that holds the root, or the root) is already `true`, `Written { key }` after setting `projects["<key>"].hasTrustDialogAccepted` while no `claude` runs, `Inherited { by, reason }` when Fabric did not write but a trusted ancestor still skips the dialog, and `LeftToClaude { reason }` when nothing trusts the root and Fabric did not write, such as `ClaudeRunning { pids }` while a `claude` runs; the CLI calls it before launch and is the one place that shows the last two as warnings, because in both Claude drops the workspace's project settings (ADR 0006, Trust on the target, amended 2026-10-20).
- Codex: `codex resume <id>` in the terminal, or `codex exec --json` for a new thread and `codex exec resume <id> --json` for the job's headless, with `-c projects."<root>".trust_level=trusted` and a typed `CodexOverrides` (sandbox `read-only` or `workspace-write`, approval policy, approvals reviewer, model, reasoning effort) on every run, and the prompt after `--`. `danger-full-access` cannot be expressed. The app-server is not used.
- Settings on a resume, added 2026-10-21 (gate finding F10). Every field of `CodexOverrides` is optional. On a resume the Codex adapter fills each field it was not configured with from the last `turn_context` of the lineage, and the Claude adapter passes `--model` with the last assistant model of the transcripts. Both first run the binary they would launch with `--version`, in the workspace the launch starts it in, and refuse, before anything starts, when it is older than the newest version the session records (`HarnessOlderThanSession`) or does not print one that parses (`HarnessVersionUnknown`). For Codex that is the higher of any `cli_version` and the known writer the job carries (`Codex::with_known_writer`, amended 2026-10-21 after review), because a Codex resume records no version. One type, `HarnessVersion`, parses and orders versions by semver precedence for this guard and for the doctor row. `Running::settings` returns a `LaunchSettings`: each setting passed, with whether it came from the session or was configured, and each recorded setting left out, with why, which the CLI prints. ADR 0006, Settings travel with the session, records the decision.
- Headless output, added 2026-10-21. `ClaudeCode::read_headless_line` and `Codex::read_headless_line` read a headless run's standard output line by line into a `TurnReport { session, answer, failure }` and say when a line named a session the report did not hold. Claude prints one result object, `{"type":"result", "is_error", "result", "session_id", ...}`, and a failure inside the run as that result too (code.claude.com/docs/en/headless, read 2026-10-21). Codex prints the events of `codex-rs/exec/src/exec_events.rs`, the same from 0.142.0 to 0.163.0: `thread.started { thread_id }` first, for `exec resume` too, with the resumed id; `item.completed` items, the last `agent_message` being the answer; `turn.completed` or `turn.failed { error.message }`. An `error` event can precede a retry, so it ends nothing. The same source reading found what the launch relies on: the flags after `exec resume` are global, so `--json` and `-c` are accepted there; with a prompt argument and standard input on `/dev/null`, `codex exec` reads the empty input and goes on; exec rollouts use the interactive layout under `sessions/YYYY/MM/DD/`, and `exec resume` appends to the same file; exec forces approvals off, and a trusted path makes its default sandbox workspace-write.
- Every adapter: the child gets the caller's `env` minus the parent-session markers of `markers.rs` (ADR 0006, amended 2026-10-20 for gate finding F2), so a harness launched from inside a Claude Code or Codex session is a session of its own.
- ACP: a minimal typed client over stdio JSON-RPC (`initialize`, `session/new`, `session/load`, `session/resume`, `session/prompt`, `session/cancel`, `session/update`), field names from `schema/v1/schema.json` at v1.26.0. A session continues with `session/resume` when advertised, else `session/load`, else the launch is refused (`LoadNotSupported`). `session/request_permission` is answered by a `PermissionPolicy`, deny by default, and `cancelled` for a session cancelled since its last prompt. One writer task sends every message as a whole line, so a dropped request never leaves half a line on the agent's stdin; a line from the agent over 16 MiB closes the connection, and so do 1024 lines waiting for an agent that stopped reading (`Saturated`); a failed write fails every waiting request; the handshake has a 45 second deadline. The official `agent-client-protocol` crate (3.4.0, Apache-2.0, released 2026-10-16) is maintained but was not used: it went from 2.2.0 to 3.4.0 in four weeks, brings its own executor abstraction and a dozen crates, and the methods needed here have a fixed wire shape. At most 1024 updates wait unread; a full queue drops the newest and counts it, because a reader that waited would never deliver the response a caller is waiting for. A launch whose handshake fails or is dropped kills the agent's process group. Capture and restore take, from the declared state directory, only files with a path component that is the session id or the id plus an extension, so session `s1` never carries `s10.json`. The state directory may not be the home or lie under `~/.fabric` (`StateDirHoldsFabric`), a symlink at it or above it refuses the capture, and launch arguments that hold a secret shape refuse it too (`SecretInArguments`), because the arguments travel in the manifest unredacted.

### Verified, and what still needs a live run

Verified on the Pro on 2026-10-20: `claude --version` prints `2.1.294 (Claude Code)` and `codex --version` prints `codex-cli 0.155.1`. The flags were read from `claude --help`, `codex --help`, `codex resume --help` and `codex exec resume --help` of those versions. The project key rule, the session registry and the `file-history-snapshot` record shape were read on the owner's machine, names and key structure only. Every test runs against fixtures and fake binaries.

Still needs a live run on the two-Mac gate: a real `claude --resume` of a restored transcript with one tool turn (no 400), a real `codex exec resume` of a restored lineage, whether Codex's scan accepts `sessions/fabric/<thread_id>/` with the SQLite row absent, the trust pre-seed on the Air, and the tombstones (`continued-in`, `thread/archive`), which this crate does not write yet.

## Alternatives considered

1. **Keep the trait in `fabric-core` with an opaque process handle.** The handle would be a core type with no core user until the supervisor exists. Rejected for now; the trait moves when `RunningHarness` lands.
2. **A restore rule without a departure record**, where the incoming copy always wins for rewritable files. A file edited on this node after the job left would be overwritten without a word. Rejected.
3. **Carry the previous manifest in the job record** so restore knows what the departing node sent. It changes the trait and the core's view of opaque state for a fact only the departing node needs. Rejected in favour of the local record.
4. **Fail open when the Claude registry cannot answer.** It keeps restores working on the Air's older Claude, at the price of a silent lost append. Rejected: a refusal that names the running process is recoverable, a lost append is not.

## Consequences

- The core does not depend on `tokio` or on any adapter.
- A restore onto a node where any Claude older than 2.1.294 runs is refused until that Claude exits.
- Rewritable files come back only to the node that left them untouched. A node that edited one after the job left gets `StateDiverged` and the orphan export path of ADR 0003.
- `~/.fabric/harness/departed/` holds up to two small files per captured session, the active record and a pending one; nothing removes them yet.
- The core must call `confirm_departure` after every publish of a capture, or rewritable files never come back to the node that left them.

## Open questions

- Where do the account email and organization UUID sit in a real transcript? Read it on the gate, then redact them.
- Does Claude 2.1.300 or later keep the registry format read on 2.1.294? Check on each Claude upgrade before raising the floor.
- When should a departure record be removed? Likely when the ledger records the job as live elsewhere for good, which needs ADR 0003's supervisor.

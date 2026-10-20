# ADR 0013: Harness trait as built

Status: accepted, 2026-10-20

Supersedes the section "The `Harness` trait" of ADR 0006 and the section "What the first implementation shipped" that was appended to it. The rest of ADR 0006 stands: which harnesses get adapters, the credential rules, opaque state, no path rewrite, and cross-harness resume as an experiment.

Written after the first implementation (`crates/fabric-harness`, branch `feat/harness`, PR #6) and two review rounds. ADR 0009 says an ADR is superseded, not edited, when a decision changes. Moving the trait out of the core and changing its signatures are decisions, so they are recorded here.

## Context

ADR 0006 put the trait in `fabric-core` with `stop`, a session-level `detect(workspace)`, `capture(workspace, out) -> Captured` and `restore(state, from_root, to_root)` with a rewrite. Building it showed four things.

- `launch` returns a handle that owns a child process, which is a `tokio` type. The core is pure: no IO, no clock, no process.
- `stop`, the session-level `detect` and the `Daemon` case need the supervisor of ADR 0003, which does not exist yet.
- ADR 0011 fixed the root to the same absolute path on every node, so a restore never rewrites a path and never needs `from_root` and `to_root`.
- The review found that "equal or byte prefix", the restore rule of ADR 0006, only fits files that grow. Claude rewrites memory, task files and sidecar metadata in place.

## Decision

### The trait

`fabric_harness::Harness` lives in `fabric-harness` and has five methods.

```text
trait Harness: Send + Sync {
    fn kind(&self) -> HarnessKind;
    fn detect(&self) -> BoxFuture<Result<Option<Installed>, HarnessError>>;
        Is the binary installed, at which path, with which version.
    fn capture(&self, workspace, session: &SessionId, out) -> BoxFuture<Result<StateManifest, HarnessError>>;
        Copy what `session` needs into `out`, redacted, and write `out/manifest.json` last.
    fn restore(&self, workspace, state) -> BoxFuture<Result<Restored, HarnessError>>;
        Place a captured tree on this node, all or nothing.
    fn launch(&self, workspace, Launch, Io, env) -> BoxFuture<Result<Running, HarnessError>>;
        Start the harness in a new process group with exactly `env`.
}
```

The session is an explicit argument, so an adapter never guesses which session a job continues. `ClaudeCode::latest_session` and `Codex::latest_session` exist for `fabric job init` to adopt a session the user started by hand. When ADR 0003's `RunningHarness` lands, the trait can move down into the core and return that handle; the method list does not change for that. `stop`, the session-level `detect` and the `Daemon` case come with the supervisor.

`capture` and `restore` copy files on a blocking thread. Dropping their future does not stop the copy, so the caller awaits them to completion and does not `launch` the same session until they return. Each adapter takes the `PATH` value to search in its constructor; the library never reads its own process environment.

`Launch` carries the first prompt: `New { prompt }` or `Resume { session, prompt: Option<String> }`. A headless run (`Io::Piped`) needs the prompt, and each adapter refuses one without it (`HeadlessNeedsPrompt`) before it starts a process.

### The manifest and what restore may do

`capture` writes the tree under one prefix, `home/`, and then `manifest.json`. A `StateManifest` holds the harness kind, the session, the physical root, the home, the harness version, the redaction counts, and every file with its content kind, its length, its BLAKE3 digest and its mutability. Its fields are private. A manifest comes from `StateManifest::new` or from a file, and both check the same rules: format 1, every path under `home/`, no temporary file, strictly sorted. Staging removes an earlier `manifest.json` from `out` and fsyncs the directory before it writes anything, so a capture killed in a reused directory never leaves an old manifest naming files it has partly replaced.

A restore accepts only the root and home of the capture (`RootMismatch`, `HomeMismatch`, ADR 0011), only the destinations its adapter captures from (`UnexpectedFile`), and only a manifest whose mutability for a path matches the adapter's own rule. It checks every staged digest, then plans every destination, and writes only when all of them pass. A destination passes when:

- it is missing, equal to the incoming bytes, or a byte prefix of them;
- it is this node's own copy, and the incoming bytes extend what capture's redaction makes of it; the local bytes stay, secrets and signed blocks included, and only the incoming suffix is added;
- it is `Replaceable` and still holds the bytes this node recorded when the job last left it.

Anything else is `StateDiverged { path }`, and nothing is written.

Mutability is part of the data. `AppendOnly` covers transcripts and rollouts, which only grow. `Immutable` covers tool results, file-history backups and uploads, which are written once. `Replaceable` covers everything Claude rewrites in place (memory, task files, sidecar metadata) and all ACP state, which Fabric cannot see into. At capture the adapter records the digest of each `Replaceable` file on this node, under `~/.fabric/harness/departed/<harness>/<session>.json`, before it writes the manifest. That record never travels. A job that went from the Pro to the Air and comes back replaces the Pro's `MEMORY.md` with the Air's only if the Pro's file was not edited after the job left.

Every write goes through a temporary sibling, an fsync and a rename, keeps the mode of the file it replaces (0600 for a new one), and fsyncs the directory. Crash tests abort capture after each staged file and restore after each placed file, at named steps (the test-only `crashpoints` feature), and check that no manifest a fresh node would accept is left, that every placed file is whole and that the next run converges. A process killed between a rename and the directory fsync loses nothing, because the page cache outlives it, so that window needs a power cut to test, not a kill.

### Running harnesses

Capture and restore refuse while the harness may be writing the session (`HarnessRunning`, `ActiveWriter`).

- Claude: the session registry `~/.claude/sessions/<pid>.json` answers only when it exists, every record in it parses, and the installed Claude is at least 2.1.294, the version it was read on. Otherwise any running `claude` process refuses the capture or restore. The Air runs 2.1.199 today, so there it means closing every Claude session first. A `ps` answer other than found or not found is an error, never "dead".
- Codex: Codex's own thread writer lock, held for the whole copy, taken as `codex-rs/rollout/src/writer_lock.rs` takes it.
- ACP: the adapter cannot see whether a generic agent writes a session. The caller owns that guard, through the handle `launch_acp` returns.

### Redaction as built

At capture every Claude file is redacted; Codex and ACP state travels byte for byte and is refused when a secret shape is found in it (`SecretInState { path, line }`).

- `.jsonl` and `.json` files go through the JSON scanner: every string outside a signed block is decoded, redacted and re-encoded. A signed block is a Claude content block whose signature the API checks: `type` `thinking` with a `signature`, or `type` `redacted_thinking`. Any other object with a `signature` key is ordinary JSON. A signed block that holds a secret is dropped whole when it is an array element and counted as `signed-block-dropped`; a block nested in a dropped one goes with it; otherwise the capture fails (`SecretInSignedBlock`).
- Every other Claude file is redacted as text, line by line. A line that is not UTF-8 is redacted as bytes and otherwise kept. A PEM private key that spans lines is redacted as one block: the BEGIN line from its header on, every line up to the END line, and to the end of the file when no END line comes.
- Rules: Anthropic, OpenAI, GitHub, AWS, Slack, Google and Stripe key shapes (Stripe webhook secrets included), JWTs, PEM private key blocks, `Authorization` headers, bearer tokens whose value looks like a token, credentials in URLs (`postgres://user:password@host`), and the value of an upper-case variable whose name says secret, token, password, API key, private key, access key, credentials, `DATABASE_URL` or `DSN`. Every key rule starts on an ASCII word boundary, so `sk-` inside `disk-usage-analyzer-service` is not a key. A rule that does not compile fails every capture.
- `file-history/<sid>/` holds Claude's verbatim backups of edited files, named by a hash of the path. The transcripts' `file-history-snapshot` records map each backup to the path it tracks. A backup whose original is on the denylist stays behind, and so does a backup no record names; both are counted.
- Not done: the account email and the organization UUID in `session_context` and `credential_org` attachments, which ADR 0006 lists. No transcript read so far showed where those fields sit, and a guessed field path would redact the wrong thing or nothing. They land with the first live capture on the gate, when the shape can be read from a real transcript. Until then the account email can travel inside the job's own state, between the user's own two machines.

### The denylist

`denylist::check` refuses credential names (`.credentials.json`, `auth.json`, `.claude.json`, `.netrc`, `id_*`, `.env*` except examples), key and database suffixes, credential directories, and the host-state directories and files of both harnesses. It runs inside staging before anything is copied, again on every manifest entry at restore, and on the original path of every file-history backup.

### Launch

- Claude: `claude --resume <sid> <flags>` in the terminal, or `claude -p --output-format json --resume <sid> <flags> -- <prompt>` headless. `Launch::New` passes `--session-id <uuid4>`, so the id is known before the process starts. The flags are a typed `LaunchFlags`: `--settings`, `--mcp-config`, `--add-dir` and `--plugin-dir` with absolute paths, `--permission-mode` from a closed set, `--fallback-model` with a parsed `ModelName`. `--bg`, a second `--resume`, a bare `--` and `bypassPermissions` cannot be expressed; adding the last one is a decision, not a missing case. A settings or MCP overlay that holds a literal secret refuses the launch (`SecretInOverlay`). `ClaudeCode::preseed_trust` sets `projects["<root>"].hasTrustDialogAccepted` while no `claude` runs; the CLI calls it before launch.
- Codex: `codex resume <id>` in the terminal, or `codex exec resume <id> --json` headless, with `-c projects."<root>".trust_level=trusted` and a typed `CodexOverrides` (sandbox `read-only` or `workspace-write`, approval policy, model, reasoning effort) on every run, and the prompt after `--`. `danger-full-access` cannot be expressed. The app-server is not used.
- ACP: a minimal typed client over stdio JSON-RPC (`initialize`, `session/new`, `session/load`, `session/resume`, `session/prompt`, `session/cancel`, `session/update`), field names from `schema/v1/schema.json` at v1.26.0. `session/request_permission` is answered by a `PermissionPolicy`, deny by default. At most 1024 updates wait unread; a full queue drops the newest and counts it, because a reader that waited would never deliver the response a caller is waiting for. A launch whose handshake fails or is dropped kills the agent's process group. Capture and restore take, from the declared state directory, only files with a path component that is the session id or the id plus an extension, so session `s1` never carries `s10.json`.

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
- `~/.fabric/harness/departed/` holds one small file per captured session; nothing removes them yet.

## Open questions

- Where do the account email and organization UUID sit in a real transcript? Read it on the gate, then redact them.
- Does Claude 2.1.300 or later keep the registry format read on 2.1.294? Check on each Claude upgrade before raising the floor.
- When should a departure record be removed? Likely when the ledger records the job as live elsewhere for good, which needs ADR 0003's supervisor.

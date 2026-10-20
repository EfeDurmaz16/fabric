# ADR 0006: Harness adapters

Status: accepted, 2026-10-19

Amended 2026-10-20 after the first implementation (`crates/fabric-harness`, branch `feat/harness`) and its review: the section "What the first implementation shipped" records the trait as built, where it departs from the text above, what was verified against the installed binaries and what still needs a live run.

Amended 2026-10-19 after step 1 research. The section on the two first adapters now records the exact surfaces the research settled on (`2026-10-19-step1-design-input.md`, sections 1.1, 1.2, 5.3, 5.4 and decisions D13 to D17, D20, D21, D26 to D28; `step1/H1-claude-code.md`, sections 1, 6, 10 and 11; `step1/H2-codex.md`, sections 7, 9, 10 and 13; `step1/H6-environment-and-triggers.md`, section 2.4): Claude through its CLI, Codex through `codex exec resume` first and the app-server only behind a capability probe, no per-job config directories, overlays passed as flags, trust pre-seeded on the target, the harness's own tombstone written during capture, and the credential and licence rules. The trait did not change. References to the root rule now point at ADR 0011.

## Context

The agent that does the work changes weekly. Claude Code keys resume to a working directory (claude-code #5768, #28745, cited in `2026-10-19-t3code-vs-fabric.md`, section 2). Codex exposes `thread/resume` through its app-server JSON-RPC. T3 Code, when it hands off, starts a fresh provider session fed with text and loses tool state and pending approvals (same note, section 1). The Agent Client Protocol (ACP) now has adapters for both: `zed-industries/claude-agent-acp` and `agentclientprotocol/codex-acp` (agentclientprotocol.com, agents list, checked 2026-10-19), with `session/new`, `session/load` gated by the `loadSession` capability, `session/prompt` and `session/cancel`.

Fabric's core must not know any of this. If it does, every vendor release is a core release.

## Decision

### The `Harness` trait

Defined in `fabric-core`, implemented per harness in `fabric-harness`.

```text
trait Harness: Send + Sync {
    fn kind(&self) -> HarnessKind;

    fn detect(&self, workspace: &Path) -> BoxFuture<'_, Result<Option<Detected>, HarnessError>>;
        Is there a session of this harness for this workspace? Where is its state?
        Is it running right now, and as what kind of process?

    fn stop(&self, running: &RunningHarness, deadline: Duration) -> BoxFuture<'_, Result<(), HarnessError>>;
        Ask the harness to finish its current turn and exit within the deadline.
        Spawned and External: the harness's graceful signal or command, then wait on the pid.
        Daemon: the harness's own stop command. StopTimedOut when the deadline passes.

    fn capture(&self, workspace: &Path, out: &Path) -> BoxFuture<'_, Result<Captured, HarnessError>>;
        Copy everything a stopped harness needs to continue into `out`, redacted.
        Returns what was copied, any paths that are absolute, and what was redacted.
        The core calls it only after `stop` returned or `detect` reported nothing running.

    fn restore(&self, state: &Path, from_root: &Path, to_root: &Path) -> BoxFuture<'_, Result<(), HarnessError>>;
        Place the captured state where this harness expects it on this node,
        rewriting paths from `from_root` to `to_root` when they differ.
        Refuses with HarnessStateDiverged when state already on disk would be lost (below).

    fn launch(&self, workspace: &Path, resume: ResumeHint, env: &[(String, String)]) -> BoxFuture<'_, Result<RunningHarness, HarnessError>>;
        Start the harness so that it continues the same session, in a new process group,
        with exactly this environment, and return the handle the supervisor records.
}

HarnessKind     = ClaudeCode | Codex | Acp { program: ProgramName, args: [String] }   the program is a bare name (no slash, no NUL, no leading `-`, not empty) looked up on PATH, never run through a shell (review amendments, 2026-10-19)
Detected        = { state_root: PathBuf, session_id: Option<String>, running: Option<RunningHarness> }
Captured        = { entries: [Entry], absolute_roots: [PathBuf], redactions: [RedactionReport] }
RedactionReport = { rule: String, count: u32 }
```

`RunningHarness` is the core-owned handle defined in ADR 0003 (`Spawned`, `External` or `Daemon`); no tokio or std process type crosses the trait. Killing is not an adapter method: for `Spawned` and `External` the supervisor sends `killpg` itself, so fencing never depends on adapter code, and for `Daemon` it repeats `stop` with a zero deadline until `detect` reports nothing running. `stop` and `capture` are separate because they fail differently: a harness that will not stop in time is a reason to abort suspend and leave the job live, while a copy that fails is a reason to retry the copy.

The harness is chosen at runtime from the ledger (`--harness codex` on a job created under Claude Code), so the core holds a `Box<dyn Harness>`. The methods return `BoxFuture`, defined in `fabric-core` (ADR 0005), because native `async fn` in a trait is not dyn compatible on the pinned toolchain and its futures cannot be required to be `Send`. The core calls them in this order: suspend is `detect`, `stop`, `capture`; resume is `restore`, `launch`; fencing a `Daemon` is `stop`. It stores whatever `capture` produced as the `harness_state` tree of the snapshot (ADR 0004) and never opens it. The process that calls `launch` is the `fabric resume` CLI, which passes its own environment filtered to the names the `EnvSpec` lists plus the variables the harness needs to find its login and its terminal (`HOME`, `PATH`, `TERM`, `SHELL`, `LANG`), so a daemon's environment never leaks into a harness and a secret outside the spec never does either.

### ACP is the generic path

Any agent that speaks ACP and reports `loadSession: true` gets the `Acp` adapter for free:

- `stop`: send `session/cancel` if a prompt is in flight, then end the agent process.
- `capture`: record the ACP `sessionId` and copy the agent's state directory as declared in the Fabric job config for that agent.
- `launch`: start the agent command, `initialize`, `session/load` with the recorded id and the new `cwd`, and return `Spawned` with its pid, process group and start time.

Agents whose state directory is unknown to Fabric cannot be captured and the adapter says so at `detect` time.

### Claude Code and Codex adapters first

These two get dedicated adapters because the first gate is Claude Code and the second harness is Codex, and because both have known native resume paths that ACP wraps. Amended 2026-10-19: the surfaces below are the research's decisions, not options.

**Claude Code, through the CLI.**

- State lives under `~/.claude/projects/<slug>/`, where the slug is the workspace path with non-alphanumerics replaced by `-`, plus sidecars that one session can spread across several slugs (`<sid>/subagents/`, `tool-results/`, `file-history/<sid>/`, `tasks/<sid prefix>/`, `plans/`, `uploads/<sid>/`). `capture` enumerates `projects/*/<sid>*` and the sidecars by session id, never one slug. `restore` places them under the same keys because the root is identical on every node (ADR 0011).
- `launch` runs a foreground `claude --resume <session-id>` in the user's terminal, from the workspace root, and returns `Spawned`. It never uses `claude --bg`, whose own supervisor restarts a killed session and would defeat the fencing kill (ADR 0003), and it never sets `CLAUDE_CODE_RESUME_INTERRUPTED_TURN`, so a turn cut by a kill shows Claude the cut-off marker instead of re-running the tool. Headless checks, such as the adapter's own integration test asking the resumed session for its working directory, use `claude -p --resume <session-id>`; `-p` sessions are hidden from the picker and `--continue`, which is why the user-facing launch is interactive.
- Resume restores the conversation, model, agent and permission mode and nothing passed as a flag. The job record therefore holds the launch flags and `launch` passes them on every start: `--settings <job overlay file>`, `--mcp-config <job file>`, `--add-dir`, `--plugin-dir`, `--permission-mode`, `--fallback-model`, with `autoMemoryDirectory` and `plansDirectory` pinned in the overlay. Secrets in the overlay are references (`${VAR}`), never values, and the adapter refuses to write an overlay that holds a literal token.
- `stop` on a foreground session is the graceful exit the research names (H1, section 11.3). `detect` reports a session under the background supervisor (`claude agents --json`) as `Daemon`; `stop` for it is `claude stop <id>`, never a signal, because the supervisor restarts a process that "exited unexpectedly" and `claude respawn --all` restarts every supervised session after an update (design input, A34, D27). A `--bg` session is captured only after `stop` returned.
- Native tombstone: before `MoveStarted` is written (ADR 0003), `capture` appends a `continued-in` record `{ type, sessionId, continuedInSessionId, timestamp }` to the departed transcript, which is the record the CLI itself writes when a conversation continues elsewhere. Whether `claude --resume` and the picker honour it is an open experiment (design input, A32); the record is written regardless because it costs one line and the abort path removes it by truncating to the recorded byte length.
- Reuse of claude-handoff 0.3.0's exporter and redaction rules will be evaluated rather than rewritten (landscape section 8.1).

**Codex, through `codex exec resume` first.**

- State is the rollout lineage under `$CODEX_HOME/sessions/**/rollout-<ts>-<thread_id>[_<rollout_id>].jsonl[.zst]`, including every `history_base` predecessor of a paginated thread, plus parent and child threads of a multi-agent run. `capture` copies the lineage byte for byte; `restore` places it under `sessions/fabric/<thread_id>/` on the target, which the lookup accepts (H2, experiments E2 and E5). The SQLite files, `thread-writer-locks/`, `shell_snapshots/`, `environments.toml` and `auth.json` are never captured.
- `launch` runs the stable CLI: `codex exec resume <thread-id>` from the workspace root, with the sandbox, approval policy, model and effort from the job record passed as `-c` overrides on every run, because a cold resume on 0.155.1 downgraded `workspace-write` to `read-only` when they were not passed (H2, experiment E3). The app-server JSON-RPC surface is used only for interactive attach and only behind a per-version capability probe: `initialize`, then a check that every method the adapter needs is present and not marked experimental for this binary. `thread/resume { threadId, cwd, sandbox, config }` is stable in that base form; `path`, `history`, `historyMode`, `runtimeWorkspaceRoots`, `permissions` and `environments` are experimental and the adapter does not depend on them. The reason is churn: 79 commits to `app-server-protocol` in the 30 days to 2026-10-19, `thread/rollback` removed in 0.156.0, and "experimental, not supported for production workloads" in OpenAI's own docs (H2, section 10).
- `stop` and `capture` hold Codex's own writer lock for the whole copy: take `.coordination.lock`, then `try_lock` on `thread-writer-locks/<thread_id>.lock`, the pattern Codex uses for publication. Checking that the lock is free and then copying is a race the ChatGPT desktop app, automations and spawned child threads can lose for Fabric (design input, A8).
- Native tombstone: `capture` calls `thread/archive`, which moves the rollout to `archived_sessions/`, refuses while a live writer holds the lock, and removes the thread from `codex exec resume` and the picker; the abort path calls `thread/unarchive`. `capture` also lists every `automations/*/automation.toml` whose `target_thread_id` is the job's thread and refuses until they are disabled, because a scheduled automation is a writer on the departed machine (design input, section 4.8.1).

**Rules shared by both adapters.**

- No per-job config directory. `CLAUDE_CONFIG_DIR` and `CODEX_HOME` stay at the user's defaults on every node. Both harnesses key the login to the config directory (Codex derives its keyring entry from the `CODEX_HOME` path; Claude keeps `.credentials.json` or the Keychain item per config directory), so a per-job directory is a per-job login, which breaks the credential rule below (design input, D15; H6, section 2.4, item 6). Per-job settings are overlays passed as flags, never writes into `~/.claude.json` or `~/.codex/config.toml` while a harness may be running.
- Trust on the target. Claude blocks an interactive session on a new path with a trust dialog, and `--bg` exits with "Workspace not trusted". Before `launch`, the Claude adapter sets `projects["<root>"].hasTrustDialogAccepted = true` in `~/.claude.json` by read, modify, temp, rename, and only while no `claude` process runs on the node; if one runs, it refuses with `ClaudeRunning` and names the process, because Claude rewrites that file itself. Codex trust is an exact key on the path, so the adapter passes `-c projects."<root>".trust_level=trusted` on every run instead of editing `config.toml`.
- Versions are recorded in the job record on every suspend (`claude --version`, `codex --version`) and compared by `fabric doctor` (ADR 0008). Claude 2.1.300 fixed a lost first transcript entry after a crash-cut resume, which is the kill -9 gate path, so the floor for the gate is 2.1.300 on both nodes; neither Mac meets it today (Pro 2.1.294, Air 2.1.199; `step1/preflight-2026-10-19.md`), and auto-update is turned off for the duration of a measured run (`DISABLE_AUTOUPDATER=1`, pinned Homebrew Codex).
- Secondary harnesses come later and in this order: OpenCode (export and import JSON, the cheapest carry), then Pi (one JSONL under a directory Fabric may own). Copilot, Cursor and Amp wait for a read of their terms (design input, D17).

Each adapter is a module in `fabric-harness` behind a cargo feature. The core compiles with no adapter enabled.

### Credentials and terms

Added 2026-10-19.

- Fabric never carries, copies or proxies a credential: not `.credentials.json`, not a Keychain item, not `auth.json`, not an OAuth token in an MCP configuration. Each node signs in once, at its own console, through the vendor's own flow; a login over SSH may fail to reach the Keychain and fall back to a plaintext file, so the first login on the Air is done at the Air (design input, section 5.4). `fabric doctor` reports login presence as yes or no and never the account.
- That rule is Fabric policy, stricter than one vendor requires. Anthropic forbids developers from collecting, storing or intermediating Claude.ai credentials or session tokens; OpenAI's own documentation describes copying `auth.json` to a headless machine. Fabric keeps the strict rule on both because two copies of one login on two machines is the split-brain the lease exists to prevent.
- Running the unmodified vendor CLI under the user's own login on the user's own machines is the use both vendors allow: Anthropic's compliance page covers "ordinary use of Claude Code" and signing in to the unmodified binary with one's own subscription, and documents `-p` and `setup-token` for scripts; OpenAI allows local and open-source use of the app-server and has never allowed it for commercial or hosted services. An adapter that runs a harness for other people is not covered by either reading and is out of scope (design input, D20; H1, section 10; H2, section 9). Fabric does not modify either binary, does not remove an auth method, and does not use either product's name in its own.
- Code the adapters may draw on: claude-handoff, session-roam and session-migrate are MIT; openai/codex is Apache-2.0; `coder/agentapi` is MIT. cmux is GPL and is studied, never copied (design input, D21).

### Harness state is opaque to the core

The core sees `harness_state` as a list of relative paths and chunk ids. The only thing the core does with it is hash, store, transfer and hand it back to the adapter. The core never redacts, never parses JSONL, never rewrites paths. Redaction is therefore part of the `Harness` contract, not an option: `capture` returns a `RedactionReport` per rule applied, and the suspend output prints the counts next to the excluded workspace paths (ADR 0004).

For the Claude Code adapter the contract is concrete, because the research shows what the state carries (H1, sections on the config directory and on identity):

- Never included: `.credentials.json`, anything derived from the Keychain, `~/.claude.json` as a whole (it holds `oauthAccount`, `machineID`, `remoteControlMachineId`, `userID`), `sessions/`, `session-env/`, `shell-snapshots/`, `jobs/`, `daemon/`, `backups/`, `cache/`, `telemetry/`, `stats-cache.json`. Each node signs in once through Anthropic's own flow; carrying credentials would also breach the terms the research cites.
- Redacted in the transcript and sidecars, following claude-handoff 0.3.0's rules: secret patterns, `.env` values that appear in tool results, the account email and the organization UUID in `session_context` and `credential_org` attachments.
- A secret inside a signed `thinking` block cannot be edited without breaking the block (next section). The adapter then drops that whole block, which the API accepts, and counts it in the report.

### State outside the root is protected like the root

The workspace marker (ADR 0004) keeps a restore from writing over unpublished work in the root. Harness state lives outside the root, so the adapter must give the same guarantee itself. Before `restore` overwrites a file it owns on this node, it compares the bytes on disk with the incoming bytes: equal, or a byte prefix of the incoming file, is allowed, because nothing on disk is lost; anything else is `HarnessStateDiverged { path }` and nothing is written. This is the rule cmux uses for Claude transcripts (research H1, section 9) and it fits because transcripts only grow. The case it catches is the old holder of a fenced job: its transcript continued past the fork while the job's transcript continued elsewhere. The way out is `fabric orphan export`, which captures the divergent state into an orphan snapshot (ADR 0003); after that the files on disk may be replaced.

### Path rewrites: none by default, never inside a signature

`restore(state, from_root, to_root)` receives `SnapshotManifest.root` as `from_root` (ADR 0004); `Captured.absolute_roots` is what the adapter reports at capture and the core checks it against that field. With `from_root == to_root` it performs no rewrite at all, and in v0 that is the only case the Claude Code adapter accepts (ADR 0011). The reason is a fact about the state, not caution: Claude Code thinking blocks carry an encrypted `signature`, and a modified block is rejected with a 400 on the next tool turn (research H1, item 2). A blanket rewrite of the transcript breaks the resumed session at its first tool call, which is exactly the first gate.

When roots differ, which no v0 path allows for Claude Code, the adapter rewrites only the project directory key, `cwd` fields and sidecar references, following claude-handoff's placeholder rules at path-component boundaries, and never a `thinking` block or any field under a signature. Since 2.1.300 the transcript also records the system prompt of the first request, environment block included, and reuses it until compaction; a rewrite under different roots must either rewrite that record or pass `--system-prompt-snapshot off` on the first resume (design input, A2). Under the same root the stale fields are host-level only. The property test for this is stated now so that it exists before the code: for arbitrary roots, a rewritten transcript leaves every signed block byte-identical.

### Cross-harness resume is an experiment, not a promise

`fabric resume <job> --harness codex` on a job captured under Claude Code cannot load Claude's native state into Codex. What it can do is produce a neutral brief (goal, recent decisions from the ledger, `git log` and `git status` of the restored workspace, the last user turn if the source adapter exposes it) and start the new harness with that brief as its first prompt. This is the same text-handoff shape T3 Code uses and it loses tool state and approvals. It is listed as a later experiment. Fabric does not claim it works until a measured test says how much is lost.

### What the first implementation shipped (2026-10-20)

`crates/fabric-harness` is the first code behind this ADR. It follows the decisions above and departs from the text in the places listed here, so that a reader of the code and a reader of the ADR see the same thing.

**The trait as built.** `fabric_harness::Harness` has five methods: `kind`, `detect` (is the binary installed, at which path and version), `capture(workspace, session, out)`, `restore(workspace, state)` and `launch(workspace, Launch, Io, env)`. It lives in `fabric-harness`, not in `fabric-core`, because `launch` returns a handle that owns the child process (a `tokio` type), and the core is pure. When the supervisor of ADR 0003 lands with the core-owned `RunningHarness`, the trait can move down and return that handle instead; nothing in the method list needs to change for that. `stop`, the session-level `detect` of the text above (is a session running, under which supervisor) and the `Daemon` case are not in this crate yet; they belong with the supervisor. The session a job continues is an explicit `SessionId` argument, so the adapter never guesses; `ClaudeCode::latest_session` and `Codex::latest_session` exist for `fabric job init` to adopt a session the user started by hand.

**`Launch` carries the first prompt.** `Launch::New { prompt }` and `Launch::Resume { session, prompt: Option<String> }`. A headless run (`Io::Piped`) needs the prompt: `claude -p --resume <id>` and `codex exec resume <id>` read one, and both adapters refuse a headless launch without it (`HeadlessNeedsPrompt`) instead of starting a process that waits on a closed stdin.

**Capture writes a tree and a manifest.** `capture` copies into `out/` under one prefix, `home/`, for files under the user's home, and writes `manifest.json` (`StateManifest`: kind, session, physical root, home, harness version, and every file with its content kind, length and BLAKE3 digest, plus redaction counts). Files inside the workspace, such as `.claude/` and `CLAUDE.local.md`, are the workspace snapshot's to carry (ADR 0004), never an adapter's. The core stores the tree as `harness_state` without reading it, as the text above requires; `manifest.json` is just one more file in that tree.

**No path rewrite; restore refuses another root or home.** As ADR 0011 rule 3 says, every adapter refuses a restore whose manifest root or home differs from the target's, with `RootMismatch { from, to }` or `HomeMismatch { from, to }`, before anything is written. The crate holds no rewrite code. The JSON scanner (`jsontext`) exists for redaction only. `tests/jsontext_props.rs` states the signed-block rule for it: a block with no secret stays byte-identical, a block with a secret is dropped whole however many of its neighbours are dropped too, and nothing else changes.

**Redaction as built.** At capture every Claude file is redacted. `.jsonl` and `.json` files pass through the JSON scanner with the standard rule set: Anthropic, OpenAI, GitHub, AWS, Slack, Google and Stripe key shapes, JWTs, PEM private key blocks and bearer tokens. Every other Claude file (`tool-results/*.txt`, `file-history/`, `uploads/`, memory) is redacted line by line as text. A line that is not UTF-8, such as one cut by `kill -9` inside a multi-byte character, is redacted as bytes and otherwise kept as it is. A secret inside a signed block drops the whole block when the block is an array element (a content block), counted as `signed-block-dropped`; otherwise the capture fails (`SecretInSignedBlock`). Codex rollouts and ACP state travel byte for byte, because a paginated Codex prefix is referenced by byte offset and ACP state is opaque. So they are scanned instead, compressed rollouts included, and the capture is refused (`SecretInState { path, line }`) when a secret shape is found. A symlink inside session state fails the capture (`SymlinkInState`); it is never followed.

**The denylist is hard-coded and checked twice.** `denylist::check` refuses credential names (`.credentials.json`, `auth.json`, `.claude.json`, `.netrc`, `id_*`, `.env*` except examples), key and database suffixes, credential directories, and the host-state directories and files of both harnesses listed in the text above. It runs inside `files::stage` before anything is copied, and again on every manifest entry at restore.

**Restore writes all or nothing, only where capture reads.** Each adapter accepts only the destinations it captures from: for Claude, `projects/<key>/<sid>.jsonl`, `projects/<key>/<sid>/`, the workspace key's `memory/`, `file-history/<sid>/`, `tasks/session-<first 8 of sid>/` and `uploads/<sid>/`; for Codex, a rollout file name under `sessions/` or `archived_sessions/`; for ACP, a path under the declared state directory that names the session. Any other path is `UnexpectedFile` before anything is written. `files::place_all` then checks every destination first and writes only when all pass: a file already on disk must be equal to the incoming bytes, a prefix of them, or this node's own copy whose redacted form the incoming bytes extend. In that last case the local bytes stay, secrets and signed blocks included, and only the incoming suffix is added. That is how a job that went Pro to Air comes back to the Pro without `StateDiverged`. Anything else is `StateDiverged { path }` and nothing is written. Files are written through a temporary sibling and a rename, keep the mode of the file they replace (0600 for a new one), and the directory is fsynced. Capture and restore run on a blocking thread and refuse while the harness writes the session: Claude through its session registry, Codex through its own thread writer lock.

**Claude Code adapter.** Captures `projects/*/<sid>.jsonl` and `projects/*/<sid>/` across every project key (sidecars of one session do spread across keys), the project memory under the workspace key, `file-history/<sid>/`, `tasks/session-<first 8 of sid>/` and `uploads/<sid>/`. The project key rule is `slug::slug`: every character outside ASCII letters and digits becomes `-`, on the physical path; a key over 200 characters is refused (`ProjectKeyTooLong`) because Claude's hash suffix is undocumented. Launch runs `claude --resume <sid> <flags>` in the terminal, or `claude -p --output-format json --resume <sid> <flags> -- <prompt>` headless; `Launch::New` passes `--session-id <uuid4>` so the id is known before the process starts. `ClaudeCode::preseed_trust` edits `projects["<root>"].hasTrustDialogAccepted` by read, modify, temp, rename, keeps the file's mode, refuses while any `claude` process runs (by name and by the resolved binary path, so an npm install running as `node` is seen; a `pgrep` failure is an error, not "none"), and never creates a missing `~/.claude.json`. The CLI calls it before launch; the adapter does not do it on its own.

**Codex adapter.** Finds the thread's rollouts by file name under `sessions/` and `archived_sessions/` (any depth, as Codex's own scan does), follows `session_meta.history_base.thread_id` (a rollout id) through plain and `.zst` files, and refuses with `LineageBroken` when a prefix is missing. Two files with one rollout id are accepted only when one is a byte prefix of the other, and the longer one is used; otherwise `LineageAmbiguous`. It holds `.coordination.lock` and then `thread-writer-locks/<thread_id>.lock` with `File::try_lock` for the whole capture and the whole restore, and refuses with `ActiveWriter` when another process holds it. Restore places each rollout over the file that already holds that rollout on this node, so a thread that comes back does not fork; a rollout this node has never seen goes under `sessions/fabric/<thread_id>/`. Launch runs `codex resume <id>` in the terminal or `codex exec resume <id> --json` headless, with `-c projects."<root>".trust_level=trusted` and the job's `-c` overrides on every run, and the prompt after `--` so a prompt that starts with `-` is never read as a flag. The app-server is not used.

**ACP adapter.** A minimal typed client over stdio JSON-RPC (`initialize`, `session/new`, `session/load`, `session/resume`, `session/prompt`, `session/cancel`, `session/update`), with field names taken from `schema/v1/schema.json` at schema v1.26.0. `session/request_permission` is answered by a `PermissionPolicy`, deny by default. One writer task sends every message as a whole line, so a dropped request never leaves half a line on the agent's stdin; a line from the agent over 16 MiB closes the connection. The handshake has a 45 second deadline, and a launch whose handshake fails or is dropped kills the agent's process group. The official `agent-client-protocol` crate (3.4.0, Apache-2.0, released 2026-10-16) is maintained but was not used: it went from 2.2.0 to 3.4.0 in four weeks, brings its own executor abstraction and a dozen crates, and the five methods needed here have a fixed wire shape. `AcpHarness` carries a declared state directory as opaque bytes and continues a session with `session/resume` when advertised, else `session/load`, else refuses (`LoadNotSupported`). An undeclared state directory is reported when capture or restore is asked for (`StateDirNotDeclared`), not at `detect`, because `detect` in this crate only answers whether the binary is installed; the session-level `detect` of the text above waits for the supervisor. The ACP adapter cannot see whether a generic agent is writing a session, so that guard stays with the caller, through the handle `launch_acp` returns.

**Verified against the installed binaries on the Pro, 2026-10-20.** `claude --version` prints `2.1.294 (Claude Code)` and `codex --version` prints `codex-cli 0.155.1`; `detect` parses both. The flags the adapters pass were read from `claude --help`, `codex --help`, `codex resume --help` and `codex exec resume --help` of those versions: `--resume`, `--session-id`, `-p`, `--output-format`, `--settings`, `--mcp-config`, `--permission-mode`; `resume [SESSION_ID] [PROMPT]`, `exec resume [SESSION_ID] [PROMPT]`, `--json`, `-c key=value`. The project key rule was checked against the owner's own store (`/Users/efebarandurmaz/Desktop/compass` is filed as `-Users-efebarandurmaz-Desktop-compass`; `tasks/session-<8 hex>`; `file-history/<uuid>`), reading names only. No interactive session was started and no real transcript was captured; every test runs against fixtures and fake binaries.

**Still needs a live run.** On the two-Mac gate: a real `claude --resume` of a restored transcript with one tool turn (no 400), a real `codex exec resume` of a restored lineage under the same account, whether Codex's scan accepts `sessions/fabric/<thread_id>/` on a real `CODEX_HOME` with the SQLite row absent, the trust pre-seed on the Air, and the tombstones (`continued-in`, `thread/archive`), which this crate does not write yet. The supervisor pieces (`stop`, `Daemon` detection through `claude agents --json`, the automation check for Codex) wait for ADR 0003's `RunningHarness`.

## Alternatives considered

1. **Drive agents through their SDKs** (Claude Agent SDK, Codex app-server as a hosted service). Both carry terms that restrict third-party hosting and login use (landscape section 8.1). Fabric runs the user's own CLI under the user's own login on the user's own machines, which is the path the research reads as allowed. SDK coupling is also the fastest way to a core release per vendor release. Rejected for the core; an adapter may use an SDK internally if its terms allow.
2. **`coder/agentapi` as the only path.** HTTP over several CLIs, MIT. It does not capture or restore state, which is the part Fabric needs. Could be a launch mechanism inside an adapter. Not the trait.
3. **A Fabric-defined session format that every agent must export to.** Nobody would implement it. Rejected. The opaque state directory plus per-adapter knowledge is the pragmatic version.
4. **Transcript-only moves** (claude-handoff, session-roam). Covers Claude only and omits the workspace. Used as a reference and possibly a library inside the Claude adapter.

## Consequences

- Adding a harness is one module and one feature flag. No core change.
- A vendor changing its state layout breaks one adapter's tests, not the system.
- `detect` returning `None` is a normal answer and the CLI explains it ("no Claude Code session found for this workspace").
- Adapter tests are integration tests against the real CLI when installed, skipped with a clear message when it is not. Property tests cover redaction over arbitrary JSON, including the byte-identical signed block property above; path rewriting has none because no rewrite exists in this phase (ADR 0011).

## Open questions

- Does `session/load` in `claude-agent-acp` restore the same context as native `claude --resume`? Test both on the gate.
- Do the native tombstones hold? After a scripted move, on the source run `claude --resume <sid>`, the Ctrl+A picker, `codex resume <id>` and `codex exec resume <id>`; pass is a refusal or a visible fork, fail is a silent resume (design input, A32, A33). If both fail, the remaining guard is moving the native files out of the harness store on depart, which makes abort harder, and ADR 0003's open question about moving the departed copy becomes a decision.
- Pending approvals: drain or cancel at capture, re-ask on launch. The research recommends not moving them (t3code note, section 2). Confirm with the first real suspend of a job mid-approval.
- Subagent transcripts and memory files outside the project directory: adapter decides what is part of "state". Start with what claude-handoff includes.

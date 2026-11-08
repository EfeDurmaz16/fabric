# Harness adapters

The contract an adapter in `crates/fabric-harness` meets, as the code has it today, and one section per adapter PR 5 adds: OpenCode, pi, Gemini CLI, Cursor Agent CLI, and GitHub Copilot CLI as a conditional last one. ADR 0006 holds the decisions behind the contract and ADR 0015 the trait as built; this page is the working reference for whoever writes or reviews an adapter. Versions and layouts below were read on 2026-11-08 from the binaries installed on the owner's Mac (OpenCode 1.18.34, pi 0.80.3, Cursor Agent CLI 2026.10.01, Claude Code 2.1.294, Codex 0.163.0), from `--help`, and from file, table, column and key names only; no transcript content was read, and every fixture in the tests is synthetic. Gemini CLI is not installed there, so its section comes from its source and documentation at v0.67.0 and its fake is built from the documented format alone.

The hard part first. Two of the four stores keep credentials beside the session: OpenCode's one SQLite file holds its login tokens next to every message, and Cursor's chat store is encrypted with the key in the same file. Fabric's invariant is that a carried file holds no file on the credential denylist and no secret-shaped string. So the OpenCode adapter carries rows the agent exports, never the file, and the Cursor adapter carries nothing of the chat until Cursor documents a format Fabric can scan. Everything else in this page follows from that invariant and from path identity (ADR 0011): a session is restored under the same workspace path and the same home, and nothing in it is rewritten.

## The contract

### The trait

`fabric_harness::Harness` (`crates/fabric-harness/src/harness.rs`) has seven methods. The core drives every adapter through them alone; no vendor type crosses the boundary.

```text
fn kind(&self) -> HarnessKind
    Which adapter this is: a variant of fabric_core::harness::HarnessKind.
fn fidelity(&self) -> Fidelity
    The highest fidelity this adapter reaches, declared once, true before any refusal.
fn detect(&self) -> Result<Option<Installed>>
    The binary on the adapter's PATH value and what it answers to --version; None when absent.
fn capture(&self, workspace, session: &SessionId, out) -> Result<StateManifest>
    Copy what `session` needs into `out`, redacted or refused, and write out/manifest.json last.
fn confirm_departure(&self, manifest) -> Result<()>
    The snapshot holding this capture is published; its departure record now counts.
fn restore(&self, workspace, state) -> Result<Restored>
    Place a captured tree on this node, all or nothing; say which session the harness continues.
fn launch(&self, workspace, Launch, env) -> Result<Running>
    Start the harness held, in a new process group, with `env` minus the parent-session markers.
```

Rules every adapter keeps:

- The session is an explicit argument. An adapter never guesses which session a job continues. A `latest_session(workspace)` helper may exist for `fabric job init` to adopt a session the user started by hand.
- The constructor takes the home as an `AbsPath` and the `PATH` value to search; the adapter never reads its own process environment except to probe a version, and that probe runs with the adapter's `PATH`. The workspace is parsed once into its physical `AbsPath`, which feeds the manifest and every path key the harness derives from the workspace.
- `capture` and `restore` run on a blocking thread through `pipeline.rs`, which owns the order of the steps and the checks; an adapter supplies its sources and, at restore, where each tree path lands and how it treats that path. Dropping their future does not stop the copy, so the caller awaits them and never launches the same session before they return.
- `launch` returns the harness held, stopped before its exec, until the caller records the pid and calls `Running::release` (ADR 0015, issue #17). A `Launch::Interactive` inherits the terminal; a `Launch::Headless` has no stdin and captured stdout and stderr, except for an agent that speaks a protocol on its stdio, whose pipes its client owns and whose turn arrives on `Running::take_turn`. A headless run cannot be built without a prompt.
- A resume carries the settings the session ran with, not the ones the node has configured (gate finding F10a): the adapter reads them from the session and passes them as typed flags, each one the adapter was configured with winning (`Origin::Pinned` over `Origin::Session`), and `Running::settings` lists what was passed (`PassedSetting`) and what was left out with why (`NotCarried`). A recorded value is cut to 64 characters with control characters replaced before anything prints it.
- Every launch runs with `env` minus the markers of a Claude Code or Codex session Fabric itself runs under (`markers.rs`), so a harness started from inside an agent's tool call is a session of its own.
- Capture and restore refuse while the harness may be writing the session (`HarnessRunning`, `ActiveWriter`), by the harness's own lock where it has one, by its process registry where it keeps one, and otherwise by any running process of that program. The ACP adapter cannot see this and leaves the guard to the caller.

### What capture must include

`capture` writes the tree under one prefix, `home/`, then `manifest.json`. The `StateManifest` holds the harness kind, the session, the physical root, the home, the harness version, the redaction counts, and every file with its tree path, its `Content` (`Opaque`, `Text`, `JsonLines`, `JsonDocument`), its `Mutability` (`AppendOnly`, `Immutable`, `Replaceable`), its length and its BLAKE3 digest. Its fields are private; a manifest comes from `StateManifest::new` or from a file, and both check the same rules: format 1, every path under `home/`, no temporary file, strictly sorted.

An adapter's sources are exactly the files the harness reads to continue the session, and nothing a second session shares. Name matching is exact: session `s1` never carries `s10.json`. A file on the denylist (`denylist.rs`: credential file names, key and database suffixes, dotenv files, credential directories, each harness's host state, Fabric's own `~/.fabric`) is refused on its tree path right before the copy, whichever adapter listed it; the list is policy, not configuration. A symlink in session state, at the store or above it under the home, refuses the capture (`SymlinkInState`). `out` belongs to the capture: a file in it that the new manifest does not list is removed before the manifest is written.

Mutability is data the adapter declares per file, and restore depends on it. `AppendOnly` is a transcript that only grows: restore accepts a destination that is missing, equal, or a byte prefix of the incoming bytes. `Immutable` is written once. `Replaceable` is rewritten in place by the harness: at capture the adapter records the digest of each such file in a pending departure record under `~/.fabric/harness/departed/<harness>/<session>.pending.json`, `confirm_departure` promotes it once the snapshot is published, and restore replaces the file only when it still holds the bytes this node recorded. A file that fails every rule is `StateDiverged`, and nothing is written. Restore also lists what a capture of the session would take on this node now, and a file on that list the incoming state does not place is removed only when the confirmed departure record says this node left it untouched.

A restore accepts only the root and home of the capture (`RootMismatch`, `HomeMismatch`), only destinations the adapter itself captures from (`UnexpectedFile`), only entries whose content and mutability match the adapter's own rule for that path (`EntryMismatch`), no destination through a symlink (`SymlinkInState`) and no two destinations that differ only in letter case (`DuplicateTreePath`). It checks every staged digest, plans every destination, and writes only when all of them pass. Every write is a temporary sibling, an fsync and a rename.

### Redaction

Two regimes exist, and an adapter picks one per file by its `Carry`:

- Redacted (`Carry::Text`, `JsonLines`, `JsonDocument`): the Claude adapter's regime. Text is redacted line by line; JSON goes through the scanner in `jsontext.rs`, which decodes every string outside a signed block, redacts it and re-encodes it. A signed block (a `thinking` block with a `signature`, or `redacted_thinking`) is never edited: one that holds a secret is dropped whole when it is an array element and counted as `signed-block-dropped`, otherwise the capture fails (`SecretInSignedBlock`).
- Refused (`Carry::OpaqueRefuseSecrets`): the Codex and ACP regime, and the one the PR 5 adapters use. Bytes travel verbatim and the capture is refused when a line holds a secret shape, on the raw line or in any JSON string literal once decoded (`SecretInState { path, line }`). A refused thread stays refused until the secret is gone from it.

The rules are in `redact.rs`: Anthropic, OpenAI, GitHub, AWS, Slack, Google and Stripe key shapes, JWTs, PEM private key blocks, `Authorization` headers, bearer tokens, credentials in URLs, and the value of an upper-case variable whose name says secret, token, password, API key, private key, access key, credentials, `DATABASE_URL` or `DSN`, except a short number under a qualified name (`MAX_TOKENS=512`). Every key rule starts on an ASCII word boundary. Launch arguments go through the same rules (`refuse_secret_args`, `SecretInArguments`), because the harness kind with its arguments is recorded in the ledger and the manifest unredacted; `--api-key sk-...` is caught joined or apart. Redaction is by pattern: it removes common key shapes, not every secret, and the README says so.

### The version guard and the writer-version rule

An older harness must not run a session a newer one wrote (gate finding F10b, ADR 0015): Codex 0.142.4 appended records without a field 0.155.1 writes, and 0.155.1 then refused the session. Before a resume starts, the adapter asks the binary it would run for its version, in the workspace and with the launch environment, and refuses when it is older than the newest version the session records or when it cannot be read (`version::refuse_older`, `HarnessOlderThanSession`, `HarnessVersionUnknown`). `Running::version` reports the version that now runs the session.

The writer-version rule says where that floor comes from. A harness that records its version on every write, as Claude does per transcript line, gives the floor from the session alone. A harness that records it only when it creates a session, as Codex does in `session_meta`, needs the node to carry the newest writer: a capture records the installed version it asked in the workspace, or the job's known writer when that is newer, in `StateManifest::harness_version`; the node keeps it as the session's known writer (`SessionRecord::known_writer`), raises it to `Running::version` after a resume, and hands it back to the adapter on the next capture (`Codex::with_known_writer`). `--force` on a move does not lower it. A PR 5 adapter states in its section which case it is, and an adapter whose format records no version at all says so and is not guarded.

Each adapter also pins the version it was read on for any vendor rule it relies on (Codex's writer lock at 0.155.1, Claude's session registry at 2.1.294) and takes the cautious path on an older or unknown binary (`version::at_least`). A PR 5 adapter pins the version its layout was read on, the ones in the first paragraph of this page, the same way.

### The fidelity declaration

`Fidelity` (`harness.rs`) has four values, with the stored names the managed continuation and the Portable Session Format use (`fabric_core::managed::package::Fidelity` on the M5 branch, ADR 0024; ADR 0029): `native_session`, `new_native_session_with_package`, `workspace_only`, `unsupported`. Nobody keeps a second vocabulary.

- `Harness::fidelity` is the declared ceiling of the adapter, true before any refusal: the README table and a client can say it without running anything. Claude Code, Codex and ACP declare `NativeSession`.
- `Restored::fidelity` is what one restore reached, never above the ceiling, and `Restored::session` is the session the harness continues on this node: the manifest's, unless the harness minted a new id when the adapter handed it the state. The node records `Restored::session` as the job's session (`restore_harness` in `fabricd`), so the next capture takes the session that actually runs, and logs the fidelity. `pipeline::restore` sets `NativeSession` and the manifest's session, because it places the harness's own files; an adapter that then imports them and gets another id back lowers both fields itself.
- An adapter never claims `NativeSession` when the id changed (decision of 2026-11-08, OpenCode). `NewNativeSessionWithPackage` is inside the default `allowed_fidelity` of the M5 branch, so a job accepts it unless it narrowed the set.
- The achieved fidelity is not yet written into the job's record: that lands with the managed continuation (PR 1 of the roadmap, M5b). Until then an adapter's roundtrip test asserts the fidelity through `Restored`, and its end-to-end row through the node's log line, which names the captured and the continued session and the fidelity when they differ.

### Features, names and where a kind appears

Each adapter is a cargo feature of `fabric-harness`, on by default: `acp`, `claude`, `codex`, and since PR 5 `opencode`, `pi`, `gemini`, `cursor`. CI builds the crate with no feature and with each one alone (`clippy -D warnings`), so a module must compile without its siblings. A kind is one variant of `fabric_core::harness::HarnessKind`, appended after the last so the postcard index of every kind a ledger holds stays what it was: `OpenCode`, `Pi`, `GeminiCli`, `CursorAgent`, displayed as `opencode`, `pi`, `gemini-cli`, `cursor-agent`. The CLI takes `--harness opencode | pi | gemini | gemini-cli | cursor | cursor-agent` (`fabricd::harnesses::parse_harness`). The client API writes `{ "kind": "opencode" | "pi" | "gemini_cli" | "cursor_agent" }`; the set of harness kinds is a closed enum of the API, so adding them bumped `API_VERSION` to 2 (ADR 0018, Compatibility rule 3), with `protocol/fixtures/closed/harness.json`, one `request/job_new_<kind>.json` per kind and the TypeScript `Harness` schema updated together. The node builds the adapter in `fabricd::harnesses::adapter` and the CLI in `fabric::adapter_for`, from the home and the `PATH` alone; none of the four reads `harness.toml`.

Until an owner builds a step, the adapter answers it with `HarnessError::NotBuilt { program, step }`, which names this page. `kind`, `fidelity` and `detect` are real from the start. The variant goes when the last adapter is built.

### Fake harnesses, e2e rows and fixtures

Every adapter is tested against a fake on `PATH` and a synthetic on-disk layout; no test touches a real session. Three shapes exist and a new adapter copies the one its harness matches:

- A shell-script fake that answers `--version` and logs its arguments, working directory and environment (`crates/fabric-harness/tests/common/mod.rs` for Claude; `fake_codex_reporting` and `fake_codex_answering` inline in `tests/codex_roundtrip.rs`, which answer `codex-cli <version>` and write argv to `$FAKE_LOG` and the environment names to `$FAKE_ENV_LOG`). Written through `tests/executable/mod.rs`, so Linux never sees `ETXTBSY`.
- A real fake agent for a protocol harness (`tests/fake_acp_agent/mod.rs`): one JSON file per session under `FAKE_STATE_DIR`, shaped by `FAKE_LOAD`, `FAKE_RESUME`, `FAKE_ASK_PERMISSION`, `FAKE_PID_FILE`, `FAKE_ENV_FILE`, `FAKE_PROMPT_ERROR` and `FAKE_LINGER`; built again as the `fabric-fake-agent` binary for the end-to-end suite.
- The end-to-end fakes in `crates/fabric/tests/e2e/world.rs` (`install_fake_claude`, `install_fake_codex`): shell scripts that keep their session files where the real harness keeps them, under the test's home, and answer the headless protocol the CLI reads.

The end-to-end suite (`crates/fabric/tests/e2e`) starts two `fabric serve` daemons on loopback, makes a job, suspends on one node and resumes on the other (`move_and_resume` in `main.rs`); `headless.rs` holds one row per harness that runs a turn without a terminal and is captured by the next suspend, and `acp_cli.rs` the ACP rows. PR 5 reserves one module per adapter, `e2e/opencode.rs`, `e2e/pi.rs`, `e2e/gemini.rs`, `e2e/cursor.rs`, and each owner installs its fake from its own module through `Env::install(name, script)`, so `world.rs` and `main.rs` are not edited twice.

Fixtures live under `crates/fabric-harness/tests/fixtures/<adapter>/` (`tests/fixtures/README.md` has the rules): the agent's real layout with synthetic content, every secret replaced by a value the redactor matches (so the refusal tests fire on it), the home written as the test's temporary home at test time, never a path from a real machine, and the version the layout was read on in a `VERSION` file beside it. An integration test `tests/<adapter>_roundtrip.rs` per adapter is reserved with its `[[test]] required-features` entry.

### Decisions recorded for PR 5

Selected on 2026-11-08 under Efe's delegation of 2026-11-02 (`m5-m6-decisions-2026-11-02.md`, section PR 5); Efe can veto any of them at the pull request.

1. Order: OpenCode, pi, Gemini CLI, Cursor Agent CLI, then GitHub Copilot CLI only if its id-only resume holds in a test within a day; Droid, Kilo, Amp, Goose and Crush after PR 5; Aider as a workspace snapshot; Warp `unsupported`.
2. Cursor: no local ACP transcript and an encrypted headless store beside its key. The adapter declares `workspace_only`; the opaque store is not captured until Cursor documents the format (fail closed, no guessed continuation). The e2e refusal case stays; the ACP case is `new_native_session_with_package` or dropped.
3. OpenCode: if `opencode import` mints a new session id, the restore reports `new_native_session_with_package` in `Restored`, which the default `allowed_fidelity` permits, and never `native_session`.
4. Copilot CLI: in PR 5 only as the last adapter and only if its id-only resume holds; otherwise a documented contributor task after PR 13.
5. Made here, for the shared surface: the four kinds are variants of `HarnessKind` and features of `fabric-harness`; the client API goes to version 2 because its harness set is closed; the trait gains `fidelity` and `Restored` gains `session` and `fidelity`; the node records `Restored::session`.

## OpenCode

Owner A. Feature `opencode`, kind `HarnessKind::OpenCode`, program `opencode`, read on 1.18.34. OpenCode is the largest open-source harness after Claude Code and Codex (3,431,953 npm downloads in the week of 2026-11-01, 225,452 stars, 7 percent workplace use in the JetBrains 2026 survey as 36Kr reports it), and Kilo CLI vendors it (`.opencode-version` `v1.18.26` on Kilo-Org/kilocode), so the adapter serves two programs once its schema expectations are versioned.

**Session location and format.** One SQLite file, `~/.local/share/opencode/opencode.db`, in WAL mode (`opencode.db-shm`, `opencode.db-wal` beside it), holding every project: tables `session` (columns `id` with a `ses_` prefix and 30 characters, `project_id`, `workspace_id`, `parent_id`, `slug`, `directory`, `path`, `title`, `version`, `share_url`, summary and token columns, `revert`, `permission`, `agent`, `model`, times), `message` (`data` is one JSON column), `part`, `todo`, `session_message`, `session_input`, `session_context_epoch`, `session_share`, `project` (`worktree` is the absolute path), `project_directory`, `workspace`, `event`, `permission`, and the login tables `account` (`access_token`, `refresh_token`), `account_state`, `control_account` (`access_token`), `credential` (`value`), plus `session_share.secret`. `session.directory` is the absolute workspace path, which is the path key ADR 0011 keeps. Git snapshots of tool edits sit under `~/.local/share/opencode/snapshot/<hash>`; `auth.json` (mode 0600), `log/`, `repos/` and `tool-output/` are beside them.

**Captured and restored.** The JSON `opencode export <id>` writes, as one `Carry::OpaqueRefuseSecrets` file under `home/.local/share/opencode/fabric/<id>.export.json`, `Mutability::Replaceable`; the adapter runs the export itself at capture, so it never reads the database. `opencode export --sanitize` exists ("redact sensitive transcript and file data") and is the vendor's own redaction; the adapter runs Fabric's scan on the output in either case, and the roundtrip test compares what each removes. Restore places the file and runs `opencode import <file>`, which prints `Imported session: <id>`; the id it prints is `Restored::session`. The database file, its WAL, `auth.json` and the snapshot directory never travel; the denylist refuses `auth.json` and the `.db` suffix by name.

**Carried settings.** `model` (`-m provider/model`), `agent` (`--agent`) and `variant` (`--variant`, the provider-specific reasoning effort), read from the exported session's own record of the `session` row, each pinned value winning; `SettingName::Model`, `Provider` and `Effort` carry them, with `agent` reported under a new `SettingName::Agent`.

**Headless command.** `opencode run --format json -m <provider/model> [--agent <name>] [--variant <v>] -- <prompt>`; the adapter reads the JSON events for the session id and the last assistant text into a `TurnReport`, as `Codex::read_headless_line` does. **Resume.** `opencode run -s <id> ...` continues the session; `--fork` is never passed. Interactive: `opencode -s <id>` in the workspace. The exact event names come from `opencode run --format json` of 1.18.34 and go into the module doc with the date they were read.

**Achieved fidelity and why.** Declared `native_session`. Reached `native_session` when `import` keeps the id, `new_native_session_with_package` when it mints one; the server-side import code that decides this was not located in the research pass, so the first test of the adapter settles it against the real binary, and the fake can do either (`FAKE_OPENCODE_NEW_ID=1`).

**Refusals.** `SecretInState` on any line of the export; `ActiveWriter` while an `opencode` process holds the database open (`lsof` on the `.db`, as the Codex adapter asks), until the writer rule is read from the source and pinned; `HarnessOlderThanSession` when the installed `opencode` is older than the version the export records (`session.version`), the writer-version rule's first case; `SessionNotFound` when `opencode export` says the id is unknown; a `-s` id that does not match the `ses_` shape is refused before any process starts. The database is never opened by Fabric, so there is no SQLite dependency and no version-fragile row SQL.

**Fake harness.** A shell `opencode` that answers `--version` with `1.18.34`, keeps a fake `opencode.db` with an `account` row holding a fake token the redactor matches (`sk-ant-FAKE...`), writes an export JSON for `export <id>`, prints `Imported session: <id>` for `import <file>` (the same id, or a new one under `FAKE_OPENCODE_NEW_ID=1`), and for `run` logs argv and prints the JSON events of one turn. e2e row: `e2e_an_opencode_job_moves_and_resumes_without_its_database`: both earlier turns present on B, `-m` carries the model, the fake token string appears nowhere in the snapshot or on B, and `Restored::fidelity` is the one the fake's id behaviour implies.

## pi

Owner A. Feature `pi`, kind `HarnessKind::Pi`, program `pi`, read on 0.80.3 (`@earendil-works/pi-coding-agent`, author Mario Zechner, repository earendil-works/pi). Its npm count (6,148,981 a week) is inflated by products that embed the SDK; Earendil's own claim is hundreds of thousands of weekly users. It is the cleanest store of the four and the one that proves the adapter guide on a third native format.

**Session location and format.** One JSONL file per session, `~/.pi/agent/sessions/--<workspace path with / replaced by ->--/<ISO timestamp with : as ->_<uuid>.jsonl` (for a workspace `/home/u/w` the directory is `--home-u-w--`). Version 3 of the format (`docs/session-format.md` in the installed package): a header entry `{"type":"session","id":"<uuid>","cwd":"<workspace>","timestamp":...,"version":3}`, then `model_change` (`provider`, `modelId`), `thinking_level_change` (`thinkingLevel`), and `message`, `text` and `thinking` entries forming a tree. An Anthropic reply carries a `thinkingSignature`, a signed block that cannot be edited.

**Captured and restored.** That one file, `Carry::OpaqueRefuseSecrets`, `Mutability::AppendOnly`, under `home/.pi/agent/sessions/<dir>/<file>`; the directory key is derived from the physical workspace path exactly as pi derives it, and the id is matched on the file name's `_<uuid>.jsonl` suffix, so a partial id never matches. Restore places it under the same key. `auth.json` (0600), `settings.json` and `trust.json` (documented, absent on the owner's Mac) never travel.

**Carried settings.** The provider and model of the last `model_change` and the level of the last `thinking_level_change`, as `SettingName::Provider`, `Model` and `Effort`, passed with pi's `--model <provider/model>` and `--thinking <level>` flags (owner A confirms the exact spelling against `pi --help` of 0.80.3 before the fake copies it); pinned values win.

**Headless command.** `pi -p --mode json --session <path> <prompt>`, reading the JSON output for the answer; `--session-id` and `--session-dir` are not used, so pi files the session where the adapter looks for it. **Resume.** `pi --session <path or id>` in the workspace, interactive; the adapter passes the full path it placed, never a partial id, which pi would also accept.

**Achieved fidelity and why.** `native_session`: the same file, byte for byte, under the same path key, and pi continues it by its own id. The header `cwd` equals the workspace, which ADR 0011 guarantees.

**Refusals.** `SecretInState` on any line; `ActiveWriter` while a `pi` process runs for that workspace (pi takes no lock, so the guard is by process, as Claude's is below 2.1.294); `LineageBroken` is not possible, there is one file; `SessionNotFound` when no file of that id sits under the workspace's key; two files with the same uuid under one key is `LineageAmbiguous`. The format records no pi version, so the version guard has no floor from the session and the manifest carries the installed version only (the writer-version rule's second case, stated here).

**Fake harness.** A shell `pi` that answers `--version` with `0.80.3`, writes the JSONL described above with a `thinkingSignature`, a `model_change` and a `thinking_level_change`, writes a fake `auth.json`, and logs argv for `--session`. e2e row: `e2e_a_pi_session_moves_and_resumes_as_the_same_file`: equal bytes on B, `auth.json` absent from the snapshot, provider, model and thinking level reported as carried, fidelity `native_session`.

## Gemini CLI

Owner B. Feature `gemini`, kind `HarnessKind::GeminiCli`, program `gemini`, from google-gemini/gemini-cli `main` and v0.67.0 (2026-11-04); not installed on the owner's Mac. Consumer tiers were retired on 2026-06-18 in favour of Antigravity; it stays usable with paid API keys, and its weekly downloads are down 38 percent from the June peak but rising since September (305,018 in the week of 2026-09-07 to 482,840 in the week of 2026-11-01).

**Session location and format.** Chats under `~/.gemini/tmp/<slug>/chats/`: a main session is `session-<ISO minute with : as ->-<first 8 characters of the id>.jsonl`, a name collision adds a counter, `session-<ts>-<n>-<id8>.jsonl`, and a subagent's file is `<full id>.jsonl` under `chats/<parent id>/` (`chatRecordingService.ts`, `chatRecordingTypes.ts`). The session's plans, tracker and tasks sit under `<slug>/<id>/plans`, `<slug>/<id>/tracker`, `<slug>/<id>/tasks` (`storage.ts`). The slug is `slugify(basename(project root))` with a `-<n>` suffix on collision, regex `^[a-z0-9-]+$`, assigned by the registry `~/.gemini/projects.json` and owned by a `.project_root` marker inside the slug directory that `verifySlugOwnership` checks; legacy SHA-256 directories are migrated (`projectRegistry.ts`). The published docs still say `tmp/<project_hash>/`; the code is the reference. Default cleanup deletes sessions older than 30 days, with a one-day minimum.

**Captured and restored.** The chat file resolved from the id (the 8-character prefix, then the full id read from the first record to rule out a second session sharing the prefix), its subagent files, and the three side directories, all `Carry::OpaqueRefuseSecrets`; the chat and subagent files `AppendOnly`, the side directories `Replaceable`. Restore places them under the slug the registry on this node assigns to the workspace, writes the `projects.json` entry and the `.project_root` marker when they are missing, and refuses a slug directory whose marker names another root. The generic ACP adapter is refused for Gemini: with `<slug>/` as state directory it would accept the `<id>/` side directories and miss the chat file, a partial capture reported as a session. `oauth_creds.json`, `settings.json` and `.env` never travel; the denylist refuses the first and third by name.

**Carried settings.** The model, read from the chat's own records (the field is named when the fixture is written from the documented format), as `SettingName::Model` with `--model`; pinned values win.

**Headless command.** `gemini -p <prompt>` with `--output-format json` where v0.67.0 offers it, else the text output; the session id is read from the chat file the run created. **Resume.** Over ACP, `gemini --acp` with `session/load` (the ACP client already speaks it), or headless with `gemini --resume <uuid>`; the interactive resume is `gemini --resume <uuid>` in the workspace.

**Achieved fidelity and why.** `native_session`: the chat file is replayed by `session/load` or `--resume` under its own id, once the resolver maps the id to the file and the registry entry is in place.

**Refusals.** `SecretInState`; `SessionNotFound` when no chat file matches the prefix; an ambiguous prefix with two full ids is `LineageAmbiguous`; a slug directory whose `.project_root` names another workspace is `StateDiverged`; `ActiveWriter` while a `gemini` process runs for the workspace. The chat records the CLI version per session file; it is the floor of the version guard where present.

**Fake harness.** The `fabric-fake-agent` shape, laid out as Gemini: it writes `tmp/<slug>/chats/session-<ts>-<id8>.jsonl`, `projects.json`, `.project_root`, a second session that shares the 8-character prefix, and a fake `oauth_creds.json`, and answers `session/load`. e2e row: `e2e_a_gemini_session_found_by_its_short_id_resumes_through_session_load`: capture takes the chat file and the `<id>/` directories and not the prefix-sharing session, B's registry maps the workspace to the slug, `session/load` replays both turns, the OAuth file is absent.

## Cursor Agent CLI

Owner B. Feature `cursor`, kind `HarnessKind::CursorAgent`, program `cursor-agent` (also installed as `agent`), read on 2026.10.01-e373342. Blocked on format, not on code: the decision of 2026-11-08 is `workspace_only` by default.

**Session location and format.** A headless or interactive chat lives in `~/.cursor/chats/<md5 of the workspace path>/<chat id>/`: `meta.json` (`createdAtMs`, `cwd`, `hasConversation`, `schemaVersion`, `updatedAtMs`, sometimes `title`), `prompt_history.json`, and `store.db` with `store.db-shm` and `store.db-wal`. `store.db` has exactly two tables, `blobs(id TEXT PRIMARY KEY, data BLOB)` with 64-hex content-addressed ids and encrypted data, and `meta(key TEXT PRIMARY KEY, value TEXT)` with one row, key `0`, whose value is hex-encoded JSON with `agentId`, `blobEncryptionKey`, `createdAt`, `isRunEverything`, `latestRootBlobId`, `mode`, `name` and often `subagentInfo`. Eleven of fifteen stores on the owner's Mac could not be opened read-only while their WAL existed and needed `immutable=1`. An ACP session (`cursor-agent acp`) leaves only `~/.cursor/acp-sessions/<id>/meta.json` with `cwd` and `schemaVersion` 1, and none of the eight ACP session ids there appears under `chats/`: the ACP transcript is server-side or in memory. `cli-config.json` holds `authInfo`, `model`, `permissions`, `sandbox` and `approvalMode`.

**Captured and restored.** Nothing of the chat. The encrypted blobs cannot be scanned for the invariant, decrypting them would mean parsing a private format with its key, and the ACP path has no local transcript. The adapter's `capture` succeeds with an empty file list and a manifest that names the session and the reason in its redaction counts (`cursor-store-not-carried`), so a move still carries the workspace and the job's record; `restore` places nothing and reports `Restored::fidelity` `WorkspaceOnly` with the manifest's session. `cli-config.json` and the chat directories never travel.

**Carried settings.** The model, when the job pinned one, as `--model`; nothing is read from the store. `--sandbox` and `--mode plan|ask` are pinned settings too, never recorded ones.

**Headless command.** `cursor-agent -p --output-format json [--model <m>] <prompt>` in the workspace, which starts a new chat; the chat id is read from the JSON output when it names one, else from the newest directory under `chats/<md5>/`. **Resume.** None of the stored chat. A `fabric job resume` under `cursor` starts a new chat in the restored workspace, with the goal as the first prompt when none is given, as a `workspace_only` continuation does; `--resume <chat id>` is used only on the node that holds that chat, which a moved job by definition does not.

**Achieved fidelity and why.** Declared and reached `workspace_only`. The ACP case would be `new_native_session_with_package` at best, a new ACP session handed the package, and is dropped from PR 5 unless the package exists on this branch when the row is written.

**Refusals.** `SecretInArguments` for `--api-key <key>` or any secret shape in the launch arguments; a job that narrowed `allowed_fidelity` to `native_session` alone is refused before any write by the fidelity check, with the reason in this section's words; `HarnessRunning` while a `cursor-agent` process runs in the workspace, so a capture of the workspace snapshot does not race its edits.

**Fake harness.** A shell `cursor-agent` that answers `--version` with `2026.10.01-e373342`, writes `chats/<md5>/<id>/store.db` with a `meta` row holding a `blobEncryptionKey` and `acp-sessions/<id>/meta.json`, and for `-p` logs argv and prints a JSON answer. e2e row: `e2e_a_cursor_chat_is_refused_unless_the_job_allows_workspace_only`: without the opt-in the move is refused before any write and says why; with it the workspace resumes on B and a new chat starts; in both cases `cli-config.json` and `store.db` never travel.

## GitHub Copilot CLI (conditional)

Not in Efe's list; the usage case is the fourth largest measured (1,983,364 npm downloads a week). In PR 5 only as the last adapter and only if its id-only resume holds in a test within a day; otherwise a documented contributor task after PR 13, and no `HarnessKind` variant is added for it here.

**Session location and format.** `~/.copilot/session-state/<id>/` per session and `~/.copilot/session-store.db`, which the docs say "can be recovered from session files" (docs.github.com, Copilot CLI chronicle). Session data is synced to the GitHub account by default. **Headless and resume.** `copilot -p <prompt>`, `copilot --resume=<id>`, `copilot --continue`. **Expected fidelity.** `native_session` by id alone when both nodes are logged in to the same account and the sync holds, as Amp's threads are; the `session-state/<id>/` directory carried as opaque refused-on-secret files otherwise. **Refusals.** A target without the login refuses, as the Claude and Codex login rows do. **Fake.** A shell `copilot` that keeps `session-state/<id>/` and answers `--resume=<id>`; e2e row `e2e_a_copilot_session_dir_moves_and_resumes_by_id`, the pi shape.

## Beyond PR 5

For the record, from the same research: Droid (`~/.factory/sessions/<path>/<uuid>.jsonl` plus `<uuid>.settings.json`, community-documented and undated) would follow the pi shape; Kilo CLI reuses the OpenCode adapter under another program name once its vendored version is pinned; Amp is an id-only adapter (server-side threads, both nodes logged in to one account, `amp threads continue <id>`); Goose through the generic ACP adapter (`goose acp`, sessions in `~/.local/share/goose/sessions/sessions.db`); Crush needs no state adapter, only a quiesce check for its in-workspace `.crush/crush.db`; Aider is the workspace snapshot plus `--restore-chat-history`, a replay, so `new_native_session_with_package`; Warp has no documented local session and is `unsupported`.

## Owners and the shared surface

Owner A: `crates/fabric-harness/src/opencode/`, `src/pi/`, `tests/opencode_roundtrip.rs`, `tests/pi_roundtrip.rs`, `tests/fixtures/opencode/`, `tests/fixtures/pi/`, `crates/fabric/tests/e2e/opencode.rs`, `e2e/pi.rs`, and their sections of this page and the README table.

Owner B: `src/gemini/`, `src/cursor/`, `tests/gemini_roundtrip.rs`, `tests/cursor_roundtrip.rs`, `tests/fixtures/gemini/`, `tests/fixtures/cursor/`, `e2e/gemini.rs`, `e2e/cursor.rs`, their sections, and Copilot CLI if its test holds.

Shared, done in the contract commits so neither owner edits them: the `HarnessKind` variants and their display names; `Fidelity`, `Harness::fidelity`, `Restored::session` and `Restored::fidelity`, and the node recording `Restored::session`; the `NotBuilt` error; the four features, module scaffolds, `[[test]]` entries and empty roundtrip and e2e files; `parse_harness`, the CLI help, the adapter construction in `fabricd::harnesses` and `fabric::adapter_for`, and the probe tables in `fabricd::ops`; `API_VERSION` 2 with `closed/harness.json`, the `job_new_<kind>` fixtures, `required.txt` and the TypeScript schema; the per-feature clippy loop in CI; this page, the README table and the fixture rules. A setting name an adapter needs that `SettingName` lacks (`Agent` for OpenCode) is added by its owner in `harness.rs` with one line and no other change there.

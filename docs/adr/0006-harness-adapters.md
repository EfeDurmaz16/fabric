# ADR 0006: Harness adapters

Status: accepted, 2026-10-19

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

HarnessKind     = ClaudeCode | Codex | Acp { command: String }
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

These two get dedicated adapters because the first gate is Claude Code and the second harness is Codex, and because both have known native resume paths that ACP wraps:

- Claude Code: state lives under the user's Claude projects directory keyed by workspace path; `restore` places it under the same key because the root is identical on every node (ADR 0008), and `launch` runs a foreground `claude --resume` in the user's terminal and returns `Spawned`. It never uses `claude --bg`, whose own supervisor restarts a killed session and would defeat the fencing kill (ADR 0003). `stop` on a foreground session is the graceful exit the research names (H1, section 11.3). Reuse of claude-handoff 0.3.0's exporter and redaction rules will be evaluated rather than rewritten (landscape section 8.1).
- Codex: rollout files plus `thread/resume` over app-server, or the `codex-acp` adapter if it proves equal. Decide by testing both on the real gate.

Each adapter is a module in `fabric-harness` behind a cargo feature. The core compiles with no adapter enabled.

### Harness state is opaque to the core

The core sees `harness_state` as a list of relative paths and chunk ids. The only thing the core does with it is hash, store, transfer and hand it back to the adapter. The core never redacts, never parses JSONL, never rewrites paths. Redaction is therefore part of the `Harness` contract, not an option: `capture` returns a `RedactionReport` per rule applied, and the suspend output prints the counts next to the excluded workspace paths (ADR 0004).

For the Claude Code adapter the contract is concrete, because the research shows what the state carries (H1, sections on the config directory and on identity):

- Never included: `.credentials.json`, anything derived from the Keychain, `~/.claude.json` as a whole (it holds `oauthAccount`, `machineID`, `remoteControlMachineId`, `userID`), `sessions/`, `session-env/`, `shell-snapshots/`, `jobs/`, `daemon/`, `backups/`, `cache/`, `telemetry/`, `stats-cache.json`. Each node signs in once through Anthropic's own flow; carrying credentials would also breach the terms the research cites.
- Redacted in the transcript and sidecars, following claude-handoff 0.3.0's rules: secret patterns, `.env` values that appear in tool results, the account email and the organization UUID in `session_context` and `credential_org` attachments.
- A secret inside a signed `thinking` block cannot be edited without breaking the block (next section). The adapter then drops that whole block, which the API accepts, and counts it in the report.

### Path rewrites: none by default, never inside a signature

`restore(state, from_root, to_root)` with `from_root == to_root` performs no rewrite at all, and in v0 that is the only case the Claude Code adapter accepts (ADR 0008). The reason is a fact about the state, not caution: Claude Code thinking blocks carry an encrypted `signature`, and a modified block is rejected with a 400 on the next tool turn (research H1, item 2). A blanket rewrite of the transcript breaks the resumed session at its first tool call, which is exactly the first gate.

When roots differ, which no v0 path allows for Claude Code, the adapter rewrites only the project directory key, `cwd` fields and sidecar references, following claude-handoff's placeholder rules at path-component boundaries, and never a `thinking` block or any field under a signature. The property test for this is stated now so that it exists before the code: for arbitrary roots, a rewritten transcript leaves every signed block byte-identical.

### Cross-harness resume is an experiment, not a promise

`fabric resume <job> --harness codex` on a job captured under Claude Code cannot load Claude's native state into Codex. What it can do is produce a neutral brief (goal, recent decisions from the ledger, `git log` and `git status` of the restored workspace, the last user turn if the source adapter exposes it) and start the new harness with that brief as its first prompt. This is the same text-handoff shape T3 Code uses and it loses tool state and approvals. It is listed as a later experiment. Fabric does not claim it works until a measured test says how much is lost.

## Alternatives considered

1. **Drive agents through their SDKs** (Claude Agent SDK, Codex app-server as a hosted service). Both carry terms that restrict third-party hosting and login use (landscape section 8.1). Fabric runs the user's own CLI under the user's own login on the user's own machines, which is the path the research reads as allowed. SDK coupling is also the fastest way to a core release per vendor release. Rejected for the core; an adapter may use an SDK internally if its terms allow.
2. **`coder/agentapi` as the only path.** HTTP over several CLIs, MIT. It does not capture or restore state, which is the part Fabric needs. Could be a launch mechanism inside an adapter. Not the trait.
3. **A Fabric-defined session format that every agent must export to.** Nobody would implement it. Rejected. The opaque state directory plus per-adapter knowledge is the pragmatic version.
4. **Transcript-only moves** (claude-handoff, session-roam). Covers Claude only and omits the workspace. Used as a reference and possibly a library inside the Claude adapter.

## Consequences

- Adding a harness is one module and one feature flag. No core change.
- A vendor changing its state layout breaks one adapter's tests, not the system.
- `detect` returning `None` is a normal answer and the CLI explains it ("no Claude Code session found for this workspace").
- Adapter tests are integration tests against the real CLI when installed, skipped with a clear message when it is not. Unit tests cover path rewriting with property tests over arbitrary roots, including the byte-identical signed block property above.

## Open questions

- Does `session/load` in `claude-agent-acp` restore the same context as native `claude --resume`? Test both on the gate.
- Pending approvals: drain or cancel at capture, re-ask on launch. The research recommends not moving them (t3code note, section 2). Confirm with the first real suspend of a job mid-approval.
- Subagent transcripts and memory files outside the project directory: adapter decides what is part of "state". Start with what claude-handoff includes.

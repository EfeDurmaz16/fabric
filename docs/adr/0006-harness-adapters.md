# ADR 0006: Harness adapters

Status: accepted, 2026-10-19

## Context

The agent that does the work changes weekly. Claude Code keys resume to a working directory (claude-code #5768, #28745, cited in `2026-10-19-t3code-vs-fabric.md`, section 2). Codex exposes `thread/resume` through its app-server JSON-RPC. T3 Code, when it hands off, starts a fresh provider session fed with text and loses tool state and pending approvals (same note, section 1). The Agent Client Protocol (ACP) now has adapters for both: `zed-industries/claude-agent-acp` and `agentclientprotocol/codex-acp` (agentclientprotocol.com, agents list, checked 2026-10-19), with `session/new`, `session/load` gated by the `loadSession` capability, `session/prompt` and `session/cancel`.

Fabric's core must not know any of this. If it does, every vendor release is a core release.

## Decision

### The `Harness` trait

Defined in `fabric-core`, implemented per harness in `fabric-harness`.

```text
trait Harness {
    fn kind(&self) -> HarnessKind;

    async fn detect(&self, workspace: &Path) -> Result<Option<Detected>, HarnessError>;
        Is there a session of this harness for this workspace? Where is its state?

    async fn capture(&self, workspace: &Path, out: &Path) -> Result<Captured, HarnessError>;
        Ask the running harness to stop at a turn boundary, then copy everything it needs
        to continue into `out`. Returns what was copied and any paths that are absolute.

    async fn restore(&self, state: &Path, from_root: &Path, to_root: &Path) -> Result<(), HarnessError>;
        Place the captured state where this harness expects it on this node,
        rewriting paths from `from_root` to `to_root`.

    async fn launch(&self, workspace: &Path, resume: ResumeHint) -> Result<Child, HarnessError>;
        Start the harness so that it continues the same session.
}

HarnessKind = ClaudeCode | Codex | Acp { command: String }
```

The core calls these four in order during suspend (`detect`, `capture`) and resume (`restore`, `launch`). It stores whatever `capture` produced as the `harness_state` tree of the snapshot (ADR 0004) and never opens it.

### ACP is the generic path

Any agent that speaks ACP and reports `loadSession: true` gets the `Acp` adapter for free:

- `capture`: send `session/cancel` if a prompt is in flight, record the ACP `sessionId`, and copy the agent's state directory as declared in the Fabric job config for that agent.
- `launch`: start the agent command, `initialize`, `session/load` with the recorded id and the new `cwd`.

Agents whose state directory is unknown to Fabric cannot be captured and the adapter says so at `detect` time.

### Claude Code and Codex adapters first

These two get dedicated adapters because the first gate is Claude Code and the second harness is Codex, and because both have known native resume paths that ACP wraps:

- Claude Code: state lives under the user's Claude projects directory keyed by workspace path; `restore` rewrites that key to the new root; `launch` uses native resume. Reuse of claude-handoff 0.3.0's exporter and redaction rules will be evaluated rather than rewritten (landscape section 8.1).
- Codex: rollout files plus `thread/resume` over app-server, or the `codex-acp` adapter if it proves equal. Decide by testing both on the real gate.

Each adapter is a module in `fabric-harness` behind a cargo feature. The core compiles with no adapter enabled.

### Harness state is opaque to the core

The core sees `harness_state` as a list of relative paths and chunk ids. The only thing the core does with it is hash, store, transfer and hand it back to the adapter. The core never redacts, never parses JSONL, never rewrites paths. If an adapter needs to strip account identifiers it does so in `capture`.

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
- Adapter tests are integration tests against the real CLI when installed, skipped with a clear message when it is not. Unit tests cover path rewriting with property tests over arbitrary roots.

## Open questions

- Does `session/load` in `claude-agent-acp` restore the same context as native `claude --resume`? Test both on the gate.
- Pending approvals: drain or cancel at capture, re-ask on launch. The research recommends not moving them (t3code note, section 2). Confirm with the first real suspend of a job mid-approval.
- Subagent transcripts and memory files outside the project directory: adapter decides what is part of "state". Start with what claude-handoff includes.

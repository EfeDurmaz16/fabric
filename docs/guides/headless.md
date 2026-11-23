# Headless turns

A job can take one turn without a terminal: from a script, over SSH, from another device. The turn is recorded like an interactive run, so the next suspend captures the session.

```sh
fabric job run <job> --headless --prompt "Run the tests and fix the first failure."
fabric job resume <job> --headless --prompt "What did you change?"
```

What happens:

- Claude Code runs as `claude -p --output-format json`; Codex as `codex exec --json`; an ACP agent through `session/prompt`.
- The answer goes to standard output, then a last line naming the session (`codex session <thread>` or `claude-code session <id>`). Everything else goes to standard error, so `answer=$(fabric job run ...)` holds the answer and the session line.
- Ctrl-C, or the hang-up of a closed SSH session, is passed on to the harness, so its turn does not keep running unattended.
- An ACP turn the agent reports as failed ends the command with a failure exit, even when the agent process exits 0.

Nobody is there to answer a permission prompt:

- A headless Claude turn is denied every tool call that would ask, unless the workspace's permission rules allow it.
- Codex runs `exec` with approvals off, except on a node that sends approvals to auto review, where it keeps that node's policy. It runs in a workspace-write sandbox, because the trust override marks the workspace trusted, unless it resumes a thread, which keeps its own model, effort and sandbox.

A headless Claude turn on a node whose `claude` login has lapsed, or that was started over SSH, fails with Claude's own `Failed to authenticate`. Check `fabric doctor` first: `claude.login` and `codex.login` say what the harness itself answers.

## Continue a Codex thread on a Vercel executor

`fabric codex-handoff` is an experimental, one-turn command. Close the original
Codex client after its current turn and wait for the thread to be saved. The
local Codex app-server resumes that thread using the machine's existing
ChatGPT login. It selects the supplied Vercel exec-server for the turn.

Prepare a dedicated Sandbox with its workspace already restored and
`codex exec-server` listening on a published port with capability-token
authentication. Use a fresh token for each sandbox, configure its SHA-256
digest on the executor, and restrict outbound access at the sandbox provider.
Codex uses external-sandbox mode for that turn, so the VM provides isolation.
Use the same compatible Codex release on the host and executor.

```sh
printf '%s\n' "$FABRIC_EXECUTOR_TOKEN" |
  fabric codex-handoff --thread <THREAD_ID> \
    --executor wss://<SANDBOX>.vercel.run --cwd <REMOTE_WORKSPACE> \
    --prompt 'Continue from the last turn'
```

Use `--model` if you need to select a model available to the local ChatGPT
account. Only the executor capability token goes on stdin. Vercel provisioning,
workspace restoration, and final deletion remain the caller's responsibility.
For a second executor, run the command again with the same thread ID, the
second route and token, and its restored workspace. Each invocation runs after
the previous turn has completed. Keeping the macOS host available is required
while the turn runs.

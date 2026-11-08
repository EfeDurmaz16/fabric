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

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

Close the original Codex client after its current turn so its thread is saved.
Choose an output directory outside the source checkout. Run the managed
handoff from a directory linked to the Vercel project, or point `--vercel-project` at a linked directory. Install the matching Capsule
worker on PATH or pass its path with `--worker`.

```sh
fabric codex-handoff --thread <THREAD_ID> --workspace <GIT_ROOT> --output <NEW_DIR> --prompt 'Continue from the last turn'
```

The local app-server keeps the existing login and thread. Capsule starts a
fresh VM, installs the matching stable Codex release, restricts network egress,
and restores a checkpoint before accepting the turn. Fabric writes
`answer.txt` and `workspace.tar.gz` locally before telling Capsule to delete the
sandbox. The source checkout stays in place. The archive contains the returned
checkout with its Git index. It is not extracted automatically.

The checkpoint carries a shallow checkout of HEAD, staged and unstaged changes,
and non-ignored untracked files filtered by Fabric's default secret paths and
`.fabricignore`.
It supports UTF-8 regular files and a 64 MiB limit, rejects submodules, special index entries, and tracked paths
blocked by the policy, and omits ignored files and Git history.
Keep the workspace idle during capture. The host must stay awake for the model
turn. If Fabric cannot save the output, Capsule reports the sandbox name and
leaves the VM available until its 10-minute lifetime expires.

### Use a prepared executor

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

## Detach a handoff onto a personal Linux runner

Use an always-available Linux host you trust, with SSH key authentication,
persistent storage, `lsof`, and compatible Fabric, Codex and Capsule builds. Make
`fabric` available to non-interactive SSH commands. On the runner, configure
the Vercel project credentials and authorize Codex with ChatGPT once as the
dedicated runner user. See [Codex headless sign-in](https://learn.chatgpt.com/docs/auth#login-on-headless-devices).
The worker and SSH commands must use the same user and private state directory.
Keep the worker running independently of SSH, for example with a systemd
service enabled at boot and configured to restart it after a failure.

```sh
fabric codex-runner serve --worker /path/to/capsule-bridge-worker --vercel-project /path/to/linked-project
```

On your Mac, close the thread's original Codex client after its current turn.
Submit from the local checkout:

```sh
fabric codex-runner submit --host my-runner --thread THREAD_ID --workspace /path/to/repo --prompt 'Continue the task'
```

Fabric prints the job ID after the runner syncs the checkpoint, thread and
request to its disk and publishes the job. You can disconnect after receiving
that ID. The runner processes the job, saves `answer.txt`,
`workspace.tar.gz`, and the updated native thread under `result/thread/`,
then gives Capsule permission to delete the work VM. Check the status.

```sh
ssh my-runner fabric codex-runner status JOB_ID
```

The report gives the runner's result directory, which you can copy over SSH.
The runner's default queue is `~/.local/share/fabric-codex-runner`. It processes
one job at a time. Use a dedicated Codex home on the runner, and avoid editing
the transferred thread concurrently on the Mac. On restart, the runner
recovers a running job under its original ID without starting another Codex
turn. The runner collects the original sandbox workspace and saves the latest
durable thread. If that attempt needs to be retried, the status is `recovering`.
An `interrupted` job with a saved result can be inspected locally. If the sandbox expired or never
restored its workspace, the result includes the submitted Git checkpoint
and the saved thread. The status detail identifies which workspace was saved.
Use a Capsule worker that supports named handoffs and recovery.

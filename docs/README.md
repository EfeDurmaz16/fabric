# Fabric

Fabric moves a coding-agent job from one machine to another and lets you pick it up where it stopped: the uncommitted work, the dependency caches and the agent's own conversation go with it, and at most one machine can run the job at a time. Fabric is a working name; it will change.

## What moves, what does not

<!-- matrix:begin -->
| On the source | On the target after a move | Why |
|---|---|---|
| Commits, branches, staged and unstaged changes, untracked files, the job's stashes | The same, byte for byte | Carried as a git bundle plus two private commits (index and working tree); your own index and refs are not touched |
| Dependency caches: `node_modules`, `.venv`, Rust `target/` | Carried when the target's toolchain matches the recorded key, else made again by the setup step | The key is exact strings: Node major and package manager, `rustc -vV` and `CARGO_HOME`, Python version and home. A cache with no lockfile always travels |
| Build output: `dist`, `.next`, `build`, `out`, `.turbo` | Carried under its ecosystem's key, else dropped for the next build | Output made for another OS or CPU is left behind |
| `.env` and other secret-shaped untracked or ignored files | Stay on the source | `.env.example` travels. A secret that is tracked or stashed is in git history already, so it travels and suspend names it |
| Claude Code session | Continues with `claude --resume`, the transcript's model | Transcript, sidecars, project memory and file history, redacted by pattern |
| Codex session | Continues the same thread, with its model, provider, effort and sandbox | The thread's rollouts, byte for byte; a secret shape in them refuses the capture |
| ACP agent session | Continues with `session/resume` or `session/load` | The files named for the session in the agent's declared state directory |
| Logins, keychains, `~/.ssh`, `~/.aws`, `auth.json`, `~/.claude.json`, `~/.fabric` | Stay on the source | A hard-coded denylist, checked before every copy |
| A harness that is still running | Nothing moves | Capture refuses while the session may be written; suspend stops the harness first |

Both nodes need the same user name and the same home path, and `git` on the `PATH`: nothing rewrites paths (ADR 0011). One harness moves to the same harness; Claude to Codex is not supported. Proved on two real Macs: a Codex job, on 2026-10-21. Claude Code across two Macs is not recorded in this repository's evidence (gate G2 is pending). The end-to-end test, two nodes on one machine with fake harnesses, moves a job with uncommitted work, `.env`, `dist` and `node_modules` and checks each of them; the crates' own tests cover the other rows against fixtures.
<!-- matrix:end -->

## Start here

1. [Install](start/install.md): build the binary from source. There is no release yet.
2. [First move](start/first-move.md): one job, suspended and resumed on one Mac, then moved to a second one.
3. [What moves and what does not](start/what-moves.md): the table above, row by row.

Then the [concepts](concepts/job.md), the [guides](guides/two-macs.md), the [CLI reference](reference/cli.md), generated from the binary, and the [decisions](adr/README.md) behind all of it. The [roadmap](roadmap.md) says what comes next and what is only on a branch.

These pages describe `main`. A claim that holds only on a branch says so.

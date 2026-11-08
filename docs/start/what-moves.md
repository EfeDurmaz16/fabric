# What moves and what does not

This is the contract of a move on `main` today: what a suspend captures, what a move carries to the other node, and what stays where it was. The table below is the one the README and the site's first page show; it is written here once and copied there by `scripts/docs/gen.py`.

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

Both nodes need the same user name and the same home path, and `git` on the `PATH`: nothing rewrites paths (ADR 0011). One harness moves to the same harness; Claude to Codex is not supported. A Codex job moved between two real Macs on 2026-10-21, on the binary of that day, before M2 to M4 merged. That run's log is not in this repository; the README keeps its timings, and it has not been run again on `main` (gate G1 is pending). Claude Code has never been moved between two machines (gate G2 is pending). The end-to-end test, two nodes on one machine with fake harnesses, moves a job with uncommitted work, `.env`, `dist` and `node_modules` and checks each of them; the crates' own tests cover the other rows against fixtures.
<!-- matrix:end -->

## Each row in more detail

**Git state.** Suspend makes two commits with a private index, `I` for the index and `W` for the working tree, and puts them, the job's stash refs and the history the target lacks in a bundle. Restore rebuilds the same `git status`, the same staged diff and the same unstaged diff. Nothing is written into your `.git` except those commits, the snapshot ref and the job's stash refs ([ADR 0014](../adr/0014-store-as-built.md)).

**Caches and build output.** Every ignored path gets a class: secret, dependency cache, build output, tool cache, or precious. A generic name such as `target` or `dist` is a cache only beside the file of the tool that makes it (`Cargo.toml`, `package.json` and the rest). A cache travels when its carry key matches on the target; otherwise the `EnvSpec` setup step makes it again there (`pnpm install --frozen-lockfile`, `uv sync --frozen`, `cargo build`). A mismatch never fails a move; it makes it slower, and `move` says which key differed. Before anything changes hands, `move` prints one line per ignored path: `carry`, `rebuild` with its setup step, `refuse`, or `leave`. `fabric doctor --peer <node> --job <job>` prints the same lines without moving ([ADR 0012](../adr/0012-dependency-carry.md)).

**Secrets.** Which path is a secret is decided by one policy in `fabric-core`. The list can be widened per job, never narrowed. Harness sessions are scanned too: Claude files are redacted by pattern (key shapes, JWTs, PEM blocks, bearer tokens, credentials in URLs, values of variables named like secrets), and Codex and ACP state is refused when a secret shape is found in it. Pattern redaction removes common key shapes, not every secret ([ADR 0015](../adr/0015-harness-trait-as-built.md), Redaction as built).

**Sessions, by harness.**

| Harness | What is captured | How it continues | Proof on `main` |
|---|---|---|---|
| Claude Code | `projects/*/<sid>.jsonl` and sidecars, project memory, filtered `file-history/<sid>/`, `tasks/` and `uploads/` for the session | `claude --resume <sid> --model <transcript model>`; refuses a Claude older than the one that wrote the session | Fixtures and fakes, end to end on one machine |
| Codex | The thread's rollouts under `sessions/` and `archived_sessions/`, following its lineage | `codex resume` or `codex exec resume`, with the thread's settings; refuses a Codex older than the newest writer | Fakes end to end. Two real Macs on 2026-10-21, before M2 to M4 and with no log in this repository |
| ACP agent | Files named for the session id in the state directory the job declares | `session/resume` when advertised, else `session/load`, else refused | A fake ACP agent, end to end |

A session moves to the same harness only. A neutral text brief to start another harness is an experiment ADR 0006 names, not a feature.

**Same user and home.** A job's root is the same absolute path on every node, and so is the home the harness state lives under, so nothing inside the workspace or the session needs rewriting. Two Macs with different user names cannot move a job today ([ADR 0011](../adr/0011-path-identity.md)).

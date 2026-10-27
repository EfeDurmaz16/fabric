# ADR 0008: Placement and triggers

Status: accepted, 2026-10-19

2026-10-20: [ADR 0016](0016-node-and-cli-as-built.md) records `fabric doctor` as built, with the rows that warn where this table refuses and why.

Amended 2026-10-27 after gate finding F13 (issue #28, M2-08): the version row of the job's harness, Claude's as Codex's, orders the two versions with the parse and order of the resume guard (ADR 0006) instead of comparing strings. It refuses, and `--force` waives, only a target older than the source or than the newest version known to have written the session; a newer target warns, because the guard lets a newer harness resume the session. Two versions that differ where either does not parse still refuse, since they cannot be ordered. The row of a harness the job does not use is unchanged.

Amended 2026-10-21 after gate finding F10: the harness version rows still compare strings and `--force` still waives them, and when the target's version is the older one the note says so, read through the same parse and order the resume guard of ADR 0006 uses. That guard, not this row, keeps an older harness off a session, and no flag waives it.

Amended 2026-10-19 after step 1 research. The root rule moved to ADR 0011. This ADR now states the one placement command of this phase, `fabric job move <job> --to <node>`, the pull form of it, the rule that every automatic choice prints its reason, and the preflight `fabric doctor` runs before any move (`2026-10-19-step1-design-input.md`, sections 8.0 and 9, D18 and D19; `step1/preflight-2026-10-19.md`). Triggers stay out of this phase.

## Context

Two findings from the research cut this ADR down to size. First, task-need placement (GPU, VPN, battery, local DB) is documented in no product and asked for in zero issues; T3 Code ships a resource-based "Auto balance" off by default (`2026-10-19-competitor-landscape.md`, section 7, W3). The report's advice is to drop placement from the pitch. Second, the one trigger with real white space is "laptop is about to sleep" (W2), and it is also the hardest to get right, because a move that starts and does not finish is the two-copies bug (ADR 0003).

A third finding arrived with the preflight. The two gate machines agree on the user and the home directory and on almost nothing else: pnpm 11.23.0 against none, Node v22.23.2 (ABI 127) against v26.0.0 (ABI 147), rustc 1.97.1 against 1.96.0, macOS SDK 27.0 against none reported, Claude 2.1.294 against 2.1.199, Codex 0.155.1 against 0.142.4, macOS 26.6.2 against 26.5.2, and iCloud Desktop sync off against on (`step1/preflight-2026-10-19.md`). None of those strings is checked by any tool on the path. Fabric must check them before it moves anything.

Still, the user must never type a folder or a machine name unless they want to. So some rule must exist.

## Decision

### Placement in this phase: explicit, and every automatic choice says why

There is no placement policy. There are two commands and both name the node:

- `fabric job move <job> --to <node>` on the owner pushes the job to `<node>` by the move protocol of ADR 0003 and does not launch a harness there.
- `fabric resume <job>` on a node that does not own the job pulls it by the same protocol, from the owner, and then launches, because the command that resumes is the command that holds the terminal the harness needs (ADR 0003, Who supervises the harness). `fabric resume` on the owner relaunches and moves nothing.

Nothing picks a node. A warm-cache rule was considered (prefer the node that already holds most of the latest snapshot's chunks) and dropped, because right after a move the warmest node is always the one the user just left. The warm-cache measure survives as information: `fabric status` shows, per paired node, the fraction of the latest snapshot's chunks that node holds, and the owner of every job, so the user can see where a move would be cheap and who holds what before typing anything.

Where Fabric does choose, it prints the reason in the command's output, one line per choice. In this phase the choices are: the workspace root ("same path as the source, `/Users/me/oss/x`", ADR 0011), the transport ("TCP+TLS: QUIC to `fab1...` timed out after 2 s", ADR 0005), each cache carried or rebuilt ("`node_modules` rebuilt: pnpm 11.23.0 on the source, none here", ADR 0012), and each setup step run ("`pnpm install --frozen-lockfile`: `node_modules` missing", ADR 0007). A choice the user cannot see is a choice the user cannot correct, and the research's one documented placement failure is a silent one (T3 #15814, cited in the design input, D19).

The two-Mac gate is a push from the Pro to the Air (the Pro dials, the Air cannot) followed by `fabric resume` on the Air, and a pull from the Pro to bring the job back (ADR 0005, Pairing).

### `fabric doctor`: the preflight

`fabric doctor` runs on one node and prints that node's facts; `fabric doctor --against <node>` compares them with a paired node's and is run by `fabric job move` and by a pulling `fabric resume` before the first byte moves. Every fact is an exact string, compared for equality, never parsed into a version range. The table is the rule:

| Fact | How it is read | On mismatch |
|---|---|---|
| Username and `HOME` | `id -un`, `$HOME` | Refuse. The root would differ and nothing in this phase rewrites paths (ADR 0011). |
| pnpm version and store path | `pnpm --version`, `pnpm store path` | Carry `node_modules` off; the lockfile install runs (ADR 0012). |
| Node version and ABI | `node --version`, `node -p process.versions.modules` | Carry `node_modules` off when the major differs (ADR 0012). |
| rustc | `rustc -vV`, whole output | Carry `target` off; rebuild (ADR 0012). |
| `CARGO_HOME` | `${CARGO_HOME:-$HOME/.cargo}` | Carry `target` off. |
| macOS version and SDK | `sw_vers -productVersion`, `xcrun --show-sdk-version` | Warn. Carry of native build output is already keyed on rustc and Node. |
| Claude Code version | `claude --version` | Refuse when the job's harness is Claude and the versions differ; the floor for the gate is 2.1.300 on both (ADR 0006). Amended 2026-10-27 (F13): refuse only an older target; warn for a newer one. |
| Codex version | `codex --version` | Refuse when the job's harness is Codex and the versions differ. Amended 2026-10-21 (F10): both version rows name an older target in the note; `--force` still waives them, because the resume refuses a harness older than the session's newest writer before it starts (ADR 0006, Settings travel with the session). Amended 2026-10-27 (F13): refuse only an older target; warn for a newer one. |
| Harness login | `claude auth status`, `codex login status`, exit code only | Refuse when the target has no login for the job's harness. Reported as yes or no; the account is never printed. Amended 2026-10-27 (F12): when Claude reports no login, a daemon outside the desktop session also asks `security show-keychain-info` whether it can read the login Keychain, by exit code only (`claude.keychain`, `readable` or `locked`), and a locked one changes the refusal's note: the login is there but a daemon started over SSH cannot see it, so start `fabric serve` from the desktop session, in Terminal or as a launchd user agent. The row still refuses with no waiver. |
| Other logins the job record names | `gh auth status`, `npm whoami`, `vercel whoami`, each only when named | Warn before the harness starts a turn that would push or publish. |
| Clock offset | `sntp -d time.apple.com` on each node, the difference | Refuse above 5 s when any mtime-keyed cache is carried (Cargo, `tsc --build`, turbo); warn otherwise. Measured on 2026-10-19: Pro +0.064 s, Air +0.055 s. |
| Synced folders under the root | `defaults read com.apple.finder FXICloudDriveDesktop FXICloudDriveDocuments FXICloudDriveEnabled`; the root against `~/Library/Mobile Documents`, Dropbox, Google Drive, Syncthing folders | Refuse (ADR 0004, Where a root may live). The Air syncs Desktop and Documents today. |
| Harness supervisors and pickers | `claude agents --json`; held files under `thread-writer-locks/`; whether the ChatGPT desktop app runs | Refuse to move a job whose session is supervised or whose thread lock is held by another process until it is stopped (ADR 0006). |
| Auto-update | `DISABLE_AUTOUPDATER` set; Homebrew pin on `codex` | Warn. A version equal today can differ at the next move without any action by the user. |
| Network | QUIC then TCP+TLS reachability to the peer (ADR 0005) | Refuse when neither path connects; report which one will be used. |

`--force` turns every Refuse above into a Warn except two rows: the login row, because a resume without a login fails inside the harness with a worse message, and the username and `HOME` row, because the root would not exist on the target (ADR 0011). The output lists every row with both values, and the job record stores the comparison with the move, so a later "why did this rebuild" has an answer.

The measured facts of 2026-10-19 mean the first gate fixture must not depend on pnpm or on native Node modules: it is a small npm-installed, pure-JavaScript project. ADR 0012 keys every `node_modules` on the Node major, pure JavaScript included, so across Node 22 and Node 26 the fixture takes the install path, and the same fixture proves the carry path once the Air runs Node 22 (ADR 0012, The first gate fixture). `fabric doctor` must still report every mismatch above, because that is the report the owner will read before the first real job.

### Triggers in this phase

None. `fabric suspend`, `fabric job move` and `fabric resume` are typed by the user or run by a script the user wrote. There is no daemon watching for sleep, lid close, battery or network loss, and no automatic move of any kind.

### Sleep watcher, later

The watcher is the feature the research calls the strongest surviving claim (W2). It is deferred, not dropped, because it needs ADR 0003 proven under crash first and because the time budget is small: about 30 seconds on macOS and 5 seconds by default on Linux (design input, section 7). When it comes:

- macOS: subscribe to `NSWorkspace.willSleepNotification` and `IOKit` power assertions; on lid close or low battery, run `suspend` with a time budget and hold a short power assertion while it runs.
- Linux: `systemd-logind` inhibitor lock (delay type) on `PrepareForSleep`.
- The watcher only ever suspends and pushes the snapshot to paired peers. It never moves the lease by itself: a move needs the target's decision (ADR 0003), and a watcher that started one on lid close would freeze the job on a sleeping source. Resume stays a user action on the other machine, which after a suspend plus push is a pull against a sleeping owner, and therefore a steal with `--force` until the witness ADR exists.

## Alternatives considered

1. **Resource-based auto balance** (T3 Code). Exists, off by default, caused at least one user to run on the wrong machine with no undo (t3code note, section 4). Rejected for this phase; an explicit command with a printed reason is the undo.
2. **Task-need placement** (GPU, VPN). No demand evidence; adds inputs nobody has asked for. Deferred until a GPU node exists.
3. **Follow the user: resume means here, nothing else** (this ADR's first version). Correct as far as it went, but it had no verb for sending a job to a node whose owner is the only one that can dial, which is the gate. Kept as the pull form; the push form was added.
4. **Agent-chosen placement** (expose a `place` tool to the agent). Interesting and cheap once the policy exists; it is a client of the policy, not the policy. Later.
5. **Automatic failover on host loss.** Needs a witness (ADR 0003). Later, with its own ADR.
6. **A preflight that parses versions and allows ranges.** Every mismatch the research found was an exact-string coupling (pnpm's `storeDir` compare, Cargo's `rustc -vV` fingerprint, Claude's slug). Ranges would let through exactly the cases that break. Rejected; equality or a named fallback.

## Consequences

- `move` and `resume` are predictable: the job goes where the user named, and every choice Fabric made on the way is one line in the output.
- Placement code in this phase is `fabric doctor`, the per-node chunk fraction in `fabric status`, and nothing else. Unit tests cover the doctor table row by row with synthetic facts, one test per row, each asserting the refuse, warn or fallback the table names.
- The sleep watcher ships only after the crash tests of ADR 0003 and 0004 pass on the two-Mac gate.

## Open questions

- Time budget for a sleep-triggered suspend on macOS: the system gives little. Measure how long a snapshot of a real workspace takes before promising anything.
- Which doctor rows may be waived per job and recorded in the job record, so that a known and accepted mismatch stops being printed on every move?
- Whether `fabric doctor` should also run the N-API scan of carried `.node` files that would relax the Node major rule (ADR 0012, Open questions).

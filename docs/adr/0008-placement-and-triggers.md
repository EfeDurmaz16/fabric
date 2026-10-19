# ADR 0008: Placement and triggers

Status: accepted, 2026-10-19

## Context

Two findings from the research cut this ADR down to size. First, task-need placement (GPU, VPN, battery, local DB) is documented in no product and asked for in zero issues; T3 Code ships a resource-based "Auto balance" off by default (`2026-10-19-competitor-landscape.md`, section 7, W3). The report's advice is to drop placement from the pitch. Second, the one trigger with real white space is "laptop is about to sleep" (W2), and it is also the hardest to get right, because a move that starts and does not finish is the two-copies bug (ADR 0003).

Still, the user must never type a folder or a machine name unless they want to. So some rule must exist.

## Decision

### Placement v0

`fabric resume <job>` chooses a node with this rule, in order:

1. `--on <node>` if given. Explicit always wins.
2. Otherwise, among paired nodes that are awake (answered a `Hello` within the last 30 seconds): prefer the node with the warmest cache for this job, measured as the fraction of the latest snapshot's chunk ids already in that node's catalog.
3. Tie: the node where the command is being run.
4. No node awake except the local one: the local one.

The workspace path on the chosen node is `<fabric-data>/workspaces/<job-id>/` by default, or a per-node root the user configured once. The user never types it per job.

That is the whole policy. No CPU, memory, battery or cost inputs in v0. Each would be a `PlacementInput` behind one trait, added when a real job needs it.

### Triggers v0

Manual only: `fabric suspend` and `fabric resume`, typed by the user or run by a script. There is no daemon watching for sleep in v0.

### Sleep watcher, later

The watcher is the feature the research calls the strongest surviving claim (W2). It is deferred, not dropped, because it needs ADR 0003 proven under crash first. When it comes:

- macOS: subscribe to `NSWorkspace.willSleepNotification` and `IOKit` power assertions; on lid close or low battery, run `suspend` with a time budget and hold a short power assertion while it runs.
- Linux: `systemd-logind` inhibitor lock (delay type) on `PrepareForSleep`.
- The watcher only ever suspends. It never resumes elsewhere by itself in the first version; resume stays a user action on the other machine. That removes the automatic-claim risk from ADR 0003.

## Alternatives considered

1. **Resource-based auto balance** (T3 Code). Exists, off by default, caused at least one user to run on the wrong machine with no undo (t3code note, section 4). Rejected for v0; warm cache is a better signal for a mover than free CPU.
2. **Task-need placement** (GPU, VPN). No demand evidence; adds inputs nobody has asked for. Deferred until a GPU node exists.
3. **Agent-chosen placement** (expose a `place` tool to the agent). Interesting and cheap once the policy exists; it is a client of the policy, not the policy. Later.
4. **Automatic failover on host loss.** Needs replication before loss and quorum (ADR 0003). Later, with its own ADR.

## Consequences

- The default resume is predictable: it goes where the data already is, or stays local.
- Placement code is small enough that the first version fits in one module with unit tests over synthetic catalogs.
- The sleep watcher ships only after the crash tests of ADR 0003 and 0004 pass on the two-Mac gate.

## Open questions

- Should `resume` without `--on` ever pick a remote node, or should it always mean "here" and `fabric move` mean "there"? Leaning toward `resume` means here and `--on` means there, which makes the rule even smaller. Decide in the CLI stage.
- Time budget for a sleep-triggered suspend on macOS: the system gives little. Measure how long a snapshot of a real workspace takes before promising anything.

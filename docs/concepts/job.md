# Job

A job is the unit of development work: a goal, a workspace, a harness that works in it, and the record of everything that happened to it. The job, not the machine, is what Fabric moves ([ADR 0001](../adr/0001-job-as-unit-of-work.md)).

A job has:

- an id, a ULID such as `01M6Y06V8AZTQNFJ1373219EP3`;
- a goal, in your words;
- a root, the absolute path of its workspace, the same on every node (`~/fabric/jobs/<id>/work` for a job made with `fabric job new`);
- a harness: Claude Code, Codex, or an ACP agent (`acp:<program>`);
- a [ledger](ledger.md) of its events and the [snapshots](snapshot.md) its suspends made;
- an `EnvSpec`: the toolchain pins, setup steps and cache paths that make a node able to run it, never secret values ([ADR 0007](../adr/0007-environment-spec.md));
- a lease: which [node](node.md) may run it, at which epoch.

There are two kinds. A **standalone job**, made with `fabric job new`, is decided by the node that holds its lease, and moves by the cooperative move protocol. This is the kind that does work today. A [managed job](managed-job.md), made with `fabric managed create`, is decided by an [authority](authority.md) and its journal instead. The two kinds never convert into each other.

Commands: `fabric job new`, `run`, `suspend`, `move`, `resume`, `status`, `ls`, `log`, `approve`, `reject`, `steal`, `repair`, `export` ([CLI reference](../reference/cli.md)).

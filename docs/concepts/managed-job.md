# Managed job

A managed job is a job whose decisions are made by the [authority](authority.md) and its journal rather than by the node that owns a ledger ([ADR 0019](../adr/0019-managed-job-task-attempt.md)). Its commands carry operation keys, its progress is a work revision that starts at 1, and its work is split into tasks, each run by [attempts](attempt.md).

What works on `main`:

```sh
fabric managed create --repo <a git repository> --goal "..." --harness codex --operation-key k1
fabric managed status <job>
fabric managed cancel <job> --reason "done" --operation-key k2
fabric managed ls
```

`create` with a key it already answered prints the same job again. `status` shows the revision, the state, the generation, the primary task and its attempts.

What does not work on `main`: nothing runs a managed job. `create` records the job and its primary task, which stays `open`; no command a user or a client can send starts an attempt, and no harness is launched. The launch path is [roadmap](../roadmap.md) PR 1. Use standalone jobs for real work today.

The journal schema, the operation states and the replay rules are in [Managed jobs and the journal](../managed-jobs.md). The guide is [Managed jobs](../guides/managed-jobs.md).

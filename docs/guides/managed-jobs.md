# Managed jobs

A [managed job](../concepts/managed-job.md) is recorded and decided by the node's [authority](../concepts/authority.md) and its journal. Every command takes an operation key, and the same key asks the same command again, so a client that lost an answer can repeat the command safely.

Read this first: on `main`, nothing runs a managed job. You can create, read, list and cancel one; no attempt starts and no harness launches. Use a standalone job (`fabric job new`) for real work. The launch path is [roadmap](../roadmap.md) PR 1.

```sh
fabric managed create --repo <a git repository> --goal "managed: say hello" --harness codex --operation-key k1
fabric managed create --repo <a git repository> --goal "managed: say hello" --harness codex --operation-key k1   # the same answer
fabric managed status <job>
fabric managed cancel <job> --reason "done" --operation-key k2
fabric managed ls
```

Rules that hold:

- An operation key is 1 to 128 characters of `A-Z a-z 0-9 . _ : -`. A ULID is a good key.
- The same key with the same command gets the stored answer and no second effect. The same key with a changed body is refused.
- `cancel` takes `--expected-revision` to refuse a cancel when the job changed since you read it.
- `status` prints the revision, the state, the generation, and each task with its attempts.

The same commands are on the client API (`managed_create`, `managed_cancel`, `managed_status`, `managed_list`) and on the TypeScript SDK (`managedCreate`, `managedCancel`, `managedStatus`, `managedList`).

The journal's schema, operation states and replay rules, and the provider bridge that managed attempts will use, are in [Managed jobs and the journal](../managed-jobs.md) and [the provider bridge](../bridge-protocol.md).

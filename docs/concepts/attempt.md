# Attempt

An attempt is one execution of one task of a [managed job](managed-job.md) on one resource, under one generation. A retry is a new attempt. An attempt that disappears is `lost`, never `cancelled`, because nobody saw it stop ([ADR 0019](../adr/0019-managed-job-task-attempt.md)).

Each attempt runs under a grant from the [authority](authority.md): the job, its generation, the attempt, the executor and the scope of the workspace. Exactly one grant is current per job. The generation goes up by one every time the authority grants a new attempt, which is what makes an older attempt's reports stale; see [fence](fence.md).

An effect an attempt asks a provider for, such as creating a sandbox through the bridge to a Capsule worker, is a provider operation. The journal records it `dispatched` before the request is written, and records `unknown`, never failed, when the answer is lost. A new attempt is refused while an effect of an earlier one may still act (the replacement barrier, [ADR 0022](../adr/0022-reconciliation-and-cleanup-authorization.md)).

On `main` the journal models attempts, generations and provider operations, and its tests drive them. No command on the CLI or the client API starts an attempt: placement that grants them is M5 (PR #38, draft), and the path that launches a granted attempt is roadmap PR 1.

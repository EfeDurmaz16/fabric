# Residuals

A residual is something crashed or ended work left on a node: a harness capture cut short, a restore that did not finish, a harness group a fenced tenure left running, a launch-path copy of an attempt that ended, or a provider resource an ended attempt made through the bridge. `fabric gc` lists them; `fabric gc --yes` removes exactly what it listed ([ADR 0030](../adr/0030-residuals-and-gc.md)).

**The live set is never touched.** It is the tenures the node owns, the jobs a move or an operation holds now, the managed attempts the journal holds `granted`, `running` or `unknown` (at any generation), and the attempts the node's runner runs now. The journal's part and the provider creates are read in one call, so they are of one state.

**Labels say whose a residual is.** A restore writes a `fabric-label` file into its staging directory before any tree:

```text
fabric staging label v1
job <job id>
node <node id>
epoch <epoch>
```

A staging directory whose label names this node and an epoch it no longer owns is a residual; one with no label gc reads, or a label naming another node, is kept. A launch-path file is labelled by its attempt, and is a residual only when the journal on this node holds that attempt ended; an attempt the journal here does not hold, as one run for another authority, is kept.

**Provider resources need a fresh approval.** A resource an ended attempt made is removed only through the provider worker named in the command and only with an `--approve <OPERATION>=<APPROVAL>` given for that resource in that run. Neither the CLI nor the node stores an approval, so a second run asks again.

**What gc cannot classify, it keeps.** Anything else under `<state>/jobs` or `<state>/runs` is reported with a reason and left alone.

<!-- residuals:begin -->
| Kind | What | Where | Label | Owner | When it is garbage |
|---|---|---|---|---|---|
| `harness_state_next` | A harness capture cut short. | `<state>/jobs/<job>/harness-state.next` | the job; the epoch of its marker | the suspend or move that captures the job's harness state | always once found under the job's lock: a capture writes it and renames it into place under that lock |
| `harness_state_old` | The harness state a capture set aside. | `<state>/jobs/<job>/harness-state.old` | the job; the epoch of its marker | the capture that put a newer state in place | once `harness-state` is in place; kept as `only_copy` when it is the only state the job has |
| `restore_staging` | A restore that did not finish. | `<target>.fabric-restoring` beside a job's root or its harness state | its `fabric-label` file: the job, the node and the epoch the restore was for | the resume of that tenure, which finishes it | when the label names this node and an epoch it no longer owns |
| `harness_group` | A harness process group a standalone tenure left running. | the group named by `<state>/jobs/<job>/harness` | the job and the epoch the record names | the tenure that launched it | when this node no longer owns that epoch; removing it stops the group |
| `harness_record` | The record of a harness group whose processes are gone. | `<state>/jobs/<job>/harness` | the job and the epoch the record names | the tenure that launched the harness | when this node no longer owns that epoch |
| `launch_record` | A launch-path task's record whose processes are gone. | `<state>/runs/records/<attempt>/<task>.json` | the attempt, and its job, task and generation as the journal holds them | the executor running the attempt | when the journal holds the attempt ended and no runner here runs it |
| `launch_group` | A launch-path task's process group that still runs. | the group named by `<state>/runs/records/<attempt>/<task>.json` | as for its record | the executor running the attempt | as for its record; removing it stops the group, then the record |
| `restored_state` | A copy of a cut's harness state restored for an attempt. | `<state>/runs/restored/<attempt>` | the attempt, and its job, task and generation as the journal holds them | the launch that prepares the attempt; a failed preparation leaves it | when the journal holds the attempt ended and no runner here runs it |
| `captured_state` | A harness state an attempt's checkpoint captured. | `<state>/runs/harness-states/<attempt>` | as for a restored state | the checkpoint of the attempt, until its cut is sent | when the journal holds the attempt ended and no runner here runs it |
| `work_package` | The work package of a continuation. | `<home>/fabric/jobs/<job>/package-<attempt>.json` | the job and the attempt, and its task and generation as the journal holds them | the attempt the package was written for | when the journal holds the attempt ended and no runner here runs it |
| `taken_record` | The runner's record of a launch it took. | `<state>/runs/taken/<attempt>.json` | the grant it holds: the job, the attempt and the generation | the runner: it answers a launch sent again `already_launched` | never: removing it would let a launch sent again run twice; kept as `preserved` |
| `provider_resource` | A provider resource a managed attempt made through the bridge. | the provider, at the locator the journal holds for its create | the job, task, attempt and generation the create was dispatched for | that attempt | when that attempt may no longer run; removed only through the worker, with a fresh approval for that resource |
| `departed_copy` | A departed copy. | `<root parent>/departed/<epoch>` | the job and the epoch in its name | the user (ADR 0003) | never by gc: kept as `preserved` |
| `orphan_copy` | An orphan copy. | `<root parent>/orphan/<epoch>` | the job and the epoch in its name | the user (ADR 0003) | never by gc: kept as `preserved` |
| `unknown` | Something under a Fabric directory gc cannot name. | under `<state>/jobs` or `<state>/runs` | none gc can read | unknown | never by gc: reported and kept |

Why gc keeps what it keeps, the `reason` of each kept item:

- `preserved`: Kept by design: a departed or an orphan copy may hold work only the user can judge, and a taken record fences a launch sent again.
- `busy`: An operation runs on the job now; run gc again once it ends.
- `moving`: The job is moving: this node waits for its target's decision.
- `only_copy`: The only harness state the job has; the next capture puts it back in place.
- `unlabeled`: No label gc can read, or one that does not name this job.
- `other_node`: A label naming another node.
- `unknown_job`: A job this node does not hold.
- `unknown_attempt`: An attempt the journal on this node does not hold, as one run for another authority: gc cannot tell whether it may still run.
- `unlocated`: A provider create with no locator: attach it first.
<!-- residuals:end -->

# Fabric job client

Connect a trusted Node.js 22+ backend to the private socket of a local Fabric node.
The node routes requests to the job's authority, which checks the facade token.
The application holding the socket has the local OS user's privileges. Run the
client in that trusted process and keep the socket and bearer within that boundary.

To try a checkout, run `bun install --frozen-lockfile` and `npm pack` in this
directory. Install the resulting `fabric-jobs-client-0.1.0.tgz` from your own
Node project using `npm install /absolute/path/to/fabric-jobs-client-0.1.0.tgz`.

```js
import { FabricJobs, JobId, NodeId } from "@fabric-jobs/client";

export async function observe({ socketPath, authority, token, jobId, cursor }) {
  const jobs = new FabricJobs({ socketPath, authority: NodeId.parse(authority), token });
  const job = JobId.parse(jobId);
  const inspection = await jobs.inspect(job);
  const page = await jobs.watch(job, { after: cursor, waitMs: 1000 });
  return { inspection, events: page.events, cursor: page.next, closed: page.closed };
}
```

Start with a null cursor. Apply a page and persist its `next` cursor together.
A reconnect reads from the saved cursor. A watch is bounded and can return an
empty page. Check `closed` before asking for another page.

| Method | Result |
| --- | --- |
| `jobs.submit(spec, { key })` | A job ID, operation ID, revision and replay flag |
| `jobs.inspect(job)` | The authority's state for the managed job |
| `jobs.watch(job, { after, waitMs })` | A bounded page of events and its next cursor |
| `jobs.checkpoint(job, { key, expectedRevision })` | Acceptance of the checkpoint request |
| `jobs.move(job, { key, pin, fidelity, expectedRevision })` | Acceptance of a facade continue request |
| `jobs.cancel(job, { key, reason, expectedRevision })` | Acceptance of cancellation for the job or an optional task |

Create an operation key with `newOperationKey()` and save it with the command
before the first send. Retry an uncertain call using that key and the original
inputs. Checkpoints and moves can keep running after acceptance. Inspect the
outcome, or follow the watch. A `FacadeRefusal` carries the stable
`refusal.code` and `refusal.reason`, plus `refusal.current_revision` for a
revision mismatch. Counters travel as decimal strings.

The lower-level `FabricClient` exposes node-owner administration and raw
`facadeCall` for existing callers. See the [facade wire contract](https://github.com/EfeDurmaz16/fabric/blob/main/docs/facade-protocol.md).

## Six-method smoke test

Start a disposable Fabric authority and mint a short-lived token for the six
facade operations. Leave the test secret named `FABRIC_SDK_MISSING_TEST_SECRET`
absent on that authority. From the clean consumer project run:

```sh
node node_modules/@fabric-jobs/client/examples/jobs-smoke.mjs "$socket_path" "$authority" < "$bearer_file"
```

The script submits and inspects a parked job, exercises watch, checkpoint and
move, then cancels it. It checks that the checkpoint is refused because the
parked job has no live attempt. The move assertion checks acceptance by the
authority. Store the token file with owner-only permissions.

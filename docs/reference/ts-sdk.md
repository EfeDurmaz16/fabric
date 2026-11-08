# TypeScript SDK

`sdk/typescript` is a TypeScript client of the [client API](client-api.md) for Node 22 and Bun. It is private and not published to npm; build it from the repository. Its only runtime dependency is zod, which checks every message at the boundary.

## Build and test

```sh
cargo build -p fabric --locked
cd sdk/typescript
bun install --frozen-lockfile
bun run typecheck
bun run build                                     # JavaScript and declarations in dist/
FABRIC_BIN="$PWD/../../target/debug/fabric" bun test
```

The tests run the same contract test as the Rust client against `protocol/fixtures`, an end-to-end test against a real node, and a test that imports the packed package with plain Node.

## Use

```ts
import { FabricClient } from "./dist/index.js";

const client = new FabricClient({ socketPath: `${process.env.FABRIC_HOME}/fabric.sock` });
const jobs = await client.jobList();
```

`socketPath` is required; the SDK does not read `FABRIC_HOME` itself.

Methods on `main`: `hello`, `nodeId`, `nodeAdd`, `nodeList`, `nodePing`, `jobNew`, `jobList`, `jobStatus`, `jobLog`, `jobRun`, `jobResume`, `jobSuspend`, `jobMove`, `jobSteal`, `jobDecide`, `jobRepair`, `jobExport`, `doctor`, `launchCheck`, `harnessStarted`, `harnessSession`, `harnessExited`, `managedCreate`, `managedCancel`, `managedStatus` and `managedList`, plus `call`, which sends any typed request. A call takes `onEvent` to see progress events as they arrive. A refusal rejects with a typed error (`RefusedError`, `NoDaemonError`, `IncompatibleError` and the rest in `src/errors.ts`).

Limits today: `jobRun` and `jobResume` return a run plan, and the caller launches the harness and reports it. There is no single call that runs a turn and returns the answer.

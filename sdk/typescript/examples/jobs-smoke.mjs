// Run from a project with the packed client installed. Supply a disposable,
// trusted node's socket and authority id as arguments and a short-lived token
// on stdin. Keep this fixture's required secret absent so placement parks.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { FabricJobs, FacadeRefusal, NodeId, newOperationKey } from "@fabric-jobs/client";

const [socketPath, authorityId] = process.argv.slice(2);
if (!socketPath || !authorityId) {
  throw new Error("Usage: node jobs-smoke.mjs <socket-path> <authority-id> (bearer on stdin)");
}
const authority = NodeId.parse(authorityId);
const token = readFileSync(0, "utf8").trim();
const jobs = new FabricJobs({ socketPath, authority, token });
const submit = {
  goal: "External client package smoke test",
  harness: { kind: "codex" },
  acceptance: [],
  source: { kind: "empty" },
  policy: { pin: null, secrets: [{ name: "FABRIC_SDK_MISSING_TEST_SECRET", attempt: "any", resource_kinds: ["node"] }] },
};
const key = newOperationKey();
const accepted = await jobs.submit(submit, { key });
const job = accepted.job;
try {
  const replay = await jobs.submit(submit, { key });
  assert.equal(replay.operation, accepted.operation);
  assert.equal(replay.replayed, true);
  assert.equal((await jobs.inspect(job)).job.job, job);
  const page = await jobs.watch(job, { waitMs: 0 });
  assert.ok(page.events.length > 0);
  await assert.rejects(
    jobs.checkpoint(job, { key: newOperationKey() }),
    (error) => error instanceof FacadeRefusal && error.refusal.code === "refused" && error.refusal.reason === "not_live",
  );
  const move = await jobs.move(job, { key: newOperationKey(), pin: authority });
  assert.equal(move.job, job);
  const next = await jobs.watch(job, { after: page.next, waitMs: 0 });
  assert.ok(next.events.length > 0);
  console.log(JSON.stringify({ job, submitReplayed: true, checkpoint: "not_live", move: move.operation, cursor: next.next }));
} finally {
  const stopped = await jobs.cancel(job, { key: newOperationKey(), reason: "smoke test complete" });
  assert.equal(stopped.job, job);
}

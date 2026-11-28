// Executed from the temporary project that installed the npm tarball.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { FabricJobs, FacadeRefusal } from "@fabric-jobs/client";

const { endpoint, ca, token, action, spec, job, cursor } = JSON.parse(readFileSync(0, "utf8"));
const jobs = new FabricJobs({ endpoint, token, ca: readFileSync(ca) });

async function consume() {
  switch (action) {
    case "retry":
      return jobs.submit(spec, { key: "https-submit" });
    case "read":
      assert.equal((await jobs.inspect(job)).job.job, job);
      await assert.rejects(jobs.cancel(job, { key: "denied", reason: "read-only" }),
        (e) => e instanceof FacadeRefusal && e.refusal.code === "scope" && e.refusal.reason === "operation");
      return jobs.watch(job, { after: cursor, waitMs: 0 });
    case "move":
      await assert.rejects(jobs.checkpoint(job, { key: "https-checkpoint" }),
        (e) => e instanceof FacadeRefusal && e.refusal.code === "refused" && e.refusal.reason === "not_live");
      return jobs.move(job, { key: "https-move", pin: spec.policy.pin });
    case "cancel":
      return jobs.cancel(job, { key: "https-cancel", reason: "fixture cleanup" });
    default:
      throw new Error("unknown smoke test action");
  }
}

console.log(JSON.stringify(await consume()));

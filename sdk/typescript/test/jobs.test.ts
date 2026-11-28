import { afterEach, expect, spyOn, test } from "bun:test";
import { FabricClient, FabricJobs, Counter, FacadeRefusal, JobId, NodeId, OperationId, OperationKey, ProtocolViolation, type JobSpec } from "../src/index.ts";

const authority = NodeId.parse("fab1aeaqcaibaeaqcaibaeaqcaibaeaqcaibaeaqcaibaeaqcaibaeaq");
const job = JobId.parse("01JABCDEFGHJKMNPQRSTVWXYZ0");
const key = OperationKey.parse("sdk:stable-key");
const revision = Counter.parse("9007199254740993");
const operation = OperationId.parse("01JABCDEFGHJKMNPQRSTVWXYZ1");
const token = "a".repeat(64);
const client = new FabricJobs({ socketPath: "/nowhere/fabric.sock", authority, token });
const accepted = { type: "accepted", job, operation, key, revision, replayed: true } as const;
const call = () => spyOn(FabricClient.prototype, "facadeCall");

afterEach(() => {
  call().mockRestore();
});

test("mutations bind the caller's key and forward exact wire options", async () => {
  const transport = call().mockResolvedValue(accepted);
  const submit: JobSpec = { goal: "build", acceptance: [], source: { kind: "empty" }, harness: { kind: "codex" } };
  expect(await client.submit(submit, { key })).toEqual(accepted);
  expect(await client.checkpoint(job, { key, expectedRevision: revision })).toEqual(accepted);
  expect(await client.move(job, { key, pin: authority, fidelity: ["native_session"] })).toEqual(accepted);
  expect(await client.cancel(job, { key, reason: "done" })).toEqual(accepted);
  expect(transport.mock.calls).toEqual([
    [authority, { type: "submit", token, key, submit }],
    [authority, { type: "checkpoint", token, key, job, expected_revision: revision }],
    [authority, { type: "continue", token, key, job, expected_revision: null, pin: authority, fidelity: ["native_session"] }],
    [authority, { type: "cancel", token, key, job, expected_revision: null, task: null, reason: "done" }],
  ]);
});

test("watch preserves decimal cursors through empty pages and defaults to the start", async () => {
  const transport = call().mockResolvedValue({ type: "events", job, next: revision, events: [], closed: null, lagged: false });
  expect((await client.watch(job, { after: revision, waitMs: 0 })).next).toBe(revision);
  expect(transport).toHaveBeenCalledWith(authority, { type: "watch", job, token, after: revision, wait_ms: 0 });
  await client.watch(job);
  expect(transport).toHaveBeenCalledWith(authority, { type: "watch", job, token, after: null });
});

test("a facade refusal preserves the authority's reason and current revision", async () => {
  const refusal = { code: "revision_mismatch", reason: "revision", message: "revision changed", current_revision: revision } as const;
  call().mockResolvedValue({ type: "refused", refusal });
  expect.assertions(2);
  try {
    await client.checkpoint(job, { key });
  } catch (error) {
    expect(error).toBeInstanceOf(FacadeRefusal);
    expect((error as FacadeRefusal).refusal).toEqual(refusal);
  }
});

test("responses must identify the requested job, effect and expected response type", async () => {
  const otherJob = JobId.parse("01JABCDEFGHJKMNPQRSTVWXYZ2");
  const transport = call().mockResolvedValue({ ...accepted, job: otherJob });
  await expect(client.cancel(job, { key, reason: "done" })).rejects.toBeInstanceOf(ProtocolViolation);
  transport.mockResolvedValue({ ...accepted, key: OperationKey.parse("another-key") });
  await expect(client.checkpoint(job, { key })).rejects.toBeInstanceOf(ProtocolViolation);
  await expect(client.inspect(job)).rejects.toBeInstanceOf(ProtocolViolation);
  transport.mockResolvedValue({ type: "events", job: otherJob, events: [], next: null, lagged: false, closed: null });
  await expect(client.watch(job)).rejects.toBeInstanceOf(ProtocolViolation);
});

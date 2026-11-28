import { FabricClient, type ClientOptions } from "./client.ts";
import {
  FacadeRefusal, ProtocolViolation, type ClientMessage, type FacadeAnswer, type Submit,
} from "./facade.ts";
import type { Counter, Fidelity, Inspection, JobId, ManagedSource, NodeId, OperationKey, TaskId } from "./schema.ts";

import { gatewayCaller, type GatewayOptions } from "./gateway.ts";

export type JobClientOptions =
  | (ClientOptions & { readonly authority: NodeId; readonly token: string; readonly endpoint?: never })
  | (GatewayOptions & { readonly authority?: never; readonly socketPath?: never });
/** Remote facade submissions accept a snapshot, pinned Git commit or empty workspace. */
export type JobSpec = Omit<Submit, "source"> & { source: Extract<ManagedSource, { kind: "git" | "snapshot" | "empty" }> };
export type AcceptedOperation = Extract<FacadeAnswer, { type: "accepted" }>;
export type JobEvents = Extract<FacadeAnswer, { type: "events" }>;

type Change = { key: OperationKey; expectedRevision?: Counter };
type Effect = Extract<ClientMessage, { key: OperationKey }>;

function expectType<T extends "accepted" | "events" | "inspection">(
  answer: FacadeAnswer,
  type: T,
): asserts answer is Extract<FacadeAnswer, { type: T }> {
  if (answer.type === "refused") {
    const { code, reason, message, current_revision } = answer.refusal;
    throw new FacadeRefusal(code, reason, message, current_revision);
  }
  if (answer.type !== type) throw new ProtocolViolation("unexpected_answer", `expected ${type}, received ${answer.type}`);
}

/** Six operations on a managed authority. Connect through a private node socket or an HTTPS gateway. */
export class FabricJobs {
  readonly #call: (message: ClientMessage) => Promise<FacadeAnswer>;
  readonly #token: string;

  constructor(options: JobClientOptions) {
    this.#token = options.token;
    if (options.endpoint !== undefined) {
      if (options.socketPath !== undefined || options.authority !== undefined) {
        throw new TypeError("the gateway owns its authority and socket path");
      }
      this.#call = gatewayCaller(options);
    } else {
      const client = new FabricClient(options);
      const authority = options.authority;
      this.#call = (message) => client.facadeCall(authority, message);
    }
  }

  /** Persist the key before submitting. Reuse it with the identical spec after an uncertain reply. */
  submit(spec: JobSpec, options: { key: OperationKey }): Promise<AcceptedOperation> {
    return this.#effect({ type: "submit", token: this.#token, key: options.key, submit: spec });
  }

  async inspect(job: JobId): Promise<Inspection> {
    const answer = await this.#call({ type: "inspect", token: this.#token, job });
    expectType(answer, "inspection");
    if (answer.inspection.job.job !== job) throw new ProtocolViolation("job_mismatch", "inspection names a different job");
    return answer.inspection;
  }

  /** Read one bounded page. Persist next after applying the events to resume after disconnect. */
  async watch(job: JobId, options: { after?: Counter | null; waitMs?: number } = {}): Promise<JobEvents> {
    const answer = await this.#call({
      type: "watch", token: this.#token, job, after: options.after ?? null,
      ...(options.waitMs === undefined ? {} : { wait_ms: options.waitMs }),
    });
    expectType(answer, "events");
    if (answer.job !== job) throw new ProtocolViolation("job_mismatch", "events name a different job");
    let cursor = options.after ?? null;
    for (const event of answer.events) {
      if (cursor !== null && BigInt(event.seq) <= BigInt(cursor)) {
        throw new ProtocolViolation("bad_cursor", "watch event sequence did not advance");
      }
      cursor = event.seq;
    }
    if (answer.next !== cursor) throw new ProtocolViolation("bad_cursor", "watch cursor does not match the delivered events");
    return answer;
  }

  checkpoint(job: JobId, options: Change): Promise<AcceptedOperation> {
    return this.#effect({
      type: "checkpoint", token: this.#token, job, key: options.key, expected_revision: options.expectedRevision ?? null,
    });
  }

  move(job: JobId, options: Change & { pin?: NodeId; fidelity?: Fidelity[] }): Promise<AcceptedOperation> {
    return this.#effect({
      type: "continue", token: this.#token, job, key: options.key, expected_revision: options.expectedRevision ?? null,
      pin: options.pin ?? null, ...(options.fidelity === undefined ? {} : { fidelity: options.fidelity }),
    });
  }

  cancel(job: JobId, options: Change & { reason: string; task?: TaskId }): Promise<AcceptedOperation> {
    return this.#effect({
      type: "cancel", token: this.#token, job, key: options.key, expected_revision: options.expectedRevision ?? null,
      reason: options.reason, task: options.task ?? null,
    });
  }

  async #effect(message: Effect): Promise<AcceptedOperation> {
    const answer = await this.#call(message);
    expectType(answer, "accepted");
    if (answer.key !== message.key || ("job" in message && answer.job !== message.job)) {
      throw new ProtocolViolation("operation_mismatch", "acceptance names a different operation key or job");
    }
    return answer;
  }
}

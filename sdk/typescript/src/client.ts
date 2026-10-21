// A client of one local node, mirroring crates/fabric-client/src/client.rs.
//
// Nothing is sent before the state directory and the socket prove to be
// this user's alone, because a run plan read from the socket decides what
// the caller launches (ADR 0016). Every call is one connection: a hello,
// the node's welcome, one request, the node's events as they happen and
// one final answer, whose kind must be the request's op.

import type { Stats } from "node:fs";
import { lstat } from "node:fs/promises";
import { createConnection, type Socket } from "node:net";
import { dirname, resolve } from "node:path";
import type { z } from "zod";

import {
  ClosedBeforeAnswerError,
  ClosedBeforeWelcomeError,
  FabricError,
  FrameIoError,
  IncompatibleError,
  InvalidRequestError,
  MalformedError,
  NoDaemonError,
  RefusedError,
  UnexpectedError,
  UnsupportedError,
  UnsupportedFieldError,
  UntrustedError,
} from "./errors.ts";
import { createFrameDecoder, decodeJson, encodeFrame } from "./frame.ts";
import {
  API_VERSION,
  Answer,
  capabilitiesNeeded,
  type ClientEnv,
  type Decision,
  type DoctorReport,
  type Epoch,
  type Harness,
  Hello,
  HelloAnswer,
  isReplyTo,
  type JobCreated,
  type JobId,
  type JobList,
  type JobLog,
  type JobStatus,
  type KnownEvent,
  type Lease,
  type NodeId,
  type NodeIdentity,
  type Paired,
  type PeerList,
  type Pid,
  type Pong,
  type Registered,
  type ReplyTo,
  Request,
  type RunPlan,
  type SessionId,
  type SessionRecorded,
  type Suspended,
  type SystemPath,
  type Welcome,
} from "./schema.ts";

/** This SDK's version, which it names in its hello. */
export const SDK_VERSION = "0.1.0";

export type ClientOptions = {
  /** The node's control socket, `<state>/fabric.sock`. */
  readonly socketPath: string;
  /** The name and version the hello carries, for the node's log. */
  readonly name?: string;
};

export type CallOptions = {
  /**
   * Called with each event as it arrives, in order. An event of a kind
   * this client does not know is skipped. If it throws, the call rejects
   * with that error; the node still finishes the command.
   */
  readonly onEvent?: (event: KnownEvent) => void;
};

/** A harness the caller launched from a run plan. */
export type Launched = {
  readonly job: JobId;
  /** The epoch the plan was made under. */
  readonly epoch: Epoch;
  /** The harness process, which leads its own process group. */
  readonly pid: Pid;
  readonly harness: Harness;
};

/** A client of the node whose control socket is at a path. */
export class FabricClient {
  readonly socketPath: string;
  readonly name: string;

  constructor(options: ClientOptions) {
    this.socketPath = options.socketPath;
    this.name = options.name ?? `fabric-sdk-typescript/${SDK_VERSION}`;
  }

  /** Connects, checks the api version and returns what the node says about itself. */
  async hello(): Promise<Welcome> {
    const { exchange, welcome } = await this.connect();
    exchange.close();
    return welcome;
  }

  /**
   * Sends `request` and returns its reply, handing each event to
   * `onEvent` as it arrives. The request is validated first, so one that
   * does not parse never leaves the client.
   */
  async call<R extends Request>(request: R, options: CallOptions = {}): Promise<ReplyTo<R["op"]>> {
    const checked = Request.safeParse(request);
    if (!checked.success) {
      throw new InvalidRequestError(checked.error.message, checked.error);
    }
    const op: R["op"] = request.op;
    const { exchange, welcome } = await this.connect();
    try {
      if (!welcome.capabilities.includes(op)) {
        throw new UnsupportedError(op);
      }
      const unlisted = capabilitiesNeeded(checked.data).find((name) => !welcome.capabilities.includes(name));
      if (unlisted !== undefined) {
        throw new UnsupportedFieldError(op, unlisted);
      }
      await exchange.send(checked.data);
      for (;;) {
        const answer = await exchange.receive(Answer);
        if (answer === null) {
          throw new ClosedBeforeAnswerError(op);
        }
        switch (answer.type) {
          case "event":
            if (answer.event.kind !== "unrecognized") {
              options.onEvent?.(answer.event);
            }
            break;
          case "done": {
            const got = answer.reply.kind;
            if (isReplyTo(answer.reply, op)) {
              return answer.reply;
            }
            throw new UnexpectedError(op, got);
          }
          case "refused":
            throw new RefusedError(answer.refusal);
        }
      }
    } finally {
      exchange.close();
    }
  }

  /** This node's id and listening address. */
  nodeId(options?: CallOptions): Promise<NodeIdentity> {
    return this.call({ op: "node_id" }, options);
  }

  /** Pairs a peer, at `addr` (`host:port`) when this node can dial it. */
  nodeAdd(node: NodeId, options: CallOptions & { readonly addr?: string } = {}): Promise<Paired> {
    return this.call({ op: "node_add", node, addr: options.addr }, options);
  }

  /** The paired peers. */
  nodeList(options?: CallOptions): Promise<PeerList> {
    return this.call({ op: "node_list" }, options);
  }

  /** Dials a peer and pings it. */
  nodePing(node: NodeId, options?: CallOptions): Promise<Pong> {
    return this.call({ op: "node_ping", node }, options);
  }

  /**
   * Makes a job from the repository at `repo`, an absolute path on the
   * node; a relative one never leaves the client, since the node would
   * read it against its own working directory.
   */
  jobNew(repo: SystemPath, goal: string, harness: Harness, options?: CallOptions): Promise<JobCreated> {
    return this.call({ op: "job_new", repo, goal, harness }, options);
  }

  /** Every job the node knows. */
  jobList(options?: CallOptions): Promise<JobList> {
    return this.call({ op: "job_list" }, options);
  }

  /** One job's state on the node. */
  jobStatus(job: JobId, options?: CallOptions): Promise<JobStatus> {
    return this.call({ op: "job_status", job }, options);
  }

  /** One job's ledger. */
  jobLog(job: JobId, options?: CallOptions): Promise<JobLog> {
    return this.call({ op: "job_log", job }, options);
  }

  /**
   * Checks the job may run on the node and returns what to launch. `env`
   * is the environment the caller launches the harness in, such as
   * `clientEnvFromProcess()`; the node keeps it for work no client asks
   * for, such as a peer's pull of the job.
   */
  jobRun(job: JobId, options: CallOptions & { readonly env?: ClientEnv } = {}): Promise<RunPlan> {
    return this.call({ op: "job_run", job, env: options.env }, options);
  }

  /** Pulls the job if another node owns it, restores it, and returns what to launch. */
  jobResume(
    job: JobId,
    options: CallOptions & {
      /** The node that holds the job, when this node has never seen it. */
      readonly on?: NodeId;
      /** A harness to use instead of the job's. */
      readonly harness?: Harness;
      /** Waive the preflight refusals that can be waived. */
      readonly force?: boolean;
      /** The caller's environment, such as `clientEnvFromProcess()`; the node's own when absent. */
      readonly env?: ClientEnv;
    } = {},
  ): Promise<RunPlan> {
    const { on, harness, force = false, env } = options;
    return this.call({ op: "job_resume", job, on, harness, force, env }, options);
  }

  /**
   * Stops the harness, snapshots the workspace and records the suspend,
   * measuring with `env`, the caller's environment, or the node's own.
   */
  jobSuspend(job: JobId, options: CallOptions & { readonly env?: ClientEnv } = {}): Promise<Suspended> {
    return this.call({ op: "job_suspend", job, env: options.env }, options);
  }

  /** Moves the job to the peer `to` and returns the lease once the move is decided. */
  jobMove(
    job: JobId,
    to: NodeId,
    options: CallOptions & { readonly force?: boolean; readonly env?: ClientEnv } = {},
  ): Promise<Lease> {
    const { force = false, env } = options;
    return this.call({ op: "job_move", job, to, force, env }, options);
  }

  /** Takes the job without its owner's consent; without `force` the node refuses with the cost. */
  jobSteal(
    job: JobId,
    options: CallOptions & { readonly reason?: string; readonly force?: boolean } = {},
  ): Promise<Lease> {
    const { reason = "", force = false } = options;
    return this.call({ op: "job_steal", job, reason, force }, options);
  }

  /** Records an approval or a rejection, with the user's note, and returns the recorded text. */
  async jobDecide(job: JobId, decision: Decision, note: string, options?: CallOptions): Promise<string> {
    const reply = await this.call({ op: "job_decide", job, decision, note }, options);
    return reply.text;
  }

  /** The node's facts, or, with `peer`, their comparison with the peer's, measured with `env` or the node's own. */
  async doctor(
    options: CallOptions & { readonly peer?: NodeId; readonly job?: JobId; readonly env?: ClientEnv } = {},
  ): Promise<DoctorReport> {
    const { peer, job, env } = options;
    const reply = await this.call({ op: "doctor", peer, job, env }, options);
    return reply.report;
  }

  /**
   * Tells the node the planned harness runs as `launch.pid`, so it records
   * it and appends `Resumed`; `session` is the session it continues or
   * created, when known.
   */
  harnessStarted(launch: Launched, session?: SessionId, options?: CallOptions): Promise<Registered> {
    const { job, epoch, pid, harness } = launch;
    return this.call({ op: "harness_started", job, epoch, pid, harness, session }, options);
  }

  /** Tells the node the running harness named `session`. */
  harnessSession(launch: Launched, session: SessionId, options?: CallOptions): Promise<SessionRecorded> {
    const { job, epoch, pid, harness } = launch;
    return this.call({ op: "harness_session", job, epoch, pid, harness, session }, options);
  }

  /** Tells the node the harness `pid` of `job` exited. */
  async harnessExited(job: JobId, pid: Pid, options?: CallOptions): Promise<void> {
    await this.call({ op: "harness_exited", job, pid }, options);
  }

  private async connect(): Promise<{ exchange: Exchange; welcome: Welcome }> {
    const socket = resolve(this.socketPath);
    await checkPrivate(dirname(socket));
    await checkPrivate(socket);
    const exchange = openExchange(await dial(this.socketPath));
    try {
      let answer: HelloAnswer | null;
      try {
        await exchange.send(Hello.parse({ api: API_VERSION, client: this.name }));
        answer = await exchange.receive(HelloAnswer);
      } catch (error) {
        // An older node reads the hello as a postcard request it cannot
        // decode and closes, sometimes before it reads all of it.
        if (error instanceof FrameIoError) {
          throw new ClosedBeforeWelcomeError(this.socketPath, error);
        }
        throw error;
      }
      if (answer === null) {
        throw new ClosedBeforeWelcomeError(this.socketPath);
      }
      if (answer.type === "welcome" && answer.api === API_VERSION) {
        return { exchange, welcome: answer };
      }
      throw new IncompatibleError(API_VERSION, answer.api, answer.node_version);
    } catch (error) {
      exchange.close();
      throw error;
    }
  }
}

/** One connection's frames in both directions. */
type Exchange = {
  send(value: unknown): Promise<void>;
  /** The next frame parsed with `schema`, or `null` when the node closed between frames. */
  receive<T extends z.ZodType>(schema: T): Promise<z.output<T> | null>;
  close(): void;
};

function dial(path: string): Promise<Socket> {
  return new Promise((resolvePromise, reject) => {
    const socket = createConnection({ path });
    const failed = (error: Error): void => {
      socket.destroy();
      reject(new NoDaemonError(path, error));
    };
    socket.once("error", failed);
    socket.once("connect", () => {
      socket.off("error", failed);
      resolvePromise(socket);
    });
  });
}

function openExchange(socket: Socket): Exchange {
  const decoder = createFrameDecoder();
  const frames: Uint8Array[] = [];
  // A failure waits behind the frames that arrived before it, so a caller
  // reads every event the node sent before the socket broke.
  let failure: FabricError | undefined;
  let ended = false;
  let wake: (() => void) | undefined;
  const notify = (): void => {
    const waiting = wake;
    wake = undefined;
    waiting?.();
  };

  socket.on("data", (chunk: Buffer) => {
    decoder.push(chunk);
    try {
      for (let frame = decoder.next(); frame !== undefined; frame = decoder.next()) {
        frames.push(frame);
      }
    } catch (error) {
      failure ??= error instanceof FabricError ? error : new FrameIoError(error);
      socket.destroy();
    }
    notify();
  });
  socket.on("error", (error) => {
    failure ??= new FrameIoError(error);
    notify();
  });
  socket.on("end", () => {
    ended = true;
    notify();
  });
  socket.on("close", () => {
    ended = true;
    notify();
  });

  return {
    send(value) {
      return new Promise((resolvePromise, reject) => {
        socket.write(encodeFrame(value), (error) => {
          if (error) {
            reject(new FrameIoError(error));
          } else {
            resolvePromise();
          }
        });
      });
    },
    async receive(schema) {
      for (;;) {
        const frame = frames.shift();
        if (frame !== undefined) {
          const parsed = schema.safeParse(decodeJson(frame));
          if (!parsed.success) {
            throw new MalformedError(parsed.error.message, parsed.error);
          }
          return parsed.data;
        }
        if (failure !== undefined) {
          throw failure;
        }
        if (ended) {
          if (decoder.buffered() > 0) {
            throw new FrameIoError(new Error("the connection closed inside a frame"));
          }
          return null;
        }
        await new Promise<void>((resolvePromise) => {
          wake = resolvePromise;
        });
      }
    },
    close() {
      socket.destroy();
    },
  };
}

/**
 * Refuses `path` unless this user owns it and, for a directory, no one
 * else may enter it. A missing path passes: the connect that follows
 * reports it as no daemon.
 */
async function checkPrivate(path: string): Promise<void> {
  let stats: Stats;
  try {
    stats = await lstat(path);
  } catch (error) {
    if (errorCode(error) === "ENOENT") {
      return;
    }
    throw new FrameIoError(error);
  }
  const me = process.geteuid?.();
  if (me === undefined) {
    throw new UntrustedError(path, "cannot be checked on this platform");
  }
  if (stats.uid !== me) {
    throw new UntrustedError(path, `belongs to user ${stats.uid}, not to you (${me})`);
  }
  if (stats.isSymbolicLink()) {
    throw new UntrustedError(path, "is a symlink");
  }
  if (stats.isDirectory() && (stats.mode & 0o077) !== 0) {
    throw new UntrustedError(path, `has mode ${(stats.mode & 0o777).toString(8)}, which lets others in`);
  }
}

function errorCode(error: unknown): string | undefined {
  return error instanceof Error && "code" in error && typeof error.code === "string" ? error.code : undefined;
}

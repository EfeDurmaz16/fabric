// The client against a fake node on a temporary unix socket: what it sends,
// what it hands to `onEvent`, and which error each broken exchange gives.

import { afterEach, describe, expect, test } from "bun:test";
import { chmodSync, mkdtempSync, readFileSync, rmSync, symlinkSync } from "node:fs";
import { createServer, type Server, type Socket } from "node:net";
import { join } from "node:path";

import {
  clientEnvFromProcess,
  ClosedBeforeAnswerError,
  ClosedBeforeWelcomeError,
  createFrameDecoder,
  decodeJson,
  encodeFrame,
  FabricClient,
  IncompatibleError,
  JobId,
  type KnownEvent,
  NoDaemonError,
  OPS,
  PathDir,
  RefusedError,
  SDK_VERSION,
  SnapshotId,
  SystemPath,
  UnexpectedError,
  UnsupportedError,
  UnsupportedFieldError,
  UntrustedError,
} from "../src/index.ts";

const NODE = "fab1aeaqcaibaeaqcaibaeaqcaibaeaqcaibaeaqcaibaeaqcaibaeaq";
const JOB = JobId.parse("01JABCDEFGHJKMNPQRSTVWXYZ0");
const SNAPSHOT = SnapshotId.parse("ab".repeat(32));

function welcome(capabilities: readonly string[] = OPS): unknown {
  return { type: "welcome", api: 1, node: NODE, node_version: "0.1.0", capabilities };
}

/** One accepted connection, as the fake node sees it. */
type Peer = {
  /** The next frame from the client, or `null` once it closed. */
  read(): Promise<unknown>;
  send(value: unknown): void;
  end(): void;
};

type FakeNode = {
  socketPath: string;
  /** Every frame each connection received, once the connection closed. */
  connections: Promise<unknown[]>[];
};

const cleanups: Array<() => void> = [];

afterEach(() => {
  for (const cleanup of cleanups.splice(0)) {
    cleanup();
  }
});

/** A directory under /tmp, short enough for a socket path, with `mode`. */
function tempDir(mode = 0o700): string {
  const dir = mkdtempSync("/tmp/fsdk");
  chmodSync(dir, mode);
  cleanups.push(() => rmSync(dir, { recursive: true, force: true }));
  return dir;
}

function peer(socket: Socket, received: unknown[]): Peer {
  const decoder = createFrameDecoder();
  const frames: unknown[] = [];
  let closed = false;
  let wake: (() => void) | undefined;
  socket.on("data", (chunk: Buffer) => {
    decoder.push(chunk);
    for (let frame = decoder.next(); frame !== undefined; frame = decoder.next()) {
      const value = decodeJson(frame);
      received.push(value);
      frames.push(value);
    }
    wake?.();
  });
  socket.on("close", () => {
    closed = true;
    wake?.();
  });
  socket.on("error", () => {});
  return {
    async read() {
      for (;;) {
        if (frames.length > 0) {
          return frames.shift();
        }
        if (closed) {
          return null;
        }
        await new Promise<void>((resolve) => {
          wake = resolve;
        });
      }
    },
    send(value) {
      socket.write(encodeFrame(value));
    },
    end() {
      socket.end();
    },
  };
}

/** Starts a node on `<dir>/fabric.sock` that runs `script` on each connection, after it drains. */
async function fakeNode(script: (peer: Peer) => Promise<void>, dir = tempDir()): Promise<FakeNode> {
  const socketPath = join(dir, "fabric.sock");
  const connections: Promise<unknown[]>[] = [];
  const server: Server = createServer((socket) => {
    const received: unknown[] = [];
    connections.push(
      new Promise((resolve) => {
        socket.on("close", () => resolve(received));
        const connection = peer(socket, received);
        void script(connection).then(async () => {
          // Read until the client leaves, so anything it sent late is recorded.
          while ((await connection.read()) !== null) {}
        });
      }),
    );
  });
  await new Promise<void>((resolve) => server.listen(socketPath, resolve));
  cleanups.push(() => server.close());
  return { socketPath, connections };
}

/** A script that welcomes the client, reads its request and answers with `frames`. */
function answers(...frames: unknown[]): (peer: Peer) => Promise<void> {
  return async (connection) => {
    await connection.read();
    connection.send(welcome());
    await connection.read();
    for (const frame of frames) {
      connection.send(frame);
    }
    connection.end();
  };
}

async function rejection(promise: Promise<unknown>): Promise<unknown> {
  try {
    await promise;
  } catch (error) {
    return error;
  }
  throw new Error("the call did not reject");
}

describe("the client", () => {
  test("names this SDK and its version in the hello", async () => {
    const node = await fakeNode(answers({ type: "done", reply: { kind: "job_list", jobs: [], closed: [] } }));
    await new FabricClient({ socketPath: node.socketPath }).jobList();
    const [received] = await Promise.all(node.connections);
    expect(received?.[0]).toStrictEqual({ api: 1, client: `fabric-sdk-typescript/${SDK_VERSION}` });
    expect(received?.[1]).toStrictEqual({ op: "job_list" });
    const manifest: unknown = JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8"));
    expect(manifest).toMatchObject({ version: SDK_VERSION });
  });

  test("skips unknown event kinds and hands known ones to onEvent in order", async () => {
    const node = await fakeNode(
      answers(
        { type: "event", event: { kind: "stopping_harness", pid: 4242 } },
        { type: "event", event: { kind: "teleported", to: "mars" } },
        {
          type: "event",
          event: { kind: "snapshot_published", snapshot: SNAPSHOT, files: 3, bytes: 10, new_objects: 1 },
        },
        { type: "event", event: { kind: "suspended", snapshot: SNAPSHOT, epoch: 1 } },
        { type: "done", reply: { kind: "job_suspend", snapshot: SNAPSHOT } },
      ),
    );
    const events: KnownEvent[] = [];
    const suspended = await new FabricClient({ socketPath: node.socketPath }).jobSuspend(JOB, {
      onEvent: (event) => events.push(event),
    });
    expect(events.map((event) => event.kind)).toStrictEqual(["stopping_harness", "snapshot_published", "suspended"]);
    expect(suspended).toStrictEqual({ kind: "job_suspend", snapshot: SNAPSHOT });
  });

  test("rejects a reply of another op with Unexpected", async () => {
    const node = await fakeNode(answers({ type: "done", reply: { kind: "job_suspend", snapshot: SNAPSHOT } }));
    const error = await rejection(new FabricClient({ socketPath: node.socketPath }).jobStatus(JOB));
    expect(error).toBeInstanceOf(UnexpectedError);
    expect(error).toMatchObject({ op: "job_status", got: "job_suspend" });
  });

  test("rejects a refusal with Refused, carrying its kind, reason and message", async () => {
    const refusal = {
      kind: "not_owner",
      reason: { code: "unknown_job", job: JOB },
      message: `job ${JOB} is not known on this node`,
    };
    const node = await fakeNode(answers({ type: "refused", refusal }));
    const error = await rejection(new FabricClient({ socketPath: node.socketPath }).jobStatus(JOB));
    expect(error).toBeInstanceOf(RefusedError);
    expect(error).toMatchObject({ kind: "not_owner", reason: refusal.reason, message: refusal.message });
  });

  test("rejects an incompatible node with Incompatible and sends no request", async () => {
    const node = await fakeNode(async (connection) => {
      await connection.read();
      connection.send({ type: "incompatible", api: 2, node_version: "0.2.0" });
    });
    const error = await rejection(new FabricClient({ socketPath: node.socketPath }).jobList());
    expect(error).toBeInstanceOf(IncompatibleError);
    expect(error).toMatchObject({ client: 1, node: 2, nodeVersion: "0.2.0" });
    const [received] = await Promise.all(node.connections);
    expect(received).toHaveLength(1);
  });

  test("rejects a welcome of another api version with Incompatible and sends no request", async () => {
    const node = await fakeNode(async (connection) => {
      await connection.read();
      connection.send({ type: "welcome", api: 2, node: NODE, node_version: "0.2.0", capabilities: OPS });
    });
    const error = await rejection(new FabricClient({ socketPath: node.socketPath }).jobList());
    expect(error).toBeInstanceOf(IncompatibleError);
    expect(error).toMatchObject({ client: 1, node: 2, nodeVersion: "0.2.0" });
    const [received] = await Promise.all(node.connections);
    expect(received).toHaveLength(1);
  });

  test("rejects an op the node does not list with Unsupported and sends no request", async () => {
    const node = await fakeNode(async (connection) => {
      await connection.read();
      connection.send(welcome(OPS.filter((op) => op !== "job_steal")));
    });
    const error = await rejection(new FabricClient({ socketPath: node.socketPath }).jobSteal(JOB));
    expect(error).toBeInstanceOf(UnsupportedError);
    expect(error).toMatchObject({ op: "job_steal" });
    const [received] = await Promise.all(node.connections);
    expect(received).toHaveLength(1);
  });

  // Review finding on PR #13: a node that does not know a request field ignores it, so a client sends a
  // root only to a node that lists it, and an older node never measures with its own value in its place.
  test("rejects a root the node does not list with UnsupportedField and sends no request", async () => {
    const env = { path: [PathDir.parse("/usr/bin")], roots: { cargo_home: SystemPath.parse("/Users/me/.cargo") } };
    const node = await fakeNode(async (connection) => {
      await connection.read();
      connection.send(welcome(["doctor", "env.roots.rustup_home"]));
    });
    const error = await rejection(new FabricClient({ socketPath: node.socketPath }).doctor({ env }));
    expect(error).toBeInstanceOf(UnsupportedFieldError);
    expect(error).toMatchObject({ op: "doctor", capability: "env.roots.cargo_home" });
    const [received] = await Promise.all(node.connections);
    expect(received).toHaveLength(1);

    // A node that lists the root gets the request.
    const listing = await fakeNode(async (connection) => {
      await connection.read();
      connection.send(welcome(["doctor", "env.roots.cargo_home"]));
      await connection.read();
      connection.send({ type: "done", reply: { kind: "doctor", report: { kind: "facts", facts: [] } } });
      connection.end();
    });
    await new FabricClient({ socketPath: listing.socketPath }).doctor({ env });
    const [sent] = await Promise.all(listing.connections);
    expect(sent?.[1]).toMatchObject({ op: "doctor", env: { roots: { cargo_home: "/Users/me/.cargo" } } });
  });

  test("rejects a node that closes right after the hello with ClosedBeforeWelcome", async () => {
    const node = await fakeNode(async (connection) => {
      await connection.read();
      connection.end();
    });
    const error = await rejection(new FabricClient({ socketPath: node.socketPath }).hello());
    expect(error).toBeInstanceOf(ClosedBeforeWelcomeError);
  });

  test("rejects a node that closes after the request with ClosedBeforeAnswer", async () => {
    const node = await fakeNode(answers({ type: "event", event: { kind: "catching_up" } }));
    const error = await rejection(new FabricClient({ socketPath: node.socketPath }).jobList());
    expect(error).toBeInstanceOf(ClosedBeforeAnswerError);
    expect(error).toMatchObject({ op: "job_list" });
  });

  test("rejects a socket directory others may enter with Untrusted before connecting", async () => {
    const node = await fakeNode(answers(), tempDir(0o755));
    const error = await rejection(new FabricClient({ socketPath: node.socketPath }).hello());
    expect(error).toBeInstanceOf(UntrustedError);
    expect(node.connections).toHaveLength(0);
  });

  // A symlink could lead anywhere, such as to a socket another user serves run plans on.
  test("rejects a socket path that is a symlink with Untrusted before connecting", async () => {
    const node = await fakeNode(answers());
    const link = join(tempDir(), "fabric.sock");
    symlinkSync(node.socketPath, link);
    const error = await rejection(new FabricClient({ socketPath: link }).hello());
    expect(error).toBeInstanceOf(UntrustedError);
    expect(node.connections).toHaveLength(0);
  });

  test("rejects a socket nothing listens on with NoDaemon", async () => {
    const error = await rejection(new FabricClient({ socketPath: join(tempDir(), "fabric.sock") }).hello());
    expect(error).toBeInstanceOf(NoDaemonError);
  });
});

describe("clientEnvFromProcess", () => {
  // Review finding on PR #13: entries the shell resolves, such as `/opt/homebrew//bin`, were left out
  // because they are not in the core's one text form. A relative entry or `.` would name a directory of
  // the node's choosing, so it is left out, not sent; every absolute one is kept as written. The Rust
  // test `the_client_env_keeps_every_absolute_entry_and_root_as_written` reads the same variables.
  test("keeps every absolute PATH entry and root as written and leaves out the relative ones", () => {
    const env = clientEnvFromProcess({
      PATH: "/usr/local/bin/:.:bin::/opt/x/.git:/opt/homebrew//bin:/usr/bin/../bin:/cafe\u0301/bin://",
      CARGO_HOME: "/Users/me//.cargo/",
      RUSTUP_HOME: ".rustup",
      PNPM_HOME: "",
      HOME: "/Users/me",
    });
    expect(JSON.parse(JSON.stringify(env))).toStrictEqual({
      path: ["/usr/local/bin/", "/opt/x/.git", "/opt/homebrew//bin", "/usr/bin/../bin", "/cafe\u0301/bin", "//"],
      roots: { cargo_home: "/Users/me//.cargo/" },
    });
  });
});

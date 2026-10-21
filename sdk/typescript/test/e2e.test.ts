// The SDK against a real `fabric serve`: a job made, listed, read, decided
// and suspended over the client API, a refusal read as typed data, and a
// hello of another api answered `incompatible`. It needs FABRIC_BIN, the
// path to a built `fabric` binary; without it the suite is skipped.

import { afterAll, beforeAll, describe, expect, setDefaultTimeout, test } from "bun:test";
import { type ChildProcess, execFileSync, spawn } from "node:child_process";
import { createSocket } from "node:dgram";
import { once } from "node:events";
import { existsSync, mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { createConnection, createServer } from "node:net";
import { delimiter, join } from "node:path";

import {
  ClientEnv,
  createFrameDecoder,
  type DoctorReport,
  decodeJson,
  encodeFrame,
  Epoch,
  FabricClient,
  HelloAnswer,
  JobId,
  type KnownEvent,
  NoDaemonError,
  RefusedError,
  SystemPath,
  type Welcome,
} from "../src/index.ts";

const BIN = process.env["FABRIC_BIN"] ?? "";
const GOAL = "fix the flaky test";
const START_TIMEOUT_MS = 30_000;

if (BIN === "") {
  console.warn("e2e: FABRIC_BIN is not set, so the end-to-end suite against a real daemon is skipped");
}

setDefaultTimeout(120_000);

/** What the suite shares: one daemon's directories, a client of it and what it said first. */
type World = {
  home: string;
  repo: string;
  socketPath: string;
  client: FabricClient;
  welcome: Welcome;
  job?: JobId;
};

let world: World | undefined;
/** What afterAll stops and removes, set as soon as it exists so a failed start is cleaned up too. */
let started: { root: string; daemon?: ChildProcess } | undefined;

function the(): World {
  if (world === undefined) {
    throw new Error("the daemon did not start");
  }
  return world;
}

function theJob(): JobId {
  const { job } = the();
  if (job === undefined) {
    throw new Error("no job was made");
  }
  return job;
}

function git(cwd: string, ...args: string[]): void {
  execFileSync(
    "git",
    ["-c", "user.name=fabric", "-c", "user.email=fabric@example.com", "-c", "commit.gpgsign=false", ...args],
    { cwd, env: { ...process.env, GIT_CONFIG_GLOBAL: "/dev/null" }, stdio: "ignore" },
  );
}

function makeRepo(repo: string): void {
  mkdirSync(join(repo, "src"), { recursive: true });
  writeFileSync(join(repo, "README.md"), "hello\n");
  writeFileSync(join(repo, "src", "greet.txt"), "hello, world\n");
  git(repo, "init", "-q", "-b", "main");
  git(repo, "add", "-A");
  git(repo, "commit", "-q", "-m", "first");
}

/** A `PATH` that holds git and the base system, and nothing of the user's shell. */
function daemonPath(): string {
  const gitDir = (process.env["PATH"] ?? "")
    .split(delimiter)
    .find((dir) => dir !== "" && existsSync(join(dir, "git")));
  return [...(gitDir === undefined ? [] : [gitDir]), "/usr/bin", "/bin"].join(delimiter);
}

/** A loopback port free for both TCP and UDP, since the node listens on QUIC and on TCP. */
async function freePort(): Promise<number> {
  for (let attempt = 0; attempt < 50; attempt += 1) {
    const server = createServer();
    server.listen(0, "127.0.0.1");
    await once(server, "listening");
    const address = server.address();
    server.close();
    if (address === null || typeof address === "string") {
      continue;
    }
    const udp = createSocket("udp4");
    const bound = await new Promise<boolean>((resolve) => {
      udp.once("error", () => resolve(false));
      udp.bind(address.port, "127.0.0.1", () => resolve(true));
    });
    udp.close();
    if (bound) {
      return address.port;
    }
  }
  throw new Error("no loopback port is free for both TCP and UDP");
}

/** The daemon's welcome once its socket answers; only a socket not yet there is waited for. */
async function waitForHello(client: FabricClient, daemon: ChildProcess, output: () => string): Promise<Welcome> {
  const deadline = Date.now() + START_TIMEOUT_MS;
  for (;;) {
    if (daemon.exitCode !== null || daemon.signalCode !== null) {
      throw new Error(`the daemon exited before it answered:\n${output()}`);
    }
    try {
      return await client.hello();
    } catch (error) {
      if (!(error instanceof NoDaemonError) || Date.now() > deadline) {
        throw new Error(`the daemon did not welcome the client: ${String(error)}\n${output()}`, { cause: error });
      }
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
}

describe.skipIf(BIN === "")("against a real fabric daemon", () => {
  beforeAll(async () => {
    // Under /tmp, resolved, so the socket path stays within the 104 bytes a
    // unix socket allows on macOS and the job root is the path the node computes.
    const root = realpathSync(mkdtempSync("/tmp/fsdke"));
    started = { root };
    const home = join(root, "home");
    const state = join(root, "state");
    const repo = join(root, "repo");
    mkdirSync(home, { mode: 0o700 });
    mkdirSync(state, { mode: 0o700 });
    makeRepo(repo);
    const port = await freePort();
    const env: Record<string, string> = { HOME: home, FABRIC_HOME: state, PATH: daemonPath() };
    const level = process.env["FABRIC_LOG"];
    if (level !== undefined) {
      env["FABRIC_LOG"] = level;
    }
    const daemon = spawn(BIN, ["serve", "--listen", `127.0.0.1:${port}`], {
      env,
      stdio: ["ignore", "pipe", "pipe"],
    });
    started.daemon = daemon;
    let log = "";
    daemon.stdout?.on("data", (chunk: Buffer) => {
      log += chunk.toString();
    });
    daemon.stderr?.on("data", (chunk: Buffer) => {
      log += chunk.toString();
    });
    const output = (): string => log;
    const socketPath = join(state, "fabric.sock");
    const client = new FabricClient({ socketPath, name: "fabric-sdk-typescript-e2e/0.1.0" });
    const welcome = await waitForHello(client, daemon, output);
    world = { home, repo, socketPath, client, welcome };
  });

  afterAll(async () => {
    if (started === undefined) {
      return;
    }
    const { daemon, root } = started;
    if (daemon !== undefined && daemon.exitCode === null && daemon.signalCode === null) {
      const exited = once(daemon, "exit");
      daemon.kill("SIGTERM");
      const killer = setTimeout(() => daemon.kill("SIGKILL"), 10_000);
      await exited;
      clearTimeout(killer);
    }
    rmSync(root, { recursive: true, force: true });
  });

  test("node_id names the node the welcome named", async () => {
    const { client, welcome } = the();
    const identity = await client.nodeId();
    expect(identity.node).toBe(welcome.node);
  });

  test("job_list is empty on a new node", async () => {
    const list = await the().client.jobList();
    expect(list.jobs).toHaveLength(0);
    expect(list.closed).toHaveLength(0);
  });

  test("job_new makes a job at epoch 1 rooted under the home", async () => {
    const { client, home, repo } = the();
    const created = await client.jobNew(SystemPath.parse(repo), GOAL, { kind: "claude_code" });
    expect<number>(created.epoch).toBe(1);
    expect(created.root).toBe(join(home, "fabric", "jobs", created.job, "work"));
    expect(created.repo).toBe(repo);
    expect(created.harness).toStrictEqual({ kind: "claude_code" });
    the().job = created.job;
  });

  test("job_list shows the job with this node as its owner", async () => {
    const list = await the().client.jobList();
    expect(list.jobs).toHaveLength(1);
    expect(list.jobs[0]).toMatchObject({ job: theJob(), standing: "owner", epoch: 1, goal: GOAL });
  });

  test("job_status says owner, a live workspace and no harness", async () => {
    const { client, home, welcome } = the();
    const status = await client.jobStatus(theJob());
    expect(status).toMatchObject({
      job: theJob(),
      node: welcome.node,
      owner: welcome.node,
      epoch: 1,
      goal: GOAL,
      view: { kind: "owner" },
      workspace: {
        root: join(home, "fabric", "jobs", theJob(), "work"),
        state: { kind: "current", marker: { kind: "live" } },
      },
      process: { kind: "not_running" },
    });
    expect(status.snapshot).toBeUndefined();
  });

  test("job_log starts with the created record", async () => {
    const { client, welcome } = the();
    const log = await client.jobLog(theJob());
    expect(log.records[0]?.event).toMatchObject({
      kind: "created",
      created_by: welcome.node,
      harness: { kind: "claude_code" },
      goal: GOAL,
    });
  });

  test("job_decide records the approval with its note", async () => {
    expect(await the().client.jobDecide(theJob(), "approve", "ship it")).toBe("approved: ship it");
  });

  test("job_suspend reports its steps as events and returns the snapshot they name", async () => {
    const events: KnownEvent[] = [];
    const suspended = await the().client.jobSuspend(theJob(), { onEvent: (event) => events.push(event) });
    const published = events.find((event) => event.kind === "snapshot_published");
    expect(published).toMatchObject({ kind: "snapshot_published", snapshot: suspended.snapshot });
    expect(events.at(-1)).toStrictEqual({ kind: "suspended", snapshot: suspended.snapshot, epoch: Epoch.parse(1) });
    const status = await the().client.jobStatus(theJob());
    expect(status.snapshot).toStrictEqual({ id: suspended.snapshot, suspended: true });
    const log = await the().client.jobLog(theJob());
    expect(log.records.map((record) => record.event.kind)).toStrictEqual([
      "created",
      "decision_recorded",
      "snapshot_published",
      "suspended",
    ]);
  });

  test("a job the node does not know is refused as not_owner with the reason unknown_job", async () => {
    const unknown = JobId.parse("7ZZZZZZZZZZZZZZZZZZZZZZZZZ");
    let refused: unknown;
    try {
      await the().client.jobStatus(unknown);
    } catch (error) {
      refused = error;
    }
    expect(refused).toBeInstanceOf(RefusedError);
    expect(refused).toMatchObject({ kind: "not_owner", reason: { code: "unknown_job", job: unknown } });
  });

  // The node measures in the environment the client sends, not its own (review finding on PR #8).
  // Without one it measures in the last one a client sent, or its own until a client sends one, as
  // it does for a peer (review finding on PR #13). No earlier test sends an environment.
  test("doctor measures with the client's roots, and without them with the last a client sent", async () => {
    const { client, home } = the();
    const cargoHome = (report: DoctorReport): string | undefined =>
      report.kind === "facts" ? report.facts.find((fact) => fact.name === "cargo.home")?.value : undefined;
    const env = ClientEnv.parse({ path: daemonPath().split(delimiter), roots: { cargo_home: "/x/cargo" } });
    expect(cargoHome(await client.doctor())).toBe(join(home, ".cargo"));
    expect(cargoHome(await client.doctor({ env }))).toBe("/x/cargo");
    expect(cargoHome(await client.doctor())).toBe("/x/cargo");
  });

  test("a hello of another api version is answered incompatible", async () => {
    const socket = createConnection({ path: the().socketPath });
    await once(socket, "connect");
    socket.write(encodeFrame({ api: 999, client: "fabric-sdk-typescript-e2e/0.1.0" }));
    const decoder = createFrameDecoder();
    const body = await new Promise<Uint8Array>((resolve, reject) => {
      socket.on("data", (chunk: Buffer) => {
        decoder.push(chunk);
        const frame = decoder.next();
        if (frame !== undefined) {
          resolve(frame);
        }
      });
      socket.on("error", reject);
      socket.on("close", () => reject(new Error("the node closed before it answered the hello")));
    });
    socket.destroy();
    expect(HelloAnswer.parse(decodeJson(body))).toMatchObject({ type: "incompatible", api: 1 });
  });
});

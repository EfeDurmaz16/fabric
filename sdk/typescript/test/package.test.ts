// The package as a Node project gets it: built with tsconfig.build.json,
// packed by npm with the manifest's `files`, installed in another project
// and imported by plain Node, which refuses to strip types there, then
// read by `tsc` through the manifest's `types`. The rest of the suite runs
// the sources under Bun and sees none of this. It needs `node` 22 or later
// and `npm` on PATH.

import { afterAll, beforeAll, describe, expect, setDefaultTimeout, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { copyFileSync, cpSync, mkdirSync, mkdtempSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const SDK = fileURLToPath(new URL("..", import.meta.url));
const TSC = join(SDK, "node_modules", "typescript", "bin", "tsc");
const NODE_FLOOR = 22;

setDefaultTimeout(120_000);

const root = mkdtempSync(join(tmpdir(), "fabric-pack-"));
afterAll(() => rmSync(root, { recursive: true, force: true }));

type Ran = { status: number | null; stdout: string; stderr: string };

function run(command: string, args: readonly string[], cwd: string): Ran {
  const ran = spawnSync(command, args, {
    cwd,
    encoding: "utf8",
    env: {
      ...process.env,
      NODE_OPTIONS: "",
      npm_config_cache: join(root, "npm-cache"),
      npm_config_update_notifier: "false",
    },
  });
  if (ran.error !== undefined) {
    throw new Error(`could not run ${command}: ${ran.error.message}; this test needs node ${NODE_FLOOR} or later and npm`);
  }
  return { status: ran.status, stdout: ran.stdout, stderr: ran.stderr };
}

/**
 * Builds the SDK beside a copy of its manifest and sources, packs that as npm would publish it, and installs
 * the SDK and its locked runtime dependency from local tarballs in a clean Node project.
 */
function install(): string {
  const pkg = join(root, "pkg");
  mkdirSync(pkg);
  copyFileSync(join(SDK, "package.json"), join(pkg, "package.json"));
  cpSync(join(SDK, "src"), join(pkg, "src"), { recursive: true });
  const built = run("node", [TSC, "-p", join(SDK, "tsconfig.build.json"), "--outDir", join(pkg, "dist")], SDK);
  expect(built).toMatchObject({ status: 0, stdout: "" });

  const packed = run("npm", ["pack", pkg, "--ignore-scripts", "--pack-destination", root], root);
  expect(packed.status).toBe(0);
  const tarballs = readdirSync(root).filter((name) => name.endsWith(".tgz"));
  expect(tarballs).toHaveLength(1);

  const dependency = run("npm", ["pack", join(SDK, "node_modules", "zod"), "--ignore-scripts", "--pack-destination", root], root);
  expect(dependency.status).toBe(0);
  const app = join(root, "app");
  mkdirSync(app);
  writeFileSync(join(app, "package.json"), JSON.stringify({ private: true, type: "module" }));
  const installed = run("npm", [
    "install", "--offline", "--ignore-scripts", "--no-audit", "--no-fund",
    join(root, tarballs[0]!), join(root, dependency.stdout.trim()),
  ], app);
  expect(installed.status).toBe(0);
  return app;
}

// Review finding on PR #13: `exports` named src/index.ts, and Node refuses to strip types under node_modules
// (ERR_UNSUPPORTED_NODE_MODULES_TYPE_STRIPPING), so the package could not be imported once installed.
describe("the package", () => {
  let app = "";
  beforeAll(() => {
    app = install();
  });

  test("imports under plain Node from node_modules, as JavaScript, and runs", () => {
    const script = `
      import { API_VERSION, FabricJobs, Hello, JobId, NodeId, NoDaemonError, SDK_VERSION } from "@fabric-jobs/client";
      let error;
      try {
        const jobs = new FabricJobs({
          socketPath: "/fabric-pack-test-missing/fabric.sock",
          authority: NodeId.parse("fab1aeaqcaibaeaqcaibaeaqcaibaeaqcaibaeaqcaibaeaqcaibaeaq"),
          token: "0".repeat(64),
        });
        await jobs.inspect(JobId.parse("01JABCDEFGHJKMNPQRSTVWXYZ0"));
      } catch (thrown) {
        error = thrown;
      }
      console.log(JSON.stringify({
        node: Number(process.versions.node.split(".")[0]),
        entry: import.meta.resolve("@fabric-jobs/client").replace(/^.*\\/node_modules\\//, "node_modules/"),
        hello: Hello.parse({ api: API_VERSION, client: "node" }),
        sdk: SDK_VERSION,
        noDaemon: error instanceof NoDaemonError,
      }));
    `;
    const imported = run("node", ["--input-type=module", "--eval", script], app);
    expect({ status: imported.status, stderr: imported.stderr }).toStrictEqual({ status: 0, stderr: "" });
    const seen: { node: number } & Record<string, unknown> = JSON.parse(imported.stdout);
    expect(seen.node).toBeGreaterThanOrEqual(NODE_FLOOR);
    expect(seen).toMatchObject({
      entry: "node_modules/@fabric-jobs/client/dist/index.js",
      hello: { api: 2, client: "node" },
      sdk: "0.1.0",
      noDaemon: true,
    });
  });

  test("gives a TypeScript consumer its declarations", () => {
    writeFileSync(
      join(app, "consumer.ts"),
      [
        `import { API_VERSION, FabricClient, FabricJobs, JobId, NodeId, type Inspection, type Welcome } from "@fabric-jobs/client";`,
        `const client: FabricClient = new FabricClient({ socketPath: "/nowhere/fabric.sock" });`,
        `declare const jobs: FabricJobs, id: JobId, authority: NodeId;`,
        `export const inspection: Promise<Inspection> = jobs.inspect(id);`,
        `// @ts-expect-error A mutating call requires a caller-owned operation key.`,
        `jobs.move(id, { pin: authority });`,
        `export const api: 2 = API_VERSION;`,
        `export const welcome: Promise<Welcome> = client.hello();`,
        `// @ts-expect-error A socket path is a string; with no declarations the import would be any and this would pass.`,
        `new FabricClient({ socketPath: 42 });`,
        ``,
      ].join("\n"),
    );
    const compilerOptions = {
      target: "es2023",
      lib: ["es2023"],
      module: "nodenext",
      moduleResolution: "nodenext",
      strict: true,
      noEmit: true,
      skipLibCheck: true,
      types: [],
    };
    writeFileSync(join(app, "tsconfig.json"), `${JSON.stringify({ compilerOptions, files: ["consumer.ts"] })}\n`);
    const checked = run("node", [TSC, "-p", join(app, "tsconfig.json")], app);
    expect({ status: checked.status, stdout: checked.stdout }).toStrictEqual({ status: 0, stdout: "" });
  });
});

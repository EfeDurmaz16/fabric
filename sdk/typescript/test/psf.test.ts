// The TypeScript reader of the Portable Session Format against the shared
// fixtures in protocol/psf/v1, and the conformance of the three readers:
// this one, scripts/psf_check.py and, when FABRIC_BIN names a built fabric,
// the Rust crate through `fabric psf verdicts`. Each prints one verdict
// line per fixture and per generated document, and the lines must be equal.

import { describe, expect, test } from "bun:test";
import { execFileSync } from "node:child_process";
import { cpSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import {
  achievable,
  manifestDigest,
  PSF_CODES,
  PsfContainerError,
  PsfError,
  readContainer,
  readManifest,
  type Target,
} from "../src/index.ts";

const repo = fileURLToPath(new URL("../../..", import.meta.url));
const fixtures = join(repo, "protocol/psf/v1");
const python = join(repo, "scripts/psf_check.py");

const TARGET: Target = {
  root: "/Users/me/fabric/jobs/01JABCDEFGHJKMNPQRSTVWXYZ0/work",
  home: "/Users/me",
  harnesses: [
    ["claude_code", "2.1.294"],
    ["codex", "0.163.0"],
  ],
};

type Line = Record<string, unknown>;

// One line, keys sorted, no whitespace: the form every reader prints.
function line(value: Line): string {
  const sorted = Object.fromEntries(Object.keys(value).sort().map((k) => [k, value[k]]));
  return JSON.stringify(sorted);
}

function caseVerdict(name: string, bytes: Uint8Array): string {
  try {
    const m = readManifest(bytes);
    return line({
      achieved: achievable(m, TARGET).achieved,
      digest: m.digest,
      fidelity: m.value.fidelity.level,
      name,
      outcome: "accept",
      package: m.value.package !== null,
    });
  } catch (e) {
    if (!(e instanceof PsfError)) throw e;
    return line({ code: e.code, field: e.field, name, outcome: "refuse" });
  }
}

async function containerVerdict(name: string, dir: string): Promise<string> {
  try {
    const c = await readContainer(dir);
    return line({
      achieved: achievable(c.manifest, TARGET, c.detached).achieved,
      detached: c.detached,
      digest: c.manifest.digest,
      name,
      outcome: "accept",
    });
  } catch (e) {
    if (!(e instanceof PsfContainerError)) throw e;
    return line({ code: e.code, field: e.field, name, outcome: "refuse", path: e.path });
  }
}

interface Fixture {
  name: string;
  verdict: string;
  expect: Record<string, unknown>;
}

async function fixtureVerdicts(): Promise<Fixture[]> {
  const out: Fixture[] = [];
  for (const file of readdirSync(join(fixtures, "cases")).sort()) {
    if (!file.endsWith(".json")) continue;
    const c = JSON.parse(readFileSync(join(fixtures, "cases", file), "utf8")) as { input: string; expect: Line };
    const name = `cases/${file.slice(0, -".json".length)}`;
    out.push({ name, verdict: caseVerdict(name, Buffer.from(c.input, "utf8")), expect: c.expect });
  }
  for (const dir of readdirSync(join(fixtures, "containers")).sort()) {
    const src = join(fixtures, "containers", dir);
    const expect = (JSON.parse(readFileSync(join(src, "expect.json"), "utf8")) as { expect: Line }).expect;
    const scratch = mkdtempSync(join(tmpdir(), "psf-"));
    try {
      const copy = join(scratch, dir);
      cpSync(src, copy, { recursive: true, filter: (from) => from !== join(src, "expect.json") });
      out.push({ name: `containers/${dir}`, verdict: await containerVerdict(`containers/${dir}`, copy), expect });
    } finally {
      rmSync(scratch, { recursive: true, force: true });
    }
  }
  return out;
}

// Every accepted fixture input, then each one changed at every value it
// holds, the same edits as crates/fabric-psf/tests/conformance.rs.
function generated(): string[] {
  const seeds: string[] = [];
  for (const file of readdirSync(join(fixtures, "cases")).sort()) {
    const c = JSON.parse(readFileSync(join(fixtures, "cases", file), "utf8")) as { input: string; expect: Line };
    if (c.expect.outcome === "accept") seeds.push(c.input);
  }
  type Step = string | number;
  const paths = (value: unknown, at: Step[], out: Step[][]): void => {
    out.push(at);
    if (Array.isArray(value)) value.forEach((v, i) => paths(v, [...at, i], out));
    else if (typeof value === "object" && value !== null) {
      for (const [k, v] of Object.entries(value)) paths(v, [...at, k], out);
    }
  };
  const edit = (root: unknown, at: Step[], replace: { value: unknown } | undefined): void => {
    let node = root as Record<string | number, unknown>;
    for (const step of at.slice(0, -1)) node = node[step] as Record<string | number, unknown>;
    const last = at[at.length - 1] as Step;
    if (Array.isArray(node)) {
      if (replace === undefined) node.splice(last as number, 1);
      else node[last as number] = replace.value;
    } else if (replace === undefined) delete node[last];
    else node[last] = replace.value;
  };
  const replacements: unknown[] = [
    null,
    "",
    "x",
    "0",
    "01",
    [],
    {},
    true,
    7,
    `ghp_${"a1".repeat(18)}`,
    "/a/../b",
    "a/.GIT/b",
    "/Users/other",
    "native_session",
    "new_native_session_with_package",
    "unsupported",
    "codex",
    "Café",
    "2.1.294-rc.1",
    "0.163.1-alpha.2+b",
  ];
  const out: string[] = [];
  for (const seed of seeds) {
    out.push(seed);
    const value = JSON.parse(seed) as unknown;
    const all: Step[][] = [];
    paths(value, [], all);
    for (const at of all.filter((p) => p.length > 0)) {
      const removed = structuredClone(value);
      edit(removed, at, undefined);
      out.push(JSON.stringify(removed));
      for (const r of replacements) {
        const changed = structuredClone(value);
        edit(changed, at, { value: r });
        out.push(JSON.stringify(changed));
      }
    }
    for (const key of ["commit", "snapshot", "uri"]) {
      for (const r of [null, {}, []]) {
        const changed = structuredClone(value);
        edit(changed, ["source", key], { value: r });
        const source = (changed as { source: Record<string, unknown> }).source;
        (changed as { source: unknown }).source = Object.fromEntries(Object.entries(source).sort());
        out.push(JSON.stringify(changed));
      }
    }
    const long = "g".repeat(16 * 1024 + 1);
    out.push(seed.replace('"writer":"', `"writer":"${long.slice(0, 120)}`));
    out.push(seed.replace('"goal":"', `"goal":"${long}`));
    out.push(`${seed}\n`, ` ${seed}`);
    out.push(seed.replace('"psf":"1"', '"psf":"1","psf":"1"'));
    out.push(seed.replace("{", '{"zz_new":"kept",'), seed.replace("{", '{"Bad":"x",'));
    out.push(seed.replace('"requires":[]', '"requires":["signature"]'));
    for (const n of ["1e400", "-1e400", "9".repeat(400), "1e-400"]) out.push(seed.replace('"requires":[]', `"requires":[${n}]`));
    out.push(seed.replace("{", '{"__proto__":"x",'), seed.replace("{", '{"constructor":"x",'));
  }
  for (let depth = 126; depth <= 130; depth++) out.push(`{"a":${"[".repeat(depth - 1)}${"]".repeat(depth - 1)}}`);
  out.push("", "[]", "null", "{}", '{"psf":"2"}', '{"psf":1}', "﻿{}", '{"a":"\\ud800"}', '{"a":NaN}');
  out.push('{"psf":"2","n":2}', '{"z":"x","psf":"2"}', '{ "psf": "2" }', '{"Key":"x","psf":"2"}', '{"psf":"1","psf":"2"}');
  return out;
}

describe("psf fixtures", () => {
  test("every case and container reads as its expect says", async () => {
    const all = await fixtureVerdicts();
    expect(all.length).toBe(26);
    for (const { name, verdict, expect: want } of all) {
      const got = JSON.parse(verdict) as Line;
      expect(got.outcome).toBe(want.outcome);
      const keys =
        want.outcome === "accept"
          ? name.startsWith("cases/")
            ? ["digest", "fidelity", "package"]
            : ["digest", "detached"]
          : name.startsWith("cases/")
            ? ["code", "field"]
            : ["code", "path"];
      for (const key of keys) expect([name, key, got[key]]).toEqual([name, key, want[key]]);
    }
  });

  test("the refusal codes are the closed set of section 10", () => {
    const covered = new Set(
      readdirSync(join(fixtures, "cases"))
        .map((f) => JSON.parse(readFileSync(join(fixtures, "cases", f), "utf8")) as { expect: Line })
        .filter((c) => c.expect.outcome === "refuse")
        .map((c) => c.expect.code),
    );
    expect([...covered].sort()).toEqual([...PSF_CODES].sort());
  });

  test("a digest is the sha256 of the bytes", () => {
    expect(manifestDigest(new Uint8Array())).toBe("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
  });

  test("a bound attachment on another home falls to workspace_only with the reason", () => {
    const c = JSON.parse(readFileSync(join(fixtures, "cases/valid_native_session.json"), "utf8")) as { input: string };
    const m = readManifest(Buffer.from(c.input, "utf8"));
    const result = achievable(m, { ...TARGET, home: "/home/me" });
    expect(result.offered).toBe("native_session");
    expect(result.achieved).toBe("workspace_only");
    expect(result.why.join(" ")).toContain("bound to another root or home");
  });
});

test("a pre-release target compares by semver precedence", () => {
  const c = JSON.parse(readFileSync(join(fixtures, "cases/valid_native_session.json"), "utf8")) as { input: string };
  const m = readManifest(Buffer.from(c.input, "utf8"));
  const on = (version: string) => achievable(m, { ...TARGET, harnesses: [["claude_code", version]] }).achieved;
  expect(on("2.1.294-rc.1")).toBe("workspace_only");
  expect(on("2.1.295-alpha.10.2")).toBe("native_session");
  expect(on("2.1.294+build.7")).toBe("native_session");
  expect(on("2.1.294-")).toBe("workspace_only");
});

describe("psf conformance", () => {
  test("the Python reader prints the same line for every fixture", async () => {
    const ours = (await fixtureVerdicts()).map((f) => f.verdict);
    const theirs = execFileSync("python3", ["-I", python, "--verdicts", fixtures], { encoding: "utf8" })
      .split("\n")
      .filter((l) => l !== "");
    expect(theirs).toEqual(ours);
  });

  test("the Python reader prints the same line for every generated document", () => {
    const inputs = generated();
    const scratch = mkdtempSync(join(tmpdir(), "psf-lines-"));
    try {
      const file = join(scratch, "lines");
      writeFileSync(file, inputs.map((i) => `${JSON.stringify(i)}\n`).join(""));
      const theirs = execFileSync("python3", ["-I", python, "--lines", file], {
        encoding: "utf8",
        maxBuffer: 64 << 20,
      })
        .split("\n")
        .filter((l) => l !== "");
      expect(theirs.length).toBe(inputs.length);
      const disagreements = inputs.flatMap((input, i) => {
        const ours = caseVerdict(`lines/${i}`, Buffer.from(input, "utf8"));
        return ours === theirs[i] ? [] : [{ input: input.slice(0, 300), ours, theirs: theirs[i] }];
      });
      expect(disagreements).toEqual([]);
    } finally {
      rmSync(scratch, { recursive: true, force: true });
    }
  }, 60_000);

  const bin = process.env.FABRIC_BIN;
  test.skipIf(bin === undefined)("the Rust reader prints the same line for every fixture", async () => {
    const ours = (await fixtureVerdicts()).map((f) => f.verdict);
    const theirs = execFileSync(bin as string, ["psf", "verdicts", fixtures], { encoding: "utf8" })
      .split("\n")
      .filter((l) => l !== "");
    expect(theirs).toEqual(ours);
  });
});

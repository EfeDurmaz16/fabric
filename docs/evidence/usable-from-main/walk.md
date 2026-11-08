# Walk of `main` on one Mac, 2026-11-03

`main` at `24bb3c4` (merge of PR #32) in a fresh worktree with its own target directory, built with `cargo build --release --locked` (1 m 28 s, `fabric 0.1.0`), then walked from the README as a new user would, on one Mac (the second was off; [../two-mac-gate-pending.md](../two-mac-gate-pending.md) lists what waits for it). Paths below: `<state>` is the short scratch state directory `FABRIC_HOME` named, `<repo>` a two-file git repository made for the walk, `<home>` the user's home. Codex 0.163.0 (logged in) and Claude Code 2.1.294 were the harnesses on the Mac; Capsule's worker was built from Capsule `main` at `34f2a2d` (merge of its PR #317) with `pnpm install --frozen-lockfile` and `pnpm --filter "@capsule-sdk/bridge-worker..." run build`.

## Node

```
$ FABRIC_HOME=<a 100-character scratch path> fabric serve --listen 127.0.0.1:17777
error: the control socket <...>/fabric.sock is too long for a Unix socket; use a shorter FABRIC_HOME
```

Fabric refused before listening, as it should; the README now says to keep the path short. The directory already held `daemon.lock`, `jobs`, `node.key` and `store` from that attempt, so `serve` writes its state before it checks the socket path (noted, not changed: the next `serve` in a shorter directory is a new node anyway).

```
$ FABRIC_HOME=<state> fabric serve --listen 127.0.0.1:17777
node fab1btcp...iwea
listening on 127.0.0.1:17777

$ fabric node id
fab1btcp...iwea
listening on 127.0.0.1:17777

$ fabric doctor
arch: aarch64
claude.login: no
claude.version: 2.1.294 (Claude Code)
codex.login: yes
codex.version: codex-cli 0.163.0
fabric.version: 0.1.0
git.version: git version 2.54.0 (Apple Git-157)
node.version: v22.23.2
os: macos
... (bun, cargo, pnpm, python, rustc, uv, yarn versions and homes)
```

## A standalone job under Codex: new, headless turn, suspend, resume, inspect

```
$ fabric job new --repo <repo> --goal "say hello and name one file" --harness codex
01M6F7QFMX5J45E4PPN8BRSA0T
root <home>/fabric/jobs/01M6F7QFMX5J45E4PPN8BRSA0T/work: a clone of <repo> under ~/fabric/jobs, the same path on every node with this home (ADR 0011)
harness codex; this node owns the job at epoch 1

$ fabric job run 01M6F7QFMX5J45E4PPN8BRSA0T --headless --prompt "Say hello in one short sentence and name one file in this repository. Do not edit anything."
Hello! This repository contains `src/add.ts`.
codex session 01a19e7b-e4e2-7d50-bdea-1da8808baef1
  (stderr) starting a new codex session
  (stderr) Resumed recorded at epoch 1; codex runs as pid 44103
  (stderr) recorded codex session 01a19e7b-e4e2-7d50-bdea-1da8808baef1
exit 0

$ fabric job suspend 01M6F7QFMX5J45E4PPN8BRSA0T
captured codex session 01a19e7b-e4e2-7d50-bdea-1da8808baef1 (1 files)
snapshot ea6fe48e...222b55: 2 ignored and harness files and a git bundle of the tracked and untracked work, 175275 bytes, 4 new objects
suspended in snapshot ea6fe48e...222b55; this node keeps the lease at epoch 1

$ fabric job resume 01M6F7QFMX5J45E4PPN8BRSA0T --headless --prompt "What file did you name in your previous answer? Reply in one sentence."
I named `src/add.ts`.
codex session 01a19e7b-e4e2-7d50-bdea-1da8808baef1
  (stderr) workspace already at <home>/fabric/jobs/01M6F7QFMX5J45E4PPN8BRSA0T/work
  (stderr) continuing codex session 01a19e7b-e4e2-7d50-bdea-1da8808baef1 with model gpt-6.1-sol, model provider openai, effort high, sandbox workspace-write, writable roots none, network access off (recorded in the session)
  (stderr) approval on-request is not carried (codex exec sets it: never, or this node's own under auto review); codex uses its own setting
  (stderr) Resumed recorded at epoch 1; codex runs as pid 48153
exit 0

$ fabric job status 01M6F7QFMX5J45E4PPN8BRSA0T      (after the suspend)
job 01M6F7QFMX5J45E4PPN8BRSA0T
goal: say hello and name one file
harness: codex
owner: fab1btcp...iwea (this node) at epoch 1
this node: owner
snapshot: ea6fe48e...222b55 (suspended in it)
session: captured in snapshot ea6fe48e...222b55
workspace: <home>/fabric/jobs/01M6F7QFMX5J45E4PPN8BRSA0T/work (clean at snapshot ea6fe48e...222b55)
harness: not running
records: 4

$ fabric job log 01M6F7QFMX5J45E4PPN8BRSA0T          (after the resume)
   0 epoch 1 at 1793660272285 Created by fab1btcp...iwea under codex: say hello and name one file
   1 epoch 1 at 1793660281569 Resumed on fab1btcp...iwea under codex
   2 epoch 1 at 1793660302787 SnapshotPublished ea6fe48e...222b55
   3 epoch 1 at 1793660302792 Suspended in ea6fe48e...222b55
   4 epoch 1 at 1793660310747 Resumed on fab1btcp...iwea under codex

$ fabric job ls
01M6F7Q5K0SVWFKJ9PMFGWG2ND owner epoch 1 say hello and name one file
01M6F7QFMX5J45E4PPN8BRSA0T owner epoch 1 say hello and name one file
```

The Codex turn's stderr also carried Codex's own `rmcp::transport` lines about an MCP server in the user's Codex config that wanted a login; they are Codex's, not Fabric's.

## A headless Claude Code turn on this Mac

```
$ fabric job new --repo <repo> --goal "say hello and name one file"        (harness claude, the default)
01M6F7Q5K0SVWFKJ9PMFGWG2ND ...

$ fabric job run 01M6F7Q5K0SVWFKJ9PMFGWG2ND --headless --prompt "Say hello in one short sentence and name one file in this repository. Do not edit anything."
claude-code session 78ec602c-22dd-481e-9cd0-ab769fa00f1c
  (stderr) starting a new claude-code session
  (stderr) warning: Fabric did not trust <home>/fabric/jobs/01M6F7Q5K0SVWFKJ9PMFGWG2ND/work (claude runs as pid ...); Claude asks in its own trust dialog
  (stderr) Resumed recorded at epoch 1; claude-code runs as pid 49774
  (stderr) the turn failed: Failed to authenticate: OAuth session expired and could not be refreshed
exit 1
```

`doctor` had said `claude.login: no`, which is what `claude` answered the node; the turn's failure is Claude's own message for the same state (the `claude` login of the shell the node was started from had lapsed, while other `claude` processes ran on the Mac). Fabric did what the README says: it started the session, reported the failure and exited 1, and `fabric job log` holds the `Resumed` record. A real Claude turn needs `claude login` in that shell first; it is also gate G2 on the two-Mac list.

## A managed job through the journal

```
$ fabric managed create --repo <repo> --goal "managed: say hello" --harness codex --operation-key ufm-create-1
01M6F7SGC8ADPDN066VCE8FKNR
managed job at revision 1, active, generation 1
goal: managed: say hello
task 01M6F7SGC8N28BGCHAE7HE1NPR (primary): open

$ fabric managed create --repo <repo> --goal "managed: say hello" --harness codex --operation-key ufm-create-1
01M6F7SGC8ADPDN066VCE8FKNR
managed job at revision 1, active, generation 1
goal: managed: say hello
task 01M6F7SGC8N28BGCHAE7HE1NPR (primary): open
operation 01M6F7SGC8T67Q5SKEEFY5WTW5 answered again for key ufm-create-1

$ fabric managed create --repo <repo> --goal "managed: something else" --harness codex --operation-key ufm-create-1
error: operation key ufm-create-1 was used for another command
exit 1

$ fabric managed cancel 01M6F7SGC8ADPDN066VCE8FKNR --reason "quickstart walk done" --operation-key ufm-cancel-1
01M6F7SGC8ADPDN066VCE8FKNR
managed job at revision 2, cancelled, generation 1
goal: managed: say hello
task 01M6F7SGC8N28BGCHAE7HE1NPR (primary): cancelled

$ fabric managed cancel 01M6F7SGC8ADPDN066VCE8FKNR --reason "quickstart walk done" --operation-key ufm-cancel-1
... the same four lines ...
operation 01M6F7SYB5DQMMAVYVHYE4FQ4H answered again for key ufm-cancel-1

$ fabric managed ls
01M6F7SGC8ADPDN066VCE8FKNR
managed job at revision 2, cancelled, generation 1
...

$ fabric job status 01M6F7SGC8ADPDN066VCE8FKNR
error: job 01M6F7SGC8ADPDN066VCE8FKNR is a managed job; `managed_status` shows it, and standalone commands do not act on it
exit 1

$ ls <state>/managed/
journal.sqlite  journal.sqlite-shm  journal.sqlite-wal
```

The node's log: `fabricd::authority: made the managed journal path=<state>/managed/journal.sqlite` at the first managed command, not at `serve`.

## The TypeScript SDK against the same node

A Bun script importing `FabricClient` and `JobId` from `sdk/typescript/src/index.ts` with `socketPath: $FABRIC_HOME/fabric.sock`:

```
hello: {"type":"welcome","api":1,"node":"fab1btcp...iwea","node_version":"0.1.0","capabilities":["node_id","node_add","node_list","node_ping","job_new","job_list","job_status","job_log","job_run","job_resume","job_suspend","job_move","job_steal","job_decide","doctor","harness_started","harness_session","harness_exited","launch_check","managed_create","managed_cancel","managed_status","managed_list","job_repair","job_export","harness_started.planned","harness_started.version","env.roots.cargo_home","env.roots.rustup_home","env.roots.pnpm_home","env.roots.xdg_data_home"]}
nodeId: {"kind":"node_id","node":"fab1btcp...iwea","listen":"127.0.0.1:17777"}
jobList: {"kind":"job_list","jobs":[{"job":"01M6F7Q5K0SVWFKJ9PMFGWG2ND","standing":"owner","epoch":1,...},{"job":"01M6F7QFMX5J45E4PPN8BRSA0T","standing":"owner","epoch":1,...}],"closed":[]}
jobStatus: {"kind":"job_status","job":"01M6F7QFMX5J45E4PPN8BRSA0T",...,"harness":{"kind":"codex"},"snapshot":{"id":"ea6fe48e...222b55","suspended":false,"session":{"kind":"captured"}},"workspace":{...,"state":{"kind":"current","marker":{"kind":"live"}}},"process":{"kind":"not_running"},"records":5}
jobLog records: 5
managedList: {"kind":"managed_list","jobs":[{"job":"01M6F7SGC8ADPDN066VCE8FKNR","revision":"2","generation":"1","state":"cancelled",...,"tasks":[{"task":"01M6F7SGC8N28BGCHAE7HE1NPR","state":"cancelled"}],"attempts":[]}]}
managedStatus: {"kind":"managed_status","job":"01M6F7SGC8ADPDN066VCE8FKNR","revision":"2","generation":"1","state":"cancelled",...}
standalone status of a managed id refused: RefusedError job 01M6F7SGC8ADPDN066VCE8FKNR is a managed job; `managed_status` shows it, and standalone commands do not act on it
```

Revision and generation travel as strings, as docs/managed-jobs.md says.

## The bridge against the Capsule worker built from Capsule `main`

```
$ CAPSULE_BRIDGE_WORKER=<capsule>/packages/bridge-worker/dist/bin.js cargo test -p fabric-bridge --test capsule_worker --locked -- --ignored
running 8 tests
test the_docker_profile_on_the_local_engine ... ok
test the_handshake_negotiates_the_fixture_profile_and_refuses_another_version ... ok
test a_repeated_idempotency_key_is_never_sent_again ... ok
test create_exec_observe_destroy_keep_the_scoped_locator ... ok
test a_killed_worker_leaves_unknown_and_a_restart_sends_nothing_twice ... ok
test observe_reads_only_the_operations_own_resource ... ok
test denied_and_unsupported_requests_fail_before_any_effect ... ok
test every_shared_scenario_over_the_workers_stdio ... ok
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.75s
```

`CAPSULE_BRIDGE_DOCKER_IMAGE` was not set, so the Docker test took its skip path. CI's pin of the Capsule commit was moved from `a4d597a` to this `34f2a2d` in the same change.

## Stop

```
$ kill -TERM <serve pid>
  (serve) exit 0
$ fabric node id
error: no fabric daemon at <state>/fabric.sock (Connection refused (os error 61)); start one with `fabric serve`
exit 80
```

No harness process the node had launched (pids 44103, 48153, 49774) was left running. The socket file stays in `<state>` after a clean stop; the CLI's message for it is the one above, and the next `serve` takes the path over. The two job roots under `~/fabric/jobs` were removed by hand afterwards.

## What changed because of the walk

README: a `One Mac, first run` section with the commands above, the `fabric-journal` crate in the status table, ADRs 0019 to 0021 in the status row and the Decisions table, `--locked` on the release build. CONTRIBUTING: `scripts/check-fixtures.sh` in the gate list (CI already ran it) and a pointer to the bridge tests. Status lines of docs/managed-jobs.md, docs/bridge-protocol.md and ADRs 0019 to 0021 now say they are on `main`. No code changed: nothing the walk met was a bug in Fabric.

## Again after M4, 2026-11-08

`main` at `f59f027` (merge of PR #36, M4) merged into this branch, built again in a fresh worktree with `cargo build --release --locked` (2 m 10 s). The parts the merge could break were walked again on one Mac with a scratch `FABRIC_HOME` and a scratch `HOME`, under the fake `codex` of the end-to-end tests (`crates/fabric/tests/e2e/world.rs`), so no real turn ran:

- `serve`, `node id` and `doctor` as above (`codex.login: yes` from the fake).
- `job new`, `job run --headless`, `job suspend`, `job resume --headless`, `job status` and `job log` printed the same lines as on 2026-11-03: the session recorded, captured in the snapshot, and continued with the model, effort and sandbox it carried; five ledger records.
- `managed create` twice with key `k1` (the second `answered again for key k1`), `managed status`, `managed cancel --operation-key k2` (revision 2, cancelled) and `managed ls`.
- `sdk/typescript`: `bun install --frozen-lockfile`, `bun run build` and `bun test` against that release binary, 62 pass, 0 fail.
- `scripts/check-hygiene.sh`, `scripts/check-fixtures.sh` and `scripts/check-budget.sh` against `origin/main` pass.

M4 adds no command to the README's walk: reconciliation runs inside the node against a bridge worker, and its proof is the bridge tests against the Capsule worker that CI runs at the pinned Capsule commit (M4's head, on Capsule `main` since its PR #318).

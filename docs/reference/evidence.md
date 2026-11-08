<!-- This page is written by scripts/docs/gen.py. Do not edit it; run the script. -->

# Evidence

What was run to prove a claim, kept in the repository: gate logs, A/B performance runs, walks of `main`, interop runs. One row per entry of `docs/evidence`, with the first heading of its main page. Everything here is on `main`; work proved only on a branch keeps its evidence on that branch.

| Entry | What it holds |
|---|---|
| [m2-baseline](../evidence/m2-baseline/defects.md) | M2 defect register |
| [m2-core](../evidence/m2-core/gates.md) | M2 core: gates |
| [m2-deferred-integration](../evidence/m2-deferred-integration/README.md) | Integration of the M2 deferred work |
| [m2-integration](../evidence/m2-integration/gates.md) | M2 integrated: gates |
| [m2-review](../evidence/m2-review/README.md) | M2 review findings: fixes, gates and performance |
| [m2-review-pr31](../evidence/m2-review-pr31/README.md) | Review of PR #31: five findings on the integrated M2 head |
| [m3-bridge](../evidence/m3-bridge/gates.md) | M3 provider bridge, Fabric side: gates |
| [m3-decisions](../evidence/m3-decisions/README.md) | M3 evidence: the decisions of 2026-10-29 |
| [m3-deferred](../evidence/m3-deferred/README.md) | M3 evidence: the deferred items |
| [m3-interop](../evidence/m3-interop/gates.md) | M3 provider bridge across implementations: gates and interop runs |
| [m3-pr](../evidence/m3-pr/README.md) | M3 PR evidence: interop with the final Capsule worker, gates |
| [m3-reconcile](../evidence/m3-reconcile/README.md) | M3 evidence: reconciling Fabric and Capsule |
| [m3-review](../evidence/m3-review/README.md) | M3 review findings: fixes, gates and performance |
| [m4](../evidence/m4/README.md) | M4 evidence: the seam between Fabric and Capsule |
| [m4-reconcile](../evidence/m4-reconcile/README.md) | M4 evidence: reconciliation and cleanup in Fabric |
| [m5](../evidence/m5/README.md) | M5 evidence: intent-first placement and the observed job |
| [m6](../evidence/m6/README.md) | M6 evidence: loss recovery on the merged tree |
| [perf-ab](../evidence/perf-ab/README.md) | A/B of main and the M2 branch |
| [perf-cli-protocol-ops](../evidence/perf-cli-protocol-ops/README.md) | A/B of the cli-protocol-ops work against the anchor |
| [perf-durability](../evidence/perf-durability/README.md) | Durability flushes: items 3, 12 and 16, and the restore |
| [perf-facts-net-carry](../evidence/perf-facts-net-carry/README.md) | Performance of the facts, net and carry fixes |
| [perf-launch-hold](../evidence/perf-launch-hold/README.md) | Launch hold: register while the trampoline starts |
| [perf-node-runtime](../evidence/perf-node-runtime/README.md) | Node runtime: counters, not timings |
| [perf-store-git](../evidence/perf-store-git/README.md) | The store-git group: counters against the anchor |
| [two-mac-gate-pending.md](../evidence/two-mac-gate-pending.md) | Checks that need the second Mac |
| [usable-from-main](../evidence/usable-from-main/walk.md) | Walk of `main` on one Mac, 2026-11-03 |

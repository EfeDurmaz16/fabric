# Roadmap

The next fifteen pull requests. They were selected on 2026-11-08 under Efe's delegation and approved by him the same day. They run as parallel workstreams, a few at a time, and each one merges only with CI green, review findings resolved and an adversarial review done.

The direction: Fabric goes from a tool that moves a job between two Macs to a portable format for coding-agent sessions, with a reference runtime that runs that format on any machine you own. Capsule, a separate project with its own name, is how a job reaches a sandbox: it performs provider actions for Fabric with policy and receipts.

None of the fifteen is on `main` yet, this one included until it merges. A row's acceptance line is what must be true for it to merge; until then, nothing on this page is a claim about what Fabric does. The rest of these docs describe `main`.

| # | Pull request | What changes | Acceptance |
|---|---|---|---|
| 1 | Launch path | A placed managed job starts on the chosen node | Planned handoff, carried work package, no secret anywhere in it, proved with two local nodes |
| 2 | Real loss scenarios and the two-Mac gate on `main` | One live copy under loss and races, on two real hosts | Five loss scenarios run for real; gates G1, G2 and G3 pass on `main`; an unedited video of about 80 seconds |
| 3 | v0.1: the new name, `brew install`, `job init` in place, the CLI contract | A stranger installs in two minutes | A clean-Mac walk under 10 minutes; the CLI design rules pass; `--json` on every command; stable exit codes |
| 4 | Portable session format v1 | The internal session format becomes a public contract | A spec, fixtures, and Rust and TypeScript parsers; three readers pass the same fixtures; a draft RFC post |
| 5 | Harness adapters | Not a Claude Code and Codex tool only | For each new adapter, suspend, move and resume end to end against a fake; a fidelity table in the README |
| 6 | ACP session export and resume: a proposal and a reference | The contract enters a standard | An RFD open in the Agent Client Protocol project; a demo in one ACP client |
| 7 | A plugin API for UI integrations | A button in other people's products | One external project runs the integration |
| 8 | Workspace sync as its own crate and CLI | The best part is usable alone | A dirty tree with `node_modules` moves between two machines in seconds; a benchmark table |
| 9 | Capsule as a placement target | A job goes from a laptop to a sandbox and back, with no hosted service | A job moves Mac to sandbox to Mac with one live copy; a fixture and local Docker first, free provider tiers last, with a heads-up to Efe before any account is made or anything is spent |
| 10 | Enroll any Linux machine as a node | Your own fleet, and a relay node | A binary and a join token; a VPS relay keeps a job alive while the laptop sleeps |
| 11 | Crash-safe cleanup, gc and a residual report | A runtime that does not burn money | After `kill -9`, residual resources are found by label and removed under a fresh approval |
| 12 | Receipts and spans out: OpenTelemetry, `inspect --json`, audit export | What platform teams ask for to observe it | One real run seen in Jaeger; an audit export as JSON lines |
| 13 | Docs site, the what-moves matrix, the ADR index, the adapter guide | Writing an adapter takes a day | An outside contributor's adapter PR merges with the guide alone |
| 14 | Release train: cargo-dist, GitHub Releases, a Homebrew tap, SBOM, provenance | Trust and install friction | Every tag ships unsigned binaries for three platforms; the README explains Gatekeeper |
| 15 | Performance in CI | Speed claims are measured where everyone can see them | A paired A/B job on one runner runs on every PR, posts its table, and fails on an interval above 1; the local timing runs are retired |

Order. PRs 4, 13 and 15 and the Capsule half of 11 start first. PR 1 is under way; 2, 5 and 8 follow it. PRs 3 and 14 wait for the name. 9, 10, 11 (the Fabric half) and 12 come next, then 6 and 7.

Decided with the list:

- Performance is measured in GitHub Actions, as a paired A/B on one runner with an interval, not on a developer's Mac. Until PR 15 lands, no PR runs a local timing benchmark.
- Harness adapters cover the most used coding agents today, ranked by research first, plus pi, Cursor Agent CLI, OpenCode and Gemini CLI.
- No npm publish yet. No Apple notarization: releases are unsigned, through a Homebrew tap, and the README explains Gatekeeper.
- The product name will change; PRs 3 and 14 carry the new one.

Not in these fifteen: a public internet facade, mobile clients, a provider marketplace, hosted billing. They wait until people outside the project ask for them.

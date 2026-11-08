# The CLI contract

What a person, a script or an agent can rely on from every `fabric` command. Each rule is traced to the [Vercel CLI design guidelines](https://vercel.com/design/cli) (the guide), the section named in brackets. The rules are enforced in one module, `crates/fabric/src/contract.rs`, and held for every command in the tree by `crates/fabric/tests/cli_contract.rs`, which walks the real clap command tree, and by the e2e test `streams`, which runs every command that answers against a live node in words and under `--json` and fails on a leaf command it neither runs nor names as held elsewhere. A command added later fails both until it keeps the rules, and a reply added to the client API does not compile until `render::answer` and `render::document` say whether it is a read or a change. The help of every command is in [reference/cli.md](reference/cli.md), generated from `--help`.

This page describes the behaviour, not the names: the command vocabulary (`job new`, `node`, `ls` and the rest) is unchanged and waits for the rename.

## Streams

[Give each stream one job; Principles: stdout is the answer]

- Standard output carries only the answer. A read prints its data: `job status`, `job log`, `job ls`, `node ls`, `doctor`, `gc`, `job import --psf` and `psf inspect`. A change prints the bare identifier of what it changed or made, one line, so `id=$(fabric job new --repo . --goal "...")` holds the job id and nothing else:

  | Command | Standard output |
  |---|---|
  | `job new`, `managed create`, `managed cancel` | the job id |
  | `job suspend`, `move`, `steal`, `approve`, `reject`, `repair` | the job id |
  | `job export` | the snapshot id |
  | `job export --psf` | the document digest of the container it wrote |
  | `node add` | the node id |
  | `node id` | this node's id |
  | `gc --yes` | one row per residual it removed, as `gc` lists them |

- Everything else goes to standard error: progress (each phase of a move, a suspend, a restore, a setup step), context (`root ...`, `harness ...`, `listening on ...`), the hint of an empty list, a count of rows left out, warnings, prompts and errors.
- An empty list is success: exit 0, nothing on standard output, a first-use hint on standard error [Tables, details & empty results].
- `job run` and `job resume` hand standard output to the harness [Streams & other protocols: Child process]. A headless turn prints the harness's answer and a last line naming the session there, as before; `fabric`'s own lines are on standard error.
- `serve` runs until stopped; the node id and address it listens on are context, on standard error.
- `psf inspect` and `psf verdicts` read only files and never ask a node, so no node is missing for them; their failure is input that does not read.

## JSON

[Machine results; Machine errors]

- `--json` works on every command at any depth. Standard output then holds exactly one JSON document and a newline, indented when standard output is a terminal and on one line otherwise. Standard error stays empty on success: progress, notes and warnings are not printed, and log lines below `error` are off unless `--debug` is given.
- The document mirrors the client API (ADR 0018), the same fields the TypeScript SDK reads, keyed by the noun: `{ "job": {...} }` for a read of one, the API's own list keys (`{ "jobs": [...], "closed": [...] }`, `{ "peers": [...] }`, `{ "records": [...] }`) for a list, and `{ "job": {...}, "outcome": "<verb>" }` for a change, the outcome one of `created`, `suspended`, `moved`, `stolen`, `recorded`, `repaired`, `exported`, `added`, `canceled`, `removed` or `unchanged` (a managed command answered again for its operation key). `gc` answers with the report's own keys (`removing`, `live`, `residuals`, `kept`), with `"outcome": "removed"` under `--yes` and no outcome on a dry run.
- A failure under `--json` writes nothing to standard output and exactly one single-line JSON document to standard error, the last line there:

  ```json
  {"status":"error","reason":"no_daemon","message":"No Fabric node is running for this user.","next":[{"command":"fabric serve","argv":["fabric","serve"]}],"data":{"socket":"/tmp/fabric-try/fabric.sock"},"retryable":true}
  ```

  | Field | Rule |
  |---|---|
  | `status` | `action_required` when the command was blocked before any effect and input or a human's approval would let it run; otherwise `error` |
  | `reason` | A stable `snake_case` code: the node's own reason (`unknown_job`, `not_owner`, `preflight`, `no_snapshot`, `harness_running`, ... the registry in `fabric-client`'s `api.rs`), or the CLI's (below) |
  | `message` | The human headline; never parse it |
  | `next` | The recovery command, as text and as argv, with `when` for a step only a human may approve |
  | `data` | The reason's facts as the client API carries them, the socket, the preflight report, or a status that read in part |
  | `retryable` | `true` only when running the same command again is safe; `false` for a refusal and for a lost connection whose outcome is unknown |
  | `userActionRequired` | `true` when a human must approve: `--force` over a preflight, `--yes` for setup steps |

  The CLI's own reasons: `unknown_option`, `unexpected_argument` (a stray word no argument takes), `unknown_command`, `missing_argument`, `invalid_arguments` (usage, exit 2), `no_daemon` (exit 80), `untrusted_state`, `incompatible_node`, `unknown_outcome`, `unreadable_record`, `invalid_container` (a Portable Session Format container, manifest or fixture set that does not read), `stale_container` (exit 85: this node's head for the job is not the revision a container names), `confirmation_required`, `workspace_busy`, `node_error`, `gc_incomplete` (exit 1: a removal `fabric gc --yes` listed failed; the report is the data), `approval_required` (exit 84: a provider resource `fabric gc --yes` was given no approval for), and `failed` for any other error (exit 1).
- `job run` and `job resume` refuse `--json` as a usage error (exit 2), because the harness owns standard output. This is a recorded exception to "every command that reads or changes a resource supports it".
- Help is text under `--json` too: `fabric help --json`, `fabric job --help --json` and a bare group with `--json` print the help and exit 0. Help as JSON waits for the rename (below).
- `managed create` and `managed cancel` without `--operation-key` run under a key the CLI makes. In words it is on standard error before the request; under `--json` it is the answer's `operation.operation_key`, and every failure carries it as `data.operation_key`. When the answer was lost (`unknown_outcome`) the failure's `next` is the same command with `--operation-key <key>`, which the node answers with its first answer and never runs twice; the same command without it would make a second job.
- `--version --json` prints `{"version":"0.1.0"}`.
- Keys are the client API's `snake_case`, not the guide's `camelCase`, because the guide's first JSON rule is to mirror the API and never to rewrite its keys; the SDK and the CLI then read the same names. The error document's own fields (`userActionRequired`) follow the guide.

## Exit codes

[Exit codes; Classify the failure]

| Code | Meaning |
|---|---|
| 0 | Done, an empty result included; help; version |
| 1 | Any other failure |
| 2 | Usage error or missing input: an unknown command or option, an invalid value, a missing argument, `--json` on `run` or `resume` |
| 80 | No node answers on the control socket: none started, or a socket a stopped node left behind |
| 81 | Not the owner: another node owns the job, this node does not know it, or the plan's epoch is over |
| 82 | Frozen: a move this node started waits for its target, or the job is frozen after a fork |
| 83 | A peer could not be reached, or gave no answer in time |
| 84 | Refused without `--force` (the preflight, a steal's dry run), without `--yes` (setup steps not approved) or without an approval (a provider resource `gc --yes` may not remove) |
| 85 | The job's state stops the command |
| 128 + n | Stopped by signal n; Ctrl-C is 130 |
| the harness's code | `run` and `resume` exit with the harness's code when it exits on its own |

80 to 85 are a recorded exception to the guide's single code 1 for operational failures [Record exceptions]: they are the written contract of ADR 0016 that scripts and the triggers of ADR 0008 read, they are finer than 1, and they collide with none of 0, 1, 2, 130 and 128 + n. The test is the class table in `fabric-client`'s `Reason::kind` and `refusal_exit` in `crates/fabric/src/lib.rs`.

## Errors

[Design errors as recovery; Classify the failure]

Every failure prints in one shape on standard error:

```text
Error: No Fabric node is running for this user.
  Socket          /tmp/fabric-try/fabric.sock
  Cause           No such file or directory (os error 2)

  Start one:
    fabric serve
```

- The first line is the headline: what failed and the known cause, as one sentence. On a terminal it starts with `✗` instead of `Error:`.
- Context and identifiers follow as rows, the label in a 16-character column.
- After a blank line comes the recovery: an instruction ending in `:` and the command, indented 4 spaces, on its own line so it can be copied. A refusal whose sentence names a `fabric` command offers that command; a refusal `--force` would waive offers the same command with `--force`, for after a human approves; unapproved setup steps offer it with `--yes`.
- A usage error names the unknown option, the stray argument or the unknown command and the command path, gives clap's one suggestion as a `Did you mean` row, and points to the command's help: `Error: Unknown option --bogus for fabric job ls.` or `Error: Unexpected argument extra for fabric job ls.`, then `fabric job ls --help`.
- Text that came from a node, a peer or an agent prints with its control characters escaped, in errors as everywhere else (`write_lines`, `fabric_client::sentences::printable`) [Files, stdin & sensitive data: remote text is data].

## Modes

[Resolve modes independently]

`contract::Modes` is the one place that reads the terminals and the environment:

- Interactive, so a prompt may appear, only when none of `--json`, `--non-interactive` and a nonempty `CI` is set and standard input and standard error are terminals. Otherwise nothing prompts: the one prompt `fabric` has, for setup steps a node has not run for a job, answers no and the command fails with exit 84 and a recovery that adds `--yes`. `--json` turns prompts off and grants no consent [Principles].
- Prompts are on standard error and default to No [Ask for a real choice].
- Rich presentation (the `✗` glyph) only when standard error is a terminal and `TERM` is not `dumb`.

## Color

[Color, width & platform]

Color resolves highest first: `--no-color`, a nonempty `NO_COLOR`, `FORCE_COLOR` (`0` is off), then whether standard error is a terminal that renders it. Today the only color is the red of the error glyph or prefix; turning it off removes only the escape codes. Help follows clap's own detection and is plain under `--no-color` and `NO_COLOR`. JSON and identifiers are never styled.

## Help

[Help & discovery; Examples & suggestions]

- `fabric --help`, `fabric -h`, `fabric help` and a bare `fabric` print the root help; `fabric <path> --help`, `-h` and `fabric help <path>` print the same text at any depth. A group without an action, such as `fabric job`, prints its help, with or without options (`fabric job --json`). `fabric help` is a command like the others: it takes the global options and `-h`/`--help`. Help goes to standard output, exits 0, and does no work.
- Each help has, in order: the usage line, a one-line summary, the commands or the arguments and options (options sorted by long name), a one-line pointer to the global options in the root help, examples, and a `Learn more:` link to the command's section of the reference.
- The root help lists the commands grouped in the order a user meets them (set up a node, work on jobs, managed jobs) instead of one `Commands:` list, then the global options in full.
- Every leaf command has examples, from `contract::EXAMPLES`. They use canonical names and long options with `<UPPER_CASE>` placeholders, and the test fills each placeholder in and parses every example through the real command tree.
- Value names say what a value is: `--to <NODE>`, `--repo <PATH>`, `--goal <TEXT>`.

## Global options

[Options & parsing]

Every command takes these at any depth, before or after the command path:

| Option | Meaning |
|---|---|
| `-h, --help` | Print help |
| `-v, --version` | Print the version alone (`0.1.0`), and exit 0, even where the command's required input is missing; an unknown option or an invalid value beside it still fails |
| `--json` | One JSON document on standard output; a failure as one JSON line on standard error |
| `--non-interactive` | Never prompt; fail with what is needed instead |
| `--no-color` | Disable color |
| `-d, --debug` | Diagnostic log lines on standard error (`FABRIC_LOG` still wins) |

Options are parsed strictly: an unknown option or an extra positional fails before any work, and no command or option is matched by a prefix [Options & parsing]. Recurring inputs use one name everywhere: `--to` for a target node, `--harness` for a harness, `--force` waives a refusal, `--yes` answers a confirmation, and they are different options [Bind consent to consequences]. A command that removes anything (`gc`) only reports without `--yes`, and `tests/cli_contract.rs` holds every such command to it.

## Times

[Time, numbers & units]

`job log` prints each record's time as ISO 8601 in UTC (`2026-11-08T15:30:14.167Z`) instead of milliseconds since the epoch. The JSON keeps the client API's number.

## Left for the rename

Deliberately not in this change, because each changes a command's name, a taught spelling or a table a script may parse, and those decisions wait for the rename:

- Plural groups (`jobs`, `nodes`) with singular aliases, `list` canonical with `ls` as the alias, `create` for `job new`, and top-level workflow verbs (`fabric move`, `init`, `pair`) [Command grammar; Names, shortcuts & aliases].
- Non-terminal tables with a header row, tab-separated, and relative ages in terminal tables [Tables, details & empty results].
- A receipt block on standard error after a change (`✓ Moved`, rows, one next step) [Design the whole transcript].
- `--help --json` and `help search` [Machine-readable help & search], completion, `--cwd`, and a consent tier that types the job id for `steal` [Bind consent to consequences].
- Agent detection (`CLAUDECODE`, `CODEX_*`) as a non-interactive signal; today an agent's shell is non-interactive because its standard input is not a terminal.
- Hiding `managed` from the root help while it is experimental; today it is listed with an `(experimental)` label.
- A camelCase JSON surface, if the SDK moves to one; today both read the API's `snake_case`.
- `Warning:` as the prefix of warnings, which still print as `warning:`.

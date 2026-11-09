<!-- This page is written by scripts/docs/cli-reference.sh from `fabric --help`. Do not edit it; run the script. -->

# CLI reference

Every command of `fabric` on `main`, as its own `--help` prints it. CI regenerates this page and fails when it differs, so it says what the binary does today.

Exit codes (ADR 0016, Exit codes): 0 for success, the harness's own code when it exits, 128 plus the signal when a signal stopped it, 80 when no node answers on the control socket, 81 not the owner, 82 frozen, 83 a peer unreachable, 84 refused without `--force` or `--yes`, 85 the job's state stops the command, 2 a usage error, and 1 any other failure. See [ADR 0016](../adr/0016-node-and-cli-as-built.md) for each one, and [the CLI contract](../cli-contract.md) for the rules every command keeps: streams, `--json`, errors, help and prompts.

## `fabric`

```text
Usage: fabric [OPTIONS] <COMMAND>
Move a development job between your machines and harnesses

Set up a node:
  serve    Run the node daemon in the foreground
  node     Read this node's identity and pair it with other nodes
  doctor   Print this node's facts, or compare them with a paired node's before a move
  gc       Report what crashed or ended work left on this node; remove it with --yes

Work on jobs:
  job      Make, run, suspend, move and resume jobs
  psf      Read Portable Session Format containers

Managed jobs:
  managed  Make, cancel and read managed jobs, and the M5 operations on them (experimental)
  facade   Mint and revoke facade tokens; reach a job's authority from any of your nodes (experimental)
  inspect  Read a managed job's whole state, or one attempt's, from its authority
  audit    Export the authority's journal for an audit, as JSON lines

Global Options:
  -d, --debug            Print diagnostic detail on stderr
  -h, --help             Print help
      --json             Print the result as one JSON document on stdout, and a failure as one JSON line on stderr
      --no-color         Disable color
      --non-interactive  Never prompt; fail with the input or approval needed instead
  -v, --version          Print the version


Examples:
  Run a node on this machine
    fabric serve

  Make a job from a repository
    fabric job new --repo <PATH> --goal <TEXT>

  Move a job to a paired node
    fabric job move <JOB> --to <NODE>

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md
```

### `fabric serve`

```text
Usage: fabric serve [OPTIONS]
Run the node daemon in the foreground

Options:
      --listen <ADDR>        Where both transports listen [default: 0.0.0.0:7777]
      --otlp-endpoint <URL>  Export one OpenTelemetry span per operation as OTLP/HTTP JSON to this collector, `http://host:port` (spans go to /v1/traces). Without it, `OTEL_EXPORTER_OTLP_ENDPOINT` is read; without both, nothing is exported
      --wire <WIRE>          Which wires this node dials: QUIC then TCP+TLS, or only one of them [default: auto] [possible values: auto, quic, tcp-tls]

Global Options: See `fabric --help`

Examples:
  Run the node, listening on one port
    fabric serve --listen 0.0.0.0:7777

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-serve
```

### `fabric node`

```text
Usage: fabric node <COMMAND>
Read this node's identity and pair it with other nodes

Commands:
  id    Print this node's id and address
  add   Pair a node; give its address when this node can dial it
  ls    List the paired nodes
  ping  Ping a paired node over its session, dialing it when none is open

Global Options: See `fabric --help`

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-node
```

#### `fabric node id`

```text
Usage: fabric node id
Print this node's id and address



Global Options: See `fabric --help`

Examples:
  Read this node's id in a script
    fabric node id

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-node-id
```

#### `fabric node add`

```text
Usage: fabric node add <NODE> [ADDR]
Pair a node; give its address when this node can dial it

Arguments:
  <NODE>  The node's id, as `fabric node id` prints it there
  [ADDR]  Where it listens, `host:port`; leave it out for a node behind a NAT that dials in

Global Options: See `fabric --help`

Examples:
  Pair a node this one can dial
    fabric node add <NODE> <HOST:PORT>

  Pair a node behind a NAT that dials in
    fabric node add <NODE>

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-node-add
```

#### `fabric node ls`

```text
Usage: fabric node ls
List the paired nodes



Global Options: See `fabric --help`

Examples:
  List the paired nodes as JSON
    fabric node ls --json

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-node-ls
```

#### `fabric node ping`

```text
Usage: fabric node ping <NODE>
Ping a paired node over its session, dialing it when none is open

Arguments:
  <NODE>  The node

Global Options: See `fabric --help`

Examples:
  Ping a paired node
    fabric node ping <NODE>

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-node-ping
```

### `fabric doctor`

```text
Usage: fabric doctor [OPTIONS]
Print this node's facts, or compare them with a paired node's before a move

Options:
      --job <JOB>    The job a move would carry; only with --peer
      --peer <NODE>  The node to compare with

Global Options: See `fabric --help`

Examples:
  Print this node's facts
    fabric doctor

  Check a move before making it
    fabric doctor --peer <NODE> --job <JOB>

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-doctor
```

### `fabric gc`

```text
Usage: fabric gc [OPTIONS]
Report what crashed or ended work left on this node; remove it with --yes

Options:
      --approve <OPERATION=APPROVAL>  Remove the provider resource this create made, with an approval given for it in this run alone; repeat for each
      --profile <PROFILE>             The capability profile the worker serves
      --worker <PATH>                 The provider worker approved removals go through, such as Capsule's bridge worker
      --worker-config <PATH>          The worker's configuration file
      --yes                           Remove what the report lists; without it gc only reports

Global Options: See `fabric --help`

Examples:
  Report what crashed work left
    fabric gc

  Remove what the report lists
    fabric gc --yes

  Remove a provider resource with an approval for it
    fabric gc --yes --approve <OPERATION_APPROVAL> --worker <PATH> --profile <PROFILE>

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-gc
```

### `fabric job`

```text
Usage: fabric job <COMMAND>
Make, run, suspend, move and resume jobs

Commands:
  new      Make a job from a repository, rooted at ~/fabric/jobs/<id>/work
  run      Run the job's harness here, attached to this terminal
  suspend  Stop the harness, snapshot the workspace and record the suspend
  resume   Pull the job here if another node owns it, restore it, and run it
  move     Move the job to a paired node by the cooperative move protocol
  steal    Take the job without its owner's consent; prints what it would do without --force
  status   Print the job's state on this node
  ls       List every job this node knows
  log      Print the job's ledger
  approve  Record an approval in the ledger
  reject   Record a rejection in the ledger
  repair   Settle a job frozen after a fork by the fork rule: take the peer's chain when it wins, keeping this node's ledger and its snapshots
  export   Snapshot an orphan workspace of the job without making it the job's head, and keep the snapshot; with --psf, write the job's latest snapshot as a Portable Session Format container instead
  import   Check a Portable Session Format container against the job it names on this node, and say what this node can continue

Global Options: See `fabric --help`

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-job
```

#### `fabric job new`

```text
Usage: fabric job new [OPTIONS] --repo <PATH> --goal <TEXT>
Make a job from a repository, rooted at ~/fabric/jobs/<id>/work

Options:
      --goal <TEXT>        The goal in your words
      --harness <HARNESS>  The harness: claude, codex, opencode, pi, gemini, cursor or acp:<program> [default: claude]
      --repo <PATH>        The repository to clone into the job's root

Global Options: See `fabric --help`

Examples:
  Make a job and keep its id
    fabric job new --repo <PATH> --goal <TEXT>

  Make a job for Codex
    fabric job new --repo <PATH> --goal <TEXT> --harness codex

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-job-new
```

#### `fabric job run`

```text
Usage: fabric job run [OPTIONS] <JOB>
Run the job's harness here, attached to this terminal

Arguments:
  <JOB>  The job

Options:
      --headless       Run one turn without a terminal, print the answer and the session, and exit
      --prompt <TEXT>  The first message to send; the goal for a new session
      --yes            Run setup steps this node has not approved for the job, as the workspace now is, without asking

Global Options: See `fabric --help`

Examples:
  Run the job in this terminal
    fabric job run <JOB>

  Run one turn in a script
    fabric job run <JOB> --prompt <TEXT> --headless

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-job-run
```

#### `fabric job suspend`

```text
Usage: fabric job suspend <JOB>
Stop the harness, snapshot the workspace and record the suspend

Arguments:
  <JOB>  The job

Global Options: See `fabric --help`

Examples:
  Suspend the job here
    fabric job suspend <JOB>

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-job-suspend
```

#### `fabric job resume`

```text
Usage: fabric job resume [OPTIONS] <JOB>
Pull the job here if another node owns it, restore it, and run it

Arguments:
  <JOB>  The job

Options:
      --force              Waive the preflight refusals that can be waived when pulling
      --harness <HARNESS>  Continue under another harness: claude, codex, opencode, pi, gemini, cursor or acp:<program>
      --headless           Run one turn without a terminal, print the answer and the session, and exit
      --on <NODE>          The node that holds the job, when this node has never seen it
      --prompt <TEXT>      The first message to send
      --yes                Run setup steps this node has not approved for the job, as the workspace now is, without asking

Global Options: See `fabric --help`

Examples:
  Resume a job here
    fabric job resume <JOB>

  Pull a job from the node that holds it
    fabric job resume <JOB> --on <NODE>

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-job-resume
```

#### `fabric job move`

```text
Usage: fabric job move [OPTIONS] --to <NODE> <JOB>
Move the job to a paired node by the cooperative move protocol

Arguments:
  <JOB>  The job

Options:
      --force      Waive the preflight refusals that can be waived
      --to <NODE>  The node to move it to

Global Options: See `fabric --help`

Examples:
  Move the job to a paired node
    fabric job move <JOB> --to <NODE>

  Move it past the refusals a human waived
    fabric job move <JOB> --to <NODE> --force

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-job-move
```

#### `fabric job steal`

```text
Usage: fabric job steal [OPTIONS] <JOB>
Take the job without its owner's consent; prints what it would do without --force

Arguments:
  <JOB>  The job

Options:
      --force          Do it
      --reason <TEXT>  Why, for the ledger [default: ""]

Global Options: See `fabric --help`

Examples:
  See what a steal would do
    fabric job steal <JOB>

  Take the job when its owner is gone for good
    fabric job steal <JOB> --force --reason <TEXT>

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-job-steal
```

#### `fabric job status`

```text
Usage: fabric job status <JOB>
Print the job's state on this node

Arguments:
  <JOB>  The job

Global Options: See `fabric --help`

Examples:
  Read the job's state
    fabric job status <JOB>

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-job-status
```

#### `fabric job ls`

```text
Usage: fabric job ls
List every job this node knows



Global Options: See `fabric --help`

Examples:
  List the jobs as JSON
    fabric job ls --json

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-job-ls
```

#### `fabric job log`

```text
Usage: fabric job log <JOB>
Print the job's ledger

Arguments:
  <JOB>  The job

Global Options: See `fabric --help`

Examples:
  Read the job's ledger
    fabric job log <JOB>

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-job-log
```

#### `fabric job approve`

```text
Usage: fabric job approve [OPTIONS] <JOB>
Record an approval in the ledger

Arguments:
  <JOB>  The job

Options:
      --note <TEXT>  Your words [default: ""]

Global Options: See `fabric --help`

Examples:
  Approve the work with a note
    fabric job approve <JOB> --note <TEXT>

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-job-approve
```

#### `fabric job reject`

```text
Usage: fabric job reject [OPTIONS] <JOB>
Record a rejection in the ledger

Arguments:
  <JOB>  The job

Options:
      --note <TEXT>  Your words [default: ""]

Global Options: See `fabric --help`

Examples:
  Reject the work with a note
    fabric job reject <JOB> --note <TEXT>

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-job-reject
```

#### `fabric job repair`

```text
Usage: fabric job repair <JOB>
Settle a job frozen after a fork by the fork rule: take the peer's chain when it wins, keeping this node's ledger and its snapshots

Arguments:
  <JOB>  The job

Global Options: See `fabric --help`

Examples:
  Settle a forked job
    fabric job repair <JOB>

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-job-repair
```

#### `fabric job export`

```text
Usage: fabric job export [OPTIONS] <JOB>
Snapshot an orphan workspace of the job without making it the job's head, and keep the snapshot; with --psf, write the job's latest snapshot as a Portable Session Format container instead

Arguments:
  <JOB>  The job

Options:
      --path <PATH>  A workspace a restore set aside under orphan/ beside the job's root; the workspace at the root when left out
      --psf <DIR>    The container directory to write, which must not exist

Global Options: See `fabric --help`

Examples:
  Keep the workspace at the root as a snapshot
    fabric job export <JOB>

  Keep an orphan workspace
    fabric job export <JOB> --path <PATH>

  Write the job as a portable session and keep its digest
    fabric job export <JOB> --psf <DIR>

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-job-export
```

#### `fabric job import`

```text
Usage: fabric job import --psf <DIR>
Check a Portable Session Format container against the job it names on this node, and say what this node can continue

Options:
      --psf <DIR>  The container directory

Global Options: See `fabric --help`

Examples:
  Check a portable session against this node
    fabric job import --psf <DIR>

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-job-import
```

### `fabric psf`

```text
Usage: fabric psf <COMMAND>
Read Portable Session Format containers

Commands:
  inspect  Check a container or a manifest and print its digest, the fidelity it offers and the attachments it lacks

Global Options: See `fabric --help`

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-psf
```

#### `fabric psf inspect`

```text
Usage: fabric psf inspect <CONTAINER>
Check a container or a manifest and print its digest, the fidelity it offers and the attachments it lacks

Arguments:
  <CONTAINER>  A container directory, or a `manifest.json`

Global Options: See `fabric --help`

Examples:
  Read a portable session's digest and fidelity
    fabric psf inspect <CONTAINER>

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-psf-inspect
```

### `fabric managed`

```text
Usage: fabric managed <COMMAND>
Make, cancel and read managed jobs, and the M5 operations on them (experimental)

Commands:
  create      Make a managed job from a repository, a git revision, a snapshot, an imported directory or nothing
  cancel      Cancel a managed job, or one task of it; a live attempt is stopped first
  note        Record a decision with its reason, a question, or an answer on a job's history
  register    Register a build or test task with the program it runs under a task of a job
  checkpoint  Take a checkpoint cut of the job's live attempt; the attempt goes on
  continue    Continue the job on a newly placed attempt: a quiescence, a cut, a placement
  supersede   Take the grant from the job's unknown attempt and grant nothing; its writes are refused from then on
  recover     Recover the job's unknown attempt from an acknowledged checkpoint on another node, whatever its policy
  inspect     Everything the journal holds about a job: placements, continuations, notes, provider operations
  import      Capture a directory on this node into a snapshot a job can start from, untouched
  status      One managed job: its revision, state, tasks and attempts
  ls          Every managed job this node's authority holds [alias: list]

Global Options: See `fabric --help`

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-managed
```

#### `fabric managed create`

```text
Usage: fabric managed create [OPTIONS] --goal <TEXT> <--repo <PATH>|--git <URI>|--commit <COMMIT>|--snapshot <SNAPSHOT>|--empty|--import <PATH>>
Make a managed job from a repository, a git revision, a snapshot, an imported directory or nothing

Options:
      --acceptance <ACCEPTANCE>
          What the work must satisfy, in your words; repeatable
      --allow-fidelity <ALLOW_FIDELITY>
          A continuation fidelity to allow, by its name on the fidelity ladder; repeatable
      --commit <COMMIT>
          The exact commit, 40 or 64 lowercase hex characters
      --disk-bytes <DISK_BYTES>
          The free disk a node needs to run the job, in bytes
      --drain-budget <DRAIN_BUDGET>
          How long a quiescence may take, in milliseconds (1000 to 600000)
      --empty
          Start from nothing
      --export <EXPORT>
          A workspace path that may leave the node although it looks like a secret; repeatable
      --git <URI>
          A git URI (https, ssh, git+ssh or file) to clone; needs --commit
      --goal <TEXT>
          The goal in your words
      --harness <HARNESS>
          The harness: claude, codex, opencode, pi, gemini, cursor or acp:<program> [default: claude]
      --import <PATH>
          A directory on this node the node imports into a snapshot first, untouched
      --operation-key <KEY>
          The command's key; the same key and arguments again give the same job
      --pin <PIN>
          The one node the job may run on
      --recovery <POLICY>
          What the authority does when the job's node goes silent; block by default
      --repo <PATH>
          A repository on this node to clone into the job's workspace
      --secret <SECRETS>
          A secret the tasks need by name ([A-Z0-9_]); repeatable
      --snapshot <SNAPSHOT>
          A published snapshot to start from

Global Options: See `fabric --help`

Examples:
  Make a managed job under a key a retry can reuse
    fabric managed create --repo <PATH> --goal <TEXT> --operation-key <KEY>

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-managed-create
```

#### `fabric managed cancel`

```text
Usage: fabric managed cancel [OPTIONS] <JOB>
Cancel a managed job, or one task of it; a live attempt is stopped first

Arguments:
  <JOB>  The job

Options:
      --expected-revision <REVISION>  Refuse unless the job is at this revision
      --operation-key <KEY>           The command's key; the same key and arguments again give the same answer
      --reason <TEXT>                 Why, for the journal [default: ""]
      --task <TASK>                   The task to cancel; the whole job without it

Global Options: See `fabric --help`

Examples:
  Cancel a managed job at a known revision
    fabric managed cancel <JOB> --expected-revision <REVISION>

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-managed-cancel
```

#### `fabric managed note`

```text
Usage: fabric managed note [OPTIONS] <--question <QUESTION>|--decision <DECISION>|--answer <REVISION>> <JOB>
Record a decision with its reason, a question, or an answer on a job's history

Arguments:
  <JOB>  The job

Options:
      --answer <REVISION>             The revision of the question this answers; needs --text
      --decision <DECISION>           What was decided; needs --reason
      --expected-revision <REVISION>  Refuse unless the job is at this revision
      --operation-key <KEY>           The command's key; the same key and arguments again give the same answer
      --question <QUESTION>           A question left open
      --reason <REASON>               Why it was decided
      --text <TEXT>                   The answer's text

Global Options: See `fabric --help`

Examples:
  Record a decision and why
    fabric managed note <JOB> --decision <TEXT> --reason <TEXT>

  Leave a question open
    fabric managed note <JOB> --question <TEXT>

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-managed-note
```

#### `fabric managed register`

```text
Usage: fabric managed register [OPTIONS] --parent <PARENT> --label <LABEL> --class <CLASS> <JOB> -- <ARGV>...
Register a build or test task with the program it runs under a task of a job

Arguments:
  <JOB>      The job
  <ARGV>...  The command and its arguments, after `--`

Options:
      --class <CLASS>                 build or test
      --cwd <CWD>                     Where it runs, relative to the attempt's root; the root without it
      --env <ENV>                     A variable of the node's environment it may see, by name; repeatable
      --expected-revision <REVISION>  Refuse unless the job is at this revision
      --label <LABEL>                 Your label for it
      --operation-key <KEY>           The command's key; the same key and arguments again give the same answer
      --parent <PARENT>               The task it is registered under

Global Options: See `fabric --help`

Examples:
  Register a test task under a task of the job
    fabric managed register <JOB> --parent <TASK> --label <TEXT> --class test -- cargo test

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-managed-register
```

#### `fabric managed checkpoint`

```text
Usage: fabric managed checkpoint [OPTIONS] <JOB>
Take a checkpoint cut of the job's live attempt; the attempt goes on

Arguments:
  <JOB>  The job

Options:
      --expected-revision <REVISION>  Refuse unless the job is at this revision
      --operation-key <KEY>           The command's key; the same key and arguments again give the same answer

Global Options: See `fabric --help`

Examples:
  Cut the live attempt and let it go on
    fabric managed checkpoint <JOB>

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-managed-checkpoint
```

#### `fabric managed continue`

```text
Usage: fabric managed continue [OPTIONS] <JOB>
Continue the job on a newly placed attempt: a quiescence, a cut, a placement

Arguments:
  <JOB>  The job

Options:
      --allow-fidelity <ALLOW_FIDELITY>
          A fidelity this continuation may achieve; repeatable; the job's allowance without it
      --expected-revision <REVISION>
          Refuse unless the job is at this revision
      --operation-key <KEY>
          The command's key; the same key and arguments again give the same answer
      --pin <PIN>
          The one node the new attempt may run on

Global Options: See `fabric --help`

Examples:
  Continue the job on a named node
    fabric managed continue <JOB> --pin <NODE>

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-managed-continue
```

#### `fabric managed supersede`

```text
Usage: fabric managed supersede [OPTIONS] <JOB>
Take the grant from the job's unknown attempt and grant nothing; its writes are refused from then on

Arguments:
  <JOB>  The job

Options:
      --attempt <ATTEMPT>             The unknown attempt; the job's live attempt without it
      --expected-revision <REVISION>  Refuse unless the job is at this revision
      --operation-key <KEY>           The command's key; the same key and arguments again give the same answer
      --reason <TEXT>                 Why, for the history [default: "superseded by the operator"]

Global Options: See `fabric --help`

Examples:
  Take the grant from the job's unknown attempt
    fabric managed supersede <JOB> --reason <TEXT>

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-managed-supersede
```

#### `fabric managed recover`

```text
Usage: fabric managed recover [OPTIONS] <JOB>
Recover the job's unknown attempt from an acknowledged checkpoint on another node, whatever its policy

Arguments:
  <JOB>  The job

Options:
      --attempt <ATTEMPT>             The unknown attempt; the job's live attempt without it
      --expected-revision <REVISION>  Refuse unless the job is at this revision
      --from <SNAPSHOT>               The acknowledged checkpoint to start from; the job's newest without it
      --operation-key <KEY>           The command's key; the same key and arguments again give the same answer
      --reason <TEXT>                 Why, for the history [default: "recovered by the operator"]

Global Options: See `fabric --help`

Examples:
  Recover the unknown attempt from the newest acknowledged checkpoint
    fabric managed recover <JOB>

  Recover a named attempt from a named checkpoint
    fabric managed recover <JOB> --attempt <ATTEMPT> --from <SNAPSHOT>

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-managed-recover
```

#### `fabric managed inspect`

```text
Usage: fabric managed inspect <JOB>
Everything the journal holds about a job: placements, continuations, notes, provider operations

Arguments:
  <JOB>  The job

Global Options: See `fabric --help`

Examples:
  Read the job's placements and continuations
    fabric managed inspect <JOB>

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-managed-inspect
```

#### `fabric managed import`

```text
Usage: fabric managed import [OPTIONS] <DIR>
Capture a directory on this node into a snapshot a job can start from, untouched

Arguments:
  <DIR>  The directory

Options:
      --export <EXPORT>  A path that may leave although it looks like a secret; repeatable

Global Options: See `fabric --help`

Examples:
  Capture a directory into a snapshot
    fabric managed import <PATH>

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-managed-import
```

#### `fabric managed status`

```text
Usage: fabric managed status <JOB>
One managed job: its revision, state, tasks and attempts

Arguments:
  <JOB>  The job

Global Options: See `fabric --help`

Examples:
  Read a managed job
    fabric managed status <JOB>

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-managed-status
```

#### `fabric managed ls`

```text
Usage: fabric managed ls
Every managed job this node's authority holds



Global Options: See `fabric --help`

Examples:
  List the managed jobs
    fabric managed ls

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-managed-ls
```

### `fabric facade`

```text
Usage: fabric facade <COMMAND>
Mint and revoke facade tokens; reach a job's authority from any of your nodes (experimental)

Commands:
  mint        Mint a token on this node's authority; the bearer is printed once
  revoke      Revoke a token by its id
  submit      Submit a job to an authority
  inspect     Inspect a job at its authority
  watch       A page of a job's events after a cursor
  cancel      Cancel a job or one task at its authority
  checkpoint  Checkpoint a job at its authority
  continue    Continue a job at its authority

Global Options: See `fabric --help`

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-facade
```

#### `fabric facade mint`

```text
Usage: fabric facade mint [OPTIONS] --actor <ACTOR> <--ttl-ms <TTL_MS>|--expires-ms <EXPIRES_MS>>
Mint a token on this node's authority; the bearer is printed once

Options:
      --actor <ACTOR>            Who acts under the token: 1 to 64 characters of [A-Za-z0-9._-]
      --expires-ms <EXPIRES_MS>  When the token stops working, in Unix milliseconds
      --job <JOBS>               A job the token may act on; repeatable; every owned job without it
      --node <NODES>             A node the token may pin or place on; repeatable; any node without it
      --ttl-ms <TTL_MS>          How long the token works from now, in milliseconds

Global Options: See `fabric --help`

Examples:
  Mint a token that works for an hour
    fabric facade mint --actor <ACTOR> --ttl-ms 3600000

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-facade-mint
```

#### `fabric facade revoke`

```text
Usage: fabric facade revoke <TOKEN_ID>
Revoke a token by its id

Arguments:
  <TOKEN_ID>  The token

Global Options: See `fabric --help`

Examples:
  Revoke a token
    fabric facade revoke <TOKEN_ID>

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-facade-revoke
```

#### `fabric facade submit`

```text
Usage: fabric facade submit [OPTIONS] --authority <AUTHORITY> --token <TOKEN> --goal <TEXT> <--empty|--snapshot <SNAPSHOT>|--git <URI>|--commit <COMMIT>>
Submit a job to an authority

Options:
      --acceptance <ACCEPTANCE>
          What the work must satisfy, in your words; repeatable
      --allow-fidelity <ALLOW_FIDELITY>
          A continuation fidelity to allow, by its name on the fidelity ladder; repeatable
      --authority <AUTHORITY>
          The authority node; this node's own id is allowed
      --commit <COMMIT>
          The exact commit, 40 or 64 lowercase hex characters
      --disk-bytes <DISK_BYTES>
          The free disk a node needs to run the job, in bytes
      --drain-budget <DRAIN_BUDGET>
          How long a quiescence may take, in milliseconds (1000 to 600000)
      --empty
          Start from nothing
      --export <EXPORT>
          A workspace path that may leave the node although it looks like a secret; repeatable
      --git <URI>
          A git URI (https, ssh, git+ssh or file) to clone; needs --commit
      --goal <TEXT>
          The goal in your words
      --harness <HARNESS>
          The harness: claude, codex or acp:<program> [default: claude]
      --operation-key <KEY>
          The command's key; the same key and arguments again give the same answer
      --pin <PIN>
          The one node the job may run on
      --recovery <POLICY>
          What the authority does when the job's node goes silent; block by default
      --secret <SECRETS>
          A secret the tasks need by name ([A-Z0-9_]); repeatable
      --snapshot <SNAPSHOT>
          A published snapshot to start from
      --token <TOKEN>
          The bearer, 64 lowercase hex characters, as `fabric facade mint` printed it

Global Options: See `fabric --help`

Examples:
  Submit a job from nothing to an authority
    fabric facade submit --authority <NODE> --token <BEARER> --empty --goal <TEXT>

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-facade-submit
```

#### `fabric facade inspect`

```text
Usage: fabric facade inspect --authority <AUTHORITY> --token <TOKEN> <JOB>
Inspect a job at its authority

Arguments:
  <JOB>  The job

Options:
      --authority <AUTHORITY>  The authority node; this node's own id is allowed
      --token <TOKEN>          The bearer, 64 lowercase hex characters, as `fabric facade mint` printed it

Global Options: See `fabric --help`

Examples:
  Inspect a job at its authority
    fabric facade inspect --authority <NODE> --token <BEARER> <JOB>

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-facade-inspect
```

#### `fabric facade watch`

```text
Usage: fabric facade watch [OPTIONS] --authority <AUTHORITY> --token <TOKEN> <JOB>
A page of a job's events after a cursor

Arguments:
  <JOB>  The job

Options:
      --after <AFTER>          The last event sequence you have; the first event without it
      --authority <AUTHORITY>  The authority node; this node's own id is allowed
      --token <TOKEN>          The bearer, 64 lowercase hex characters, as `fabric facade mint` printed it
      --wait-ms <WAIT_MS>      How long the authority waits for a first event, in milliseconds (0 to 25000)

Global Options: See `fabric --help`

Examples:
  Wait up to 20 seconds for the job's next events
    fabric facade watch --authority <NODE> --token <BEARER> <JOB> --wait-ms 20000

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-facade-watch
```

#### `fabric facade cancel`

```text
Usage: fabric facade cancel [OPTIONS] --authority <AUTHORITY> --token <TOKEN> <JOB>
Cancel a job or one task at its authority

Arguments:
  <JOB>  The job

Options:
      --authority <AUTHORITY>         The authority node; this node's own id is allowed
      --expected-revision <REVISION>  Refuse unless the job is at this revision
      --operation-key <KEY>           The command's key; the same key and arguments again give the same answer
      --reason <TEXT>                 Why [default: ""]
      --task <TASK>                   The task; the whole job without it
      --token <TOKEN>                 The bearer, 64 lowercase hex characters, as `fabric facade mint` printed it

Global Options: See `fabric --help`

Examples:
  Cancel a job at its authority
    fabric facade cancel --authority <NODE> --token <BEARER> <JOB>

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-facade-cancel
```

#### `fabric facade checkpoint`

```text
Usage: fabric facade checkpoint [OPTIONS] --authority <AUTHORITY> --token <TOKEN> <JOB>
Checkpoint a job at its authority

Arguments:
  <JOB>  The job

Options:
      --authority <AUTHORITY>         The authority node; this node's own id is allowed
      --expected-revision <REVISION>  Refuse unless the job is at this revision
      --operation-key <KEY>           The command's key; the same key and arguments again give the same answer
      --token <TOKEN>                 The bearer, 64 lowercase hex characters, as `fabric facade mint` printed it

Global Options: See `fabric --help`

Examples:
  Checkpoint a job at its authority
    fabric facade checkpoint --authority <NODE> --token <BEARER> <JOB>

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-facade-checkpoint
```

#### `fabric facade continue`

```text
Usage: fabric facade continue [OPTIONS] --authority <AUTHORITY> --token <TOKEN> <JOB>
Continue a job at its authority

Arguments:
  <JOB>  The job

Options:
      --allow-fidelity <ALLOW_FIDELITY>
          A fidelity this continuation may achieve; repeatable
      --authority <AUTHORITY>
          The authority node; this node's own id is allowed
      --expected-revision <REVISION>
          Refuse unless the job is at this revision
      --operation-key <KEY>
          The command's key; the same key and arguments again give the same answer
      --pin <PIN>
          The one node the new attempt may run on
      --token <TOKEN>
          The bearer, 64 lowercase hex characters, as `fabric facade mint` printed it

Global Options: See `fabric --help`

Examples:
  Continue a job on a named node
    fabric facade continue --authority <NODE> --token <BEARER> <JOB> --pin <NODE>

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-facade-continue
```

### `fabric inspect`

```text
Usage: fabric inspect [OPTIONS] <JOB>
Read a managed job's whole state, or one attempt's, from its authority

Arguments:
  <JOB>  The managed job

Options:
      --attempt <ATTEMPT>  Narrow the attempts, generations, evidence, provider operations and last report to this attempt of the job

Global Options: See `fabric --help`

Examples:
  Read a managed job's whole state as JSON
    fabric inspect <JOB> --json

  Read one attempt of the job
    fabric inspect <JOB> --attempt <ATTEMPT> --json

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-inspect
```

### `fabric audit`

```text
Usage: fabric audit <COMMAND>
Export the authority's journal for an audit, as JSON lines

Commands:
  export  Export the journal's operations, events, evidence and provider operations as JSON lines, oldest first; `--json` wraps them as `{"records":[...]}`

Global Options: See `fabric --help`

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-audit
```

#### `fabric audit export`

```text
Usage: fabric audit export [OPTIONS]
Export the journal's operations, events, evidence and provider operations as JSON lines, oldest first; `--json` wraps them as `{"records":[...]}`

Options:
      --since <TIME>  The window's start, inclusive: an RFC 3339 time, or a date, which is that day's midnight UTC
      --until <TIME>  The window's end, exclusive: an RFC 3339 time, or a date, which is that day's midnight UTC, so the day itself is left out

Global Options: See `fabric --help`

Examples:
  Export one week of the journal as JSON lines
    fabric audit export --since 2026-11-01 --until 2026-11-08

  Export everything as one JSON document
    fabric audit export --json

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-audit-export
```

### `fabric help`

```text
Usage: fabric help [COMMAND]...
Print the help of a command

Arguments:
  [COMMAND]...  The command, such as `job move`

Global Options: See `fabric --help`

Examples:
  Print the help of a command
    fabric help job move

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-help
```


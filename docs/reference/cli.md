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

Work on jobs:
  job      Make, run, suspend, move and resume jobs
  psf      Read Portable Session Format containers

Managed jobs:
  managed  Make, cancel and read managed jobs (experimental)

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
      --listen <ADDR>  Where both transports listen [default: 0.0.0.0:7777]
      --wire <WIRE>    Which wires this node dials: QUIC then TCP+TLS, or only one of them [default: auto] [possible values: auto, quic, tcp-tls]

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
      --harness <HARNESS>  The harness: claude, codex or acp:<program> [default: claude]
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
      --harness <HARNESS>  Continue under another harness: claude, codex or acp:<program>
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
Make, cancel and read managed jobs (experimental)

Commands:
  create  Make a managed job from a repository
  cancel  Cancel a managed job none of whose attempts may still run
  status  One managed job: its revision, state, tasks and attempts
  ls      Every managed job this node's authority holds [alias: list]

Global Options: See `fabric --help`

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-managed
```

#### `fabric managed create`

```text
Usage: fabric managed create [OPTIONS] --repo <PATH> --goal <TEXT>
Make a managed job from a repository

Options:
      --goal <TEXT>          The goal in your words
      --harness <HARNESS>    The harness: claude, codex or acp:<program> [default: claude]
      --operation-key <KEY>  The command's key; the same key and arguments again give the same job
      --repo <PATH>          The repository to clone into the job's workspace

Global Options: See `fabric --help`

Examples:
  Make a managed job under a key a retry can reuse
    fabric managed create --repo <PATH> --goal <TEXT> --operation-key <KEY>

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-managed-create
```

#### `fabric managed cancel`

```text
Usage: fabric managed cancel [OPTIONS] <JOB>
Cancel a managed job none of whose attempts may still run

Arguments:
  <JOB>  The job

Options:
      --expected-revision <REVISION>  Refuse unless the job is at this revision
      --operation-key <KEY>           The command's key; the same key and arguments again give the same answer
      --reason <TEXT>                 Why, for the journal [default: ""]

Global Options: See `fabric --help`

Examples:
  Cancel a managed job at a known revision
    fabric managed cancel <JOB> --expected-revision <REVISION>

Learn more: https://github.com/EfeDurmaz16/fabric/blob/main/docs/reference/cli.md#fabric-managed-cancel
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


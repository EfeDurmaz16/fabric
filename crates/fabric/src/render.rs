//! The human text of the CLI: one line per event, and the lines of each reply.
//!
//! Invariant: the node answers with data (ADR 0018) and the CLI turns it
//! into the words `fabric` prints, so a script that reads the CLI's output
//! and a program that reads the client API see the same facts. An event's
//! line comes from `fabric_client::sentences`, which the node's log uses
//! too; a reply's lines are made here. Each line is the one the node
//! printed before the client API existed.

use fabric_client::api::{
    DoctorReport, JobStatus, LedgerEvent, Marker, PreflightRow, Process, Registered, Reply,
    SessionRecorded, View, WorkspaceState,
};
use fabric_client::sentences::row;

/// The lines an event prints; none for a kind this CLI does not know.
pub(crate) use fabric_client::sentences::event;

/// The lines a reply prints; a run plan and a suspend's, move's or
/// steal's lease print none, since their events said it all.
#[expect(
    clippy::too_many_lines,
    reason = "one arm per reply kind, so every line the CLI prints reads in one place"
)]
pub(crate) fn reply(reply: &Reply) -> Vec<String> {
    match reply {
        Reply::NodeId(identity) => vec![
            identity.node.to_string(),
            format!("listening on {}", identity.listen),
        ],
        Reply::NodeAdd(paired) => vec![match &paired.addr {
            Some(addr) => format!("paired {} at {addr}", paired.node),
            None => format!(
                "paired {} with no address: it may dial this node, and this node never dials it",
                paired.node
            ),
        }],
        Reply::NodeList(list) if list.peers.is_empty() => {
            vec![
                "no paired nodes; pair one with `fabric node add <node-id> <host:port>`".to_owned(),
            ]
        }
        Reply::NodeList(list) => list
            .peers
            .iter()
            .map(|peer| {
                format!(
                    "{} {} ({})",
                    peer.node,
                    if peer.addrs.is_empty() {
                        "-".to_owned()
                    } else {
                        peer.addrs.join(",")
                    },
                    if peer.connected {
                        "connected"
                    } else {
                        "not connected"
                    }
                )
            })
            .collect(),
        Reply::NodePing(pong) => vec![format!(
            "{} answered in {} ms over {}; its clock is {} ms from this one",
            pong.node, pong.rtt_ms, pong.wire, pong.clock_skew_ms
        )],
        Reply::JobNew(created) => vec![
            created.job.to_string(),
            format!(
                "root {}: a clone of {} under ~/fabric/jobs, the same path on every node with this home (ADR 0011)",
                created.root.display(),
                created.repo.display()
            ),
            format!(
                "harness {}; this node owns the job at epoch {}",
                created.harness, created.epoch
            ),
        ],
        Reply::JobList(list) if list.jobs.is_empty() && list.closed.is_empty() => {
            vec!["no jobs; make one with `fabric job new --repo <path> --goal <text>`".to_owned()]
        }
        Reply::JobList(list) => list
            .jobs
            .iter()
            .map(|job| {
                format!(
                    "{} {} epoch {} {}",
                    job.job, job.standing, job.epoch, job.goal
                )
            })
            .chain(
                list.closed
                    .iter()
                    .map(|closed| format!("{} closed: {}", closed.job, closed.error)),
            )
            .collect(),
        Reply::JobStatus(status) => job_status(status),
        Reply::JobLog(log) => {
            let left_out = usize::try_from(log.left_out).unwrap_or(usize::MAX);
            let cut = (left_out > 0).then(|| {
                format!(
                    "the first {left_out} records are left out; the latest {} fill one answer",
                    log.records.len()
                )
            });
            cut.into_iter()
                .chain(log.records.iter().enumerate().map(|(n, record)| {
                    format!(
                        "{:>4} epoch {} at {} {}",
                        left_out.saturating_add(n),
                        record.epoch,
                        record.at,
                        ledger_event(&record.event)
                    )
                }))
                .collect()
        }
        Reply::JobDecide(decided) => vec![format!("recorded: {}", decided.text)],
        Reply::Doctor {
            report: DoctorReport::Facts { facts },
        } => facts
            .iter()
            .map(|fact| format!("{}: {}", fact.name, fact.value.replace('\n', " / ")))
            .collect(),
        Reply::Doctor {
            report: DoctorReport::Compared { rows, .. },
        } => rows.iter().map(row).collect(),
        Reply::HarnessStarted(registered) => vec![self::registered(registered)],
        Reply::HarnessSession(recorded) => vec![session_recorded(recorded)],
        Reply::JobRun(_)
        | Reply::JobResume(_)
        | Reply::JobSuspend(_)
        | Reply::JobMove(_)
        | Reply::JobSteal(_)
        | Reply::HarnessExited => Vec::new(),
    }
}

/// The line for a launch the node recorded.
pub(crate) fn registered(registered: &Registered) -> String {
    format!(
        "Resumed recorded at epoch {}; {} runs as pid {}",
        registered.epoch, registered.harness, registered.pid
    )
}

/// The line for a session the node recorded for a running harness.
pub(crate) fn session_recorded(recorded: &SessionRecorded) -> String {
    format!("recorded {} session {}", recorded.harness, recorded.session)
}

/// The facts of the rows that stop a move made without `--force`.
pub(crate) fn blocking(rows: &[PreflightRow]) -> Vec<&str> {
    rows.iter()
        .filter(|row| row.outcome.blocks(false))
        .map(|row| row.fact.as_str())
        .collect()
}

/// The sentence of the first part of `status` the node could not read, in
/// the order its lines print. The CLI prints the lines before it and then
/// fails with it, as it did when the node itself stopped there.
pub(crate) fn first_unreadable(status: &JobStatus) -> Option<&str> {
    if let View::Unreadable { error } = &status.view {
        return Some(error);
    }
    if let Some(error) = &status.workspace_unreadable {
        return Some(error);
    }
    if let Process::Unreadable { error } = &status.process {
        return Some(error);
    }
    None
}

/// The lines of `status`, up to the first part the node could not read.
fn job_status(status: &JobStatus) -> Vec<String> {
    let id = status.job;
    let mut lines = vec![
        format!("job {id}"),
        format!("goal: {}", status.goal),
        format!("harness: {}", status.harness),
        format!(
            "owner: {}{} at epoch {}",
            status.owner,
            if status.owner == status.node {
                " (this node)"
            } else {
                ""
            },
            status.epoch
        ),
    ];
    lines.push(match &status.view {
        View::Unreadable { .. } => return lines,
        View::Frozen { peer, epoch } => format!(
            "this node: frozen; {peer}'s ledger at epoch {epoch} does not continue this node's, so the job was stolen while this node ran it. This copy never runs again; run `fabric job resume {id}` on {peer} to continue there"
        ),
        View::Owner => "this node: owner".to_owned(),
        View::Departing { move_id, target } => format!(
            "this node: departing to {target} under move {move_id}; frozen until {target} answers. Run `fabric job move {id} --to {target}` to ask again, or `fabric job steal {id} --force` on another node"
        ),
        View::Departed { owner, epoch } => format!(
            "this node: departed; {owner} owns the job at epoch {epoch} and this copy never runs"
        ),
        View::Replica { owner, .. } => {
            format!("this node: replica; run `fabric job resume {id}` to pull it from {owner}")
        }
    });
    lines.push(match &status.snapshot {
        None => "snapshot: none yet".to_owned(),
        Some(snapshot) if snapshot.suspended => {
            format!("snapshot: {} (suspended in it)", snapshot.id)
        }
        Some(snapshot) => format!("snapshot: {} (resumed since)", snapshot.id),
    });
    if status.workspace_unreadable.is_some() {
        return lines;
    }
    lines.push(match &status.workspace {
        None => "workspace: none on this node".to_owned(),
        Some(workspace) => {
            let root = workspace.root.display();
            match &workspace.state {
                WorkspaceState::Current { marker } => {
                    format!("workspace: {root} ({})", marker_words(marker))
                }
                WorkspaceState::Stale { left_at } => format!(
                    "workspace: {root} was left at epoch {left_at}, before this node's tenure; `fabric job resume {id}` sets it aside and restores the latest snapshot"
                ),
                WorkspaceState::AwaitingDecision { target } => format!(
                    "workspace: {root} is clean at the snapshot offered to {target} and waits for its decision"
                ),
                WorkspaceState::CleanCopy => format!(
                    "workspace: {root} is clean at a published snapshot; this node never runs it and moves it aside when it next settles the job"
                ),
                WorkspaceState::Orphan => format!(
                    "workspace: {root} is an orphan: it may hold changes made after the last snapshot this node published, and nothing here deletes it"
                ),
            }
        }
    });
    lines.push(match &status.process {
        Process::Unreadable { .. } => return lines,
        Process::Running { pid, epoch } => {
            format!("harness: running as pid {pid} since epoch {epoch}")
        }
        Process::GroupRemains { pid } => format!(
            "harness: pid {pid} exited and processes it started still run in its group; `fabric job suspend {id}` stops them"
        ),
        Process::NotRunning => "harness: not running".to_owned(),
    });
    lines.push(format!("records: {}", status.records));
    lines
}

fn marker_words(marker: &Marker) -> String {
    match marker {
        Marker::Live => "live: the harness may have changed it since the last snapshot".to_owned(),
        Marker::Suspending => "a suspend did not finish".to_owned(),
        Marker::Clean { snapshot } => format!("clean at snapshot {snapshot}"),
        Marker::Restoring { snapshot } => format!("a restore of {snapshot} did not finish"),
    }
}

fn ledger_event(event: &LedgerEvent) -> String {
    match event {
        LedgerEvent::Created {
            created_by,
            harness,
            goal,
        } => format!("Created by {created_by} under {harness}: {goal}"),
        LedgerEvent::MoveStarted { move_id, target } => {
            format!("MoveStarted {move_id} toward {target}")
        }
        LedgerEvent::MoveImported { move_id, from } => {
            format!("MoveImported {move_id} from {from}")
        }
        LedgerEvent::MoveAborted { move_id } => format!("MoveAborted {move_id}"),
        LedgerEvent::Stolen { by, from, reason } => format!("Stolen by {by} from {from}: {reason}"),
        LedgerEvent::SnapshotPublished { snapshot, .. } => format!("SnapshotPublished {snapshot}"),
        LedgerEvent::Suspended { snapshot } => format!("Suspended in {snapshot}"),
        LedgerEvent::Resumed { node, harness, .. } => format!("Resumed on {node} under {harness}"),
        LedgerEvent::EnvSpecChanged { env_spec } => format!("EnvSpecChanged {env_spec}"),
        LedgerEvent::DecisionRecorded { text } => format!("DecisionRecorded: {text}"),
        LedgerEvent::Note { text } => format!("Note: {text}"),
        LedgerEvent::Unrecognized => "a record of a kind this fabric does not know".to_owned(),
    }
}

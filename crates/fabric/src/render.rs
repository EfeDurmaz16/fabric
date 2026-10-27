//! The human text of the CLI: one line per event, and the lines of each reply.
//!
//! Invariant: the node answers with data (ADR 0018) and the CLI turns it
//! into the words `fabric` prints, so a script that reads the CLI's output
//! and a program that reads the client API see the same facts. An event's
//! line comes from `fabric_client::sentences`, which the node's log uses
//! too; a reply's lines are made here. Each line is the one the node
//! printed before the client API existed.
//!
//! The lines hold text as the node sent it: a goal, a reason, a decision,
//! a note, an ACP harness's arguments, a peer's facts and paths from a
//! snapshot are in the words of whoever wrote them. They print through
//! `write_lines`, which escapes their control characters, so this module
//! never escapes a field itself.

use fabric_client::api::{
    CarryReport, DoctorReport, Event, JobStatus, LedgerEvent, Marker, PreflightRow, Process,
    Registered, Reply, SessionRecorded, SnapshotSession, View, WorkspaceState,
};
use fabric_client::sentences::{NO_SESSION, carry_lines, named, row};

/// The lines an event prints: none for a kind this CLI does not know, and
/// none for the session a launch continues, which the CLI says itself once
/// the harness has started, with the settings the launch carried from the
/// session (gate finding F10).
pub(crate) fn event(event: &Event) -> Vec<String> {
    match event {
        Event::ContinuingSession { .. } => Vec::new(),
        _ => fabric_client::sentences::event(event),
    }
}

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
        Reply::NodeList(list) if list.peers.is_empty() && list.left_out == 0 => {
            vec![
                "no paired nodes; pair one with `fabric node add <node-id> <host:port>`".to_owned(),
            ]
        }
        Reply::NodeList(list) => (list.left_out > 0)
            .then(|| {
                format!(
                    "the first {} paired nodes are left out; the other {} fill one answer",
                    list.left_out,
                    list.peers.len()
                )
            })
            .into_iter()
            .chain(list.peers.iter().map(|peer| {
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
            }))
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
        Reply::JobList(list)
            if list.jobs.is_empty() && list.closed.is_empty() && list.left_out == 0 =>
        {
            vec!["no jobs; make one with `fabric job new --repo <path> --goal <text>`".to_owned()]
        }
        Reply::JobList(list) => (list.left_out > 0)
            .then(|| {
                format!(
                    "the {} oldest jobs are left out; the latest {} fill one answer, and `fabric job status <job>` shows any job",
                    list.left_out,
                    list.jobs.len() + list.closed.len()
                )
            })
            .into_iter()
            .chain(list.jobs.iter().map(|job| {
                format!(
                    "{} {} epoch {} {}",
                    job.job, job.standing, job.epoch, job.goal
                )
            }))
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
            report: DoctorReport::Compared { peer, rows, carry },
        } => rows
            .iter()
            .map(row)
            .chain(carry.iter().flat_map(|carry| carry_report(*peer, carry)))
            .collect(),
        Reply::HarnessStarted(registered) => vec![self::registered(registered)],
        Reply::HarnessSession(recorded) => vec![session_recorded(recorded)],
        Reply::ManagedCreate(operation) | Reply::ManagedCancel(operation) => {
            let mut lines = fabric_client::sentences::managed_job(&operation.job);
            if operation.replayed {
                lines.push(format!(
                    "operation {} answered again for key {}",
                    operation.operation, operation.operation_key
                ));
            }
            lines
        }
        Reply::ManagedStatus(job) => fabric_client::sentences::managed_job(job),
        Reply::ManagedList(list) => list
            .jobs
            .iter()
            .flat_map(fabric_client::sentences::managed_job)
            .collect(),
        Reply::JobRun(_)
        | Reply::JobResume(_)
        | Reply::JobSuspend(_)
        | Reply::JobMove(_)
        | Reply::JobSteal(_)
        | Reply::HarnessExited
        | Reply::LaunchCheck => Vec::new(),
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

/// What `peer` would do with each ignored path of a job's latest snapshot.
fn carry_report(peer: fabric_core::id::NodeId, carry: &CarryReport) -> Vec<String> {
    let Some(snapshot) = carry.snapshot else {
        return vec![
            "no snapshot yet; a move decides each ignored path once it has suspended the job"
                .to_owned(),
        ];
    };
    let header = format!(
        "what {peer} would do with each ignored path of snapshot {snapshot}; a move decides again on the snapshot it takes:"
    );
    let mut lines: Vec<String> = std::iter::once(header)
        .chain(carry_lines(&carry.paths, &carry.more, false))
        .collect();
    if !carry.session_captured {
        lines.push(NO_SESSION.to_owned());
    }
    lines
}

/// Why `doctor --peer` fails as a preflight would: the rows that stop a
/// move made without `--force`, else the paths the peer can neither use
/// nor make again; `None` when a move would go ahead.
pub(crate) fn refused_move(rows: &[PreflightRow], carry: Option<&CarryReport>) -> Option<String> {
    let blocking = blocking(rows);
    let what = if blocking.is_empty() {
        let carry = carry.filter(|carry| !carry.refused.is_empty())?;
        format!(
            "the target can neither use nor rebuild {}",
            named(&carry.refused, carry.refused_more)
        )
    } else {
        blocking.join(", ")
    };
    Some(format!(
        "the preflight refused the move: {what}; `fabric job move` and `fabric job resume` take --force to waive what can be waived"
    ))
}

/// The facts of the rows that stop a move made without `--force`.
fn blocking(rows: &[PreflightRow]) -> Vec<&str> {
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
    if let Some(snapshot) = &status.snapshot
        && let Some(session) = &snapshot.session
    {
        let id = snapshot.id;
        lines.push(match session {
            SnapshotSession::Captured => format!("session: captured in snapshot {id}"),
            SnapshotSession::NotCaptured => format!("session: {NO_SESSION}"),
            SnapshotSession::Unreadable { error } => {
                format!("session: snapshot {id} could not be read: {error}")
            }
        });
    }
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
        Process::Unconfirmed { pid } => format!(
            "harness: pid {pid} leads a process group and its start time could not be read, so whether it is still the harness is not known"
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

#[cfg(test)]
mod tests {
    use fabric_client::api::Outcome;

    use super::*;

    /// A goal, a reason, a decision, a note or an ACP harness's arguments
    /// are in the words of the node that wrote the record, a paired peer
    /// included, and `job status`, `job ls` and `job log` print them. An
    /// ANSI or OSC sequence, a C1 control or a line break among them must
    /// print as visible escapes, never reach the terminal (Codex review of
    /// #8, issue #23). The lines escape where they are written
    /// (`write_lines`), as every line the CLI prints does (review of PR
    /// #27).
    #[test]
    fn ledger_text_prints_with_its_control_characters_escaped() {
        use fabric_client::api::{JobList, JobLog, JobSummary, LogRecord, Standing};
        use fabric_core::harness::{HarnessKind, ProgramName};
        use fabric_core::id::{Epoch, EventId, JobId, NodeId};

        let hostile = "fix\u{1b}]0;owned\u{7}\u{1b}[2J\u{9b}31m\r\nowner: forged";
        let escaped = r"fix\u{1b}]0;owned\u{7}\u{1b}[2J\u{9b}31m\r\nowner: forged";
        let job = JobId::from_bytes([1; 16]);
        let node = NodeId::from_bytes([2; 32]);
        let harness = HarnessKind::Acp {
            program: ProgramName::parse("agent").unwrap(),
            args: vec![hostile.to_owned()],
        };
        let status = JobStatus {
            job,
            node,
            goal: hostile.to_owned(),
            harness: harness.clone(),
            owner: node,
            epoch: Epoch::FIRST,
            view: View::Owner,
            snapshot: None,
            workspace: None,
            workspace_unreadable: None,
            process: Process::NotRunning,
            records: 5,
        };
        let list = JobList {
            jobs: vec![JobSummary {
                job,
                standing: Standing::Owner,
                epoch: Epoch::FIRST,
                goal: hostile.to_owned(),
            }],
            closed: Vec::new(),
            left_out: 0,
        };
        let record = |event| LogRecord {
            id: EventId::from_bytes([3; 16]),
            epoch: Epoch::FIRST,
            at: 0,
            event,
        };
        let log = JobLog {
            left_out: 0,
            records: vec![
                record(LedgerEvent::Created {
                    created_by: node,
                    harness: harness.clone(),
                    goal: hostile.to_owned(),
                }),
                record(LedgerEvent::Resumed {
                    node,
                    snapshot: None,
                    harness,
                }),
                record(LedgerEvent::Stolen {
                    by: node,
                    from: node,
                    reason: hostile.to_owned(),
                }),
                record(LedgerEvent::DecisionRecorded {
                    text: hostile.to_owned(),
                }),
                record(LedgerEvent::Note {
                    text: hostile.to_owned(),
                }),
            ],
        };
        for (answer, holding) in [
            (Reply::JobStatus(Box::new(status)), 2),
            (Reply::JobList(list), 1),
            (Reply::JobLog(log), 5),
        ] {
            let mut printed = Vec::new();
            crate::write_lines(&mut printed, &reply(&answer)).unwrap();
            let printed = String::from_utf8(printed).unwrap();
            let lines: Vec<&str> = printed.lines().collect();
            assert!(
                lines.iter().all(|line| !line.chars().any(char::is_control)),
                "{lines:?}"
            );
            assert_eq!(
                lines.iter().filter(|line| line.contains(escaped)).count(),
                holding,
                "{lines:?}"
            );
        }
    }

    fn row(fact: &str, outcome: Outcome) -> PreflightRow {
        PreflightRow {
            fact: fact.to_owned(),
            here: None,
            peer: None,
            outcome,
            note: String::new(),
        }
    }

    /// `doctor --peer --job` fails as the move it previews would, with the
    /// sentence the move's own refusal has: a node reports the paths a
    /// peer can neither use nor make again, and before the client API it
    /// refused with them itself, exit 84. A row that stops the move comes
    /// first, as in a move.
    #[test]
    fn doctor_fails_on_a_refused_path_as_the_move_would() {
        let carry = CarryReport {
            refused: vec!["node_modules".to_owned(), "a/node_modules".to_owned()],
            refused_more: 3,
            ..CarryReport::default()
        };
        let waived =
            "; `fabric job move` and `fabric job resume` take --force to waive what can be waived";
        assert_eq!(
            refused_move(&[row("os", Outcome::Warn)], Some(&carry)),
            Some(format!(
                "the preflight refused the move: the target can neither use nor rebuild node_modules, a/node_modules and 3 more{waived}"
            ))
        );
        assert_eq!(
            refused_move(&[row("claude", Outcome::Refuse)], Some(&carry)),
            Some(format!("the preflight refused the move: claude{waived}"))
        );
        assert_eq!(
            refused_move(&[row("os", Outcome::Warn)], Some(&CarryReport::default())),
            None
        );
        assert_eq!(refused_move(&[row("os", Outcome::Warn)], None), None);
    }
}

//! The human text of the CLI: one line per event, and the lines of each reply.
//!
//! Invariant: the node answers with data (ADR 0018) and this module alone
//! turns it into the words `fabric` prints, so a script that reads the
//! CLI's output and a program that reads the client API see the same facts.
//! Each line is the one the node printed before the client API existed.

use fabric_client::api::{
    DoctorReport, Event, HarnessSession, JobStatus, LedgerEvent, Marker, Outcome, PreflightRow,
    Process, Registered, Reply, SessionRecorded, View, WorkspaceState,
};

/// The lines an event prints; none for a kind this CLI does not know.
#[expect(
    clippy::too_many_lines,
    reason = "one arm per event kind, so every line the CLI prints reads in one place"
)]
pub(crate) fn event(event: &Event) -> Vec<String> {
    let line = match event {
        Event::CatchingUp => "this node is still catching up with its peers after a restart; what follows may not show what they wrote".to_owned(),
        Event::Preflight { rows, .. } => return rows.iter().map(row).collect(),
        Event::StaleWorkspaceKept { job, root, left_at } => format!(
            "the workspace at {} was left at epoch {left_at}, before this node's tenure, so it is not captured; `fabric job resume {job}` sets it aside",
            root.display()
        ),
        Event::AlreadySuspended { snapshot } => format!("already suspended in snapshot {snapshot}"),
        Event::WorkspaceChanged { snapshot } => format!(
            "the workspace changed after snapshot {snapshot}, by hand or by a harness Fabric did not start; capturing it again"
        ),
        Event::StoppingHarness { pid } => format!("stopping the harness, pid {pid}"),
        Event::FinishingSuspend { snapshot } => {
            format!("finishing the suspend that published {snapshot}")
        }
        Event::SessionCaptured {
            harness,
            session,
            files,
        } => format!("captured {harness} session {session} ({files} files)"),
        Event::SnapshotPublished {
            snapshot,
            files,
            bytes,
            new_objects,
        } => format!(
            "snapshot {snapshot}: {files} files, {bytes} bytes, {new_objects} new objects"
        ),
        Event::LeftBehind { path } => format!("left behind (secret pattern): {path}"),
        Event::NoDepartureRecord { session } => format!(
            "no departure record waits for session {session}; its rewritable files stay this node's"
        ),
        Event::Suspended { snapshot, epoch } => format!(
            "suspended in snapshot {snapshot}; this node keeps the lease at epoch {epoch}"
        ),
        Event::Replicated { peer } => format!("copied the ledger and the snapshot to {peer}"),
        Event::NotReplicated { peer, error } => format!("not copied to {peer}: {error}"),
        Event::MoveReoffered { move_id, target } => {
            format!("move {move_id} toward {target} is open; offering it again")
        }
        Event::AlreadyMoved { owner, epoch } => {
            format!("{owner} already owns the job at epoch {epoch}")
        }
        Event::TargetHoldsCopy { target } => {
            format!("{target} holds the ledger and the snapshot")
        }
        Event::MoveStarted { move_id, target } => {
            format!("MoveStarted {move_id}: this copy does not run until {target} decides")
        }
        Event::NoAnswer { target, error } => {
            format!("no answer from {target} ({error}); asking for its decision")
        }
        Event::MoveGaveUp { detail } => format!("last error: {detail}"),
        Event::Imported { owner, epoch } => {
            format!("{owner} imported the job and owns it at epoch {epoch}")
        }
        Event::Declined { target, epoch } => format!(
            "{target} declined the move; this node owns the job again at epoch {epoch}"
        ),
        Event::DecisionLeft { standing } => format!("the decision left this node as {standing}"),
        Event::MoveUndecided { move_id, target } => {
            format!("move {move_id} toward {target} is still undecided")
        }
        Event::MoveUnanswered {
            move_id,
            target,
            error,
        } => format!("move {move_id} toward {target} is open and {target} did not answer: {error}"),
        Event::MoveWaits { target } => {
            format!("the open move waits for {target}, which did not answer")
        }
        Event::Fenced { epoch, pid } => format!(
            "fenced: epoch {epoch} is over for this node; stopping harness pid {pid}"
        ),
        Event::CopyMovedAside { to } => format!(
            "this node's copy moved out of the root to {}; it is kept, never run",
            to.display()
        ),
        Event::Pulling { job, holder } => format!("pulling job {job} from {holder}"),
        Event::HolderStartedMove { holder, move_id } => {
            format!("{holder} started move {move_id} toward this node")
        }
        Event::ImportedHere { move_id, epoch } => {
            format!("imported move {move_id}; this node owns the job at epoch {epoch}")
        }
        Event::HolderNotTold { holder, error } => format!(
            "{holder} has not seen the import yet ({error}); it will ask for the decision"
        ),
        Event::WorkspaceInPlace { root } => format!("workspace already at {}", root.display()),
        Event::RootChosen {
            root,
            recorded: None,
        } => format!(
            "root {}: the same path as the source (ADR 0011)",
            root.display()
        ),
        Event::RootChosen {
            root,
            recorded: Some(recorded),
        } => format!(
            "root {}: the source root {} is not this job's root under this node's home, so the job lands in the state directory and a harness keyed to the path cannot continue there",
            root.display(),
            recorded.display()
        ),
        Event::CacheCarried { path } => format!("{path}: carried, the keys match"),
        Event::CacheRebuilt { path, why } => format!("{path}: rebuilt here, {why}"),
        Event::Restored {
            snapshot,
            files,
            bytes,
        } => format!("restored snapshot {snapshot}: {files} files, {bytes} bytes"),
        Event::OrphanSetAside {
            root,
            left_at,
            epoch,
            to,
        } => format!(
            "the workspace at {} was left at epoch {left_at}, before this node's tenure at epoch {epoch}, and may differ from the job's latest snapshot; it moved to {}, kept, never run",
            root.display(),
            to.display()
        ),
        Event::SessionRestored { harness, session } => {
            format!("restored {harness} session {session}")
        }
        Event::StealWarning { warning } => {
            let described = fabric_core::moves::StealWarning {
                owner: warning.owner,
                last_record_at: warning.last_record_at,
                snapshot: warning.snapshot,
                owner_stopped_at_snapshot: warning.owner_stopped_at_snapshot,
            };
            format!("warning: {}", described.with_age(warning.age_ms))
        }
        Event::Stole { job, epoch } => format!(
            "this node owns the job at epoch {epoch}; run `fabric job resume {job}` to restore and launch it here"
        ),
        Event::OwnerNotTold { owner, error } => {
            format!("{owner} has not seen the steal yet: {error}")
        }
        Event::ContinuingSession { harness, session } => {
            format!("continuing {harness} session {session}")
        }
        Event::NewSession {
            harness,
            previous: None,
        } => format!("starting a new {harness} session"),
        Event::NewSession {
            harness,
            previous: Some(HarnessSession {
                harness: previous,
                session,
            }),
        } => format!(
            "starting a new {harness} session; the {previous} session {session} stays in the snapshot (cross-harness resume is an experiment, ADR 0006)"
        ),
        Event::NotCaughtUp { peer, error } => format!("did not catch up with {peer}: {error}"),
        Event::PeerSilent { peer, waited_s } => format!(
            "{peer} did not answer within {waited_s} s, so a move or a steal there is not known here"
        ),
        Event::LeftAsItWas { error } => format!("left as it was: {error}"),
        Event::Frozen { peer, epoch } => {
            format!("frozen: {peer}'s ledger at epoch {epoch} does not continue this node's")
        }
        Event::Unrecognized => return Vec::new(),
    };
    vec![line]
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
        Reply::JobLog(log) => log
            .records
            .iter()
            .enumerate()
            .map(|(n, record)| {
                format!(
                    "{n:>4} epoch {} at {} {}",
                    record.epoch,
                    record.at,
                    ledger_event(&record.event)
                )
            })
            .collect(),
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
    lines.push(match status.process {
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

/// One preflight row, its outcome and fact in columns (ADR 0008).
fn row(row: &PreflightRow) -> String {
    let outcome = match row.outcome {
        Outcome::Equal => "ok",
        Outcome::Fallback => "rebuild",
        Outcome::Warn => "warn",
        Outcome::Refuse | Outcome::RefuseAlways => "refuse",
    };
    let show = |value: &Option<String>| value.as_deref().unwrap_or("absent").replace('\n', " / ");
    let mut line = format!(
        "{outcome:<7} {:<16} here: {}  peer: {}",
        row.fact,
        show(&row.here),
        show(&row.peer)
    );
    if !row.note.is_empty() {
        line.push_str("  (");
        line.push_str(&row.note);
        line.push(')');
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scripts read the preflight by columns: the outcome padded to 7, the
    /// fact to 16, an absent value as `absent`, a value's lines joined.
    #[test]
    fn a_preflight_row_prints_in_columns() {
        let refused = PreflightRow {
            fact: "claude".to_owned(),
            here: Some("2.1.3".to_owned()),
            peer: None,
            outcome: Outcome::RefuseAlways,
            note: "log in there first".to_owned(),
        };
        assert_eq!(
            row(&refused),
            "refuse  claude           here: 2.1.3  peer: absent  (log in there first)"
        );
        let equal = PreflightRow {
            fact: "macos_sdk".to_owned(),
            here: Some("26.0\n/sdk".to_owned()),
            peer: Some("26.0\n/sdk".to_owned()),
            outcome: Outcome::Equal,
            note: String::new(),
        };
        assert_eq!(
            row(&equal),
            "ok      macos_sdk        here: 26.0 / /sdk  peer: 26.0 / /sdk"
        );
    }
}

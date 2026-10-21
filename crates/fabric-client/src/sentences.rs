//! The sentences of the client API's events, one line each: what the CLI
//! prints for each step of a command, and what a node logs for the steps
//! of work no client asked for, such as recovery or a peer's request.
//!
//! Invariant: an event reads as the same line wherever it is shown, so a
//! script that greps the CLI's output or the daemon's log finds the line
//! the node printed before the client API existed. A program reads the
//! event's fields instead; these words are for people.

use crate::api::{Event, HarnessSession, Outcome, PreflightRow};

/// The lines an event reads as; none for a kind this client does not know.
#[expect(
    clippy::too_many_lines,
    reason = "one arm per event kind, so every line an event reads as is in one place"
)]
#[must_use]
pub fn event(event: &Event) -> Vec<String> {
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
        Event::WorkspaceGone { root } => format!(
            "the clean workspace at {} is gone; restoring the latest snapshot",
            root.display()
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

/// One preflight row, its outcome and fact in columns (ADR 0008).
#[must_use]
pub fn row(row: &PreflightRow) -> String {
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

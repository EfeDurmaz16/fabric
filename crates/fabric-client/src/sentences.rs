//! The sentences of the client API's events, one line each: what the CLI
//! prints for each step of a command, and what a node logs for the steps
//! of work no client asked for, such as recovery or a peer's request.
//!
//! Invariant: an event reads as the same line wherever it is shown, so a
//! script that greps the CLI's output or the daemon's log finds the line
//! the node printed before the client API existed. A program reads the
//! event's fields instead; these words are for people.
//!
//! Every event line leaves here with its control characters escaped
//! (`printable`). An event names paths from a snapshot, the job's harness
//! with the arguments its ledger holds, and words a peer sent. A job
//! learned from a paired node, or a tree an agent wrote, holds whatever
//! they chose there, and an ANSI or OSC sequence among it would otherwise
//! act on the terminal of whoever reads the CLI's output or the node's
//! log, and a line break would forge a line.

use std::borrow::Cow;

use crate::api::{
    CaptureGroup, CaptureNote, CarryFor, CarryLine, Event, HarnessSession, Outcome, PreflightRow,
    RestoredClasses, Verdict, VerdictCount, Written,
};

/// What a snapshot without harness state means for the conversation (gate finding F9).
pub const NO_SESSION: &str =
    "no harness session captured; the conversation will not continue on the target";

/// The lines an event reads as, each with its control characters escaped;
/// none for a kind this client does not know.
#[must_use]
pub fn event(event: &Event) -> Vec<String> {
    lines(event)
        .into_iter()
        .map(|line| {
            if line.chars().any(char::is_control) {
                printable(&line).into_owned()
            } else {
                line
            }
        })
        .collect()
}

/// `text` with each control character written as its escape, such as
/// `\u{1b}` for ESC, `\u{9b}` for a C1 CSI and `\n` for a line break, so
/// it prints as visible characters on the one line it belongs to. A
/// backslash the text holds prints as it is: the line is for people, and a
/// program reads the field from the client API instead.
#[must_use]
pub fn printable(text: &str) -> Cow<'_, str> {
    if !text.chars().any(char::is_control) {
        return Cow::Borrowed(text);
    }
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_control() {
            escaped.extend(c.escape_debug());
        } else {
            escaped.push(c);
        }
    }
    Cow::Owned(escaped)
}

/// The lines of an event as its fields make them, before `event` escapes them.
#[expect(
    clippy::too_many_lines,
    reason = "one arm per event kind, so every line an event reads as is in one place"
)]
fn lines(event: &Event) -> Vec<String> {
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
            git_bundle: true,
        } => format!(
            "snapshot {snapshot}: {files} ignored and harness files and a git bundle of the tracked and untracked work, {bytes} bytes, {new_objects} new objects"
        ),
        Event::SnapshotPublished {
            snapshot,
            files,
            bytes,
            new_objects,
            git_bundle: false,
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
            classes: None,
        } => format!("restored snapshot {snapshot}: {files} files, {bytes} bytes"),
        Event::Restored {
            snapshot,
            files,
            bytes,
            classes: Some(classes),
        } => format!(
            "restored snapshot {snapshot}: {} ({})",
            amount(Written {
                files: *files,
                bytes: *bytes
            }),
            restored_parts(classes)
        ),
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
        Event::SessionNeverStarted {
            harness,
            session,
            kept,
        } => {
            let kept = match kept {
                Some(HarnessSession {
                    harness: kept,
                    session: before,
                }) => format!("the snapshot keeps the {kept} session {before} it held before"),
                None => "the snapshot carries no harness state and the next run starts a new session"
                    .to_owned(),
            };
            format!(
                "{harness} session {session} left no state on this node and no snapshot holds it, so it never started; {kept}"
            )
        }
        Event::CaptureReport { groups } => return groups.iter().flat_map(capture_lines).collect(),
        Event::CarryDecided {
            context,
            snapshot,
            paths,
            more,
        } => {
            let header = match context {
                CarryFor::Move { target } => {
                    format!("what {target} does with each ignored path of snapshot {snapshot}:")
                }
                CarryFor::Pull => format!(
                    "what this node would do with each ignored path of snapshot {snapshot}:"
                ),
                CarryFor::Restore => {
                    format!("what this node does with each ignored path of snapshot {snapshot}:")
                }
            };
            let restore = matches!(context, CarryFor::Restore);
            return std::iter::once(header)
                .chain(carry_lines(paths, more, restore))
                .collect();
        }
        Event::NoSessionCaptured { .. } => NO_SESSION.to_owned(),
        Event::CarriedByForce { paths, more } => format!(
            "--force: {} travels as it is, and the target writes it",
            named(paths, *more)
        ),
        Event::KnownWriterUnknown { owner, error } => {
            format!("{owner} did not say the job's known writer: {error}")
        }
        Event::KnownWriterUnreadable { owner, text, error } => format!(
            "{owner} named {text} as the job's known writer, which is not a version: {error}"
        ),
        Event::AbortNotTold { holder, error } => format!(
            "{holder} has not seen the abort yet ({error}); it will ask for the decision"
        ),
        Event::PeerNotAsked { peer } => format!(
            "{peer} has no address here and no session open with this node, so a move or a steal there is not known here"
        ),
        Event::LiveWorkspaceGone { root } => format!(
            "the workspace at {} is gone; restoring the latest snapshot, and work since it that no suspend captured is lost",
            root.display()
        ),
        Event::SubmodulesLeftEmpty { paths, more } => format!(
            "warning: submodules left empty, since a snapshot carries a submodule's commit and not its files: {}; run `git submodule update --init` in the workspace before the harness needs them",
            named(paths, *more)
        ),
        Event::EscapingLinksRestored { paths, more } => format!(
            "warning: links restored that point outside the tree they were restored into: {}; a harness that follows one reads or writes outside it",
            named(paths, *more)
        ),
        Event::Unrecognized => return Vec::new(),
    };
    vec![line]
}

/// The lines of carry decisions: one per path, its verdict and path in
/// columns and why after them, then one that counts the rest by verdict
/// (ADR 0008). In a restore a refused path is written anyway, because it
/// arrived through `--force`, or the job was already here, and leaving it
/// out would lose it.
pub fn carry_lines(paths: &[CarryLine], more: &[VerdictCount], restore: bool) -> Vec<String> {
    let mut lines: Vec<String> = paths
        .iter()
        .map(|line| {
            let mut text = format!("{:<7} {}  ({})", line.verdict.word(), line.path, line.why);
            if restore && line.verdict == Verdict::Refuse {
                text.push_str("; written anyway, since nothing here can make it again");
            }
            text
        })
        .collect();
    if !more.is_empty() {
        let total: u64 = more.iter().map(|count| count.paths).sum();
        let counts: Vec<String> = more
            .iter()
            .map(|count| format!("{} {}", count.verdict.word(), count.paths))
            .collect();
        lines.push(format!("{total} more paths: {}", counts.join(", ")));
    }
    lines
}

/// `a, b and 3 more`: the paths a carry refusal names, then how many it does not.
#[must_use]
pub fn named(paths: &[String], more: u64) -> String {
    let mut named = paths.join(", ");
    if more > 0 {
        named = format!("{named} and {more} more");
    }
    named
}

/// The lines of one group of a capture's report: one per item, then one
/// that counts the rest; none for a note this client does not know.
fn capture_lines(group: &CaptureGroup) -> Vec<String> {
    let words = match group.note {
        CaptureNote::Secret => "left behind (secret pattern)",
        CaptureNote::Excluded => "left behind (.fabricignore)",
        CaptureNote::SecretHarness => "left behind (secret pattern, harness state)",
        CaptureNote::TrackedSecret => {
            "travels in the snapshot though a secret pattern matches it (tracked)"
        }
        CaptureNote::StashedSecret => {
            "travels in the snapshot though a secret pattern matches it (stashed)"
        }
        CaptureNote::Filtered => {
            "travels as git's check-in output, not its own bytes (filter attribute)"
        }
        CaptureNote::Special => "left behind (not a file, link or directory)",
        CaptureNote::SpecialHarness => "left behind (not a file, link or directory, harness state)",
        CaptureNote::Unreadable => {
            "left behind with everything under it (this user cannot read the directory)"
        }
        CaptureNote::NestedRepository => "left behind (nested repository)",
        CaptureNote::NestedGit => "left behind (the .git of a nested repository)",
        CaptureNote::NestedGitHarness => {
            "left behind (the .git of a nested repository, harness state)"
        }
        CaptureNote::StrippedUserinfo => "travels without its user info (git config)",
        CaptureNote::DroppedConfig => "left behind (git config)",
        CaptureNote::Unrecognized => return Vec::new(),
    };
    let mut lines: Vec<String> = group
        .items
        .iter()
        .map(|item| format!("{words}: {item}"))
        .collect();
    if group.more > 0 {
        lines.push(format!("{words}: {} more", group.more));
    }
    lines
}

/// `3 files, 54 bytes`.
fn amount(written: Written) -> String {
    format!(
        "{} {}, {} {}",
        written.files,
        if written.files == 1 { "file" } else { "files" },
        written.bytes,
        if written.bytes == 1 { "byte" } else { "bytes" }
    )
}

/// What a restore wrote by class, as its line says it after the total.
fn restored_parts(classes: &RestoredClasses) -> String {
    let mut parts = Vec::new();
    if classes.git {
        parts.push(format!("tracked: {}", amount(classes.tracked)));
        parts.push(format!("untracked: {}", amount(classes.untracked)));
        parts.push(format!("ignored: {}", amount(classes.ignored)));
    } else {
        parts.push(format!("workspace: {}", amount(classes.ignored)));
    }
    parts.push(format!("harness state: {}", amount(classes.harness)));
    parts.join("; ")
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
    use fabric_core::harness::{HarnessKind, ProgramName};

    use super::*;

    /// A restore names links and submodules from the snapshot, and a run
    /// plan names the job's harness with the arguments its ledger holds. A
    /// pulled job's snapshot and ledger hold what a peer, or an agent in its
    /// tree, wrote there, and a `RelPath` takes control characters. Each
    /// event line prints them as visible escapes, so a link named with an
    /// OSC 52 clipboard write or a clear screen never acts on the terminal
    /// (review of PR #27).
    #[test]
    fn event_lines_print_with_their_control_characters_escaped() {
        let hostile = "x\u{1b}]52;c;aGk=\u{7}\u{1b}[2J\u{9b}31m\r\nforged";
        let escaped = r"x\u{1b}]52;c;aGk=\u{7}\u{1b}[2J\u{9b}31m\r\nforged";
        let paths = vec![hostile.to_owned()];
        let harness = HarnessKind::Acp {
            program: ProgramName::parse("agent").unwrap(),
            args: vec![hostile.to_owned()],
        };
        for hostile_event in [
            Event::EscapingLinksRestored {
                paths: paths.clone(),
                more: 0,
            },
            Event::SubmodulesLeftEmpty { paths, more: 0 },
            Event::NewSession {
                harness,
                previous: None,
            },
        ] {
            let lines = event(&hostile_event);
            assert!(
                lines.iter().all(|line| !line.chars().any(char::is_control)),
                "{lines:?}"
            );
            assert!(lines.iter().any(|line| line.contains(escaped)), "{lines:?}");
        }
    }

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

//! `gc`: what crashed or ended work left on this node, found by label and
//! removed only with the caller's approval (ADR 0030,
//! docs/concepts/residuals.md).
//!
//! Invariant: nothing in the live set is listed or removed. The live set
//! is the tenures this node owns (a standalone job's epoch, from its
//! ledger), every job a move holds, every job an operation runs on now,
//! the managed attempts the journal holds as ones that may still run
//! (`granted`, `running` and `unknown`, at whatever generation), and the
//! attempts this node's runner and executor run now. The journal's part
//! and the provider creates are read in one call on the authority's
//! thread, so both are of one state. A launch-path file is removed only
//! when the journal holds its attempt ended, which no later event undoes;
//! one of an attempt the journal here does not hold is reported and kept.
//! A job's files are judged and removed under that job's lock, which every
//! operation that writes them holds, so no operation starts between the
//! verdict and the removal; a job whose lock is taken is reported busy and
//! left alone. A residual gc cannot classify is reported and kept. A
//! provider resource is removed only through `Bridge::cleanup`, with an
//! approval the caller gave for this call and that one resource; gc
//! stores none, and the cleanup records through the authority one call at
//! a time, so the authority serves its other calls while the worker acts.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fabric_bridge::{Bridge, Cleanup, Config};
use fabric_client::api::{
    GcApproval, GcKept, GcLabel, GcLive, GcReport, GcResidual, GcWorker, Reason,
};
use fabric_core::id::{Epoch, JobId, NodeId};
use fabric_core::managed::launch::GrantToken;
use fabric_core::managed::{AttemptId, DrainBudget, OperationId, OperationKey};
use fabric_core::moves::LeaseView;
use fabric_journal::{ProviderDispatch, ProviderStatus, Residual};
use fabric_store::restore::{STAGING_LABEL, open_up, staging_dir};

use crate::error::NodeError;
use crate::executor::LaunchRecord;
use crate::job::Job;
use crate::local::Liveness;
use crate::node::Node;
use crate::ops::FENCE_GRACE;
use crate::paths;
use crate::runner::Taken;

/// What a residual is. `docs/concepts/residuals.md` is generated from
/// these docs by `scripts/docs/gen.py`: the first line says what it is,
/// and the `Where`, `Label`, `Owner` and `Garbage` lines fill the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Kind {
    /// A harness capture cut short.
    ///
    /// Where: `<state>/jobs/<job>/harness-state.next`
    /// Label: the job; the epoch of its marker
    /// Owner: the suspend or move that captures the job's harness state
    /// Garbage: always once found under the job's lock: a capture writes it and renames it into place under that lock
    HarnessStateNext,
    /// The harness state a capture set aside.
    ///
    /// Where: `<state>/jobs/<job>/harness-state.old`
    /// Label: the job; the epoch of its marker
    /// Owner: the capture that put a newer state in place
    /// Garbage: once `harness-state` is in place; kept as `only_copy` when it is the only state the job has
    HarnessStateOld,
    /// A restore that did not finish.
    ///
    /// Where: `<target>.fabric-restoring` beside a job's root or its harness state
    /// Label: its `fabric-label` file: the job, the node and the epoch the restore was for
    /// Owner: the resume of that tenure, which finishes it
    /// Garbage: when the label names this node and an epoch it no longer owns
    RestoreStaging,
    /// A harness process group a standalone tenure left running.
    ///
    /// Where: the group named by `<state>/jobs/<job>/harness`
    /// Label: the job and the epoch the record names
    /// Owner: the tenure that launched it
    /// Garbage: when this node no longer owns that epoch; removing it stops the group
    HarnessGroup,
    /// The record of a harness group whose processes are gone.
    ///
    /// Where: `<state>/jobs/<job>/harness`
    /// Label: the job and the epoch the record names
    /// Owner: the tenure that launched the harness
    /// Garbage: when this node no longer owns that epoch
    HarnessRecord,
    /// A launch-path task's record whose processes are gone.
    ///
    /// Where: `<state>/runs/records/<attempt>/<task>.json`
    /// Label: the attempt, and its job, task and generation as the journal holds them
    /// Owner: the executor running the attempt
    /// Garbage: when the journal holds the attempt ended and no runner here runs it
    LaunchRecord,
    /// A launch-path task's process group that still runs.
    ///
    /// Where: the group named by `<state>/runs/records/<attempt>/<task>.json`
    /// Label: as for its record
    /// Owner: the executor running the attempt
    /// Garbage: as for its record; removing it stops the group, then the record
    LaunchGroup,
    /// A copy of a cut's harness state restored for an attempt.
    ///
    /// Where: `<state>/runs/restored/<attempt>`
    /// Label: the attempt, and its job, task and generation as the journal holds them
    /// Owner: the launch that prepares the attempt; a failed preparation leaves it
    /// Garbage: when the journal holds the attempt ended and no runner here runs it
    RestoredState,
    /// A harness state an attempt's checkpoint captured.
    ///
    /// Where: `<state>/runs/harness-states/<attempt>`
    /// Label: as for a restored state
    /// Owner: the checkpoint of the attempt, until its cut is sent
    /// Garbage: when the journal holds the attempt ended and no runner here runs it
    CapturedState,
    /// The work package of a continuation.
    ///
    /// Where: `<home>/fabric/jobs/<job>/package-<attempt>.json`
    /// Label: the job and the attempt, and its task and generation as the journal holds them
    /// Owner: the attempt the package was written for
    /// Garbage: when the journal holds the attempt ended and no runner here runs it
    WorkPackage,
    /// The runner's record of a launch it took.
    ///
    /// Where: `<state>/runs/taken/<attempt>.json`
    /// Label: the grant it holds: the job, the attempt and the generation
    /// Owner: the runner: it answers a launch sent again `already_launched`
    /// Garbage: never: removing it would let a launch sent again run twice; kept as `preserved`
    TakenRecord,
    /// A provider resource a managed attempt made through the bridge.
    ///
    /// Where: the provider, at the locator the journal holds for its create
    /// Label: the job, task, attempt and generation the create was dispatched for
    /// Owner: that attempt
    /// Garbage: when that attempt may no longer run; removed only through the worker, with a fresh approval for that resource
    ProviderResource,
    /// A departed copy.
    ///
    /// Where: `<root parent>/departed/<epoch>`
    /// Label: the job and the epoch in its name
    /// Owner: the user (ADR 0003)
    /// Garbage: never by gc: kept as `preserved`
    DepartedCopy,
    /// An orphan copy.
    ///
    /// Where: `<root parent>/orphan/<epoch>`
    /// Label: the job and the epoch in its name
    /// Owner: the user (ADR 0003)
    /// Garbage: never by gc: kept as `preserved`
    OrphanCopy,
    /// Something under a Fabric directory gc cannot name.
    ///
    /// Where: under `<state>/jobs` or `<state>/runs`
    /// Label: none gc can read
    /// Owner: unknown
    /// Garbage: never by gc: reported and kept
    Unknown,
}

impl Kind {
    /// Every kind, in the order the inventory lists them.
    #[cfg(test)]
    pub(crate) const ALL: [Kind; 15] = [
        Kind::HarnessStateNext,
        Kind::HarnessStateOld,
        Kind::RestoreStaging,
        Kind::HarnessGroup,
        Kind::HarnessRecord,
        Kind::LaunchRecord,
        Kind::LaunchGroup,
        Kind::RestoredState,
        Kind::CapturedState,
        Kind::WorkPackage,
        Kind::TakenRecord,
        Kind::ProviderResource,
        Kind::DepartedCopy,
        Kind::OrphanCopy,
        Kind::Unknown,
    ];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Kind::HarnessStateNext => "harness_state_next",
            Kind::HarnessStateOld => "harness_state_old",
            Kind::RestoreStaging => "restore_staging",
            Kind::HarnessGroup => "harness_group",
            Kind::HarnessRecord => "harness_record",
            Kind::LaunchRecord => "launch_record",
            Kind::LaunchGroup => "launch_group",
            Kind::RestoredState => "restored_state",
            Kind::CapturedState => "captured_state",
            Kind::WorkPackage => "work_package",
            Kind::TakenRecord => "taken_record",
            Kind::ProviderResource => "provider_resource",
            Kind::DepartedCopy => "departed_copy",
            Kind::OrphanCopy => "orphan_copy",
            Kind::Unknown => "unknown",
        }
    }
}

/// Why gc keeps something it found. `docs/concepts/residuals.md` lists
/// these from here, one line each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Keep {
    /// Kept by design: a departed or an orphan copy may hold work only the user can judge, and a taken record fences a launch sent again.
    Preserved,
    /// An operation runs on the job now; run gc again once it ends.
    Busy,
    /// The job is moving: this node waits for its target's decision.
    Moving,
    /// The only harness state the job has; the next capture puts it back in place.
    OnlyCopy,
    /// No label gc can read, or one that does not name this job.
    Unlabeled,
    /// A label naming another node.
    OtherNode,
    /// A job this node does not hold.
    UnknownJob,
    /// An attempt the journal on this node does not hold, as one run for another authority: gc cannot tell whether it may still run.
    UnknownAttempt,
    /// A provider create with no locator: attach it first.
    Unlocated,
}

impl Keep {
    /// Every reason, in the order the inventory lists them.
    #[cfg(test)]
    pub(crate) const ALL: [Keep; 9] = [
        Keep::Preserved,
        Keep::Busy,
        Keep::Moving,
        Keep::OnlyCopy,
        Keep::Unlabeled,
        Keep::OtherNode,
        Keep::UnknownJob,
        Keep::UnknownAttempt,
        Keep::Unlocated,
    ];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Keep::Preserved => "preserved",
            Keep::Busy => "busy",
            Keep::Moving => "moving",
            Keep::OnlyCopy => "only_copy",
            Keep::Unlabeled => "unlabeled",
            Keep::OtherNode => "other_node",
            Keep::UnknownJob => "unknown_job",
            Keep::UnknownAttempt => "unknown_attempt",
            Keep::Unlocated => "unlocated",
        }
    }
}

/// The first line of a staging label, which names its version.
const LABEL_MAGIC: &str = "fabric staging label v1";

/// Who a restore's staging directory is for, written into it as
/// `fabric-label` before any tree: `<magic>\njob <id>\nnode <id>\nepoch <n>\n`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StagingLabel {
    /// The job.
    pub(crate) job: JobId,
    /// The node that restores.
    pub(crate) node: NodeId,
    /// The tenure the restore serves.
    pub(crate) epoch: Epoch,
}

impl StagingLabel {
    /// The label's bytes.
    pub(crate) fn encode(&self) -> String {
        format!(
            "{LABEL_MAGIC}\njob {}\nnode {}\nepoch {}\n",
            self.job,
            self.node,
            self.epoch.get()
        )
    }

    /// Reads a label; `None` for anything this version does not write.
    pub(crate) fn parse(text: &str) -> Option<StagingLabel> {
        let mut lines = text.strip_suffix('\n')?.split('\n');
        if lines.next()? != LABEL_MAGIC {
            return None;
        }
        let mut field = |name: &str| {
            lines
                .next()
                .and_then(|line| line.strip_prefix(name))
                .and_then(|value| value.strip_prefix(' '))
                .map(str::to_owned)
        };
        let job = field("job")?.parse().ok()?;
        let node = field("node")?.parse().ok()?;
        let epoch = Epoch::new(field("epoch")?.parse().ok()?)?;
        lines
            .next()
            .is_none()
            .then_some(StagingLabel { job, node, epoch })
    }
}

/// The largest label gc reads.
const LABEL_MAX: u64 = 4096;

/// Reads the label of the staging directory `staging`; `None` when it has
/// none this version reads.
fn read_label(staging: &Path) -> Option<StagingLabel> {
    use std::io::Read;
    let path = staging.join(STAGING_LABEL);
    let meta = std::fs::symlink_metadata(&path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let mut text = String::new();
    std::fs::File::open(&path)
        .ok()?
        .take(LABEL_MAX)
        .read_to_string(&mut text)
        .ok()?;
    StagingLabel::parse(&text)
}

fn label(job: JobId) -> GcLabel {
    GcLabel {
        job,
        epoch: None,
        node: None,
        task: None,
        attempt: None,
        generation: None,
    }
}

fn tenure(job: JobId, epoch: Option<Epoch>) -> GcLabel {
    GcLabel {
        epoch,
        ..label(job)
    }
}

/// Whether `path` is a directory, not a link to one.
fn is_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir())
}

/// Whether anything, a dangling link included, is at `path`.
fn exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// Removes the directory at `path`, opening up any read-only directory
/// in it first, never through a link.
fn remove_tree(path: &Path) -> Result<(), String> {
    open_up(path).map_err(|e| e.to_string())?;
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// The report as it is built.
#[derive(Debug, Default)]
struct Found {
    live: Vec<GcLive>,
    residuals: Vec<GcResidual>,
    kept: Vec<GcKept>,
}

impl Found {
    fn live(&mut self, holds: &str, label: GcLabel) {
        self.live.push(GcLive {
            holds: holds.to_owned(),
            label,
        });
    }

    fn keep(&mut self, kind: Kind, path: Option<PathBuf>, label: Option<GcLabel>, why: Keep) {
        self.kept.push(GcKept {
            kind: kind.name().to_owned(),
            path,
            operation: None,
            label,
            reason: why.name().to_owned(),
        });
    }

    fn residual(&mut self, kind: Kind, label: GcLabel) -> &mut GcResidual {
        self.residuals.push(GcResidual {
            kind: kind.name().to_owned(),
            path: None,
            pid: None,
            operation: None,
            locator: None,
            label,
            state: "found".to_owned(),
            detail: None,
        });
        let last = self.residuals.len() - 1;
        &mut self.residuals[last]
    }

    fn into_report(mut self, removing: bool) -> GcReport {
        let at = |kind: &str,
                  path: &Option<PathBuf>,
                  pid: Option<NonZeroU32>,
                  op: Option<OperationId>| {
            (
                kind.to_owned(),
                path.clone(),
                pid,
                op.map(|op| op.to_string()),
            )
        };
        self.live.sort_by(|a, b| {
            (
                a.label.job,
                &a.holds,
                a.label.attempt.map(|x| x.to_string()),
            )
                .cmp(&(
                    b.label.job,
                    &b.holds,
                    b.label.attempt.map(|x| x.to_string()),
                ))
        });
        self.residuals.sort_by(|a, b| {
            at(&a.kind, &a.path, a.pid, a.operation).cmp(&at(&b.kind, &b.path, b.pid, b.operation))
        });
        self.kept.sort_by(|a, b| {
            at(&a.kind, &a.path, None, a.operation).cmp(&at(&b.kind, &b.path, None, b.operation))
        });
        GcReport {
            removing,
            live: self.live,
            residuals: self.residuals,
            kept: self.kept,
        }
    }
}

fn bad_request(message: String) -> NodeError {
    NodeError::Managed {
        reason: Box::new(Reason::BadRequest),
        message,
    }
}

/// A provider create gc found, with the scope it was dispatched for.
#[derive(Debug, Clone)]
struct Provider {
    residual: Residual,
    label: GcLabel,
    profile: String,
}

/// What gc read of the managed journal, in one call on the authority's
/// thread, so the live attempts and the provider creates are of one state.
#[derive(Debug, Default)]
struct Managed {
    /// Every attempt the journal holds: whose it is, and whether it may
    /// still run (`granted`, `running` or `unknown`).
    attempts: BTreeMap<AttemptId, (GcLabel, bool)>,
    /// Every create a job's attempts may have left, with its dispatch.
    creates: Vec<(Residual, ProviderDispatch)>,
}

/// What the journal and this node's runner say of an attempt.
enum Verdict {
    /// It may still run, or runs here now: never touched.
    Live,
    /// The journal holds it ended, and nothing here runs it.
    Ended(GcLabel),
    /// The journal here does not hold it.
    Unheld,
}

/// The launch path's part of the live set: the attempts this node runs now.
#[derive(Debug, Default)]
struct Running {
    tokens: Vec<GrantToken>,
    open: BTreeSet<AttemptId>,
}

impl Running {
    fn runs(&self, attempt: AttemptId) -> bool {
        self.open.contains(&attempt) || self.tokens.iter().any(|t| t.attempt == attempt)
    }
}

fn attempt_label(token: &GrantToken) -> GcLabel {
    GcLabel {
        attempt: Some(token.attempt),
        generation: Some(token.generation),
        ..label(token.job)
    }
}

impl Managed {
    fn verdict(&self, attempt: AttemptId, running: &Running) -> Verdict {
        if running.runs(attempt) {
            return Verdict::Live;
        }
        match self.attempts.get(&attempt) {
            Some((_, true)) => Verdict::Live,
            Some((label, false)) => Verdict::Ended(label.clone()),
            None => Verdict::Unheld,
        }
    }
}

/// How long a worker has to answer one cleanup.
const CLEANUP_DEADLINE_MS: u64 = 60_000;

/// The worker approved removals go through.
pub(crate) enum Worker {
    /// Started for this call from the request's spec.
    Spawn(GcWorker),
    /// Already connected: an in-process worker in a test.
    #[cfg(test)]
    Ready(Bridge),
}

impl Node {
    /// Serves `gc`: reports what crashed or ended work left, and removes it
    /// when `remove` is set (ADR 0030).
    pub(crate) async fn gc(
        self: &Arc<Self>,
        remove: bool,
        approvals: Vec<GcApproval>,
        worker: Option<GcWorker>,
    ) -> Result<GcReport, NodeError> {
        self.gc_with(remove, approvals, worker.map(Worker::Spawn))
            .await
    }

    /// `gc`, with the worker as given.
    pub(crate) async fn gc_with(
        self: &Arc<Self>,
        remove: bool,
        approvals: Vec<GcApproval>,
        worker: Option<Worker>,
    ) -> Result<GcReport, NodeError> {
        if !remove && !approvals.is_empty() {
            return Err(bad_request(
                "an approval removes a provider resource, so it needs remove".into(),
            ));
        }
        // Recovery settles every job first (it stops a fenced harness and
        // moves a departed copy aside), so gc never races it.
        self.recovered().await;
        let mut found = Found::default();
        // The runner first: an attempt it runs is live whatever the
        // journal read after says, and one launched after this read wrote
        // nothing the reads below can see as ended.
        let (tokens, open) = self.runner.running().await;
        let running = Running {
            tokens,
            open: open.into_iter().collect(),
        };
        for token in &running.tokens {
            found.live("launch", attempt_label(token));
        }
        let managed = self.gc_read_journal().await?;
        let providers = self.gc_providers(&managed, &running, &mut found);
        let mut approved = Vec::new();
        for approval in approvals {
            let Some(at) = providers
                .iter()
                .position(|p| p.residual.creating == approval.operation)
            else {
                return Err(bad_request(format!(
                    "operation {} is not a provider resource gc lists as a residual",
                    approval.operation
                )));
            };
            if approved.iter().any(|(i, _)| *i == at) {
                return Err(bad_request(format!(
                    "operation {} has two approvals; give one",
                    approval.operation
                )));
            }
            approved.push((at, approval.approval));
        }
        if !approved.is_empty() && worker.is_none() {
            return Err(bad_request(
                "a provider resource is removed through a worker; name one".into(),
            ));
        }
        self.gc_jobs(remove, &mut found).await?;
        self.gc_home(&mut found);
        self.gc_runs(remove, &managed, &running, &mut found).await;
        let first = found.residuals.len();
        for provider in &providers {
            let entry = found.residual(Kind::ProviderResource, provider.label.clone());
            entry.operation = Some(provider.residual.creating);
            entry.locator = provider.residual.locator.as_ref().map(|l| l.text.clone());
            if remove {
                "approval_required".clone_into(&mut entry.state);
            }
        }
        if let Some(worker) = worker.filter(|_| !approved.is_empty()) {
            let outcomes = self.gc_cleanup(worker, &providers, &approved).await;
            for ((at, _), outcome) in approved.iter().zip(outcomes) {
                let entry = &mut found.residuals[first + at];
                match outcome {
                    Ok(()) => "removed".clone_into(&mut entry.state),
                    Err(why) => {
                        "failed".clone_into(&mut entry.state);
                        entry.detail = Some(why);
                    }
                }
            }
        }
        Ok(found.into_report(remove))
    }

    /// Every attempt the journal holds and every create its jobs may have
    /// left, read in one call so the two are of one state: an attempt
    /// granted after it is in neither, and one granted before it is live
    /// in both.
    async fn gc_read_journal(&self) -> Result<Managed, NodeError> {
        Ok(self
            .authority
            .read_journal(|journal| {
                let mut managed = Managed::default();
                let Some(journal) = journal else {
                    return Ok(managed);
                };
                for job in journal.jobs()? {
                    let Some(state) = journal.job(job)? else {
                        continue;
                    };
                    for attempt in &state.attempts {
                        let named = GcLabel {
                            task: Some(attempt.task),
                            attempt: Some(attempt.id),
                            generation: Some(attempt.generation),
                            ..label(job)
                        };
                        managed
                            .attempts
                            .insert(attempt.id, (named, attempt.status.may_run()));
                    }
                    for residual in journal.residuals(job)? {
                        if let Some(op) = journal.provider_operation(residual.creating)? {
                            managed.creates.push((residual, op.dispatch));
                        }
                    }
                }
                Ok(managed)
            })
            .await?)
    }

    /// The live attempts, and the provider creates of attempts that may no
    /// longer run; one with no locator is kept.
    fn gc_providers(
        &self,
        managed: &Managed,
        running: &Running,
        found: &mut Found,
    ) -> Vec<Provider> {
        for (label, live) in managed.attempts.values() {
            if *live {
                found.live("attempt", label.clone());
            }
        }
        let mut providers = Vec::new();
        for (residual, dispatch) in &managed.creates {
            let named = GcLabel {
                task: Some(dispatch.task),
                attempt: Some(dispatch.attempt),
                generation: Some(dispatch.generation),
                ..label(dispatch.job)
            };
            // An attempt the journal does not hold has no verdict here, so
            // its create is live as well: nothing is removed on a guess.
            if !matches!(
                managed.verdict(dispatch.attempt, running),
                Verdict::Ended(_)
            ) {
                continue;
            }
            if residual.locator.is_none() || residual.status == ProviderStatus::Dispatched {
                found.kept.push(GcKept {
                    kind: Kind::ProviderResource.name().to_owned(),
                    path: None,
                    operation: Some(residual.creating),
                    label: Some(named),
                    reason: Keep::Unlocated.name().to_owned(),
                });
                continue;
            }
            providers.push(Provider {
                residual: residual.clone(),
                label: named,
                profile: dispatch.profile.clone(),
            });
        }
        providers
    }

    /// Judges, and with `remove` removes, what each standalone job left.
    async fn gc_jobs(&self, remove: bool, found: &mut Found) -> Result<(), NodeError> {
        let jobs: Vec<(JobId, Arc<tokio::sync::Mutex<Job>>)> = self
            .jobs
            .lock()
            .await
            .iter()
            .map(|(id, job)| (*id, Arc::clone(job)))
            .collect();
        let known: BTreeSet<JobId> = jobs.iter().map(|(id, _)| *id).collect();
        for (id, job) in jobs {
            match job.try_lock() {
                Ok(job) => self.gc_job(&job, remove, found).await?,
                Err(_) => {
                    found.live("busy", label(id));
                    for path in self.candidates(id, None) {
                        if exists(&path) {
                            found.keep(Kind::Unknown, Some(path), Some(label(id)), Keep::Busy);
                        }
                    }
                }
            }
        }
        // A job directory the node did not load holds nothing gc can judge.
        if let Ok(entries) = std::fs::read_dir(self.cfg.state.jobs()) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let job = name.to_str().and_then(|name| name.parse::<JobId>().ok());
                match job {
                    Some(job) if known.contains(&job) => {}
                    Some(job) => {
                        found.keep(
                            Kind::Unknown,
                            Some(entry.path()),
                            Some(label(job)),
                            Keep::UnknownJob,
                        );
                    }
                    None => found.keep(Kind::Unknown, Some(entry.path()), None, Keep::Unlabeled),
                }
            }
        }
        Ok(())
    }

    /// Every staging directory a restore for `id` may leave: beside the
    /// job's root under the home, beside its fallback root, beside its
    /// harness state, and beside the root `marker` names.
    fn candidates(&self, id: JobId, marker: Option<&Path>) -> Vec<PathBuf> {
        let dir = self.cfg.state.job(id);
        let mut targets = vec![
            paths::job_root(&self.cfg.home, id),
            dir.fallback_root(),
            dir.harness_state(),
        ];
        targets.extend(marker.map(Path::to_owned));
        let mut out: Vec<PathBuf> = targets.iter().filter_map(|t| staging_dir(t)).collect();
        out.push(dir.harness_state_next());
        out.push(dir.harness_state_old());
        out.sort();
        out.dedup();
        out
    }

    async fn gc_job(&self, job: &Job, remove: bool, found: &mut Found) -> Result<(), NodeError> {
        let id = job.id();
        let view = job.view(self.id);
        let owned = match view {
            LeaseView::Owner(token) if !job.is_frozen()? => Some(token.epoch),
            _ => None,
        };
        let marker = job.marker()?;
        let marked = marker.as_ref().map(|m| m.epoch);
        if let Some(epoch) = owned {
            found.live("tenure", tenure(id, Some(epoch)));
        }
        let dir = job.dir().clone();
        if matches!(view, LeaseView::Departing { .. }) {
            found.live("moving", tenure(id, marked));
            for path in self.candidates(id, marker.as_ref().map(|m| m.root.as_path())) {
                if exists(&path) {
                    found.keep(Kind::Unknown, Some(path), Some(label(id)), Keep::Moving);
                }
            }
            return Ok(());
        }
        // A capture writes `.next` and renames it into place under this
        // lock; found here, it was cut short, whatever the tenure.
        let next = dir.harness_state_next();
        if exists(&next) {
            if is_dir(&next) {
                let entry = found.residual(Kind::HarnessStateNext, tenure(id, marked));
                entry.path = Some(next.clone());
                if remove {
                    settle(entry, remove_tree(&next));
                }
            } else {
                found.keep(
                    Kind::HarnessStateNext,
                    Some(next.clone()),
                    Some(label(id)),
                    Keep::Unlabeled,
                );
            }
        }
        let old = dir.harness_state_old();
        if exists(&old) {
            if !is_dir(&old) {
                found.keep(
                    Kind::HarnessStateOld,
                    Some(old.clone()),
                    Some(label(id)),
                    Keep::Unlabeled,
                );
            } else if exists(&dir.harness_state()) {
                let entry = found.residual(Kind::HarnessStateOld, tenure(id, marked));
                entry.path = Some(old.clone());
                if remove {
                    settle(entry, remove_tree(&old));
                }
            } else {
                found.keep(
                    Kind::HarnessStateOld,
                    Some(old.clone()),
                    Some(tenure(id, marked)),
                    Keep::OnlyCopy,
                );
            }
        }
        let stagings: Vec<PathBuf> = self
            .candidates(id, marker.as_ref().map(|m| m.root.as_path()))
            .into_iter()
            .filter(|path| *path != next && *path != old && exists(path))
            .collect();
        for staging in stagings {
            if !is_dir(&staging) {
                found.keep(
                    Kind::RestoreStaging,
                    Some(staging),
                    Some(label(id)),
                    Keep::Unlabeled,
                );
                continue;
            }
            let Some(read) = read_label(&staging).filter(|read| read.job == id) else {
                found.keep(
                    Kind::RestoreStaging,
                    Some(staging),
                    Some(label(id)),
                    Keep::Unlabeled,
                );
                continue;
            };
            let named = GcLabel {
                node: Some(read.node),
                ..tenure(id, Some(read.epoch))
            };
            if read.node != self.id {
                found.keep(
                    Kind::RestoreStaging,
                    Some(staging),
                    Some(named),
                    Keep::OtherNode,
                );
                continue;
            }
            // The restore of the tenure this node owns: the next resume
            // finishes it, so it is live.
            if owned == Some(read.epoch) {
                continue;
            }
            let entry = found.residual(Kind::RestoreStaging, named);
            entry.path = Some(staging.clone());
            if remove {
                settle(entry, remove_tree(&staging));
            }
        }
        if let Some(record) = job.harness_record()?
            && owned != Some(record.epoch)
        {
            let named = tenure(id, Some(record.epoch));
            match record.liveness().await {
                Liveness::Unknown => {
                    found.keep(
                        Kind::HarnessGroup,
                        Some(dir.harness()),
                        Some(named),
                        Keep::Unlabeled,
                    );
                }
                Liveness::Gone => {
                    let entry = found.residual(Kind::HarnessRecord, named);
                    entry.path = Some(dir.harness());
                    if remove {
                        settle(entry, job.clear_harness_record().map_err(|e| e.to_string()));
                    }
                }
                Liveness::Leader | Liveness::Group => {
                    let entry = found.residual(Kind::HarnessGroup, named);
                    entry.pid = Some(record.pid);
                    if remove {
                        let stopped = match record.stop(FENCE_GRACE).await {
                            Ok(()) => job.clear_harness_record().map_err(|e| e.to_string()),
                            Err(e) => Err(e.to_string()),
                        };
                        settle(entry, stopped);
                    }
                }
            }
        }
        for root in [paths::job_root(&self.cfg.home, id), dir.fallback_root()] {
            let Some(parent) = root.parent() else {
                continue;
            };
            for (sub, kind) in [
                ("departed", Kind::DepartedCopy),
                ("orphan", Kind::OrphanCopy),
            ] {
                let Ok(entries) = std::fs::read_dir(parent.join(sub)) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let epoch = entry
                        .file_name()
                        .to_str()
                        .and_then(|name| name.parse().ok())
                        .and_then(Epoch::new);
                    found.keep(
                        kind,
                        Some(entry.path()),
                        Some(tenure(id, epoch)),
                        Keep::Preserved,
                    );
                }
            }
        }
        Ok(())
    }

    /// The staging directories under the home of jobs this node does not
    /// hold: reported and kept, since nothing here says whose they are
    /// beyond their label.
    fn gc_home(&self, found: &mut Found) {
        let Ok(entries) = std::fs::read_dir(paths::home_jobs(&self.cfg.home)) else {
            return;
        };
        for entry in entries.flatten() {
            let Some(job) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<JobId>().ok())
            else {
                continue;
            };
            if self.cfg.state.job(job).dir().exists() {
                continue;
            }
            let Some(staging) = staging_dir(&paths::job_root(&self.cfg.home, job)) else {
                continue;
            };
            if !exists(&staging) {
                continue;
            }
            match read_label(&staging).filter(|read| read.job == job) {
                Some(read) => {
                    let named = GcLabel {
                        node: Some(read.node),
                        ..tenure(job, Some(read.epoch))
                    };
                    let why = if read.node == self.id {
                        Keep::UnknownJob
                    } else {
                        Keep::OtherNode
                    };
                    found.keep(Kind::RestoreStaging, Some(staging), Some(named), why);
                }
                None => found.keep(
                    Kind::RestoreStaging,
                    Some(staging),
                    Some(label(job)),
                    Keep::Unlabeled,
                ),
            }
        }
    }

    /// Removes each approved provider resource through `worker`, one
    /// cleanup each, recorded through the authority one call at a time;
    /// the outcome per approval, in order.
    async fn gc_cleanup(
        &self,
        worker: Worker,
        providers: &[Provider],
        approved: &[(usize, String)],
    ) -> Vec<Result<(), String>> {
        let bridge = match worker {
            Worker::Spawn(spec) => spawn(&spec).await,
            #[cfg(test)]
            Worker::Ready(bridge) => Ok(bridge),
        };
        let bridge = match bridge {
            Ok(bridge) => bridge,
            Err(e) => {
                let why = format!("the worker did not start: {e}");
                return approved.iter().map(|_| Err(why.clone())).collect();
            }
        };
        let mut outcomes = Vec::new();
        for (at, approval) in approved {
            let provider = &providers[*at];
            outcomes.push(self.gc_destroy(&bridge, provider, approval.clone()).await);
        }
        bridge.close().await;
        outcomes
    }

    /// One approved destroy. The bridge awaits the worker on the runtime
    /// and records each step through the authority's queue, so the
    /// authority's thread is never held while the worker acts.
    async fn gc_destroy(
        &self,
        bridge: &Bridge,
        provider: &Provider,
        approval: String,
    ) -> Result<(), String> {
        if bridge.profile().profile != provider.profile {
            return Err(format!(
                "the worker serves profile {}; the resource was made under {}",
                bridge.profile().profile,
                provider.profile
            ));
        }
        let id = OperationId::from_bytes(ulid::Ulid::generate().to_bytes());
        let key = OperationKey::parse(&format!("gc-{id}")).map_err(|e| e.to_string())?;
        let cleanup = Cleanup {
            creating: provider.residual.creating,
            id,
            key,
            approval,
            deadline_ms: CLEANUP_DEADLINE_MS,
        };
        let mut log = self.authority.recorder();
        let outcome = async {
            let flight = bridge.cleanup(&mut log, cleanup).await?;
            flight.finish(&mut log).await
        }
        .await;
        match outcome {
            Ok(outcome) if outcome.status == ProviderStatus::Succeeded => Ok(()),
            Ok(outcome) => Err(format!(
                "the destroy is {}; observe it, then run gc again",
                outcome.status.name()
            )),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Judges, and with `remove` removes, what the launch path left under
    /// `<state>/runs` and the work packages beside managed roots: only
    /// what the journal holds an ended attempt for.
    async fn gc_runs(&self, remove: bool, managed: &Managed, running: &Running, found: &mut Found) {
        let runs = self.cfg.state.root().join(RUNS);
        let Ok(entries) = std::fs::read_dir(&runs) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            match entry.file_name().to_str() {
                Some(RECORDS) => {
                    self.gc_records(remove, &path, managed, running, found)
                        .await
                }
                Some(RESTORED) => {
                    gc_attempt_dirs(remove, &path, Kind::RestoredState, managed, running, found);
                }
                Some(CAPTURED) => {
                    gc_attempt_dirs(remove, &path, Kind::CapturedState, managed, running, found);
                }
                Some(TAKEN) => gc_taken(&path, managed, found),
                _ => found.keep(Kind::Unknown, Some(path), None, Keep::Unlabeled),
            }
        }
        self.gc_packages(remove, managed, running, found);
    }

    /// The executor's launch records: a group that still runs, or a record
    /// of one that is gone, of an ended attempt.
    async fn gc_records(
        &self,
        remove: bool,
        dir: &Path,
        managed: &Managed,
        running: &Running,
        found: &mut Found,
    ) {
        let Ok(attempts) = std::fs::read_dir(dir) else {
            return;
        };
        for attempt_dir in attempts.flatten() {
            let path = attempt_dir.path();
            let Some(attempt) = parse_name::<AttemptId>(&attempt_dir.file_name()) else {
                found.keep(Kind::Unknown, Some(path), None, Keep::Unlabeled);
                continue;
            };
            let named = match managed.verdict(attempt, running) {
                Verdict::Live => continue,
                Verdict::Unheld => {
                    found.keep(Kind::LaunchRecord, Some(path), None, Keep::UnknownAttempt);
                    continue;
                }
                Verdict::Ended(named) => named,
            };
            let Ok(tasks) = std::fs::read_dir(&path) else {
                continue;
            };
            for task in tasks.flatten() {
                let file = task.path();
                let record = match crate::local::read::<LaunchRecord>(&file) {
                    Ok(Some(record)) if record.attempt == attempt => record,
                    _ => {
                        found.keep(
                            Kind::LaunchRecord,
                            Some(file),
                            Some(named.clone()),
                            Keep::Unlabeled,
                        );
                        continue;
                    }
                };
                let task_label = GcLabel {
                    task: Some(record.task),
                    ..named.clone()
                };
                match record.group().await {
                    Some(pid) => {
                        let entry = found.residual(Kind::LaunchGroup, task_label);
                        entry.pid = Some(pid);
                        if remove {
                            record.stop(DrainBudget::default()).await;
                            settle(
                                entry,
                                crate::local::remove(&file).map_err(|e| e.to_string()),
                            );
                        }
                    }
                    None => {
                        let entry = found.residual(Kind::LaunchRecord, task_label);
                        entry.path = Some(file.clone());
                        if remove {
                            settle(
                                entry,
                                crate::local::remove(&file).map_err(|e| e.to_string()),
                            );
                        }
                    }
                }
            }
            if remove {
                // Empty once every record went; one kept keeps it.
                let _ = std::fs::remove_dir(&path);
            }
        }
    }

    /// The work packages beside managed roots: `<home>/fabric/jobs/<job>/package-<attempt>.json`.
    fn gc_packages(&self, remove: bool, managed: &Managed, running: &Running, found: &mut Found) {
        let Ok(jobs) = std::fs::read_dir(paths::home_jobs(&self.cfg.home)) else {
            return;
        };
        for job in jobs.flatten() {
            let Ok(files) = std::fs::read_dir(job.path()) else {
                continue;
            };
            for file in files.flatten() {
                let Some(attempt) = file
                    .file_name()
                    .to_str()
                    .and_then(|name| name.strip_prefix(PACKAGE_PREFIX))
                    .and_then(|name| name.strip_suffix(".json"))
                    .and_then(|id| id.parse::<AttemptId>().ok())
                else {
                    continue;
                };
                let path = file.path();
                match managed.verdict(attempt, running) {
                    Verdict::Live => {}
                    Verdict::Unheld => {
                        found.keep(Kind::WorkPackage, Some(path), None, Keep::UnknownAttempt);
                    }
                    Verdict::Ended(named) => {
                        let entry = found.residual(Kind::WorkPackage, named);
                        entry.path = Some(path.clone());
                        if remove {
                            settle(
                                entry,
                                crate::local::remove(&path).map_err(|e| e.to_string()),
                            );
                        }
                    }
                }
            }
        }
    }
}

/// The launch path's directory under the state directory, and its parts.
const RUNS: &str = "runs";
const RECORDS: &str = "records";
const RESTORED: &str = "restored";
const CAPTURED: &str = "harness-states";
const TAKEN: &str = "taken";
/// The name a work package starts with.
const PACKAGE_PREFIX: &str = "package-";

fn parse_name<T: std::str::FromStr>(name: &std::ffi::OsStr) -> Option<T> {
    name.to_str().and_then(|name| name.parse().ok())
}

/// The per-attempt directories under `dir` (`restored/` or
/// `harness-states/`) of ended attempts.
fn gc_attempt_dirs(
    remove: bool,
    dir: &Path,
    kind: Kind,
    managed: &Managed,
    running: &Running,
    found: &mut Found,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(attempt) = parse_name::<AttemptId>(&entry.file_name()) else {
            found.keep(Kind::Unknown, Some(path), None, Keep::Unlabeled);
            continue;
        };
        if !is_dir(&path) {
            found.keep(kind, Some(path), None, Keep::Unlabeled);
            continue;
        }
        match managed.verdict(attempt, running) {
            Verdict::Live => {}
            Verdict::Unheld => found.keep(kind, Some(path), None, Keep::UnknownAttempt),
            Verdict::Ended(named) => {
                let entry = found.residual(kind, named);
                entry.path = Some(path.clone());
                if remove {
                    settle(entry, remove_tree(&path));
                }
            }
        }
    }
}

/// The runner's taken records: reported and kept, since each answers a
/// launch sent again.
fn gc_taken(dir: &Path, managed: &Managed, found: &mut Found) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let named = match crate::local::read::<Taken>(&path) {
            Ok(Some(taken)) => Some(
                managed
                    .attempts
                    .get(&taken.token.attempt)
                    .map_or_else(|| attempt_label(&taken.token), |(named, _)| named.clone()),
            ),
            _ => None,
        };
        let why = if named.is_some() {
            Keep::Preserved
        } else {
            Keep::Unlabeled
        };
        found.keep(Kind::TakenRecord, Some(path), named, why);
    }
}

/// Starts the worker `spec` names, with `PATH`, `HOME` and the `DOCKER_*`
/// variables of the node and nothing else, so no credential of the node's
/// environment reaches it.
async fn spawn(spec: &GcWorker) -> Result<Bridge, fabric_bridge::BridgeError> {
    let mut command = tokio::process::Command::new(spec.program.as_path());
    command.env_clear();
    for (name, value) in std::env::vars_os() {
        let keep = name
            .to_str()
            .is_some_and(|n| n == "PATH" || n == "HOME" || n.starts_with("DOCKER_"));
        if keep {
            command.env(name, value);
        }
    }
    command.arg("--profile").arg(&spec.profile);
    if let Some(config) = &spec.config {
        command.arg("--config").arg(config.as_path());
    }
    Bridge::spawn(command, Config::new(&spec.profile)).await
}

/// Records how one removal ended.
fn settle(entry: &mut GcResidual, removed: Result<(), String>) {
    match removed {
        Ok(()) => "removed".clone_into(&mut entry.state),
        Err(why) => {
            "failed".clone_into(&mut entry.state);
            entry.detail = Some(why);
        }
    }
}

#[cfg(test)]
mod tests;

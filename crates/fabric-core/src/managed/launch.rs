//! The launch path (M5): what the authority sends the node it granted an
//! attempt to, what that node sends back, and the program a build or test
//! task runs. Pure: no IO, no clock. The node link carries these shapes
//! as they are (`fabric-net`, protocol 4); the authority and the node
//! both read them from here, so neither owner names a field twice.
//!
//! Invariant: every message from a node names its grant with a
//! [`GrantToken`], and the authority acts on it only while that token is
//! the job's live grant. A report at another generation is refused
//! `StaleGeneration` and recorded, never applied (replay rule 7). A
//! heartbeat is answered with the fence's own verdict for each grant
//! ([`FenceVerdict`], ADR 0028), the one verdict type a node obeys: a
//! verdict that ends the grant ([`FenceVerdict::ends_grant`]) stops what
//! the node runs under it. A report the fence refuses is answered with the refusal
//! that names the same verdict ([`ReportRefusal::fenced`]).

use serde::{Deserialize, Serialize};

use super::fence::FenceVerdict;
use super::ids::{AttemptId, Generation, TaskId, WorkRevision};
use super::lifecycle::TaskClass;
use super::model::{AttemptStatus, Intent, JobPolicy, Observation, SourceBinding};
use super::package::{AllowedFidelity, PACKAGE_MAX_BYTES, WorkPackage};
use super::secret_ref::SecretName;
use crate::env::EnvVarName;
use crate::harness::HarnessKind;
use crate::id::{JobId, NodeId, SnapshotId};
use crate::path::RelPath;

/// The most arguments a program names, the command included.
pub const MAX_ARGV: usize = 64;
/// The most bytes of one argument.
pub const MAX_ARG_BYTES: usize = 4096;
/// The most variables a program's allowlist names.
pub const MAX_ENV_ALLOWLIST: usize = 64;
/// The most tasks one launch asks a node to spawn beside the harness task.
pub const MAX_TASK_LAUNCHES: usize = 256;
/// The most grants one heartbeat names.
pub const MAX_HEARTBEAT_ATTEMPTS: usize = 64;

/// What a build or test task runs: the executor spawns `argv[0]` with the
/// rest as its arguments, in `cwd` under the attempt's root (the root when
/// `None`), with the job's secrets under their names and, of the node's
/// own environment, only the variables `env_allowlist` names plus what
/// the executor always passes (`PATH`, `HOME`). A harness task has no
/// program: it runs through its adapter.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Program {
    /// The command and its arguments; never empty.
    pub argv: Vec<String>,
    /// Where it runs, relative to the attempt's root.
    pub cwd: Option<RelPath>,
    /// The variables of the node's environment it may see.
    pub env_allowlist: Vec<EnvVarName>,
}

/// Why a program is refused.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, thiserror::Error)]
pub enum ProgramError {
    /// `argv` is empty.
    #[error("a program names a command")]
    EmptyArgv,
    /// More than `MAX_ARGV` arguments.
    #[error("a program has at most {MAX_ARGV} arguments, not {count}")]
    TooManyArgs {
        /// How many it had.
        count: usize,
    },
    /// One argument is over `MAX_ARG_BYTES`.
    #[error("argument {index} is {bytes} bytes; at most {MAX_ARG_BYTES}")]
    ArgTooLong {
        /// Which argument.
        index: usize,
        /// Its length.
        bytes: usize,
    },
    /// One argument holds a control character or a NUL.
    #[error("argument {index} holds a control character")]
    ControlCharacter {
        /// Which argument.
        index: usize,
    },
    /// More than `MAX_ENV_ALLOWLIST` variables.
    #[error("a program allows at most {MAX_ENV_ALLOWLIST} variables, not {count}")]
    TooManyEnv {
        /// How many it had.
        count: usize,
    },
    /// One argument looks like a credential (`fabric_core::credential`).
    #[error("argument {index} looks like a credential")]
    CredentialLike {
        /// Which argument.
        index: usize,
    },
}

impl Program {
    /// Refuses an empty, oversize or credential-carrying program. The
    /// authority runs it at `RegisterTask`; a node runs it again before
    /// it spawns, since it trusts no bound it did not check.
    pub fn check(&self) -> Result<(), ProgramError> {
        if self.argv.is_empty() {
            return Err(ProgramError::EmptyArgv);
        }
        if self.argv.len() > MAX_ARGV {
            return Err(ProgramError::TooManyArgs {
                count: self.argv.len(),
            });
        }
        for (index, arg) in self.argv.iter().enumerate() {
            if arg.len() > MAX_ARG_BYTES {
                return Err(ProgramError::ArgTooLong {
                    index,
                    bytes: arg.len(),
                });
            }
            if arg.chars().any(char::is_control) {
                return Err(ProgramError::ControlCharacter { index });
            }
            if crate::credential::looks_like_credential(arg) {
                return Err(ProgramError::CredentialLike { index });
            }
        }
        if self.env_allowlist.len() > MAX_ENV_ALLOWLIST {
            return Err(ProgramError::TooManyEnv {
                count: self.env_allowlist.len(),
            });
        }
        Ok(())
    }
}

/// One grant, as the node that holds it names it on every message: the
/// job, the attempt and the generation it was granted at. M6's covered
/// writes and heartbeats carry the same token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GrantToken {
    /// The job.
    pub job: JobId,
    /// The attempt.
    pub attempt: AttemptId,
    /// The generation the attempt was granted at.
    pub generation: Generation,
}

/// A task the node spawns in the attempt: a registered build or test task
/// with its program. Carried in a launch for the open tasks that have one
/// and in a `Spawn` instruction for one registered while the attempt runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskLaunch {
    /// The task.
    pub task: TaskId,
    /// Its class; `Build` or `Test`.
    pub class: TaskClass,
    /// What it runs.
    pub program: Program,
}

/// What a planned handoff asks of the new attempt's node: the attempt it
/// continues and the fidelities the continue allows (the job's allowance
/// cut by the command's). The node offers what it can
/// (`offered_fidelity`), takes the highest allowed, and reports it in
/// `Continued`; nothing allowed is reported `HandoffBlocked`, never a
/// lower fidelity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContinuationAsk {
    /// The attempt this one continues.
    pub from: AttemptId,
    /// What the continuation allows.
    pub allowed: AllowedFidelity,
}

/// The authority's request to the node it granted an attempt to: run it.
/// Everything the node needs to prepare the root, resolve the secrets and
/// start the harness is here, so the node reads nothing from the
/// authority but the snapshot's bytes. Idempotent by `token.attempt`: a
/// second launch of an attempt the node already runs is answered
/// `AlreadyLaunched`, so an authority that restarts after it sent one
/// sends it again and loses nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchRequest {
    /// The grant.
    pub token: GrantToken,
    /// The task the attempt runs; a harness task.
    pub task: TaskId,
    /// The job's intent: the goal and the harness.
    pub intent: Intent,
    /// Where the workspace comes from when there is no checkpoint.
    pub source: SourceBinding,
    /// The job's policy: the drain budget, the secrets, the export allowlist.
    pub policy: JobPolicy,
    /// The snapshot the attempt starts from, when it has one.
    pub initial_checkpoint: Option<SnapshotId>,
    /// The node whose store holds `initial_checkpoint`'s bundle chain; the
    /// launching node pulls what it lacks from it with the existing
    /// transfer. Required when `initial_checkpoint` is `Some`.
    pub snapshot_at: Option<NodeId>,
    /// The open build and test tasks under `task` that have a program, to
    /// spawn once the harness runs; at most `MAX_TASK_LAUNCHES`.
    pub tasks: Vec<TaskLaunch>,
    /// `Some` for the new attempt of a planned handoff.
    pub continuation: Option<ContinuationAsk>,
    /// The work package of a planned handoff, built by the authority from
    /// the journal; `Some` exactly when `continuation` is.
    pub package: Option<WorkPackage>,
}

impl LaunchRequest {
    /// Refuses a launch the node must not take as sent: more than
    /// `MAX_TASK_LAUNCHES` tasks, a task's program failing its check, a
    /// checkpoint without the node that holds it, a package without a
    /// continuation or the reverse, a package past `PACKAGE_MAX_BYTES`,
    /// or one for another job. Pure; the receiving side runs it before
    /// it serves anything, since it trusts no bound it did not check.
    pub fn check(&self) -> Result<(), LaunchRefusal> {
        if self.tasks.len() > MAX_TASK_LAUNCHES {
            return Err(LaunchRefusal::Inconsistent {
                reason: format!(
                    "{} tasks, above the {MAX_TASK_LAUNCHES} bound",
                    self.tasks.len()
                ),
            });
        }
        for launch in &self.tasks {
            launch
                .program
                .check()
                .map_err(|error| LaunchRefusal::BadProgram {
                    task: launch.task,
                    error,
                })?;
        }
        if self.initial_checkpoint.is_some() && self.snapshot_at.is_none() {
            return Err(LaunchRefusal::Inconsistent {
                reason: "an initial checkpoint names no node that holds it".to_owned(),
            });
        }
        match (&self.continuation, &self.package) {
            (Some(_), None) => Err(LaunchRefusal::Inconsistent {
                reason: "a continuation carries no package".to_owned(),
            }),
            (None, Some(_)) => Err(LaunchRefusal::Inconsistent {
                reason: "a package without a continuation".to_owned(),
            }),
            (Some(_), Some(package)) => {
                if package.job != self.token.job {
                    return Err(LaunchRefusal::Inconsistent {
                        reason: "the package is another job's".to_owned(),
                    });
                }
                let bytes = package.canonical_json().len();
                if bytes > PACKAGE_MAX_BYTES {
                    return Err(LaunchRefusal::Inconsistent {
                        reason: format!("the package is {bytes} bytes, above {PACKAGE_MAX_BYTES}"),
                    });
                }
                Ok(())
            }
            (None, None) => Ok(()),
        }
    }
}

/// The node's answer to a launch, at once: whether it took the grant.
/// Taking it means the launch is durably recorded on the node; the attempt
/// is `Started` by a report once its harness process exists, which may be
/// long after a clone or a pull.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LaunchAnswer {
    /// The node recorded the launch and its runner is starting it.
    Accepted,
    /// The node did not take it.
    Refused {
        /// Why.
        reason: LaunchRefusal,
    },
}

/// Why a node refused a launch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum LaunchRefusal {
    /// The node already runs this attempt; not an error for the sender.
    #[error("the attempt is already launched here")]
    AlreadyLaunched,
    /// The node runs or ran a newer generation of the job.
    #[error("superseded by generation {current}")]
    Superseded {
        /// The newest generation the node knows for the job.
        current: Generation,
    },
    /// The harness the intent names is not installed here.
    #[error("harness {harness:?} is not installed here")]
    HarnessMissing {
        /// The harness.
        harness: HarnessKind,
    },
    /// A secret the policy names does not resolve here.
    #[error("secret {name} does not resolve here")]
    SecretUnresolved {
        /// Its name.
        name: SecretName,
    },
    /// A task's program failed its check.
    #[error("task {task}: {error}")]
    BadProgram {
        /// The task.
        task: TaskId,
        /// Why.
        error: ProgramError,
    },
    /// `initial_checkpoint` without `snapshot_at`, or a package without a
    /// continuation, or the reverse.
    #[error("the launch is inconsistent: {reason}")]
    Inconsistent {
        /// Which rule.
        reason: String,
    },
    /// The node could not record the launch.
    #[error("the node could not take the launch: {reason}")]
    Internal {
        /// Why.
        reason: String,
    },
}

/// What the authority asks of a running attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Instruction {
    /// Spawn a task registered while the attempt runs.
    Spawn(TaskLaunch),
    /// Quiesce and cut; then resume (a `Checkpoint` command) or end the
    /// attempt (a `Continue`). The node reports `Checkpointed` or
    /// `HandoffBlocked`; after `HandoffBlocked` the attempt goes on
    /// whatever `then` says.
    Checkpoint {
        /// What follows the cut.
        then: AfterCut,
    },
    /// Stop every group of the attempt (`SIGTERM`, the drain budget,
    /// `SIGKILL`) and report `Exited` with `Cancelled`; a cancel's dispatch.
    Stop {
        /// Why, for the node's log.
        reason: String,
    },
}

/// What a node does with the attempt after a cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AfterCut {
    /// Lift the freeze; the attempt continues here.
    Resume,
    /// End the attempt once the report is `Recorded`; its groups are
    /// stopped if anything still runs.
    End,
}

/// The node's answer to an instruction, at once: whether it took it. The
/// outcome comes by report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum InstructAnswer {
    /// The node took it.
    Accepted,
    /// The node did not.
    Refused {
        /// Why.
        reason: InstructRefusal,
    },
}

/// Why a node refused an instruction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum InstructRefusal {
    /// The node runs no such attempt.
    #[error("the attempt is not running here")]
    NotRunning,
    /// The token's generation is not what the node runs for the job.
    #[error("superseded by generation {current}")]
    Superseded {
        /// The generation the node runs.
        current: Generation,
    },
    /// A spawn while the attempt is quiescing or stopped.
    #[error("the attempt is frozen; nothing is spawned")]
    Frozen,
    /// A quiescence or a stop is already under way.
    #[error("the attempt is already {what}")]
    Busy {
        /// `quiescing` or `stopping`.
        what: String,
    },
    /// The program failed its check.
    #[error(transparent)]
    BadProgram(ProgramError),
    /// The node could not act.
    #[error("the node could not act: {reason}")]
    Internal {
        /// Why.
        reason: String,
    },
}

/// What a node tells the authority about an attempt it runs: one
/// observation under the grant's token. The observation names the same
/// attempt and generation as the token, or the report is refused
/// `TokenMismatch` before anything is read from the journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    /// The grant.
    pub token: GrantToken,
    /// What happened: `Started`, `Receipts`, `Checkpointed`,
    /// `HandoffBlocked`, `Continued` or `Exited` (M6 adds `Captured`).
    pub observation: Observation,
}

/// The authority's answer to a report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReportAnswer {
    /// The journal committed it; `revision` is the job's after it.
    Recorded {
        /// The job's revision after the commit.
        revision: WorkRevision,
    },
    /// The authority did not apply it.
    Refused {
        /// Why.
        reason: ReportRefusal,
    },
}

/// Why the authority refused a report. `StaleGeneration` is the fence:
/// the token is not the job's live grant, and the node stops the attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum ReportRefusal {
    /// The token's generation is older than the job's; recorded as
    /// `StaleObservationRefused`.
    #[error("the grant is stale; the job is at generation {current}")]
    StaleGeneration {
        /// The job's generation.
        current: Generation,
    },
    /// No such job here.
    #[error("unknown job")]
    UnknownJob,
    /// No such attempt of the job.
    #[error("unknown attempt")]
    UnknownAttempt,
    /// The reporting peer is not the node the attempt was granted to.
    #[error("the attempt was granted to another node")]
    WrongNode,
    /// The attempt's grant was taken (ADR 0028); `by` is the generation
    /// granted in its place, `None` when nothing was granted yet.
    #[error("the grant was superseded")]
    Superseded {
        /// The generation granted in its place, if one was.
        by: Option<Generation>,
    },
    /// The attempt is the job's but may not write: `unknown`, or ended.
    #[error("the attempt may not write: it is {status:?}")]
    NotWritable {
        /// Its status.
        status: AttemptStatus,
    },
    /// The observation names another attempt or generation than the token.
    #[error("the observation does not match the token")]
    TokenMismatch,
    /// The observation is not one a node may report (`Placed`, `Lost`,
    /// `Unknown`, `ResourceUnavailable`, `ResourceReachable`).
    #[error("not an attempt observation a node reports")]
    NotReportable,
    /// The domain refused it for another reason, by name.
    #[error("refused: {reason}")]
    Domain {
        /// The refusal's snake-case name.
        reason: String,
    },
    /// The authority is not open; the node tries again after its next heartbeat.
    #[error("the authority is unavailable")]
    Unavailable,
}

impl ReportRefusal {
    /// The refusal that names a fence verdict, `None` for `Live`: the
    /// report path and the heartbeat path refuse a grant for the same
    /// reasons (ADR 0028).
    #[must_use]
    pub fn fenced(verdict: &FenceVerdict) -> Option<Self> {
        Some(match verdict {
            FenceVerdict::Live => return None,
            FenceVerdict::Stale { current } => ReportRefusal::StaleGeneration { current: *current },
            FenceVerdict::Superseded { by } => ReportRefusal::Superseded { by: *by },
            FenceVerdict::NotWritable { status } => ReportRefusal::NotWritable {
                status: status.clone(),
            },
            FenceVerdict::WrongNode { .. } => ReportRefusal::WrongNode,
            FenceVerdict::UnknownAttempt => ReportRefusal::UnknownAttempt,
            FenceVerdict::UnknownJob => ReportRefusal::UnknownJob,
        })
    }

    /// Whether the refusal ends the grant the report was sent under: every
    /// refusal the fence makes whose verdict ends it
    /// ([`FenceVerdict::ends_grant`]). The node stops the attempt and
    /// reports nothing more under it. `NotWritable { Unknown }` waits (the
    /// next heartbeat from the grant's node brings it back), and
    /// `TokenMismatch`, `NotReportable`, `Domain` and `Unavailable` say
    /// nothing of the grant.
    #[must_use]
    pub fn ends_grant(&self) -> bool {
        match self {
            ReportRefusal::StaleGeneration { .. }
            | ReportRefusal::UnknownJob
            | ReportRefusal::UnknownAttempt
            | ReportRefusal::WrongNode
            | ReportRefusal::Superseded { .. } => true,
            ReportRefusal::NotWritable { status } => *status != AttemptStatus::Unknown,
            ReportRefusal::TokenMismatch
            | ReportRefusal::NotReportable
            | ReportRefusal::Domain { .. }
            | ReportRefusal::Unavailable => false,
        }
    }
}

impl Report {
    /// The attempt and generation an observation reports, or `None` for
    /// one a node never sends.
    #[must_use]
    pub fn reported(observation: &Observation) -> Option<(AttemptId, Generation)> {
        match observation {
            Observation::Started {
                attempt,
                generation,
            }
            | Observation::Checkpointed {
                attempt,
                generation,
                ..
            }
            | Observation::Receipts {
                attempt,
                generation,
                ..
            }
            | Observation::HandoffBlocked {
                attempt,
                generation,
                ..
            }
            | Observation::Continued {
                attempt,
                generation,
                ..
            }
            | Observation::Exited {
                attempt,
                generation,
                ..
            }
            | Observation::Captured {
                attempt,
                generation,
                ..
            } => Some((*attempt, *generation)),
            // The authority makes these itself; a node never reports them.
            Observation::Placed { .. }
            | Observation::Lost { .. }
            | Observation::Unknown { .. }
            | Observation::ResourceUnavailable { .. }
            | Observation::ResourceReachable { .. } => None,
        }
    }

    /// Refuses a report whose observation is not reportable or names
    /// another attempt or generation than its token. Pure; the journal
    /// checks come after it.
    pub fn check(&self) -> Result<(), ReportRefusal> {
        let (attempt, generation) =
            Self::reported(&self.observation).ok_or(ReportRefusal::NotReportable)?;
        if attempt != self.token.attempt || generation != self.token.generation {
            return Err(ReportRefusal::TokenMismatch);
        }
        Ok(())
    }
}

/// A node's heartbeat: every grant it runs, so the authority sees it is
/// alive and the node learns which grants still stand. Sent every
/// `HEARTBEAT_MS` while the node runs an attempt; never a journal write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Heartbeat {
    /// The grants the node runs; at most `MAX_HEARTBEAT_ATTEMPTS`.
    pub attempts: Vec<GrantToken>,
}

/// How often a node heartbeats while it runs an attempt: the interval
/// the authority's presence counts silence in (ADR 0028), one constant.
pub const HEARTBEAT_MS: u64 = super::model::HEARTBEAT_INTERVAL_MS;

/// The authority's answer: the fence's verdict on each token, in the
/// order asked (ADR 0028). A verdict that ends the grant
/// ([`FenceVerdict::ends_grant`]) stops the attempt on the node, which
/// reports nothing more under it. A token the authority could not judge
/// (its journal is closed) gets no verdict, which says nothing of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeartbeatAnswer {
    /// The verdicts.
    pub verdicts: Vec<(AttemptId, FenceVerdict)>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::ParseIdError;

    fn program(argv: &[&str]) -> Program {
        Program {
            argv: argv.iter().map(|s| (*s).to_owned()).collect(),
            cwd: None,
            env_allowlist: Vec::new(),
        }
    }

    #[test]
    fn a_program_names_a_command_within_its_bounds() {
        assert_eq!(program(&[]).check(), Err(ProgramError::EmptyArgv));
        assert_eq!(program(&["cargo", "test"]).check(), Ok(()));
        let many: Vec<&str> = std::iter::repeat_n("x", MAX_ARGV + 1).collect();
        assert_eq!(
            program(&many).check(),
            Err(ProgramError::TooManyArgs {
                count: MAX_ARGV + 1
            })
        );
        let long = "a".repeat(MAX_ARG_BYTES + 1);
        assert_eq!(
            program(&["sh", &long]).check(),
            Err(ProgramError::ArgTooLong {
                index: 1,
                bytes: MAX_ARG_BYTES + 1
            })
        );
        assert_eq!(
            program(&["sh", "-c", "echo\u{7}"]).check(),
            Err(ProgramError::ControlCharacter { index: 2 })
        );
    }

    #[test]
    fn a_report_must_match_its_token() -> Result<(), ParseIdError> {
        let job: JobId = "01K8W3A00000000000000000J1".parse()?;
        let attempt: AttemptId = "01K8W3A00000000000000000A1".parse()?;
        let other: AttemptId = "01K8W3A00000000000000000A2".parse()?;
        let token = GrantToken {
            job,
            attempt,
            generation: Generation::FIRST,
        };
        let ok = Report {
            token,
            observation: Observation::Started {
                attempt,
                generation: Generation::FIRST,
            },
        };
        assert_eq!(ok.check(), Ok(()));
        let mismatch = Report {
            token,
            observation: Observation::Started {
                attempt: other,
                generation: Generation::FIRST,
            },
        };
        assert_eq!(mismatch.check(), Err(ReportRefusal::TokenMismatch));
        let stale = Report {
            token,
            observation: Observation::Started {
                attempt,
                generation: Generation::FIRST.next().ok_or(ParseIdError::BadUlid)?,
            },
        };
        assert_eq!(stale.check(), Err(ReportRefusal::TokenMismatch));
        let lost = Report {
            token,
            observation: Observation::Lost {
                attempt,
                generation: Generation::FIRST,
            },
        };
        assert_eq!(lost.check(), Err(ReportRefusal::NotReportable));
        Ok(())
    }

    /// M6's `Captured` is a node's report; the two presence observations
    /// are the authority's own and never come from a node (ADR 0028).
    #[test]
    fn a_report_carries_captured_and_never_a_presence_observation() -> Result<(), ParseIdError> {
        let job: JobId = "01K8W3A00000000000000000J1".parse()?;
        let attempt: AttemptId = "01K8W3A00000000000000000A1".parse()?;
        let token = GrantToken {
            job,
            attempt,
            generation: Generation::FIRST,
        };
        let report = |observation| Report { token, observation };
        let captured = report(Observation::Captured {
            attempt,
            generation: Generation::FIRST,
            snapshot: SnapshotId::of(b"captured"),
            cut: super::super::lifecycle::CutDigest::from_bytes([1; 32]),
        });
        assert_eq!(captured.check(), Ok(()));
        let unavailable = report(Observation::ResourceUnavailable {
            attempt,
            generation: Generation::FIRST,
            last_heartbeat_ms: None,
            at_ms: 1,
        });
        assert_eq!(unavailable.check(), Err(ReportRefusal::NotReportable));
        let reachable = report(Observation::ResourceReachable {
            attempt,
            generation: Generation::FIRST,
        });
        assert_eq!(reachable.check(), Err(ReportRefusal::NotReportable));
        Ok(())
    }
}

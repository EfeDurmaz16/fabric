//! M2 acceptance through a real daemon (docs/managed-jobs.md): a managed
//! job keeps its identity and progress across a `SIGKILL` of the daemon,
//! a duplicate command gets the same operation before and after the
//! restart, a key reused for another command is refused and writes
//! nothing, a refusal is kept under its key, and standalone and managed
//! commands each refuse the other mode's job.

use fabric_client::api::{ManagedOperation, Reason, RefusalKind, SystemPath};
use fabric_client::{Client, ClientError};
use fabric_core::harness::HarnessKind;
use fabric_core::id::JobId;
use fabric_core::managed::{OperationKey, WorkRevision};

use crate::world::{Env, Fallible, Node, Wire};

fn key(text: &str) -> Fallible<OperationKey> {
    Ok(OperationKey::parse(text)?)
}

/// The reason of a refused call; any other outcome fails the test.
fn reason<T: std::fmt::Debug>(outcome: Result<T, ClientError>) -> Fallible<Reason> {
    match outcome {
        Err(ClientError::Refused(refusal)) => Ok(refusal.reason().clone()),
        other => Err(format!("not a refusal: {other:?}").into()),
    }
}

/// A started node, a client of it and the fixture repository as a request names it.
fn started(env: &std::sync::Arc<Env>) -> Fallible<(Node, Client, SystemPath)> {
    let mut node = Node::new(env, "a", Wire::TcpTls)?;
    node.start(None)?;
    let client = Client::new(node.state().join("fabric.sock"));
    let repo = SystemPath::parse(env.repo.to_str().ok_or("the repository is not UTF-8")?)?;
    Ok((node, client, repo))
}

async fn create(
    client: &Client,
    repo: &SystemPath,
    key: &str,
    goal: &str,
) -> Fallible<Result<ManagedOperation, ClientError>> {
    Ok(client
        .managed_create(
            OperationKey::parse(key)?,
            repo.clone(),
            goal.into(),
            HarnessKind::Codex,
            |_| {},
        )
        .await)
}

/// A cancel that expects revision 7, which the job never reached by then.
async fn stale_cancel(
    client: &Client,
    job: JobId,
) -> Fallible<Result<ManagedOperation, ClientError>> {
    Ok(client
        .managed_cancel(
            key("cancel-stale")?,
            job,
            WorkRevision::new(7),
            "late".into(),
            |_| {},
        )
        .await)
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_a_managed_job_keeps_identity_and_progress_across_a_daemon_kill() -> Fallible {
    let env = Env::new()?;
    let (mut node, client, repo) = started(&env)?;
    assert!(
        !node.state().join("managed").exists(),
        "a start made the journal"
    );

    let made = create(&client, &repo, "create-1", "make the greeting louder").await??;
    assert!(!made.replayed);
    assert_eq!(made.job.revision, WorkRevision::FIRST);
    assert_eq!(made.job.tasks.len(), 1);
    assert!(node.state().join("managed/journal.sqlite").exists());

    // The same command under the same key: the same operation and job.
    let again = create(&client, &repo, "create-1", "make the greeting louder").await??;
    assert!(again.replayed);
    assert_eq!(
        (again.operation, again.job.job),
        (made.operation, made.job.job)
    );

    // The daemon dies without a word and comes back.
    node.kill()?;
    node.start(None)?;
    let after = create(&client, &repo, "create-1", "make the greeting louder").await??;
    assert!(after.replayed);
    assert_eq!(
        (after.operation, after.job.job, after.job.primary_task),
        (made.operation, made.job.job, made.job.primary_task)
    );
    assert_eq!(after.job.revision, WorkRevision::FIRST);
    let listed = client.managed_list(|_| {}).await?;
    assert_eq!(listed.jobs.len(), 1);
    assert_eq!(listed.jobs[0].job, made.job.job);

    // Progress: a cancel at the expected revision closes the job, and it
    // stays closed across another kill.
    let cancelled = client
        .managed_cancel(
            key("cancel-1")?,
            made.job.job,
            Some(WorkRevision::FIRST),
            "done".into(),
            |_| {},
        )
        .await?;
    assert_eq!(cancelled.job.state, "cancelled");
    assert_eq!(cancelled.job.revision, WorkRevision::new(2).ok_or("two")?);
    node.kill()?;
    node.start(None)?;
    let shown = client.managed_status(made.job.job, |_| {}).await?;
    assert_eq!(shown, cancelled.job);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_managed_refusals_are_kept_and_modes_stay_apart() -> Fallible {
    let env = Env::new()?;
    let (mut node, client, repo) = started(&env)?;
    let made = create(&client, &repo, "create-1", "make the greeting louder").await??;

    // The same key with another command is refused and writes nothing.
    let conflict = create(&client, &repo, "create-1", "something else").await?;
    assert!(matches!(
        reason(conflict)?,
        Reason::OperationConflict { .. }
    ));
    assert_eq!(client.managed_list(|_| {}).await?.jobs.len(), 1);

    // A refusal is kept under its key, across a kill.
    let stale = stale_cancel(&client, made.job.job).await?;
    let Err(ClientError::Refused(refusal)) = &stale else {
        return Err(format!("not a refusal: {stale:?}").into());
    };
    assert_eq!(refusal.kind(), RefusalKind::JobState);
    assert!(matches!(
        refusal.reason(),
        Reason::ManagedRefused { refusal, replayed: false, .. } if refusal == "revision_mismatch"
    ));
    node.kill()?;
    node.start(None)?;
    assert!(matches!(
        reason(stale_cancel(&client, made.job.job).await?)?,
        Reason::ManagedRefused { replayed: true, .. }
    ));

    // A standalone command refuses the managed job by its mode.
    assert!(matches!(
        reason(client.job_status(made.job.job, |_| {}).await)?,
        Reason::ManagedJob { .. }
    ));
    let printed = node.fails(&["job", "status", &made.job.job.to_string()])?;
    assert!(printed.contains("is a managed job"), "{printed}");

    // A managed command refuses a standalone job, and makes nothing for it.
    let standalone: JobId = crate::new_job(&env, &node, crate::AGENT)?.parse()?;
    assert!(matches!(
        reason(client.managed_status(standalone, |_| {}).await)?,
        Reason::StandaloneJob { .. }
    ));
    assert!(matches!(
        reason(
            client
                .managed_cancel(key("c")?, standalone, None, "x".into(), |_| {})
                .await
        )?,
        Reason::StandaloneJob { .. }
    ));
    assert_eq!(client.managed_list(|_| {}).await?.jobs.len(), 1);
    Ok(())
}

/// `fabric managed` makes, reads, lists and cancels managed jobs through
/// the same calls: a command run again with its `--operation-key` is the
/// same command, one run without a key gets a fresh one it prints, and
/// the same key with another command, or a stale revision, is refused.
#[test]
fn e2e_the_cli_makes_reads_and_cancels_managed_jobs_under_their_keys() -> Fallible {
    let env = Env::new()?;
    let mut node = Node::new(&env, "a", Wire::TcpTls)?;
    node.start(None)?;
    let repo = env.repo.to_str().ok_or("the repository is not UTF-8")?;
    let create = |key: Option<&str>, goal: &str| {
        let mut args = vec![
            "managed",
            "create",
            "--repo",
            repo,
            "--goal",
            goal,
            "--harness",
            "codex",
        ];
        if let Some(key) = key {
            args.extend(["--operation-key", key]);
        }
        node.run(&args)
    };
    let job_of = |printed: &str| -> Fallible<String> {
        let line = printed.lines().next().ok_or("nothing printed")?;
        let _: JobId = line.parse()?;
        Ok(line.to_owned())
    };

    let made = create(Some("make-1"), "make the greeting louder")?;
    assert!(made.status.success(), "{}", crate::world::text(&made));
    let job = job_of(&String::from_utf8(made.stdout)?)?;
    let again = node.ok(&[
        "managed",
        "create",
        "--repo",
        repo,
        "--goal",
        "make the greeting louder",
        "--harness",
        "codex",
        "--operation-key",
        "make-1",
    ])?;
    assert_eq!(job_of(&again)?, job, "{again}");
    assert!(again.contains("answered again for key make-1"), "{again}");

    let fresh = create(None, "another goal")?;
    let (stdout, stderr) = (
        String::from_utf8(fresh.stdout.clone())?,
        String::from_utf8(fresh.stderr.clone())?,
    );
    assert!(fresh.status.success(), "{stdout}{stderr}");
    assert!(stderr.starts_with("operation key "), "{stderr}");
    assert_ne!(job_of(&stdout)?, job);

    let conflict = create(Some("make-1"), "something else")?;
    let said = crate::world::text(&conflict);
    assert_eq!(conflict.status.code(), Some(1), "{said}");
    assert!(said.contains("was used for another command"), "{said}");

    let status = node.ok(&["managed", "status", &job])?;
    assert!(status.contains("managed job at revision 1"), "{status}");
    let listed = node.ok(&["managed", "ls"])?;
    assert!(listed.contains(&job), "{listed}");

    let stale = node.fails(&[
        "managed",
        "cancel",
        &job,
        "--expected-revision",
        "7",
        "--operation-key",
        "cancel-stale",
    ])?;
    assert!(stale.contains("revision"), "{stale}");
    let cancelled = node.ok(&[
        "managed",
        "cancel",
        &job,
        "--expected-revision",
        "1",
        "--reason",
        "done",
        "--operation-key",
        "cancel-1",
    ])?;
    assert!(cancelled.contains("cancelled"), "{cancelled}");
    Ok(())
}

/// The launch path's client surface (docs/managed-jobs.md, "Launch path"):
/// `fabric managed register` records a test task with its program under
/// the job's primary task, the answer and `managed status` show it with
/// what it runs, and a program that carries a credential is refused
/// `bad_program` before the journal records anything.
#[test]
fn e2e_the_cli_registers_a_test_task_with_its_program() -> Fallible {
    let env = Env::new()?;
    let mut node = Node::new(&env, "a", Wire::TcpTls)?;
    node.start(None)?;
    let repo = env.repo.to_str().ok_or("the repository is not UTF-8")?;
    let made = node.ok(&[
        "managed",
        "create",
        "--repo",
        repo,
        "--goal",
        "make the tests pass",
        "--harness",
        "codex",
        "--operation-key",
        "make-1",
    ])?;
    let job = made.lines().next().ok_or("nothing printed")?.to_owned();
    let primary = made
        .lines()
        .find_map(|line| {
            line.strip_prefix("task ")?
                .strip_suffix(" (primary): open")
                .map(str::to_owned)
        })
        .ok_or_else(|| format!("no open primary task in {made}"))?;

    let registered = node.ok(&[
        "managed",
        "register",
        &job,
        "--parent",
        &primary,
        "--label",
        "unit tests",
        "--class",
        "test",
        "--cwd",
        "src",
        "--env",
        "CARGO_HOME",
        "--operation-key",
        "register-1",
        "--",
        "sh",
        "-c",
        "exit 0",
    ])?;
    assert!(
        registered.contains("(unit tests): open"),
        "the answer names the task: {registered}"
    );
    let runs = r#"  runs "sh" "-c" "exit 0" in src"#;
    assert!(registered.contains(runs), "{registered}");
    let status = node.ok(&["managed", "status", &job])?;
    assert!(status.contains(runs), "status shows the program: {status}");

    let again = node.ok(&[
        "managed",
        "register",
        &job,
        "--parent",
        &primary,
        "--label",
        "unit tests",
        "--class",
        "test",
        "--cwd",
        "src",
        "--env",
        "CARGO_HOME",
        "--operation-key",
        "register-1",
        "--",
        "sh",
        "-c",
        "exit 0",
    ])?;
    assert!(
        again.contains("answered again for key register-1"),
        "{again}"
    );

    let token = format!("ghp_{}", "a".repeat(36));
    let refused = node.fails(&[
        "managed", "register", &job, "--parent", &primary, "--label", "leaky", "--class", "build",
        "--", "curl", "-H", &token,
    ])?;
    assert!(refused.contains("bad program"), "{refused}");
    assert!(
        !refused.contains(&token),
        "the refusal echoes no credential: {refused}"
    );
    let wrong = node.fails(&[
        "managed", "register", &job, "--parent", &primary, "--label", "x", "--class", "harness",
        "--", "true",
    ])?;
    assert!(wrong.contains("build or test"), "{wrong}");
    Ok(())
}

/// The provider read surface carried from M4, through the CLI: a job's
/// journal seeded, while its daemon is down, with provider operations in
/// `unknown` (with and without the scope an absent attach searched),
/// `attached`, `observed`, `abandoned` and a destroy `dispatched`, which the
/// restarted daemon reconciles to `unknown`; `fabric managed
/// inspect` prints each status as it is, each blocker kind and each
/// residual kind, with the target, the search and the abandonment reason.
#[test]
#[expect(clippy::too_many_lines, reason = "the seed reads best as one script")]
fn e2e_the_cli_inspects_provider_operations_blockers_and_residuals() -> Fallible {
    use fabric_core::id::NodeId;
    use fabric_core::managed::{
        AttemptId, Command, Envelope, NewIds, OperationId, RequestDigest, ResourceRef, TaskId,
    };
    use fabric_journal::{
        AbandonApproval, Acknowledged, EffectClass, Journal, LocatorRecord, ProviderAnswer,
        ProviderDispatch, ProviderStatus, Purpose, ResourceKey,
    };

    let env = Env::new()?;
    let mut node = Node::new(&env, "a", Wire::TcpTls)?;
    node.start(None)?;
    // Pinned to a node that is not paired: the job is parked and no
    // attempt of it runs anywhere while the journal is seeded.
    let elsewhere = "fab1ambqgaydambqgaydambqgaydambqgaydambqgaydambqgaydambq";
    let made = node.ok(&[
        "managed",
        "create",
        "--empty",
        "--goal",
        "provision a sandbox",
        "--harness",
        "codex",
        "--pin",
        elsewhere,
        "--operation-key",
        "make-1",
    ])?;
    let job: JobId = made.lines().next().ok_or("nothing printed")?.parse()?;
    node.kill()?;

    let path = node.state().join("managed").join(fabric_journal::FILE_NAME);
    let mut journal = Journal::open(&path)?;
    let state = journal.job(job)?.ok_or("the job is not in the journal")?;
    let at = 1_762_128_000_000;
    journal.submit(
        &Envelope {
            key: key("start-seeded")?,
            job: Some(job),
            expected: None,
            body: Command::StartAttempt {
                task: state.job.primary,
                resource: ResourceRef::Node {
                    node: elsewhere.parse::<NodeId>()?,
                },
                initial_checkpoint: None,
            },
        },
        NewIds {
            job,
            task: TaskId::from_bytes([0x61; 16]),
            attempt: AttemptId::from_bytes([0x62; 16]),
            operation: OperationId::from_bytes([0x63; 16]),
            at_ms: at,
        },
    )?;
    let attempt = journal
        .job(job)?
        .and_then(|s| s.attempts.last().cloned())
        .ok_or("no attempt was granted")?;
    let locator = LocatorRecord {
        text: r#"{"kind":"sandbox","native_id":"sb-1","provider":"fixture"}"#.into(),
        key: ResourceKey::from_bytes([0x45; 32]),
    };
    let searched = r#"{"kind":"sandbox","provider":"fixture"}"#;
    let op = |n: u8| OperationId::from_bytes([0x40 + n; 16]);
    let dispatch =
        |n: u8, action: &str, target: Option<LocatorRecord>| -> Fallible<ProviderDispatch> {
            Ok(ProviderDispatch {
                operation: op(n),
                key: key(&format!("provider-{n}"))?,
                digest: RequestDigest::from_bytes([n; 32]),
                job,
                task: attempt.task,
                attempt: attempt.id,
                generation: attempt.generation,
                profile: "fixture".into(),
                action: action.into(),
                effect: match action {
                    "sandbox.create" => EffectClass::Create,
                    "sandbox.destroy" => EffectClass::Destroy,
                    _ => EffectClass::Mutate,
                },
                purpose: Purpose::Effect,
                target,
            })
        };
    let lost = ProviderAnswer {
        code: Some("timeout".into()),
        reason: Some("timeout".into()),
        effect: Some("unknown".into()),
    };
    let none = ProviderAnswer::default();
    for n in [1, 2, 3] {
        journal.provider_begin(&dispatch(n, "sandbox.create", None)?, None, at)?;
    }
    journal.provider_record(op(1), ProviderStatus::Unknown, &lost, None, at)?;
    journal.provider_record(op(2), ProviderStatus::Unknown, &lost, None, at)?;
    journal.provider_searched(op(2), &lost, searched, at)?;
    journal.provider_record(op(3), ProviderStatus::Unknown, &lost, None, at)?;
    journal.provider_record(op(3), ProviderStatus::Attached, &none, Some(&locator), at)?;
    journal.provider_begin(
        &dispatch(4, "sandbox.exec", Some(locator.clone()))?,
        None,
        at,
    )?;
    journal.provider_record(op(4), ProviderStatus::Unknown, &lost, None, at)?;
    journal.provider_record(op(4), ProviderStatus::Observed, &none, None, at)?;
    journal.provider_begin(
        &dispatch(5, "sandbox.exec", Some(locator.clone()))?,
        None,
        at,
    )?;
    journal.provider_record(op(5), ProviderStatus::Unknown, &lost, None, at)?;
    journal.provider_abandon(
        &AbandonApproval {
            operation: op(5),
            expected: ProviderStatus::Unknown,
            acknowledged: Acknowledged::Resource(locator.key),
            reason: "the console shows it ended".into(),
        },
        at,
    )?;
    journal.provider_begin(
        &dispatch(6, "sandbox.destroy", Some(locator.clone()))?,
        None,
        at,
    )?;
    drop(journal);

    node.start(None)?;
    let shown = node.ok(&["managed", "inspect", &job.to_string()])?;
    for (n, status) in [
        (1, "unknown"),
        (2, "unknown"),
        (3, "attached"),
        (4, "observed"),
        (5, "abandoned"),
        // Recorded dispatched; the daemon's start-up reconciliation made it
        // unknown, since nobody can tell whether it reached the provider.
        (6, "unknown"),
    ] {
        let line = format!("provider operation {}: ", op(n));
        let at = shown
            .find(&line)
            .ok_or_else(|| format!("no line for {}: {shown}", op(n)))?;
        let line = shown[at..].lines().next().unwrap_or_default();
        assert!(
            line.ends_with(&format!("({status})")) || line.contains(&format!("({status}) at ")),
            "{line}"
        );
    }
    assert!(shown.contains(&format!("  searched {searched}")), "{shown}");
    assert!(
        shown.contains(&format!("  sent to {}", locator.text)),
        "{shown}"
    );
    assert!(
        shown.contains("  abandoned: the console shows it ended"),
        "{shown}"
    );
    for (kind, n, status) in [
        ("create_unresolved", 1, "unknown"),
        ("exec_may_run", 5, "abandoned"),
        ("stop_not_confirmed", 6, "unknown"),
    ] {
        let line = format!("blocks a replacement: {kind} {} ({status})", op(n));
        assert!(shown.contains(&line), "{line} in {shown}");
    }
    assert!(
        shown.contains(&format!(
            "may remain: the resource of {} (attached) at {}",
            op(3),
            locator.text
        )),
        "{shown}"
    );
    assert!(
        shown.contains(&format!(
            "may remain: the resource of {} (unknown), not found in {searched}",
            op(2)
        )),
        "{shown}"
    );
    Ok(())
}

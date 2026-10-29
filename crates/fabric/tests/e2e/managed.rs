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

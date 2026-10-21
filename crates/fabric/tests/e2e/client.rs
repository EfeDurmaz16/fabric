//! A real node driven through `fabric-client` instead of the CLI: the
//! replies are typed and agree with what the CLI prints, a suspend streams
//! its steps as events before its reply, a refusal arrives with its reason,
//! a client of another api version is turned away at the hello, and the
//! node measures in the environment of the CLI that asked, not its own.

use fabric_client::api::{
    Answer, Decision, Event, HelloAnswer, LedgerEvent, Marker, Process, Reason, RefusalKind, Reply,
    Request, Standing, SystemPath, View, WorkspaceState,
};
use fabric_client::{Client, ClientError, frame};
use fabric_core::harness::HarnessKind;
use fabric_core::id::{Epoch, JobId};
use serde_json::json;
use tokio::net::UnixStream;

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::world::{Env, Fallible, Node, Wire, finish, pair, text};

/// A started node, and a client of it.
fn started(env: &std::sync::Arc<Env>) -> Fallible<(Node, Client)> {
    let mut node = Node::new(env, "a", Wire::Quic)?;
    node.start(None)?;
    let client = Client::new(node.state().join("fabric.sock"));
    Ok((node, client))
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_a_client_makes_reads_decides_and_suspends_a_job_with_typed_answers() {
    let env = Env::new().unwrap();
    let (node, client) = started(&env).unwrap();

    let welcome = client.hello().await.unwrap();
    assert_eq!(welcome.node.to_string(), node.id);
    let identity = client.node_id(|_| {}).await.unwrap();
    assert_eq!(identity.node, welcome.node);
    assert!(client.job_list(|_| {}).await.unwrap().jobs.is_empty());

    let created = client
        .job_new(
            SystemPath::parse(env.repo.to_str().unwrap()).unwrap(),
            "make the greeting louder".to_owned(),
            HarnessKind::ClaudeCode,
            |_| {},
        )
        .await
        .unwrap();
    assert_eq!(created.epoch, Epoch::FIRST);
    assert_eq!(created.root, env.work(&created.job.to_string()));
    let listed = client.job_list(|_| {}).await.unwrap();
    assert_eq!(listed.jobs.len(), 1);
    assert_eq!(listed.jobs[0].job, created.job);
    assert_eq!(listed.jobs[0].standing, Standing::Owner);

    let status = client.job_status(created.job, |_| {}).await.unwrap();
    assert_eq!((status.owner, status.view), (welcome.node, View::Owner));
    assert_eq!(status.snapshot, None);
    assert_eq!(
        status.workspace.map(|workspace| workspace.state),
        Some(WorkspaceState::Current {
            marker: Marker::Live
        })
    );
    assert_eq!(status.process, Process::NotRunning);
    // The CLI prints the same status from the same reply.
    let printed = node
        .ok(&["job", "status", &created.job.to_string()])
        .unwrap();
    assert!(printed.contains(&format!("owner: {} (this node) at epoch 1", node.id)));

    let text = client
        .job_decide(created.job, Decision::Approve, "ship it".to_owned(), |_| {})
        .await
        .unwrap();
    assert_eq!(text, "approved: ship it");

    let mut events = Vec::new();
    let suspended = client
        .job_suspend(created.job, None, |event| events.push(event))
        .await
        .unwrap();
    let published = events.iter().find_map(|event| match event {
        Event::SnapshotPublished { snapshot, .. } => Some(*snapshot),
        _ => None,
    });
    assert_eq!(published, Some(suspended.snapshot));
    assert!(
        matches!(events.last(), Some(Event::Suspended { snapshot, epoch }) if *snapshot == suspended.snapshot && *epoch == Epoch::FIRST),
        "{events:?}"
    );

    let status = client.job_status(created.job, |_| {}).await.unwrap();
    let snapshot = status.snapshot.unwrap();
    assert_eq!(snapshot.id, suspended.snapshot);
    assert!(snapshot.suspended);
    let log = client.job_log(created.job, |_| {}).await.unwrap();
    let kinds: Vec<&LedgerEvent> = log.records.iter().map(|record| &record.event).collect();
    assert!(matches!(
        kinds.as_slice(),
        [
            LedgerEvent::Created { .. },
            LedgerEvent::DecisionRecorded { .. },
            LedgerEvent::SnapshotPublished { .. },
            LedgerEvent::Suspended { .. }
        ]
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_a_refusal_from_a_node_arrives_with_its_reason() {
    let env = Env::new().unwrap();
    let (_node, client) = started(&env).unwrap();
    let job: JobId = "01JABCDEFGHJKMNPQRSTVWXYZ0".parse().unwrap();
    let refused = client.job_status(job, |_| {}).await;
    let Err(ClientError::Refused(refusal)) = refused else {
        panic!("{refused:?}");
    };
    assert_eq!(refusal.kind(), RefusalKind::NotOwner);
    assert_eq!(refusal.reason(), &Reason::UnknownJob { job });
    assert!(refusal.message().contains("is not known on this node"));
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_a_node_turns_away_another_api_version_and_a_request_it_cannot_read() {
    let env = Env::new().unwrap();
    let (node, _client) = started(&env).unwrap();
    let socket = node.state().join("fabric.sock");

    let mut stream = UnixStream::connect(&socket).await.unwrap();
    frame::write(&mut stream, &json!({"api": 999, "client": "future"}))
        .await
        .unwrap();
    let answer: HelloAnswer = frame::read(&mut stream).await.unwrap().unwrap();
    assert!(
        matches!(answer, HelloAnswer::Incompatible { api: 1, .. }),
        "{answer:?}"
    );
    assert!(frame::read_bytes(&mut stream).await.unwrap().is_none());

    let mut stream = UnixStream::connect(&socket).await.unwrap();
    frame::write(&mut stream, &json!({"api": 1, "client": "test"}))
        .await
        .unwrap();
    let _welcome: HelloAnswer = frame::read(&mut stream).await.unwrap().unwrap();
    frame::write(&mut stream, &json!({"op": "job_teleport"}))
        .await
        .unwrap();
    let answer: Answer = frame::read(&mut stream).await.unwrap().unwrap();
    let Answer::Refused { refusal } = answer else {
        panic!("{answer:?}");
    };
    assert_eq!(refusal.reason(), &Reason::BadRequest);

    // A hello or a request written as a JSON array, its fields in the
    // Rust declaration order, is not read (review finding on PR #13).
    let mut stream = UnixStream::connect(&socket).await.unwrap();
    frame::write(&mut stream, &json!([1, "positional"]))
        .await
        .unwrap();
    let answer: HelloAnswer = frame::read(&mut stream).await.unwrap().unwrap();
    assert!(
        matches!(answer, HelloAnswer::Incompatible { api: 1, .. }),
        "{answer:?}"
    );
    let mut stream = UnixStream::connect(&socket).await.unwrap();
    frame::write(&mut stream, &json!({"api": 1, "client": "test"}))
        .await
        .unwrap();
    let _welcome: HelloAnswer = frame::read(&mut stream).await.unwrap().unwrap();
    frame::write(
        &mut stream,
        &json!(["job_status", "01JABCDEFGHJKMNPQRSTVWXYZ0"]),
    )
    .await
    .unwrap();
    let answer: Answer = frame::read(&mut stream).await.unwrap().unwrap();
    let Answer::Refused { refusal } = answer else {
        panic!("{answer:?}");
    };
    assert_eq!(refusal.reason(), &Reason::BadRequest);

    // The same request as a typed client sends it is served.
    let mut stream = UnixStream::connect(&socket).await.unwrap();
    frame::write(&mut stream, &json!({"api": 1, "client": "test"}))
        .await
        .unwrap();
    let _welcome: HelloAnswer = frame::read(&mut stream).await.unwrap().unwrap();
    frame::write(&mut stream, &Request::JobList).await.unwrap();
    let answer: Answer = frame::read(&mut stream).await.unwrap().unwrap();
    assert!(
        matches!(
            answer,
            Answer::Done {
                reply: Reply::JobList(_)
            }
        ),
        "{answer:?}"
    );
}

/// Review finding on PR #13: a job record that does not read made `fabric
/// job status` print only the error, where before the client API it
/// printed every line up to the failing record. The node now reports each
/// record that does not read in its place, and the CLI prints the lines
/// before the first one and then fails with its sentence, exit 1, as it did.
#[tokio::test(flavor = "multi_thread")]
async fn e2e_a_record_that_does_not_read_leaves_the_rest_of_the_status() {
    let env = Env::new().unwrap();
    let (node, client) = started(&env).unwrap();
    let job = client
        .job_new(
            SystemPath::parse(env.repo.to_str().unwrap()).unwrap(),
            "make the greeting louder".to_owned(),
            HarnessKind::ClaudeCode,
            |_| {},
        )
        .await
        .unwrap()
        .job;
    let id = job.to_string();
    let dir = node.state().join("jobs").join(&id);
    let status = |printed: usize, record: &str| {
        let output = node.run(&["job", "status", &id]).unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        assert_eq!(output.status.code(), Some(1), "{stdout}{stderr}");
        assert_eq!(stdout.lines().count(), printed, "{stdout}");
        assert!(stdout.starts_with(&format!("job {id}\n")), "{stdout}");
        let path = dir.join(record);
        assert!(
            stderr.starts_with("error: ") && stderr.contains(path.to_str().unwrap()),
            "{stderr}"
        );
    };

    std::fs::write(dir.join("harness"), "not a record").unwrap();
    status(7, "harness");
    let read = client.job_status(job, |_| {}).await.unwrap();
    assert!(matches!(read.process, Process::Unreadable { .. }));
    assert_eq!(read.view, View::Owner);
    assert!(read.workspace.is_some());

    std::fs::write(dir.join("marker"), "not a record").unwrap();
    status(6, "marker");
    let read = client.job_status(job, |_| {}).await.unwrap();
    assert!(read.workspace.is_none());
    assert!(
        read.workspace_unreadable
            .is_some_and(|error| error.contains("marker"))
    );

    std::fs::write(dir.join("forked"), "not a record").unwrap();
    status(4, "forked");
    let read = client.job_status(job, |_| {}).await.unwrap();
    assert!(matches!(read.view, View::Unreadable { .. }));
    assert_eq!(read.goal, "make the greeting louder");
}

/// Review finding on PR #13: a node measured for its peers, a peer's
/// preflight and the capture a peer's pull asks for, in the environment
/// the daemon started with, while it measured for its own CLI in the
/// CLI's. With `CARGO_HOME` set in the shell and not for the daemons,
/// `doctor --peer` and a pull compared the shell's value with the daemon's
/// and rebuilt `target/` where they carried it before. A node now keeps the
/// environment its last local client sent, across a restart, and measures
/// for its peers with it.
#[test]
fn e2e_a_node_measures_for_its_peers_in_the_environment_its_cli_sent() {
    let env = Env::new().unwrap();
    let mut a = Node::new(&env, "a", Wire::Quic).unwrap();
    let mut b = Node::new(&env, "b", Wire::Quic).unwrap();
    a.start(None).unwrap();
    b.start(None).unwrap();
    pair(&a, &b).unwrap();
    let cargo = env.root.join("shell-cargo");
    let shell = [("CARGO_HOME", cargo.as_path())];
    let peer = b.id.clone();
    let compared = |a: &Node| {
        let doctor = a.spawn_with(&["doctor", "--peer", &peer], &shell).unwrap();
        let output = finish(doctor, Duration::from_mins(1)).unwrap();
        let printed = text(&output);
        assert!(output.status.success(), "{printed}");
        printed
            .lines()
            .find(|line| line.contains("cargo.home"))
            .unwrap_or_else(|| panic!("no cargo.home row in:\n{printed}"))
            .to_owned()
    };
    // Until a client of b sends an environment, b measures in its own.
    let row = compared(&a);
    assert!(row.starts_with("rebuild"), "{row}");

    let local = b.spawn_with(&["doctor"], &shell).unwrap();
    assert!(
        finish(local, Duration::from_mins(1))
            .unwrap()
            .status
            .success()
    );
    let row = compared(&a);
    assert!(row.starts_with("ok"), "{row}");
    assert!(row.contains(cargo.to_str().unwrap()), "{row}");

    b.kill().unwrap();
    b.start(None).unwrap();
    let row = compared(&a);
    assert!(row.starts_with("ok"), "{row}");
}

/// Review finding on PR #13: `fabric serve` exited at start when its own
/// `PATH` or a tool root named a path that is not UTF-8, applying the
/// client API's rule to an environment that never crosses the socket. It
/// starts and answers, as it did before the client API.
#[test]
fn e2e_a_daemon_whose_own_environment_is_not_utf8_starts() {
    let env = Env::new().unwrap();
    let mut node = Node::new(&env, "a", Wire::Quic).unwrap();
    let mut path = env.path().to_owned();
    path.push(OsStr::from_bytes(b":/opt/caf\xe9/bin"));
    node.daemon_env("PATH", path);
    node.daemon_env(
        "CARGO_HOME",
        OsStr::from_bytes(b"/home/\xff/.cargo").to_owned(),
    );
    node.start(None).unwrap();
    assert!(node.ok(&["node", "id"]).unwrap().contains(&node.id));
}

/// Review finding on PR #8: capture measured carry keys with the node's
/// `CARGO_HOME` instead of the requesting CLI's. The node here starts with
/// neither `CARGO_HOME` nor `PNPM_HOME` nor the CLI's tool directory, so
/// every value below can only come from the CLI's environment. The tool
/// directory and `CARGO_HOME` are written with `//` and a trailing `/`,
/// as a shell resolves them and the core's one text form does not, and
/// still reach the node (review finding on PR #13).
#[test]
fn e2e_doctor_measures_in_the_clis_environment_not_the_nodes() {
    let env = Env::new().unwrap();
    let (node, _client) = started(&env).unwrap();
    let bin = env.root.join("cli-bin");
    std::fs::create_dir(&bin).unwrap();
    let pnpm = bin.join("pnpm");
    std::fs::write(
        &pnpm,
        "#!/bin/sh\ncase \"$1\" in\n--version) echo 9.9.9 ;;\nstore) echo \"$PNPM_HOME/store\" ;;\nesac\n",
    )
    .unwrap();
    std::fs::set_permissions(&pnpm, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut path = env.root.clone().into_os_string();
    path.push("//cli-bin/:");
    path.push(env.path());
    let cargo = PathBuf::from(format!("{}//cli-cargo/", env.root.display()));
    let pnpm_home = env.root.join("cli-pnpm");
    let doctor = node
        .spawn_with(
            &["doctor"],
            &[
                ("PATH", Path::new(&path)),
                ("CARGO_HOME", &cargo),
                ("PNPM_HOME", &pnpm_home),
            ],
        )
        .unwrap();
    let output = finish(doctor, Duration::from_mins(1)).unwrap();
    let printed = text(&output);
    assert!(output.status.success(), "{printed}");
    assert!(
        printed.contains(&format!("cargo.home: {}", cargo.display())),
        "{printed}"
    );
    assert!(printed.contains("pnpm.version: 9.9.9"), "{printed}");
    assert!(
        printed.contains(&format!("pnpm.store: {}/store", pnpm_home.display())),
        "{printed}"
    );
}

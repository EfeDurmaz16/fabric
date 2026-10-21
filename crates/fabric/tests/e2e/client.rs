//! A real node driven through `fabric-client` instead of the CLI: the
//! replies are typed and agree with what the CLI prints, a suspend streams
//! its steps as events before its reply, a refusal arrives with its reason,
//! and a client of another api version is turned away at the hello.

use fabric_client::api::{
    Answer, Decision, Event, HelloAnswer, LedgerEvent, Marker, Process, Reason, RefusalKind, Reply,
    Request, Standing, View, WorkspaceState,
};
use fabric_client::{Client, ClientError, frame};
use fabric_core::harness::HarnessKind;
use fabric_core::id::{Epoch, JobId};
use serde_json::json;
use tokio::net::UnixStream;

use crate::world::{Env, Fallible, Node, Wire};

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
            env.repo.clone(),
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

//! What `Client` does with each kind of answer, against a scripted node on
//! a temporary socket: an event of an unknown kind is skipped, a reply for
//! another op is refused, an incompatible node, a welcome of another api
//! version, and an op or a root the node does not list stop the call
//! before the request is sent, and a node that closes after the hello
//! reads as an older node.

use std::error::Error;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use fabric_client::api::{ClientEnv, Event, PathDir, Request, Roots, SystemPath};
use fabric_client::{Client, ClientError, frame};
use serde_json::{Value, json};
use tokio::net::UnixListener;
use tokio::task::JoinHandle;

type Fallible<T> = Result<T, Box<dyn Error + Send + Sync>>;

const NODE: &str = "fab1aeaqcaibaeaqcaibaeaqcaibaeaqcaibaeaqcaibaeaqcaibaeaq";
const JOB: &str = "01JABCDEFGHJKMNPQRSTVWXYZ0";

/// What the scripted node does after it read the hello.
enum Script {
    /// Answers the hello with `welcome`, reads the request and sends `answers`.
    Serve { welcome: Value, answers: Vec<Value> },
    /// Answers the hello with `answer` and closes.
    Hello(Value),
    /// Closes without answering.
    Close,
}

/// A scripted node: its directory, its socket, and the task that returns the request it read, if any.
type Node = (
    tempfile::TempDir,
    PathBuf,
    JoinHandle<Fallible<Option<Value>>>,
);

/// A node that serves one connection by `script`.
fn node(script: Script) -> Fallible<Node> {
    let dir = tempfile::tempdir()?;
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = dir.path().join("fabric.sock");
    let listener = UnixListener::bind(&socket)?;
    let served = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await?;
        let _hello: Option<Value> = frame::read(&mut stream).await?;
        match script {
            Script::Close => Ok(None),
            Script::Hello(answer) => {
                frame::write(&mut stream, &answer).await?;
                // A client that sends a request anyway would show up here.
                Ok(frame::read(&mut stream).await?)
            }
            Script::Serve { welcome, answers } => {
                frame::write(&mut stream, &welcome).await?;
                let request: Option<Value> = frame::read(&mut stream).await?;
                for answer in &answers {
                    frame::write(&mut stream, answer).await?;
                }
                Ok(request)
            }
        }
    });
    Ok((dir, socket, served))
}

fn welcome(ops: &[&str]) -> Value {
    json!({"type": "welcome", "api": 1, "node": NODE, "node_version": "0.1.0", "capabilities": ops})
}

fn status_request() -> Fallible<Request> {
    Ok(Request::JobStatus { job: JOB.parse()? })
}

#[tokio::test]
async fn events_reach_the_caller_in_order_and_unknown_kinds_are_skipped() {
    let (_dir, socket, served) = node(Script::Serve {
        welcome: welcome(&["job_suspend"]),
        answers: vec![
            json!({"type": "event", "event": {"kind": "stopping_harness", "pid": 7}}),
            json!({"type": "event", "event": {"kind": "teleported", "to": "mars"}}),
            json!({"type": "event", "event": {"kind": "catching_up"}}),
            json!({"type": "done", "reply": {"kind": "job_suspend", "snapshot": "ab".repeat(32)}}),
        ],
    })
    .unwrap();
    let mut events = Vec::new();
    let suspended = Client::new(&socket)
        .job_suspend(JOB.parse().unwrap(), None, |event| events.push(event))
        .await
        .unwrap();
    assert_eq!(suspended.snapshot.to_string(), "ab".repeat(32));
    assert!(matches!(
        events.as_slice(),
        [Event::StoppingHarness { pid }, Event::CatchingUp] if pid.get() == 7
    ));
    assert_eq!(served.await.unwrap().unwrap().unwrap()["op"], "job_suspend");
}

#[tokio::test]
async fn a_reply_for_another_op_is_refused() {
    let (_dir, socket, _served) = node(Script::Serve {
        welcome: welcome(&["job_status"]),
        answers: vec![json!({"type": "done", "reply": {"kind": "harness_exited"}})],
    })
    .unwrap();
    let got = Client::new(&socket)
        .call(&status_request().unwrap(), |_| {})
        .await;
    assert!(
        matches!(got, Err(ClientError::Unexpected { op, got }) if op.name() == "job_status" && got.name() == "harness_exited"),
        "{got:?}"
    );
}

#[tokio::test]
async fn an_incompatible_node_gets_no_request() {
    let (_dir, socket, served) = node(Script::Hello(
        json!({"type": "incompatible", "api": 2, "node_version": "0.2.0"}),
    ))
    .unwrap();
    let got = Client::new(&socket)
        .call(&status_request().unwrap(), |_| {})
        .await;
    assert!(
        matches!(&got, Err(ClientError::Incompatible { client: 1, node: 2, node_version }) if node_version == "0.2.0"),
        "{got:?}"
    );
    assert_eq!(served.await.unwrap().unwrap(), None);
}

/// ADR 0018, Compatibility rule 1: the client checks the number in the
/// welcome too, so a node that welcomes it at another api version gets
/// no request (review finding on PR #13).
#[tokio::test]
async fn a_welcome_of_another_api_version_gets_no_request() {
    let mut answer = welcome(&["job_status"]);
    answer["api"] = json!(2);
    let (_dir, socket, served) = node(Script::Hello(answer)).unwrap();
    let got = Client::new(&socket)
        .call(&status_request().unwrap(), |_| {})
        .await;
    assert!(
        matches!(&got, Err(ClientError::Incompatible { client: 1, node: 2, node_version }) if node_version == "0.1.0"),
        "{got:?}"
    );
    assert_eq!(served.await.unwrap().unwrap(), None);
}

#[tokio::test]
async fn an_op_the_node_does_not_list_is_not_sent() {
    let (_dir, socket, served) = node(Script::Hello(welcome(&["job_list"]))).unwrap();
    let got = Client::new(&socket)
        .call(&status_request().unwrap(), |_| {})
        .await;
    assert!(
        matches!(got, Err(ClientError::Unsupported { op }) if op.name() == "job_status"),
        "{got:?}"
    );
    assert_eq!(served.await.unwrap().unwrap(), None);
}

/// ADR 0018, Compatibility rule 2: a node that does not know a request
/// field ignores it, so a client sends a root only to a node that lists
/// it, and an older node never measures with its own value in place of
/// the client's. A root the client leaves unset needs the same: it means
/// removed for the tool, which a node that does not know the root never
/// does, so it would run the tool with the daemon's value (review
/// findings on PR #13).
#[tokio::test]
async fn a_root_the_node_does_not_list_is_not_sent() {
    let doctor = Request::Doctor {
        peer: None,
        job: None,
        env: Some(ClientEnv {
            path: vec![PathDir::parse("/usr/bin").unwrap()],
            roots: Roots {
                cargo_home: Some(SystemPath::parse("/Users/me/.cargo").unwrap()),
                ..Roots::default()
            },
        }),
    };
    let (_dir, socket, served) =
        node(Script::Hello(welcome(&["doctor", "env.roots.rustup_home"]))).unwrap();
    let got = Client::new(&socket).call(&doctor, |_| {}).await;
    assert!(
        matches!(got, Err(ClientError::UnsupportedField { op, capability: "env.roots.cargo_home" }) if op.name() == "doctor"),
        "{got:?}"
    );
    assert_eq!(served.await.unwrap().unwrap(), None);

    // A node that lists the root it sets, but not the ones it leaves unset, does not get it either.
    let (_dir, socket, served) =
        node(Script::Hello(welcome(&["doctor", "env.roots.cargo_home"]))).unwrap();
    let got = Client::new(&socket).call(&doctor, |_| {}).await;
    assert!(
        matches!(got, Err(ClientError::UnsupportedField { op, capability: "env.roots.rustup_home" }) if op.name() == "doctor"),
        "{got:?}"
    );
    assert_eq!(served.await.unwrap().unwrap(), None);

    // A node that lists every root gets the request.
    let mut capabilities = vec!["doctor"];
    capabilities.extend(Roots::capabilities());
    let (_dir, socket, served) = node(Script::Serve {
        welcome: welcome(&capabilities),
        answers: vec![json!({"type": "done", "reply": {"kind": "doctor", "report": {"kind": "facts", "facts": []}}})],
    })
    .unwrap();
    Client::new(&socket).call(&doctor, |_| {}).await.unwrap();
    let sent = served.await.unwrap().unwrap().unwrap();
    assert_eq!(sent["env"]["roots"]["cargo_home"], "/Users/me/.cargo");
}

/// A node from before the client API reads the hello as a postcard
/// request it cannot decode and closes the connection.
#[tokio::test]
async fn a_node_that_closes_after_the_hello_reads_as_an_older_node() {
    let (_dir, socket, _served) = node(Script::Close).unwrap();
    let got = Client::new(&socket).hello().await;
    assert!(
        matches!(got, Err(ClientError::ClosedBeforeWelcome { .. })),
        "{got:?}"
    );
}

#[tokio::test]
async fn a_refusal_arrives_typed_with_the_nodes_sentence() {
    let (_dir, socket, _served) = node(Script::Serve {
        welcome: welcome(&["job_status"]),
        answers: vec![json!({"type": "refused", "refusal": {
            "kind": "not_owner",
            "reason": {"code": "unknown_job", "job": JOB},
            "message": "job is not known on this node"
        }})],
    })
    .unwrap();
    let got = Client::new(&socket)
        .call(&status_request().unwrap(), |_| {})
        .await;
    let Err(ClientError::Refused(refusal)) = got else {
        panic!("{got:?}");
    };
    assert_eq!(
        refusal.reason(),
        &fabric_client::api::Reason::UnknownJob {
            job: JOB.parse().unwrap()
        }
    );
    assert_eq!(refusal.message(), "job is not known on this node");
}

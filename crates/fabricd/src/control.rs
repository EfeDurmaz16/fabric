//! The control socket: the client API of ADR 0018, served to local clients.
//!
//! Invariant: the socket lives in the state directory, which only its owner
//! can enter (mode 0700), and a connection whose peer user id is not this
//! process's own is closed before anything is read, so every request comes
//! from the user who runs the node. A connection carries one exchange: a
//! hello, the welcome or an incompatible answer, one request, the events
//! of the command as the node makes them, and one final answer. A request
//! that does not parse is refused, never guessed at, and one over
//! [`frame::MAX_REQUEST`] is refused unparsed, so one request costs the
//! node a bounded amount of memory. A client that leaves
//! does not stop the command, and a client that stops reading does not
//! hold it: the command runs as its own task, so a move or a suspend never
//! waits on a socket while it holds a job's lock, and one that stopped
//! halfway would leave more to recover than one that finishes.

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use fabric_client::api::{
    API_VERSION, Answer, Event, Hello, HelloAnswer, Reason, Refusal, Request, Welcome,
};
use fabric_client::frame::{self, FrameError};
use tokio::net::UnixStream;
use tokio::sync::mpsc;

use crate::node::{Node, Out};

/// How long a client may take to send its hello, and then its request.
pub const CLIENT_DEADLINE: Duration = Duration::from_secs(30);

/// Serves one connection the node accepted on its control socket.
pub(crate) async fn serve(node: Arc<Node>, mut stream: UnixStream) {
    let me = rustix::process::geteuid().as_raw();
    match stream.peer_cred() {
        Ok(peer) if peer.uid() == me => {}
        Ok(peer) => {
            tracing::warn!(
                uid = peer.uid(),
                "refused a control connection from another user"
            );
            return;
        }
        Err(e) => {
            tracing::warn!(error = %e, "refused a control connection whose user is unknown");
            return;
        }
    }
    let Some(hello) = read(&mut stream, "hello").await else {
        return;
    };
    // A hello this large is no api's, and parsing it would cost the node
    // about thirty times its size.
    let hello = if hello.len() > frame::MAX_REQUEST {
        Err(too_large("hello", hello.len()))
    } else {
        frame::parse::<Hello>(&hello).map_err(|e| e.to_string())
    };
    let hello = match hello {
        Ok(hello) if hello.api == API_VERSION => hello,
        parsed => {
            if let Err(e) = &parsed {
                tracing::info!(error = %e, "a control client sent a hello that does not parse");
            }
            let answer = HelloAnswer::Incompatible {
                api: API_VERSION,
                node_version: node.version.as_str().to_owned(),
            };
            let _ = send(&mut stream, &answer).await;
            return;
        }
    };
    let welcome = HelloAnswer::Welcome(Welcome {
        api: API_VERSION,
        node: node.id(),
        node_version: node.version.as_str().to_owned(),
        capabilities: fabric_client::api::capabilities()
            .into_iter()
            .map(str::to_owned)
            .collect(),
    });
    if send(&mut stream, &welcome).await.is_err() {
        return;
    }
    let Some(request) = read(&mut stream, "request").await else {
        return;
    };
    let request = if request.len() > frame::MAX_REQUEST {
        Err(too_large("request", request.len()))
    } else {
        frame::parse::<Request>(&request).map_err(|e| {
            format!("the request does not parse, or names an op this node does not serve: {e}")
        })
    };
    let request = match request {
        Ok(request) => request,
        Err(message) => {
            let refusal = Refusal::new(Reason::BadRequest, message);
            let _ = send(&mut stream, &Answer::Refused { refusal }).await;
            return;
        }
    };
    tracing::debug!(client = %hello.client, op = %request.op(), "control request");
    run(node, request, stream).await;
}

/// Runs `request`, writing each event the moment the client takes it, then the final answer.
async fn run(node: Arc<Node>, request: Request, stream: UnixStream) {
    let (events, made) = mpsc::unbounded_channel();
    let command = async move { node.handle(request, Out::streaming(events)).await };
    relay(command, made, stream).await;
}

/// Runs `command` as its own task and writes the events it sends on
/// `made` to `stream`, in order, then its answer.
///
/// The command never waits on the client. A client that stops reading,
/// such as a CLI stopped with Ctrl-Z or writing to a paused terminal, only
/// lets the channel grow, while the command runs to its end and lets go
/// of the job's lock; the client reads the rest when it reads again. The
/// channel closes when the command ends, because its sender lives in the
/// command's [`Out`], so every event the command made precedes its answer.
async fn relay<C>(command: C, mut made: mpsc::UnboundedReceiver<Event>, mut stream: UnixStream)
where
    C: Future<Output = Answer> + Send + 'static,
{
    let command = tokio::spawn(command);
    while let Some(event) = made.recv().await {
        if !deliver(&mut stream, event).await {
            // The client left; the command runs on without it.
            return;
        }
    }
    let answer = command.await.unwrap_or_else(|e| Answer::Refused {
        refusal: Refusal::new(
            Reason::Internal,
            format!("the command stopped before it answered: {e}"),
        ),
    });
    let refusal = match (send(&mut stream, &answer).await, answer) {
        (Ok(()) | Err(FrameError::Io(_)), _) => return,
        // A refusal's kind sets the exit code and its message names the
        // command that resolves it, so it keeps both, and its reason: only
        // a path that is not UTF-8 reads as the message shows it.
        (Err(_), Answer::Refused { refusal }) => {
            Refusal::new(lossy(refusal.reason().clone()), refusal.message())
        }
        // Nothing of it was written, so the client still reads a final
        // frame, and learns the command ran rather than ask for it again.
        (Err(e), Answer::Done { .. } | Answer::Event { .. }) => {
            Refusal::new(Reason::Internal, unwritable(&e))
        }
    };
    let _ = send(&mut stream, &Answer::Refused { refusal }).await;
}

/// `reason` with each path written as `Path::display` shows it, U+FFFD in
/// place of what is not UTF-8, so it has a JSON form. Every reason is
/// named, so a new one fails to compile until it says whether it holds a
/// path.
fn lossy(reason: Reason) -> Reason {
    let shown = |path: PathBuf| PathBuf::from(path.to_string_lossy().into_owned());
    match reason {
        Reason::ChangedDuringMove { job, root, target } => Reason::ChangedDuringMove {
            job,
            root: shown(root),
            target,
        },
        Reason::StaleWorkspace { job, root, left_at } => Reason::StaleWorkspace {
            job,
            root: shown(root),
            left_at,
        },
        Reason::WorkspaceOccupied { root } => Reason::WorkspaceOccupied { root: shown(root) },
        Reason::SetupRunning { job, root, lock } => Reason::SetupRunning {
            job,
            root: shown(root),
            lock: shown(lock),
        },
        Reason::RootNotADirectory { job, root } => Reason::RootNotADirectory {
            job,
            root: shown(root),
        },
        Reason::SyncedRoot { root, folder } => Reason::SyncedRoot {
            root: shown(root),
            folder: shown(folder),
        },
        Reason::UnknownJob { .. }
        | Reason::NotOwner { .. }
        | Reason::Departed { .. }
        | Reason::StaleEpoch { .. }
        | Reason::EpochAhead { .. }
        | Reason::WrongJob { .. }
        | Reason::Departing { .. }
        | Reason::Forked { .. }
        | Reason::ForkBehind { .. }
        | Reason::Unreachable { .. }
        | Reason::UnknownPeer { .. }
        | Reason::Preflight { .. }
        | Reason::CarryRefused { .. }
        | Reason::StealNeedsForce { .. }
        | Reason::HarnessRunning { .. }
        | Reason::HarnessGone { .. }
        | Reason::HarnessNotRecorded { .. }
        | Reason::RanDuringMove { .. }
        | Reason::HarnessChangedDuringMove { .. }
        | Reason::HarnessChangedDuringPull { .. }
        | Reason::PlanOvertaken { .. }
        | Reason::SessionMoved { .. }
        | Reason::NoWorkspace { .. }
        | Reason::Unfinished { .. }
        | Reason::NoSnapshot { .. }
        | Reason::BadAddress { .. }
        | Reason::Peer { .. }
        | Reason::Fetch { .. }
        | Reason::PeerAhead { .. }
        | Reason::PeerBehind { .. }
        | Reason::PeerForked { .. }
        | Reason::MoveUnlinked { .. }
        | Reason::MoveStale { .. }
        | Reason::MoveToSelf { .. }
        | Reason::StealByOwner { .. }
        | Reason::StealWhileDeparting { .. }
        | Reason::StealByTarget { .. }
        | Reason::StealWithoutSnapshot { .. }
        | Reason::SecretInArguments { .. }
        | Reason::BadRequest
        | Reason::Internal
        | Reason::ManagedRefused { .. }
        | Reason::OperationConflict { .. }
        | Reason::ManagedUnavailable
        | Reason::StandaloneJob { .. }
        | Reason::ManagedJob { .. }
        | Reason::Unrecognized => reason,
    }
}

/// Why the node did not parse a hello or a request of `len` bytes.
fn too_large(what: &str, len: usize) -> String {
    format!(
        "the {what} is {len} bytes, over the {} bytes a node parses",
        frame::MAX_REQUEST
    )
}

/// Why an answer could not be framed, for the refusal that replaces it.
fn unwritable(error: &FrameError) -> String {
    match error {
        FrameError::TooLarge(len) => format!(
            "the answer is {len} bytes, over the {} the control socket carries; the command itself ran",
            frame::MAX_FRAME
        ),
        // Such as a path that is not UTF-8, under a home that is not.
        e => format!("the answer cannot be written as JSON ({e}); the command itself ran"),
    }
}

async fn deliver(stream: &mut UnixStream, event: Event) -> bool {
    match send(stream, &Answer::Event { event }).await {
        Err(FrameError::Io(_)) => false,
        // One event that cannot be framed, too large or holding a path
        // that is not UTF-8, is dropped; the client still gets the rest.
        Ok(()) | Err(_) => true,
    }
}

/// Reads one frame within `CLIENT_DEADLINE`; `None`, logged, when the client sent none.
async fn read(stream: &mut UnixStream, what: &str) -> Option<Vec<u8>> {
    match tokio::time::timeout(CLIENT_DEADLINE, frame::read_bytes(stream)).await {
        Ok(Ok(Some(bytes))) => Some(bytes),
        Ok(Ok(None)) => {
            tracing::debug!("a control client left before its {what}");
            None
        }
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "unreadable control {what}");
            None
        }
        Err(_) => {
            tracing::warn!("a control client sent no {what} within {CLIENT_DEADLINE:?}");
            None
        }
    }
}

/// Writes one frame, logging why it could not be.
async fn send<T: serde::Serialize>(stream: &mut UnixStream, value: &T) -> Result<(), FrameError> {
    let sent = frame::write(stream, value).await;
    match &sent {
        Ok(()) => {}
        Err(FrameError::Io(e)) => {
            tracing::info!(error = %e, "a control client left before the answer; the command ran to its end");
        }
        Err(e) => tracing::warn!(error = %e, "a control answer could not be written"),
    }
    sent
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    use std::path::PathBuf;

    use fabric_client::api::{JobCreated, Reply};
    use fabric_core::harness::HarnessKind;
    use fabric_core::id::{Epoch, JobId};
    use tokio::sync::oneshot;

    use super::*;

    /// Review finding on PR #13: under a home that is not UTF-8, which
    /// the node accepts, a job's root has no JSON form. Such an answer
    /// ended the connection with no final frame, so a client could not
    /// tell that `job_new` had made the job and might make it again, and
    /// such an event ended the relay before the rest. The event is now
    /// dropped and the answer becomes an `internal` refusal saying that
    /// the command ran.
    #[tokio::test]
    async fn an_answer_with_no_json_form_is_refused_and_such_an_event_dropped() {
        let job: JobId = "01JABCDEFGHJKMNPQRSTVWXYZ0".parse().unwrap();
        let root = PathBuf::from(OsString::from_vec(
            b"/home/\xff/fabric/jobs/01JABCDEFGHJKMNPQRSTVWXYZ0/work".to_vec(),
        ));
        let (node_side, mut client) = UnixStream::pair().unwrap();
        let (live, made) = mpsc::unbounded_channel();
        let command = async move {
            let mut out = Out::streaming(live);
            out.emit(Event::StaleWorkspaceKept {
                job,
                root: root.clone(),
                left_at: Epoch::FIRST,
            });
            out.emit(Event::LeftBehind {
                path: "after".to_owned(),
            });
            Answer::Done {
                reply: Reply::JobNew(JobCreated {
                    job,
                    root,
                    repo: PathBuf::from("/repo"),
                    harness: HarnessKind::ClaudeCode,
                    epoch: Epoch::FIRST,
                }),
            }
        };
        relay(command, made, node_side).await;

        let next: Answer = frame::read(&mut client).await.unwrap().unwrap();
        assert_eq!(
            next,
            Answer::Event {
                event: Event::LeftBehind {
                    path: "after".to_owned()
                }
            }
        );
        let last: Answer = frame::read(&mut client).await.unwrap().unwrap();
        let Answer::Refused { refusal } = last else {
            panic!("{last:?}");
        };
        assert_eq!(refusal.reason(), &Reason::Internal);
        assert!(
            refusal.message().ends_with("the command itself ran"),
            "{}",
            refusal.message()
        );
    }

    /// Review finding on PR #13: a refusal whose reason names a path
    /// that is not UTF-8, such as a stale workspace under such a home,
    /// reached the client as an `internal` refusal saying the command ran.
    /// The CLI exited 1 instead of 85 with its hint, and a client could
    /// take a `job_new` refused for its root as a job made. Such a refusal
    /// now keeps its kind, its reason and its message, and the path reads
    /// as the message shows it.
    #[tokio::test]
    async fn a_refusal_with_a_path_that_is_not_utf8_keeps_its_kind_and_reason() {
        let job: JobId = "01JABCDEFGHJKMNPQRSTVWXYZ0".parse().unwrap();
        let target = "fab1aibaeaqcaibaeaqcaibaeaqcaibaeaqcaibaeaqcaibaeaqcaiba"
            .parse()
            .unwrap();
        let bad = PathBuf::from(OsString::from_vec(b"/home/\xff/work".to_vec()));
        let shown = PathBuf::from("/home/\u{fffd}/work");
        let cases = [
            (
                Reason::StaleWorkspace {
                    job,
                    root: bad.clone(),
                    left_at: Epoch::FIRST,
                },
                Reason::StaleWorkspace {
                    job,
                    root: shown.clone(),
                    left_at: Epoch::FIRST,
                },
            ),
            (
                Reason::ChangedDuringMove {
                    job,
                    root: bad.clone(),
                    target,
                },
                Reason::ChangedDuringMove {
                    job,
                    root: shown.clone(),
                    target,
                },
            ),
            (
                Reason::WorkspaceOccupied { root: bad.clone() },
                Reason::WorkspaceOccupied {
                    root: shown.clone(),
                },
            ),
            (
                Reason::SyncedRoot {
                    root: bad.clone(),
                    folder: bad.clone(),
                },
                Reason::SyncedRoot {
                    root: shown.clone(),
                    folder: shown.clone(),
                },
            ),
        ];
        for (made, read) in cases {
            let message = format!("refused at {}; run fabric job resume", bad.display());
            let (node_side, mut client) = UnixStream::pair().unwrap();
            let (live, made_events) = mpsc::unbounded_channel();
            let refusal = Refusal::new(made, message.clone());
            let command = async move {
                drop(Out::streaming(live));
                Answer::Refused { refusal }
            };
            relay(command, made_events, node_side).await;

            let last: Answer = frame::read(&mut client).await.unwrap().unwrap();
            assert_eq!(
                last,
                Answer::Refused {
                    refusal: Refusal::new(read, message)
                }
            );
        }
    }

    /// Review finding on PR #13: a client that stayed connected but
    /// stopped reading stalled a suspend between two events, with the
    /// job's lock held, until it read again. The command now ends on its
    /// own, and the client still gets every event in order, then the
    /// answer, once it reads.
    #[tokio::test]
    async fn a_client_that_stops_reading_does_not_stall_the_command() {
        // About 2 MB of events, far more than a socket buffer holds on macOS or Linux.
        const EVENTS: usize = 2_000;
        let (node_side, mut client) = UnixStream::pair().unwrap();
        let (live, made) = mpsc::unbounded_channel();
        let (ended, command_ended) = oneshot::channel();
        let command = async move {
            let mut out = Out::streaming(live);
            for i in 0..EVENTS {
                out.emit(Event::LeftBehind {
                    path: format!("{i:05}/{}", "x".repeat(1_000)),
                });
                // One step at a time, as a suspend makes them.
                tokio::task::yield_now().await;
            }
            let _ = ended.send(());
            Answer::Done {
                reply: Reply::HarnessExited,
            }
        };
        let relayed = tokio::spawn(relay(command, made, node_side));

        tokio::time::timeout(Duration::from_secs(10), command_ended)
            .await
            .expect("the command waited for a client that stopped reading")
            .unwrap();
        for i in 0..EVENTS {
            let answer: Answer = frame::read(&mut client).await.unwrap().unwrap();
            let prefix = format!("{i:05}/");
            assert!(
                matches!(&answer, Answer::Event { event: Event::LeftBehind { path } } if path.starts_with(&prefix)),
                "event {i}: {answer:?}"
            );
        }
        let last: Answer = frame::read(&mut client).await.unwrap().unwrap();
        assert_eq!(
            last,
            Answer::Done {
                reply: Reply::HarnessExited
            }
        );
        relayed.await.unwrap();
    }

    /// One frame of exactly `len` bytes of JSON: a `node_id` request, or a
    /// hello, padded with a field the node does not know.
    fn padded(head: &str, len: usize) -> Vec<u8> {
        let json = format!("{head},\"x\":\"{}\"}}", "a".repeat(len - head.len() - 8));
        assert_eq!(json.len(), len);
        let mut frame = u32::try_from(len).unwrap().to_le_bytes().to_vec();
        frame.extend_from_slice(json.as_bytes());
        frame
    }

    /// Opens a connection to `node`, sends `hello`, and returns the
    /// client's end with the node's answer.
    async fn hello(node: &Arc<Node>, hello: &[u8]) -> (UnixStream, HelloAnswer) {
        use tokio::io::AsyncWriteExt;

        let (node_side, mut client) = UnixStream::pair().unwrap();
        tokio::spawn(serve(Arc::clone(node), node_side));
        client.write_all(hello).await.unwrap();
        let answer = frame::read(&mut client).await.unwrap().unwrap();
        (client, answer)
    }

    /// Review finding on PR #13: parsing a frame takes up to about thirty
    /// times its size, so a 16 MiB request holding zeros in a field the
    /// node does not know took over 500 MiB of the node's memory. A hello
    /// or a request over `MAX_REQUEST` is refused unparsed, as `bad_request`
    /// or as an incompatible hello; one of exactly `MAX_REQUEST` bytes
    /// still reads.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_hello_or_a_request_over_the_request_cap_is_refused_unparsed() {
        use tokio::io::AsyncWriteExt;

        let (_dir, node) = crate::ops::tests::node();
        let api = format!("{{\"api\":{API_VERSION},\"client\":\"test\"");
        for (len, refused) in [(frame::MAX_REQUEST, false), (frame::MAX_REQUEST + 1, true)] {
            let (mut client, welcome) = hello(&node, &padded(&api, 64)).await;
            assert!(matches!(welcome, HelloAnswer::Welcome(_)), "{welcome:?}");
            client
                .write_all(&padded("{\"op\":\"node_id\"", len))
                .await
                .unwrap();
            let answer: Answer = frame::read(&mut client).await.unwrap().unwrap();
            if refused {
                let Answer::Refused { refusal } = answer else {
                    panic!("{answer:?}");
                };
                assert_eq!(refusal.reason(), &Reason::BadRequest);
                assert_eq!(
                    refusal.message(),
                    format!("the request is {len} bytes, over the 1048576 bytes a node parses")
                );
            } else {
                assert!(
                    matches!(
                        answer,
                        Answer::Done {
                            reply: Reply::NodeId(_)
                        }
                    ),
                    "{answer:?}"
                );
            }

            let (_client, answer) = hello(&node, &padded(&api, len)).await;
            assert_eq!(
                matches!(answer, HelloAnswer::Incompatible { .. }),
                refused,
                "{answer:?}"
            );
        }
    }
}

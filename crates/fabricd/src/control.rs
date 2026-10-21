//! The control socket: the client API of ADR 0018, served to local clients.
//!
//! Invariant: the socket lives in the state directory, which only its owner
//! can enter (mode 0700), and a connection whose peer user id is not this
//! process's own is closed before anything is read, so every request comes
//! from the user who runs the node. A connection carries one exchange: a
//! hello, the welcome or an incompatible answer, one request, the events
//! of the command as the node makes them, and one final answer. A request
//! that does not parse is refused, never guessed at. A client that leaves
//! does not stop the command: a move or a suspend that stopped halfway
//! would leave more to recover than one that finishes.

use std::sync::Arc;
use std::time::Duration;

use fabric_client::api::{
    API_VERSION, Answer, Event, Hello, HelloAnswer, Op, Reason, Refusal, Request, Welcome,
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
    let hello = match serde_json::from_slice::<Hello>(&hello) {
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
        capabilities: Op::ALL.iter().map(|op| op.name().to_owned()).collect(),
    });
    if send(&mut stream, &welcome).await.is_err() {
        return;
    }
    let Some(request) = read(&mut stream, "request").await else {
        return;
    };
    let request = match serde_json::from_slice::<Request>(&request) {
        Ok(request) => request,
        Err(e) => {
            let refusal = Refusal::new(
                Reason::BadRequest,
                format!("the request does not parse, or names an op this node does not serve: {e}"),
            );
            let _ = send(&mut stream, &Answer::Refused { refusal }).await;
            return;
        }
    };
    tracing::debug!(client = %hello.client, op = %request.op(), "control request");
    run(node, request, stream).await;
}

/// Runs `request`, writing each event the moment the command makes it, then the final answer.
async fn run(node: Arc<Node>, request: Request, mut stream: UnixStream) {
    let (events, mut made) = mpsc::unbounded_channel();
    let work = async move { node.handle(request, Out::streaming(events)).await };
    tokio::pin!(work);
    let mut connected = true;
    let answer = loop {
        tokio::select! {
            biased;
            Some(event) = made.recv() => {
                connected = connected && deliver(&mut stream, event).await;
            }
            answer = &mut work => break answer,
        }
    };
    // The command made these before it ended, so they go before its answer.
    while let Ok(event) = made.try_recv() {
        connected = connected && deliver(&mut stream, event).await;
    }
    if !connected {
        return;
    }
    if let Err(FrameError::TooLarge(len)) = send(&mut stream, &answer).await {
        let refusal = Refusal::new(
            Reason::Internal,
            format!(
                "the answer is {len} bytes, over the {} the control socket carries; the command itself ran",
                frame::MAX_FRAME
            ),
        );
        let _ = send(&mut stream, &Answer::Refused { refusal }).await;
    }
}

async fn deliver(stream: &mut UnixStream, event: Event) -> bool {
    match send(stream, &Answer::Event { event }).await {
        // One event too large to frame is dropped; the client still gets the rest.
        Ok(()) | Err(FrameError::TooLarge(_)) => true,
        Err(_) => false,
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

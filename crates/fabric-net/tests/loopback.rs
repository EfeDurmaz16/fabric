//! Two nodes in one process over loopback, on real QUIC and real TLS over
//! TCP: the hello exchange, the trust rule at both ends, the frame cap on a
//! live stream, the call deadline, chunk resume after a dropped connection,
//! stream isolation under backpressure, and the QUIC first dial.
#![cfg(all(feature = "quic", feature = "tcp_tls"))]

use std::collections::BTreeMap;
use std::error::Error;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use fabric_core::BoxFuture;
use fabric_core::id::{ChunkId, NodeId};
use fabric_net::endpoint::Endpoint;
use fabric_net::frame::{FRAME_VERSION, MAX_CONTROL_FRAME};
use fabric_net::identity::NodeIdentity;
use fabric_net::quic::QuicTransport;
use fabric_net::rpc::{RpcError, Service, Session};
use fabric_net::tcp::TcpTlsTransport;
use fabric_net::tls::Trust;
use fabric_net::transport::{Connection, Transport, TransportError, TransportKind};
use fabric_net::wire::{
    ChunkBatch, Fault, FaultText, Hello, PROTOCOL_VERSION, RequestBody, ResponseBody, VersionText,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

type TestResult = Result<(), Box<dyn Error + Send + Sync>>;

const DEADLINE: Duration = Duration::from_secs(5);
const BOTH: [TransportKind; 2] = [TransportKind::Quic, TransportKind::TcpTls];

#[derive(Debug)]
struct Paired(Vec<NodeId>);

impl Trust for Paired {
    fn is_paired(&self, node: NodeId) -> bool {
        self.0.contains(&node)
    }
}

struct Node {
    identity: NodeIdentity,
    hello: Hello,
}

impl Node {
    fn new() -> Result<Self, Box<dyn Error + Send + Sync>> {
        let identity = NodeIdentity::generate()?;
        let hello = Hello::now(identity.node_id(), VersionText::new("test")?);
        Ok(Self { identity, hello })
    }

    fn id(&self) -> NodeId {
        self.identity.node_id()
    }

    fn bind(
        &self,
        kind: TransportKind,
        paired: &[NodeId],
    ) -> Result<Arc<dyn Transport>, TransportError> {
        let trust: Arc<dyn Trust> = Arc::new(Paired(paired.to_vec()));
        let addr: SocketAddr = ([127, 0, 0, 1], 0).into();
        Ok(match kind {
            TransportKind::Quic => Arc::new(QuicTransport::bind(&self.identity, trust, addr)?),
            TransportKind::TcpTls => Arc::new(TcpTlsTransport::bind(&self.identity, trust, addr)?),
        })
    }
}

struct FakeService {
    chunks: BTreeMap<ChunkId, Vec<u8>>,
    ping_delay: Duration,
    drop_on_call: Option<(usize, mpsc::Sender<()>)>,
    calls: AtomicUsize,
}

impl FakeService {
    fn new() -> Self {
        Self {
            chunks: BTreeMap::new(),
            ping_delay: Duration::ZERO,
            drop_on_call: None,
            calls: AtomicUsize::new(0),
        }
    }
}

impl Service for FakeService {
    fn call(&self, _peer: NodeId, request: RequestBody) -> BoxFuture<'_, ResponseBody> {
        Box::pin(async move {
            match request {
                RequestBody::Ping => {
                    tokio::time::sleep(self.ping_delay).await;
                    ResponseBody::Pong
                }
                _ => ResponseBody::Fault(Fault::Internal(FaultText::truncated("not in this test"))),
            }
        })
    }

    fn chunk(&self, _peer: NodeId, id: ChunkId) -> BoxFuture<'_, Option<Vec<u8>>> {
        Box::pin(async move {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some((at, stop)) = &self.drop_on_call
                && call == *at
            {
                tokio::time::sleep(Duration::from_millis(300)).await;
                let _ = stop.send(()).await;
                std::future::pending::<()>().await;
            }
            self.chunks.get(&id).cloned()
        })
    }
}

/// Accepts one connection, runs the acceptor's hello and serves until the connection ends or `stop` fires.
fn serve_one(
    transport: Arc<dyn Transport>,
    hello: Hello,
    service: Arc<FakeService>,
    stop: Option<mpsc::Receiver<()>>,
) -> JoinHandle<Result<(), RpcError>> {
    tokio::spawn(async move {
        let connection = transport.accept().await?;
        let session = Session::accept(connection, hello, DEADLINE).await?;
        match stop {
            None => session.serve(service).await,
            Some(mut stop) => tokio::select! {
                served = session.serve(service) => served,
                _ = stop.recv() => {
                    session.close();
                    Ok(())
                }
            },
        }
    })
}

async fn connect(
    transport: &dyn Transport,
    peer: NodeId,
    addr: SocketAddr,
    hello: Hello,
) -> Result<Session, RpcError> {
    let connection = transport.connect(peer, addr).await?;
    Session::connect(connection, hello, DEADLINE).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn paired_nodes_exchange_hello_and_ping() -> TestResult {
    for kind in BOTH {
        let (a, b) = (Node::new()?, Node::new()?);
        let server = b.bind(kind, &[a.id()])?;
        let client = a.bind(kind, &[b.id()])?;
        let serving = serve_one(
            server.clone(),
            b.hello.clone(),
            Arc::new(FakeService::new()),
            None,
        );

        let session = connect(
            client.as_ref(),
            b.id(),
            server.local_addr()?,
            a.hello.clone(),
        )
        .await?;
        assert_eq!(session.peer(), b.id());
        assert_eq!(session.kind(), kind);
        assert_eq!(session.remote().protocol_version, PROTOCOL_VERSION);
        session.ping(DEADLINE).await?;
        session.ping(DEADLINE).await?;

        session.close();
        assert!(serving.await?.is_err(), "serve ends with the connection");
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dial_to_the_wrong_node_id_fails_inside_the_handshake() -> TestResult {
    for kind in BOTH {
        let (a, b, c) = (Node::new()?, Node::new()?, Node::new()?);
        let server = b.bind(kind, &[a.id()])?;
        let client = a.bind(kind, &[b.id(), c.id()])?;
        let accepting = {
            let server = server.clone();
            tokio::spawn(async move {
                tokio::time::timeout(Duration::from_millis(800), server.accept()).await
            })
        };

        let err = client
            .connect(c.id(), server.local_addr()?)
            .await
            .err()
            .ok_or("a dial that expects the wrong node must fail")?;
        assert!(
            matches!(
                err,
                TransportError::Handshake { .. } | TransportError::Tls(_)
            ),
            "{kind}: {err}"
        );
        assert!(
            accepting.await?.is_err(),
            "{kind}: the server must not surface a connection to the wrong dialer"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unpaired_dialer_is_refused_at_accept() -> TestResult {
    for kind in BOTH {
        let (a, b) = (Node::new()?, Node::new()?);
        let server = b.bind(kind, &[])?;
        let client = a.bind(kind, &[b.id()])?;
        let accepting = {
            let server = server.clone();
            tokio::spawn(async move {
                tokio::time::timeout(Duration::from_millis(800), server.accept()).await
            })
        };

        let dialed = client.connect(b.id(), server.local_addr()?).await;
        let no_session = match dialed {
            Err(_) => true,
            // TLS lets a client send before the server's verdict; the first
            // byte in either direction then fails.
            Ok(connection) => Session::connect(connection, a.hello.clone(), Duration::from_secs(2))
                .await
                .is_err(),
        };
        assert!(no_session, "{kind}: an unpaired dialer must get no session");
        assert!(
            accepting.await?.is_err(),
            "{kind}: an unpaired dialer must never surface at accept"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_oversized_frame_gets_no_answer_and_the_connection_survives() -> TestResult {
    for kind in BOTH {
        let (a, b) = (Node::new()?, Node::new()?);
        let server = b.bind(kind, &[a.id()])?;
        let client = a.bind(kind, &[b.id()])?;
        let _serving = serve_one(
            server.clone(),
            b.hello.clone(),
            Arc::new(FakeService::new()),
            None,
        );
        let session = connect(
            client.as_ref(),
            b.id(),
            server.local_addr()?,
            a.hello.clone(),
        )
        .await?;

        let mut stream = session.connection().open().await?;
        let declared = u32::try_from(MAX_CONTROL_FRAME + 1)?;
        stream.write_all(&declared.to_le_bytes()).await?;
        stream.write_all(&[FRAME_VERSION; 64]).await?;
        let _ = stream.shutdown().await;
        let mut answer = Vec::new();
        let read = tokio::time::timeout(DEADLINE, stream.read_to_end(&mut answer)).await?;
        assert!(
            read.is_err() || answer.is_empty(),
            "{kind}: no response may follow an oversized frame"
        );

        session.ping(DEADLINE).await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_slow_peer_trips_the_call_deadline() -> TestResult {
    for kind in BOTH {
        let (a, b) = (Node::new()?, Node::new()?);
        let server = b.bind(kind, &[a.id()])?;
        let client = a.bind(kind, &[b.id()])?;
        let mut service = FakeService::new();
        service.ping_delay = Duration::from_secs(3);
        let _serving = serve_one(server.clone(), b.hello.clone(), Arc::new(service), None);
        let session = connect(
            client.as_ref(),
            b.id(),
            server.local_addr()?,
            a.hello.clone(),
        )
        .await?;

        let deadline = Duration::from_millis(300);
        let started = std::time::Instant::now();
        let err = session
            .ping(deadline)
            .await
            .err()
            .ok_or("the call must fail")?;
        assert!(
            matches!(err, RpcError::Deadline(d) if d == deadline),
            "{kind}: {err}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{kind}: the deadline must fire on the caller"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hello_naming_another_node_is_refused_by_the_acceptor() -> TestResult {
    for kind in BOTH {
        let (a, b, c) = (Node::new()?, Node::new()?, Node::new()?);
        let server = b.bind(kind, &[a.id()])?;
        let client = a.bind(kind, &[b.id()])?;
        let serving = serve_one(
            server.clone(),
            b.hello.clone(),
            Arc::new(FakeService::new()),
            None,
        );

        let mut lying = a.hello.clone();
        lying.node = c.id();
        let connection = client.connect(b.id(), server.local_addr()?).await?;
        let _ = Session::connect(connection, lying, DEADLINE).await;
        let err = serving.await?.err().ok_or("the acceptor must refuse")?;
        assert!(
            matches!(err, RpcError::HelloMismatch { proved, claimed } if proved == a.id() && claimed == c.id()),
            "{kind}: {err}"
        );
    }
    Ok(())
}

fn chunk_set(count: usize) -> BTreeMap<ChunkId, Vec<u8>> {
    (0..count)
        .map(|i| {
            let byte = u8::try_from(i).unwrap_or(u8::MAX);
            let bytes: Vec<u8> = (0..48 * 1024u32)
                .map(|j| byte ^ u8::try_from(j % 251).unwrap_or(0))
                .collect();
            (ChunkId::of(&bytes), bytes)
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_chunk_transfer_resumes_after_the_connection_drops() -> TestResult {
    for kind in BOTH {
        let (a, b) = (Node::new()?, Node::new()?);
        let server = b.bind(kind, &[a.id()])?;
        let client = a.bind(kind, &[b.id()])?;
        let chunks = chunk_set(24);
        let ids: Vec<ChunkId> = chunks.keys().copied().collect();

        let (stop_tx, stop_rx) = mpsc::channel(1);
        let mut first = FakeService::new();
        first.chunks = chunks.clone();
        first.drop_on_call = Some((7, stop_tx));
        let first_serving = serve_one(
            server.clone(),
            b.hello.clone(),
            Arc::new(first),
            Some(stop_rx),
        );

        let session = connect(
            client.as_ref(),
            b.id(),
            server.local_addr()?,
            a.hello.clone(),
        )
        .await?;
        let mut stream = session
            .want_chunks(ChunkBatch::new(ids.clone())?, Duration::from_secs(5))
            .await?;
        let mut got = BTreeMap::new();
        let interrupted = loop {
            match stream.next().await {
                Ok(Some((id, bytes))) => {
                    got.insert(id, bytes);
                }
                Ok(None) => break None,
                Err(err) => break Some(err),
            }
        };
        assert!(interrupted.is_some(), "{kind}: the first batch must be cut");
        assert_eq!(
            got.len(),
            7,
            "{kind}: the chunks sent before the drop arrive"
        );
        let missing: Vec<ChunkId> = stream.missing().collect();
        assert_eq!(missing.len(), 24 - got.len());
        first_serving.await??;

        let mut second = FakeService::new();
        second.chunks = chunks.clone();
        let _second_serving = serve_one(server.clone(), b.hello.clone(), Arc::new(second), None);
        let session = connect(
            client.as_ref(),
            b.id(),
            server.local_addr()?,
            a.hello.clone(),
        )
        .await?;
        let mut stream = session
            .want_chunks(ChunkBatch::new(missing)?, Duration::from_secs(5))
            .await?;
        let mut resumed = 0;
        while let Some((id, bytes)) = stream.next().await? {
            assert!(
                got.insert(id, bytes).is_none(),
                "{kind}: a chunk arrived twice"
            );
            resumed += 1;
        }
        assert_eq!(resumed, 17, "{kind}: only the missing chunks travel again");
        assert_eq!(got, chunks, "{kind}: every chunk is whole and verified");
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_streams_do_not_cross_under_backpressure() -> TestResult {
    const STREAMS: u8 = 3;
    const LEN: usize = 3 << 20;
    for kind in BOTH {
        let (a, b) = (Node::new()?, Node::new()?);
        let server = b.bind(kind, &[a.id()])?;
        let client = a.bind(kind, &[b.id()])?;
        let echo = {
            let server = server.clone();
            tokio::spawn(async move {
                let connection: Arc<dyn Connection> = Arc::from(server.accept().await?);
                let mut tasks = Vec::new();
                for _ in 0..STREAMS {
                    let (mut send, mut recv) = connection.next().await?.split();
                    tasks.push(tokio::spawn(async move {
                        let copied = tokio::io::copy(&mut recv, &mut send).await?;
                        send.shutdown().await?;
                        Ok::<u64, std::io::Error>(copied)
                    }));
                }
                for task in tasks {
                    let copied = task
                        .await
                        .map_err(|e| TransportError::Closed(e.to_string()))??;
                    assert_eq!(usize::try_from(copied).ok(), Some(LEN));
                }
                // Dropping the connection now would discard echoed bytes the
                // client has not read yet; the client closes when it is done.
                assert!(connection.next().await.is_err());
                Ok::<(), TransportError>(())
            })
        };

        let connection = client.connect(b.id(), server.local_addr()?).await?;
        let mut streams = Vec::new();
        for i in 0..STREAMS {
            let (mut send, mut recv) = connection.open().await?.split();
            let payload: Vec<u8> = (0..LEN)
                .map(|j| i ^ u8::try_from(j % 199).unwrap_or(0))
                .collect();
            let expected = payload.clone();
            let writer = tokio::spawn(async move {
                send.write_all(&payload).await?;
                send.shutdown().await
            });
            streams.push(tokio::spawn(async move {
                let mut back = Vec::with_capacity(LEN);
                recv.read_to_end(&mut back).await?;
                writer.await??;
                Ok::<bool, std::io::Error>(back == expected)
            }));
        }
        for stream in streams {
            assert!(
                stream.await??,
                "{kind}: a stream echoed another stream's bytes"
            );
        }
        connection.close();
        echo.await??;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn quic_is_dialed_first_and_tcp_is_the_fallback() -> TestResult {
    let (a, b) = (Node::new()?, Node::new()?);
    let trust_a: Arc<dyn Trust> = Arc::new(Paired(vec![b.id()]));
    let trust_b: Arc<dyn Trust> = Arc::new(Paired(vec![a.id()]));
    let any: SocketAddr = ([127, 0, 0, 1], 0).into();
    let server = Arc::new(Endpoint::bind(&b.identity, trust_b.clone(), any)?);
    let client =
        Endpoint::bind(&a.identity, trust_a, any)?.with_quic_timeout(Duration::from_millis(500));

    let accepting = {
        let server = server.clone();
        tokio::spawn(async move { server.accept().await })
    };
    let dialed = client.dial(b.id(), server.local_addr()?).await?;
    assert_eq!(dialed.connection.kind(), TransportKind::Quic);
    assert!(dialed.fallback_reason.is_none());
    assert_eq!(accepting.await??.kind(), TransportKind::Quic);

    // A port where UDP is a black hole and only TCP answers.
    let udp_sink = std::net::UdpSocket::bind(any)?;
    let tcp_only_addr = SocketAddr::new(any.ip(), udp_sink.local_addr()?.port());
    let tcp_only = Arc::new(TcpTlsTransport::bind(&b.identity, trust_b, tcp_only_addr)?);
    let accepting = {
        let tcp_only = tcp_only.clone();
        tokio::spawn(async move { tcp_only.accept().await })
    };
    let dialed = client.dial_any(b.id(), &[tcp_only_addr.into()]).await?;
    assert_eq!(dialed.connection.kind(), TransportKind::TcpTls);
    let reason = dialed.fallback_reason.ok_or("the fallback must say why")?;
    assert!(reason.starts_with("quic:"), "{reason}");
    assert_eq!(accepting.await??.kind(), TransportKind::TcpTls);
    Ok(())
}

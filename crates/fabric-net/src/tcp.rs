//! The TCP fallback: TLS over TCP with a small stream multiplexer.
//!
//! Invariant: the same pinned verifiers as QUIC decide the handshake, so a
//! peer that QUIC would refuse is refused here too. Inside the TLS stream
//! every logical stream is framed as `u32 stream id | u8 kind | u32 length |
//! payload`; the dialer opens odd ids and the acceptor even ids, a stream
//! exists for the peer once its first frame arrives, and a `Fin` frame ends
//! one direction. A payload above `MAX_MUX_PAYLOAD` ends the connection, so
//! one frame is the most memory a peer can make this node hold per read.
//!
//! The multiplexer is one TCP stream, so a stream nobody reads holds up the
//! others once its buffer is full. The protocol reads every stream it
//! opens, and the ledger records which transport served a resume so slow
//! paths are visible.

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use fabric_core::BoxFuture;
use fabric_core::id::NodeId;
use rustls::pki_types::ServerName;
use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::task::{AbortHandle, JoinHandle};
use tokio_rustls::{TlsAcceptor, TlsConnector, TlsStream};

use crate::identity::NodeIdentity;
use crate::pipe::{PipeReader, PipeWriter, pipe};
use crate::tls::{Credentials, Refusal, TlsError, Trust, peer_of};
use crate::transport::{Connection, Stream, Transport, TransportError, TransportKind};

/// The most payload bytes in one multiplexer frame.
pub const MAX_MUX_PAYLOAD: usize = 64 << 10;

/// How many bytes one logical stream buffers in each direction before the writer waits.
pub const STREAM_BUFFER: usize = 1 << 20;

/// How long a TLS handshake may take before the connection is dropped.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

const HEADER_LEN: usize = 9;
const KIND_DATA: u8 = 0;
const KIND_FIN: u8 = 1;
const OUT_QUEUE: usize = 64;

/// A TCP listener that dials and accepts under one node identity.
pub struct TcpTlsTransport {
    listener: TcpListener,
    acceptor: TlsAcceptor,
    credentials: Arc<Credentials>,
}

impl std::fmt::Debug for TcpTlsTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TcpTlsTransport")
            .field("local_addr", &self.listener.local_addr().ok())
            .finish_non_exhaustive()
    }
}

impl TcpTlsTransport {
    /// Binds a TCP listener at `addr` that accepts paired peers and dials as `identity`.
    ///
    /// Call it inside a tokio runtime.
    pub fn bind(
        identity: &NodeIdentity,
        trust: Arc<dyn Trust>,
        addr: SocketAddr,
    ) -> Result<Self, TransportError> {
        let credentials = Arc::new(Credentials::new(identity)?);
        let acceptor = TlsAcceptor::from(Arc::new(credentials.server_config(trust)?));
        let std_listener = std::net::TcpListener::bind(addr)?;
        std_listener.set_nonblocking(true)?;
        let listener = TcpListener::from_std(std_listener)?;
        Ok(Self {
            listener,
            acceptor,
            credentials,
        })
    }

    async fn dial(
        &self,
        peer: NodeId,
        addr: SocketAddr,
    ) -> Result<Box<dyn Connection>, TransportError> {
        let handshake = |err: &dyn std::fmt::Display| TransportError::Handshake {
            addr,
            reason: err.to_string(),
        };
        let tcp = TcpStream::connect(addr).await?;
        tcp.set_nodelay(true)?;
        let refusal = Arc::new(Refusal::default());
        let connector =
            TlsConnector::from(Arc::new(self.credentials.client_config(peer, &refusal)?));
        let name = ServerName::IpAddress(addr.ip().into());
        let tls = tokio::time::timeout(HANDSHAKE_TIMEOUT, connector.connect(name, tcp))
            .await
            .map_err(|_| TransportError::Timeout(HANDSHAKE_TIMEOUT))?
            .map_err(|err| refusal.take().map_or_else(|| handshake(&err), Into::into))?;
        let actual = peer_of(tls.get_ref().1.peer_certificates())?;
        if actual != peer {
            return Err(TlsError::PeerMismatch {
                expected: peer,
                actual,
            }
            .into());
        }
        Ok(Box::new(Mux::spawn(
            TlsStream::Client(tls),
            Side::Dialer,
            peer,
            addr,
        )))
    }

    async fn next_connection(&self) -> Result<Box<dyn Connection>, TransportError> {
        loop {
            let (tcp, remote) = self.listener.accept().await?;
            if let Err(err) = tcp.set_nodelay(true) {
                tracing::debug!(%remote, error = %err, "tcp nodelay refused");
            }
            let accepted = tokio::time::timeout(HANDSHAKE_TIMEOUT, self.acceptor.accept(tcp)).await;
            match accepted {
                Ok(Ok(tls)) => match peer_of(tls.get_ref().1.peer_certificates()) {
                    Ok(peer) => {
                        return Ok(Box::new(Mux::spawn(
                            TlsStream::Server(tls),
                            Side::Acceptor,
                            peer,
                            remote,
                        )));
                    }
                    Err(err) => {
                        tracing::warn!(%remote, error = %err, "tcp peer without a node certificate");
                    }
                },
                Ok(Err(err)) => {
                    tracing::debug!(%remote, error = %err, "tcp handshake refused");
                }
                Err(_) => {
                    tracing::debug!(%remote, "tcp handshake timed out");
                }
            }
        }
    }
}

impl Transport for TcpTlsTransport {
    fn kind(&self) -> TransportKind {
        TransportKind::TcpTls
    }

    fn local_addr(&self) -> Result<SocketAddr, TransportError> {
        Ok(self.listener.local_addr()?)
    }

    fn connect(
        &self,
        peer: NodeId,
        addr: SocketAddr,
    ) -> BoxFuture<'_, Result<Box<dyn Connection>, TransportError>> {
        Box::pin(self.dial(peer, addr))
    }

    fn accept(&self) -> BoxFuture<'_, Result<Box<dyn Connection>, TransportError>> {
        Box::pin(self.next_connection())
    }
}

#[derive(Clone, Copy)]
enum Side {
    Dialer,
    Acceptor,
}

impl Side {
    fn first_id(self) -> u32 {
        match self {
            Side::Dialer => 1,
            Side::Acceptor => 2,
        }
    }

    fn opens(self, id: u32) -> bool {
        id % 2 == self.first_id() % 2
    }
}

struct Outgoing {
    id: u32,
    kind: u8,
    payload: Vec<u8>,
}

struct Mux {
    peer: NodeId,
    remote: SocketAddr,
    next_id: AtomicU32,
    out: mpsc::Sender<Outgoing>,
    incoming: tokio::sync::Mutex<mpsc::Receiver<Stream>>,
    readers: Arc<Mutex<HashMap<u32, PipeWriter>>>,
    tasks: Vec<JoinHandle<()>>,
}

impl Mux {
    fn spawn(tls: TlsStream<TcpStream>, side: Side, peer: NodeId, remote: SocketAddr) -> Self {
        let (read_half, write_half) = tokio::io::split(tls);
        let (out, out_rx) = mpsc::channel::<Outgoing>(OUT_QUEUE);
        let (incoming_tx, incoming_rx) = mpsc::channel::<Stream>(OUT_QUEUE);
        let readers: Arc<Mutex<HashMap<u32, PipeWriter>>> = Arc::default();

        let writer = tokio::spawn(write_loop(write_half, out_rx));
        let writer_abort = writer.abort_handle();
        let reader = tokio::spawn(read_loop(
            read_half,
            side,
            readers.clone(),
            out.clone(),
            incoming_tx,
            writer_abort,
        ));
        Self {
            peer,
            remote,
            next_id: AtomicU32::new(side.first_id()),
            out,
            incoming: tokio::sync::Mutex::new(incoming_rx),
            readers,
            tasks: vec![writer, reader],
        }
    }

    fn open_local(&self) -> Result<Stream, TransportError> {
        if self.out.is_closed() {
            return Err(TransportError::Closed("tcp connection ended".to_owned()));
        }
        let id = self.next_id.fetch_add(2, Ordering::Relaxed);
        Ok(attach(id, &self.readers, &self.out))
    }
}

/// Makes the two pipes of one logical stream, registers the inbound writer and starts the outbound pump.
fn attach(
    id: u32,
    readers: &Mutex<HashMap<u32, PipeWriter>>,
    out: &mpsc::Sender<Outgoing>,
) -> Stream {
    let (inbound_writer, inbound_reader) = pipe(STREAM_BUFFER);
    let (outbound_writer, outbound_reader) = pipe(STREAM_BUFFER);
    lock(readers).insert(id, inbound_writer);
    tokio::spawn(pump(id, outbound_reader, out.clone()));
    Stream::new(outbound_writer, inbound_reader)
}

async fn pump(id: u32, mut source: PipeReader, out: mpsc::Sender<Outgoing>) {
    let mut buf = vec![0u8; MAX_MUX_PAYLOAD];
    loop {
        let n = source.read(&mut buf).await.unwrap_or_default();
        let frame = if n == 0 {
            Outgoing {
                id,
                kind: KIND_FIN,
                payload: Vec::new(),
            }
        } else {
            Outgoing {
                id,
                kind: KIND_DATA,
                payload: buf[..n].to_vec(),
            }
        };
        if out.send(frame).await.is_err() || n == 0 {
            return;
        }
    }
}

async fn write_loop(mut wire: WriteHalf<TlsStream<TcpStream>>, mut out: mpsc::Receiver<Outgoing>) {
    while let Some(frame) = out.recv().await {
        let Ok(len) = u32::try_from(frame.payload.len()) else {
            return;
        };
        let mut bytes = Vec::with_capacity(HEADER_LEN + frame.payload.len());
        bytes.extend_from_slice(&frame.id.to_le_bytes());
        bytes.push(frame.kind);
        bytes.extend_from_slice(&len.to_le_bytes());
        bytes.extend_from_slice(&frame.payload);
        if let Err(err) = wire.write_all(&bytes).await {
            tracing::debug!(error = %err, "tcp mux write ended");
            return;
        }
    }
    let _ = wire.shutdown().await;
}

async fn read_loop(
    mut wire: ReadHalf<TlsStream<TcpStream>>,
    side: Side,
    readers: Arc<Mutex<HashMap<u32, PipeWriter>>>,
    out: mpsc::Sender<Outgoing>,
    incoming: mpsc::Sender<Stream>,
    writer: AbortHandle,
) {
    if let Err(err) = demux(&mut wire, side, &readers, &out, &incoming).await {
        tracing::debug!(error = %err, "tcp mux read ended");
    }
    // Every inbound writer goes away, so open streams read end of file.
    lock(&readers).clear();
    writer.abort();
}

async fn demux(
    wire: &mut ReadHalf<TlsStream<TcpStream>>,
    side: Side,
    readers: &Arc<Mutex<HashMap<u32, PipeWriter>>>,
    out: &mpsc::Sender<Outgoing>,
    incoming: &mpsc::Sender<Stream>,
) -> io::Result<()> {
    loop {
        let mut header = [0u8; HEADER_LEN];
        wire.read_exact(&mut header).await?;
        let id = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
        let kind = header[4];
        let len = u32::from_le_bytes([header[5], header[6], header[7], header[8]]) as usize;
        if len > MAX_MUX_PAYLOAD {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("mux frame of {len} bytes exceeds {MAX_MUX_PAYLOAD}"),
            ));
        }
        let mut payload = vec![0u8; len];
        wire.read_exact(&mut payload).await?;
        match kind {
            KIND_DATA => {
                let known = lock(readers).contains_key(&id);
                if !known {
                    if side.opens(id) {
                        // A frame for a stream this side closed; the peer learns from the Fin it got.
                        continue;
                    }
                    let stream = attach(id, readers, out);
                    if incoming.send(stream).await.is_err() {
                        return Ok(());
                    }
                }
                let writer = lock(readers).remove(&id);
                if let Some(mut writer) = writer
                    && writer.write_all(&payload).await.is_ok()
                {
                    lock(readers).insert(id, writer);
                }
            }
            KIND_FIN => {
                lock(readers).remove(&id);
            }
            other => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("mux frame kind {other} is unknown"),
                ));
            }
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    // The guarded maps and channels are consistent at every lock boundary.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Connection for Mux {
    fn peer(&self) -> NodeId {
        self.peer
    }

    fn kind(&self) -> TransportKind {
        TransportKind::TcpTls
    }

    fn remote_addr(&self) -> SocketAddr {
        self.remote
    }

    fn open(&self) -> BoxFuture<'_, Result<Stream, TransportError>> {
        Box::pin(async { self.open_local() })
    }

    fn next(&self) -> BoxFuture<'_, Result<Stream, TransportError>> {
        Box::pin(async {
            let mut incoming = self.incoming.lock().await;
            incoming
                .recv()
                .await
                .ok_or_else(|| TransportError::Closed("tcp connection ended".to_owned()))
        })
    }

    fn close(&self) {
        for task in &self.tasks {
            task.abort();
        }
        lock(&self.readers).clear();
    }
}

impl Drop for Mux {
    fn drop(&mut self) {
        self.close();
    }
}

//! The stream multiplexer of the TCP fallback.
//!
//! Invariant: a stream id names one stream for the life of a connection and
//! is never reused. Inside the byte stream every logical stream is framed as
//! `u32 stream id | u8 kind | u32 length | payload`. The dialer opens odd
//! ids and the acceptor even ids, and each side puts its ids on the wire in
//! rising order: a stream takes its id when its first frame is queued, under
//! one lock. So a peer id above every id seen so far is a new stream, and
//! any other id that has no open stream is a closed one whose frames are
//! dropped. A stream never comes back from `next` once dropped.
//!
//! `Data` carries bytes, `Fin` ends one direction, and `Reset` tells the
//! peer this side's reader is gone, so the peer's writer fails with
//! `BrokenPipe` instead of sending into nothing, as QUIC's `STOP_SENDING`
//! does. The peer may hold at most `MAX_STREAMS` streams open, as quinn
//! allows by default; a stream above that is reset at once, and this side
//! waits in `open` instead of going over the peer's limit. A frame payload
//! above `MAX_MUX_PAYLOAD` ends the connection. So a peer can make this node
//! hold at most `MAX_STREAMS` stream buffers and one frame.
//!
//! Each direction of a stream has its own credit, as each QUIC stream has
//! its own window. A sender starts with `STREAM_BUFFER` bytes of credit and
//! spends it on `Data`; the receiver's buffer holds exactly that much, and a
//! `Credit` frame hands back what its reader has taken. The reader of the
//! connection therefore never waits on one stream: a stream nobody reads
//! stops only its own sender, and a peer that sends past its credit, or
//! grants more than a buffer holds, ends the connection.
//!
//! The connection lives while its handle or any of its streams does, as a
//! QUIC connection does; `close` ends it at once and every stream on it
//! fails. As on QUIC, a side that has written nothing for `KEEP_ALIVE`
//! sends a `KeepAlive` frame, and a side that has read nothing for
//! `IDLE_TIMEOUT` ends the connection, so a peer that slept or changed
//! networks does not leave a half-open connection behind.

use std::collections::HashMap;
use std::future::poll_fn;
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use fabric_core::BoxFuture;
use fabric_core::id::NodeId;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot, watch};
use tokio::task::AbortHandle;

use crate::idle::IdleReader;
use crate::pipe::{Drained, PipeReader, PipeWriter, Refused, pipe};
use crate::quic::{IDLE_TIMEOUT, KEEP_ALIVE};
use crate::transport::{Connection, Stream, TransportError, TransportKind};

/// The most payload bytes in one multiplexer frame.
pub const MAX_MUX_PAYLOAD: usize = 64 << 10;

/// The credit each direction of a stream starts with, and the bytes its receiver buffers.
pub const STREAM_BUFFER: usize = 1 << 20;

/// The most streams either side may hold open on one connection, quinn's default.
pub const MAX_STREAMS: usize = 100;

/// A receiver hands credit back once its reader took this many bytes.
const CREDIT_STEP: usize = STREAM_BUFFER / 4;

const HEADER_LEN: usize = 9;
const CREDIT_LEN: usize = 4;
const OUT_QUEUE: usize = 64;

/// What one multiplexer frame does to its stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum FrameKind {
    /// Carries the stream's next bytes.
    Data,
    /// Ends the sender's direction of the stream.
    Fin,
    /// The sender's reader is gone; stop sending on the stream.
    Reset,
    /// Gives the receiver of this frame more credit on the stream: a `u32` byte count.
    Credit,
    /// Says the sender is alive; it names no stream.
    KeepAlive,
}

impl FrameKind {
    fn byte(self) -> u8 {
        match self {
            FrameKind::Data => 0,
            FrameKind::Fin => 1,
            FrameKind::Reset => 2,
            FrameKind::Credit => 3,
            FrameKind::KeepAlive => 4,
        }
    }
}

impl TryFrom<u8> for FrameKind {
    type Error = io::Error;

    fn try_from(byte: u8) -> Result<Self, Self::Error> {
        match byte {
            0 => Ok(FrameKind::Data),
            1 => Ok(FrameKind::Fin),
            2 => Ok(FrameKind::Reset),
            3 => Ok(FrameKind::Credit),
            4 => Ok(FrameKind::KeepAlive),
            other => Err(invalid(format!("mux frame kind {other} is unknown"))),
        }
    }
}

fn invalid(reason: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, reason)
}

/// Which end of the connection this side is; it fixes which stream ids it opens.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Side {
    /// The side that dialed; it opens odd ids.
    Dialer,
    /// The side that accepted; it opens even ids.
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
    kind: FrameKind,
    payload: Vec<u8>,
}

impl Outgoing {
    fn bare(id: u32, kind: FrameKind) -> Self {
        Self {
            id,
            kind,
            payload: Vec::new(),
        }
    }
}

/// Dropped when the handle and every stream are gone; that ends the connection gracefully.
struct Alive {
    _signal: oneshot::Sender<()>,
}

/// One open stream's claim: a place under the stream cap, and the connection kept alive.
struct Slot {
    _permit: OwnedSemaphorePermit,
    _alive: Arc<Alive>,
}

/// The inbound half of an open stream: where its `Data` frames go.
///
/// Dropping it fails the stream's reader; only `finish`, on the peer's
/// `Fin`, ends it cleanly. So a stream cut off by a lost or closed
/// connection never reads as one that ended.
struct Inbound {
    writer: Option<PipeWriter>,
    _slot: Arc<Slot>,
}

impl Inbound {
    fn new(writer: PipeWriter, slot: Arc<Slot>) -> Self {
        Self {
            writer: Some(writer),
            _slot: slot,
        }
    }

    fn push(&self, payload: &[u8]) -> Result<(), Refused> {
        match &self.writer {
            Some(writer) => writer.push(payload),
            None => Err(Refused::ReaderGone),
        }
    }

    /// Ends the stream cleanly: its reader sees end of stream after the bytes already delivered.
    fn finish(mut self) {
        drop(self.writer.take());
    }
}

impl Drop for Inbound {
    fn drop(&mut self) {
        if let Some(writer) = self.writer.take() {
            writer.abort();
        }
    }
}

/// The outbound half of an open stream: dropping it stops its pump.
struct Outbound {
    _stop: oneshot::Sender<()>,
    _slot: Arc<Slot>,
    /// The bytes this side may still send before the peer hands credit back.
    credit: Arc<Semaphore>,
}

#[derive(Default)]
struct Streams {
    inbound: HashMap<u32, Inbound>,
    outbound: HashMap<u32, Outbound>,
    /// The highest id the peer has opened; zero before its first.
    peer_high: u32,
    closed: bool,
}

struct Shared {
    side: Side,
    out: mpsc::Sender<Outgoing>,
    streams: Mutex<Streams>,
    /// The next id this side opens; held while a stream queues its first frame.
    next_id: tokio::sync::Mutex<Option<u32>>,
    local_slots: Arc<Semaphore>,
    peer_slots: Arc<Semaphore>,
    /// Turns true when the connection ends, for streams that have no id yet.
    ended: watch::Sender<bool>,
}

impl Shared {
    fn streams(&self) -> std::sync::MutexGuard<'_, Streams> {
        // The maps are whole at every lock boundary.
        self.streams.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Fails every open stream: readers get an error, writers `BrokenPipe`.
    fn fail_all(&self) {
        let (inbound, outbound) = {
            let mut streams = self.streams();
            streams.closed = true;
            (
                std::mem::take(&mut streams.inbound),
                std::mem::take(&mut streams.outbound),
            )
        };
        drop((inbound, outbound));
        self.local_slots.close();
        self.peer_slots.close();
        self.ended.send_replace(true);
    }
}

fn closed() -> TransportError {
    TransportError::Closed("tcp connection ended".to_owned())
}

/// One multiplexed connection over a byte stream.
pub(crate) struct Mux {
    peer: NodeId,
    remote: SocketAddr,
    shared: Arc<Shared>,
    alive: Arc<Alive>,
    incoming: tokio::sync::Mutex<mpsc::Receiver<Stream>>,
    tasks: [AbortHandle; 2],
}

impl Mux {
    /// Starts the reader and writer tasks over `io`.
    pub(crate) fn spawn<IO>(io: IO, side: Side, peer: NodeId, remote: SocketAddr) -> Self
    where
        IO: AsyncRead + AsyncWrite + Send + 'static,
    {
        let (read_half, write_half) = tokio::io::split(io);
        let (out, out_rx) = mpsc::channel::<Outgoing>(OUT_QUEUE);
        // Every waiting stream holds a slot, so the queue never fills.
        let (incoming_tx, incoming_rx) = mpsc::channel::<Stream>(MAX_STREAMS);
        let (alive_tx, alive_rx) = oneshot::channel();
        let alive = Arc::new(Alive { _signal: alive_tx });
        let shared = Arc::new(Shared {
            side,
            out,
            streams: Mutex::default(),
            next_id: tokio::sync::Mutex::new(Some(side.first_id())),
            local_slots: Arc::new(Semaphore::new(MAX_STREAMS)),
            peer_slots: Arc::new(Semaphore::new(MAX_STREAMS)),
            ended: watch::Sender::new(false),
        });
        let writer = tokio::spawn(write_loop(write_half, out_rx));
        let reader = tokio::spawn(read_loop(
            read_half,
            Arc::clone(&shared),
            Arc::downgrade(&alive),
            alive_rx,
            incoming_tx,
            writer.abort_handle(),
        ));
        Self {
            peer,
            remote,
            shared,
            alive,
            incoming: tokio::sync::Mutex::new(incoming_rx),
            tasks: [reader.abort_handle(), writer.abort_handle()],
        }
    }

    async fn open_local(&self) -> Result<Stream, TransportError> {
        let permit = Arc::clone(&self.shared.local_slots)
            .acquire_owned()
            .await
            .map_err(|_| closed())?;
        if self.shared.streams().closed {
            return Err(closed());
        }
        let slot = Arc::new(Slot {
            _permit: permit,
            _alive: Arc::clone(&self.alive),
        });
        let (inbound_writer, inbound_reader) = pipe(STREAM_BUFFER);
        let (outbound_writer, outbound_reader) = pipe(STREAM_BUFFER);
        tokio::spawn(pump_local(
            Arc::clone(&self.shared),
            outbound_reader,
            inbound_writer,
            slot,
        ));
        Ok(Stream::new(outbound_writer, inbound_reader))
    }
}

/// Sends a locally opened stream: it takes its id with its first frame.
async fn pump_local(shared: Arc<Shared>, source: PipeReader, inbound: PipeWriter, slot: Arc<Slot>) {
    let drained = inbound.drained();
    let inbound = Inbound::new(inbound, Arc::clone(&slot));
    let mut ended = shared.ended.subscribe();
    let first = tokio::select! {
        first = poll_fn(|cx| source.poll_take(cx, MAX_MUX_PAYLOAD)) => first.unwrap_or_default(),
        // Dropping `inbound` fails the stream's reader.
        _ = ended.wait_for(|ended| *ended) => return,
    };
    let (stop_tx, stop_rx) = oneshot::channel();
    // The first frame spends its own length of the stream's credit.
    let credit = Arc::new(Semaphore::new(STREAM_BUFFER - first.len()));
    let id = {
        let mut next = shared.next_id.lock().await;
        let Some(id) = *next else {
            tracing::warn!("tcp connection ran out of stream ids");
            return;
        };
        {
            let mut streams = shared.streams();
            if streams.closed {
                return;
            }
            streams.inbound.insert(id, inbound);
            if !first.is_empty() {
                streams.outbound.insert(
                    id,
                    Outbound {
                        _stop: stop_tx,
                        _slot: slot,
                        credit: Arc::clone(&credit),
                    },
                );
            }
        }
        *next = id.checked_add(2);
        tokio::spawn(grant_credit(Arc::clone(&shared), id, drained));
        // Queued under the id lock, so ids reach the wire in rising order.
        let only_fin = first.is_empty();
        let frame = if only_fin {
            Outgoing::bare(id, FrameKind::Fin)
        } else {
            Outgoing {
                id,
                kind: FrameKind::Data,
                payload: first,
            }
        };
        if shared.out.send(frame).await.is_err() || only_fin {
            return;
        }
        id
    };
    send_rest(&shared, id, source, stop_rx, &credit).await;
}

/// Sends a peer-opened stream's answers; its id and slot were registered when it arrived.
async fn pump_peer(
    shared: Arc<Shared>,
    id: u32,
    source: PipeReader,
    stop: oneshot::Receiver<()>,
    credit: Arc<Semaphore>,
) {
    send_rest(&shared, id, source, stop, &credit).await;
}

/// Frames what the stream's writer writes, within the stream's credit, until it finishes, the peer resets it, or the connection ends.
async fn send_rest(
    shared: &Shared,
    id: u32,
    source: PipeReader,
    mut stop: oneshot::Receiver<()>,
    credit: &Semaphore,
) {
    loop {
        let taken = tokio::select! {
            biased;
            _ = &mut stop => return,
            taken = poll_fn(|cx| source.poll_take(cx, MAX_MUX_PAYLOAD)) => taken.unwrap_or_default(),
        };
        if taken.is_empty() {
            // The slot goes before the Fin, so a peer that saw the Fin
            // never counts this stream against the cap.
            shared.streams().outbound.remove(&id);
            let _ = shared.out.send(Outgoing::bare(id, FrameKind::Fin)).await;
            return;
        }
        // At most MAX_MUX_PAYLOAD, so it fits a u32.
        let Ok(len) = u32::try_from(taken.len()) else {
            return;
        };
        tokio::select! {
            biased;
            _ = &mut stop => return,
            spent = credit.acquire_many(len) => match spent {
                Ok(spent) => spent.forget(),
                Err(_) => return,
            },
        }
        let frame = Outgoing {
            id,
            kind: FrameKind::Data,
            payload: taken,
        };
        tokio::select! {
            biased;
            _ = &mut stop => return,
            sent = shared.out.send(frame) => if sent.is_err() {
                return;
            },
        }
    }
}

/// Hands the peer credit for `id` each time the stream's reader takes `CREDIT_STEP` bytes, until the peer's direction ends.
async fn grant_credit(shared: Arc<Shared>, id: u32, drained: Drained) {
    while let Some(taken) = poll_fn(|cx| drained.poll_report(cx, CREDIT_STEP)).await {
        // At most STREAM_BUFFER, so it fits a u32.
        let Ok(taken) = u32::try_from(taken) else {
            return;
        };
        let frame = Outgoing {
            id,
            kind: FrameKind::Credit,
            payload: taken.to_le_bytes().to_vec(),
        };
        if shared.out.send(frame).await.is_err() {
            return;
        }
    }
}

async fn write_loop(
    mut wire: tokio::io::WriteHalf<impl AsyncWrite>,
    mut out: mpsc::Receiver<Outgoing>,
) {
    loop {
        let frame = match out.try_recv() {
            Ok(frame) => frame,
            Err(mpsc::error::TryRecvError::Empty) => {
                // TLS takes written bytes into its own buffer and sends them
                // only as the socket has room; with nothing queued, push
                // them out before waiting, or a stream's last bytes sit
                // there until the next frame.
                if let Err(err) = wire.flush().await {
                    tracing::debug!(error = %err, "tcp mux flush ended");
                    return;
                }
                match tokio::time::timeout(KEEP_ALIVE, out.recv()).await {
                    Ok(Some(frame)) => frame,
                    Ok(None) => break,
                    Err(_) => Outgoing::bare(0, FrameKind::KeepAlive),
                }
            }
            Err(mpsc::error::TryRecvError::Disconnected) => break,
        };
        let Ok(len) = u32::try_from(frame.payload.len()) else {
            return;
        };
        let mut bytes = Vec::with_capacity(HEADER_LEN + frame.payload.len());
        bytes.extend_from_slice(&frame.id.to_le_bytes());
        bytes.push(frame.kind.byte());
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
    wire: tokio::io::ReadHalf<impl AsyncRead>,
    shared: Arc<Shared>,
    alive: Weak<Alive>,
    gone: oneshot::Receiver<()>,
    incoming: mpsc::Sender<Stream>,
    writer: AbortHandle,
) {
    let mut wire = IdleReader::new(wire, IDLE_TIMEOUT);
    tokio::select! {
        ended = demux(&mut wire, &shared, &alive, &incoming) => {
            if let Err(err) = ended {
                tracing::debug!(error = %err, "tcp mux read ended");
            }
            shared.fail_all();
            writer.abort();
        }
        // The handle and every stream are gone: stop reading and let the
        // writer flush what is queued, then shut the connection down.
        _ = gone => {}
    }
}

async fn demux(
    wire: &mut (impl AsyncRead + Unpin),
    shared: &Arc<Shared>,
    alive: &Weak<Alive>,
    incoming: &mpsc::Sender<Stream>,
) -> io::Result<()> {
    loop {
        let mut header = [0u8; HEADER_LEN];
        wire.read_exact(&mut header).await?;
        let id = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
        let kind = FrameKind::try_from(header[4])?;
        let len = u32::from_le_bytes([header[5], header[6], header[7], header[8]]) as usize;
        if len > MAX_MUX_PAYLOAD {
            return Err(invalid(format!(
                "mux frame of {len} bytes exceeds {MAX_MUX_PAYLOAD}"
            )));
        }
        let mut payload = vec![0u8; len];
        wire.read_exact(&mut payload).await?;
        match kind {
            FrameKind::Data | FrameKind::Fin => {
                if !shared.side.opens(id) && id > shared.streams().peer_high {
                    accept_stream(shared, id, alive, incoming).await;
                }
                if kind == FrameKind::Fin {
                    let finished = shared.streams().inbound.remove(&id);
                    if let Some(stream) = finished {
                        stream.finish();
                    }
                } else if !deliver(shared, id, &payload)? {
                    reset(shared, id).await;
                }
            }
            FrameKind::Reset => {
                shared.streams().outbound.remove(&id);
            }
            FrameKind::Credit => credit(shared, id, &payload)?,
            FrameKind::KeepAlive => {}
        }
    }
}

/// Opens the stream the peer just started, or resets it when the peer is at the cap or nobody accepts.
async fn accept_stream(
    shared: &Arc<Shared>,
    id: u32,
    alive: &Weak<Alive>,
    incoming: &mpsc::Sender<Stream>,
) {
    shared.streams().peer_high = id;
    let (Ok(permit), Some(alive)) = (
        Arc::clone(&shared.peer_slots).try_acquire_owned(),
        alive.upgrade(),
    ) else {
        tracing::debug!(id, "refusing a peer stream over the cap");
        let _ = shared.out.send(Outgoing::bare(id, FrameKind::Reset)).await;
        return;
    };
    let slot = Arc::new(Slot {
        _permit: permit,
        _alive: alive,
    });
    let (inbound_writer, inbound_reader) = pipe(STREAM_BUFFER);
    let (outbound_writer, outbound_reader) = pipe(STREAM_BUFFER);
    let (stop_tx, stop_rx) = oneshot::channel();
    let credit = Arc::new(Semaphore::new(STREAM_BUFFER));
    tokio::spawn(grant_credit(
        Arc::clone(shared),
        id,
        inbound_writer.drained(),
    ));
    {
        let mut streams = shared.streams();
        streams
            .inbound
            .insert(id, Inbound::new(inbound_writer, Arc::clone(&slot)));
        streams.outbound.insert(
            id,
            Outbound {
                _stop: stop_tx,
                _slot: slot,
                credit: Arc::clone(&credit),
            },
        );
    }
    tokio::spawn(pump_peer(
        Arc::clone(shared),
        id,
        outbound_reader,
        stop_rx,
        credit,
    ));
    if incoming
        .try_send(Stream::new(outbound_writer, inbound_reader))
        .is_err()
    {
        // The handle is gone, so nobody will take this stream.
        reset(shared, id).await;
    }
}

/// Puts a `Data` payload in its stream's buffer without waiting.
///
/// Returns false when the stream's reader is gone, so the caller resets
/// it once; a closed stream, ours or the peer's, has no entry and its
/// frames are dropped. A payload the buffer cannot hold means the peer
/// sent past its credit, and ends the connection.
fn deliver(shared: &Shared, id: u32, payload: &[u8]) -> io::Result<bool> {
    let mut streams = shared.streams();
    let Some(stream) = streams.inbound.get(&id) else {
        return Ok(true);
    };
    match stream.push(payload) {
        Ok(()) => Ok(true),
        Err(Refused::Full) => Err(invalid(format!(
            "the peer sent past its credit on stream {id}"
        ))),
        Err(Refused::ReaderGone) => {
            streams.inbound.remove(&id);
            Ok(false)
        }
    }
}

/// Adds the credit the peer handed back for `id`; more than one buffer's worth ends the connection.
fn credit(shared: &Shared, id: u32, payload: &[u8]) -> io::Result<()> {
    let bytes: [u8; CREDIT_LEN] = payload
        .try_into()
        .map_err(|_| invalid(format!("a credit frame of {} bytes", payload.len())))?;
    let granted = u32::from_le_bytes(bytes) as usize;
    let streams = shared.streams();
    // A stream this side finished or the peer reset has no entry left.
    let Some(stream) = streams.outbound.get(&id) else {
        return Ok(());
    };
    if stream.credit.available_permits().saturating_add(granted) > STREAM_BUFFER {
        return Err(invalid(format!(
            "the peer granted more than a buffer of credit on stream {id}"
        )));
    }
    stream.credit.add_permits(granted);
    Ok(())
}

/// Forgets the inbound half of `id` and tells the peer to stop sending on it.
async fn reset(shared: &Shared, id: u32) {
    shared.streams().inbound.remove(&id);
    let _ = shared.out.send(Outgoing::bare(id, FrameKind::Reset)).await;
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
        Box::pin(self.open_local())
    }

    fn next(&self) -> BoxFuture<'_, Result<Stream, TransportError>> {
        Box::pin(async {
            let mut incoming = self.incoming.lock().await;
            incoming.recv().await.ok_or_else(closed)
        })
    }

    fn close(&self) {
        for task in &self.tasks {
            task.abort();
        }
        self.shared.fail_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::DuplexStream;

    const ADDR: SocketAddr =
        SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 1);

    fn node() -> NodeId {
        NodeId::from_bytes([9; 32])
    }

    /// A peer that speaks the frame format by hand, to send what an honest multiplexer never would.
    struct RawPeer(DuplexStream);

    impl RawPeer {
        async fn send(&mut self, id: u32, kind: FrameKind, payload: &[u8]) {
            let mut bytes = id.to_le_bytes().to_vec();
            bytes.push(kind.byte());
            bytes.extend_from_slice(&u32::try_from(payload.len()).unwrap().to_le_bytes());
            bytes.extend_from_slice(payload);
            self.0.write_all(&bytes).await.unwrap();
        }

        async fn recv(&mut self) -> (u32, FrameKind, Vec<u8>) {
            let mut header = [0u8; HEADER_LEN];
            self.0.read_exact(&mut header).await.unwrap();
            let len = u32::from_le_bytes(header[5..9].try_into().unwrap()) as usize;
            let mut payload = vec![0u8; len];
            self.0.read_exact(&mut payload).await.unwrap();
            (
                u32::from_le_bytes(header[..4].try_into().unwrap()),
                FrameKind::try_from(header[4]).unwrap(),
                payload,
            )
        }
    }

    /// An acceptor-side multiplexer and a raw dialer on the other end.
    fn acceptor_and_raw() -> (Mux, RawPeer) {
        let (ours, theirs) = tokio::io::duplex(4 << 20);
        (
            Mux::spawn(ours, Side::Acceptor, node(), ADDR),
            RawPeer(theirs),
        )
    }

    async fn nothing_from_next(mux: &Mux) -> bool {
        tokio::time::timeout(Duration::from_millis(200), mux.next())
            .await
            .is_err()
    }

    /// A stream this side dropped stays closed: the peer's later frames for
    /// it are dropped, answered with one reset, and never surface as a new stream.
    #[tokio::test]
    async fn a_dropped_stream_never_comes_back_and_its_peer_is_reset_once() {
        let (mux, mut raw) = acceptor_and_raw();
        raw.send(1, FrameKind::Data, b"request").await;
        let stream = mux.next().await.unwrap();
        drop(stream);
        for _ in 0..3 {
            raw.send(1, FrameKind::Data, b"more").await;
        }
        assert!(nothing_from_next(&mux).await, "a closed id came back");
        let mut answers = vec![raw.recv().await, raw.recv().await];
        answers.sort_unstable();
        assert_eq!(
            answers,
            [
                (1, FrameKind::Fin, Vec::new()),
                (1, FrameKind::Reset, Vec::new())
            ],
            "the dropped writer finishes and the dropped reader resets"
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(200), raw.recv())
                .await
                .is_err(),
            "one reset, not one per frame"
        );
    }

    /// A peer that opens more than `MAX_STREAMS` streams has the extra ones reset.
    #[tokio::test]
    async fn a_peer_stream_over_the_cap_is_reset() {
        let (mux, mut raw) = acceptor_and_raw();
        let ids: Vec<u32> = (0..=u32::try_from(MAX_STREAMS).unwrap())
            .map(|n| 2 * n + 1)
            .collect();
        for id in &ids {
            raw.send(*id, FrameKind::Data, b"x").await;
        }
        let mut held = Vec::new();
        for _ in 0..MAX_STREAMS {
            held.push(mux.next().await.unwrap());
        }
        assert!(nothing_from_next(&mux).await);
        let over = *ids.last().unwrap();
        assert_eq!(raw.recv().await, (over, FrameKind::Reset, Vec::new()));
    }

    /// A reset from the peer stops this side's writer instead of letting it send into nothing.
    #[tokio::test]
    async fn a_reset_from_the_peer_fails_the_local_writer() {
        let (mux, mut raw) = acceptor_and_raw();
        let mut stream = mux.open().await.unwrap();
        stream.write_all(b"first").await.unwrap();
        let (id, kind, _) = raw.recv().await;
        assert_eq!(kind, FrameKind::Data);
        raw.send(id, FrameKind::Reset, &[]).await;
        let chunk = vec![0u8; 64 << 10];
        let mut failed = None;
        for _ in 0..64 {
            if let Err(err) = stream.write_all(&chunk).await {
                failed = Some(err.kind());
                break;
            }
        }
        assert_eq!(failed, Some(io::ErrorKind::BrokenPipe));
    }

    /// A peer that sends more than the credit it was given ends the
    /// connection, instead of making this side buffer or wait on one stream.
    #[tokio::test]
    async fn a_peer_that_sends_past_its_credit_ends_the_connection() {
        let (mux, mut raw) = acceptor_and_raw();
        let frame = vec![0u8; MAX_MUX_PAYLOAD];
        for _ in 0..STREAM_BUFFER / MAX_MUX_PAYLOAD {
            raw.send(1, FrameKind::Data, &frame).await;
        }
        let mut unread = mux.next().await.unwrap();
        raw.send(1, FrameKind::Data, b"x").await;
        let ended = tokio::time::timeout(Duration::from_secs(1), mux.next()).await;
        assert!(ended.unwrap().is_err(), "the connection must end");
        let mut byte = [0u8; 1];
        assert_eq!(
            unread.read(&mut byte).await.unwrap_err().kind(),
            io::ErrorKind::ConnectionAborted
        );
    }

    /// A peer that sends nothing, not even a keep-alive, is gone: the
    /// connection ends after `IDLE_TIMEOUT` instead of staying half open.
    #[tokio::test(start_paused = true)]
    async fn a_silent_peer_ends_the_connection_after_the_idle_timeout() {
        let (mux, _raw) = acceptor_and_raw();
        let started = tokio::time::Instant::now();
        assert!(mux.next().await.is_err());
        let waited = started.elapsed();
        assert!(
            waited >= IDLE_TIMEOUT && waited < IDLE_TIMEOUT + Duration::from_secs(1),
            "{waited:?}"
        );
    }

    /// Two live sides that have nothing to say keep their connection.
    #[tokio::test(start_paused = true)]
    async fn an_idle_connection_stays_up_on_keep_alives() {
        let (a, b) = tokio::io::duplex(1 << 20);
        let dialer = Mux::spawn(a, Side::Dialer, node(), ADDR);
        let acceptor = Mux::spawn(b, Side::Acceptor, node(), ADDR);
        tokio::time::sleep(IDLE_TIMEOUT * 3).await;
        let mut stream = dialer.open().await.unwrap();
        stream.write_all(b"still here").await.unwrap();
        stream.shutdown().await.unwrap();
        let mut arrived = acceptor.next().await.unwrap();
        let mut bytes = Vec::new();
        arrived.read_to_end(&mut bytes).await.unwrap();
        assert_eq!(bytes, b"still here");
    }

    /// A stream opened but not yet written has no id; it still fails when the connection ends.
    #[tokio::test]
    async fn a_stream_without_an_id_fails_when_the_connection_closes() {
        let (mux, _raw) = acceptor_and_raw();
        let mut stream = mux.open().await.unwrap();
        mux.close();
        let mut byte = [0u8; 1];
        let read = tokio::time::timeout(Duration::from_secs(1), stream.read(&mut byte)).await;
        assert_eq!(
            read.unwrap().unwrap_err().kind(),
            io::ErrorKind::ConnectionAborted
        );
    }

    /// TLS holds written bytes until it is flushed, as `BufWriter` does
    /// here; a stream's last frames must still reach the peer when nothing
    /// else is queued behind them.
    #[tokio::test]
    async fn the_writer_flushes_the_wire_when_its_queue_runs_empty() {
        let (a, b) = tokio::io::duplex(1 << 20);
        let dialer = Mux::spawn(tokio::io::BufWriter::new(a), Side::Dialer, node(), ADDR);
        let acceptor = Mux::spawn(b, Side::Acceptor, node(), ADDR);
        let mut stream = dialer.open().await.unwrap();
        stream.write_all(b"ask").await.unwrap();
        stream.shutdown().await.unwrap();
        let mut arrived = tokio::time::timeout(Duration::from_secs(1), acceptor.next())
            .await
            .expect("the frames stayed in the wire's buffer")
            .unwrap();
        let mut bytes = Vec::new();
        arrived.read_to_end(&mut bytes).await.unwrap();
        assert_eq!(bytes, b"ask");
    }

    /// Ids reach the wire in the order streams first write, whatever order they were opened in.
    #[tokio::test]
    async fn streams_written_out_of_open_order_all_arrive() {
        let (a, b) = tokio::io::duplex(1 << 20);
        let dialer = Mux::spawn(a, Side::Dialer, node(), ADDR);
        let acceptor = Mux::spawn(b, Side::Acceptor, node(), ADDR);
        let mut first = dialer.open().await.unwrap();
        let mut second = dialer.open().await.unwrap();
        let mut got = Vec::new();
        for (stream, byte) in [(&mut second, 0u8), (&mut first, 1)] {
            stream.write_all(&[byte]).await.unwrap();
            stream.shutdown().await.unwrap();
            let mut arrived = acceptor.next().await.unwrap();
            let mut bytes = Vec::new();
            arrived.read_to_end(&mut bytes).await.unwrap();
            got.push(bytes);
        }
        assert_eq!(got, [[0], [1]]);
    }
}

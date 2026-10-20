//! Accepting connections in the background, for both wires.
//!
//! Invariant: one handshake never holds up another. Each socket a listener
//! accepts finishes its handshake in its own task under `HANDSHAKE_TIMEOUT`,
//! at most `MAX_HANDSHAKES` at once and at most `MAX_PER_SOURCE` from one
//! source address, so a host that opens sockets and stays silent holds a
//! few slots, not all of them. Only a finished connection from a
//! node that is still paired reaches the queue that `accept` reads. Reading
//! the queue is cancel-safe, so a caller that races two transports in
//! `select!` drops no half-done handshake. The trust decision is refreshed
//! on a blocking thread before each handshake and again after it, so the
//! verifier inside the handshake never touches a file.

use std::collections::HashMap;
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, PoisonError};
use std::time::Duration;

use tokio::sync::{Mutex, Semaphore, mpsc};
use tokio::task::AbortHandle;

use crate::tls::Trust;
use crate::transport::{Connection, TransportError};

/// How long an accepted socket may take to finish its handshake.
pub(crate) const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// The most handshakes one listener runs at once; the next socket waits for a slot.
pub(crate) const MAX_HANDSHAKES: usize = 32;

/// The most handshakes one source address may run at once; a socket over it is refused.
pub(crate) const MAX_PER_SOURCE: usize = 4;

/// How many finished connections wait for `accept` before handshakes wait instead.
const QUEUE: usize = 16;

/// The finished connections of one listener, in the order their handshakes ended.
pub(crate) struct Inbox {
    queue: Mutex<mpsc::Receiver<Box<dyn Connection>>>,
    accept_loop: AbortHandle,
}

impl Inbox {
    /// Spawns the listener's accept loop, which hands each socket to `Handshakes::run`.
    pub(crate) fn spawn<F, Fut>(trust: Arc<dyn Trust>, accept_loop: F) -> Self
    where
        F: FnOnce(Handshakes) -> Fut,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let (tx, rx) = mpsc::channel(QUEUE);
        let handshakes = Handshakes {
            tx,
            slots: Arc::new(Semaphore::new(MAX_HANDSHAKES)),
            sources: Arc::default(),
            trust,
        };
        let task = tokio::spawn(accept_loop(handshakes));
        Self {
            queue: Mutex::new(rx),
            accept_loop: task.abort_handle(),
        }
    }

    /// Waits for the next finished connection; cancelling the wait loses nothing.
    pub(crate) async fn next(&self) -> Result<Box<dyn Connection>, TransportError> {
        self.queue
            .lock()
            .await
            .recv()
            .await
            .ok_or(TransportError::EndpointClosed)
    }
}

impl Drop for Inbox {
    fn drop(&mut self) {
        self.accept_loop.abort();
    }
}

/// How many handshakes each source address runs now.
type Sources = Arc<std::sync::Mutex<HashMap<IpAddr, usize>>>;

/// One running handshake's claim on its source address; dropping it gives the claim back.
struct SourceClaim {
    sources: Sources,
    ip: IpAddr,
}

impl SourceClaim {
    /// Claims a place for `ip`, or `None` when it already runs `MAX_PER_SOURCE` handshakes.
    fn take(sources: &Sources, ip: IpAddr) -> Option<Self> {
        let mut running = sources.lock().unwrap_or_else(PoisonError::into_inner);
        let count = running.entry(ip).or_default();
        if *count >= MAX_PER_SOURCE {
            return None;
        }
        *count += 1;
        Some(Self {
            sources: Arc::clone(sources),
            ip,
        })
    }
}

impl Drop for SourceClaim {
    fn drop(&mut self) {
        let mut running = self.sources.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(count) = running.get_mut(&self.ip) {
            *count -= 1;
            if *count == 0 {
                running.remove(&self.ip);
            }
        }
    }
}

/// Runs the handshakes of one listener: bounded in number, per source and in time.
pub(crate) struct Handshakes {
    tx: mpsc::Sender<Box<dyn Connection>>,
    slots: Arc<Semaphore>,
    sources: Sources,
    trust: Arc<dyn Trust>,
}

impl Handshakes {
    /// Waits for a free slot, then finishes `handshake` in its own task.
    ///
    /// A source that already runs `MAX_PER_SOURCE` handshakes is refused at
    /// once: `handshake` is dropped unpolled, which closes its socket.
    /// Returns false once nobody reads the queue, so the accept loop can stop.
    pub(crate) async fn run<H>(&self, remote: SocketAddr, handshake: H) -> bool
    where
        H: Future<Output = Result<Box<dyn Connection>, TransportError>> + Send + 'static,
    {
        let Some(claim) = SourceClaim::take(&self.sources, remote.ip()) else {
            tracing::debug!(%remote, "refusing a socket over the per-source handshake cap");
            return !self.tx.is_closed();
        };
        let Ok(slot) = Arc::clone(&self.slots).acquire_owned().await else {
            return false;
        };
        if self.tx.is_closed() {
            return false;
        }
        let tx = self.tx.clone();
        let trust = Arc::clone(&self.trust);
        tokio::spawn(async move {
            let _claims = (slot, claim);
            refresh(&trust).await;
            let connection = match tokio::time::timeout(HANDSHAKE_TIMEOUT, handshake).await {
                Err(_) => return tracing::debug!(%remote, "handshake timed out"),
                Ok(Err(err)) => return tracing::debug!(%remote, error = %err, "handshake refused"),
                Ok(Ok(connection)) => connection,
            };
            // The verifier checked pairing inside the handshake; checking
            // again here keeps an unpair made during the handshake.
            refresh(&trust).await;
            if !trust.is_paired(connection.peer()) {
                tracing::info!(%remote, peer = %connection.peer(), "peer unpaired during the handshake");
                connection.close();
                return;
            }
            // A send fails only when the transport is gone, and then
            // dropping the connection is the right end for it.
            let _ = tx.send(connection).await;
        });
        true
    }
}

/// Runs `trust.refresh` on a blocking thread, off the threads that drive connections.
async fn refresh(trust: &Arc<dyn Trust>) {
    let trust = Arc::clone(trust);
    if let Err(err) = tokio::task::spawn_blocking(move || trust.refresh()).await {
        tracing::error!(error = %err, "refreshing the trust decision failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_core::id::NodeId;
    use tokio::sync::oneshot;

    #[derive(Debug)]
    struct NoOne;

    impl Trust for NoOne {
        fn is_paired(&self, _node: NodeId) -> bool {
            false
        }
    }

    /// A handshake that never ends and reports when it is dropped.
    fn stalled() -> (
        impl Future<Output = Result<Box<dyn Connection>, TransportError>> + Send + 'static,
        oneshot::Receiver<()>,
    ) {
        let (alive, dropped) = oneshot::channel::<()>();
        let handshake = async move {
            let _alive = alive;
            std::future::pending().await
        };
        (handshake, dropped)
    }

    /// One host that opens sockets and stays silent holds at most
    /// `MAX_PER_SOURCE` handshakes; its next socket is refused, and another
    /// host still gets in.
    #[tokio::test]
    async fn one_source_runs_at_most_its_share_of_handshakes() {
        let (tx, _rx) = mpsc::channel(QUEUE);
        let handshakes = Handshakes {
            tx,
            slots: Arc::new(Semaphore::new(MAX_HANDSHAKES)),
            sources: Arc::default(),
            trust: Arc::new(NoOne),
        };
        let attacker: SocketAddr = ([203, 0, 113, 7], 40_000).into();
        let mut held = Vec::new();
        for _ in 0..MAX_PER_SOURCE {
            let (handshake, dropped) = stalled();
            assert!(handshakes.run(attacker, handshake).await);
            held.push(dropped);
        }
        let (handshake, mut over) = stalled();
        assert!(handshakes.run(attacker, handshake).await);
        assert!(
            over.try_recv()
                .is_err_and(|e| e == oneshot::error::TryRecvError::Closed)
        );

        let (handshake, mut other) = stalled();
        assert!(
            handshakes
                .run(([198, 51, 100, 2], 40_000).into(), handshake)
                .await
        );
        tokio::task::yield_now().await;
        assert_eq!(other.try_recv(), Err(oneshot::error::TryRecvError::Empty));
        for mut dropped in held {
            assert_eq!(dropped.try_recv(), Err(oneshot::error::TryRecvError::Empty));
        }
    }
}

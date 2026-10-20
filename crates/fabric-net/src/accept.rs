//! Accepting connections in the background, for both wires.
//!
//! Invariant: one handshake never holds up another. Each socket a listener
//! accepts finishes its handshake in its own task under `HANDSHAKE_TIMEOUT`,
//! at most `MAX_HANDSHAKES` at once, and only a finished connection from a
//! node that is still paired reaches the queue that `accept` reads. Reading
//! the queue is cancel-safe, so a caller that races two transports in
//! `select!` drops no half-done handshake. The trust decision is refreshed
//! on a blocking thread before each handshake and again after it, so the
//! verifier inside the handshake never touches a file.

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, Semaphore, mpsc};
use tokio::task::AbortHandle;

use crate::tls::Trust;
use crate::transport::{Connection, TransportError};

/// How long an accepted socket may take to finish its handshake.
pub(crate) const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// The most handshakes one listener runs at once; the next socket waits for a slot.
pub(crate) const MAX_HANDSHAKES: usize = 32;

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

/// Runs the handshakes of one listener: bounded in number and in time.
pub(crate) struct Handshakes {
    tx: mpsc::Sender<Box<dyn Connection>>,
    slots: Arc<Semaphore>,
    trust: Arc<dyn Trust>,
}

impl Handshakes {
    /// Waits for a free slot, then finishes `handshake` in its own task.
    ///
    /// Returns false once nobody reads the queue, so the accept loop can stop.
    pub(crate) async fn run<H>(&self, remote: SocketAddr, handshake: H) -> bool
    where
        H: Future<Output = Result<Box<dyn Connection>, TransportError>> + Send + 'static,
    {
        let Ok(slot) = Arc::clone(&self.slots).acquire_owned().await else {
            return false;
        };
        if self.tx.is_closed() {
            return false;
        }
        let tx = self.tx.clone();
        let trust = Arc::clone(&self.trust);
        tokio::spawn(async move {
            let _slot = slot;
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

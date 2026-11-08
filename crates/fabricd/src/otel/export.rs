//! The exporter: a bounded queue the node's tasks put spans on without
//! waiting, and one task that posts them in batches as OTLP/HTTP JSON
//! over a plain TCP connection.

use std::fmt;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use fabric_core::id::NodeId;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};

use super::span::{OpSpan, body};

/// Spans the queue holds; one more is dropped and counted.
pub(crate) const QUEUE: usize = 1024;

/// Spans one post carries at most.
pub(crate) const BATCH: usize = 256;

/// How long a span waits for its batch at most.
const EVERY: Duration = Duration::from_secs(2);

/// How long one post may take, connect to answer.
const POST_DEADLINE: Duration = Duration::from_secs(5);

/// How often a dropped batch is logged at most.
const WARN_EVERY: Duration = Duration::from_mins(1);

/// Where spans go: an `http://host[:port][/base]` URL. The exporter posts
/// to `<base>/v1/traces`, as OTLP/HTTP's `OTEL_EXPORTER_OTLP_ENDPOINT` says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OtlpEndpoint {
    host: String,
    port: u16,
    path: String,
}

/// Why a text is not an OTLP endpoint this node can post to.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OtlpEndpointError {
    /// An `https` URL: the node speaks no TLS to a collector.
    #[error("https is not supported: run a local collector and give its http://host:4318 URL")]
    Https,
    /// Not `http://host[:port][/base]`.
    #[error("not an http://host[:port] URL")]
    NotHttp,
    /// The port is not a number from 1 to 65535.
    #[error("the port is not a number from 1 to 65535")]
    Port,
}

impl FromStr for OtlpEndpoint {
    type Err = OtlpEndpointError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text.starts_with("https://") {
            return Err(OtlpEndpointError::Https);
        }
        let rest = text
            .strip_prefix("http://")
            .ok_or(OtlpEndpointError::NotHttp)?;
        let (authority, base) = rest.find('/').map_or((rest, ""), |i| rest.split_at(i));
        if base.contains(['?', '#']) {
            return Err(OtlpEndpointError::NotHttp);
        }
        let port = |text: &str| {
            text.parse::<u16>()
                .ok()
                .filter(|p| *p != 0)
                .ok_or(OtlpEndpointError::Port)
        };
        let (host, port, v6) = if let Some(bracketed) = authority.strip_prefix('[') {
            let (host, after) = bracketed
                .split_once(']')
                .ok_or(OtlpEndpointError::NotHttp)?;
            let port = match after {
                "" => 80,
                after => port(after.strip_prefix(':').ok_or(OtlpEndpointError::NotHttp)?)?,
            };
            (host, port, true)
        } else {
            match authority.split_once(':') {
                Some((host, text)) => (host, port(text)?, false),
                None => (authority, 80, false),
            }
        };
        let valid = !host.is_empty()
            && host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || (v6 && b == b':'));
        if !valid {
            return Err(OtlpEndpointError::NotHttp);
        }
        Ok(OtlpEndpoint {
            host: host.to_owned(),
            port,
            path: format!("{}/v1/traces", base.trim_end_matches('/')),
        })
    }
}

impl fmt::Display for OtlpEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.host.contains(':') {
            write!(f, "http://[{}]:{}{}", self.host, self.port, self.path)
        } else {
            write!(f, "http://{}:{}{}", self.host, self.port, self.path)
        }
    }
}

enum Message {
    Span(Box<OpSpan>),
    Flush(oneshot::Sender<()>),
}

struct Shared {
    queue: mpsc::Sender<Message>,
    dropped: AtomicU64,
}

/// The node's handle on the exporter; a clone is cheap. Off, it holds
/// nothing, and every call returns at once without allocating.
#[derive(Clone, Default)]
pub(crate) struct Tracer(Option<Arc<Shared>>);

impl fmt::Debug for Tracer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.0.is_some() {
            "Tracer(on)"
        } else {
            "Tracer(off)"
        })
    }
}

impl Tracer {
    /// No exporter: nothing is made and nothing is sent.
    pub(crate) fn off() -> Tracer {
        Tracer(None)
    }

    /// Starts the exporter task for `node`'s spans to `endpoint`. Call it
    /// inside a tokio runtime.
    pub(crate) fn start(endpoint: OtlpEndpoint, node: NodeId) -> Tracer {
        let (queue, spans) = mpsc::channel(QUEUE);
        let shared = Arc::new(Shared {
            queue,
            dropped: AtomicU64::new(0),
        });
        tokio::spawn(run(endpoint, Arc::downgrade(&shared), node, spans));
        Tracer(Some(shared))
    }

    /// Whether spans are exported; a caller builds none when not.
    pub(crate) fn on(&self) -> bool {
        self.0.is_some()
    }

    /// Queues `span` without waiting; a full queue drops and counts it.
    pub(crate) fn emit(&self, span: OpSpan) {
        if let Some(shared) = &self.0
            && shared
                .queue
                .try_send(Message::Span(Box::new(span)))
                .is_err()
        {
            shared.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Spans dropped so far: a full queue, or a post that failed.
    #[cfg(test)]
    pub(crate) fn dropped(&self) -> u64 {
        self.0
            .as_ref()
            .map_or(0, |s| s.dropped.load(Ordering::Relaxed))
    }

    /// Posts what is queued, waiting `deadline` at most: at shutdown.
    pub(crate) async fn flush(&self, deadline: Duration) {
        let Some(shared) = &self.0 else {
            return;
        };
        let (done, wait) = oneshot::channel();
        let asked = tokio::time::timeout(deadline, async {
            if shared.queue.send(Message::Flush(done)).await.is_ok() {
                let _ = wait.await;
            }
        })
        .await;
        if asked.is_err() {
            tracing::warn!("spans not exported before the node stopped");
        }
    }
}

/// The exporter task: a batch goes when it holds `BATCH` spans, when
/// `EVERY` passed since its first, on a flush, and when every handle is
/// gone. A post that fails drops its batch: spans are never retried.
async fn run(
    endpoint: OtlpEndpoint,
    shared: std::sync::Weak<Shared>,
    node: NodeId,
    mut spans: mpsc::Receiver<Message>,
) {
    let mut batch: Vec<OpSpan> = Vec::new();
    let mut warned: Option<Instant> = None;
    let mut due = tokio::time::Instant::now() + EVERY;
    loop {
        let message = tokio::select! {
            m = spans.recv() => m,
            () = tokio::time::sleep_until(due), if !batch.is_empty() => {
                send(&endpoint, node, &mut batch, &shared, &mut warned).await;
                continue;
            }
        };
        match message {
            Some(Message::Span(span)) => {
                if batch.is_empty() {
                    due = tokio::time::Instant::now() + EVERY;
                }
                batch.push(*span);
                if batch.len() >= BATCH {
                    send(&endpoint, node, &mut batch, &shared, &mut warned).await;
                }
            }
            Some(Message::Flush(done)) => {
                send(&endpoint, node, &mut batch, &shared, &mut warned).await;
                let _ = done.send(());
            }
            None => {
                send(&endpoint, node, &mut batch, &shared, &mut warned).await;
                return;
            }
        }
    }
}

async fn send(
    endpoint: &OtlpEndpoint,
    node: NodeId,
    batch: &mut Vec<OpSpan>,
    shared: &std::sync::Weak<Shared>,
    warned: &mut Option<Instant>,
) {
    if batch.is_empty() {
        return;
    }
    let spans = std::mem::take(batch);
    let posted = match serde_json::to_vec(&body(node, &spans)) {
        Ok(bytes) => tokio::time::timeout(POST_DEADLINE, post(endpoint, &bytes))
            .await
            .unwrap_or(Err(Failure::Timeout)),
        Err(_) => Err(Failure::Encode),
    };
    if let Err(failure) = posted {
        let count = spans.len() as u64;
        if let Some(shared) = shared.upgrade() {
            shared.dropped.fetch_add(count, Ordering::Relaxed);
        }
        if warned.is_none_or(|at| at.elapsed() >= WARN_EVERY) {
            *warned = Some(Instant::now());
            tracing::warn!(
                spans = count,
                why = failure.name(),
                "dropped spans the OTLP endpoint did not take"
            );
        }
    }
}

/// Why a post did not deliver its batch, by name only.
#[derive(Debug, Clone, Copy)]
enum Failure {
    Encode,
    Connect,
    Write,
    Read,
    Timeout,
    Status(u16),
}

impl Failure {
    fn name(self) -> String {
        match self {
            Failure::Encode => "encode".into(),
            Failure::Connect => "connect".into(),
            Failure::Write => "write".into(),
            Failure::Read => "read".into(),
            Failure::Timeout => "timeout".into(),
            Failure::Status(code) => format!("status {code}"),
        }
    }
}

/// One `POST <path>` with `body` over a fresh connection, closed after
/// the answer; success is a 2xx status.
async fn post(endpoint: &OtlpEndpoint, body: &[u8]) -> Result<(), Failure> {
    let mut stream = TcpStream::connect((endpoint.host.as_str(), endpoint.port))
        .await
        .map_err(|_| Failure::Connect)?;
    let head = format!(
        "POST {} HTTP/1.1\r\nHost: {}:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nUser-Agent: fabricd/{}\r\nConnection: close\r\n\r\n",
        endpoint.path,
        endpoint.host,
        endpoint.port,
        body.len(),
        env!("CARGO_PKG_VERSION"),
    );
    stream
        .write_all(head.as_bytes())
        .await
        .map_err(|_| Failure::Write)?;
    stream.write_all(body).await.map_err(|_| Failure::Write)?;
    // The status line is all this reads; `HTTP/1.1 200 OK` fits in 16 bytes.
    let mut line = [0_u8; 16];
    let mut read = 0;
    while read < 12 {
        let n = stream
            .read(&mut line[read..])
            .await
            .map_err(|_| Failure::Read)?;
        if n == 0 {
            return Err(Failure::Read);
        }
        read += n;
    }
    let status = std::str::from_utf8(&line[9..12])
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or(Failure::Read)?;
    if line.starts_with(b"HTTP/1.") && (200..300).contains(&status) {
        Ok(())
    } else {
        Err(Failure::Status(status))
    }
}

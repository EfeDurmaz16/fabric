//! A reader with a limit on silence, and a writer with a limit on stalls.
//!
//! Invariant: a read fails with `TimedOut` once `limit` passes with no byte
//! arriving, and every byte that arrives restarts the limit. A write fails
//! the same way once it has waited `limit` with no byte taken. So a slow
//! link that keeps moving is never cut, and a silent one always is.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::time::{Instant, Sleep};

/// A reader that fails with `TimedOut` once `limit` passes with no byte arriving.
pub(crate) struct IdleReader<R> {
    inner: R,
    limit: Duration,
    timer: Pin<Box<Sleep>>,
}

impl<R> IdleReader<R> {
    /// Wraps `inner`; the limit starts now.
    pub(crate) fn new(inner: R, limit: Duration) -> Self {
        Self {
            inner,
            limit,
            timer: Box::pin(tokio::time::sleep(limit)),
        }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for IdleReader<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        match Pin::new(&mut self.inner).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => {
                if buf.filled().len() > before {
                    let next = Instant::now() + self.limit;
                    self.timer.as_mut().reset(next);
                }
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
            Poll::Pending => match self.timer.as_mut().poll(cx) {
                Poll::Ready(()) => Poll::Ready(Err(io::ErrorKind::TimedOut.into())),
                Poll::Pending => Poll::Pending,
            },
        }
    }
}

/// A writer that fails with `TimedOut` once a write, flush or shutdown has
/// waited `limit` with no progress.
///
/// Only time spent waiting counts: the limit starts when a call first
/// returns `Pending` and ends at the next call that makes progress, so the
/// time between writes, while the caller works on what to send next, is
/// never charged to the peer.
pub(crate) struct StallWriter<W> {
    inner: W,
    limit: Duration,
    stalled: Option<Pin<Box<Sleep>>>,
}

impl<W> StallWriter<W> {
    /// Wraps `inner`; no limit runs until a call waits.
    pub(crate) fn new(inner: W, limit: Duration) -> Self {
        Self {
            inner,
            limit,
            stalled: None,
        }
    }

    /// Passes a ready result through and clears the stall; on `Pending`,
    /// starts the limit if it is not running and fails once it ends.
    fn settle<T>(
        &mut self,
        cx: &mut Context<'_>,
        polled: Poll<io::Result<T>>,
    ) -> Poll<io::Result<T>> {
        match polled {
            Poll::Ready(result) => {
                self.stalled = None;
                Poll::Ready(result)
            }
            Poll::Pending => {
                let limit = self.limit;
                let timer = self
                    .stalled
                    .get_or_insert_with(|| Box::pin(tokio::time::sleep(limit)));
                match timer.as_mut().poll(cx) {
                    Poll::Ready(()) => Poll::Ready(Err(io::ErrorKind::TimedOut.into())),
                    Poll::Pending => Poll::Pending,
                }
            }
        }
    }
}

impl<W: AsyncWrite + Unpin> AsyncWrite for StallWriter<W> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let polled = Pin::new(&mut self.inner).poll_write(cx, data);
        self.settle(cx, polled)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let polled = Pin::new(&mut self.inner).poll_flush(cx);
        self.settle(cx, polled)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let polled = Pin::new(&mut self.inner).poll_shutdown(cx);
        self.settle(cx, polled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    const LIMIT: Duration = Duration::from_secs(1);

    /// A write that waits past the limit fails; time spent between writes
    /// is not counted, and a writer that keeps taking bytes is never cut.
    #[tokio::test(start_paused = true)]
    async fn a_write_fails_only_after_a_stall_of_the_limit() {
        let (near, mut far) = tokio::io::duplex(4);
        let mut writer = StallWriter::new(near, LIMIT);
        writer.write_all(b"1234").await.unwrap();
        tokio::time::sleep(LIMIT * 3).await;
        let drain = tokio::spawn(async move {
            let mut byte = [0u8; 1];
            for _ in 0..12 {
                tokio::time::sleep(LIMIT / 2).await;
                tokio::io::AsyncReadExt::read_exact(&mut far, &mut byte)
                    .await
                    .unwrap();
            }
            far
        });
        writer.write_all(b"abcdefgh").await.unwrap();
        let _far = drain.await.unwrap();
        let started = tokio::time::Instant::now();
        let stalled = writer.write_all(b"xxxxxxxx").await;
        assert_eq!(stalled.unwrap_err().kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() >= LIMIT);
    }
}

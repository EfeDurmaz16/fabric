//! A reader with a limit on silence.
//!
//! Invariant: a read fails with `TimedOut` once `limit` passes with no byte
//! arriving, and every byte that arrives restarts the limit. So a slow link
//! that keeps moving is never cut, and a silent one always is.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, ReadBuf};
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

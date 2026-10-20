//! An in-memory byte pipe with a capacity (used by the TCP multiplexer).
//!
//! Invariant: a writer that outruns the reader waits instead of growing
//! memory; dropping the writer gives the reader a clean end of stream after
//! the bytes already written, and aborting it gives the reader an error
//! instead, so a stream cut off by a lost connection never reads as one that
//! ended; dropping the reader makes every later write fail with
//! `BrokenPipe`, so a stream nobody reads cannot fill the heap. A `push`
//! puts all its bytes in or none, and `Drained` counts what the reader took,
//! which is the credit the multiplexer hands back to the peer, and reports
//! the reader's drop at once, so the multiplexer can stop the peer's sender
//! without waiting for it to send again.

use std::collections::VecDeque;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// Where the write end is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Writer {
    /// It may still write.
    Open,
    /// It finished: the reader sees end of stream after the buffered bytes.
    Finished,
    /// It aborted: the reader's next read fails.
    Aborted,
}

struct State {
    buf: VecDeque<u8>,
    capacity: usize,
    writer: Writer,
    reader_gone: bool,
    /// Bytes the reader took that `Drained` has not reported yet.
    taken: usize,
    read_waker: Option<Waker>,
    write_waker: Option<Waker>,
    drained_waker: Option<Waker>,
}

impl State {
    /// Records `n` bytes taken by the reader and wakes whoever waits for room or for credit.
    fn took(&mut self, n: usize) {
        self.taken += n;
        if let Some(waker) = self.write_waker.take() {
            waker.wake();
        }
        if let Some(waker) = self.drained_waker.take() {
            waker.wake();
        }
    }
}

/// The read end of a pipe.
pub(crate) struct PipeReader(Arc<Mutex<State>>);

/// The write end of a pipe.
pub(crate) struct PipeWriter(Arc<Mutex<State>>);

/// Makes a pipe that buffers at most `capacity` bytes.
pub(crate) fn pipe(capacity: usize) -> (PipeWriter, PipeReader) {
    let state = Arc::new(Mutex::new(State {
        buf: VecDeque::new(),
        capacity,
        writer: Writer::Open,
        reader_gone: false,
        taken: 0,
        read_waker: None,
        write_waker: None,
        drained_waker: None,
    }));
    (PipeWriter(state.clone()), PipeReader(state))
}

fn lock(state: &Mutex<State>) -> std::sync::MutexGuard<'_, State> {
    // The guarded data is plain bytes and flags; a poisoned lock holds a
    // consistent value, so the pipe keeps working after a panic elsewhere.
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn lost() -> io::Error {
    io::Error::new(io::ErrorKind::ConnectionAborted, "the connection was lost")
}

impl PipeReader {
    /// Takes up to `max` buffered bytes, waiting for some; an empty result is the end of the stream.
    pub(crate) fn poll_take(&self, cx: &mut Context<'_>, max: usize) -> Poll<io::Result<Vec<u8>>> {
        let mut state = lock(&self.0);
        if state.writer == Writer::Aborted {
            return Poll::Ready(Err(lost()));
        }
        if state.buf.is_empty() {
            if state.writer == Writer::Finished {
                return Poll::Ready(Ok(Vec::new()));
            }
            state.read_waker = Some(cx.waker().clone());
            return Poll::Pending;
        }
        let n = max.min(state.buf.len());
        let taken: Vec<u8> = state.buf.drain(..n).collect();
        state.took(n);
        Poll::Ready(Ok(taken))
    }
}

impl AsyncRead for PipeReader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let mut state = lock(&self.0);
        if state.writer == Writer::Aborted {
            return Poll::Ready(Err(lost()));
        }
        if state.buf.is_empty() {
            if state.writer == Writer::Finished {
                return Poll::Ready(Ok(()));
            }
            state.read_waker = Some(cx.waker().clone());
            return Poll::Pending;
        }
        let n = buf.remaining().min(state.buf.len());
        let (front, back) = state.buf.as_slices();
        let from_front = n.min(front.len());
        buf.put_slice(&front[..from_front]);
        buf.put_slice(&back[..n - from_front]);
        state.buf.drain(..n);
        state.took(n);
        Poll::Ready(Ok(()))
    }
}

impl Drop for PipeReader {
    fn drop(&mut self) {
        let mut state = lock(&self.0);
        state.reader_gone = true;
        state.buf.clear();
        if let Some(waker) = state.write_waker.take() {
            waker.wake();
        }
        if let Some(waker) = state.drained_waker.take() {
            waker.wake();
        }
    }
}

/// Why `push` put nothing in the pipe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refused {
    /// The bytes do not fit in the room left.
    Full,
    /// Nobody will read them.
    ReaderGone,
}

/// What `Drained` saw the reader of one pipe do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Report {
    /// The reader took this many bytes since the last report.
    Took(usize),
    /// The writer finished or aborted, so no more bytes will come.
    Ended,
    /// The reader was dropped while the writer was still open.
    ReaderGone,
}

/// Counts the bytes the reader of one pipe takes.
pub(crate) struct Drained(Arc<Mutex<State>>);

impl Drained {
    /// Waits until the reader took at least `at_least` bytes since the last
    /// report, the writer ended, or the reader is gone, and says which.
    pub(crate) fn poll_report(&self, cx: &mut Context<'_>, at_least: usize) -> Poll<Report> {
        let mut state = lock(&self.0);
        if state.writer != Writer::Open {
            return Poll::Ready(Report::Ended);
        }
        if state.reader_gone {
            return Poll::Ready(Report::ReaderGone);
        }
        if state.taken >= at_least {
            return Poll::Ready(Report::Took(std::mem::take(&mut state.taken)));
        }
        state.drained_waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

impl PipeWriter {
    /// Puts all of `data` in the pipe, or none of it when it does not fit or nobody reads.
    pub(crate) fn push(&self, data: &[u8]) -> Result<(), Refused> {
        let mut state = lock(&self.0);
        if state.reader_gone {
            return Err(Refused::ReaderGone);
        }
        if data.len() > state.capacity.saturating_sub(state.buf.len()) {
            return Err(Refused::Full);
        }
        state.buf.extend(data);
        if let Some(waker) = state.read_waker.take() {
            waker.wake();
        }
        Ok(())
    }

    /// Returns a handle that reports what the reader takes.
    pub(crate) fn drained(&self) -> Drained {
        Drained(Arc::clone(&self.0))
    }

    /// Ends the pipe with an error: the reader's next read fails, buffered bytes or not.
    pub(crate) fn abort(self) {
        let mut state = lock(&self.0);
        state.writer = Writer::Aborted;
        state.buf.clear();
        if let Some(waker) = state.read_waker.take() {
            waker.wake();
        }
        if let Some(waker) = state.drained_waker.take() {
            waker.wake();
        }
    }
}

impl AsyncWrite for PipeWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let mut state = lock(&self.0);
        if state.reader_gone {
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        }
        if data.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let room = state.capacity.saturating_sub(state.buf.len());
        if room == 0 {
            state.write_waker = Some(cx.waker().clone());
            return Poll::Pending;
        }
        let n = room.min(data.len());
        state.buf.extend(&data[..n]);
        if let Some(waker) = state.read_waker.take() {
            waker.wake();
        }
        Poll::Ready(Ok(n))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        finish(&self.0);
        Poll::Ready(Ok(()))
    }
}

/// Ends an open pipe cleanly; an aborted one stays aborted.
fn finish(state: &Mutex<State>) {
    let mut state = lock(state);
    if state.writer == Writer::Open {
        state.writer = Writer::Finished;
    }
    if let Some(waker) = state.read_waker.take() {
        waker.wake();
    }
    if let Some(waker) = state.drained_waker.take() {
        waker.wake();
    }
}

impl Drop for PipeWriter {
    fn drop(&mut self) {
        finish(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn a_full_pipe_makes_the_writer_wait_for_the_reader() {
        let (mut w, mut r) = pipe(4);
        w.write_all(b"abcd").await.unwrap();
        let writer = tokio::spawn(async move {
            w.write_all(b"efgh").await.unwrap();
            w.shutdown().await.unwrap();
        });
        tokio::task::yield_now().await;
        assert!(!writer.is_finished());
        let mut out = Vec::new();
        r.read_to_end(&mut out).await.unwrap();
        assert_eq!(out, b"abcdefgh");
        writer.await.unwrap();
    }

    #[tokio::test]
    async fn dropping_the_writer_ends_the_stream_and_dropping_the_reader_breaks_the_pipe() {
        let (mut w, mut r) = pipe(16);
        w.write_all(b"xy").await.unwrap();
        drop(w);
        let mut out = Vec::new();
        r.read_to_end(&mut out).await.unwrap();
        assert_eq!(out, b"xy");

        let (mut w, r) = pipe(16);
        drop(r);
        assert_eq!(
            w.write_all(b"z").await.unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
    }

    #[tokio::test]
    async fn an_aborted_pipe_fails_the_reader_instead_of_ending_it() {
        let (mut w, mut r) = pipe(16);
        w.write_all(b"xy").await.unwrap();
        w.abort();
        let mut out = Vec::new();
        assert_eq!(
            r.read_to_end(&mut out).await.unwrap_err().kind(),
            io::ErrorKind::ConnectionAborted
        );
    }
}

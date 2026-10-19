//! An in-memory byte pipe with a capacity (used by the TCP multiplexer).
//!
//! Invariant: a writer that outruns the reader waits instead of growing
//! memory; dropping the writer gives the reader a clean end of stream after
//! the bytes already written; dropping the reader makes every later write
//! fail with `BrokenPipe`, so a stream nobody reads cannot fill the heap.

use std::collections::VecDeque;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

struct State {
    buf: VecDeque<u8>,
    capacity: usize,
    writer_gone: bool,
    reader_gone: bool,
    read_waker: Option<Waker>,
    write_waker: Option<Waker>,
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
        writer_gone: false,
        reader_gone: false,
        read_waker: None,
        write_waker: None,
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

impl AsyncRead for PipeReader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let mut state = lock(&self.0);
        if state.buf.is_empty() {
            if state.writer_gone {
                return Poll::Ready(Ok(()));
            }
            state.read_waker = Some(cx.waker().clone());
            return Poll::Pending;
        }
        let n = buf.remaining().min(state.buf.len());
        for byte in state.buf.drain(..n) {
            buf.put_slice(&[byte]);
        }
        if let Some(waker) = state.write_waker.take() {
            waker.wake();
        }
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
        let mut state = lock(&self.0);
        state.writer_gone = true;
        if let Some(waker) = state.read_waker.take() {
            waker.wake();
        }
        Poll::Ready(Ok(()))
    }
}

impl Drop for PipeWriter {
    fn drop(&mut self) {
        let mut state = lock(&self.0);
        state.writer_gone = true;
        if let Some(waker) = state.read_waker.take() {
            waker.wake();
        }
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
}

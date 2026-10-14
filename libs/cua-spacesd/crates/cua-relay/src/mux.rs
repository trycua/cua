// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Drives one yamux connection on a task and hands out streams.

use std::collections::VecDeque;
use std::future::poll_fn;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};

use futures_util::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, oneshot};

/// Largest number of concurrent streams on one machine connection (the
/// yamux-level cap; [`crate::server::RelayConfig::max_streams_per_machine`]
/// is the application-level one checked per request, normally lower). Safe
/// default, down from the earlier 1024: kept low deliberately (S7) since
/// [`DEFAULT_WINDOW_BYTES`] must grow with it (yamux requires the
/// connection window to be at least 256 KiB per stream).
pub const MAX_STREAMS: usize = 256;

/// Default per-machine yamux connection receive window: the most one
/// machine's connection buffers across all of its streams at once (S7).
/// Down from an earlier 1 GiB, which let a single machine with enough slow
/// readers exhaust the relay pod's memory on its own; this is deliberately
/// just [`MAX_STREAMS`] `* 256 KiB` (yamux's per-stream default), the
/// smallest value yamux accepts for that stream count, so widen
/// [`MAX_STREAMS`] and this together if you need bigger per-stream
/// throughput.
pub const DEFAULT_WINDOW_BYTES: usize = MAX_STREAMS * 256 * 1024;

/// yamux configuration shared by both ends. `max_streams` and
/// `window_bytes` are the per-machine caps (S7); `window_bytes` must be at
/// least `max_streams * 256 KiB` (yamux's own per-stream default window) or
/// this panics, the way `yamux::Config` itself asserts.
pub fn config(max_streams: usize, window_bytes: usize) -> yamux::Config {
    let mut config = yamux::Config::default();
    config.set_max_num_streams(max_streams);
    config.set_max_connection_receive_window(Some(window_bytes));
    config
}

type OpenReply = oneshot::Sender<Result<yamux::Stream, yamux::ConnectionError>>;

/// Opens outbound streams on a running connection.
#[derive(Clone)]
pub struct MuxHandle {
    open: mpsc::Sender<OpenReply>,
}

impl MuxHandle {
    /// Opens a stream. Fails once the connection is gone.
    pub async fn open(&self) -> std::io::Result<yamux::Stream> {
        let (tx, rx) = oneshot::channel();
        self.open
            .send(tx)
            .await
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::NotConnected, "tunnel closed"))?;
        rx.await
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::NotConnected, "tunnel closed"))?
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::ConnectionAborted, e.to_string()))
    }

    /// True once the driver task has stopped.
    pub fn is_closed(&self) -> bool {
        self.open.is_closed()
    }
}

/// Runs `io` as a yamux connection until it ends. Inbound streams go to
/// `inbound` (dropped, which resets them, if the receiver is full).
pub fn spawn<T>(
    io: T,
    mode: yamux::Mode,
    inbound: mpsc::Sender<yamux::Stream>,
    max_streams: usize,
    window_bytes: usize,
) -> (
    MuxHandle,
    tokio::task::JoinHandle<Result<(), yamux::ConnectionError>>,
)
where
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (open_tx, mut open_rx) = mpsc::channel::<OpenReply>(256);
    let mut connection = yamux::Connection::new(io, config(max_streams, window_bytes), mode);
    let task = tokio::spawn(async move {
        let mut pending: VecDeque<OpenReply> = VecDeque::new();
        let result = poll_fn(|cx: &mut Context<'_>| {
            while let Poll::Ready(Some(reply)) = open_rx.poll_recv(cx) {
                pending.push_back(reply);
            }
            while !pending.is_empty() {
                match connection.poll_new_outbound(cx) {
                    Poll::Ready(result) => {
                        let reply = pending.pop_front().expect("non-empty");
                        let failed = result.is_err();
                        let _ = reply.send(result);
                        if failed {
                            break;
                        }
                    }
                    Poll::Pending => break,
                }
            }
            loop {
                match connection.poll_next_inbound(cx) {
                    Poll::Ready(Some(Ok(stream))) => {
                        let _ = inbound.try_send(stream);
                    }
                    Poll::Ready(Some(Err(e))) => return Poll::Ready(Err(e)),
                    Poll::Ready(None) => return Poll::Ready(Ok(())),
                    Poll::Pending => return Poll::Pending,
                }
            }
        })
        .await;
        for reply in pending.drain(..) {
            let _ = reply.send(Err(yamux::ConnectionError::Closed));
        }
        result
    });
    (MuxHandle { open: open_tx }, task)
}

/// Byte counters for a machine.
#[derive(Debug, Default)]
pub struct Counters {
    /// Bytes from clients to the machine.
    pub to_machine: AtomicU64,
    /// Bytes from the machine to clients.
    pub from_machine: AtomicU64,
}

/// A stream wrapper that counts bytes into [`Counters`].
pub struct Counted<S> {
    inner: S,
    counters: Arc<Counters>,
}

impl<S> Counted<S> {
    /// Wraps `inner`.
    pub fn new(inner: S, counters: Arc<Counters>) -> Self {
        Self { inner, counters }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Counted<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        let result = Pin::new(&mut self.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(n)) = &result {
            self.counters
                .from_machine
                .fetch_add(*n as u64, Ordering::Relaxed);
        }
        result
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Counted<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let result = Pin::new(&mut self.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(n)) = &result {
            self.counters
                .to_machine
                .fetch_add(*n as u64, Ordering::Relaxed);
        }
        result
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_close(cx)
    }
}

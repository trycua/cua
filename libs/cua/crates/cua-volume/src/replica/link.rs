// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! One framed connection to a peer, with request/reply correlation.
//!
//! A [`Link`] owns a writer task and a reader task over any duplex stream
//! (a mesh stream in the product, TCP in tests, an in-memory pipe in
//! simulations). Messages that answer a request (`re` set) go to the waiting
//! [`Link::call`]; everything else goes to the handler, each on its own task
//! so a slow handler never blocks the reader (and so a heartbeat is always
//! read).

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::sync::{Notify, mpsc, oneshot};

use super::MachineId;
use super::wire::{self, Chan, Frame, Msg};

/// Any duplex byte stream.
pub trait Duplex: AsyncRead + AsyncWrite + Send + Unpin + 'static {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin + 'static> Duplex for T {}

/// A boxed stream.
pub type Stream = Box<dyn Duplex>;

/// Bytes crossing links (frames, as sent and received).
#[derive(Debug, Default)]
pub struct Counters {
    pub sent: AtomicU64,
    pub received: AtomicU64,
}

/// What a handler receives.
pub type Handler = Arc<
    dyn Fn(Arc<Link>, Frame, Vec<u8>) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync,
>;

/// What the writer task is asked to do.
enum Out {
    Bytes(Vec<u8>),
    /// Fires once everything queued before it has been written.
    Flush(oneshot::Sender<()>),
}

type Waiter = oneshot::Sender<(Frame, Vec<u8>)>;

/// One connection.
pub struct Link {
    pub peer: MachineId,
    pub peer_name: String,
    pub chan: Chan,
    /// This side dialed.
    pub initiated: bool,
    /// When the connection came up.
    pub created: std::time::Instant,
    out: mpsc::UnboundedSender<Out>,
    calls: Mutex<HashMap<u64, Waiter>>,
    next: AtomicU64,
    closed: AtomicBool,
    closed_notify: Notify,
    counters: Arc<Counters>,
    /// Peer facts from its Hello (the node reads them).
    pub hello: Mutex<Option<wire::Hello>>,
    tasks: Mutex<Vec<tokio::task::AbortHandle>>,
}

impl std::fmt::Debug for Link {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Link")
            .field("peer", &self.peer)
            .field("chan", &self.chan)
            .field("closed", &self.is_closed())
            .finish()
    }
}

/// Why a call failed.
#[derive(Debug)]
pub enum CallError {
    Closed,
    Timeout,
    Io(std::io::Error),
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallError::Closed => f.write_str("the link to the machine closed"),
            CallError::Timeout => f.write_str("the machine did not answer in time"),
            CallError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl Link {
    /// Starts the reader and writer tasks over `stream`.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn(
        stream: Stream,
        peer: MachineId,
        peer_name: String,
        chan: Chan,
        initiated: bool,
        counters: Arc<Counters>,
        handler: Handler,
        on_close: impl FnOnce(Arc<Link>) + Send + 'static,
    ) -> Arc<Link> {
        let (mut rd, mut wr) = tokio::io::split(stream);
        let (tx, mut rx) = mpsc::unbounded_channel::<Out>();
        let link = Arc::new(Link {
            peer,
            peer_name,
            chan,
            initiated,
            created: std::time::Instant::now(),
            out: tx,
            calls: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
            closed: AtomicBool::new(false),
            closed_notify: Notify::new(),
            counters: counters.clone(),
            hello: Mutex::new(None),
            tasks: Mutex::new(vec![]),
        });
        let l = link.clone();
        let c = counters.clone();
        let wtask = tokio::spawn(async move {
            while let Some(out) = rx.recv().await {
                match out {
                    Out::Bytes(bytes) => {
                        c.sent.fetch_add(bytes.len() as u64, Ordering::Relaxed);
                        if wr.write_all(&bytes).await.is_err() || wr.flush().await.is_err() {
                            break;
                        }
                    }
                    Out::Flush(done) => {
                        let _ = done.send(());
                    }
                }
            }
            let _ = wr.shutdown().await;
            l.close();
        });
        let l = link.clone();
        let rtask = tokio::spawn(async move {
            #[allow(clippy::while_let_loop)]
            loop {
                match wire::read(&mut rd).await {
                    Ok(Some((frame, data))) => {
                        l.counters
                            .received
                            .fetch_add((data.len() + 64) as u64, Ordering::Relaxed);
                        if let Some(re) = frame.re {
                            let waiter = l.calls.lock().expect("calls").remove(&re);
                            if let Some(w) = waiter {
                                let _ = w.send((frame, data));
                            }
                        } else {
                            let h = handler.clone();
                            let l2 = l.clone();
                            tokio::spawn(async move { h(l2, frame, data).await });
                        }
                    }
                    Ok(None) | Err(_) => break,
                }
            }
            l.close();
            on_close(l);
        });
        link.tasks
            .lock()
            .expect("tasks")
            .extend([wtask.abort_handle(), rtask.abort_handle()]);
        link
    }

    /// Waits (briefly) until what was sent has been handed to the stream.
    pub async fn flush(&self, within: Duration) {
        let (tx, rx) = oneshot::channel();
        if self.out.send(Out::Flush(tx)).is_ok() {
            let _ = tokio::time::timeout(within, rx).await;
        }
    }

    /// Marks the link closed and fails every waiting call.
    pub fn close(&self) {
        if !self.closed.swap(true, Ordering::SeqCst) {
            self.calls.lock().expect("calls").clear();
            self.closed_notify.notify_waiters();
            // A dead connection must not keep its tasks (and so its socket)
            // alive: the peer sees the end of the stream and redials.
            for t in self.tasks.lock().expect("tasks").drain(..) {
                t.abort();
            }
        }
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Resolves when the link closes.
    pub async fn closed(&self) {
        loop {
            let n = self.closed_notify.notified();
            if self.is_closed() {
                return;
            }
            n.await;
        }
    }

    /// Sends a message with no reply expected.
    pub fn send(&self, msg: Msg, data: &[u8]) -> bool {
        self.send_frame(
            Frame {
                id: self.next.fetch_add(1, Ordering::Relaxed),
                re: None,
                msg,
            },
            data,
        )
    }

    /// Answers request `id`.
    pub fn reply(&self, id: u64, msg: Msg, data: &[u8]) -> bool {
        self.send_frame(
            Frame {
                id: self.next.fetch_add(1, Ordering::Relaxed),
                re: Some(id),
                msg,
            },
            data,
        )
    }

    fn send_frame(&self, f: Frame, data: &[u8]) -> bool {
        if self.is_closed() {
            return false;
        }
        match wire::encode(&f, data) {
            Ok(b) => self.out.send(Out::Bytes(b)).is_ok(),
            Err(_) => false,
        }
    }

    /// Like [`Link::call`], and a timeout closes the link: a bulk request
    /// that gets no answer means the path is dead (a half-open connection
    /// looks exactly like a slow one until something gives up on it), and the
    /// next attempt must dial afresh.
    pub async fn call_strict(
        &self,
        msg: Msg,
        data: &[u8],
        timeout: Duration,
    ) -> Result<(Msg, Vec<u8>), CallError> {
        let r = self.call(msg, data, timeout).await;
        if matches!(r, Err(CallError::Timeout)) {
            self.close();
        }
        r
    }

    /// Sends a request and waits for its reply.
    pub async fn call(
        &self,
        msg: Msg,
        data: &[u8],
        timeout: Duration,
    ) -> Result<(Msg, Vec<u8>), CallError> {
        if self.is_closed() {
            return Err(CallError::Closed);
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.calls.lock().expect("calls").insert(id, tx);
        if !self.send_frame(Frame { id, re: None, msg }, data) {
            self.calls.lock().expect("calls").remove(&id);
            return Err(CallError::Closed);
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok((f, d))) => Ok((f.msg, d)),
            Ok(Err(_)) => Err(CallError::Closed),
            Err(_) => {
                self.calls.lock().expect("calls").remove(&id);
                Err(CallError::Timeout)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replica::wire::Rep;

    fn echo() -> Handler {
        Arc::new(|link, frame, data| {
            Box::pin(async move {
                if let Msg::Req(_) = frame.msg {
                    link.reply(frame.id, Msg::Rep(Rep::Range), &data);
                }
            })
        })
    }

    #[tokio::test]
    async fn calls_get_their_replies_and_a_closed_link_fails_them() {
        let (a, b) = tokio::io::duplex(1 << 16);
        let ca = Arc::new(Counters::default());
        let la = Link::spawn(
            Box::new(a),
            "b".into(),
            "B".into(),
            Chan::Control,
            true,
            ca.clone(),
            echo(),
            |_| {},
        );
        let lb = Link::spawn(
            Box::new(b),
            "a".into(),
            "A".into(),
            Chan::Control,
            false,
            Arc::new(Counters::default()),
            echo(),
            |_| {},
        );
        let (m, d) = la
            .call(
                Msg::Req(wire::Req::Head { key: "k".into() }),
                b"hi",
                Duration::from_secs(2),
            )
            .await
            .unwrap();
        assert_eq!(m, Msg::Rep(Rep::Range));
        assert_eq!(d, b"hi");
        assert!(ca.sent.load(Ordering::Relaxed) > 0);
        lb.close();
        drop(lb);
        // The other side's writer ends when its handle is gone; the call fails.
        let r = la
            .call(
                Msg::Req(wire::Req::Head { key: "k".into() }),
                b"",
                Duration::from_millis(300),
            )
            .await;
        assert!(r.is_err() || la.is_closed());
    }
}

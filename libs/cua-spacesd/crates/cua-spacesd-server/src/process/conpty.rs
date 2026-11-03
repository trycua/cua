// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! PTY processes on Windows, over ConPTY (via `portable-pty`, MIT).
//!
//! ConPTY hands out blocking pipe handles, so one thread pumps output and one
//! pumps input, each through a bounded channel. The pseudo console is closed
//! once the child exits ([`PtyMaster::close`]); that is what ends the output
//! pipe, since ConPTY keeps it open after the child is gone.
//!
//! Compile-checked with `cargo xwin check --target x86_64-pc-windows-msvc`.
//! Not yet exercised on a Windows host.

use std::io::{self, Read, Write};
use std::pin::Pin;
use std::process::ExitStatus;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::PollSender;

use super::spawn::SpawnSpec;

/// Largest chunk moved through either pipe at once.
const CHUNK: usize = 64 * 1024;
/// Chunks queued per direction (bounds memory at `QUEUE * CHUNK`).
const QUEUE: usize = 16;

fn other(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

/// Keeps the `io::ErrorKind` (e.g. `NotFound`) when portable-pty wraps one.
fn from_anyhow(error: anyhow::Error) -> io::Error {
    match error.downcast::<io::Error>() {
        Ok(error) => error,
        Err(error) => other(error),
    }
}

fn pty_size(cols: u16, rows: u16, px_w: u16, px_h: u16) -> PtySize {
    PtySize {
        rows: rows.max(1),
        cols: cols.max(1),
        pixel_width: px_w,
        pixel_height: px_h,
    }
}

/// A child running under ConPTY.
pub struct ConPtyChild {
    killer: Box<dyn ChildKiller + Send + Sync>,
    exit: Option<oneshot::Receiver<io::Result<ExitStatus>>>,
}

impl ConPtyChild {
    /// Waits for exit. Cancel-safe: dropping the future keeps the result.
    pub async fn wait(&mut self) -> io::Result<ExitStatus> {
        let Some(exit) = self.exit.as_mut() else {
            return Err(other("process was already waited for"));
        };
        let result = exit
            .await
            .unwrap_or_else(|_| Err(other("ConPTY wait thread ended")));
        self.exit = None;
        result
    }

    /// Terminates the process (`TerminateProcess`).
    pub fn start_kill(&mut self) -> io::Result<()> {
        self.killer.kill()
    }
}

struct PendingOutput {
    rx: mpsc::Receiver<Vec<u8>>,
    chunk: Vec<u8>,
    pos: usize,
}

struct Inner {
    /// `None` once closed; dropping the last handle closes the pseudo console.
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    output: Mutex<PendingOutput>,
    input: Mutex<PollSender<Vec<u8>>>,
}

/// The ConPTY side shared by the reader, the writer and resize.
#[derive(Clone)]
pub struct PtyMaster {
    inner: Arc<Inner>,
}

impl PtyMaster {
    /// Sets the console size.
    pub fn resize(&self, cols: u16, rows: u16, px_w: u16, px_h: u16) -> io::Result<()> {
        let master = self.inner.master.lock().expect("pty master lock");
        let Some(master) = master.as_ref() else {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "the pseudo console is closed",
            ));
        };
        master
            .resize(pty_size(cols, rows, px_w, px_h))
            .map_err(from_anyhow)
    }

    /// Closes the pseudo console so the output pipe reaches EOF. May block
    /// while ConPTY flushes; call it off the async runtime.
    pub fn close(&self) {
        let master = self.inner.master.lock().expect("pty master lock").take();
        drop(master);
    }
}

impl AsyncRead for PtyMaster {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let mut pending = self.inner.output.lock().expect("pty output lock");
        loop {
            if pending.pos < pending.chunk.len() {
                let n = buf.remaining().min(pending.chunk.len() - pending.pos);
                let start = pending.pos;
                buf.put_slice(&pending.chunk[start..start + n]);
                pending.pos += n;
                return Poll::Ready(Ok(()));
            }
            match pending.rx.poll_recv(cx) {
                Poll::Ready(Some(chunk)) => {
                    pending.chunk = chunk;
                    pending.pos = 0;
                }
                // Reader thread ended: EOF.
                Poll::Ready(None) => return Poll::Ready(Ok(())),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl AsyncWrite for PtyMaster {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        if data.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let mut input = self.inner.input.lock().expect("pty input lock");
        match input.poll_reserve(cx) {
            Poll::Ready(Ok(())) => {}
            Poll::Ready(Err(_)) => return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into())),
            Poll::Pending => return Poll::Pending,
        }
        let n = data.len().min(CHUNK);
        input
            .send_item(data[..n].to_vec())
            .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
        Poll::Ready(Ok(n))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

fn read_loop(mut reader: Box<dyn Read + Send>, tx: mpsc::Sender<Vec<u8>>) {
    let mut buf = vec![0u8; CHUNK];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => return,
            Ok(n) => {
                if tx.blocking_send(buf[..n].to_vec()).is_err() {
                    return;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return,
        }
    }
}

fn write_loop(mut writer: Box<dyn Write + Send>, mut rx: mpsc::Receiver<Vec<u8>>) {
    while let Some(chunk) = rx.blocking_recv() {
        if writer
            .write_all(&chunk)
            .and_then(|()| writer.flush())
            .is_err()
        {
            return;
        }
    }
}

fn thread(name: &str, body: impl FnOnce() + Send + 'static) -> io::Result<()> {
    std::thread::Builder::new()
        .name(name.into())
        .spawn(body)
        .map(drop)
}

/// Spawns `spec` under a new pseudo console. Running as another user is not
/// supported (see `lookup_user`), so `spec.switch_user` is ignored.
pub fn spawn(spec: &SpawnSpec) -> io::Result<(ConPtyChild, u32, PtyMaster)> {
    use std::os::windows::process::ExitStatusExt as _;

    let (cols, rows, px_w, px_h) = spec.pty.unwrap_or((80, 24, 0, 0));
    let pair = portable_pty::native_pty_system()
        .openpty(pty_size(cols, rows, px_w, px_h))
        .map_err(from_anyhow)?;
    let reader = pair.master.try_clone_reader().map_err(from_anyhow)?;
    let writer = pair.master.take_writer().map_err(from_anyhow)?;
    let (out_tx, out_rx) = mpsc::channel(QUEUE);
    let (in_tx, in_rx) = mpsc::channel(QUEUE);
    thread("cua-conpty-out", move || read_loop(reader, out_tx))?;
    thread("cua-conpty-in", move || write_loop(writer, in_rx))?;

    let mut command = CommandBuilder::new(&spec.command);
    command.args(&spec.args);
    command.env_clear();
    for (key, value) in &spec.env {
        command.env(key, value);
    }
    command.cwd(&spec.cwd);
    let mut child = pair.slave.spawn_command(command).map_err(from_anyhow)?;
    // Only the master keeps the pseudo console alive from here on.
    drop(pair.slave);
    let pid = child.process_id().unwrap_or(0);
    let mut killer = child.clone_killer();

    let (exit_tx, exit_rx) = oneshot::channel();
    let waited = thread("cua-conpty-wait", move || {
        let status = child
            .wait()
            .map(|status| ExitStatus::from_raw(status.exit_code()));
        let _ = exit_tx.send(status);
    });
    if let Err(error) = waited {
        let _ = killer.kill();
        return Err(error);
    }
    let master = PtyMaster {
        inner: Arc::new(Inner {
            master: Mutex::new(Some(pair.master)),
            output: Mutex::new(PendingOutput {
                rx: out_rx,
                chunk: Vec::new(),
                pos: 0,
            }),
            input: Mutex::new(PollSender::new(in_tx)),
        }),
    };
    Ok((
        ConPtyChild {
            killer,
            exit: Some(exit_rx),
        },
        pid,
        master,
    ))
}

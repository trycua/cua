//! Exec sessions: a process (or an internal operation) whose output the
//! command router serves by byte offset, per file descriptor, to any number
//! of readers and reconnects, and whose stdin takes offset-numbered writes.
//!
//! cua-spacesd numbers stdout, stderr and PTY output in one combined space;
//! Modal's `TaskExecStdioRead` numbers each descriptor from zero. A session
//! splits the combined stream into per-descriptor buffers (the last
//! [`CAP`] bytes of each are kept).

use crate::modal::fstools::StdinSink;
use bytes::Bytes;
use cua_spacesd_client::{Command, ProcessEvent, SpacesdClient, pb};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

/// Output kept per descriptor.
pub const CAP: usize = 64 << 20;
/// Largest output message.
pub const CHUNK: usize = 1 << 20;

pub const STDOUT: usize = 0;
pub const STDERR: usize = 1;

/// A byte stream numbered from zero, of which the tail is kept.
#[derive(Default)]
pub struct OutBuf {
    start: u64,
    data: Vec<u8>,
}

/// Why a read could not be served.
#[derive(Debug, PartialEq, Eq)]
pub enum ReadErr {
    /// The bytes at the offset were dropped; the oldest kept byte is at `start`.
    Evicted { start: u64 },
}

impl OutBuf {
    pub fn total(&self) -> u64 {
        self.start + self.data.len() as u64
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.data.extend_from_slice(bytes);
        if self.data.len() > CAP {
            let drop = self.data.len() - CAP / 2;
            self.data.drain(..drop);
            self.start += drop as u64;
        }
    }

    /// Up to `max` bytes at `offset`; empty when `offset` is the end.
    pub fn slice(&self, offset: u64, max: usize) -> Result<&[u8], ReadErr> {
        if offset < self.start {
            return Err(ReadErr::Evicted { start: self.start });
        }
        let at = (offset - self.start) as usize;
        if at >= self.data.len() {
            return Ok(&[]);
        }
        let end = (at + max).min(self.data.len());
        Ok(&self.data[at..end])
    }
}

/// How an exec ended.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Exit {
    pub code: Option<i32>,
    pub signal: Option<i32>,
    pub timed_out: bool,
    pub error: Option<String>,
}

impl Exit {
    pub fn code(code: i32) -> Exit {
        Exit {
            code: Some(code),
            ..Default::default()
        }
    }

    /// The wire form: a code, or a signal number.
    pub fn status(&self) -> (Option<i32>, Option<i32>) {
        match (self.code, self.signal) {
            (Some(c), _) => (Some(c), None),
            (None, Some(s)) => (None, Some(s)),
            // Never started: the shell's "cannot execute".
            _ => (Some(127), None),
        }
    }
}

/// One piece of output with the time it arrived (the sandbox's log).
#[derive(Debug, Clone, Copy)]
pub struct LogChunk {
    pub at_ns: u64,
    pub fd: usize,
    pub start: u64,
    pub len: u32,
}

/// Output chunks kept for `Sandbox.logs`.
const LOG_CHUNKS: usize = 100_000;

struct Inner {
    out: [OutBuf; 2],
    exit: Option<Exit>,
    generation: u64,
    log: std::collections::VecDeque<LogChunk>,
}

/// Control of the running process behind a session.
#[derive(Clone)]
pub struct ProcCtl {
    client: SpacesdClient,
    tag: String,
    writer: String,
    seq: Arc<AtomicU64>,
}

impl ProcCtl {
    fn selector(&self) -> pb::ProcessSelector {
        pb::ProcessSelector {
            selector: Some(pb::process_selector::Selector::Tag(self.tag.clone())),
        }
    }

    pub async fn send_stdin(&self, data: Vec<u8>) -> Result<(), tonic::Status> {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let req = pb::SendInputRequest {
            process: Some(self.selector()),
            input: Some(pb::ProcessInput {
                input: Some(pb::process_input::Input::Stdin(data)),
            }),
            sequence: seq,
            writer_id: self.writer.clone(),
        };
        self.client.process().send_input(req).await.map(|_| ())
    }

    pub async fn send_pty(&self, data: Vec<u8>) -> Result<(), tonic::Status> {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let req = pb::SendInputRequest {
            process: Some(self.selector()),
            input: Some(pb::ProcessInput {
                input: Some(pb::process_input::Input::Pty(data)),
            }),
            sequence: seq,
            writer_id: self.writer.clone(),
        };
        self.client.process().send_input(req).await.map(|_| ())
    }

    pub async fn close_stdin(&self) -> Result<(), tonic::Status> {
        self.client
            .process()
            .close_stdin(pb::CloseStdinRequest {
                process: Some(self.selector()),
            })
            .await
            .map(|_| ())
    }

    pub async fn signal(&self, signal: pb::Signal, group: bool) -> Result<(), tonic::Status> {
        self.client
            .process()
            .signal_process(pb::SignalProcessRequest {
                process: Some(self.selector()),
                signal: signal as i32,
                process_group: group,
            })
            .await
            .map(|_| ())
    }
}

/// Where a session's stdin goes.
enum Stdin {
    /// A process, plain pipe or PTY.
    Process { ctl: ProcCtl, pty: bool },
    /// An internal operation.
    Internal(StdinSink),
    /// No stdin (the process has /dev/null).
    None,
}

struct StdinState {
    written: u64,
    closed: bool,
}

/// One exec (or a sandbox's entrypoint).
pub struct Session {
    inner: Mutex<Inner>,
    tx: watch::Sender<u64>,
    stdin: tokio::sync::Mutex<(Stdin, StdinState)>,
    ctl: Mutex<Option<ProcCtl>>,
    /// The task reading the process's output; ended when the session is finished.
    pump: Mutex<Option<tokio::task::AbortHandle>>,
    /// Descriptors whose output is discarded (`DEVNULL`), by index.
    discard: [bool; 2],
    /// stderr is delivered on stdout (`TASK_EXEC_STDERR_CONFIG_STDOUT`).
    merge_stderr: bool,
    /// A PTY: all output is on stdout.
    pty: bool,
}

pub struct Config {
    pub discard_stdout: bool,
    pub discard_stderr: bool,
    pub merge_stderr: bool,
}

impl Session {
    fn new(ctl: Option<ProcCtl>, stdin: Stdin, cfg: &Config, pty: bool) -> Arc<Session> {
        let (tx, _) = watch::channel(0);
        Arc::new(Session {
            inner: Mutex::new(Inner {
                out: Default::default(),
                exit: None,
                generation: 0,
                log: Default::default(),
            }),
            tx,
            stdin: tokio::sync::Mutex::new((
                stdin,
                StdinState {
                    written: 0,
                    closed: false,
                },
            )),
            ctl: Mutex::new(ctl),
            pump: Mutex::new(None),
            discard: [cfg.discard_stdout, cfg.discard_stderr],
            merge_stderr: cfg.merge_stderr,
            pty,
        })
    }

    fn bump(&self, g: &mut Inner) {
        g.generation += 1;
        let _ = self.tx.send(g.generation);
    }

    /// Output from the process or operation (`fd`: 0 stdout, 1 stderr).
    pub fn push(&self, fd: usize, bytes: &[u8]) {
        let fd = if self.merge_stderr || self.pty {
            STDOUT
        } else {
            fd
        };
        if self.discard[fd] || bytes.is_empty() {
            return;
        }
        let mut g = self.inner.lock().expect("session");
        let start = g.out[fd].total();
        g.out[fd].push(bytes);
        let at_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        for (i, piece) in bytes.chunks(64 * 1024).enumerate() {
            g.log.push_back(LogChunk {
                at_ns,
                fd,
                start: start + (i * 64 * 1024) as u64,
                len: piece.len() as u32,
            });
        }
        while g.log.len() > LOG_CHUNKS {
            g.log.pop_front();
        }
        self.bump(&mut g);
    }

    /// The log chunks in `[since_ns, until_ns)` with their bytes (those no
    /// longer retained are skipped).
    pub fn log_between(&self, since_ns: u64, until_ns: u64) -> Vec<(LogChunk, Vec<u8>)> {
        let g = self.inner.lock().expect("session");
        g.log
            .iter()
            .filter(|c| c.at_ns >= since_ns && c.at_ns < until_ns)
            .filter_map(|c| {
                g.out[c.fd]
                    .slice(c.start, c.len as usize)
                    .ok()
                    .map(|b| (*c, b.to_vec()))
            })
            .collect()
    }

    pub fn finish(&self, exit: Exit) {
        {
            let mut g = self.inner.lock().expect("session");
            if g.exit.is_none() {
                g.exit = Some(exit);
            }
            self.bump(&mut g);
        }
        // A finished process needs no connection to its Space: the output
        // stays, the handles that hold the Space's daemon connection go.
        self.ctl.lock().expect("session ctl").take();
        if let Some(p) = self.pump.lock().expect("session pump").take() {
            p.abort();
        }
        if let Ok(mut s) = self.stdin.try_lock()
            && matches!(s.0, Stdin::Process { .. })
        {
            s.0 = Stdin::None;
        }
    }

    pub fn exit(&self) -> Option<Exit> {
        self.inner.lock().expect("session").exit.clone()
    }

    /// Waits for the exit.
    pub async fn wait(&self) -> Exit {
        let mut rx = self.tx.subscribe();
        loop {
            if let Some(e) = self.exit() {
                return e;
            }
            if rx.changed().await.is_err() {
                return self.exit().unwrap_or_default();
            }
        }
    }

    /// The next output at `offset` of `fd`, waiting for it; `Ok(None)` at
    /// the end of a finished stream.
    pub async fn read(&self, fd: usize, offset: u64) -> Result<Option<Bytes>, ReadErr> {
        let mut rx = self.tx.subscribe();
        loop {
            {
                let g = self.inner.lock().expect("session");
                let chunk = g.out[fd].slice(offset, CHUNK)?;
                if !chunk.is_empty() {
                    return Ok(Some(Bytes::copy_from_slice(chunk)));
                }
                if g.exit.is_some() {
                    return Ok(None);
                }
            }
            if rx.changed().await.is_err() {
                return Ok(None);
            }
        }
    }

    /// Bytes of `fd` produced so far.
    pub fn total(&self, fd: usize) -> u64 {
        self.inner.lock().expect("session").out[fd].total()
    }

    /// Stdin: a write numbered `offset` (the bytes written before it).
    /// A write that starts before the bytes written is a retry: its
    /// already-written prefix is skipped. One that starts after them is a
    /// gap and fails.
    pub async fn write_stdin(&self, offset: u64, data: &[u8], eof: bool) -> Result<(), StdinErr> {
        let mut g = self.stdin.lock().await;
        let (sink, state) = &mut *g;
        if offset > state.written {
            return Err(StdinErr::Gap {
                expected: state.written,
            });
        }
        let skip = (state.written - offset) as usize;
        if state.closed && skip >= data.len() && eof {
            return Ok(());
        }
        let fresh = if skip >= data.len() {
            &[][..]
        } else {
            &data[skip..]
        };
        if state.closed && !fresh.is_empty() {
            return Err(StdinErr::Closed);
        }
        if !fresh.is_empty() {
            match sink {
                Stdin::Process { ctl, pty } => {
                    let r = if *pty {
                        ctl.send_pty(fresh.to_vec()).await
                    } else {
                        ctl.send_stdin(fresh.to_vec()).await
                    };
                    r.map_err(StdinErr::Rpc)?;
                }
                Stdin::Internal(s) => s.send(Some(Bytes::copy_from_slice(fresh))).await?,
                Stdin::None => return Err(StdinErr::Closed),
            }
            state.written += fresh.len() as u64;
        }
        if eof && !state.closed {
            match sink {
                Stdin::Process { ctl, pty } => {
                    if !*pty {
                        ctl.close_stdin().await.map_err(StdinErr::Rpc)?;
                    } else {
                        // The terminal's EOF.
                        ctl.send_pty(vec![0x04]).await.map_err(StdinErr::Rpc)?;
                    }
                }
                Stdin::Internal(s) => s.send(None).await?,
                Stdin::None => {}
            }
            state.closed = true;
        }
        Ok(())
    }

    pub async fn stdin_status(&self) -> (u64, bool) {
        let g = self.stdin.lock().await;
        (g.1.written, g.1.closed)
    }

    /// Signals the process (not for internal operations).
    pub async fn signal(&self, signal: pb::Signal, group: bool) {
        let ctl = self.ctl.lock().expect("session ctl").clone();
        if let Some(c) = ctl {
            let _ = c.signal(signal, group).await;
        }
    }
}

#[derive(Debug)]
pub enum StdinErr {
    Gap { expected: u64 },
    Closed,
    Rpc(tonic::Status),
    Gone,
}

impl From<crate::modal::fstools::SinkClosed> for StdinErr {
    fn from(_: crate::modal::fstools::SinkClosed) -> Self {
        StdinErr::Gone
    }
}

/// Starts `cmd` in the guest and pumps its output into a new session.
pub async fn start(
    c: &SpacesdClient,
    mut cmd: Command,
    cfg: Config,
) -> Result<Arc<Session>, cua_spacesd_client::Error> {
    let tag = cmd
        .tag
        .clone()
        .unwrap_or_else(|| format!("modal-{}", crate::auth::new_token()));
    cmd.tag = Some(tag.clone());
    let pty = cmd.pty.is_some();
    let has_stdin = cmd.stdin || pty;
    let handle = c.spawn(cmd).await?;
    Ok(from_handle(c, tag, handle, has_stdin, pty, &cfg))
}

/// Attaches to a process that is already running (a restored machine's
/// entrypoint), replaying everything retained.
pub async fn attach(c: &SpacesdClient, tag: &str) -> Option<Arc<Session>> {
    let handle = c
        .attach(
            cua_spacesd_client::ProcessRef::Tag(tag.to_string()),
            cua_spacesd_client::Replay::All,
        )
        .await
        .ok()?;
    let cfg = Config {
        discard_stdout: false,
        discard_stderr: false,
        merge_stderr: false,
    };
    Some(from_handle(c, tag.to_string(), handle, true, false, &cfg))
}

fn from_handle(
    c: &SpacesdClient,
    tag: String,
    mut handle: cua_spacesd_client::ProcessHandle,
    has_stdin: bool,
    pty: bool,
    cfg: &Config,
) -> Arc<Session> {
    let ctl = ProcCtl {
        client: c.clone(),
        tag,
        writer: format!("modal-{:016x}", rand::random::<u64>()),
        seq: Arc::new(AtomicU64::new(0)),
    };
    let stdin = if has_stdin {
        Stdin::Process {
            ctl: ctl.clone(),
            pty,
        }
    } else {
        Stdin::None
    };
    let session = Session::new(Some(ctl), stdin, cfg, pty);
    let s = session.clone();
    let pump = tokio::spawn(async move {
        loop {
            match handle.next_event().await {
                Ok(Some(ProcessEvent::Stdout { data, .. })) => s.push(STDOUT, &data),
                Ok(Some(ProcessEvent::Pty { data, .. })) => s.push(STDOUT, &data),
                Ok(Some(ProcessEvent::Stderr { data, .. })) => s.push(STDERR, &data),
                Ok(Some(ProcessEvent::Exit(st))) => {
                    s.finish(Exit {
                        code: st.code,
                        signal: st.signal.map(|g| g as i32),
                        timed_out: st.timed_out,
                        error: st.error,
                    });
                    break;
                }
                Ok(None) => break,
                Err(e) => {
                    // The link to the guest is gone for good.
                    s.finish(Exit {
                        error: Some(e.to_string()),
                        ..Default::default()
                    });
                    break;
                }
            }
        }
    });
    *session.pump.lock().expect("session pump") = Some(pump.abort_handle());
    session
}

/// A session for an internal operation, with its stdin channel.
pub fn internal(stdin: Option<StdinSink>, cfg: &Config) -> Arc<Session> {
    let sink = stdin.map(Stdin::Internal).unwrap_or(Stdin::None);
    Session::new(None, sink, cfg, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outbuf_keeps_the_tail_and_reports_eviction() {
        let mut b = OutBuf::default();
        b.push(b"hello");
        assert_eq!(b.total(), 5);
        assert_eq!(b.slice(0, 3).unwrap(), b"hel");
        assert_eq!(b.slice(3, 10).unwrap(), b"lo");
        assert_eq!(b.slice(5, 10).unwrap(), b"");
        let big = vec![7u8; CAP];
        b.push(&big);
        assert_eq!(b.total(), (CAP + 5) as u64);
        assert!(matches!(b.slice(0, 1), Err(ReadErr::Evicted { .. })));
        assert!(!b.slice(b.total() - 10, 100).unwrap().is_empty());
    }

    #[tokio::test]
    async fn readers_see_every_byte_once_and_the_end() {
        let s = internal(
            None,
            &Config {
                discard_stdout: false,
                discard_stderr: false,
                merge_stderr: false,
            },
        );
        let r = {
            let s = s.clone();
            tokio::spawn(async move {
                let mut off = 0u64;
                let mut got = Vec::new();
                while let Some(c) = s.read(STDOUT, off).await.unwrap() {
                    off += c.len() as u64;
                    got.extend_from_slice(&c);
                }
                got
            })
        };
        s.push(STDOUT, b"ab");
        tokio::task::yield_now().await;
        s.push(STDOUT, b"cd");
        s.finish(Exit::code(3));
        assert_eq!(r.await.unwrap(), b"abcd");
        assert_eq!(s.wait().await, Exit::code(3));
        // A late reader still gets everything, from any offset.
        assert_eq!(&s.read(STDOUT, 1).await.unwrap().unwrap()[..], b"bcd");
    }

    #[tokio::test]
    async fn stderr_merges_into_stdout_when_asked() {
        let s = internal(
            None,
            &Config {
                discard_stdout: false,
                discard_stderr: false,
                merge_stderr: true,
            },
        );
        s.push(STDOUT, b"o");
        s.push(STDERR, b"e");
        s.finish(Exit::code(0));
        assert_eq!(s.total(STDOUT), 2);
        assert_eq!(s.total(STDERR), 0);
    }
}

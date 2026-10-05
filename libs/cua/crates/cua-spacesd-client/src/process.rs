//! Processes: `run` (start + collect), detached handles, reattach by tag
//! with scrollback replay, sequenced stdin, and transparent stream resume.

use crate::{
    client::SpacesdClient,
    error::{Error, Result},
};
use bytes::Bytes;
use cua_proto::env::v1::{
    self as pb, process_data::Output as DataOut, process_event::Event, process_input::Input,
    process_selector::Selector,
};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tonic::Streaming;

/// Default server-side keepalive interval requested for process streams.
pub const DEFAULT_PROCESS_KEEPALIVE: Duration = Duration::from_secs(15);

/// A command to run. No shell is involved unless [`Command::shell`] is used.
#[derive(Clone, Debug, Default)]
pub struct Command {
    /// Executable.
    pub program: String,
    /// Arguments.
    pub args: Vec<String>,
    /// Extra environment.
    pub env: BTreeMap<String, String>,
    /// Working directory.
    pub cwd: Option<String>,
    /// OS user.
    pub user: Option<String>,
    /// Wall-clock limit (enforced by the server, and by `run` client-side).
    pub timeout: Option<Duration>,
    /// Reattach tag.
    pub tag: Option<String>,
    /// Keep stdin open.
    pub stdin: bool,
    /// Run under a PTY of this size (cols, rows).
    pub pty: Option<(u32, u32)>,
    /// Scrollback ring size (0 = server default).
    pub scrollback_bytes: u64,
    /// Kill when the starting stream ends early.
    pub kill_on_disconnect: bool,
    /// Keepalive interval requested for the stream.
    pub keepalive: Option<Duration>,
}

impl Command {
    /// Runs `program` directly.
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            ..Default::default()
        }
    }

    /// Runs `line` with `/bin/sh -c`.
    pub fn shell(line: impl Into<String>) -> Self {
        Self::new("/bin/sh").arg("-c").arg(line)
    }

    /// Adds an argument.
    pub fn arg(mut self, a: impl Into<String>) -> Self {
        self.args.push(a.into());
        self
    }

    /// Adds arguments.
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Sets an environment variable.
    pub fn env(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.env.insert(k.into(), v.into());
        self
    }

    /// Sets the working directory.
    pub fn cwd(mut self, cwd: impl Into<String>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Sets the user.
    pub fn user(mut self, user: impl Into<String>) -> Self {
        self.user = Some(user.into());
        self
    }

    /// Sets the timeout.
    pub fn timeout(mut self, t: Duration) -> Self {
        self.timeout = Some(t);
        self
    }

    /// Sets the reattach tag.
    pub fn tag(mut self, tag: impl Into<String>) -> Self {
        self.tag = Some(tag.into());
        self
    }

    /// Keeps stdin open for [`ProcessHandle::write_stdin`].
    pub fn stdin(mut self, on: bool) -> Self {
        self.stdin = on;
        self
    }

    /// Runs under a PTY.
    pub fn pty(mut self, cols: u32, rows: u32) -> Self {
        self.pty = Some((cols, rows));
        self
    }

    /// Stream keepalive interval.
    pub fn keepalive(mut self, interval: Duration) -> Self {
        self.keepalive = Some(interval);
        self
    }

    /// Scrollback ring size.
    pub fn scrollback(mut self, bytes: u64) -> Self {
        self.scrollback_bytes = bytes;
        self
    }

    fn to_request(&self) -> pb::StartProcessRequest {
        pb::StartProcessRequest {
            config: Some(pb::ProcessConfig {
                command: self.program.clone(),
                args: self.args.clone(),
                env: self.env.clone().into_iter().collect(),
                cwd: self.cwd.clone().unwrap_or_default(),
                user: self.user.clone().unwrap_or_default(),
                timeout: self.timeout.map(duration_pb),
            }),
            pty: self.pty.map(|(cols, rows)| pb::PtyConfig {
                size: Some(pb::PtySize {
                    cols,
                    rows,
                    pixel_width: 0,
                    pixel_height: 0,
                }),
                term: String::new(),
            }),
            tag: self.tag.clone().unwrap_or_default(),
            stdin: self.stdin,
            keepalive_interval: Some(duration_pb(
                self.keepalive.unwrap_or(DEFAULT_PROCESS_KEEPALIVE),
            )),
            scrollback_bytes: self.scrollback_bytes,
            kill_on_disconnect: self.kill_on_disconnect,
        }
    }
}

impl From<&str> for Command {
    fn from(line: &str) -> Self {
        Command::shell(line)
    }
}

impl From<String> for Command {
    fn from(line: String) -> Self {
        Command::shell(line)
    }
}

pub(crate) fn duration_pb(d: Duration) -> pbjson_types::Duration {
    pbjson_types::Duration {
        seconds: d.as_secs() as i64,
        nanos: d.subsec_nanos() as i32,
    }
}

/// How a process ended.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExitStatus {
    /// Exit code for a normal exit.
    pub code: Option<i32>,
    /// Terminating signal.
    pub signal: Option<pb::Signal>,
    /// Stopped by its timeout.
    pub timed_out: bool,
    /// Supervisor error (for example "executable not found").
    pub error: Option<String>,
}

impl ExitStatus {
    /// Exit code 0.
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }

    fn from_end(end: &pb::ProcessEnd) -> Self {
        Self {
            code: end.exit_code,
            signal: pb::Signal::try_from(end.signal)
                .ok()
                .filter(|s| *s != pb::Signal::Unspecified),
            timed_out: end.timed_out,
            error: (!end.error.is_empty()).then(|| end.error.clone()),
        }
    }
}

/// Collected result of [`SpacesdClient::run`].
#[derive(Clone, Debug, Default)]
pub struct Output {
    /// Exit status.
    pub status: ExitStatus,
    /// stdout bytes.
    pub stdout: Vec<u8>,
    /// stderr bytes.
    pub stderr: Vec<u8>,
    /// PTY bytes (PTY processes only).
    pub pty: Vec<u8>,
}

impl Output {
    /// Exit code 0.
    pub fn success(&self) -> bool {
        self.status.success()
    }

    /// stdout as lossy UTF-8.
    pub fn stdout_str(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    /// stderr as lossy UTF-8.
    pub fn stderr_str(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
}

/// One output chunk or the exit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProcessEvent {
    /// stdout bytes at a combined-output offset.
    Stdout {
        /// Offset of the first byte.
        offset: u64,
        /// Bytes.
        data: Bytes,
    },
    /// stderr bytes.
    Stderr {
        /// Offset of the first byte.
        offset: u64,
        /// Bytes.
        data: Bytes,
    },
    /// PTY bytes.
    Pty {
        /// Offset of the first byte.
        offset: u64,
        /// Bytes.
        data: Bytes,
    },
    /// The process ended. Always the last event.
    Exit(ExitStatus),
}

/// Scrollback replay when attaching.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Replay {
    /// Live output only.
    None,
    /// Up to this many trailing bytes of scrollback.
    LastBytes(u64),
    /// Everything retained.
    All,
    /// Everything from this combined-output offset.
    FromOffset(u64),
}

enum Stream {
    Start(Streaming<pb::StartProcessResponse>),
    Connect(Streaming<pb::ConnectProcessResponse>),
}

impl Stream {
    async fn next(&mut self) -> std::result::Result<Option<pb::ProcessEvent>, tonic::Status> {
        match self {
            Stream::Start(s) => Ok(s.message().await?.and_then(|m| m.event)),
            Stream::Connect(s) => Ok(s.message().await?.and_then(|m| m.event)),
        }
    }
}

/// A running (or exited) guest process.
pub struct ProcessHandle {
    client: SpacesdClient,
    pid: u32,
    tag: Option<String>,
    stream: Option<Stream>,
    /// Next combined-output offset not yet delivered.
    next_offset: u64,
    keepalive: Duration,
    exited: Option<ExitStatus>,
    keepalives: u64,
    reconnects: u64,
    writer_id: String,
    sequence: Arc<AtomicU64>,
    pending_exit: bool,
}

impl std::fmt::Debug for ProcessHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessHandle")
            .field("pid", &self.pid)
            .field("tag", &self.tag)
            .field("next_offset", &self.next_offset)
            .finish()
    }
}

impl ProcessHandle {
    /// Guest pid.
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Tag, if any.
    pub fn tag(&self) -> Option<&str> {
        self.tag.as_deref()
    }

    /// Keepalive messages observed so far.
    pub fn keepalives_seen(&self) -> u64 {
        self.keepalives
    }

    /// Stream reconnects performed so far.
    pub fn reconnects(&self) -> u64 {
        self.reconnects
    }

    /// Next combined-output offset (bytes delivered so far, counting replay).
    pub fn output_offset(&self) -> u64 {
        self.next_offset
    }

    fn selector(&self) -> pb::ProcessSelector {
        pb::ProcessSelector {
            selector: Some(match &self.tag {
                Some(t) => Selector::Tag(t.clone()),
                None => Selector::Pid(self.pid),
            }),
        }
    }

    /// Next output chunk or the exit. `None` after the exit was returned.
    /// A dropped or silent stream (no message for three keepalive intervals)
    /// is resumed with `ConnectProcess` from the last seen offset.
    pub async fn next_event(&mut self) -> Result<Option<ProcessEvent>> {
        if self.pending_exit {
            self.pending_exit = false;
            return Ok(self.exited.clone().map(ProcessEvent::Exit));
        }
        if self.exited.is_some() && self.stream.is_none() {
            return Ok(None);
        }
        let policy = self.client.retry_policy();
        let mut failures = 0u32;
        loop {
            if self.stream.is_none() {
                match self.reconnect().await {
                    Ok(()) => {}
                    Err(e) if e.is_retryable() && failures + 1 < policy.max_attempts => {
                        failures += 1;
                        tokio::time::sleep(policy.backoff(failures)).await;
                        continue;
                    }
                    Err(e) => return Err(e),
                }
            }
            let stream = self.stream.as_mut().expect("stream present");
            let liveness = self.keepalive.saturating_mul(3) + Duration::from_secs(1);
            let next = tokio::time::timeout(liveness, stream.next()).await;
            let event = match next {
                Ok(Ok(Some(ev))) => ev,
                Ok(Ok(None)) => {
                    // Server closed without ProcessEnd: resume.
                    self.stream = None;
                    failures += 1;
                    if failures >= policy.max_attempts {
                        return Err(Error::Protocol(
                            "process stream ended without ProcessEnd".into(),
                        ));
                    }
                    continue;
                }
                Ok(Err(status)) => {
                    let err = Error::from(status);
                    if err.is_retryable() && failures + 1 < policy.max_attempts {
                        failures += 1;
                        self.stream = None;
                        tokio::time::sleep(policy.backoff(failures)).await;
                        continue;
                    }
                    return Err(err);
                }
                Err(_) => {
                    tracing::debug!(pid = self.pid, "process stream silent; reconnecting");
                    self.stream = None;
                    failures += 1;
                    if failures >= policy.max_attempts {
                        return Err(Error::Timeout(liveness));
                    }
                    continue;
                }
            };
            failures = 0;
            match event.event {
                Some(Event::Start(s)) => {
                    self.pid = s.pid;
                    if self.tag.is_none() && !s.tag.is_empty() {
                        self.tag = Some(s.tag);
                    }
                }
                Some(Event::Keepalive(_)) => self.keepalives += 1,
                Some(Event::Data(d)) => {
                    if let Some(ev) = self.accept_data(d) {
                        return Ok(Some(ev));
                    }
                }
                Some(Event::End(end)) => {
                    let status = ExitStatus::from_end(&end);
                    self.exited = Some(status.clone());
                    self.stream = None;
                    return Ok(Some(ProcessEvent::Exit(status)));
                }
                None => {}
            }
        }
    }

    fn accept_data(&mut self, d: pb::ProcessData) -> Option<ProcessEvent> {
        let (mut data, kind) = match d.output? {
            DataOut::Stdout(b) => (b, 0),
            DataOut::Stderr(b) => (b, 1),
            DataOut::Pty(b) => (b, 2),
        };
        let mut offset = d.offset;
        let end = offset + data.len() as u64;
        if end <= self.next_offset {
            return None; // already delivered
        }
        if offset < self.next_offset {
            let skip = (self.next_offset - offset) as usize;
            data.drain(..skip);
            offset = self.next_offset;
        }
        self.next_offset = end;
        let data = Bytes::from(data);
        Some(match kind {
            0 => ProcessEvent::Stdout { offset, data },
            1 => ProcessEvent::Stderr { offset, data },
            _ => ProcessEvent::Pty { offset, data },
        })
    }

    async fn reconnect(&mut self) -> Result<()> {
        self.reconnects += 1;
        let req = pb::ConnectProcessRequest {
            process: Some(self.selector()),
            replay_bytes: 0,
            replay_from_offset: Some(self.next_offset),
            keepalive_interval: Some(duration_pb(self.keepalive)),
        };
        let stream = self
            .client
            .process()
            .connect_process(req)
            .await?
            .into_inner();
        self.stream = Some(Stream::Connect(stream));
        Ok(())
    }

    /// Collects all output until exit.
    pub async fn wait(mut self) -> Result<Output> {
        let mut out = Output::default();
        while let Some(ev) = self.next_event().await? {
            match ev {
                ProcessEvent::Stdout { data, .. } => out.stdout.extend_from_slice(&data),
                ProcessEvent::Stderr { data, .. } => out.stderr.extend_from_slice(&data),
                ProcessEvent::Pty { data, .. } => out.pty.extend_from_slice(&data),
                ProcessEvent::Exit(s) => {
                    out.status = s;
                    break;
                }
            }
        }
        Ok(out)
    }

    /// Detaches from the output stream without affecting the process.
    pub fn detach(mut self) {
        self.stream = None;
    }

    /// Writes stdin (or PTY input for PTY processes) with exactly-once
    /// sequencing across retries (`SendInput`, gRPC-Web safe).
    pub async fn write_stdin(&self, data: impl Into<Bytes>) -> Result<()> {
        self.send_input(Input::Stdin(data.into().to_vec())).await
    }

    /// Writes PTY keystrokes.
    pub async fn write_pty(&self, data: impl Into<Bytes>) -> Result<()> {
        self.send_input(Input::Pty(data.into().to_vec())).await
    }

    async fn send_input(&self, input: Input) -> Result<()> {
        let seq = self.sequence.fetch_add(1, Ordering::SeqCst) + 1;
        let req = pb::SendInputRequest {
            process: Some(self.selector()),
            input: Some(pb::ProcessInput { input: Some(input) }),
            sequence: seq,
            writer_id: self.writer_id.clone(),
        };
        self.client
            .retry_policy()
            .run(|_| {
                let req = req.clone();
                async move {
                    self.client.process().send_input(req).await?;
                    Ok(())
                }
            })
            .await
    }

    /// Closes stdin.
    pub async fn close_stdin(&self) -> Result<()> {
        self.client
            .process()
            .close_stdin(pb::CloseStdinRequest {
                process: Some(self.selector()),
            })
            .await?;
        Ok(())
    }

    /// Sends a signal.
    pub async fn signal(&self, signal: pb::Signal) -> Result<()> {
        self.client
            .process()
            .signal_process(pb::SignalProcessRequest {
                process: Some(self.selector()),
                signal: signal as i32,
                process_group: false,
            })
            .await?;
        Ok(())
    }

    /// SIGKILL.
    pub async fn kill(&self) -> Result<()> {
        self.signal(pb::Signal::Kill).await
    }

    /// Resizes the PTY.
    pub async fn resize(&self, cols: u32, rows: u32) -> Result<()> {
        self.client
            .process()
            .resize_pty(pb::ResizePtyRequest {
                process: Some(self.selector()),
                size: Some(pb::PtySize {
                    cols,
                    rows,
                    pixel_width: 0,
                    pixel_height: 0,
                }),
            })
            .await?;
        Ok(())
    }
}

fn writer_id() -> String {
    format!("cua-env-{:016x}", rand::random::<u64>())
}

impl ProcessHandle {
    fn new(
        client: &SpacesdClient,
        pid: u32,
        tag: Option<String>,
        stream: Option<Stream>,
        next_offset: u64,
        keepalive: Duration,
    ) -> Self {
        Self {
            client: client.clone(),
            pid,
            tag,
            stream,
            next_offset,
            keepalive,
            exited: None,
            keepalives: 0,
            reconnects: 0,
            writer_id: writer_id(),
            sequence: Arc::new(AtomicU64::new(0)),
            pending_exit: false,
        }
    }
}

impl SpacesdClient {
    /// Starts `cmd` and returns a handle once `ProcessStart` arrives.
    pub async fn spawn(&self, cmd: impl Into<Command>) -> Result<ProcessHandle> {
        let cmd = cmd.into();
        let keepalive = cmd.keepalive.unwrap_or(DEFAULT_PROCESS_KEEPALIVE);
        let mut stream = self
            .process()
            .start_process(cmd.to_request())
            .await?
            .into_inner();
        let first = stream.message().await?;
        let start = match first.and_then(|m| m.event).and_then(|e| e.event) {
            Some(Event::Start(s)) => s,
            Some(Event::End(end)) => {
                // Failed to start: surface as an already-exited handle.
                let mut h = ProcessHandle::new(self, 0, cmd.tag.clone(), None, 0, keepalive);
                h.exited = Some(ExitStatus::from_end(&end));
                h.pending_exit = true;
                return Ok(h);
            }
            other => {
                return Err(Error::Protocol(format!(
                    "expected ProcessStart first, got {other:?}"
                )));
            }
        };
        Ok(ProcessHandle::new(
            self,
            start.pid,
            (!start.tag.is_empty()).then_some(start.tag).or(cmd.tag),
            Some(Stream::Start(stream)),
            start.scrollback_start_offset.min(start.output_end_offset),
            keepalive,
        ))
    }

    /// Starts `cmd`, collects its output and waits for exit. When the
    /// command has a timeout, the client also gives up (and kills the
    /// process) five seconds after it.
    pub async fn run(&self, cmd: impl Into<Command>) -> Result<Output> {
        let cmd = cmd.into();
        let timeout = cmd.timeout;
        let handle = self.spawn(cmd).await?;
        match timeout {
            None => handle.wait().await,
            Some(t) => {
                let deadline = t + Duration::from_secs(5);
                let selector = handle.selector();
                match tokio::time::timeout(deadline, handle.wait()).await {
                    Ok(r) => r,
                    Err(_) => {
                        let _ = self
                            .process()
                            .signal_process(pb::SignalProcessRequest {
                                process: Some(selector),
                                signal: pb::Signal::Kill as i32,
                                process_group: true,
                            })
                            .await;
                        Err(Error::Timeout(deadline))
                    }
                }
            }
        }
    }

    /// Reattaches to a process by tag (or pid), replaying scrollback.
    pub async fn attach(&self, selector: ProcessRef, replay: Replay) -> Result<ProcessHandle> {
        let keepalive = DEFAULT_PROCESS_KEEPALIVE;
        let (replay_bytes, replay_from_offset) = match replay {
            Replay::None => (0, None),
            Replay::LastBytes(n) => (n, None),
            Replay::All => (0, Some(0)),
            Replay::FromOffset(o) => (0, Some(o)),
        };
        let sel = selector.to_pb();
        let mut stream = self
            .process()
            .connect_process(pb::ConnectProcessRequest {
                process: Some(sel),
                replay_bytes,
                replay_from_offset,
                keepalive_interval: Some(duration_pb(keepalive)),
            })
            .await?
            .into_inner();
        let first = stream.message().await?;
        let start = match first.and_then(|m| m.event).and_then(|e| e.event) {
            Some(Event::Start(s)) => s,
            other => {
                return Err(Error::Protocol(format!(
                    "expected ProcessStart first, got {other:?}"
                )));
            }
        };
        let next_offset = match replay {
            Replay::None => start.output_end_offset,
            Replay::LastBytes(n) => start
                .output_end_offset
                .saturating_sub(n)
                .max(start.scrollback_start_offset),
            Replay::All => start.scrollback_start_offset,
            Replay::FromOffset(o) => o.max(start.scrollback_start_offset),
        };
        Ok(ProcessHandle::new(
            self,
            start.pid,
            (!start.tag.is_empty()).then_some(start.tag),
            Some(Stream::Connect(stream)),
            next_offset,
            keepalive,
        ))
    }

    /// Lists managed processes.
    pub async fn list_processes(&self, include_exited: bool) -> Result<Vec<pb::ProcessInfo>> {
        Ok(self
            .process()
            .list_processes(pb::ListProcessesRequest {
                include_exited,
                tag_prefix: String::new(),
            })
            .await?
            .into_inner()
            .processes)
    }
}

/// Identifies a process to attach to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProcessRef {
    /// By pid.
    Pid(u32),
    /// By tag.
    Tag(String),
}

impl ProcessRef {
    fn to_pb(&self) -> pb::ProcessSelector {
        pb::ProcessSelector {
            selector: Some(match self {
                ProcessRef::Pid(p) => Selector::Pid(*p),
                ProcessRef::Tag(t) => Selector::Tag(t.clone()),
            }),
        }
    }
}

impl From<&str> for ProcessRef {
    fn from(tag: &str) -> Self {
        ProcessRef::Tag(tag.into())
    }
}

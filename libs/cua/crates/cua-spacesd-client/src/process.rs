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
use futures_util::FutureExt as _;
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tonic::Streaming;

/// Default server-side keepalive interval requested for process streams.
pub const DEFAULT_PROCESS_KEEPALIVE: Duration = Duration::from_secs(15);

/// Read-ahead bound: messages already received are taken off the HTTP/2
/// connection up to this many bytes, so a caller that consumes events more
/// slowly than a command writes small chunks does not leave thousands of tiny
/// DATA frames buffered in h2 (which h2 treats as a flood and answers with
/// GOAWAY `ENHANCE_YOUR_CALM`, failing every stream on the connection).
const READ_AHEAD_BYTES: usize = 8 * 1024 * 1024;
/// Contiguous output of one stream is merged into events of at most this size.
const MERGE_BYTES: usize = 1024 * 1024;

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
    /// Idempotency key (`StartProcessRequest.request_id`): a retry with the
    /// same key reattaches to, or replays the outcome of, the first start
    /// instead of running the command again.
    pub request_id: Option<String>,
    /// Fencing epoch (`StartProcessRequest.epoch`, 0 = unfenced).
    pub epoch: u64,
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

    /// Idempotency key: see [`Command::request_id`].
    pub fn request_id(mut self, key: impl Into<String>) -> Self {
        self.request_id = Some(key.into());
        self
    }

    /// Fencing epoch: see [`Command::epoch`].
    pub fn epoch(mut self, epoch: u64) -> Self {
        self.epoch = epoch;
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
            request_id: self.request_id.clone().unwrap_or_default(),
            epoch: self.epoch,
        }
    }
}

/// Capability: `ProcessConfig.user` (and file `run_as`) and how the agent
/// serves it. Attribute `mode` is a [`RunAsMode`].
pub const PROCESS_RUN_AS_FEATURE: &str = "process.run_as";

/// How the guest agent switches to another user, from the `process.run_as`
/// feature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunAsMode {
    /// The agent is root: it switches users itself, no `sudo`.
    Root,
    /// The agent holds `CAP_SETUID` and `CAP_SETGID`: it switches users
    /// itself, no `sudo` (gVisor, runc and VMs alike).
    Capabilities,
    /// The agent cannot, but `sudo` is installed: the client wraps the
    /// command in `sudo -n`.
    Sudo,
    /// The guest offers no way to run as another user.
    None,
    /// An agent that predates the feature: the client tries `sudo -n` when
    /// the agent refuses to switch.
    Unknown,
}

impl RunAsMode {
    /// The agent switches users itself (the client never uses `sudo`).
    pub fn is_native(self) -> bool {
        matches!(self, Self::Root | Self::Capabilities)
    }

    fn parse(s: &str) -> Self {
        match s {
            "root" => Self::Root,
            "capabilities" => Self::Capabilities,
            "sudo" => Self::Sudo,
            "none" => Self::None,
            _ => Self::Unknown,
        }
    }
}

/// The driver refused to run a process as another user because it runs
/// unprivileged.
fn cannot_switch_user(e: &Error) -> bool {
    matches!(e, Error::PermissionDenied(d) if d.message.contains("cannot switch"))
}

impl Command {
    /// The same command for `user`, run through `sudo -n` by the driver's
    /// own user: `sudo -n -H [-u user] -- env K=V... program args`.
    fn through_sudo(&self) -> Command {
        let user = self.user.clone().unwrap_or_default();
        let mut args = vec!["-n".to_string(), "-H".to_string()];
        if user != "root" {
            args.extend(["-u".to_string(), user]);
        }
        args.extend(["--".to_string(), "env".to_string()]);
        args.extend(self.env.iter().map(|(k, v)| format!("{k}={v}")));
        args.push(self.program.clone());
        args.extend(self.args.iter().cloned());
        Command {
            program: "sudo".into(),
            args,
            user: None,
            ..self.clone()
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
    /// Raw number of the terminating signal on the guest's OS, also for
    /// signals [`ExitStatus::signal`] cannot name (SIGSEGV, SIGABRT, ...).
    pub signal_number: Option<i32>,
    /// Conventional name of the terminating signal without `SIG`, in upper
    /// case ("SEGV"); independent of the guest's OS.
    pub signal_name: Option<String>,
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
            signal_number: (end.signal_number != 0).then_some(end.signal_number),
            signal_name: (!end.signal_name.is_empty()).then(|| end.signal_name.clone()),
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

/// What the current stream produced ahead of the caller.
enum Ahead {
    Event(pb::ProcessEvent),
    Ended,
    Failed(tonic::Status),
}

fn ahead_len(a: &Ahead) -> usize {
    match a {
        Ahead::Event(pb::ProcessEvent {
            event: Some(Event::Data(d)),
        }) => match &d.output {
            Some(DataOut::Stdout(b) | DataOut::Stderr(b) | DataOut::Pty(b)) => b.len(),
            None => 0,
        },
        _ => 0,
    }
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
    /// Messages of the current stream read ahead of the caller.
    ahead: VecDeque<Ahead>,
    ahead_bytes: usize,
    /// Started through `sudo` (the driver could not switch to the user).
    via_sudo: bool,
    /// Output bytes the Space no longer held when the stream was resumed
    /// (its scrollback ring had moved past them).
    dropped_bytes: u64,
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

    /// Whether it was started through `sudo` ([`SpacesdClient::spawn`]):
    /// stop it with [`SpacesdClient::kill_tree`].
    pub fn via_sudo(&self) -> bool {
        self.via_sudo
    }

    /// Keepalive messages observed so far.
    pub fn keepalives_seen(&self) -> u64 {
        self.keepalives
    }

    /// Stream reconnects performed so far.
    pub fn reconnects(&self) -> u64 {
        self.reconnects
    }

    /// Output bytes that were lost between two events: after a resumed
    /// stream the Space's scrollback (`Command::scrollback`) had already
    /// dropped them, so they were never delivered. Zero when nothing was
    /// lost. Read it after each event to report a gap.
    pub fn dropped_bytes(&self) -> u64 {
        self.dropped_bytes
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
            let liveness = self.keepalive.saturating_mul(3) + Duration::from_secs(1);
            let next = match self.ahead.pop_front() {
                Some(a) => {
                    self.ahead_bytes -= ahead_len(&a);
                    Ok(match a {
                        Ahead::Event(ev) => Ok(Some(ev)),
                        Ahead::Ended => Ok(None),
                        Ahead::Failed(status) => Err(status),
                    })
                }
                None => {
                    let stream = self.stream.as_mut().expect("stream present");
                    tokio::time::timeout(liveness, stream.next()).await
                }
            };
            let event = match next {
                Ok(Ok(Some(ev))) => ev,
                Ok(Ok(None)) => {
                    // Server closed without ProcessEnd: resume.
                    self.drop_stream();
                    failures += 1;
                    if failures >= policy.max_attempts {
                        return Err(Error::Protocol(
                            "process stream ended without ProcessEnd".into(),
                        ));
                    }
                    continue;
                }
                Ok(Err(status)) => {
                    let flood = crate::error::is_h2_flood_goaway(&status);
                    let err = Error::from(status);
                    if err.is_retryable() && failures + 1 < policy.max_attempts {
                        failures += 1;
                        self.drop_stream();
                        // The connection was closed for being read too
                        // slowly, not for being broken: a command that
                        // keeps writing is outrunning the Space's
                        // scrollback, so the first resume is immediate.
                        if !(flood && failures == 1) {
                            tokio::time::sleep(policy.backoff(failures)).await;
                        }
                        continue;
                    }
                    return Err(err);
                }
                Err(_) => {
                    tracing::debug!(pid = self.pid, "process stream silent; reconnecting");
                    self.drop_stream();
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
                Some(Event::Data(mut d)) => {
                    self.read_ahead();
                    self.merge_ahead(&mut d);
                    if let Some(ev) = self.accept_data(d) {
                        return Ok(Some(ev));
                    }
                }
                Some(Event::End(end)) => {
                    let status = ExitStatus::from_end(&end);
                    self.exited = Some(status.clone());
                    self.drop_stream();
                    return Ok(Some(ProcessEvent::Exit(status)));
                }
                None => {}
            }
        }
    }

    fn drop_stream(&mut self) {
        self.stream = None;
        self.ahead.clear();
        self.ahead_bytes = 0;
    }

    /// Takes every message the stream has already received off the
    /// connection (without waiting), up to [`READ_AHEAD_BYTES`].
    fn read_ahead(&mut self) {
        let Some(stream) = self.stream.as_mut() else {
            return;
        };
        if matches!(self.ahead.back(), Some(Ahead::Ended | Ahead::Failed(_))) {
            return;
        }
        while self.ahead_bytes < READ_AHEAD_BYTES {
            let Some(next) = stream.next().now_or_never() else {
                break;
            };
            let a = match next {
                Ok(Some(ev)) => Ahead::Event(ev),
                Ok(None) => Ahead::Ended,
                Err(status) => Ahead::Failed(status),
            };
            let last = !matches!(a, Ahead::Event(_));
            self.ahead_bytes += ahead_len(&a);
            self.ahead.push_back(a);
            if last {
                break;
            }
        }
    }

    /// Appends the read-ahead output that directly continues `d` on the
    /// same stream, up to [`MERGE_BYTES`].
    fn merge_ahead(&mut self, d: &mut pb::ProcessData) {
        loop {
            let Some(Ahead::Event(pb::ProcessEvent {
                event: Some(Event::Data(next)),
            })) = self.ahead.front()
            else {
                return;
            };
            let (Some(out), Some(more)) = (d.output.as_mut(), next.output.as_ref()) else {
                return;
            };
            let (into, from) = match (out, more) {
                (DataOut::Stdout(a), DataOut::Stdout(b))
                | (DataOut::Stderr(a), DataOut::Stderr(b))
                | (DataOut::Pty(a), DataOut::Pty(b)) => (a, b),
                _ => return,
            };
            if d.offset + into.len() as u64 != next.offset || into.len() + from.len() > MERGE_BYTES
            {
                return;
            }
            into.extend_from_slice(from);
            let taken = self.ahead.pop_front().expect("front exists");
            self.ahead_bytes -= ahead_len(&taken);
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
        if offset > self.next_offset {
            self.dropped_bytes += offset - self.next_offset;
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
        self.drop_stream();
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

    /// Sends a signal to the process's whole group (it and what it
    /// started).
    pub async fn signal_group(&self, signal: pb::Signal) -> Result<()> {
        self.client
            .process()
            .signal_process(pb::SignalProcessRequest {
                process: Some(self.selector()),
                signal: signal as i32,
                process_group: true,
            })
            .await?;
        Ok(())
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
            ahead: VecDeque::new(),
            ahead_bytes: 0,
            via_sudo: false,
            dropped_bytes: 0,
        }
    }
}

impl SpacesdClient {
    /// Starts `cmd` and returns a handle once `ProcessStart` arrives.
    ///
    /// `cmd.user` runs it as that user: the agent takes the user's uid, gid
    /// and groups itself ([`SpacesdClient::run_as_mode`]), with no `sudo`.
    /// Only an agent that cannot switch and reports `sudo` (or predates the
    /// `process.run_as` feature) gets the command through `sudo -n`
    /// (passwordless in those older images), with its environment passed
    /// through `env`. An agent that reports it cannot run as another user at
    /// all is not asked to: the refusal, which says why, is returned.
    pub async fn spawn(&self, cmd: impl Into<Command>) -> Result<ProcessHandle> {
        let cmd = cmd.into();
        match self.start(&cmd).await {
            Err(e) if cmd.user.is_some() && cannot_switch_user(&e) => {
                match self.run_as_mode().await {
                    RunAsMode::Sudo | RunAsMode::Unknown => {}
                    _ => return Err(e),
                }
                let mut h = self.start(&cmd.through_sudo()).await?;
                h.via_sudo = true;
                Ok(h)
            }
            r => r,
        }
    }

    /// How the guest agent runs commands and file operations as another
    /// user. [`RunAsMode::Unknown`] when it does not say (an older agent,
    /// or the capabilities cannot be read).
    pub async fn run_as_mode(&self) -> RunAsMode {
        let Ok(caps) = self.capabilities().await else {
            return RunAsMode::Unknown;
        };
        caps.features
            .iter()
            .find(|f| f.name == PROCESS_RUN_AS_FEATURE)
            .map_or(RunAsMode::Unknown, |f| {
                RunAsMode::parse(f.attributes.get("mode").map_or("", String::as_str))
            })
    }

    /// SIGKILLs process `pid` and its process group; for one started
    /// through `sudo`, also as root (the driver may not signal root's
    /// processes). Best effort past the first signal.
    pub async fn kill_tree(&self, pid: u32, via_sudo: bool) -> Result<()> {
        // A handle from `attach` does not know how the process started.
        let via_sudo = via_sudo
            || self
                .list_processes(false)
                .await
                .map(|ps| {
                    ps.iter().any(|p| {
                        p.pid == pid && p.config.as_ref().is_some_and(|c| c.command == "sudo")
                    })
                })
                .unwrap_or(false);
        let signalled = self
            .process()
            .signal_process(pb::SignalProcessRequest {
                process: Some(pb::ProcessSelector {
                    selector: Some(Selector::Pid(pid)),
                }),
                signal: pb::Signal::Kill as i32,
                process_group: true,
            })
            .await;
        if via_sudo {
            let kill = Command::new("sudo")
                .args(["-n", "kill", "-KILL", "--", &format!("-{pid}")])
                .timeout(Duration::from_secs(10));
            let _ = self.run(kill).await;
        }
        signalled?;
        Ok(())
    }

    async fn start(&self, cmd: &Command) -> Result<ProcessHandle> {
        let cmd = cmd.clone();
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
        // `ConnectProcess` only reads: a dropped connection (a flood GOAWAY
        // included) before `ProcessStart` arrives is retried like any
        // idempotent call.
        let (stream, first) = self
            .retry_policy()
            .run(|_| {
                let req = pb::ConnectProcessRequest {
                    process: Some(sel.clone()),
                    replay_bytes,
                    replay_from_offset,
                    keepalive_interval: Some(duration_pb(keepalive)),
                };
                async move {
                    let mut stream = self.process().connect_process(req).await?.into_inner();
                    let first = stream.message().await?;
                    Ok((stream, first))
                }
            })
            .await?;
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
        let dropped = match replay {
            Replay::FromOffset(o) => next_offset.saturating_sub(o),
            _ => 0,
        };
        let mut handle = ProcessHandle::new(
            self,
            start.pid,
            (!start.tag.is_empty()).then_some(start.tag),
            Some(Stream::Connect(stream)),
            next_offset,
            keepalive,
        );
        handle.dropped_bytes = dropped;
        Ok(handle)
    }

    /// Sends a signal to the group of a process this client did not start
    /// (or no longer holds a handle to), found by pid or tag.
    pub async fn signal_group(&self, selector: ProcessRef, signal: pb::Signal) -> Result<()> {
        self.process()
            .signal_process(pb::SignalProcessRequest {
                process: Some(selector.to_pb()),
                signal: signal as i32,
                process_group: true,
            })
            .await?;
        Ok(())
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

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `ProcessService`: detached, reattachable processes with a bounded
//! scrollback ring, PTYs, sequenced input, signals and exit retention.

#[cfg(windows)]
mod conpty;
pub mod ring;
pub mod spawn;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use bytes::{Bytes, BytesMut};
use cua_proto::env::v1::process_data::Output as DataOutput;
use cua_proto::env::v1::process_event::Event;
use cua_proto::env::v1::process_input::Input;
use cua_proto::env::v1::process_selector::Selector;
use cua_proto::env::v1::process_service_server::{ProcessService, ProcessServiceServer};
use cua_proto::env::v1::stream_input_request::Message as StreamInputMessage;
use cua_proto::env::v1::{
    CloseStdinRequest, CloseStdinResponse, ConnectProcessRequest, ConnectProcessResponse,
    ErrorReason, KeepAlive, ListProcessesRequest, ListProcessesResponse, ProcessConfig,
    ProcessData, ProcessEnd, ProcessEvent, ProcessInfo, ProcessSelector, ProcessStart,
    ProcessState, ResizePtyRequest, ResizePtyResponse, SendInputRequest, SendInputResponse, Signal,
    SignalProcessRequest, SignalProcessResponse, StartProcessRequest, StartProcessResponse,
    StreamInputRequest, StreamInputResponse,
};
use futures_util::Stream;
use tokio::io::{AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Code, Request, Response, Status, Streaming};

use crate::config::{MAX_CHUNK_BYTES, MAX_SCROLLBACK_BYTES};
use crate::context::ServerContext;
use crate::error::{status, StatusBuilder};
use crate::util::{duration, timestamp};

use ring::{OutKind, Output};
use spawn::{PtyMaster, SpawnSpec, Spawned};

/// Default keepalive interval on process streams.
pub const DEFAULT_KEEPALIVE: Duration = Duration::from_secs(30);
/// Metadata fallback for the keepalive interval, in milliseconds.
pub const KEEPALIVE_METADATA: &str = "x-cua-keepalive-interval-ms";
/// Exited processes are kept at least this long...
pub const RETENTION_MIN_AGE: Duration = Duration::from_secs(10 * 60);
/// ...and the most recent this many are kept regardless of age.
pub const RETENTION_MIN_COUNT: usize = 256;
/// Grace between SIGTERM and SIGKILL.
pub const KILL_GRACE: Duration = Duration::from_secs(5);
/// How long to keep draining output after the process exits (a background
/// grandchild may hold the pipes open forever).
const DRAIN_AFTER_EXIT: Duration = Duration::from_secs(2);
/// Largest single `ProcessData` payload sent to a client.
const MAX_DATA_MESSAGE: usize = 1024 * 1024;

struct InputState {
    writer: Option<Box<dyn AsyncWrite + Send + Unpin>>,
    sequences: HashMap<String, u64>,
}

/// One managed process.
pub struct ProcessEntry {
    id: u64,
    pid: u32,
    tag: String,
    config: ProcessConfig,
    pty: Option<PtyMaster>,
    started_at: SystemTime,
    output: Arc<Output>,
    input: tokio::sync::Mutex<InputState>,
    end: Mutex<Option<(ProcessEnd, Instant)>>,
    attached: AtomicU32,
    timed_out: AtomicBool,
    #[cfg(not(unix))]
    kill: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

impl ProcessEntry {
    fn running(&self) -> bool {
        self.end.lock().expect("end lock").is_none()
    }

    fn end_event(&self) -> Option<ProcessEnd> {
        self.end
            .lock()
            .expect("end lock")
            .as_ref()
            .map(|(e, _)| e.clone())
    }

    fn exited_at(&self) -> Option<Instant> {
        self.end
            .lock()
            .expect("end lock")
            .as_ref()
            .map(|(_, at)| *at)
    }

    fn start_event(&self) -> ProcessStart {
        let (start, end) = self.output.bounds();
        ProcessStart {
            pid: self.pid,
            tag: self.tag.clone(),
            started_at: Some(timestamp(self.started_at)),
            scrollback_start_offset: start,
            output_end_offset: end,
        }
    }

    /// Sends `signal` to the process or its group. Refuses once the process
    /// has been reaped, so a recycled pid is never signalled.
    fn send_signal(&self, signal: Signal, group: bool) -> Result<(), Status> {
        let end = self.end.lock().expect("end lock");
        if end.is_some() {
            return Err(status(
                Code::FailedPrecondition,
                ErrorReason::ProcessNotFound,
                format!("process {} has already exited", self.pid),
            ));
        }
        #[cfg(unix)]
        {
            let Some(native) = spawn::native_signal(signal) else {
                return Err(crate::error::invalid("signal is required"));
            };
            let target = if group {
                -(self.pid as i32)
            } else {
                self.pid as i32
            };
            // SAFETY: kill(2) with a pid we spawned and have not reaped (the
            // end lock is held, and the waiter takes it before recording the
            // exit), so the pid cannot have been recycled.
            let rc = unsafe { libc::kill(target, native) };
            if rc < 0 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::ESRCH) && group {
                    // Group already gone; fall back to the leader.
                    // SAFETY: as above.
                    unsafe { libc::kill(self.pid as i32, native) };
                    return Ok(());
                }
                return Err(crate::error::internal(format!("kill: {error}")));
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            let _ = group;
            match signal {
                Signal::Kill | Signal::Term => {
                    if let Some(tx) = self.kill.lock().expect("kill lock").take() {
                        let _ = tx.send(());
                    }
                    Ok(())
                }
                _ => Err(crate::error::unsupported(
                    "pty",
                    "only SIGNAL_KILL and SIGNAL_TERM are supported on this platform",
                )),
            }
        }
    }

    fn info(&self) -> ProcessInfo {
        let (start, end) = self.output.bounds();
        let mut config = self.config.clone();
        for (key, value) in config.env.iter_mut() {
            let upper = key.to_ascii_uppercase();
            if ["TOKEN", "SECRET", "KEY", "PASSWORD"]
                .iter()
                .any(|needle| upper.contains(needle))
            {
                value.clear();
            }
        }
        let end_event = self.end_event();
        ProcessInfo {
            pid: self.pid,
            tag: self.tag.clone(),
            config: Some(config),
            pty: self.pty.is_some(),
            state: if end_event.is_some() {
                ProcessState::Exited as i32
            } else {
                ProcessState::Running as i32
            },
            end: end_event,
            started_at: Some(timestamp(self.started_at)),
            scrollback_start_offset: start,
            output_end_offset: end,
            attached_clients: self.attached.load(Ordering::SeqCst),
        }
    }
}

/// Registry of managed processes.
pub struct ProcessManager {
    ctx: ServerContext,
    entries: Mutex<Vec<Arc<ProcessEntry>>>,
    next_id: AtomicU64,
}

impl ProcessManager {
    /// Creates the manager and its retention sweeper.
    pub fn new(ctx: ServerContext) -> Arc<Self> {
        let manager = Arc::new(Self {
            ctx,
            entries: Mutex::new(Vec::new()),
            next_id: AtomicU64::new(1),
        });
        let weak = Arc::downgrade(&manager);
        let shutdown = manager.ctx.shutdown_token();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(30));
            loop {
                tokio::select! {
                    _ = tick.tick() => {}
                    _ = shutdown.cancelled() => return,
                }
                match weak.upgrade() {
                    Some(manager) => manager.sweep(),
                    None => return,
                }
            }
        });
        manager
    }

    /// Drops exited entries that are both older than
    /// [`RETENTION_MIN_AGE`] and outside the newest
    /// [`RETENTION_MIN_COUNT`] processes.
    pub fn sweep(&self) {
        let mut entries = self.entries.lock().expect("registry lock");
        let total = entries.len();
        let mut index = 0;
        entries.retain(|entry| {
            let keep = match entry.exited_at() {
                None => true,
                Some(at) => {
                    at.elapsed() < RETENTION_MIN_AGE || total - index <= RETENTION_MIN_COUNT
                }
            };
            index += 1;
            keep
        });
    }

    fn refresh_count(&self) {
        let running = self
            .entries
            .lock()
            .expect("registry lock")
            .iter()
            .filter(|e| e.running())
            .count();
        self.ctx
            .managed_processes()
            .store(running as u32, Ordering::SeqCst);
    }

    /// Resolves a selector. A pid matches a running process first, then the
    /// newest exited one (pids are recycled by the OS).
    pub fn select(&self, selector: Option<&ProcessSelector>) -> Result<Arc<ProcessEntry>, Status> {
        let selector = selector
            .and_then(|s| s.selector.as_ref())
            .ok_or_else(|| crate::error::invalid("process selector (pid or tag) is required"))?;
        let entries = self.entries.lock().expect("registry lock");
        let found = match selector {
            Selector::Pid(pid) => entries
                .iter()
                .rev()
                .filter(|e| e.pid == *pid)
                .max_by_key(|e| (e.running(), e.id))
                .cloned(),
            Selector::Tag(tag) => entries.iter().rev().find(|e| &e.tag == tag).cloned(),
        };
        found.ok_or_else(|| {
            let (key, value) = match selector {
                Selector::Pid(pid) => ("pid", pid.to_string()),
                Selector::Tag(tag) => ("tag", tag.clone()),
            };
            StatusBuilder::new(
                Code::NotFound,
                ErrorReason::ProcessNotFound,
                format!("no managed process with {key} {value}"),
            )
            .meta(key, value)
            .build()
        })
    }

    fn resolve_spec(&self, request: &StartProcessRequest) -> Result<SpawnSpec, Status> {
        let config = request
            .config
            .as_ref()
            .ok_or_else(|| crate::error::invalid("config is required"))?;
        if config.command.is_empty() {
            return Err(crate::error::invalid("config.command is required"));
        }
        let init = self.ctx.init_state();
        let user_name = if !config.user.is_empty() {
            Some(config.user.clone())
        } else {
            init.default_user.clone()
        };
        let current = spawn::current_user_name();
        let switch_user = match user_name.as_deref() {
            Some(name) if name != current => {
                let user = spawn::lookup_user(name).map_err(|e| {
                    status(
                        Code::InvalidArgument,
                        ErrorReason::Unspecified,
                        format!("user {name:?}: {e}"),
                    )
                })?;
                #[cfg(unix)]
                {
                    // SAFETY: geteuid has no preconditions.
                    let euid = unsafe { libc::geteuid() };
                    if euid != 0 && euid != user.uid {
                        return Err(status(
                            Code::PermissionDenied,
                            ErrorReason::PermissionDenied,
                            format!("the driver runs as {current:?} and cannot switch to {name:?}"),
                        ));
                    }
                }
                Some(user)
            }
            _ => None,
        };
        let home = switch_user
            .as_ref()
            .map(|u| u.home.clone())
            .or_else(crate::config::home_dir)
            .unwrap_or_else(|| PathBuf::from("/"));

        let mut env: BTreeMap<String, String> = std::env::vars()
            .filter(|(k, _)| !spawn::SCRUBBED_ENV.contains(&k.as_str()))
            .collect();
        if let Some(user) = &switch_user {
            env.insert("HOME".into(), user.home.display().to_string());
            env.insert("USER".into(), user.name.clone());
            env.insert("LOGNAME".into(), user.name.clone());
            env.insert("SHELL".into(), user.shell.clone());
        }
        for (key, value) in init.proxy_env.iter().chain(init.env.iter()) {
            if value.is_empty() {
                env.remove(key);
            } else {
                env.insert(key.clone(), value.clone());
            }
        }
        for (key, value) in &config.env {
            env.insert(key.clone(), value.clone());
        }
        if let Some(pty) = &request.pty {
            let term = if pty.term.is_empty() {
                "xterm-256color".to_owned()
            } else {
                pty.term.clone()
            };
            env.insert("TERM".into(), term);
        }

        let cwd = if !config.cwd.is_empty() {
            crate::filesystem::paths::expand(&config.cwd, &home, init.default_workdir.as_deref())
        } else {
            init.default_workdir.clone().unwrap_or(home)
        };
        let pty = request.pty.as_ref().map(|p| {
            let size = p.size.unwrap_or_default();
            (
                size.cols.clamp(1, u16::MAX as u32) as u16,
                size.rows.clamp(1, u16::MAX as u32) as u16,
                size.pixel_width.min(u16::MAX as u32) as u16,
                size.pixel_height.min(u16::MAX as u32) as u16,
            )
        });
        Ok(SpawnSpec {
            command: config.command.clone(),
            args: config.args.clone(),
            env,
            cwd,
            switch_user,
            stdin: request.stdin,
            pty,
        })
    }

    /// Starts a process. Returns the entry, or a `ProcessEnd` describing why
    /// it could not start.
    pub fn start(
        self: &Arc<Self>,
        request: &StartProcessRequest,
    ) -> Result<Result<Arc<ProcessEntry>, ProcessEnd>, Status> {
        let spec = self.resolve_spec(request)?;
        let config = request.config.clone().unwrap_or_default();
        let scrollback = match request.scrollback_bytes {
            0 => self.ctx.config().default_scrollback_bytes,
            n => n.min(MAX_SCROLLBACK_BYTES),
        };

        let mut entries = self.entries.lock().expect("registry lock");
        if !request.tag.is_empty() {
            if let Some(existing) = entries.iter().find(|e| e.tag == request.tag) {
                if existing.running() {
                    return Err(StatusBuilder::new(
                        Code::AlreadyExists,
                        ErrorReason::Unspecified,
                        format!(
                            "tag {:?} belongs to running process {}",
                            request.tag, existing.pid
                        ),
                    )
                    .meta("tag", &request.tag)
                    .meta("pid", existing.pid)
                    .build());
                }
            }
            // An exited process's tag may be reused: evict the old entry.
            entries.retain(|e| e.tag != request.tag);
        }
        if !spec.cwd.is_dir() {
            return Ok(Err(ProcessEnd {
                error: format!("working directory {} does not exist", spec.cwd.display()),
                ended_at: Some(crate::util::now_ts()),
                ..Default::default()
            }));
        }
        let spawned = if spec.pty.is_some() {
            spawn::spawn_pty(&spec)
        } else {
            spawn::spawn_pipes(&spec)
        };
        let spawned = match spawned {
            Ok(spawned) => spawned,
            Err(error) => {
                let message = if error.kind() == std::io::ErrorKind::NotFound {
                    format!("executable not found: {}", spec.command)
                } else {
                    format!("failed to start {}: {error}", spec.command)
                };
                return Ok(Err(ProcessEnd {
                    error: message,
                    ended_at: Some(crate::util::now_ts()),
                    ..Default::default()
                }));
            }
        };
        let Spawned {
            child,
            pid,
            stdin,
            stdout,
            stderr,
            pty,
        } = spawned;
        #[cfg(not(unix))]
        let (kill_tx, kill_rx) = tokio::sync::oneshot::channel::<()>();
        let writer: Option<Box<dyn AsyncWrite + Send + Unpin>> = match &pty {
            Some(master) => Some(Box::new(master.clone())),
            None => stdin,
        };
        let entry = Arc::new(ProcessEntry {
            id: self.next_id.fetch_add(1, Ordering::SeqCst),
            pid,
            tag: request.tag.clone(),
            config,
            pty: pty.clone(),
            started_at: SystemTime::now(),
            output: Arc::new(Output::new(scrollback)),
            input: tokio::sync::Mutex::new(InputState {
                writer,
                sequences: HashMap::new(),
            }),
            end: Mutex::new(None),
            attached: AtomicU32::new(0),
            timed_out: AtomicBool::new(false),
            #[cfg(not(unix))]
            kill: Mutex::new(Some(kill_tx)),
        });
        entries.push(entry.clone());
        drop(entries);

        // Readers.
        let mut readers = Vec::new();
        if let Some(master) = pty {
            readers.push(tokio::spawn(pump(
                Box::new(master),
                entry.output.clone(),
                OutKind::Pty,
            )));
        }
        if let Some(stdout) = stdout {
            readers.push(tokio::spawn(pump(
                stdout,
                entry.output.clone(),
                OutKind::Stdout,
            )));
        }
        if let Some(stderr) = stderr {
            readers.push(tokio::spawn(pump(
                stderr,
                entry.output.clone(),
                OutKind::Stderr,
            )));
        }

        // Timeout.
        if let Some(limit) = duration(entry.config.timeout.as_ref()).filter(|d| !d.is_zero()) {
            let weak = Arc::downgrade(&entry);
            tokio::spawn(async move {
                tokio::time::sleep(limit).await;
                if let Some(entry) = weak.upgrade() {
                    if entry.running() {
                        entry.timed_out.store(true, Ordering::SeqCst);
                        terminate(entry).await;
                    }
                }
            });
        }

        // Waiter.
        let manager = self.clone();
        let waited = entry.clone();
        tokio::spawn(async move {
            #[cfg(unix)]
            let status = {
                let mut child = child;
                child.wait().await
            };
            #[cfg(not(unix))]
            let status = {
                let mut child = child;
                tokio::select! {
                    status = child.wait() => status,
                    _ = kill_rx => {
                        let _ = child.start_kill();
                        child.wait().await
                    }
                }
            };
            // ConPTY keeps the output pipe open after the child exits;
            // closing the pseudo console lets the reader reach EOF.
            #[cfg(windows)]
            if let Some(master) = waited.pty.clone() {
                let _ = tokio::task::spawn_blocking(move || master.close()).await;
            }
            // Drain what the readers still have, bounded.
            let drain = futures_util::future::join_all(readers);
            let _ = tokio::time::timeout(DRAIN_AFTER_EXIT, drain).await;
            let mut end = ProcessEnd {
                timed_out: waited.timed_out.load(Ordering::SeqCst),
                ended_at: Some(crate::util::now_ts()),
                ..Default::default()
            };
            match status {
                Ok(status) => {
                    end.exit_code = status.code();
                    #[cfg(unix)]
                    {
                        use std::os::unix::process::ExitStatusExt as _;
                        if let Some(signal) = status.signal() {
                            end.signal = spawn::contract_signal(signal) as i32;
                        }
                    }
                }
                Err(error) => end.error = format!("wait failed: {error}"),
            }
            *waited.end.lock().expect("end lock") = Some((end, Instant::now()));
            waited.input.lock().await.writer = None;
            waited.output.close();
            manager.refresh_count();
            manager.sweep();
        });
        self.refresh_count();
        Ok(Ok(entry))
    }

    /// Lists processes, newest first.
    pub fn list(&self, include_exited: bool, tag_prefix: &str) -> Vec<ProcessInfo> {
        let entries = self.entries.lock().expect("registry lock");
        entries
            .iter()
            .rev()
            .filter(|e| include_exited || e.running())
            .filter(|e| e.tag.starts_with(tag_prefix))
            .map(|e| e.info())
            .collect()
    }

    #[cfg(test)]
    fn insert_for_test(
        &self,
        pid: u32,
        tag: &str,
        exited_ago: Option<Duration>,
    ) -> Arc<ProcessEntry> {
        let entry = Arc::new(ProcessEntry {
            id: self.next_id.fetch_add(1, Ordering::SeqCst),
            pid,
            tag: tag.into(),
            config: ProcessConfig::default(),
            pty: None,
            started_at: SystemTime::now(),
            output: Arc::new(Output::new(1024)),
            input: tokio::sync::Mutex::new(InputState {
                writer: None,
                sequences: HashMap::new(),
            }),
            end: Mutex::new(exited_ago.map(|ago| {
                (
                    ProcessEnd {
                        exit_code: Some(0),
                        ..Default::default()
                    },
                    Instant::now() - ago,
                )
            })),
            attached: AtomicU32::new(0),
            timed_out: AtomicBool::new(false),
            #[cfg(not(unix))]
            kill: Mutex::new(None),
        });
        self.entries.lock().unwrap().push(entry.clone());
        entry
    }
}

async fn pump(
    mut reader: Box<dyn tokio::io::AsyncRead + Send + Unpin>,
    output: Arc<Output>,
    kind: OutKind,
) {
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => output.push(kind, Bytes::copy_from_slice(&buf[..n])),
        }
    }
}

/// SIGTERM to the group, then SIGKILL after [`KILL_GRACE`].
async fn terminate(entry: Arc<ProcessEntry>) {
    if entry.send_signal(Signal::Term, true).is_err() {
        return;
    }
    let mut progress = entry.output.subscribe();
    let exited = tokio::time::timeout(KILL_GRACE, async {
        while entry.running() {
            if progress.changed().await.is_err() {
                break;
            }
        }
    })
    .await;
    if exited.is_err() || entry.running() {
        let _ = entry.send_signal(Signal::Kill, true);
    }
}

fn data_event(offset: u64, kind: OutKind, data: Bytes) -> ProcessEvent {
    let output = match kind {
        OutKind::Stdout => DataOutput::Stdout(data.to_vec()),
        OutKind::Stderr => DataOutput::Stderr(data.to_vec()),
        OutKind::Pty => DataOutput::Pty(data.to_vec()),
    };
    ProcessEvent {
        event: Some(Event::Data(ProcessData {
            offset,
            output: Some(output),
        })),
    }
}

/// Spawns the per-subscriber task that turns the ring into a gRPC stream.
fn subscribe<T: Send + 'static>(
    entry: Arc<ProcessEntry>,
    cursor: u64,
    keepalive: Duration,
    kill_on_disconnect: bool,
    wrap: fn(ProcessEvent) -> T,
) -> ReceiverStream<Result<T, Status>> {
    let (tx, rx) = mpsc::channel::<Result<T, Status>>(16);
    entry.attached.fetch_add(1, Ordering::SeqCst);
    tokio::spawn(async move {
        let mut cursor = cursor;
        let mut progress = entry.output.subscribe();
        let mut disconnected = tx
            .send(Ok(wrap(ProcessEvent {
                event: Some(Event::Start(entry.start_event())),
            })))
            .await
            .is_err();
        let mut last_sent = tokio::time::Instant::now();
        while !disconnected {
            let snapshot = *progress.borrow_and_update();
            let batch = entry.output.read_from(cursor, MAX_DATA_MESSAGE * 4);
            if !batch.chunks.is_empty() {
                // Merge contiguous same-kind chunks into messages of at most
                // MAX_DATA_MESSAGE bytes.
                let mut pending: Option<(u64, OutKind, BytesMut)> = None;
                let mut messages = Vec::new();
                for chunk in batch.chunks {
                    match &mut pending {
                        Some((offset, kind, buf))
                            if *kind == chunk.kind
                                && *offset + buf.len() as u64 == chunk.offset
                                && buf.len() + chunk.data.len() <= MAX_DATA_MESSAGE =>
                        {
                            buf.extend_from_slice(&chunk.data);
                        }
                        _ => {
                            if let Some((offset, kind, buf)) = pending.take() {
                                messages.push(data_event(offset, kind, buf.freeze()));
                            }
                            pending =
                                Some((chunk.offset, chunk.kind, BytesMut::from(&chunk.data[..])));
                        }
                    }
                }
                if let Some((offset, kind, buf)) = pending {
                    messages.push(data_event(offset, kind, buf.freeze()));
                }
                for message in messages {
                    if tx.send(Ok(wrap(message))).await.is_err() {
                        disconnected = true;
                        break;
                    }
                }
                cursor = batch.next;
                last_sent = tokio::time::Instant::now();
                continue;
            }
            if snapshot.closed {
                if let Some(end) = entry.end_event() {
                    let _ = tx
                        .send(Ok(wrap(ProcessEvent {
                            event: Some(Event::End(end)),
                        })))
                        .await;
                }
                break;
            }
            tokio::select! {
                changed = progress.changed() => {
                    if changed.is_err() {
                        break;
                    }
                }
                _ = tokio::time::sleep_until(last_sent + keepalive) => {
                    if tx.send(Ok(wrap(ProcessEvent { event: Some(Event::Keepalive(KeepAlive {})) }))).await.is_err() {
                        disconnected = true;
                    }
                    last_sent = tokio::time::Instant::now();
                }
                _ = tx.closed() => disconnected = true,
            }
        }
        entry.attached.fetch_sub(1, Ordering::SeqCst);
        if disconnected && kill_on_disconnect && entry.running() {
            terminate(entry).await;
        }
    });
    ReceiverStream::new(rx)
}

fn keepalive_interval<T>(
    field: Option<&cua_proto::wkt::Duration>,
    request: &Request<T>,
) -> Duration {
    duration(field)
        .filter(|d| !d.is_zero())
        .or_else(|| {
            request
                .metadata()
                .get(KEEPALIVE_METADATA)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .filter(|ms| *ms > 0)
                .map(Duration::from_millis)
        })
        .unwrap_or(DEFAULT_KEEPALIVE)
        .max(Duration::from_millis(100))
}

/// Writes one input chunk, enforcing sequence rules.
async fn apply_input(
    entry: &ProcessEntry,
    input: Option<&cua_proto::env::v1::ProcessInput>,
    sequence: u64,
    writer_id: &str,
) -> Result<SendInputResponse, Status> {
    let input = input
        .and_then(|i| i.input.as_ref())
        .ok_or_else(|| crate::error::invalid("input is required"))?;
    let bytes = match input {
        Input::Stdin(bytes) => bytes,
        Input::Pty(bytes) => {
            if entry.pty.is_none() {
                return Err(status(
                    Code::FailedPrecondition,
                    ErrorReason::Unspecified,
                    "process does not run under a PTY",
                ));
            }
            bytes
        }
    };
    if bytes.len() > MAX_CHUNK_BYTES as usize {
        return Err(StatusBuilder::new(
            Code::InvalidArgument,
            ErrorReason::LimitExceeded,
            format!("input chunk exceeds {MAX_CHUNK_BYTES} bytes"),
        )
        .build());
    }
    let mut state = entry.input.lock().await;
    if sequence != 0 {
        match state.sequences.get(writer_id).copied() {
            Some(last) if sequence <= last => {
                return Ok(SendInputResponse {
                    applied_sequence: last,
                    duplicate: true,
                })
            }
            Some(last) if sequence != last + 1 => {
                return Err(StatusBuilder::new(
                    Code::FailedPrecondition,
                    ErrorReason::SequenceGap,
                    format!("expected sequence {}, got {sequence}", last + 1),
                )
                .meta("expected_sequence", last + 1)
                .build())
            }
            _ => {}
        }
    }
    let Some(writer) = state.writer.as_mut() else {
        return Err(status(
            Code::FailedPrecondition,
            ErrorReason::Unspecified,
            if entry.running() {
                "stdin is closed or was not requested (set StartProcessRequest.stdin)"
            } else {
                "process has exited"
            },
        ));
    };
    writer
        .write_all(bytes)
        .await
        .and(writer.flush().await)
        .map_err(|e| {
            status(
                Code::FailedPrecondition,
                ErrorReason::DeliveryFailed,
                format!("write: {e}"),
            )
        })?;
    if sequence != 0 {
        state.sequences.insert(writer_id.to_owned(), sequence);
    }
    Ok(SendInputResponse {
        applied_sequence: sequence,
        duplicate: false,
    })
}

/// The gRPC service.
#[derive(Clone)]
pub struct ProcessServiceImpl {
    manager: Arc<ProcessManager>,
}

impl ProcessServiceImpl {
    /// Creates the service over a manager.
    pub fn new(manager: Arc<ProcessManager>) -> Self {
        Self { manager }
    }

    /// Tonic server with the driver's message limits.
    pub fn into_server(self) -> ProcessServiceServer<Self> {
        ProcessServiceServer::new(self)
            .max_decoding_message_size(crate::config::MAX_MESSAGE_BYTES as usize)
            .max_encoding_message_size(crate::config::MAX_MESSAGE_BYTES as usize)
    }
}

type EventStream<T> = Pin<Box<dyn Stream<Item = Result<T, Status>> + Send>>;

#[tonic::async_trait]
impl ProcessService for ProcessServiceImpl {
    type StartProcessStream = EventStream<StartProcessResponse>;
    type ConnectProcessStream = EventStream<ConnectProcessResponse>;

    async fn start_process(
        &self,
        request: Request<StartProcessRequest>,
    ) -> Result<Response<Self::StartProcessStream>, Status> {
        let keepalive = keepalive_interval(request.get_ref().keepalive_interval.as_ref(), &request);
        let body = request.into_inner();
        let wrap = |event| StartProcessResponse { event: Some(event) };
        match self.manager.start(&body)? {
            Ok(entry) => Ok(Response::new(Box::pin(subscribe(
                entry,
                0,
                keepalive,
                body.kill_on_disconnect,
                wrap,
            )))),
            Err(end) => {
                let events = vec![
                    Ok(wrap(ProcessEvent {
                        event: Some(Event::Start(ProcessStart {
                            tag: body.tag.clone(),
                            started_at: end.ended_at,
                            ..Default::default()
                        })),
                    })),
                    Ok(wrap(ProcessEvent {
                        event: Some(Event::End(end)),
                    })),
                ];
                Ok(Response::new(Box::pin(tokio_stream::iter(events))))
            }
        }
    }

    async fn connect_process(
        &self,
        request: Request<ConnectProcessRequest>,
    ) -> Result<Response<Self::ConnectProcessStream>, Status> {
        let keepalive = keepalive_interval(request.get_ref().keepalive_interval.as_ref(), &request);
        let body = request.into_inner();
        let entry = self.manager.select(body.process.as_ref())?;
        let cursor = match body.replay_from_offset {
            Some(offset) => offset,
            None => entry.output.cursor_for_last(body.replay_bytes),
        };
        Ok(Response::new(Box::pin(subscribe(
            entry,
            cursor,
            keepalive,
            false,
            |event| ConnectProcessResponse { event: Some(event) },
        ))))
    }

    async fn list_processes(
        &self,
        request: Request<ListProcessesRequest>,
    ) -> Result<Response<ListProcessesResponse>, Status> {
        let body = request.into_inner();
        Ok(Response::new(ListProcessesResponse {
            processes: self.manager.list(body.include_exited, &body.tag_prefix),
        }))
    }

    async fn send_input(
        &self,
        request: Request<SendInputRequest>,
    ) -> Result<Response<SendInputResponse>, Status> {
        let body = request.into_inner();
        let entry = self.manager.select(body.process.as_ref())?;
        apply_input(&entry, body.input.as_ref(), body.sequence, &body.writer_id)
            .await
            .map(Response::new)
    }

    async fn stream_input(
        &self,
        request: Request<Streaming<StreamInputRequest>>,
    ) -> Result<Response<StreamInputResponse>, Status> {
        let mut stream = request.into_inner();
        let first = stream
            .message()
            .await?
            .ok_or_else(|| crate::error::invalid("empty input stream"))?;
        let Some(StreamInputMessage::Process(selector)) = first.message else {
            return Err(crate::error::invalid(
                "the first StreamInput message must set `process`",
            ));
        };
        let entry = self.manager.select(Some(&selector))?;
        let mut written = 0u64;
        while let Some(message) = stream.message().await? {
            match message.message {
                Some(StreamInputMessage::Input(input)) => {
                    let len = match &input.input {
                        Some(Input::Stdin(b)) | Some(Input::Pty(b)) => b.len() as u64,
                        None => 0,
                    };
                    apply_input(&entry, Some(&input), 0, "").await?;
                    written += len;
                }
                Some(StreamInputMessage::Keepalive(_)) | None => {}
                Some(StreamInputMessage::Process(_)) => {
                    return Err(crate::error::invalid("`process` may only be set once"))
                }
            }
        }
        Ok(Response::new(StreamInputResponse {
            bytes_written: written,
        }))
    }

    async fn signal_process(
        &self,
        request: Request<SignalProcessRequest>,
    ) -> Result<Response<SignalProcessResponse>, Status> {
        let body = request.into_inner();
        let entry = self.manager.select(body.process.as_ref())?;
        let signal = Signal::try_from(body.signal)
            .ok()
            .filter(|s| *s != Signal::Unspecified)
            .ok_or_else(|| crate::error::invalid("unknown or unspecified signal"))?;
        entry.send_signal(signal, body.process_group)?;
        Ok(Response::new(SignalProcessResponse {}))
    }

    async fn close_stdin(
        &self,
        request: Request<CloseStdinRequest>,
    ) -> Result<Response<CloseStdinResponse>, Status> {
        let body = request.into_inner();
        let entry = self.manager.select(body.process.as_ref())?;
        let mut state = entry.input.lock().await;
        if entry.pty.is_some() {
            if let Some(writer) = state.writer.as_mut() {
                // VEOF (Ctrl-D) at the start of a line ends input in canonical
                // mode; Windows consoles take Ctrl-Z then Enter.
                let eof: &[u8] = if cfg!(windows) { b"\x1a\r" } else { b"\x04" };
                writer
                    .write_all(eof)
                    .await
                    .map_err(|e| crate::error::internal(format!("write EOF: {e}")))?;
            }
        } else if let Some(mut writer) = state.writer.take() {
            let _ = writer.shutdown().await;
        }
        Ok(Response::new(CloseStdinResponse {}))
    }

    async fn resize_pty(
        &self,
        request: Request<ResizePtyRequest>,
    ) -> Result<Response<ResizePtyResponse>, Status> {
        let body = request.into_inner();
        let entry = self.manager.select(body.process.as_ref())?;
        let Some(master) = &entry.pty else {
            return Err(status(
                Code::FailedPrecondition,
                ErrorReason::Unspecified,
                "process does not run under a PTY",
            ));
        };
        let size = body
            .size
            .ok_or_else(|| crate::error::invalid("size is required"))?;
        if size.cols == 0 || size.rows == 0 {
            return Err(crate::error::invalid("cols and rows must be at least 1"));
        }
        master
            .resize(
                size.cols.min(u16::MAX as u32) as u16,
                size.rows.min(u16::MAX as u32) as u16,
                size.pixel_width.min(u16::MAX as u32) as u16,
                size.pixel_height.min(u16::MAX as u32) as u16,
            )
            .map_err(|e| crate::error::internal(format!("resize: {e}")))?;
        Ok(Response::new(ResizePtyResponse {}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ServerConfig;

    fn manager() -> Arc<ProcessManager> {
        ProcessManager::new(ServerContext::new(ServerConfig::default(), None))
    }

    fn pid(pid: u32) -> ProcessSelector {
        ProcessSelector {
            selector: Some(Selector::Pid(pid)),
        }
    }

    #[tokio::test]
    async fn pid_reuse_prefers_the_running_process() {
        let manager = manager();
        let old = manager.insert_for_test(4242, "old", Some(Duration::from_secs(1)));
        let new = manager.insert_for_test(4242, "new", None);
        let picked = manager.select(Some(&pid(4242))).unwrap();
        assert!(Arc::ptr_eq(&picked, &new));
        // Once the new one exits too, the newest exited wins.
        *new.end.lock().unwrap() = Some((ProcessEnd::default(), Instant::now()));
        let picked = manager.select(Some(&pid(4242))).unwrap();
        assert!(Arc::ptr_eq(&picked, &new));
        assert!(!Arc::ptr_eq(&picked, &old));
        // Signalling an exited entry is refused rather than hitting a
        // recycled pid.
        assert_eq!(
            picked.send_signal(Signal::Term, false).unwrap_err().code(),
            Code::FailedPrecondition
        );
    }

    #[tokio::test]
    async fn exit_retention_keeps_recent_and_newest() {
        let manager = manager();
        // 300 processes exited an hour ago, plus 5 exited just now.
        for i in 0..300 {
            manager.insert_for_test(
                10_000 + i,
                &format!("old-{i}"),
                Some(Duration::from_secs(3600)),
            );
        }
        for i in 0..5 {
            manager.insert_for_test(
                20_000 + i,
                &format!("new-{i}"),
                Some(Duration::from_secs(1)),
            );
        }
        let running = manager.insert_for_test(30_000, "running", None);
        manager.sweep();
        let entries = manager.entries.lock().unwrap();
        // The newest 256 are kept regardless of age; everything recent and
        // everything running is kept.
        assert_eq!(entries.len(), RETENTION_MIN_COUNT);
        assert!(entries.iter().any(|e| Arc::ptr_eq(e, &running)));
        assert!(entries.iter().any(|e| e.tag == "new-0"));
        assert!(!entries.iter().any(|e| e.tag == "old-0"));
        assert!(entries.iter().any(|e| e.tag == "old-299"));
    }

    #[tokio::test]
    async fn unknown_selector_is_not_found() {
        let manager = manager();
        let error = manager
            .select(Some(&ProcessSelector {
                selector: Some(Selector::Tag("nope".into())),
            }))
            .map(|_| ())
            .unwrap_err();
        assert_eq!(error.code(), Code::NotFound);
        let info = crate::error::error_info(&error).unwrap();
        assert_eq!(info.reason, ErrorReason::ProcessNotFound as i32);
        assert_eq!(info.metadata["tag"], "nope");
    }

    #[tokio::test]
    async fn sequenced_input_detects_duplicates_and_gaps() {
        let manager = manager();
        let entry = manager.insert_for_test(1, "seq", None);
        let (client, mut server) = tokio::io::duplex(1024);
        entry.input.lock().await.writer = Some(Box::new(client));
        let input = |s: &str| cua_proto::env::v1::ProcessInput {
            input: Some(Input::Stdin(s.as_bytes().to_vec())),
        };
        let first = apply_input(&entry, Some(&input("a")), 7, "w")
            .await
            .unwrap();
        assert_eq!((first.applied_sequence, first.duplicate), (7, false));
        let dup = apply_input(&entry, Some(&input("a")), 7, "w")
            .await
            .unwrap();
        assert!(dup.duplicate);
        let gap = apply_input(&entry, Some(&input("c")), 9, "w")
            .await
            .unwrap_err();
        assert_eq!(
            crate::error::error_info(&gap).unwrap().metadata["expected_sequence"],
            "8"
        );
        apply_input(&entry, Some(&input("b")), 8, "w")
            .await
            .unwrap();
        // Independent writer ids start anywhere.
        apply_input(&entry, Some(&input("x")), 100, "other")
            .await
            .unwrap();
        let mut got = [0u8; 3];
        server.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"abx");
    }
}

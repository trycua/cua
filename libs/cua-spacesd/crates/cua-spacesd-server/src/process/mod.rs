// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `ProcessService`: detached, reattachable processes with a bounded
//! scrollback ring, PTYs, sequenced input, signals and exit retention.

#[cfg(windows)]
mod conpty;
pub mod ledger;
pub mod ring;
pub mod services;
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
    AdvanceEpochRequest, AdvanceEpochResponse, CloseStdinRequest, CloseStdinResponse,
    ConnectProcessRequest, ConnectProcessResponse, ErrorReason, ExecRecord, ExecState,
    GetExecRecordRequest, GetExecRecordResponse, KeepAlive, ListProcessesRequest,
    ListProcessesResponse, ProcessConfig, ProcessData, ProcessEnd, ProcessEvent, ProcessInfo,
    ProcessSelector, ProcessStart, ProcessState, ResizePtyRequest, ResizePtyResponse,
    SendInputRequest, SendInputResponse, Signal, SignalProcessRequest, SignalProcessResponse,
    StartProcessRequest, StartProcessResponse, StreamInputRequest, StreamInputResponse,
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

use ledger::Ledger;
use ring::{OutKind, Output};
use spawn::{PtyMaster, SpawnSpec, Spawned};

/// Capability feature for the exec ledger and fencing epoch.
pub const EXEC_LEDGER_FEATURE: &str = "process.exec_ledger";
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
/// After a small output message, more output waits up to this long to be
/// sent together. A command writing many tiny chunks otherwise becomes one
/// tiny HTTP/2 DATA frame per chunk, and a client that has a few hundred of
/// those unread answers with GOAWAY `ENHANCE_YOUR_CALM` (h2's flood
/// protection), failing every stream on its connection.
const COALESCE: Duration = Duration::from_millis(5);
/// Messages smaller than this make the next one wait ([`COALESCE`]).
const COALESCE_BELOW: usize = 16 * 1024;

struct InputState {
    writer: Option<Box<dyn AsyncWrite + Send + Unpin>>,
    sequences: HashMap<String, u64>,
}

/// One managed process.
pub struct ProcessEntry {
    id: u64,
    pid: u32,
    tag: String,
    /// `StartProcessRequest.request_id` (empty: no ledger entry).
    request_id: String,
    /// `StartProcessRequest.epoch` (0: unfenced).
    epoch: u64,
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
    /// The process id.
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Its output ring.
    pub fn output(&self) -> &Arc<Output> {
        &self.output
    }

    /// Waits until it has ended, and how.
    pub async fn ended(&self) -> ProcessEnd {
        let mut progress = self.output.subscribe();
        loop {
            if let Some(end) = self.end_event() {
                return end;
            }
            if progress.borrow().closed {
                // The ring closes just before the end is recorded.
                tokio::time::sleep(Duration::from_millis(20)).await;
                continue;
            }
            if progress.changed().await.is_err() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }

    /// Sends `signal` to the process group (see `SignalProcess`).
    pub fn signal_group(&self, signal: Signal) -> Result<(), Status> {
        self.send_signal(signal, true)
    }

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

    fn start_event(&self, deduplicated: bool) -> ProcessStart {
        let (start, end) = self.output.bounds();
        ProcessStart {
            pid: self.pid,
            tag: self.tag.clone(),
            started_at: Some(timestamp(self.started_at)),
            scrollback_start_offset: start,
            output_end_offset: end,
            deduplicated,
            request_id: self.request_id.clone(),
        }
    }

    /// The exec ledger view of this process (`request_id` processes).
    fn exec_record(&self) -> ExecRecord {
        let (_, end_offset) = self.output.bounds();
        let end = self.end_event();
        let exited = end.is_some();
        let tail = if exited {
            let from = self.output.cursor_for_last(ledger::TAIL_BYTES);
            self.output
                .read_from(from, ledger::TAIL_BYTES as usize)
                .chunks
                .into_iter()
                .map(|c| data_event_payload(c.offset, c.kind, c.data))
                .collect()
        } else {
            Vec::new()
        };
        ExecRecord {
            request_id: self.request_id.clone(),
            state: if exited {
                ExecState::Exited as i32
            } else {
                ExecState::Running as i32
            },
            epoch: self.epoch,
            pid: self.pid,
            started_at: Some(timestamp(self.started_at)),
            output_bytes: if exited { end_offset } else { 0 },
            output_sha256: if exited {
                self.output.sha256().unwrap_or_default()
            } else {
                String::new()
            },
            end,
            tail,
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
    /// The exec ledger and fencing epoch, opened on first use so servers
    /// that never see a `request_id` or epoch never touch the data dir.
    ledger: Mutex<Option<Ledger>>,
}

/// What `ProcessManager::start` did.
pub enum Started {
    /// Spawned a new process.
    New(Arc<ProcessEntry>),
    /// Could not spawn; the end says why.
    Failed(ProcessEnd),
    /// The `request_id` belongs to a process still in the registry.
    Attached(Arc<ProcessEntry>),
    /// The `request_id` is only in the ledger: replay its outcome.
    Replay(ExecRecord),
}

fn stale_epoch(requested: u64, current: u64) -> Status {
    StatusBuilder::new(
        Code::FailedPrecondition,
        ErrorReason::StaleEpoch,
        format!("epoch {requested} is older than the Space's current epoch {current}"),
    )
    .meta("current_epoch", current)
    .meta("epoch", requested)
    .build()
}

impl ProcessManager {
    /// Creates the manager and its retention sweeper.
    pub fn new(ctx: ServerContext) -> Arc<Self> {
        let manager = Arc::new(Self {
            ctx,
            entries: Mutex::new(Vec::new()),
            next_id: AtomicU64::new(1),
            ledger: Mutex::new(None),
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

    /// Runs `f` on the ledger, opening it on first use.
    fn with_ledger<T>(&self, f: impl FnOnce(&mut Ledger) -> T) -> Result<T, Status> {
        let mut slot = self.ledger.lock().expect("ledger lock");
        if slot.is_none() {
            let dir = self.ctx.config().data_dir.clone();
            let opened = Ledger::open(&dir).map_err(|e| {
                crate::error::internal(format!("exec ledger in {}: {e}", dir.display()))
            })?;
            *slot = Some(opened);
        }
        Ok(f(slot.as_mut().expect("ledger opened")))
    }

    /// Applies a request's fencing epoch: 0 passes, lower than current is
    /// refused, higher advances the epoch and kills every running process
    /// started with a lower non-zero epoch. Returns how many were killed.
    pub fn fence(&self, epoch: u64) -> Result<u32, Status> {
        if epoch == 0 {
            return Ok(0);
        }
        let advanced = self.with_ledger(|ledger| {
            let current = ledger.epoch();
            if epoch < current {
                return Err(stale_epoch(epoch, current));
            }
            let raised = epoch > current;
            ledger
                .advance(epoch)
                .map_err(|e| crate::error::internal(format!("write epoch: {e}")))?;
            Ok(raised)
        })??;
        if !advanced {
            return Ok(0);
        }
        let stale: Vec<Arc<ProcessEntry>> = self
            .entries
            .lock()
            .expect("registry lock")
            .iter()
            .filter(|e| e.epoch != 0 && e.epoch < epoch && e.running())
            .cloned()
            .collect();
        let mut killed = 0;
        for entry in stale {
            if entry.send_signal(Signal::Kill, true).is_ok() {
                killed += 1;
            }
        }
        if killed > 0 {
            tracing::info!(epoch, killed, "epoch advanced: killed stale processes");
        }
        Ok(killed)
    }

    /// The current epoch (opening the ledger).
    pub fn epoch(&self) -> Result<u64, Status> {
        self.with_ledger(|ledger| ledger.epoch())
    }

    /// What is known about `request_id`.
    pub fn exec_record(&self, request_id: &str) -> Result<Option<ExecRecord>, Status> {
        let entry = self
            .entries
            .lock()
            .expect("registry lock")
            .iter()
            .rev()
            .find(|e| e.request_id == request_id)
            .cloned();
        let stored = self.with_ledger(|ledger| ledger.get(request_id).cloned())?;
        Ok(match (entry, stored) {
            // A final ledger record is authoritative.
            (_, Some(record)) if record.state != "started" => Some(record.to_proto()),
            (Some(entry), _) => Some(entry.exec_record()),
            (None, Some(record)) => Some(record.to_proto()),
            (None, None) => None,
        })
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
                    if euid != user.uid
                        && crate::privilege::switch_mode() == crate::privilege::SwitchMode::None
                    {
                        return Err(status(
                            Code::PermissionDenied,
                            ErrorReason::PermissionDenied,
                            format!(
                                "the driver runs as {current:?} and cannot switch to {name:?} \
                                 (it is not root and holds no CAP_SETUID/CAP_SETGID)"
                            ),
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
    pub fn start(self: &Arc<Self>, request: &StartProcessRequest) -> Result<Started, Status> {
        if request.request_id.len() > ledger::MAX_REQUEST_ID {
            return Err(crate::error::invalid(format!(
                "request_id exceeds {} bytes",
                ledger::MAX_REQUEST_ID
            )));
        }
        let spec = self.resolve_spec(request)?;
        let config = request.config.clone().unwrap_or_default();
        let scrollback = match request.scrollback_bytes {
            0 => self.ctx.config().default_scrollback_bytes,
            n => n.min(MAX_SCROLLBACK_BYTES),
        };
        self.fence(request.epoch)?;
        let request_id = request.request_id.as_str();

        let mut entries = self.entries.lock().expect("registry lock");
        if !request_id.is_empty() {
            if let Some(existing) = entries.iter().rev().find(|e| e.request_id == request_id) {
                return Ok(Started::Attached(existing.clone()));
            }
            // Under the registry lock, so a concurrent start of the same key
            // cannot slip between the lookup and the `started` record.
            let known = self.with_ledger(|ledger| {
                let current = ledger.epoch();
                if request.epoch != 0 && request.epoch < current {
                    return Err(stale_epoch(request.epoch, current));
                }
                Ok(ledger.get(request_id).cloned())
            })??;
            if let Some(record) = known {
                return Ok(Started::Replay(record.to_proto()));
            }
        }
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
        let started_at = SystemTime::now();
        if !request_id.is_empty() {
            let record = ledger::Record {
                id: request_id.to_owned(),
                state: "started".into(),
                epoch: request.epoch,
                pid: 0,
                started_at_ms: ledger::ms(started_at),
                end: None,
                output_bytes: 0,
                output_sha256: String::new(),
                tail: Vec::new(),
            };
            self.with_ledger(|ledger| ledger.put(record))?
                .map_err(|e| crate::error::internal(format!("exec ledger: {e}")))?;
        }
        let failed = |manager: &Self, end: ProcessEnd| -> Result<Started, Status> {
            if !request_id.is_empty() {
                manager.record_exit(request_id, request.epoch, 0, started_at, &end, None)?;
            }
            Ok(Started::Failed(end))
        };
        if !spec.cwd.is_dir() {
            return failed(
                self,
                ProcessEnd {
                    error: format!("working directory {} does not exist", spec.cwd.display()),
                    ended_at: Some(crate::util::now_ts()),
                    ..Default::default()
                },
            );
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
                return failed(
                    self,
                    ProcessEnd {
                        error: message,
                        ended_at: Some(crate::util::now_ts()),
                        ..Default::default()
                    },
                );
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
            request_id: request.request_id.clone(),
            epoch: request.epoch,
            config,
            pty: pty.clone(),
            started_at,
            output: Arc::new(if request_id.is_empty() {
                Output::new(scrollback)
            } else {
                Output::with_digest(scrollback)
            }),
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
                            end.signal_number = signal;
                            end.signal_name = spawn::signal_name(signal).unwrap_or_default().into();
                        }
                    }
                }
                Err(error) => end.error = format!("wait failed: {error}"),
            }
            *waited.end.lock().expect("end lock") = Some((end.clone(), Instant::now()));
            waited.input.lock().await.writer = None;
            if !waited.request_id.is_empty() {
                // Recorded before the output closes, so a client that saw
                // `ProcessEnd` finds the outcome in the ledger.
                if let Err(error) = manager.record_exit(
                    &waited.request_id,
                    waited.epoch,
                    waited.pid,
                    waited.started_at,
                    &end,
                    Some(&waited.output),
                ) {
                    tracing::warn!(request_id = %waited.request_id, %error, "exec ledger");
                }
            }
            waited.output.close();
            manager.refresh_count();
            manager.sweep();
        });
        self.refresh_count();
        Ok(Started::New(entry))
    }

    /// Appends the `exited` record of `request_id`.
    fn record_exit(
        &self,
        request_id: &str,
        epoch: u64,
        pid: u32,
        started_at: SystemTime,
        end: &ProcessEnd,
        output: Option<&Output>,
    ) -> Result<(), Status> {
        let (bytes, sha256, tail) = match output {
            Some(output) => {
                let from = output.cursor_for_last(ledger::TAIL_BYTES);
                let tail = output
                    .read_from(from, ledger::TAIL_BYTES as usize)
                    .chunks
                    .iter()
                    .map(|c| ledger::tail_chunk(c.offset, c.kind, &c.data))
                    .collect();
                (output.bounds().1, output.sha256().unwrap_or_default(), tail)
            }
            None => (0, String::new(), Vec::new()),
        };
        let record = ledger::Record {
            id: request_id.to_owned(),
            state: "exited".into(),
            epoch,
            pid,
            started_at_ms: ledger::ms(started_at),
            end: Some(ledger::end_from_proto(end)),
            output_bytes: bytes,
            output_sha256: if sha256.is_empty() {
                ledger::EMPTY_SHA256.into()
            } else {
                sha256
            },
            tail,
        };
        self.with_ledger(|ledger| ledger.put(record))?
            .map_err(|e| crate::error::internal(format!("exec ledger: {e}")))
    }

    /// Running processes whose tag is exactly `tag`.
    pub fn running_with_tag(&self, tag: &str) -> Vec<Arc<ProcessEntry>> {
        self.entries
            .lock()
            .expect("entries lock")
            .iter()
            .filter(|e| e.tag == tag && e.running())
            .cloned()
            .collect()
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
            request_id: String::new(),
            epoch: 0,
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

fn data_event_payload(offset: u64, kind: OutKind, data: Bytes) -> ProcessData {
    let output = match kind {
        OutKind::Stdout => DataOutput::Stdout(data.to_vec()),
        OutKind::Stderr => DataOutput::Stderr(data.to_vec()),
        OutKind::Pty => DataOutput::Pty(data.to_vec()),
    };
    ProcessData {
        offset,
        output: Some(output),
    }
}

fn data_event(offset: u64, kind: OutKind, data: Bytes) -> ProcessEvent {
    ProcessEvent {
        event: Some(Event::Data(data_event_payload(offset, kind, data))),
    }
}

/// The events that replay a ledger record: `ProcessStart` (deduplicated),
/// the stored output tail, then `ProcessEnd`.
fn replay_events(record: ExecRecord) -> Vec<ProcessEvent> {
    let start = ProcessStart {
        pid: record.pid,
        tag: String::new(),
        started_at: record.started_at,
        scrollback_start_offset: record
            .tail
            .first()
            .map(|d| d.offset)
            .unwrap_or(record.output_bytes),
        output_end_offset: record.output_bytes,
        deduplicated: true,
        request_id: record.request_id.clone(),
    };
    let mut events = vec![ProcessEvent {
        event: Some(Event::Start(start)),
    }];
    events.extend(record.tail.into_iter().map(|data| ProcessEvent {
        event: Some(Event::Data(data)),
    }));
    events.push(ProcessEvent {
        event: Some(Event::End(record.end.unwrap_or_else(|| ProcessEnd {
            error: "the exec ledger has no outcome for this request".into(),
            lost: true,
            ..Default::default()
        }))),
    });
    events
}

/// Spawns the per-subscriber task that turns the ring into a gRPC stream.
fn subscribe<T: Send + 'static>(
    entry: Arc<ProcessEntry>,
    cursor: u64,
    keepalive: Duration,
    kill_on_disconnect: bool,
    deduplicated: bool,
    wrap: fn(ProcessEvent) -> T,
) -> ReceiverStream<Result<T, Status>> {
    let (tx, rx) = mpsc::channel::<Result<T, Status>>(16);
    entry.attached.fetch_add(1, Ordering::SeqCst);
    tokio::spawn(async move {
        let mut cursor = cursor;
        let mut progress = entry.output.subscribe();
        let mut disconnected = tx
            .send(Ok(wrap(ProcessEvent {
                event: Some(Event::Start(entry.start_event(deduplicated))),
            })))
            .await
            .is_err();
        let mut last_sent = tokio::time::Instant::now();
        // When the last output message was small: more output waits until then.
        let mut hold_until: Option<tokio::time::Instant> = None;
        while !disconnected {
            let snapshot = *progress.borrow_and_update();
            let batch = entry.output.read_from(cursor, MAX_DATA_MESSAGE * 4);
            if !batch.chunks.is_empty() {
                let bytes: usize = batch.chunks.iter().map(|c| c.data.len()).sum();
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
                hold_until =
                    (bytes < COALESCE_BELOW).then(|| tokio::time::Instant::now() + COALESCE);
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
                    // Let small writes gather (not past the end of output).
                    if let Some(until) = hold_until.take() {
                        if !progress.borrow().closed {
                            tokio::select! {
                                _ = tokio::time::sleep_until(until) => {}
                                _ = tx.closed() => disconnected = true,
                            }
                        }
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
    services: Option<Arc<services::ServiceManager>>,
}

impl ProcessServiceImpl {
    /// Creates the service over a manager.
    pub fn new(manager: Arc<ProcessManager>) -> Self {
        Self {
            manager,
            services: None,
        }
    }

    /// Serves `PutService` and the other service calls from `services`.
    pub fn with_services(mut self, services: Arc<services::ServiceManager>) -> Self {
        self.services = Some(services);
        self
    }

    fn services(&self) -> Result<&Arc<services::ServiceManager>, Status> {
        self.services.as_ref().ok_or_else(|| {
            crate::error::unsupported(services::FEATURE, "services are not kept by this server")
        })
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
            Started::New(entry) => Ok(Response::new(Box::pin(subscribe(
                entry,
                0,
                keepalive,
                body.kill_on_disconnect,
                false,
                wrap,
            )))),
            // A retry of a known request: replay everything retained from
            // the start, never run it again. A retry never kills the
            // original on disconnect.
            Started::Attached(entry) => Ok(Response::new(Box::pin(subscribe(
                entry, 0, keepalive, false, true, wrap,
            )))),
            Started::Replay(record) => {
                let events: Vec<_> = replay_events(record)
                    .into_iter()
                    .map(|e| Ok(wrap(e)))
                    .collect();
                Ok(Response::new(Box::pin(tokio_stream::iter(events))))
            }
            Started::Failed(end) => {
                let events = vec![
                    Ok(wrap(ProcessEvent {
                        event: Some(Event::Start(ProcessStart {
                            tag: body.tag.clone(),
                            started_at: end.ended_at,
                            request_id: body.request_id.clone(),
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

    async fn get_exec_record(
        &self,
        request: Request<GetExecRecordRequest>,
    ) -> Result<Response<GetExecRecordResponse>, Status> {
        let body = request.into_inner();
        if body.request_id.is_empty() {
            return Err(crate::error::invalid("request_id is required"));
        }
        let record = self.manager.exec_record(&body.request_id)?;
        Ok(Response::new(GetExecRecordResponse {
            record,
            epoch: self.manager.epoch()?,
        }))
    }

    async fn advance_epoch(
        &self,
        request: Request<AdvanceEpochRequest>,
    ) -> Result<Response<AdvanceEpochResponse>, Status> {
        let body = request.into_inner();
        let killed = self.manager.fence(body.epoch)?;
        Ok(Response::new(AdvanceEpochResponse {
            epoch: self.manager.epoch()?,
            killed,
        }))
    }

    async fn put_service(
        &self,
        request: Request<cua_proto::env::v1::PutServiceRequest>,
    ) -> Result<Response<cua_proto::env::v1::PutServiceResponse>, Status> {
        let spec = request
            .into_inner()
            .spec
            .ok_or_else(|| crate::error::invalid("spec is required"))?;
        let service = self.services()?.put(spec).await?;
        Ok(Response::new(cua_proto::env::v1::PutServiceResponse {
            service: Some(service),
        }))
    }

    async fn delete_service(
        &self,
        request: Request<cua_proto::env::v1::DeleteServiceRequest>,
    ) -> Result<Response<cua_proto::env::v1::DeleteServiceResponse>, Status> {
        self.services()?.delete(&request.into_inner().name).await?;
        Ok(Response::new(cua_proto::env::v1::DeleteServiceResponse {}))
    }

    async fn list_services(
        &self,
        _request: Request<cua_proto::env::v1::ListServicesRequest>,
    ) -> Result<Response<cua_proto::env::v1::ListServicesResponse>, Status> {
        Ok(Response::new(cua_proto::env::v1::ListServicesResponse {
            services: self.services()?.list(),
        }))
    }

    async fn control_service(
        &self,
        request: Request<cua_proto::env::v1::ControlServiceRequest>,
    ) -> Result<Response<cua_proto::env::v1::ControlServiceResponse>, Status> {
        let r = request.into_inner();
        let action = cua_proto::env::v1::ServiceAction::try_from(r.action)
            .unwrap_or(cua_proto::env::v1::ServiceAction::Unspecified);
        let service = self.services()?.control(&r.name, action).await?;
        Ok(Response::new(cua_proto::env::v1::ControlServiceResponse {
            service: Some(service),
        }))
    }

    async fn get_service_logs(
        &self,
        request: Request<cua_proto::env::v1::GetServiceLogsRequest>,
    ) -> Result<Response<cua_proto::env::v1::GetServiceLogsResponse>, Status> {
        let r = request.into_inner();
        let (data, truncated) = self.services()?.logs(&r.name, r.tail_bytes)?;
        Ok(Response::new(cua_proto::env::v1::GetServiceLogsResponse {
            data,
            truncated,
        }))
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

    #[cfg(unix)]
    fn manager_in(dir: &std::path::Path) -> Arc<ProcessManager> {
        let config = ServerConfig {
            data_dir: dir.join("data"),
            ..ServerConfig::default()
        };
        ProcessManager::new(ServerContext::new(config, None))
    }

    #[cfg(unix)]
    fn sh(dir: &std::path::Path, line: &str, request_id: &str, epoch: u64) -> StartProcessRequest {
        StartProcessRequest {
            config: Some(ProcessConfig {
                command: "/bin/sh".into(),
                args: vec!["-c".into(), line.into()],
                cwd: dir.display().to_string(),
                ..Default::default()
            }),
            request_id: request_id.into(),
            epoch,
            ..Default::default()
        }
    }

    #[cfg(unix)]
    async fn exited(entry: &Arc<ProcessEntry>) -> ProcessEnd {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            // The ledger record is written before the output closes.
            if entry.output.progress().closed {
                return entry.end_event().expect("closed after the end");
            }
            assert!(Instant::now() < deadline, "process did not exit");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[cfg(unix)]
    fn new_entry(started: Started) -> Arc<ProcessEntry> {
        match started {
            Started::New(entry) => entry,
            _ => panic!("expected a new process"),
        }
    }

    #[cfg(unix)]
    fn replayed(started: Started) -> ExecRecord {
        match started {
            Started::Replay(record) => record,
            _ => panic!("expected a ledger replay"),
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn request_id_runs_once_across_retries_and_restarts() {
        let dir = tempfile::tempdir().unwrap();
        let line = "echo run >> runs.txt; printf hello; printf oops >&2; exit 3";
        let manager = manager_in(dir.path());
        let entry = new_entry(manager.start(&sh(dir.path(), line, "call-1", 0)).unwrap());
        assert_eq!(exited(&entry).await.exit_code, Some(3));

        // A retry while the entry is retained attaches to it.
        match manager.start(&sh(dir.path(), line, "call-1", 0)).unwrap() {
            Started::Attached(again) => assert!(Arc::ptr_eq(&again, &entry)),
            _ => panic!("expected the retained process"),
        }
        let record = manager.exec_record("call-1").unwrap().unwrap();
        assert_eq!(record.state, ExecState::Exited as i32);
        assert_eq!(record.output_bytes, 9);
        assert_eq!(
            record.output_sha256,
            hex::encode(<sha2::Sha256 as sha2::Digest>::digest(b"hellooops"))
        );

        // A new server process (restart) replays the recorded outcome.
        drop(manager);
        let manager = manager_in(dir.path());
        let record = replayed(manager.start(&sh(dir.path(), line, "call-1", 0)).unwrap());
        let events = replay_events(record);
        let Some(Event::Start(start)) = &events[0].event else {
            panic!("start first")
        };
        assert!(start.deduplicated);
        assert_eq!(start.request_id, "call-1");
        let text: Vec<u8> = events
            .iter()
            .filter_map(|e| match &e.event {
                Some(Event::Data(ProcessData {
                    output: Some(DataOutput::Stdout(b) | DataOutput::Stderr(b)),
                    ..
                })) => Some(b.clone()),
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(text, b"hellooops");
        let Some(Event::End(end)) = &events.last().unwrap().event else {
            panic!("end last")
        };
        assert_eq!((end.exit_code, end.lost), (Some(3), false));
        let runs = std::fs::read_to_string(dir.path().join("runs.txt")).unwrap();
        assert_eq!(runs.lines().count(), 1, "the command ran exactly once");

        // An unknown key is absent, and a spawn failure is recorded too.
        assert!(manager.exec_record("never").unwrap().is_none());
        let mut missing = sh(dir.path(), "", "call-2", 0);
        missing.config.as_mut().unwrap().command = "/nonexistent/program".into();
        assert!(matches!(
            manager.start(&missing).unwrap(),
            Started::Failed(_)
        ));
        let record = manager.exec_record("call-2").unwrap().unwrap();
        assert_eq!(record.state, ExecState::Exited as i32);
        assert!(record.end.unwrap().error.contains("not found"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_request_running_when_the_server_stopped_is_lost() {
        let dir = tempfile::tempdir().unwrap();
        let first = manager_in(dir.path());
        let entry = new_entry(
            first
                .start(&sh(dir.path(), "sleep 30", "call-1", 0))
                .unwrap(),
        );
        assert_eq!(
            first.exec_record("call-1").unwrap().unwrap().state,
            ExecState::Running as i32
        );
        // A second server process over the same data dir stands in for a
        // restart: it never saw the process finish.
        let second = manager_in(dir.path());
        let record = second.exec_record("call-1").unwrap().unwrap();
        assert_eq!(record.state, ExecState::Lost as i32);
        assert!(record.end.as_ref().unwrap().lost);
        let replay = replayed(
            second
                .start(&sh(dir.path(), "sleep 30", "call-1", 0))
                .unwrap(),
        );
        assert!(replay.end.unwrap().lost);
        entry.send_signal(Signal::Kill, true).unwrap();
        exited(&entry).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn advancing_the_epoch_fences_older_requests() {
        let dir = tempfile::tempdir().unwrap();
        let manager = manager_in(dir.path());
        let old = new_entry(manager.start(&sh(dir.path(), "sleep 30", "a", 1)).unwrap());
        let unfenced = new_entry(manager.start(&sh(dir.path(), "sleep 30", "", 0)).unwrap());
        assert_eq!(manager.fence(2).unwrap(), 1);
        let end = exited(&old).await;
        assert_eq!(end.signal, Signal::Kill as i32);
        assert!(unfenced.running(), "unfenced processes are not touched");

        let stale = manager
            .start(&sh(dir.path(), "true", "b", 1))
            .map(|_| ())
            .unwrap_err();
        assert_eq!(stale.code(), Code::FailedPrecondition);
        let info = crate::error::error_info(&stale).unwrap();
        assert_eq!(info.reason, ErrorReason::StaleEpoch as i32);
        assert_eq!(info.metadata["current_epoch"], "2");
        assert_eq!(
            manager.fence(1).unwrap_err().code(),
            Code::FailedPrecondition
        );
        // A higher epoch on a start advances it; the epoch survives restarts.
        let next = new_entry(manager.start(&sh(dir.path(), "true", "c", 5)).unwrap());
        exited(&next).await;
        drop(manager);
        assert_eq!(manager_in(dir.path()).epoch().unwrap(), 5);
        unfenced.send_signal(Signal::Kill, true).unwrap();
        exited(&unfenced).await;
    }
}

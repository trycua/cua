// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Services: commands this guest keeps running (`ProcessService.PutService`).
//!
//! Each service is a managed process (tag `svc:<name>`) watched by a
//! supervisor task: when it ends it is started again by its restart policy
//! after a backoff that doubles on quick failures; a health probe that fails
//! `failure_threshold` times in a row ends it, so the policy restarts it.
//! Definitions are kept in `<data_dir>/services/<name>.json`, so cua-spacesd
//! starts them again when it starts (after the guest boots) unless
//! `start_on_boot` is false or a caller stopped them. Output goes to
//! `<data_dir>/services/<name>.log`, rotated at `log_max_bytes` with one
//! older file (`<name>.log.1`) kept.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use cua_proto::env::v1::{
    HealthProbe, ProcessConfig, ProcessEnd, RestartPolicy, ServiceAction, ServiceSpec,
    ServiceState, ServiceStatus, Signal, StartProcessRequest,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tonic::Status;

use super::{ProcessManager, Started};
use crate::util::{duration, timestamp};

/// Tag prefix of a service's processes.
pub const TAG_PREFIX: &str = "svc:";
/// Capability feature.
pub const FEATURE: &str = "process.services";

const DEFAULT_BACKOFF_MIN: Duration = Duration::from_secs(1);
const DEFAULT_BACKOFF_MAX: Duration = Duration::from_secs(60);
/// A run this long resets the backoff.
const STABLE_AFTER: Duration = Duration::from_secs(60);
const DEFAULT_PROBE_INTERVAL: Duration = Duration::from_secs(10);
const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_PROBE_GRACE: Duration = Duration::from_secs(10);
const DEFAULT_FAILURE_THRESHOLD: u32 = 3;
const DEFAULT_LOG_MAX: u64 = 10 * 1024 * 1024;
const DEFAULT_TAIL: u64 = 64 * 1024;
const MAX_TAIL: u64 = 4 * 1024 * 1024;
/// How long a stopped process gets after SIGTERM before SIGKILL.
const STOP_GRACE: Duration = Duration::from_secs(10);
/// Most services one guest keeps.
pub const MAX_SERVICES: usize = 64;

/// What is kept on disk for one service.
#[derive(Serialize, Deserialize)]
struct Record {
    /// The spec, as protobuf JSON-free bytes (prost) in base64.
    spec: String,
    /// A caller stopped it: not started on boot until started again.
    #[serde(default)]
    stopped: bool,
}

/// Live state of one service.
struct Unit {
    spec: ServiceSpec,
    state: Mutex<Live>,
    stop: CancellationToken,
    /// Woken when a run ends or the unit is told to stop.
    changed: Notify,
}

#[derive(Clone)]
struct Live {
    state: ServiceState,
    pid: u32,
    restarts: u32,
    started_at: Option<SystemTime>,
    last_end: Option<ProcessEnd>,
    probe_error: String,
    next_start_at: Option<SystemTime>,
}

impl Default for Live {
    fn default() -> Self {
        Self {
            state: ServiceState::Starting,
            pid: 0,
            restarts: 0,
            started_at: None,
            last_end: None,
            probe_error: String::new(),
            next_start_at: None,
        }
    }
}

/// The guest's services.
pub struct ServiceManager {
    processes: Arc<ProcessManager>,
    dir: PathBuf,
    units: Mutex<BTreeMap<String, Arc<Unit>>>,
}

/// Checks a service name.
pub fn valid_name(name: &str) -> bool {
    let b = name.as_bytes();
    !b.is_empty()
        && b.len() <= 63
        && b[0].is_ascii_alphanumeric()
        && b.iter().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'-' | b'_' | b'.')
        })
}

fn b64(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn unb64(s: &str) -> Option<Vec<u8>> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.decode(s).ok()
}

fn invalid(m: impl Into<String>) -> Status {
    crate::error::invalid(m.into())
}

fn not_found(name: &str) -> Status {
    crate::error::status(
        tonic::Code::NotFound,
        cua_proto::env::v1::ErrorReason::Unspecified,
        format!("no service {name:?}"),
    )
}

impl ServiceManager {
    /// Opens `<data_dir>/services` and starts every service that starts on
    /// boot and was not stopped.
    pub fn start(processes: Arc<ProcessManager>, data_dir: &Path) -> Arc<Self> {
        let dir = data_dir.join("services");
        let manager = Arc::new(Self {
            processes,
            dir,
            units: Mutex::new(BTreeMap::new()),
        });
        for (spec, stopped) in manager.load() {
            let on_boot = spec.start_on_boot.unwrap_or(true);
            let unit = manager.insert(spec);
            if on_boot && !stopped {
                manager.spawn_supervisor(unit);
            } else {
                unit.state.lock().expect("unit").state = ServiceState::Stopped;
            }
        }
        manager
    }

    fn load(&self) -> Vec<(ServiceSpec, bool)> {
        let Ok(read) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for e in read.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let Some(record) = std::fs::read(&p)
                .ok()
                .and_then(|b| serde_json::from_slice::<Record>(&b).ok())
            else {
                tracing::warn!(path = %p.display(), "unreadable service record");
                continue;
            };
            let Some(spec) = unb64(&record.spec)
                .and_then(|b| <ServiceSpec as prost::Message>::decode(b.as_slice()).ok())
            else {
                continue;
            };
            if valid_name(&spec.name) {
                out.push((spec, record.stopped));
            }
        }
        out
    }

    fn save(&self, spec: &ServiceSpec, stopped: bool) -> Result<(), Status> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| crate::error::internal(format!("services dir: {e}")))?;
        let record = Record {
            spec: b64(&prost::Message::encode_to_vec(spec)),
            stopped,
        };
        let path = self.dir.join(format!("{}.json", spec.name));
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(&record).unwrap_or_default())
            .and_then(|()| std::fs::rename(&tmp, &path))
            .map_err(|e| crate::error::internal(format!("save service: {e}")))
    }

    fn log_path(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.log"))
    }

    fn insert(&self, spec: ServiceSpec) -> Arc<Unit> {
        let unit = Arc::new(Unit {
            spec: spec.clone(),
            state: Mutex::new(Live::default()),
            stop: CancellationToken::new(),
            changed: Notify::new(),
        });
        self.units
            .lock()
            .expect("units")
            .insert(spec.name.clone(), unit.clone());
        unit
    }

    fn unit(&self, name: &str) -> Result<Arc<Unit>, Status> {
        self.units
            .lock()
            .expect("units")
            .get(name)
            .cloned()
            .ok_or_else(|| not_found(name))
    }

    fn status_of(&self, unit: &Unit) -> ServiceStatus {
        let live = unit.state.lock().expect("unit").clone();
        ServiceStatus {
            spec: Some(unit.spec.clone()),
            state: live.state as i32,
            pid: live.pid,
            restarts: live.restarts,
            started_at: live.started_at.map(timestamp),
            last_end: live.last_end,
            probe_error: live.probe_error,
            next_start_at: live.next_start_at.map(timestamp),
            log_path: self.log_path(&unit.spec.name).display().to_string(),
        }
    }

    /// Checks a spec and fills its defaults.
    fn check(spec: &ServiceSpec) -> Result<(), Status> {
        if !valid_name(&spec.name) {
            return Err(invalid(format!(
                "service name {:?}: use 1 to 63 lowercase letters, digits, '-', '_' or '.', \
                 starting with a letter or digit",
                spec.name
            )));
        }
        spec.config
            .as_ref()
            .filter(|c| !c.command.trim().is_empty())
            .ok_or_else(|| invalid("a service needs a command"))?;
        if let Some(p) = &spec.probe {
            let set = [p.exec.is_some(), !p.http.is_empty(), !p.tcp.is_empty()]
                .iter()
                .filter(|s| **s)
                .count();
            if set != 1 {
                return Err(invalid(
                    "a health probe checks one thing: set exactly one of exec, http or tcp",
                ));
            }
            if !p.http.is_empty()
                && !p.http.starts_with("http://")
                && !p.http.starts_with("https://")
            {
                return Err(invalid("probe http must be an http(s):// URL"));
            }
        }
        Ok(())
    }

    /// `PutService`.
    pub async fn put(self: &Arc<Self>, spec: ServiceSpec) -> Result<ServiceStatus, Status> {
        Self::check(&spec)?;
        let replaced = self.units.lock().expect("units").get(&spec.name).cloned();
        if replaced.is_none() && self.units.lock().expect("units").len() >= MAX_SERVICES {
            return Err(crate::error::status(
                tonic::Code::ResourceExhausted,
                cua_proto::env::v1::ErrorReason::Unspecified,
                format!("at most {MAX_SERVICES} services"),
            ));
        }
        if let Some(old) = replaced {
            self.halt(&old).await;
        }
        self.save(&spec, false)?;
        let unit = self.insert(spec);
        self.spawn_supervisor(unit.clone());
        Ok(self.status_of(&unit))
    }

    /// `DeleteService`.
    pub async fn delete(&self, name: &str) -> Result<(), Status> {
        let unit = self.unit(name)?;
        self.halt(&unit).await;
        self.units.lock().expect("units").remove(name);
        let _ = std::fs::remove_file(self.dir.join(format!("{name}.json")));
        let _ = std::fs::remove_file(self.log_path(name));
        let _ = std::fs::remove_file(self.dir.join(format!("{name}.log.1")));
        Ok(())
    }

    /// `ListServices`.
    pub fn list(&self) -> Vec<ServiceStatus> {
        let units: Vec<Arc<Unit>> = self
            .units
            .lock()
            .expect("units")
            .values()
            .cloned()
            .collect();
        units.iter().map(|u| self.status_of(u)).collect()
    }

    /// `ControlService`.
    pub async fn control(
        self: &Arc<Self>,
        name: &str,
        action: ServiceAction,
    ) -> Result<ServiceStatus, Status> {
        let unit = self.unit(name)?;
        match action {
            ServiceAction::Unspecified => return Err(invalid("action is required")),
            ServiceAction::Stop => {
                self.halt(&unit).await;
                self.save(&unit.spec, true)?;
                unit.state.lock().expect("unit").state = ServiceState::Stopped;
                Ok(self.status_of(&unit))
            }
            ServiceAction::Start | ServiceAction::Restart => {
                let running = matches!(
                    unit.state.lock().expect("unit").state,
                    ServiceState::Starting
                        | ServiceState::Running
                        | ServiceState::Unhealthy
                        | ServiceState::Backoff
                ) && !unit.stop.is_cancelled();
                if running && action == ServiceAction::Start {
                    return Ok(self.status_of(&unit));
                }
                self.halt(&unit).await;
                self.save(&unit.spec, false)?;
                let restarts = unit.state.lock().expect("unit").restarts;
                let fresh = self.insert(unit.spec.clone());
                fresh.state.lock().expect("unit").restarts = restarts;
                self.spawn_supervisor(fresh.clone());
                Ok(self.status_of(&fresh))
            }
        }
    }

    /// `GetServiceLogs`: the last `tail` bytes of the log (and of the older
    /// file when the current one is shorter).
    pub fn logs(&self, name: &str, tail: u64) -> Result<(Vec<u8>, bool), Status> {
        self.unit(name)?;
        let tail = match tail {
            0 => DEFAULT_TAIL,
            n => n.min(MAX_TAIL),
        };
        let current = std::fs::read(self.log_path(name)).unwrap_or_default();
        let mut data = Vec::new();
        if (current.len() as u64) < tail {
            let older = std::fs::read(self.dir.join(format!("{name}.log.1"))).unwrap_or_default();
            let need = (tail - current.len() as u64) as usize;
            let from = older.len().saturating_sub(need);
            data.extend_from_slice(&older[from..]);
        }
        data.extend_from_slice(&current);
        let truncated = data.len() as u64 > tail;
        if truncated {
            data.drain(..data.len() - tail as usize);
        }
        Ok((data, truncated))
    }

    /// Stops a unit's supervisor and its process (SIGTERM to its group,
    /// then SIGKILL after the grace).
    async fn halt(&self, unit: &Unit) {
        unit.stop.cancel();
        unit.changed.notify_waiters();
        let tag = format!("{TAG_PREFIX}{}", unit.spec.name);
        for p in self.processes.running_with_tag(&tag) {
            let _ = p.signal_group(Signal::Term);
        }
        let deadline = Instant::now() + STOP_GRACE;
        while Instant::now() < deadline && !self.processes.running_with_tag(&tag).is_empty() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        for p in self.processes.running_with_tag(&tag) {
            let _ = p.signal_group(Signal::Kill);
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && !self.processes.running_with_tag(&tag).is_empty() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    fn spawn_supervisor(self: &Arc<Self>, unit: Arc<Unit>) {
        let manager = self.clone();
        tokio::spawn(async move { manager.supervise(unit).await });
    }

    async fn supervise(self: Arc<Self>, unit: Arc<Unit>) {
        let spec = unit.spec.clone();
        let policy = RestartPolicy::try_from(spec.restart).unwrap_or(RestartPolicy::Always);
        let min = duration(spec.backoff_min.as_ref())
            .filter(|d| !d.is_zero())
            .unwrap_or(DEFAULT_BACKOFF_MIN);
        let max = duration(spec.backoff_max.as_ref())
            .filter(|d| !d.is_zero())
            .unwrap_or(DEFAULT_BACKOFF_MAX)
            .max(min);
        let mut backoff = min;
        let mut first = true;
        loop {
            if unit.stop.is_cancelled() {
                return;
            }
            if !first {
                unit.state.lock().expect("unit").restarts += 1;
            }
            first = false;
            let request = StartProcessRequest {
                config: spec.config.clone().map(|mut c| {
                    c.timeout = None;
                    c
                }),
                tag: format!("{TAG_PREFIX}{}", spec.name),
                ..Default::default()
            };
            let entry = match self.processes.start(&request) {
                Ok(Started::New(e)) => e,
                Ok(Started::Failed(end)) => {
                    {
                        let mut live = unit.state.lock().expect("unit");
                        live.state = ServiceState::Failed;
                        live.last_end = Some(end.clone());
                    }
                    self.append_log(
                        &spec,
                        format!("[cua] could not start: {}\n", end.error).as_bytes(),
                    );
                    if !self
                        .wait_backoff(&unit, policy, &end, &mut backoff, max, Duration::ZERO)
                        .await
                    {
                        return;
                    }
                    continue;
                }
                Ok(_) => return,
                Err(status) => {
                    {
                        let mut live = unit.state.lock().expect("unit");
                        live.state = ServiceState::Failed;
                        live.probe_error = status.message().to_owned();
                    }
                    return;
                }
            };
            let started = Instant::now();
            {
                let mut live = unit.state.lock().expect("unit");
                live.state = if spec.probe.is_some() {
                    ServiceState::Starting
                } else {
                    ServiceState::Running
                };
                live.pid = entry.pid();
                live.started_at = Some(SystemTime::now());
                live.next_start_at = None;
                live.probe_error.clear();
            }
            let logs = {
                let (manager, entry, spec) = (self.clone(), entry.clone(), spec.clone());
                tokio::spawn(async move { manager.pump_logs(&spec, &entry).await })
            };
            let probe = spec.probe.clone().map(|p| {
                let (unit, entry) = (unit.clone(), entry.clone());
                tokio::spawn(async move { probe_loop(&unit, &entry, p).await })
            });
            let end = tokio::select! {
                end = entry.ended() => Some(end),
                _ = unit.stop.cancelled() => None,
            };
            if let Some(p) = probe {
                p.abort();
            }
            let Some(end) = end else {
                // Stopped by a caller: `halt` ends the process.
                let _ = tokio::time::timeout(Duration::from_secs(20), entry.ended()).await;
                let _ = logs.await;
                return;
            };
            let _ = logs.await;
            {
                let mut live = unit.state.lock().expect("unit");
                live.pid = 0;
                live.last_end = Some(end.clone());
            }
            self.append_log(&spec, describe_end(&end).as_bytes());
            if !self
                .wait_backoff(&unit, policy, &end, &mut backoff, max, started.elapsed())
                .await
            {
                return;
            }
        }
    }

    /// After a run: whether to start again (after the backoff wait).
    async fn wait_backoff(
        &self,
        unit: &Unit,
        policy: RestartPolicy,
        end: &ProcessEnd,
        backoff: &mut Duration,
        max: Duration,
        ran: Duration,
    ) -> bool {
        let failed = end.exit_code != Some(0) || end.signal != 0 || !end.error.is_empty();
        let again = match policy {
            RestartPolicy::Always | RestartPolicy::Unspecified => true,
            RestartPolicy::OnFailure => failed,
            RestartPolicy::Never => false,
        };
        if !again || unit.stop.is_cancelled() {
            let mut live = unit.state.lock().expect("unit");
            if live.state != ServiceState::Failed {
                live.state = ServiceState::Exited;
            }
            return false;
        }
        if ran >= STABLE_AFTER {
            *backoff = DEFAULT_BACKOFF_MIN.min(*backoff);
        }
        let wait = *backoff;
        *backoff = (*backoff * 2).min(max);
        {
            let mut live = unit.state.lock().expect("unit");
            live.state = ServiceState::Backoff;
            live.next_start_at = Some(SystemTime::now() + wait);
        }
        tokio::select! {
            _ = tokio::time::sleep(wait) => !unit.stop.is_cancelled(),
            _ = unit.stop.cancelled() => false,
        }
    }

    /// Copies a run's output into the log file as it arrives.
    async fn pump_logs(&self, spec: &ServiceSpec, entry: &super::ProcessEntry) {
        let output = entry.output().clone();
        let mut progress = output.subscribe();
        let mut next = output.bounds().0;
        loop {
            let batch = output.read_from(next, 256 * 1024);
            if batch.next < next {
                next = batch.next;
            }
            for c in &batch.chunks {
                self.append_log(spec, &c.data);
            }
            let advanced = batch.next != next;
            next = batch.next;
            if advanced {
                continue;
            }
            if progress.borrow().closed && next >= output.bounds().1 {
                return;
            }
            if progress.changed().await.is_err() {
                return;
            }
        }
    }

    fn append_log(&self, spec: &ServiceSpec, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        let max = match spec.log_max_bytes {
            0 => DEFAULT_LOG_MAX,
            n => n,
        };
        let path = self.log_path(&spec.name);
        let _ = std::fs::create_dir_all(&self.dir);
        if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) + data.len() as u64 > max {
            let _ = std::fs::rename(&path, self.dir.join(format!("{}.log.1", spec.name)));
        }
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let _ = f.write_all(data);
        }
    }
}

fn describe_end(end: &ProcessEnd) -> String {
    if !end.error.is_empty() {
        format!("[cua] ended: {}\n", end.error)
    } else if let Some(code) = end.exit_code {
        format!("[cua] exited with code {code}\n")
    } else {
        format!("[cua] ended by signal {}\n", end.signal_number)
    }
}

/// Checks a run's health until it ends; `threshold` failures in a row end
/// the run (the policy then restarts it).
async fn probe_loop(unit: &Unit, entry: &super::ProcessEntry, probe: HealthProbe) {
    let interval = duration(probe.interval.as_ref())
        .filter(|d| !d.is_zero())
        .unwrap_or(DEFAULT_PROBE_INTERVAL);
    let timeout = duration(probe.timeout.as_ref())
        .filter(|d| !d.is_zero())
        .unwrap_or(DEFAULT_PROBE_TIMEOUT);
    let grace = duration(probe.start_period.as_ref()).unwrap_or(DEFAULT_PROBE_GRACE);
    let threshold = match probe.failure_threshold {
        0 => DEFAULT_FAILURE_THRESHOLD,
        n => n,
    };
    let started = Instant::now();
    let mut failures = 0u32;
    loop {
        tokio::time::sleep(interval.min(Duration::from_secs(3600))).await;
        let result = tokio::time::timeout(timeout, check(&probe))
            .await
            .unwrap_or_else(|_| Err(format!("no answer within {}s", timeout.as_secs_f32())));
        let give_up = {
            let mut live = unit.state.lock().expect("unit");
            match result {
                Ok(()) => {
                    failures = 0;
                    live.probe_error.clear();
                    live.state = ServiceState::Running;
                    false
                }
                Err(e) => {
                    live.probe_error = e;
                    if started.elapsed() >= grace {
                        failures += 1;
                        live.state = ServiceState::Unhealthy;
                    }
                    failures >= threshold
                }
            }
        };
        if give_up {
            tracing::warn!(service = %unit.spec.name, "health check failed {threshold} times; restarting");
            let _ = entry.signal_group(Signal::Term);
            tokio::time::sleep(STOP_GRACE).await;
            let _ = entry.signal_group(Signal::Kill);
            return;
        }
    }
}

/// One health check.
async fn check(probe: &HealthProbe) -> Result<(), String> {
    if let Some(cmd) = &probe.exec {
        return exec_probe(cmd).await;
    }
    if !probe.tcp.is_empty() {
        return tokio::net::TcpStream::connect(probe.tcp.as_str())
            .await
            .map(|_| ())
            .map_err(|e| format!("tcp {}: {e}", probe.tcp));
    }
    http_probe(&probe.http).await
}

async fn exec_probe(cmd: &ProcessConfig) -> Result<(), String> {
    let mut c = tokio::process::Command::new(&cmd.command);
    c.args(&cmd.args).envs(&cmd.env).kill_on_drop(true);
    if !cmd.cwd.is_empty() {
        c.current_dir(&cmd.cwd);
    }
    let out = c
        .output()
        .await
        .map_err(|e| format!("{}: {e}", cmd.command))?;
    if out.status.success() {
        Ok(())
    } else {
        let tail = String::from_utf8_lossy(&out.stderr);
        Err(format!(
            "exited with {}{}",
            out.status,
            tail.lines()
                .last()
                .map(|l| format!(": {l}"))
                .unwrap_or_default()
        ))
    }
}

async fn http_probe(url: &str) -> Result<(), String> {
    use http_body_util::Empty;
    use hyper_util::client::legacy::Client;
    use hyper_util::rt::TokioExecutor;
    let uri: http::Uri = url.parse().map_err(|e| format!("{url}: {e}"))?;
    if uri.scheme_str() != Some("http") {
        return Err("only http:// probes are checked here".into());
    }
    let client: Client<_, Empty<bytes::Bytes>> = Client::builder(TokioExecutor::new()).build_http();
    let resp = client.get(uri).await.map_err(|e| format!("{url}: {e}"))?;
    let code = resp.status();
    if code.is_success() || code.is_redirection() {
        Ok(())
    } else {
        Err(format!("{url}: {code}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_checked() {
        for ok in ["web", "a", "api-2", "db_main", "x.y"] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in ["", "-a", "Web", "a b", "a/b", &"a".repeat(64)] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn a_probe_checks_one_thing() {
        let mut spec = ServiceSpec {
            name: "web".into(),
            config: Some(ProcessConfig {
                command: "true".into(),
                ..Default::default()
            }),
            probe: Some(HealthProbe {
                tcp: "127.0.0.1:1".into(),
                http: "http://127.0.0.1:1/".into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(ServiceManager::check(&spec).is_err());
        spec.probe.as_mut().unwrap().http.clear();
        assert!(ServiceManager::check(&spec).is_ok());
        spec.config = None;
        assert!(ServiceManager::check(&spec).is_err());
    }
}

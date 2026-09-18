//! Bounded, single-use IPC client for the optional perception worker.

pub mod containment;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::process::{ChildStdin, ChildStdout};
use tokio::sync::{Mutex as AsyncMutex, Notify};
use uuid::Uuid;

use cua_driver_contract::{VisualParseError, VisualParseErrorCode};

use containment::ContainedChild;
#[cfg(test)]
use containment::ContainmentLimits;

const PROTOCOL_VERSION: &str = "cua-perception/1";
const DEFAULT_MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct PerceptionWorkerConfig {
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub request_timeout: Duration,
    pub max_frame_bytes: usize,
    pub warm_worker: Option<WarmWorkerPolicy>,
    #[cfg(test)]
    containment: ContainmentLimits,
}

impl PerceptionWorkerConfig {
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            args: Vec::new(),
            request_timeout: Duration::from_secs(30),
            max_frame_bytes: DEFAULT_MAX_FRAME_BYTES,
            warm_worker: None,
            #[cfg(test)]
            containment: ContainmentLimits::default(),
        }
    }

    pub fn with_bounded_reuse(mut self, policy: WarmWorkerPolicy) -> Self {
        self.warm_worker = Some(policy);
        self
    }

    /// Configuration for an installed extension where model startup is
    /// expensive. Reuse remains bounded by [`WarmWorkerPolicy::default`].
    pub fn installed(executable: impl Into<PathBuf>) -> Self {
        Self::new(executable).with_bounded_reuse(WarmWorkerPolicy::default())
    }

    fn containment_limits(&self) -> containment::ContainmentLimits {
        #[cfg(test)]
        {
            self.containment.clone()
        }
        #[cfg(not(test))]
        {
            containment::ContainmentLimits::default()
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct WarmWorkerPolicy {
    pub startup_timeout: Duration,
    pub inference_timeout: Duration,
    pub shutdown_timeout: Duration,
    pub idle_ttl: Duration,
}

impl Default for WarmWorkerPolicy {
    fn default() -> Self {
        Self {
            startup_timeout: Duration::from_secs(15),
            inference_timeout: Duration::from_secs(30),
            shutdown_timeout: Duration::from_secs(2),
            idle_ttl: Duration::from_secs(30),
        }
    }
}

#[derive(Clone, Default)]
pub struct PerceptionCancellation {
    cancelled: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

impl PerceptionCancellation {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    async fn cancelled(&self) {
        if self.is_cancelled() {
            return;
        }
        self.notify.notified().await;
    }
}

#[derive(Default)]
struct PerceptionState {
    worker: AsyncMutex<Option<WarmWorker>>,
    cancellation_epoch: AtomicU64,
    cancellation_notify: Notify,
}

impl Drop for PerceptionState {
    fn drop(&mut self) {
        self.worker.get_mut().take();
    }
}

#[derive(Clone, Default)]
pub struct PerceptionClient {
    config: Option<Arc<PerceptionWorkerConfig>>,
    state: Arc<PerceptionState>,
}

impl PerceptionClient {
    pub fn unavailable() -> Self {
        Self {
            config: None,
            state: Arc::new(PerceptionState::default()),
        }
    }

    pub fn new(config: PerceptionWorkerConfig) -> Result<Self, VisualParseError> {
        if config.request_timeout.is_zero()
            || config.max_frame_bytes == 0
            || config.max_frame_bytes > u32::MAX as usize
            || config.warm_worker.is_some_and(|policy| {
                policy.startup_timeout.is_zero()
                    || policy.inference_timeout.is_zero()
                    || policy.shutdown_timeout.is_zero()
                    || policy.idle_ttl.is_zero()
            })
        {
            return Err(error(
                VisualParseErrorCode::ResourceLimitExceeded,
                "perception worker limits must be non-zero and fit the framed protocol",
                false,
                None,
            ));
        }
        config.containment_limits().validate()?;
        Ok(Self {
            config: Some(Arc::new(config)),
            state: Arc::new(PerceptionState::default()),
        })
    }

    pub fn is_available(&self) -> bool {
        self.config.is_some()
    }

    /// Cancel any in-flight request and synchronously drop an idle worker.
    pub fn shutdown_now(&self) {
        self.state.cancellation_epoch.fetch_add(1, Ordering::AcqRel);
        self.state.cancellation_notify.notify_waiters();
        if let Ok(mut worker) = self.state.worker.try_lock() {
            worker.take();
        }
    }

    /// Launch one worker process and issue exactly one parse. Failures are
    /// returned to the caller without replaying the request.
    pub async fn parse(
        &self,
        capture_id: &str,
        width: u32,
        height: u32,
        png_bytes: &[u8],
        cancellation: &PerceptionCancellation,
    ) -> Result<Value, VisualParseError> {
        let config = self.config.as_ref().ok_or_else(|| {
            error(
                VisualParseErrorCode::NotInstalled,
                "the optional cua-perception extension is not installed",
                false,
                None,
            )
        })?;
        if cancellation.is_cancelled() {
            return Err(cancelled_error());
        }

        if let Some(policy) = config.warm_worker {
            return self
                .parse_warm(
                    config,
                    policy,
                    capture_id,
                    width,
                    height,
                    png_bytes,
                    cancellation,
                )
                .await;
        }

        let operation = self.parse_cold(config, capture_id, width, height, png_bytes);
        tokio::select! {
            _ = cancellation.cancelled() => Err(cancelled_error()),
            result = tokio::time::timeout(config.request_timeout, operation) => {
                match result {
                    Ok(result) => result,
                    Err(_) => Err(error(
                        VisualParseErrorCode::Timeout,
                        "the perception worker exceeded its request deadline",
                        true,
                        None,
                    )),
                }
            }
        }
    }

    async fn parse_cold(
        &self,
        config: &PerceptionWorkerConfig,
        capture_id: &str,
        width: u32,
        height: u32,
        png_bytes: &[u8],
    ) -> Result<Value, VisualParseError> {
        let mut worker = WarmWorker::launch(config).await?;
        let result = worker
            .parse(config, capture_id, width, height, png_bytes)
            .await?;
        worker.stdin.shutdown().await.map_err(map_io_error)?;
        let status = worker
            .contained
            .child
            .wait()
            .await
            .map_err(map_crash_error)?;
        if !status.success() {
            return Err(error(
                VisualParseErrorCode::WorkerCrashed,
                "perception worker exited unsuccessfully after responding",
                true,
                Some(status.to_string()),
            ));
        }
        Ok(result)
    }

    async fn parse_warm(
        &self,
        config: &Arc<PerceptionWorkerConfig>,
        policy: WarmWorkerPolicy,
        capture_id: &str,
        width: u32,
        height: u32,
        png_bytes: &[u8],
        cancellation: &PerceptionCancellation,
    ) -> Result<Value, VisualParseError> {
        let epoch = self.state.cancellation_epoch.load(Ordering::Acquire);
        let mut slot = tokio::select! {
            _ = cancellation.cancelled() => return Err(cancelled_error()),
            _ = runtime_cancelled(&self.state, epoch) => return Err(cancelled_error()),
            slot = self.state.worker.lock() => slot,
        };

        if slot
            .as_ref()
            .is_some_and(|worker| worker.last_used.elapsed() >= policy.idle_ttl)
        {
            if let Some(worker) = slot.take() {
                shutdown_worker(worker, policy.shutdown_timeout).await;
            }
        }

        if slot.is_none() {
            let launch = WarmWorker::launch(config);
            let worker = tokio::select! {
                _ = cancellation.cancelled() => return Err(cancelled_error()),
                _ = runtime_cancelled(&self.state, epoch) => return Err(cancelled_error()),
                result = tokio::time::timeout(policy.startup_timeout, launch) => {
                    match result {
                        Ok(result) => result?,
                        Err(_) => return Err(timeout_error("startup")),
                    }
                }
            };
            *slot = Some(worker);
        }

        let operation = slot
            .as_mut()
            .expect("warm worker was initialized")
            .parse(config, capture_id, width, height, png_bytes);
        let result = tokio::select! {
            _ = cancellation.cancelled() => Err(cancelled_error()),
            _ = runtime_cancelled(&self.state, epoch) => Err(cancelled_error()),
            result = tokio::time::timeout(policy.inference_timeout, operation) => {
                match result {
                    Ok(result) => result,
                    Err(_) => Err(timeout_error("inference")),
                }
            }
        };
        let result = match result {
            Ok(result) => result,
            Err(failure) => {
                slot.take();
                return Err(failure);
            }
        };

        let worker = slot.as_mut().expect("successful worker remains present");
        worker.last_used = Instant::now();
        worker.idle_generation = worker.idle_generation.wrapping_add(1);
        let idle_generation = worker.idle_generation;
        drop(slot);
        schedule_idle_shutdown(
            Arc::downgrade(&self.state),
            idle_generation,
            policy.idle_ttl,
            policy.shutdown_timeout,
        );
        Ok(result)
    }
}

struct WarmWorker {
    contained: ContainedChild,
    stdin: ChildStdin,
    stdout: ChildStdout,
    _working_directory: tempfile::TempDir,
    last_used: Instant,
    idle_generation: u64,
}

impl WarmWorker {
    async fn launch(config: &PerceptionWorkerConfig) -> Result<Self, VisualParseError> {
        let working_directory = tempfile::Builder::new()
            .prefix("cua-perception-")
            .tempdir()
            .map_err(|cause| {
                error(
                    VisualParseErrorCode::WorkerLaunchFailed,
                    "failed to create a private perception worker directory",
                    true,
                    Some(cause.to_string()),
                )
            })?;
        let limits = config.containment_limits();
        let mut contained = containment::spawn(
            &config.executable,
            &config.args,
            working_directory.path(),
            &limits,
        )?;
        let child = &mut contained.child;
        let mut stdin = child.stdin.take().ok_or_else(|| {
            error(
                VisualParseErrorCode::WorkerLaunchFailed,
                "perception worker stdin was unavailable",
                true,
                None,
            )
        })?;
        let mut stdout = child.stdout.take().ok_or_else(|| {
            error(
                VisualParseErrorCode::WorkerLaunchFailed,
                "perception worker stdout was unavailable",
                true,
                None,
            )
        })?;
        let health_id = format!("health-{}", Uuid::new_v4());
        write_json_frame(
            &mut stdin,
            &json!({"protocol": PROTOCOL_VERSION, "request_id": health_id, "method": "health", "params": {}}),
            config.max_frame_bytes,
        )
        .await?;
        let health = read_response(&mut stdout, config.max_frame_bytes).await?;
        let health_result = health.into_result(&health_id)?;
        if health_result.get("ready").and_then(Value::as_bool) != Some(true)
            || health_result.get("protocol").and_then(Value::as_str) != Some(PROTOCOL_VERSION)
        {
            return Err(error(
                VisualParseErrorCode::IncompatibleProtocol,
                "perception worker health response is incompatible",
                false,
                None,
            ));
        }
        Ok(Self {
            contained,
            stdin,
            stdout,
            _working_directory: working_directory,
            last_used: Instant::now(),
            idle_generation: 0,
        })
    }

    async fn parse(
        &mut self,
        config: &PerceptionWorkerConfig,
        capture_id: &str,
        width: u32,
        height: u32,
        png_bytes: &[u8],
    ) -> Result<Value, VisualParseError> {
        use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};

        let request_id = format!("parse-{}", Uuid::new_v4());
        write_json_frame(
            &mut self.stdin,
            &json!({
                "protocol": PROTOCOL_VERSION,
                "request_id": request_id,
                "method": "parse",
                "params": {
                    "capture_id": capture_id,
                    "image": {
                        "media_type": "image/png",
                        "width": width,
                        "height": height,
                        "byte_length": png_bytes.len(),
                        "data_base64": BASE64.encode(png_bytes),
                    }
                }
            }),
            config.max_frame_bytes,
        )
        .await?;
        read_response(&mut self.stdout, config.max_frame_bytes)
            .await?
            .into_result(&request_id)
    }
}

async fn runtime_cancelled(state: &PerceptionState, epoch: u64) {
    loop {
        let notified = state.cancellation_notify.notified();
        if state.cancellation_epoch.load(Ordering::Acquire) != epoch {
            return;
        }
        notified.await;
    }
}

fn schedule_idle_shutdown(
    state: std::sync::Weak<PerceptionState>,
    generation: u64,
    idle_ttl: Duration,
    shutdown_timeout: Duration,
) {
    tokio::spawn(async move {
        tokio::time::sleep(idle_ttl).await;
        let Some(state) = state.upgrade() else {
            return;
        };
        let mut slot = state.worker.lock().await;
        let should_shutdown = slot.as_ref().is_some_and(|worker| {
            worker.idle_generation == generation && worker.last_used.elapsed() >= idle_ttl
        });
        if should_shutdown {
            if let Some(worker) = slot.take() {
                shutdown_worker(worker, shutdown_timeout).await;
            }
        }
    });
}

async fn shutdown_worker(mut worker: WarmWorker, timeout: Duration) {
    let operation = async {
        let _ = worker.stdin.shutdown().await;
        let _ = worker.contained.child.wait().await;
    };
    let _ = tokio::time::timeout(timeout, operation).await;
}

#[derive(Deserialize)]
struct WorkerResponse {
    protocol: String,
    request_id: Option<String>,
    status: String,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<WorkerError>,
}

#[derive(Deserialize)]
struct WorkerError {
    code: String,
    message: String,
    #[serde(default)]
    details: Option<Value>,
}

impl WorkerResponse {
    fn into_result(self, expected_request_id: &str) -> Result<Value, VisualParseError> {
        if self.protocol != PROTOCOL_VERSION {
            return Err(error(
                VisualParseErrorCode::IncompatibleProtocol,
                "perception worker returned an incompatible protocol version",
                false,
                Some(self.protocol),
            ));
        }
        if self.request_id.as_deref() != Some(expected_request_id) {
            return Err(error(
                VisualParseErrorCode::InvalidFrame,
                "perception worker response request_id did not match",
                false,
                self.request_id,
            ));
        }
        match self.status.as_str() {
            "ok" => self.result.ok_or_else(|| {
                error(
                    VisualParseErrorCode::InvalidFrame,
                    "perception worker success omitted its result",
                    false,
                    None,
                )
            }),
            "error" => {
                let worker = self.error.ok_or_else(|| {
                    error(
                        VisualParseErrorCode::InvalidFrame,
                        "perception worker error omitted its payload",
                        false,
                        None,
                    )
                })?;
                Err(map_worker_error(worker))
            }
            _ => Err(error(
                VisualParseErrorCode::InvalidFrame,
                "perception worker returned an unknown status",
                false,
                Some(self.status),
            )),
        }
    }
}

async fn write_json_frame(
    writer: &mut (impl AsyncWrite + Unpin),
    value: &Value,
    maximum: usize,
) -> Result<(), VisualParseError> {
    let payload = serde_json::to_vec(value).map_err(|cause| {
        error(
            VisualParseErrorCode::ArtifactInvalid,
            "failed to serialize the perception request",
            false,
            Some(cause.to_string()),
        )
    })?;
    if payload.is_empty() || payload.len() > maximum {
        return Err(error(
            VisualParseErrorCode::ResourceLimitExceeded,
            "perception request exceeds the framed IPC limit",
            false,
            Some(payload.len().to_string()),
        ));
    }
    writer
        .write_all(&(payload.len() as u32).to_be_bytes())
        .await
        .map_err(map_io_error)?;
    writer.write_all(&payload).await.map_err(map_io_error)?;
    writer.flush().await.map_err(map_io_error)
}

async fn read_response(
    reader: &mut (impl AsyncRead + Unpin),
    maximum: usize,
) -> Result<WorkerResponse, VisualParseError> {
    let mut prefix = [0_u8; 4];
    reader
        .read_exact(&mut prefix)
        .await
        .map_err(map_crash_error)?;
    let declared = u32::from_be_bytes(prefix) as usize;
    if declared == 0 || declared > maximum {
        return Err(error(
            VisualParseErrorCode::InvalidFrame,
            "perception worker returned an invalid frame length",
            false,
            Some(declared.to_string()),
        ));
    }
    let mut payload = vec![0_u8; declared];
    reader
        .read_exact(&mut payload)
        .await
        .map_err(map_crash_error)?;
    serde_json::from_slice(&payload).map_err(|cause| {
        error(
            VisualParseErrorCode::InvalidFrame,
            "perception worker returned invalid JSON",
            false,
            Some(cause.to_string()),
        )
    })
}

fn map_worker_error(worker: WorkerError) -> VisualParseError {
    let code = match worker.code.as_str() {
        "incompatible_protocol" => VisualParseErrorCode::IncompatibleProtocol,
        "invalid_frame" | "invalid_json" | "invalid_request" => VisualParseErrorCode::InvalidFrame,
        "invalid_image" => VisualParseErrorCode::ArtifactInvalid,
        "resource_limit_exceeded" => VisualParseErrorCode::ResourceLimitExceeded,
        "unsupported_platform" => VisualParseErrorCode::UnsupportedPlatform,
        _ => VisualParseErrorCode::InferenceFailed,
    };
    error(
        code,
        worker.message,
        matches!(code, VisualParseErrorCode::InferenceFailed),
        worker.details.map(|value| value.to_string()),
    )
}

fn map_io_error(cause: std::io::Error) -> VisualParseError {
    error(
        VisualParseErrorCode::WorkerCrashed,
        "perception worker IPC failed",
        true,
        Some(cause.to_string()),
    )
}

fn map_crash_error(cause: std::io::Error) -> VisualParseError {
    error(
        VisualParseErrorCode::WorkerCrashed,
        "perception worker exited before completing its response",
        true,
        Some(cause.to_string()),
    )
}

fn cancelled_error() -> VisualParseError {
    error(
        VisualParseErrorCode::WorkerCancelled,
        "perception worker request was cancelled",
        true,
        None,
    )
}

fn timeout_error(phase: &str) -> VisualParseError {
    error(
        VisualParseErrorCode::Timeout,
        format!("the perception worker exceeded its {phase} deadline"),
        true,
        None,
    )
}

pub(crate) fn error(
    code: VisualParseErrorCode,
    message: impl Into<String>,
    retryable: bool,
    detail: Option<String>,
) -> VisualParseError {
    VisualParseError {
        code,
        message: message.into(),
        retryable,
        detail,
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn fixture_worker(
        mode: &str,
        counter: Option<&std::path::Path>,
    ) -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fixture-worker.py");
        let script = r#"#!/usr/bin/env python3
import base64, json, os, struct, sys, time

mode = sys.argv[1]
counter = sys.argv[2] if len(sys.argv) > 2 else None

def read_frame():
    prefix = sys.stdin.buffer.read(4)
    if len(prefix) != 4:
        sys.exit(2)
    size = struct.unpack('>I', prefix)[0]
    payload = sys.stdin.buffer.read(size)
    if len(payload) != size:
        sys.exit(3)
    return json.loads(payload)

def write_frame(value):
    payload = json.dumps(value, separators=(',', ':')).encode()
    sys.stdout.buffer.write(struct.pack('>I', len(payload)) + payload)
    sys.stdout.buffer.flush()

health = read_frame()
write_frame({'protocol':'cua-perception/1','request_id':health['request_id'],'status':'ok','result':{'ready':True,'protocol':'cua-perception/1'}})
request = read_frame()
if counter:
    with open(counter, 'a', encoding='utf-8') as handle:
        handle.write('parse\n')
if mode == 'hang':
    time.sleep(60)
elif mode == 'crash':
    sys.exit(9)
elif mode == 'oversized':
    sys.stdout.buffer.write(struct.pack('>I', 1024 * 1024))
    sys.stdout.buffer.flush()
elif mode == 'mismatch':
    write_frame({'protocol':'cua-perception/1','request_id':'wrong','status':'ok','result':{}})
else:
    image = request['params']['image']
    expected = base64.b64encode(bytes([1, 2, 3, 4])).decode()
    if image['data_base64'] != expected or image['byte_length'] != 4:
        write_frame({'protocol':'cua-perception/1','request_id':request['request_id'],'status':'error','error':{'code':'invalid_image','message':'bytes changed'}})
    else:
        write_frame({'protocol':'cua-perception/1','request_id':request['request_id'],'status':'ok','result':{'regions':[],'runtime':'fixture'}})
"#;
        std::fs::write(&path, script).unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&path, permissions).unwrap();
        let mut args = vec![mode.to_owned()];
        if let Some(counter) = counter {
            args.push(counter.display().to_string());
        }
        // Store arguments beside the script for the caller to recover.
        std::fs::write(
            directory.path().join("args.json"),
            serde_json::to_vec(&args).unwrap(),
        )
        .unwrap();
        (directory, path)
    }

    fn client(
        directory: &tempfile::TempDir,
        path: PathBuf,
        timeout: Duration,
        maximum: usize,
    ) -> PerceptionClient {
        let args: Vec<String> =
            serde_json::from_slice(&std::fs::read(directory.path().join("args.json")).unwrap())
                .unwrap();
        PerceptionClient::new(PerceptionWorkerConfig {
            executable: path,
            containment: ContainmentLimits {
                additional_writable_paths: evidence_paths(&args),
                ..ContainmentLimits::default()
            },
            args,
            request_timeout: timeout,
            max_frame_bytes: maximum,
            warm_worker: None,
        })
        .unwrap()
    }

    fn warm_client(
        directory: &tempfile::TempDir,
        path: PathBuf,
        policy: WarmWorkerPolicy,
    ) -> PerceptionClient {
        let args: Vec<String> =
            serde_json::from_slice(&std::fs::read(directory.path().join("args.json")).unwrap())
                .unwrap();
        PerceptionClient::new(PerceptionWorkerConfig {
            executable: path,
            containment: ContainmentLimits {
                additional_writable_paths: evidence_paths(&args),
                ..ContainmentLimits::default()
            },
            args,
            request_timeout: Duration::from_secs(10),
            max_frame_bytes: 1024 * 1024,
            warm_worker: Some(policy),
        })
        .unwrap()
    }

    /// The fixture workers record their evidence outside their private working
    /// directory, so the sandbox must be widened to the directories that hold
    /// it. Production callers leave this list empty.
    fn evidence_paths(args: &[String]) -> Vec<PathBuf> {
        args.iter()
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .filter_map(|path| path.parent().map(std::path::Path::to_path_buf))
            .collect()
    }

    fn warm_fixture_worker(
        counter: &std::path::Path,
        pid_file: &std::path::Path,
    ) -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("warm-worker.py");
        let script = r#"#!/usr/bin/env python3
import json, os, struct, sys, time
counter, pid_file = sys.argv[1], sys.argv[2]
with open(pid_file, 'a', encoding='utf-8') as handle: handle.write(str(os.getpid()) + '\n')
def read_frame():
 p=sys.stdin.buffer.read(4)
 if not p: return None
 if len(p)!=4: sys.exit(2)
 n=struct.unpack('>I',p)[0]; d=sys.stdin.buffer.read(n)
 if len(d)!=n: sys.exit(3)
 return json.loads(d)
def write(v):
 p=json.dumps(v,separators=(',',':')).encode(); sys.stdout.buffer.write(struct.pack('>I',len(p))+p); sys.stdout.buffer.flush()
h=read_frame(); write({'protocol':'cua-perception/1','request_id':h['request_id'],'status':'ok','result':{'ready':True,'protocol':'cua-perception/1'}})
while True:
 r=read_frame()
 if r is None: sys.exit(0)
 capture=r['params']['capture_id']
 with open(counter,'a',encoding='utf-8') as handle: handle.write(capture+'\n')
 if capture == 'crash': sys.exit(9)
 if capture == 'hang': time.sleep(60)
 write({'protocol':'cua-perception/1','request_id':r['request_id'],'status':'ok','result':{'capture':capture,'pid':os.getpid(),'regions':[]}})
"#;
        std::fs::write(&path, script).unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&path, permissions).unwrap();
        let args = vec![
            counter.display().to_string(),
            pid_file.display().to_string(),
        ];
        std::fs::write(
            directory.path().join("args.json"),
            serde_json::to_vec(&args).unwrap(),
        )
        .unwrap();
        (directory, path)
    }

    /// Reports whether the containment layer actually denied a network
    /// connection and a write outside the private working directory, and
    /// whether a write inside it still succeeds.
    fn containment_probe_worker() -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("containment-probe.py");
        let script = r#"#!/usr/bin/env python3
import errno, json, os, socket, struct, sys

def read_frame():
    prefix = sys.stdin.buffer.read(4)
    if len(prefix) != 4:
        sys.exit(2)
    size = struct.unpack('>I', prefix)[0]
    payload = sys.stdin.buffer.read(size)
    if len(payload) != size:
        sys.exit(3)
    return json.loads(payload)

def write_frame(value):
    payload = json.dumps(value, separators=(',', ':')).encode()
    sys.stdout.buffer.write(struct.pack('>I', len(payload)) + payload)
    sys.stdout.buffer.flush()

def name(failure):
    return errno.errorcode.get(failure.errno, str(failure.errno))

def probe_network():
    try:
        endpoint = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    except OSError as failure:
        return name(failure)
    try:
        endpoint.settimeout(2)
        endpoint.connect(('127.0.0.1', 9))
    except OSError as failure:
        return name(failure)
    finally:
        endpoint.close()
    return 'allowed'

def probe_write(path):
    try:
        with open(path, 'w', encoding='utf-8') as handle:
            handle.write('probe')
    except OSError as failure:
        return name(failure)
    os.unlink(path)
    return 'allowed'

def probe_fork():
    try:
        child = os.fork()
    except OSError as failure:
        return name(failure)
    if child == 0:
        os._exit(0)
    os.waitpid(child, 0)
    return 'allowed'

health = read_frame()
write_frame({'protocol':'cua-perception/1','request_id':health['request_id'],'status':'ok','result':{'ready':True,'protocol':'cua-perception/1'}})
request = read_frame()
write_frame({'protocol':'cua-perception/1','request_id':request['request_id'],'status':'ok','result':{
    'network': probe_network(),
    'fork': probe_fork(),
    'outside_write': probe_write('/tmp/cua-containment-probe-%d' % os.getpid()),
    'inside_write': probe_write(os.path.join(os.getcwd(), 'probe')),
}})
"#;
        std::fs::write(&path, script).unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&path, permissions).unwrap();
        std::fs::write(directory.path().join("args.json"), b"[]").unwrap();
        (directory, path)
    }

    #[tokio::test]
    async fn contained_worker_is_denied_network_and_writes_outside_its_directory() {
        let (directory, worker) = containment_probe_worker();
        let result = client(&directory, worker, Duration::from_secs(20), 1024 * 1024)
            .parse(
                "capture-test",
                2,
                2,
                &[1, 2, 3, 4],
                &PerceptionCancellation::default(),
            )
            .await
            .unwrap();
        // A refused connection would mean the sandbox let the syscall through.
        assert!(
            matches!(
                result["network"].as_str(),
                Some("EPERM" | "EACCES" | "EAFNOSUPPORT")
            ),
            "network probe was not denied: {result}"
        );
        assert!(
            matches!(result["outside_write"].as_str(), Some("EPERM" | "EACCES")),
            "write outside the working directory was not denied: {result}"
        );
        assert!(
            matches!(result["fork"].as_str(), Some("EPERM" | "EACCES" | "EAGAIN")),
            "worker process creation was not denied: {result}"
        );
        assert_eq!(result["inside_write"], "allowed");
    }

    #[tokio::test]
    async fn fixture_worker_receives_exact_bytes_and_returns_once() {
        let counter_directory = tempfile::tempdir().unwrap();
        let counter = counter_directory.path().join("count");
        let (directory, worker) = fixture_worker("ok", Some(&counter));
        let result = client(&directory, worker, Duration::from_secs(10), 1024 * 1024)
            .parse(
                "capture-test",
                2,
                2,
                &[1, 2, 3, 4],
                &PerceptionCancellation::default(),
            )
            .await
            .unwrap();
        assert_eq!(result["runtime"], "fixture");
        assert_eq!(std::fs::read_to_string(counter).unwrap(), "parse\n");
    }

    #[tokio::test]
    async fn malformed_response_is_not_replayed() {
        let counter_directory = tempfile::tempdir().unwrap();
        let counter = counter_directory.path().join("count");
        let (directory, worker) = fixture_worker("mismatch", Some(&counter));
        let failure = client(&directory, worker, Duration::from_secs(10), 1024 * 1024)
            .parse(
                "capture-test",
                2,
                2,
                &[1, 2, 3, 4],
                &PerceptionCancellation::default(),
            )
            .await
            .unwrap_err();
        assert_eq!(failure.code, VisualParseErrorCode::InvalidFrame);
        assert_eq!(std::fs::read_to_string(counter).unwrap(), "parse\n");
    }

    #[tokio::test]
    async fn timeout_crash_oversized_frame_and_cancellation_are_distinct() {
        let (directory, worker) = fixture_worker("hang", None);
        let failure = client(&directory, worker, Duration::from_millis(200), 1024 * 1024)
            .parse(
                "capture-test",
                2,
                2,
                &[1, 2, 3, 4],
                &PerceptionCancellation::default(),
            )
            .await
            .unwrap_err();
        assert_eq!(failure.code, VisualParseErrorCode::Timeout);

        let (directory, worker) = fixture_worker("crash", None);
        let failure = client(&directory, worker, Duration::from_secs(10), 1024 * 1024)
            .parse(
                "capture-test",
                2,
                2,
                &[1, 2, 3, 4],
                &PerceptionCancellation::default(),
            )
            .await
            .unwrap_err();
        assert_eq!(failure.code, VisualParseErrorCode::WorkerCrashed);

        let (directory, worker) = fixture_worker("oversized", None);
        let failure = client(&directory, worker, Duration::from_secs(10), 128)
            .parse(
                "capture-test",
                2,
                2,
                &[1, 2, 3, 4],
                &PerceptionCancellation::default(),
            )
            .await
            .unwrap_err();
        assert_eq!(failure.code, VisualParseErrorCode::InvalidFrame);

        let cancellation = PerceptionCancellation::default();
        cancellation.cancel();
        let failure = PerceptionClient::unavailable()
            .parse("capture-test", 2, 2, &[1, 2, 3, 4], &cancellation)
            .await
            .unwrap_err();
        assert_eq!(failure.code, VisualParseErrorCode::NotInstalled);

        let (directory, worker) = fixture_worker("hang", None);
        let client = client(&directory, worker, Duration::from_secs(2), 1024 * 1024);
        let cancellation = PerceptionCancellation::default();
        let trigger = cancellation.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(25)).await;
            trigger.cancel();
        });
        let failure = client
            .parse("capture-test", 2, 2, &[1, 2, 3, 4], &cancellation)
            .await
            .unwrap_err();
        assert_eq!(failure.code, VisualParseErrorCode::WorkerCancelled);
    }

    #[tokio::test]
    async fn warm_worker_reuses_one_process_then_idle_ttl_reaps_it() {
        let evidence = tempfile::tempdir().unwrap();
        let counter = evidence.path().join("counter");
        let pids = evidence.path().join("pids");
        let (directory, worker) = warm_fixture_worker(&counter, &pids);
        let client = warm_client(
            &directory,
            worker,
            WarmWorkerPolicy {
                startup_timeout: Duration::from_secs(10),
                inference_timeout: Duration::from_secs(2),
                shutdown_timeout: Duration::from_millis(100),
                idle_ttl: Duration::from_millis(100),
            },
        );
        for capture in ["one", "two"] {
            client
                .parse(capture, 1, 1, &[1, 2, 3, 4], &Default::default())
                .await
                .unwrap();
        }
        assert_eq!(std::fs::read_to_string(&pids).unwrap().lines().count(), 1);
        tokio::time::sleep(Duration::from_millis(250)).await;
        client
            .parse("three", 1, 1, &[1, 2, 3, 4], &Default::default())
            .await
            .unwrap();
        assert_eq!(std::fs::read_to_string(&pids).unwrap().lines().count(), 2);
        assert_eq!(
            std::fs::read_to_string(counter).unwrap(),
            "one\ntwo\nthree\n"
        );
    }

    #[tokio::test]
    async fn warm_worker_crash_and_cancel_kill_state_without_replay() {
        let evidence = tempfile::tempdir().unwrap();
        let counter = evidence.path().join("counter");
        let pids = evidence.path().join("pids");
        let (directory, worker) = warm_fixture_worker(&counter, &pids);
        let client = warm_client(
            &directory,
            worker,
            WarmWorkerPolicy {
                startup_timeout: Duration::from_secs(10),
                inference_timeout: Duration::from_secs(2),
                shutdown_timeout: Duration::from_millis(100),
                idle_ttl: Duration::from_secs(5),
            },
        );
        let failure = client
            .parse("crash", 1, 1, &[1, 2, 3, 4], &Default::default())
            .await
            .unwrap_err();
        assert_eq!(failure.code, VisualParseErrorCode::WorkerCrashed);
        client
            .parse("after-crash", 1, 1, &[1, 2, 3, 4], &Default::default())
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(&counter).unwrap(),
            "crash\nafter-crash\n"
        );

        let cancellation = PerceptionCancellation::default();
        let trigger = cancellation.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            trigger.cancel();
        });
        let failure = client
            .parse("hang", 1, 1, &[1, 2, 3, 4], &cancellation)
            .await
            .unwrap_err();
        assert_eq!(failure.code, VisualParseErrorCode::WorkerCancelled);
        client
            .parse("after-cancel", 1, 1, &[1, 2, 3, 4], &Default::default())
            .await
            .unwrap();
        let calls = std::fs::read_to_string(counter).unwrap();
        assert_eq!(calls.matches("hang\n").count(), 1);
        assert!(calls.ends_with("after-cancel\n"));
    }

    #[tokio::test]
    async fn shutdown_now_cancels_inference_and_drops_warm_worker() {
        let evidence = tempfile::tempdir().unwrap();
        let counter = evidence.path().join("counter");
        let pids = evidence.path().join("pids");
        let (directory, worker) = warm_fixture_worker(&counter, &pids);
        let client = warm_client(
            &directory,
            worker,
            WarmWorkerPolicy {
                startup_timeout: Duration::from_secs(10),
                inference_timeout: Duration::from_secs(5),
                shutdown_timeout: Duration::from_millis(100),
                idle_ttl: Duration::from_secs(5),
            },
        );
        let shutdown = client.clone();
        let observed_counter = counter.clone();
        let original_pid = client
            .parse("before-shutdown", 1, 1, &[1, 2, 3, 4], &Default::default())
            .await
            .unwrap()["pid"]
            .as_u64()
            .unwrap();
        tokio::spawn(async move {
            for _ in 0..100 {
                if std::fs::read_to_string(&observed_counter)
                    .is_ok_and(|calls| calls.contains("hang\n"))
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            shutdown.shutdown_now();
        });
        let failure = client
            .parse("hang", 1, 1, &[1, 2, 3, 4], &Default::default())
            .await
            .unwrap_err();
        assert_eq!(failure.code, VisualParseErrorCode::WorkerCancelled);
        let replacement_pid = client
            .parse("after-shutdown", 1, 1, &[1, 2, 3, 4], &Default::default())
            .await
            .unwrap()["pid"]
            .as_u64()
            .unwrap();
        assert_ne!(original_pid, replacement_pid);
    }
}

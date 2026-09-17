//! Bounded, single-use IPC client for the optional perception worker.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::Notify;
use uuid::Uuid;

use cua_driver_contract::{VisualParseError, VisualParseErrorCode};

const PROTOCOL_VERSION: &str = "cua-perception/1";
const DEFAULT_MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct PerceptionWorkerConfig {
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub request_timeout: Duration,
    pub max_frame_bytes: usize,
}

impl PerceptionWorkerConfig {
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            args: Vec::new(),
            request_timeout: Duration::from_secs(30),
            max_frame_bytes: DEFAULT_MAX_FRAME_BYTES,
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

#[derive(Clone, Default)]
pub struct PerceptionClient {
    config: Option<PerceptionWorkerConfig>,
}

impl PerceptionClient {
    pub fn unavailable() -> Self {
        Self { config: None }
    }

    pub fn new(config: PerceptionWorkerConfig) -> Result<Self, VisualParseError> {
        if config.request_timeout.is_zero()
            || config.max_frame_bytes == 0
            || config.max_frame_bytes > u32::MAX as usize
        {
            return Err(error(
                VisualParseErrorCode::ResourceLimitExceeded,
                "perception worker limits must be non-zero and fit the framed protocol",
                false,
                None,
            ));
        }
        Ok(Self {
            config: Some(config),
        })
    }

    pub fn is_available(&self) -> bool {
        self.config.is_some()
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

        let operation = self.parse_once(config, capture_id, width, height, png_bytes);
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

    async fn parse_once(
        &self,
        config: &PerceptionWorkerConfig,
        capture_id: &str,
        width: u32,
        height: u32,
        png_bytes: &[u8],
    ) -> Result<Value, VisualParseError> {
        use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};

        let mut command = Command::new(&config.executable);
        command
            .args(&config.args)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let private_working_directory = tempfile::Builder::new()
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
        command.current_dir(private_working_directory.path());
        configure_process_containment(&mut command)?;
        let mut child = command.spawn().map_err(|cause| {
            error(
                VisualParseErrorCode::WorkerLaunchFailed,
                "failed to launch the perception worker",
                true,
                Some(cause.to_string()),
            )
        })?;
        let _process_group = ProcessGroupGuard::new(child.id());
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

        let request_id = format!("parse-{}", Uuid::new_v4());
        write_json_frame(
            &mut stdin,
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
        stdin.shutdown().await.map_err(map_io_error)?;
        drop(stdin);

        let response = read_response(&mut stdout, config.max_frame_bytes).await?;
        let result = response.into_result(&request_id)?;
        let status = child.wait().await.map_err(map_crash_error)?;
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
}

#[cfg(unix)]
fn configure_process_containment(command: &mut Command) -> Result<(), VisualParseError> {
    command.process_group(0);
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        // The closure runs between fork and exec and uses only async-signal-safe
        // libc calls. A race check handles a parent that died before prctl.
        unsafe {
            command.as_std_mut().pre_exec(|| {
                let parent = libc::getppid();
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::getppid() != parent {
                    libc::raise(libc::SIGKILL);
                }
                Ok(())
            });
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn configure_process_containment(_command: &mut Command) -> Result<(), VisualParseError> {
    Ok(())
}

struct ProcessGroupGuard {
    #[cfg(unix)]
    pid: Option<u32>,
}

impl ProcessGroupGuard {
    fn new(pid: Option<u32>) -> Self {
        Self {
            #[cfg(unix)]
            pid,
        }
    }
}

impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.pid {
            if let Ok(pid) = i32::try_from(pid) {
                // The worker is its process-group leader. Killing the negative
                // pid prevents descendants from surviving cancellation.
                unsafe {
                    libc::kill(-pid, libc::SIGKILL);
                }
            }
        }
    }
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
            args,
            request_timeout: timeout,
            max_frame_bytes: maximum,
        })
        .unwrap()
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
}

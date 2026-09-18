//! Bounded, single-use IPC client for the optional perception worker.

pub mod containment;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{Mutex as AsyncMutex, Notify};
use uuid::Uuid;

use cua_driver_contract::{VisualParseError, VisualParseErrorCode};

#[cfg(test)]
use containment::ContainmentLimits;
use containment::{ContainedChild, WorkerExit, WorkerStdin, WorkerStdout};

const PROTOCOL_VERSION: &str = "cua-perception/1";
// A default-registry capture expands to four base64 bytes per three PNG bytes.
// Leave a fixed envelope for the bounded request metadata and capture ID.
const DEFAULT_MAX_FRAME_BYTES: usize =
    ((crate::capture_registry::DEFAULT_MAX_CAPTURE_BYTES + 2) / 3) * 4 + 64 * 1024;

/// Signed extension identity the launched worker must echo in every result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExpectedExtensionIdentity {
    pub id: String,
    pub version: String,
}

#[derive(Clone, Debug)]
pub struct PerceptionWorkerConfig {
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub request_timeout: Duration,
    pub max_frame_bytes: usize,
    pub warm_worker: Option<WarmWorkerPolicy>,
    pub expected_extension_identity: Option<ExpectedExtensionIdentity>,
    installed: bool,
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
            expected_extension_identity: None,
            installed: false,
            #[cfg(test)]
            containment: ContainmentLimits::default(),
        }
    }

    pub fn with_bounded_reuse(mut self, policy: WarmWorkerPolicy) -> Self {
        self.warm_worker = Some(policy);
        self
    }

    /// Configuration for an installed extension.
    ///
    /// The worker starts cold for every parse and is shut down afterwards. Warm
    /// reuse stays behind [`Self::with_bounded_reuse`] until per-platform
    /// cleanup certification passes, so an installed extension never keeps a
    /// worker process — and the screenshot it was handed — alive between
    /// requests by default.
    pub fn installed(executable: impl Into<PathBuf>) -> Self {
        let mut config = Self::new(executable);
        config.installed = true;
        config
    }

    /// Bind an installed worker invocation and its result to the identity from
    /// the verified signed extension manifest.
    ///
    /// The worker CLI must accept these arguments and copy them verbatim to
    /// `result.identity.extension`. A missing or different identity fails the
    /// parse before its result reaches Driver consumers.
    pub fn installed_with_identity(
        executable: impl Into<PathBuf>,
        id: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        let identity = ExpectedExtensionIdentity {
            id: id.into(),
            version: version.into(),
        };
        let mut config = Self::installed(executable);
        config.args.extend([
            "--extension-id".to_owned(),
            identity.id.clone(),
            "--extension-version".to_owned(),
            identity.version.clone(),
        ]);
        config.expected_extension_identity = Some(identity);
        config
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

    #[cfg(test)]
    pub(crate) fn with_test_containment(
        mut self,
        containment: containment::ContainmentLimits,
    ) -> Self {
        self.containment = containment;
        self
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
        if config
            .expected_extension_identity
            .as_ref()
            .is_some_and(|identity| {
                identity.id.trim().is_empty() || identity.version.trim().is_empty()
            })
        {
            return Err(error(
                VisualParseErrorCode::ArtifactInvalid,
                "the verified perception extension identity must be non-empty",
                false,
                None,
            ));
        }
        if config.installed && config.expected_extension_identity.is_none() {
            return Err(error(
                VisualParseErrorCode::ArtifactInvalid,
                "an installed perception extension must be bound to its verified identity",
                false,
                None,
            ));
        }
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
        // Dropping the writer closes the pipe on every platform, which is what
        // the worker observes as end of file; a named-pipe shutdown would not.
        worker.stdin.take();
        match worker.contained.wait().await.map_err(map_crash_error)? {
            WorkerExit::Success => Ok(result),
            exit => Err(map_worker_exit(exit)),
        }
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
    /// Taken to close the request pipe when the worker should observe end of
    /// file, which is the only way a named-pipe parent end signals it.
    stdin: Option<WorkerStdin>,
    stdout: WorkerStdout,
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
        )
        .await?;
        let mut stdin = contained.take_stdin().ok_or_else(|| {
            error(
                VisualParseErrorCode::WorkerLaunchFailed,
                "perception worker stdin was unavailable",
                true,
                None,
            )
        })?;
        let mut stdout = contained.take_stdout().ok_or_else(|| {
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
        verify_expected_extension_identity(
            &health_result,
            config.expected_extension_identity.as_ref(),
        )?;
        Ok(Self {
            contained,
            stdin: Some(stdin),
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

        let stdin = self.stdin.as_mut().ok_or_else(|| {
            error(
                VisualParseErrorCode::WorkerCrashed,
                "the perception worker request pipe was already closed",
                true,
                None,
            )
        })?;
        let request_id = format!("parse-{}", Uuid::new_v4());
        write_json_frame(
            stdin,
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
        let result = read_response(&mut self.stdout, config.max_frame_bytes)
            .await?
            .into_result(&request_id)?;
        verify_expected_extension_identity(&result, config.expected_extension_identity.as_ref())?;
        Ok(result)
    }
}

fn verify_expected_extension_identity(
    result: &Value,
    expected: Option<&ExpectedExtensionIdentity>,
) -> Result<(), VisualParseError> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let extension = result
        .get("identity")
        .and_then(|identity| identity.get("extension"));
    let actual_id = extension
        .and_then(|value| value.get("id"))
        .and_then(Value::as_str);
    let actual_version = extension
        .and_then(|value| value.get("version"))
        .and_then(Value::as_str);
    if actual_id != Some(expected.id.as_str()) || actual_version != Some(expected.version.as_str())
    {
        return Err(error(
            VisualParseErrorCode::ArtifactInvalid,
            "perception worker identity did not match the verified signed extension manifest",
            false,
            None,
        ));
    }
    Ok(())
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
    worker.stdin.take();
    let _ = tokio::time::timeout(timeout, worker.contained.wait()).await;
}

/// A worker that a containment ceiling ended is reported as a resource limit
/// rather than a crash, so a caller does not retry a request that cannot
/// succeed until the worker is replaced.
fn map_worker_exit(exit: WorkerExit) -> VisualParseError {
    match exit {
        WorkerExit::Success => error(
            VisualParseErrorCode::InferenceFailed,
            "the perception worker reported success without a result",
            false,
            None,
        ),
        WorkerExit::ResourceLimit(detail) => error(
            VisualParseErrorCode::ResourceLimitExceeded,
            "the perception worker exhausted a containment resource ceiling",
            false,
            Some(detail),
        ),
        WorkerExit::Failure(detail) => error(
            VisualParseErrorCode::WorkerCrashed,
            "perception worker exited unsuccessfully after responding",
            true,
            Some(detail),
        ),
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
#[derive(Clone, Debug)]
pub(crate) struct FixturePythonConfig {
    pub(crate) interpreter: PathBuf,
    pub(crate) readable_roots: Vec<PathBuf>,
    pub(crate) executable_paths: Vec<PathBuf>,
}

#[cfg(all(test, unix))]
pub(crate) fn fixture_python_config() -> Result<FixturePythonConfig, String> {
    fixture_python_config_from(
        std::env::var_os("CUA_TEST_PYTHON"),
        std::env::var_os("CUA_TEST_PYTHON_READ_ROOTS"),
        std::env::var_os("CUA_TEST_PYTHON_DYNAMIC_LINKER"),
        std::path::Path::new("/nix/store"),
    )
}

#[cfg(all(test, unix))]
fn fixture_python_config_from(
    configured_interpreter: Option<std::ffi::OsString>,
    configured_read_roots: Option<std::ffi::OsString>,
    configured_dynamic_linker: Option<std::ffi::OsString>,
    trusted_store_root: &std::path::Path,
) -> Result<FixturePythonConfig, String> {
    if let Some(interpreter) = configured_interpreter {
        let interpreter = PathBuf::from(interpreter);
        let resolved_interpreter = validate_fixture_path(&interpreter, true)?;
        let resolved_store_root = std::fs::canonicalize(trusted_store_root).ok();
        let is_nix_interpreter = interpreter.starts_with(trusted_store_root)
            || resolved_store_root
                .as_ref()
                .is_some_and(|root| resolved_interpreter.starts_with(root));

        if is_nix_interpreter {
            let configured_store_root = trusted_store_root;
            let trusted_store_root = resolved_store_root
                .ok_or_else(|| "trusted test store root is unavailable".to_owned())?;
            if !interpreter.starts_with(configured_store_root) {
                return Err("CUA_TEST_PYTHON must be inside the trusted Nix store".into());
            }
            if !resolved_interpreter.starts_with(&trusted_store_root) {
                return Err("CUA_TEST_PYTHON must resolve inside the trusted Nix store".into());
            }
            let configured_read_roots = configured_read_roots.ok_or_else(|| {
                "CUA_TEST_PYTHON_READ_ROOTS is required for a Nix store interpreter".to_owned()
            })?;
            let mut readable_roots = Vec::new();
            for root in std::env::split_paths(&configured_read_roots) {
                if root == configured_store_root || !root.starts_with(configured_store_root) {
                    return Err(
                        "CUA_TEST_PYTHON_READ_ROOTS may contain only trusted Nix store paths"
                            .into(),
                    );
                }
                let resolved = validate_fixture_path(&root, false)?;
                if resolved == trusted_store_root || !resolved.starts_with(&trusted_store_root) {
                    return Err(
                        "CUA_TEST_PYTHON_READ_ROOTS may contain only trusted Nix store paths"
                            .into(),
                    );
                }
                readable_roots.push(resolved);
            }
            if readable_roots.is_empty() {
                return Err("CUA_TEST_PYTHON_READ_ROOTS must not be empty".into());
            }
            readable_roots.sort();
            readable_roots.dedup();
            let configured_dynamic_linker = configured_dynamic_linker.ok_or_else(|| {
                "CUA_TEST_PYTHON_DYNAMIC_LINKER is required for a Nix store interpreter".to_owned()
            })?;
            let dynamic_linker = PathBuf::from(configured_dynamic_linker);
            if dynamic_linker == configured_store_root
                || !dynamic_linker.starts_with(configured_store_root)
            {
                return Err(
                    "CUA_TEST_PYTHON_DYNAMIC_LINKER must be inside the trusted Nix store".into(),
                );
            }
            let resolved_dynamic_linker = validate_fixture_path(&dynamic_linker, true)?;
            if resolved_dynamic_linker == trusted_store_root
                || !resolved_dynamic_linker.starts_with(&trusted_store_root)
            {
                return Err(
                    "CUA_TEST_PYTHON_DYNAMIC_LINKER must resolve inside the trusted Nix store"
                        .into(),
                );
            }
            if !readable_roots
                .iter()
                .any(|root| resolved_dynamic_linker.starts_with(root))
            {
                return Err(
                    "CUA_TEST_PYTHON_DYNAMIC_LINKER must resolve inside a configured Nix closure root"
                        .into(),
                );
            }
            let mut executable_paths = vec![resolved_interpreter, resolved_dynamic_linker];
            executable_paths.sort();
            executable_paths.dedup();
            return Ok(FixturePythonConfig {
                interpreter: interpreter.clone(),
                readable_roots,
                executable_paths,
            });
        }

        if configured_read_roots.is_some() || configured_dynamic_linker.is_some() {
            return Err(
                "Nix fixture path configuration is supported only for a Nix store interpreter"
                    .into(),
            );
        }
        return Ok(FixturePythonConfig {
            interpreter: interpreter.clone(),
            readable_roots: host_python_readable_roots(),
            executable_paths: vec![resolved_interpreter],
        });
    }
    if configured_read_roots.is_some() || configured_dynamic_linker.is_some() {
        return Err("Nix fixture path configuration requires CUA_TEST_PYTHON".into());
    }

    #[cfg(target_os = "macos")]
    let candidates = [
        "/Applications/Xcode.app/Contents/Developer/usr/bin/python3",
        "/usr/bin/python3",
        "/usr/local/bin/python3",
    ];
    #[cfg(not(target_os = "macos"))]
    let candidates = ["/usr/bin/python3", "/usr/local/bin/python3"];
    let interpreter = candidates
        .into_iter()
        .map(PathBuf::from)
        .find(|candidate| validate_fixture_path(candidate, true).is_ok())
        .ok_or_else(|| "no trusted fixture python interpreter is installed".to_owned())?;
    let readable_roots = host_python_readable_roots();
    #[cfg(target_os = "macos")]
    let executable_paths = [
        interpreter.clone(),
        PathBuf::from("/usr/bin/env"),
        PathBuf::from("/usr/bin/python3"),
        PathBuf::from("/usr/local/bin/python3"),
        PathBuf::from("/Applications/Xcode.app/Contents/Developer/usr/bin/python3"),
        PathBuf::from("/Applications/Xcode.app/Contents/Developer/Library/Frameworks/Python3.framework/Versions/3.9/bin/python3.9"),
        PathBuf::from("/Applications/Xcode.app/Contents/Developer/Library/Frameworks/Python3.framework/Versions/3.9/Python3"),
        PathBuf::from("/Applications/Xcode.app/Contents/Developer/Library/Frameworks/Python3.framework/Versions/3.9/Resources/Python.app/Contents/MacOS/Python"),
    ]
    .into_iter()
    .filter(|path| path.is_file())
    .collect();
    #[cfg(not(target_os = "macos"))]
    let executable_paths = vec![interpreter.clone()];
    Ok(FixturePythonConfig {
        interpreter,
        readable_roots,
        executable_paths,
    })
}

#[cfg(all(test, unix))]
fn host_python_readable_roots() -> Vec<PathBuf> {
    [
        "/usr",
        "/bin",
        "/lib",
        "/lib64",
        "/etc",
        "/opt",
        "/System",
        "/Library",
        "/private/var/db",
        "/private/var/select",
        "/Applications/Xcode.app",
    ]
    .into_iter()
    .map(PathBuf::from)
    .filter(|path| path.is_dir())
    .collect()
}

#[cfg(all(test, unix))]
fn validate_fixture_path(path: &std::path::Path, file: bool) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("test fixture paths must be absolute".into());
    }
    let resolved = std::fs::canonicalize(path)
        .map_err(|error| format!("test fixture path could not be resolved: {error}"))?;
    if (file && !resolved.is_file()) || (!file && !resolved.is_dir()) {
        return Err("test fixture path has the wrong file type".into());
    }
    Ok(resolved)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn with_fixture_interpreter(script: &str) -> String {
        let interpreter = fixture_python_interpreter();
        script.replacen(
            "#!/usr/bin/env python3",
            &format!("#!{}", interpreter.display()),
            1,
        )
    }

    fn fixture_python_interpreter() -> PathBuf {
        fixture_python_config().unwrap().interpreter
    }

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
        std::fs::write(&path, with_fixture_interpreter(script)).unwrap();
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
            containment: fixture_limits(&args),
            args,
            request_timeout: timeout,
            max_frame_bytes: maximum,
            warm_worker: None,
            expected_extension_identity: None,
            installed: false,
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
            containment: fixture_limits(&args),
            args,
            request_timeout: Duration::from_secs(10),
            max_frame_bytes: 1024 * 1024,
            warm_worker: Some(policy),
            expected_extension_identity: None,
            installed: false,
        })
        .unwrap()
    }

    /// Production limits widened by exactly what a scripted fixture needs and
    /// nothing else. Production callers leave both lists empty, which is what
    /// the enforcement probes below rely on: every path they are refused is
    /// refused under a production-shaped sandbox too.
    fn fixture_limits(args: &[String]) -> ContainmentLimits {
        ContainmentLimits {
            additional_writable_paths: evidence_paths(args),
            additional_readable_paths: interpreter_read_paths(),
            additional_executable_paths: interpreter_executable_paths(),
            ..ContainmentLimits::default()
        }
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

    /// A shipped worker is a self-contained native binary and reads only its
    /// own bundle. The fixtures are Python scripts, so the host interpreter's
    /// directories are opted in explicitly rather than by widening the derived
    /// allowlist for everyone.
    fn interpreter_read_paths() -> Vec<PathBuf> {
        fixture_python_config().unwrap().readable_roots
    }

    fn interpreter_executable_paths() -> Vec<PathBuf> {
        fixture_python_config().unwrap().executable_paths
    }

    #[test]
    fn fixture_interpreter_uses_an_absolute_containment_grant() {
        let interpreter = fixture_python_interpreter();
        assert!(interpreter.is_absolute());
        assert!(
            interpreter_executable_paths().contains(&std::fs::canonicalize(interpreter).unwrap())
        );
    }

    #[test]
    fn fixture_python_config_accepts_validated_non_nix_override() {
        let temp = tempfile::tempdir().unwrap();
        let store = temp.path().join("store");
        let outside = temp.path().join("outside");
        std::fs::create_dir(&store).unwrap();
        std::fs::create_dir(&outside).unwrap();
        let outside_interpreter = outside.join("python3");
        std::fs::write(&outside_interpreter, b"fixture").unwrap();

        assert!(fixture_python_config_from(Some("python3".into()), None, None, &store).is_err());
        let config = fixture_python_config_from(
            Some(outside_interpreter.clone().into_os_string()),
            None,
            None,
            &store,
        )
        .unwrap();
        assert_eq!(config.interpreter, outside_interpreter);
        assert_eq!(
            config.executable_paths,
            vec![std::fs::canonicalize(&config.interpreter).unwrap()]
        );
        assert!(fixture_python_config_from(
            Some(config.interpreter.clone().into_os_string()),
            Some(outside.clone().into_os_string()),
            None,
            &store,
        )
        .is_err());
        assert!(fixture_python_config_from(
            Some(config.interpreter.into_os_string()),
            None,
            Some(outside_interpreter.into_os_string()),
            &store,
        )
        .is_err());
        assert!(fixture_python_config_from(
            None,
            None,
            Some(outside.join("python3").into_os_string()),
            &store,
        )
        .is_err());
    }

    #[test]
    fn fixture_python_config_rejects_untrusted_nix_closure_roots() {
        let temp = tempfile::tempdir().unwrap();
        let store = temp.path().join("store");
        let package = store.join("python");
        let bin = package.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let interpreter = bin.join("python3");
        std::fs::write(&interpreter, b"fixture").unwrap();
        let outside = temp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();

        assert!(fixture_python_config_from(
            Some(interpreter.clone().into_os_string()),
            None,
            None,
            &store,
        )
        .is_err());
        assert!(fixture_python_config_from(
            Some(interpreter.into_os_string()),
            Some(outside.into_os_string()),
            None,
            &store,
        )
        .is_err());
    }

    #[test]
    fn fixture_python_config_grants_only_exact_closure_roots() {
        let temp = tempfile::tempdir().unwrap();
        let store = temp.path().join("store");
        let python = store.join("python");
        let dependency = store.join("dependency");
        std::fs::create_dir_all(python.join("bin")).unwrap();
        std::fs::create_dir_all(dependency.join("lib")).unwrap();
        let interpreter = python.join("bin/python3");
        let loader = dependency.join("lib/ld-linux.so");
        std::fs::write(&interpreter, b"fixture").unwrap();
        std::fs::write(&loader, b"fixture").unwrap();
        let roots = std::env::join_paths([python.clone(), dependency.clone()]).unwrap();
        let config = fixture_python_config_from(
            Some(interpreter.clone().into_os_string()),
            Some(roots),
            Some(loader.clone().into_os_string()),
            &store,
        )
        .unwrap();
        assert_eq!(config.interpreter, interpreter);
        assert_eq!(
            config.executable_paths,
            vec![
                std::fs::canonicalize(loader).unwrap(),
                std::fs::canonicalize(&interpreter).unwrap(),
            ]
        );
        assert_eq!(
            config.readable_roots,
            vec![
                std::fs::canonicalize(dependency).unwrap(),
                std::fs::canonicalize(python).unwrap(),
            ]
        );
        assert!(!config
            .readable_roots
            .contains(&std::fs::canonicalize(&store).unwrap()));
        assert!(fixture_python_config_from(
            Some(config.interpreter.into_os_string()),
            Some(store.clone().into_os_string()),
            Some(store.join("dependency/lib/ld-linux.so").into_os_string()),
            &store,
        )
        .is_err());
    }

    #[test]
    fn fixture_python_config_rejects_dynamic_linker_outside_configured_closure_roots() {
        let temp = tempfile::tempdir().unwrap();
        let store = temp.path().join("store");
        let python = store.join("python");
        let dependency = store.join("dependency");
        let outside = store.join("outside");
        std::fs::create_dir_all(python.join("bin")).unwrap();
        std::fs::create_dir_all(dependency.join("lib")).unwrap();
        std::fs::create_dir_all(outside.join("lib")).unwrap();
        let interpreter = python.join("bin/python3");
        let loader = dependency.join("lib/ld-linux.so");
        let untrusted = outside.join("lib/arbitrary");
        let escaped = dependency.join("lib/escaped-loader");
        std::fs::write(&interpreter, b"fixture").unwrap();
        std::fs::write(&loader, b"fixture").unwrap();
        std::fs::write(&untrusted, b"fixture").unwrap();
        std::os::unix::fs::symlink(&untrusted, &escaped).unwrap();
        let roots = std::env::join_paths([python, dependency]).unwrap();

        assert!(fixture_python_config_from(
            Some(interpreter.clone().into_os_string()),
            Some(roots.clone()),
            Some(escaped.into_os_string()),
            &store,
        )
        .is_err());
        assert!(fixture_python_config_from(
            Some(interpreter.into_os_string()),
            Some(roots),
            Some(untrusted.into_os_string()),
            &store,
        )
        .is_err());
    }

    #[test]
    fn fixture_python_config_rejects_dynamic_linker_configured_outside_store() {
        let temp = tempfile::tempdir().unwrap();
        let store = temp.path().join("store");
        let python = store.join("python");
        let dependency = store.join("dependency");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(python.join("bin")).unwrap();
        std::fs::create_dir_all(dependency.join("lib")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let interpreter = python.join("bin/python3");
        let loader = dependency.join("lib/ld-linux.so");
        let configured_outside = outside.join("ld-linux.so");
        std::fs::write(&interpreter, b"fixture").unwrap();
        std::fs::write(&loader, b"fixture").unwrap();
        std::os::unix::fs::symlink(&loader, &configured_outside).unwrap();
        let roots = std::env::join_paths([python, dependency]).unwrap();

        assert!(fixture_python_config_from(
            Some(interpreter.into_os_string()),
            Some(roots),
            Some(configured_outside.into_os_string()),
            &store,
        )
        .is_err());
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
        std::fs::write(&path, with_fixture_interpreter(script)).unwrap();
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

    /// Reports what the containment layer actually denied. The probe takes its
    /// targets from `capture_id` rather than from argv, because an absolute
    /// path argument is part of the worker's launch configuration and would
    /// legitimately widen the read allowlist.
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

def probe_local_socket(path):
    try:
        endpoint = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    except OSError as failure:
        return name(failure)
    try:
        endpoint.settimeout(2)
        endpoint.connect(path)
    except OSError as failure:
        return name(failure)
    finally:
        endpoint.close()
    return 'allowed'

def probe_read(path):
    try:
        with open(path, 'rb') as handle:
            handle.read(1)
    except OSError as failure:
        return name(failure)
    return 'allowed'

def probe_list(path):
    try:
        os.listdir(path)
    except OSError as failure:
        return name(failure)
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

def probe_process_memory():
    if not sys.platform.startswith('linux'):
        return 'unavailable'
    try:
        import ctypes
        libc = ctypes.CDLL(None, use_errno=True)
        # PTRACE_ATTACH against the supervising Driver process.
        if libc.ptrace(16, os.getppid(), 0, 0) == 0:
            return 'allowed'
        return errno.errorcode.get(ctypes.get_errno(), str(ctypes.get_errno()))
    except Exception as failure:
        return type(failure).__name__

health = read_frame()
write_frame({'protocol':'cua-perception/1','request_id':health['request_id'],'status':'ok','result':{'ready':True,'protocol':'cua-perception/1'}})
request = read_frame()
targets = json.loads(request['params']['capture_id'])
write_frame({'protocol':'cua-perception/1','request_id':request['request_id'],'status':'ok','result':{
    'network': probe_network(),
    'local_socket': probe_local_socket(targets['socket']),
    'secret_read': probe_read(targets['secret']),
    'secret_list': probe_list(os.path.dirname(targets['secret'])),
    'fork': probe_fork(),
    'process_memory': probe_process_memory(),
    'outside_write': probe_write(targets['outside']),
    'inside_write': probe_write(os.path.join(os.getcwd(), 'probe')),
}})
"#;
        std::fs::write(&path, with_fixture_interpreter(script)).unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&path, permissions).unwrap();
        std::fs::write(directory.path().join("args.json"), b"[]").unwrap();
        (directory, path)
    }

    /// Anything the worker is refused here is refused under a production
    /// sandbox too: the probe runs with no extra writable path and only the
    /// interpreter's own read roots.
    #[tokio::test]
    async fn contained_worker_is_denied_every_route_off_its_own_bundle() {
        let evidence = tempfile::Builder::new()
            .prefix("cua-perception-secrets-")
            .tempdir()
            .unwrap();
        let secret = evidence.path().join("credentials");
        std::fs::write(&secret, b"token").unwrap();
        let socket = evidence.path().join("desktop.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let targets = serde_json::json!({
            "secret": secret.display().to_string(),
            "socket": socket.display().to_string(),
            "outside": evidence.path().join("escape").display().to_string(),
        })
        .to_string();

        let (directory, worker) = containment_probe_worker();
        let result = client(&directory, worker, Duration::from_secs(20), 1024 * 1024)
            .parse(
                &targets,
                2,
                2,
                &[1, 2, 3, 4],
                &PerceptionCancellation::default(),
            )
            .await
            .unwrap();
        drop(listener);

        // A refused connection would mean the sandbox let the syscall through,
        // so "connection refused" is not an acceptable outcome for any of these.
        let denied = ["EPERM", "EACCES", "EAFNOSUPPORT"];
        for (probe, allowed) in [
            ("network", &denied[..]),
            ("local_socket", &denied[..]),
            ("secret_read", &denied[..2]),
            ("secret_list", &denied[..2]),
            ("outside_write", &denied[..2]),
        ] {
            let observed = result[probe].as_str().unwrap_or_default();
            assert!(
                allowed.contains(&observed),
                "the {probe} probe was not denied: {result}"
            );
        }
        assert!(
            matches!(result["fork"].as_str(), Some("EPERM" | "EACCES" | "EAGAIN")),
            "worker process creation was not denied: {result}"
        );
        assert!(
            matches!(
                result["process_memory"].as_str(),
                Some("unavailable" | "EPERM" | "EACCES" | "ENOSYS")
            ),
            "the worker could attach to the Driver process: {result}"
        );
        assert_eq!(result["inside_write"], "allowed");
    }

    #[tokio::test]
    async fn a_bundle_directory_too_broad_to_grant_fails_before_the_worker_runs() {
        // `/usr/bin` is a shared system directory: granting reads beneath it
        // would hand the worker every other program on the machine, so the
        // launch must fail closed instead.
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("args.json"), b"[]").unwrap();
        let failure = client(
            &directory,
            PathBuf::from("/usr/bin/true"),
            Duration::from_secs(10),
            1024 * 1024,
        )
        .parse(
            "capture-test",
            2,
            2,
            &[1, 2, 3, 4],
            &PerceptionCancellation::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(failure.code, VisualParseErrorCode::WorkerLaunchFailed);
        assert!(!failure.retryable);
    }

    #[tokio::test]
    async fn a_missing_opted_in_sandbox_path_fails_closed() {
        let directory = tempfile::tempdir().unwrap();
        let (fixture, worker) = fixture_worker("ok", None);
        let client = PerceptionClient::new(PerceptionWorkerConfig {
            executable: worker,
            containment: ContainmentLimits {
                additional_readable_paths: vec![directory
                    .path()
                    .join("this-directory-does-not-exist")],
                ..ContainmentLimits::default()
            },
            args: Vec::new(),
            request_timeout: Duration::from_secs(10),
            max_frame_bytes: 1024 * 1024,
            warm_worker: None,
            expected_extension_identity: None,
            installed: false,
        })
        .unwrap();
        let failure = client
            .parse(
                "capture-test",
                2,
                2,
                &[1, 2, 3, 4],
                &PerceptionCancellation::default(),
            )
            .await
            .unwrap_err();
        assert_eq!(failure.code, VisualParseErrorCode::WorkerLaunchFailed);
        drop(fixture);
    }

    #[test]
    fn an_installed_extension_starts_cold() {
        // Warm reuse keeps a worker — and the screenshot it was handed — alive
        // between requests, so it stays behind an explicit opt-in until the
        // per-platform cleanup certification passes.
        assert!(PerceptionWorkerConfig::installed("/opt/cua/worker")
            .warm_worker
            .is_none());
        assert!(PerceptionWorkerConfig::installed("/opt/cua/worker")
            .with_bounded_reuse(WarmWorkerPolicy::default())
            .warm_worker
            .is_some());
        let failure = PerceptionClient::new(PerceptionWorkerConfig::installed("/opt/cua/worker"))
            .err()
            .expect("unbound installed workers must be rejected");
        assert_eq!(failure.code, VisualParseErrorCode::ArtifactInvalid);
    }

    #[test]
    fn default_frame_limit_covers_the_default_capture_registry_limit() {
        let encoded_capture_bytes =
            ((crate::capture_registry::DEFAULT_MAX_CAPTURE_BYTES + 2) / 3) * 4;
        assert!(DEFAULT_MAX_FRAME_BYTES >= encoded_capture_bytes + 64 * 1024);
    }

    #[test]
    fn installed_extension_identity_is_launch_bound_and_verified() {
        let config = PerceptionWorkerConfig::installed_with_identity(
            "/opt/cua/worker",
            "cua-perception",
            "0.1.0",
        );
        assert_eq!(
            config.args,
            [
                "--extension-id",
                "cua-perception",
                "--extension-version",
                "0.1.0",
            ]
        );
        let expected = config.expected_extension_identity.as_ref();
        assert!(verify_expected_extension_identity(
            &json!({
                "identity": {
                    "extension": {"id": "cua-perception", "version": "0.1.0"}
                }
            }),
            expected,
        )
        .is_ok());

        let failure = verify_expected_extension_identity(
            &json!({
                "identity": {
                    "extension": {"id": "cua-perception", "version": "0.28.2"}
                }
            }),
            expected,
        )
        .unwrap_err();
        assert_eq!(failure.code, VisualParseErrorCode::ArtifactInvalid);
    }

    #[tokio::test]
    async fn installed_worker_health_must_echo_the_bound_identity() {
        let evidence = tempfile::tempdir().unwrap();
        let counter = evidence.path().join("parse-count");
        let (directory, worker) = fixture_worker("ok", Some(&counter));
        let args: Vec<String> =
            serde_json::from_slice(&std::fs::read(directory.path().join("args.json")).unwrap())
                .unwrap();
        let client = PerceptionClient::new(PerceptionWorkerConfig {
            executable: worker,
            containment: fixture_limits(&args),
            args,
            request_timeout: Duration::from_secs(10),
            max_frame_bytes: 1024 * 1024,
            warm_worker: None,
            expected_extension_identity: Some(ExpectedExtensionIdentity {
                id: "cua-perception".into(),
                version: "0.1.0".into(),
            }),
            installed: true,
        })
        .unwrap();
        let failure = client
            .parse(
                "capture-test",
                2,
                2,
                &[1, 2, 3, 4],
                &PerceptionCancellation::default(),
            )
            .await
            .unwrap_err();
        assert_eq!(failure.code, VisualParseErrorCode::ArtifactInvalid);
        assert!(
            !counter.exists(),
            "parse must not run after an unbound health response"
        );
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

    /// A stand-in for an installed extension's health or self-test hook. A
    /// shipped hook is the extension's own native binary, so as with the worker
    /// fixtures the interpreter is opted into the sandbox explicitly rather
    /// than by widening the boundary for everyone.
    fn fixture_hook(mode: &str) -> (tempfile::TempDir, PathBuf, Vec<String>) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fixture-hook.py");
        let script = r#"#!/usr/bin/env python3
import sys, time

mode = sys.argv[1]
if mode == 'chatty':
    sys.stdout.buffer.write(b'x' * (4 * 1024 * 1024))
    sys.stdout.buffer.flush()
elif mode == 'hang':
    time.sleep(120)
elif mode == 'fail':
    sys.exit(7)
sys.exit(0)
"#;
        std::fs::write(&path, with_fixture_interpreter(script)).unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&path, permissions).unwrap();
        (directory, path, vec![mode.to_owned()])
    }

    async fn run_fixture_hook(mode: &str, timeout: Duration) -> containment::HookOutcome {
        let (_directory, hook, args) = fixture_hook(mode);
        containment::run_contained_hook(&hook, &args, &fixture_limits(&args), timeout)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_contained_hook_reports_how_it_exited() {
        assert_eq!(
            run_fixture_hook("ok", Duration::from_secs(30)).await,
            containment::HookOutcome::Succeeded
        );
        assert!(matches!(
            run_fixture_hook("fail", Duration::from_secs(30)).await,
            containment::HookOutcome::Failed(_)
        ));
    }

    /// A hook that never exits is killed at its deadline rather than holding
    /// the install or status command open.
    #[tokio::test]
    async fn a_contained_hook_that_outlives_its_deadline_is_killed() {
        let started = Instant::now();
        assert_eq!(
            run_fixture_hook("hang", Duration::from_millis(500)).await,
            containment::HookOutcome::TimedOut
        );
        assert!(started.elapsed() < Duration::from_secs(30));
    }

    /// The hook's output is discarded as it arrives, so a hook that fills its
    /// pipe still finishes instead of blocking until its deadline.
    #[tokio::test]
    async fn a_contained_hook_that_floods_its_output_still_finishes() {
        assert_eq!(
            run_fixture_hook("chatty", Duration::from_secs(30)).await,
            containment::HookOutcome::Succeeded
        );
    }

    /// A hook gets no more authority than a parse: the same boundary refuses a
    /// launch it cannot fully contain.
    #[tokio::test]
    async fn a_contained_hook_refuses_a_missing_entrypoint() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("absent-hook");
        let failure = containment::run_contained_hook(
            &missing,
            &[],
            &ContainmentLimits::default(),
            Duration::from_secs(5),
        )
        .await
        .unwrap_err();
        assert!(matches!(
            failure.code,
            VisualParseErrorCode::WorkerLaunchFailed | VisualParseErrorCode::UnsupportedPlatform
        ));
    }
}

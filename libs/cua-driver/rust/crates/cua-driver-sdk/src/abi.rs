//! Stable C ABI below the public typed SDK.
//!
//! The exported functions in this module are deliberately smaller and more
//! conservative than UniFFI's generated ABI. They use versioned symbols,
//! opaque handles, caller-visible ownership, and status codes that a future
//! non-Rust native core can reproduce without implementing UniFFI internals.

use crate::runtime::{DriverRuntime, RuntimeCreateError, RuntimeOptions, RuntimeSession};
use crate::{DriverError, DriverMetadata};
use cua_driver_core::{
    authorization::{
        PermissionMode, DANGEROUS_BYPASS_ENV, DISABLE_UNRESTRICTED_ENV, PERMISSION_MODE_ENV,
    },
    session_authorization::{DelegatedSessionRequest, SessionModeCeiling},
    session_manifest::{
        load_manifest, CAPABILITY_MANIFEST_APPROVED_ENV, CAPABILITY_MANIFEST_FILE_ENV,
        SESSION_POLICY_APPROVED_ENV, SESSION_POLICY_FILE_ENV,
    },
};
use serde::Deserialize;
use serde_json::Value;
use std::ffi::c_void;
use std::future::Future;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, OnceLock,
};
use std::time::Duration;
use tokio::sync::{oneshot, Notify};

pub const CUA_DRIVER_ABI_MAJOR: u16 = 1;
pub const CUA_DRIVER_ABI_MINOR: u16 = 1;
pub const CUA_DRIVER_ABI_PATCH: u16 = 0;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Version of the implementation-neutral Cua Driver ABI.
pub struct CuaDriverAbiVersion {
    pub struct_size: u32,
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
    pub reserved: u16,
}

#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Stable status codes returned across the C ABI.
pub enum CuaDriverStatus {
    Ok = 0,
    InvalidArgument = 1,
    NullPointer = 2,
    RuntimeUnavailable = 3,
    Shutdown = 4,
    Cancelled = 5,
    Internal = 6,
    Panic = 7,
    RuntimeConflict = 8,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
/// Caller-owned byte buffer returned by the ABI.
/// Pass it to `cua_driver_buffer_free_v1`; freeing an empty buffer is harmless.
pub struct CuaDriverBuffer {
    pub data: *mut u8,
    pub len: usize,
    pub capacity: usize,
}

impl CuaDriverBuffer {
    const fn empty() -> Self {
        Self {
            data: ptr::null_mut(),
            len: 0,
            capacity: 0,
        }
    }

    fn from_string(value: String) -> Self {
        if value.is_empty() {
            return Self::empty();
        }
        let mut bytes = value.into_bytes();
        let buffer = Self {
            data: bytes.as_mut_ptr(),
            len: bytes.len(),
            capacity: bytes.capacity(),
        };
        std::mem::forget(bytes);
        buffer
    }
}

/// Opaque driver runtime handle.
pub struct CuaDriverHandle {
    runtime: Arc<DriverRuntime>,
}

/// Opaque session handle whose actions are already bound to one immutable
/// authorization context.
pub struct CuaDriverSessionHandle {
    session: Arc<RuntimeSession>,
}

struct OperationState {
    cancelled: AtomicBool,
    changed: Notify,
}

impl OperationState {
    fn new() -> Self {
        Self {
            cancelled: AtomicBool::new(false),
            changed: Notify::new(),
        }
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        // There is exactly one cancellation waiter per operation. `notify_one`
        // stores a permit when cancellation wins the race before that waiter
        // first polls; `notify_waiters` would lose that wake-up.
        self.changed.notify_one();
    }

    async fn cancelled(&self) {
        loop {
            let changed = self.changed.notified();
            if self.cancelled.load(Ordering::Acquire) {
                return;
            }
            changed.await;
        }
    }
}

/// Opaque token for one asynchronous operation.
pub struct CuaDriverOperation {
    state: Arc<OperationState>,
}

/// Completion callback for asynchronous operations. It is called exactly once
/// unless the process terminates. Result and error buffers are caller-owned.
pub type CuaDriverCompletionV1 = extern "C" fn(
    context: *mut c_void,
    status: CuaDriverStatus,
    result: CuaDriverBuffer,
    error: CuaDriverBuffer,
);

#[derive(Debug)]
struct AbiFailure {
    status: CuaDriverStatus,
    message: String,
}

impl AbiFailure {
    fn new(status: CuaDriverStatus, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct AbiDriverOptions {
    claude_code_compatibility: bool,
    authorization: Option<AbiRuntimeAuthorizationOptions>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AbiRuntimeAuthorizationOptions {
    allowed_modes: Vec<PermissionMode>,
    compatibility_mode: PermissionMode,
    #[serde(default)]
    compatibility_capability_manifest_path: Option<String>,
    #[serde(default)]
    compatibility_bounded_manifest_path: Option<String>,
    unrestricted_acknowledged: bool,
    max_session_ttl_seconds: u64,
    max_idle_ttl_seconds: u64,
}

impl AbiRuntimeAuthorizationOptions {
    fn capability_manifest_path(&self) -> Result<Option<&str>, AbiFailure> {
        if self.compatibility_capability_manifest_path.is_some()
            && self.compatibility_bounded_manifest_path.is_some()
            && self.compatibility_capability_manifest_path
                != self.compatibility_bounded_manifest_path
        {
            return Err(AbiFailure::new(
                CuaDriverStatus::InvalidArgument,
                "compatibility capability manifest path conflicts with its deprecated bounded-manifest alias",
            ));
        }
        Ok(self
            .compatibility_capability_manifest_path
            .as_deref()
            .or(self.compatibility_bounded_manifest_path.as_deref()))
    }
}

fn validate_explicit_authorization_sources(
    authorization: &AbiRuntimeAuthorizationOptions,
) -> Result<(), AbiFailure> {
    cua_driver_core::policy::validate_configured_policy().map_err(|error| {
        AbiFailure::new(
            CuaDriverStatus::InvalidArgument,
            format!("configured policy is invalid: {error}"),
        )
    })?;
    if std::env::var_os(PERMISSION_MODE_ENV).is_some()
        || std::env::var_os(DANGEROUS_BYPASS_ENV).is_some()
    {
        let compatibility_mode =
            cua_driver_core::authorization::configured_permission_mode().map_err(|error| {
                AbiFailure::new(
                    CuaDriverStatus::InvalidArgument,
                    format!(
                        "explicit runtime authorization conflicts with invalid compatibility environment: {error}"
                    ),
                )
            })?;
        if compatibility_mode != authorization.compatibility_mode {
            return Err(AbiFailure::new(
                CuaDriverStatus::InvalidArgument,
                format!(
                    "explicit compatibility mode '{}' conflicts with trusted environment mode '{}'",
                    authorization.compatibility_mode.as_str(),
                    compatibility_mode.as_str(),
                ),
            ));
        }
    }

    if authorization
        .allowed_modes
        .contains(&PermissionMode::Unrestricted)
        && environment_flag(DISABLE_UNRESTRICTED_ENV)
    {
        return Err(AbiFailure::new(
            CuaDriverStatus::InvalidArgument,
            "explicit runtime ceiling conflicts with managed configuration disabling unrestricted mode",
        ));
    }

    let capability_environment_manifest = std::env::var_os(CAPABILITY_MANIFEST_FILE_ENV);
    let legacy_environment_manifest = std::env::var_os(SESSION_POLICY_FILE_ENV);
    if capability_environment_manifest.is_some()
        && legacy_environment_manifest.is_some()
        && capability_environment_manifest != legacy_environment_manifest
    {
        return Err(AbiFailure::new(
            CuaDriverStatus::InvalidArgument,
            "capability manifest environment path conflicts with its deprecated session-policy alias",
        ));
    }
    let environment_manifest = capability_environment_manifest
        .or(legacy_environment_manifest)
        .map(std::path::PathBuf::from);
    if let Some(environment_manifest) = environment_manifest {
        let Some(explicit_manifest) = authorization
            .capability_manifest_path()?
            .map(std::path::Path::new)
        else {
            return Err(AbiFailure::new(
                CuaDriverStatus::InvalidArgument,
                "explicit runtime authorization conflicts with a compatibility capability-manifest environment value",
            ));
        };
        let environment_manifest =
            std::fs::canonicalize(&environment_manifest).map_err(|error| {
                AbiFailure::new(
                    CuaDriverStatus::InvalidArgument,
                    format!(
                        "canonicalize compatibility capability manifest {}: {error}",
                        environment_manifest.display()
                    ),
                )
            })?;
        let explicit_manifest = std::fs::canonicalize(explicit_manifest).map_err(|error| {
            AbiFailure::new(
                CuaDriverStatus::InvalidArgument,
                format!(
                    "canonicalize explicit compatibility capability manifest {}: {error}",
                    explicit_manifest.display()
                ),
            )
        })?;
        if environment_manifest != explicit_manifest {
            return Err(AbiFailure::new(
                CuaDriverStatus::InvalidArgument,
                "explicit compatibility capability manifest conflicts with the trusted environment path",
            ));
        }
    } else if (environment_flag(CAPABILITY_MANIFEST_APPROVED_ENV)
        || environment_flag(SESSION_POLICY_APPROVED_ENV))
        && authorization.capability_manifest_path()?.is_none()
    {
        return Err(AbiFailure::new(
            CuaDriverStatus::InvalidArgument,
            "explicit runtime authorization conflicts with a compatibility capability-manifest approval",
        ));
    }
    Ok(())
}

fn runtime_options_from_abi(options: AbiDriverOptions) -> Result<RuntimeOptions, AbiFailure> {
    match options.authorization {
        Some(authorization) => {
            validate_explicit_authorization_sources(&authorization)?;
            let ceiling = SessionModeCeiling::for_trusted_sessions(
                authorization.allowed_modes.clone(),
                authorization.unrestricted_acknowledged,
                Duration::from_secs(authorization.max_session_ttl_seconds),
                Duration::from_secs(authorization.max_idle_ttl_seconds),
            )
            .map_err(|error| AbiFailure::new(CuaDriverStatus::InvalidArgument, error))?;
            let manifest = authorization
                .capability_manifest_path()?
                .map(std::path::Path::new)
                .map(load_manifest)
                .transpose()
                .map_err(|error| AbiFailure::new(CuaDriverStatus::InvalidArgument, error))?
                .map(Arc::new);
            Ok(RuntimeOptions::embedded_with_ceiling(
                options.claude_code_compatibility,
                ceiling,
                authorization.compatibility_mode,
                manifest,
            ))
        }
        None => Ok(RuntimeOptions::embedded(options.claude_code_compatibility)),
    }
}

fn environment_flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AbiTrustedSessionOptions {
    public_session: String,
    mode: PermissionMode,
    ttl_seconds: u64,
    idle_ttl_seconds: u64,
    #[serde(default)]
    capability_manifest_path: Option<String>,
    #[serde(default)]
    bounded_manifest_path: Option<String>,
    /// Host-generated lifecycle lease identity. This field is accepted only
    /// by the in-process Rust bridge; it is not part of the generated public
    /// TrustedSessionOptions record or any agent-visible tool schema.
    #[serde(default)]
    transport_session: Option<String>,
}

impl AbiTrustedSessionOptions {
    fn capability_manifest_path(&self) -> Result<Option<&str>, AbiFailure> {
        if self.capability_manifest_path.is_some()
            && self.bounded_manifest_path.is_some()
            && self.capability_manifest_path != self.bounded_manifest_path
        {
            return Err(AbiFailure::new(
                CuaDriverStatus::InvalidArgument,
                "capability manifest path conflicts with its deprecated bounded-manifest alias",
            ));
        }
        Ok(self
            .capability_manifest_path
            .as_deref()
            .or(self.bounded_manifest_path.as_deref()))
    }
}

fn with_ffi_guard(
    out_error: *mut CuaDriverBuffer,
    operation: impl FnOnce() -> Result<(), AbiFailure>,
) -> CuaDriverStatus {
    unsafe {
        if !out_error.is_null() {
            *out_error = CuaDriverBuffer::empty();
        }
    }
    match catch_unwind(AssertUnwindSafe(operation)) {
        Ok(Ok(())) => CuaDriverStatus::Ok,
        Ok(Err(error)) => {
            unsafe { write_error(out_error, error.message) };
            error.status
        }
        Err(_) => {
            unsafe {
                write_error(
                    out_error,
                    "the native Cua Driver core panicked; no panic crossed the C ABI".into(),
                )
            };
            CuaDriverStatus::Panic
        }
    }
}

unsafe fn write_error(out_error: *mut CuaDriverBuffer, message: String) {
    if !out_error.is_null() {
        *out_error = CuaDriverBuffer::from_string(message);
    }
}

unsafe fn input_bytes<'a>(data: *const u8, len: usize) -> Result<&'a [u8], AbiFailure> {
    if len == 0 {
        return Ok(&[]);
    }
    if data.is_null() {
        return Err(AbiFailure::new(
            CuaDriverStatus::NullPointer,
            "non-empty input used a null data pointer",
        ));
    }
    Ok(std::slice::from_raw_parts(data, len))
}

unsafe fn required_handle<'a>(
    handle: *mut CuaDriverHandle,
) -> Result<&'a CuaDriverHandle, AbiFailure> {
    handle.as_ref().ok_or_else(|| {
        AbiFailure::new(
            CuaDriverStatus::NullPointer,
            "driver handle must not be null",
        )
    })
}

unsafe fn required_session_handle<'a>(
    handle: *mut CuaDriverSessionHandle,
) -> Result<&'a CuaDriverSessionHandle, AbiFailure> {
    handle.as_ref().ok_or_else(|| {
        AbiFailure::new(
            CuaDriverStatus::NullPointer,
            "session handle must not be null",
        )
    })
}

fn metadata_json() -> Result<String, AbiFailure> {
    serde_json::to_string(&DriverMetadata {
        driver_version: env!("CARGO_PKG_VERSION").into(),
        contract_version: cua_driver_contract::CONTRACT_VERSION.into(),
        tools_list_schema_version: cua_driver_contract::TOOLS_LIST_SCHEMA_VERSION.into(),
        capability_version: cua_driver_contract::CAPABILITY_VERSION.into(),
        mcp_protocol_version: cua_driver_contract::MCP_PROTOCOL_VERSION.into(),
        pid: std::process::id(),
        embedded: true,
        host_bundle_id: None,
    })
    .map_err(|error| AbiFailure::new(CuaDriverStatus::Internal, error.to_string()))
}

#[derive(Debug)]
struct AbiExecutor {
    runtime: tokio::runtime::Runtime,
    #[cfg(target_os = "windows")]
    threads: Arc<platform_windows::dpi::OwnedThreadDpi>,
}

impl AbiExecutor {
    fn handle(&self) -> &tokio::runtime::Handle {
        self.runtime.handle()
    }

    fn check_ready(&self) -> Result<(), AbiFailure> {
        #[cfg(target_os = "windows")]
        self.threads
            .check()
            .map_err(|error| AbiFailure::new(CuaDriverStatus::RuntimeUnavailable, error))?;
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.check_ready().is_ok()
    }
}

fn abi_executor() -> Result<&'static AbiExecutor, AbiFailure> {
    let executor = abi_executor_for_cleanup()?;
    executor.check_ready()?;
    Ok(executor)
}

fn embedded_runtime_is_available(
    runtime_running: bool,
    executor: Result<&AbiExecutor, AbiFailure>,
) -> bool {
    runtime_running && executor.is_ok_and(AbiExecutor::is_ready)
}

fn admit_trusted_session(executor: Result<&AbiExecutor, AbiFailure>) -> Result<(), AbiFailure> {
    executor?.check_ready()
}

// Shutdown must still revoke sessions and finalize recordings after DPI
// failure. It neither admits GUI work nor reports physical-pixel results.
fn abi_executor_for_cleanup() -> Result<&'static AbiExecutor, AbiFailure> {
    static EXECUTOR: OnceLock<Result<AbiExecutor, String>> = OnceLock::new();
    initialized_abi_executor(&EXECUTOR, build_abi_executor)
}

fn initialized_abi_executor<'a>(
    executor: &'a OnceLock<Result<AbiExecutor, String>>,
    initialize: impl FnOnce() -> Result<AbiExecutor, String>,
) -> Result<&'a AbiExecutor, AbiFailure> {
    match executor.get_or_init(initialize) {
        Ok(executor) => Ok(executor),
        Err(error) => Err(AbiFailure::new(
            CuaDriverStatus::RuntimeUnavailable,
            format!("create Cua Driver ABI executor: {error}"),
        )),
    }
}

fn build_abi_executor() -> Result<AbiExecutor, String> {
    #[cfg(target_os = "windows")]
    {
        return build_windows_abi_executor(Arc::new(
            platform_windows::dpi::use_per_monitor_v2_for_current_thread,
        ));
    }

    #[cfg(not(target_os = "windows"))]
    {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("cua-driver-abi")
            .enable_all()
            .build()
            .map(|runtime| AbiExecutor { runtime })
            .map_err(|error| error.to_string())
    }
}

#[cfg(target_os = "windows")]
fn build_windows_abi_executor(
    initialize_thread: Arc<dyn Fn() -> Result<(), String> + Send + Sync>,
) -> Result<AbiExecutor, String> {
    let threads = Arc::new(platform_windows::dpi::OwnedThreadDpi::new(
        initialize_thread,
    ));
    let callback_threads = threads.clone();
    let (thread_started, thread_ready) = std::sync::mpsc::channel();
    let mut builder = tokio::runtime::Builder::new_multi_thread();
    builder
        .worker_threads(2)
        .thread_name("cua-driver-abi")
        .enable_all()
        .on_thread_start(move || {
            callback_threads.initialize_current_thread();
            let _ = thread_started.send(());
        });
    let executor = builder.build().map_err(|error| error.to_string())?;

    // Validate both eagerly-created async workers and one blocking worker at
    // startup. This is NOT a substitute for the per-operation/per-thread gates:
    // the blocking pool can grow later and a task can migrate between workers.
    for _ in 0..2 {
        thread_ready
            .recv_timeout(Duration::from_secs(5))
            .map_err(|error| format!("start Cua Driver ABI async worker: {error}"))?;
    }

    let (blocking_started, blocking_ready) = std::sync::mpsc::channel();
    executor.spawn_blocking(move || {
        let _ = blocking_started.send(());
    });
    blocking_ready
        .recv_timeout(Duration::from_secs(5))
        .map_err(|error| format!("start Cua Driver ABI blocking worker: {error}"))?;

    threads.check()?;
    Ok(AbiExecutor {
        runtime: executor,
        threads,
    })
}

fn spawn_completion<F>(
    executor: tokio::runtime::Handle,
    future: F,
    callback: CuaDriverCompletionV1,
    context: *mut c_void,
) -> Result<*mut CuaDriverOperation, AbiFailure>
where
    F: Future<Output = Result<String, AbiFailure>> + Send + 'static,
{
    spawn_cleanup_completion(
        executor,
        async move {
            #[cfg(target_os = "windows")]
            {
                platform_windows::dpi::guard_future(future)
                    .await
                    .map_err(|error| AbiFailure::new(CuaDriverStatus::RuntimeUnavailable, error))?
            }
            #[cfg(not(target_os = "windows"))]
            {
                future.await
            }
        },
        callback,
        context,
    )
}

#[derive(Debug, Clone, Copy)]
enum InvokeMode {
    Regular,
    TrustedAdapter,
}

async fn invoke_runtime_to_json(
    runtime: Arc<DriverRuntime>,
    name: String,
    arguments: Value,
    mode: InvokeMode,
) -> Result<String, AbiFailure> {
    let shutdown_message = match mode {
        InvokeMode::Regular => "the Cua Driver SDK has been shut down",
        InvokeMode::TrustedAdapter => "the owning Cua Driver runtime has been shut down",
    };
    let result = match mode {
        InvokeMode::Regular => runtime.invoke(&name, arguments).await,
        InvokeMode::TrustedAdapter => runtime.invoke_from_trusted_adapter(&name, arguments).await,
    }
    .ok_or_else(|| AbiFailure::new(CuaDriverStatus::Shutdown, shutdown_message))?;
    serde_json::to_string(&result)
        .map_err(|error| AbiFailure::new(CuaDriverStatus::Internal, error.to_string()))
}

// All embedded GUI invocations, including trusted adapter calls, enter through
// this one dispatcher. The future is guarded on the ABI-owned executor, while
// OperationGuard preserves cancellation when a Rust caller drops its await.
fn start_invoke_completion(
    runtime: Arc<DriverRuntime>,
    name: String,
    arguments: Value,
    mode: InvokeMode,
    callback: CuaDriverCompletionV1,
    context: *mut c_void,
) -> Result<*mut CuaDriverOperation, AbiFailure> {
    let executor = abi_executor()?.handle().clone();
    spawn_completion(
        executor,
        invoke_runtime_to_json(runtime, name, arguments, mode),
        callback,
        context,
    )
}

fn spawn_cleanup_completion<F>(
    executor: tokio::runtime::Handle,
    future: F,
    callback: CuaDriverCompletionV1,
    context: *mut c_void,
) -> Result<*mut CuaDriverOperation, AbiFailure>
where
    F: Future<Output = Result<String, AbiFailure>> + Send + 'static,
{
    let state = Arc::new(OperationState::new());
    let operation = Box::new(CuaDriverOperation {
        state: state.clone(),
    });
    let context = context as usize;
    executor.spawn(async move {
        // A nested task converts a native panic into a JoinError so it cannot
        // unwind through either the callback or the exported C function.
        let mut work = tokio::spawn(future);
        let completed = tokio::select! {
            joined = &mut work => match joined {
                Ok(result) => result,
                Err(error) if error.is_panic() => Err(AbiFailure::new(
                    CuaDriverStatus::Panic,
                    "the native Cua Driver core panicked during an async operation",
                )),
                Err(error) => Err(AbiFailure::new(
                    CuaDriverStatus::Internal,
                    format!("native Cua Driver task failed: {error}"),
                )),
            },
            _ = state.cancelled() => {
                work.abort();
                Err(AbiFailure::new(
                    CuaDriverStatus::Cancelled,
                    "Cua Driver operation was cancelled",
                ))
            }
        };
        let (status, result, error) = match completed {
            Ok(result) => (
                CuaDriverStatus::Ok,
                CuaDriverBuffer::from_string(result),
                CuaDriverBuffer::empty(),
            ),
            Err(error) => (
                error.status,
                CuaDriverBuffer::empty(),
                CuaDriverBuffer::from_string(error.message),
            ),
        };
        callback(context as *mut c_void, status, result, error);
    });
    Ok(Box::into_raw(operation))
}

#[no_mangle]
/// Read the runtime ABI version into `out_version`.
pub unsafe extern "C" fn cua_driver_abi_version_v1(
    out_version: *mut CuaDriverAbiVersion,
) -> CuaDriverStatus {
    with_ffi_guard(ptr::null_mut(), || {
        let out_version = out_version.as_mut().ok_or_else(|| {
            AbiFailure::new(CuaDriverStatus::NullPointer, "out_version must not be null")
        })?;
        *out_version = CuaDriverAbiVersion {
            struct_size: std::mem::size_of::<CuaDriverAbiVersion>() as u32,
            major: CUA_DRIVER_ABI_MAJOR,
            minor: CUA_DRIVER_ABI_MINOR,
            patch: CUA_DRIVER_ABI_PATCH,
            reserved: 0,
        };
        Ok(())
    })
}

#[no_mangle]
/// Return whether this runtime supports a caller compiled for `major.minor`.
pub extern "C" fn cua_driver_abi_is_compatible_v1(major: u16, minor: u16) -> bool {
    major == CUA_DRIVER_ABI_MAJOR
        && matches!(
            minor.cmp(&CUA_DRIVER_ABI_MINOR),
            std::cmp::Ordering::Less | std::cmp::Ordering::Equal
        )
}

#[no_mangle]
/// Free and clear a buffer returned by this ABI. Repeated calls are harmless.
pub unsafe extern "C" fn cua_driver_buffer_free_v1(buffer: *mut CuaDriverBuffer) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let Some(buffer) = buffer.as_mut() else {
            return;
        };
        if !buffer.data.is_null() && buffer.capacity > 0 {
            drop(Vec::from_raw_parts(
                buffer.data,
                buffer.len,
                buffer.capacity,
            ));
        }
        *buffer = CuaDriverBuffer::empty();
    }));
}

#[no_mangle]
/// Create an in-process driver. `options_json` is empty or a UTF-8 JSON object.
/// It accepts `claude_code_compatibility` and an optional immutable
/// `authorization` ceiling. Unknown fields fail closed.
pub unsafe extern "C" fn cua_driver_create_v1(
    options_json: *const u8,
    options_len: usize,
    out_handle: *mut *mut CuaDriverHandle,
    out_error: *mut CuaDriverBuffer,
) -> CuaDriverStatus {
    with_ffi_guard(out_error, || {
        let out_handle = out_handle.as_mut().ok_or_else(|| {
            AbiFailure::new(CuaDriverStatus::NullPointer, "out_handle must not be null")
        })?;
        *out_handle = ptr::null_mut();
        let bytes = input_bytes(options_json, options_len)?;
        let options = if bytes.is_empty() {
            AbiDriverOptions::default()
        } else {
            serde_json::from_slice(bytes).map_err(|error| {
                AbiFailure::new(
                    CuaDriverStatus::InvalidArgument,
                    format!("invalid driver options JSON: {error}"),
                )
            })?
        };
        let runtime_options = runtime_options_from_abi(options)?;
        // The embedded runtime promises physical-pixel capture and input on
        // Windows. Refuse creation if its owned executor cannot establish
        // that contract instead of returning virtualized results later.
        abi_executor()?;
        let runtime = DriverRuntime::create(runtime_options).map_err(runtime_create_failure)?;
        *out_handle = Box::into_raw(Box::new(CuaDriverHandle { runtime }));
        Ok(())
    })
}

#[no_mangle]
/// Destroy and clear a driver handle. Repeated calls are harmless.
pub unsafe extern "C" fn cua_driver_destroy_v1(handle: *mut *mut CuaDriverHandle) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let Some(handle) = handle.as_mut() else {
            return;
        };
        let owned = std::mem::replace(handle, ptr::null_mut());
        if !owned.is_null() {
            drop(Box::from_raw(owned));
        }
    }));
}

#[no_mangle]
/// Return whether the driver still accepts operations.
pub unsafe extern "C" fn cua_driver_is_available_v1(
    handle: *mut CuaDriverHandle,
    out_available: *mut bool,
    out_error: *mut CuaDriverBuffer,
) -> CuaDriverStatus {
    with_ffi_guard(out_error, || {
        let driver = required_handle(handle)?;
        let out_available = out_available.as_mut().ok_or_else(|| {
            AbiFailure::new(
                CuaDriverStatus::NullPointer,
                "out_available must not be null",
            )
        })?;
        // `runtime.is_running()` only describes shutdown state. An embedded
        // Windows runtime is also unavailable once its owned executor has
        // lost the physical-pixel DPI contract.
        *out_available =
            embedded_runtime_is_available(driver.runtime.is_running(), abi_executor_for_cleanup());
        Ok(())
    })
}

#[no_mangle]
/// Return driver metadata as caller-owned UTF-8 JSON.
pub unsafe extern "C" fn cua_driver_metadata_json_v1(
    handle: *mut CuaDriverHandle,
    out_json: *mut CuaDriverBuffer,
    out_error: *mut CuaDriverBuffer,
) -> CuaDriverStatus {
    with_ffi_guard(out_error, || {
        let driver = required_handle(handle)?;
        if !driver.runtime.is_running() {
            return Err(AbiFailure::new(
                CuaDriverStatus::Shutdown,
                "the Cua Driver SDK has been shut down",
            ));
        }
        let out_json = out_json.as_mut().ok_or_else(|| {
            AbiFailure::new(CuaDriverStatus::NullPointer, "out_json must not be null")
        })?;
        *out_json = CuaDriverBuffer::from_string(metadata_json()?);
        Ok(())
    })
}

#[no_mangle]
/// Return the canonical SDK/MCP tool inventory as caller-owned UTF-8 JSON.
pub unsafe extern "C" fn cua_driver_list_tools_json_v1(
    handle: *mut CuaDriverHandle,
    out_json: *mut CuaDriverBuffer,
    out_error: *mut CuaDriverBuffer,
) -> CuaDriverStatus {
    with_ffi_guard(out_error, || {
        let driver = required_handle(handle)?;
        let tools = driver.runtime.tools_list().ok_or_else(|| {
            AbiFailure::new(
                CuaDriverStatus::Shutdown,
                "the Cua Driver SDK has been shut down",
            )
        })?;
        let out_json = out_json.as_mut().ok_or_else(|| {
            AbiFailure::new(CuaDriverStatus::NullPointer, "out_json must not be null")
        })?;
        *out_json = CuaDriverBuffer::from_string(
            serde_json::to_string(&tools)
                .map_err(|error| AbiFailure::new(CuaDriverStatus::Internal, error.to_string()))?,
        );
        Ok(())
    })
}

#[no_mangle]
/// Invoke a named tool asynchronously. Cancellation is requested with
/// `cua_driver_operation_cancel_v1`; release the token after completion.
pub unsafe extern "C" fn cua_driver_invoke_v1(
    handle: *mut CuaDriverHandle,
    name: *const u8,
    name_len: usize,
    arguments_json: *const u8,
    arguments_len: usize,
    callback: Option<CuaDriverCompletionV1>,
    context: *mut c_void,
    out_operation: *mut *mut CuaDriverOperation,
    out_error: *mut CuaDriverBuffer,
) -> CuaDriverStatus {
    with_ffi_guard(out_error, || {
        let driver = required_handle(handle)?;
        let callback = callback.ok_or_else(|| {
            AbiFailure::new(
                CuaDriverStatus::NullPointer,
                "completion callback must not be null",
            )
        })?;
        let out_operation = out_operation.as_mut().ok_or_else(|| {
            AbiFailure::new(
                CuaDriverStatus::NullPointer,
                "out_operation must not be null",
            )
        })?;
        *out_operation = ptr::null_mut();
        let name = std::str::from_utf8(input_bytes(name, name_len)?)
            .map_err(|error| {
                AbiFailure::new(
                    CuaDriverStatus::InvalidArgument,
                    format!("tool name must be UTF-8: {error}"),
                )
            })?
            .to_owned();
        if name.is_empty() {
            return Err(AbiFailure::new(
                CuaDriverStatus::InvalidArgument,
                "tool name must not be empty",
            ));
        }
        let arguments: Value = serde_json::from_slice(input_bytes(arguments_json, arguments_len)?)
            .map_err(|error| {
                AbiFailure::new(
                    CuaDriverStatus::InvalidArgument,
                    format!("invalid tool arguments JSON: {error}"),
                )
            })?;
        if !arguments.is_object() {
            return Err(AbiFailure::new(
                CuaDriverStatus::InvalidArgument,
                "tool arguments must be a JSON object",
            ));
        }
        *out_operation = start_invoke_completion(
            driver.runtime.clone(),
            name,
            arguments,
            InvokeMode::Regular,
            callback,
            context,
        )?;
        Ok(())
    })
}

#[no_mangle]
/// Create a trusted, immutable session binding below this runtime's ceiling.
///
/// This is a host API, not an agent tool. The returned handle carries its
/// authority in memory and cannot be reconstructed from a public session ID.
pub unsafe extern "C" fn cua_driver_session_create_v1(
    handle: *mut CuaDriverHandle,
    options_json: *const u8,
    options_len: usize,
    out_session: *mut *mut CuaDriverSessionHandle,
    out_error: *mut CuaDriverBuffer,
) -> CuaDriverStatus {
    with_ffi_guard(out_error, || {
        let driver = required_handle(handle)?;
        let out_session = out_session.as_mut().ok_or_else(|| {
            AbiFailure::new(CuaDriverStatus::NullPointer, "out_session must not be null")
        })?;
        *out_session = ptr::null_mut();
        let options: AbiTrustedSessionOptions =
            serde_json::from_slice(input_bytes(options_json, options_len)?).map_err(|error| {
                AbiFailure::new(
                    CuaDriverStatus::InvalidArgument,
                    format!("invalid trusted session options JSON: {error}"),
                )
            })?;
        if options.public_session.trim().is_empty() {
            return Err(AbiFailure::new(
                CuaDriverStatus::InvalidArgument,
                "public_session must not be empty",
            ));
        }
        // Creating a trusted session is a new GUI-work admission boundary. Do
        // not issue fresh authority after the embedded executor has entered a
        // terminal DPI failure; cleanup of existing sessions remains allowed.
        admit_trusted_session(abi_executor_for_cleanup())?;
        let capability_manifest = options
            .capability_manifest_path()?
            .map(|path| {
                cua_driver_core::session_manifest::load_manifest(std::path::Path::new(path))
                    .map(Arc::new)
                    .map_err(|error| {
                        AbiFailure::new(
                            CuaDriverStatus::InvalidArgument,
                            format!("invalid capability manifest: {error}"),
                        )
                    })
            })
            .transpose()?;
        let request = DelegatedSessionRequest {
            public_session: options.public_session,
            transport_session: options
                .transport_session
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| format!("direct-{}", uuid::Uuid::new_v4())),
            mode: options.mode,
            ttl: Duration::from_secs(options.ttl_seconds),
            idle_ttl: Duration::from_secs(options.idle_ttl_seconds),
            capability_manifest,
        };
        let session = driver
            .runtime
            .create_trusted_session(request)
            .map_err(|error| {
                let status = if matches!(
                    error,
                    cua_driver_core::session_authorization::SessionAuthorizationError::RuntimeUnavailable
                ) {
                    CuaDriverStatus::Shutdown
                } else {
                    CuaDriverStatus::InvalidArgument
                };
                AbiFailure::new(status, error.to_string())
            })?;
        *out_session = Box::into_raw(Box::new(CuaDriverSessionHandle { session }));
        Ok(())
    })
}

#[no_mangle]
/// Destroy a trusted session handle and revoke its connection-bound grants.
pub unsafe extern "C" fn cua_driver_session_destroy_v1(handle: *mut *mut CuaDriverSessionHandle) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let Some(handle) = handle.as_mut() else {
            return;
        };
        let owned = std::mem::replace(handle, ptr::null_mut());
        if !owned.is_null() {
            drop(Box::from_raw(owned));
        }
    }));
}

#[no_mangle]
/// Invoke through an already-bound trusted session context.
pub unsafe extern "C" fn cua_driver_session_invoke_v1(
    handle: *mut CuaDriverSessionHandle,
    name: *const u8,
    name_len: usize,
    arguments_json: *const u8,
    arguments_len: usize,
    callback: Option<CuaDriverCompletionV1>,
    context: *mut c_void,
    out_operation: *mut *mut CuaDriverOperation,
    out_error: *mut CuaDriverBuffer,
) -> CuaDriverStatus {
    with_ffi_guard(out_error, || {
        let session = required_session_handle(handle)?;
        let callback = callback.ok_or_else(|| {
            AbiFailure::new(
                CuaDriverStatus::NullPointer,
                "completion callback must not be null",
            )
        })?;
        let out_operation = out_operation.as_mut().ok_or_else(|| {
            AbiFailure::new(
                CuaDriverStatus::NullPointer,
                "out_operation must not be null",
            )
        })?;
        *out_operation = ptr::null_mut();
        let name = std::str::from_utf8(input_bytes(name, name_len)?)
            .map_err(|error| {
                AbiFailure::new(
                    CuaDriverStatus::InvalidArgument,
                    format!("tool name must be UTF-8: {error}"),
                )
            })?
            .to_owned();
        if name.is_empty() {
            return Err(AbiFailure::new(
                CuaDriverStatus::InvalidArgument,
                "tool name must not be empty",
            ));
        }
        let arguments: Value = serde_json::from_slice(input_bytes(arguments_json, arguments_len)?)
            .map_err(|error| {
                AbiFailure::new(
                    CuaDriverStatus::InvalidArgument,
                    format!("invalid tool arguments JSON: {error}"),
                )
            })?;
        if !arguments.is_object() {
            return Err(AbiFailure::new(
                CuaDriverStatus::InvalidArgument,
                "tool arguments must be a JSON object",
            ));
        }
        let session = session.session.clone();
        let executor = abi_executor()?.handle().clone();
        *out_operation = spawn_completion(
            executor,
            async move {
                let result = session.invoke(&name, arguments).await.ok_or_else(|| {
                    AbiFailure::new(
                        CuaDriverStatus::Shutdown,
                        "the owning Cua Driver runtime has been shut down",
                    )
                })?;
                serde_json::to_string(&result)
                    .map_err(|error| AbiFailure::new(CuaDriverStatus::Internal, error.to_string()))
            },
            callback,
            context,
        )?;
        Ok(())
    })
}

#[no_mangle]
/// Stop admission, drain admitted calls, and finalize SDK-owned resources.
pub unsafe extern "C" fn cua_driver_shutdown_v1(
    handle: *mut CuaDriverHandle,
    callback: Option<CuaDriverCompletionV1>,
    context: *mut c_void,
    out_operation: *mut *mut CuaDriverOperation,
    out_error: *mut CuaDriverBuffer,
) -> CuaDriverStatus {
    with_ffi_guard(out_error, || {
        let driver = required_handle(handle)?;
        let callback = callback.ok_or_else(|| {
            AbiFailure::new(
                CuaDriverStatus::NullPointer,
                "completion callback must not be null",
            )
        })?;
        let out_operation = out_operation.as_mut().ok_or_else(|| {
            AbiFailure::new(
                CuaDriverStatus::NullPointer,
                "out_operation must not be null",
            )
        })?;
        *out_operation = ptr::null_mut();
        let runtime = driver.runtime.clone();
        let executor = abi_executor_for_cleanup()?.handle().clone();
        *out_operation = spawn_cleanup_completion(
            executor,
            async move {
                runtime.shutdown().await;
                Ok("null".into())
            },
            callback,
            context,
        )?;
        Ok(())
    })
}

#[no_mangle]
/// Request cancellation of an asynchronous operation.
pub unsafe extern "C" fn cua_driver_operation_cancel_v1(operation: *mut CuaDriverOperation) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if let Some(operation) = operation.as_ref() {
            operation.state.cancel();
        }
    }));
}

#[no_mangle]
/// Release and clear an operation token. Repeated calls are harmless.
pub unsafe extern "C" fn cua_driver_operation_release_v1(operation: *mut *mut CuaDriverOperation) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let Some(operation) = operation.as_mut() else {
            return;
        };
        let owned = std::mem::replace(operation, ptr::null_mut());
        if !owned.is_null() {
            drop(Box::from_raw(owned));
        }
    }));
}

struct CallbackResult {
    status: CuaDriverStatus,
    result: String,
    error: String,
}

struct CallbackContext {
    sender: oneshot::Sender<CallbackResult>,
}

extern "C" fn rust_completion(
    context: *mut c_void,
    status: CuaDriverStatus,
    mut result: CuaDriverBuffer,
    mut error: CuaDriverBuffer,
) {
    let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
        let context = Box::from_raw(context.cast::<CallbackContext>());
        let result_text = copy_and_free_buffer(&mut result);
        let error_text = copy_and_free_buffer(&mut error);
        let _ = context.sender.send(CallbackResult {
            status,
            result: result_text,
            error: error_text,
        });
    }));
}

// Deliberately import the exported symbols back through their C linkage. This
// keeps the safe Rust SDK on the same ABI seam as Python, TypeScript, and C
// consumers instead of reaching around the boundary through Rust functions.
mod ffi {
    use super::{c_void, CuaDriverBuffer, CuaDriverCompletionV1, CuaDriverStatus};

    #[repr(C)]
    pub(super) struct Handle {
        _private: [u8; 0],
    }

    #[repr(C)]
    pub(super) struct Operation {
        _private: [u8; 0],
    }

    #[repr(C)]
    pub(super) struct Session {
        _private: [u8; 0],
    }

    unsafe extern "C" {
        #[link_name = "cua_driver_buffer_free_v1"]
        pub(super) fn buffer_free(buffer: *mut CuaDriverBuffer);
        #[link_name = "cua_driver_create_v1"]
        pub(super) fn create(
            options_json: *const u8,
            options_len: usize,
            out_handle: *mut *mut Handle,
            out_error: *mut CuaDriverBuffer,
        ) -> CuaDriverStatus;
        #[link_name = "cua_driver_destroy_v1"]
        pub(super) fn destroy(handle: *mut *mut Handle);
        #[link_name = "cua_driver_is_available_v1"]
        pub(super) fn is_available(
            handle: *mut Handle,
            out_available: *mut bool,
            out_error: *mut CuaDriverBuffer,
        ) -> CuaDriverStatus;
        #[link_name = "cua_driver_metadata_json_v1"]
        pub(super) fn metadata_json(
            handle: *mut Handle,
            out_json: *mut CuaDriverBuffer,
            out_error: *mut CuaDriverBuffer,
        ) -> CuaDriverStatus;
        #[link_name = "cua_driver_list_tools_json_v1"]
        pub(super) fn list_tools_json(
            handle: *mut Handle,
            out_json: *mut CuaDriverBuffer,
            out_error: *mut CuaDriverBuffer,
        ) -> CuaDriverStatus;
        #[link_name = "cua_driver_invoke_v1"]
        pub(super) fn invoke(
            handle: *mut Handle,
            name: *const u8,
            name_len: usize,
            arguments_json: *const u8,
            arguments_len: usize,
            callback: Option<CuaDriverCompletionV1>,
            context: *mut c_void,
            out_operation: *mut *mut Operation,
            out_error: *mut CuaDriverBuffer,
        ) -> CuaDriverStatus;
        #[link_name = "cua_driver_session_create_v1"]
        pub(super) fn session_create(
            handle: *mut Handle,
            options_json: *const u8,
            options_len: usize,
            out_session: *mut *mut Session,
            out_error: *mut CuaDriverBuffer,
        ) -> CuaDriverStatus;
        #[link_name = "cua_driver_session_destroy_v1"]
        pub(super) fn session_destroy(handle: *mut *mut Session);
        #[link_name = "cua_driver_session_invoke_v1"]
        pub(super) fn session_invoke(
            handle: *mut Session,
            name: *const u8,
            name_len: usize,
            arguments_json: *const u8,
            arguments_len: usize,
            callback: Option<CuaDriverCompletionV1>,
            context: *mut c_void,
            out_operation: *mut *mut Operation,
            out_error: *mut CuaDriverBuffer,
        ) -> CuaDriverStatus;
        #[link_name = "cua_driver_shutdown_v1"]
        pub(super) fn shutdown(
            handle: *mut Handle,
            callback: Option<CuaDriverCompletionV1>,
            context: *mut c_void,
            out_operation: *mut *mut Operation,
            out_error: *mut CuaDriverBuffer,
        ) -> CuaDriverStatus;
        #[link_name = "cua_driver_operation_cancel_v1"]
        pub(super) fn operation_cancel(operation: *mut Operation);
        #[link_name = "cua_driver_operation_release_v1"]
        pub(super) fn operation_release(operation: *mut *mut Operation);
    }
}

unsafe fn copy_and_free_buffer(buffer: &mut CuaDriverBuffer) -> String {
    let value = if buffer.data.is_null() || buffer.len == 0 {
        String::new()
    } else {
        String::from_utf8_lossy(std::slice::from_raw_parts(buffer.data, buffer.len)).into_owned()
    };
    ffi::buffer_free(buffer);
    value
}

struct OperationGuard {
    operation: *mut ffi::Operation,
    completed: bool,
}

// The operation pointer is an owned token whose async work holds a separate
// Arc. Cancellation and release are thread-safe operations on that token.
unsafe impl Send for OperationGuard {}

impl OperationGuard {
    fn complete(mut self) {
        self.completed = true;
    }
}

impl Drop for OperationGuard {
    fn drop(&mut self) {
        unsafe {
            if !self.completed {
                ffi::operation_cancel(self.operation);
            }
            ffi::operation_release(&mut self.operation);
        }
    }
}

/// Safe Rust owner for the versioned native handle. All public embedded SDK
/// operations pass through the exported C ABI even while the native core is
/// statically linked into the same distribution.
pub(crate) struct NativeAbiDriver {
    handle: Mutex<*mut ffi::Handle>,
    runtime_scope_key: String,
}

pub(crate) struct NativeAbiSession {
    handle: Mutex<*mut ffi::Session>,
}

// The pointer is an owned opaque handle. Operations only read it while `self`
// is borrowed, and destruction requires exclusive `Drop` access.
unsafe impl Send for NativeAbiDriver {}
unsafe impl Sync for NativeAbiDriver {}

impl NativeAbiDriver {
    pub(crate) fn create(claude_code_compatibility: bool) -> Result<Self, DriverError> {
        Self::create_configured(serde_json::json!({
            "claude_code_compatibility": claude_code_compatibility,
        }))
    }

    pub(crate) fn create_configured(options: Value) -> Result<Self, DriverError> {
        let options = options.to_string();
        let mut handle = ptr::null_mut();
        let mut error = CuaDriverBuffer::empty();
        let status =
            unsafe { ffi::create(options.as_ptr(), options.len(), &mut handle, &mut error) };
        status_result(status, &mut error, "create embedded runtime")?;
        let runtime_scope_key = unsafe {
            handle
                .cast::<CuaDriverHandle>()
                .as_ref()
                .expect("successful ABI creation returns a non-null handle")
                .runtime
                .runtime_scope_key()
        };
        Ok(Self {
            handle: Mutex::new(handle),
            runtime_scope_key,
        })
    }

    pub(crate) fn create_configured_with_authorization_provider(
        options: Value,
        provider: Arc<dyn cua_driver_core::consent::ProtectedConsentProvider>,
        activity_observer: Option<Arc<dyn crate::DriverActivityObserver>>,
    ) -> Result<Self, DriverError> {
        let options: AbiDriverOptions =
            serde_json::from_value(options).map_err(|error| DriverError::Configuration {
                reason: format!("invalid configured host options: {error}"),
            })?;
        let mut runtime_options =
            runtime_options_from_abi(options).map_err(|error| DriverError::Configuration {
                reason: error.message,
            })?;
        runtime_options.authorization_host = Some(provider);
        runtime_options.activity_observer = activity_observer;
        Self::create_for_host(runtime_options)
    }

    pub(crate) fn create_configured_with_activity_observer(
        options: Value,
        observer: Arc<dyn crate::DriverActivityObserver>,
    ) -> Result<Self, DriverError> {
        let options: AbiDriverOptions =
            serde_json::from_value(options).map_err(|error| DriverError::Configuration {
                reason: format!("invalid configured host options: {error}"),
            })?;
        let mut runtime_options =
            runtime_options_from_abi(options).map_err(|error| DriverError::Configuration {
                reason: error.message,
            })?;
        runtime_options.activity_observer = Some(observer);
        Self::create_for_host(runtime_options)
    }

    pub(crate) fn create_for_host(options: RuntimeOptions) -> Result<Self, DriverError> {
        abi_executor().map_err(|error| DriverError::Protocol {
            reason: error.message,
        })?;
        let runtime = DriverRuntime::create(options).map_err(map_runtime_create_error)?;
        let runtime_scope_key = runtime.runtime_scope_key();
        let handle = Box::into_raw(Box::new(CuaDriverHandle { runtime })).cast::<ffi::Handle>();
        Ok(Self {
            handle: Mutex::new(handle),
            runtime_scope_key,
        })
    }

    // This is the narrow ABI bridge between the public host options and the
    // runtime options. Keeping the fields explicit avoids exposing the
    // internal RuntimeOptions type across the bridge.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn create_configured_for_host(
        options: Value,
        cursor: cursor_overlay::CursorConfig,
        host_owns_permission_ux: bool,
        host_bundle_id: Option<String>,
        prepare_desktop_environment: bool,
        register_host_tools: Option<fn(&mut cua_driver_core::tool::ToolRegistry)>,
        authorization_host: Option<Arc<dyn cua_driver_core::consent::ProtectedConsentProvider>>,
        activity_observer: Option<Arc<dyn crate::DriverActivityObserver>>,
    ) -> Result<Self, DriverError> {
        let options: AbiDriverOptions =
            serde_json::from_value(options).map_err(|error| DriverError::Configuration {
                reason: format!("invalid configured host options: {error}"),
            })?;
        let mut runtime_options =
            runtime_options_from_abi(options).map_err(|error| DriverError::Configuration {
                reason: error.message,
            })?;
        runtime_options.cursor = cursor;
        runtime_options.host_owns_permission_ux = host_owns_permission_ux;
        runtime_options.host_bundle_id = host_bundle_id;
        runtime_options.prepare_desktop_environment = prepare_desktop_environment;
        runtime_options.register_host_tools = register_host_tools;
        runtime_options.authorization_host = authorization_host;
        runtime_options.activity_observer = activity_observer;
        Self::create_for_host(runtime_options)
    }

    fn raw_handle(&self) -> *mut ffi::Handle {
        *self.handle.lock().unwrap()
    }

    pub(crate) fn runtime_scope_key(&self) -> &str {
        &self.runtime_scope_key
    }

    pub(crate) fn history(&self) -> Option<Arc<cua_driver_core::history::HistoryManager>> {
        unsafe {
            self.raw_handle()
                .cast::<CuaDriverHandle>()
                .as_ref()
                .and_then(|handle| handle.runtime.history())
        }
    }

    pub(crate) fn is_available(&self) -> bool {
        let mut available = false;
        let mut error = CuaDriverBuffer::empty();
        let status = unsafe { ffi::is_available(self.raw_handle(), &mut available, &mut error) };
        let _ = status_result(status, &mut error, "query embedded runtime");
        status == CuaDriverStatus::Ok && available
    }

    pub(crate) fn metadata(&self) -> Result<DriverMetadata, DriverError> {
        let json = self.read_json(ffi::metadata_json, "read driver metadata")?;
        serde_json::from_str(&json).map_err(|error| DriverError::Protocol {
            reason: format!("invalid native metadata JSON: {error}"),
        })
    }

    pub(crate) fn tools_list(&self) -> Result<Value, DriverError> {
        let json = self.read_json(ffi::list_tools_json, "list native tools")?;
        serde_json::from_str(&json).map_err(|error| DriverError::Protocol {
            reason: format!("invalid native tools JSON: {error}"),
        })
    }

    pub(crate) fn create_session(&self, options: Value) -> Result<NativeAbiSession, DriverError> {
        let options = options.to_string();
        let mut handle = ptr::null_mut();
        let mut error = CuaDriverBuffer::empty();
        let status = unsafe {
            ffi::session_create(
                self.raw_handle(),
                options.as_ptr(),
                options.len(),
                &mut handle,
                &mut error,
            )
        };
        status_result(status, &mut error, "create trusted session")?;
        Ok(NativeAbiSession {
            handle: Mutex::new(handle),
        })
    }

    fn read_json(
        &self,
        function: unsafe extern "C" fn(
            *mut ffi::Handle,
            *mut CuaDriverBuffer,
            *mut CuaDriverBuffer,
        ) -> CuaDriverStatus,
        operation: &str,
    ) -> Result<String, DriverError> {
        let mut result = CuaDriverBuffer::empty();
        let mut error = CuaDriverBuffer::empty();
        let status = unsafe { function(self.raw_handle(), &mut result, &mut error) };
        status_result(status, &mut error, operation)?;
        Ok(unsafe { copy_and_free_buffer(&mut result) })
    }

    pub(crate) async fn invoke(&self, name: &str, arguments: Value) -> Result<Value, DriverError> {
        if self.raw_handle().is_null() {
            return Err(DriverError::Shutdown);
        }
        let arguments = serde_json::to_vec(&arguments).map_err(|error| DriverError::Protocol {
            reason: format!("serialize {name} ABI arguments: {error}"),
        })?;
        let (receiver, guard) = {
            let (sender, receiver) = oneshot::channel();
            let context = Box::into_raw(Box::new(CallbackContext { sender })).cast::<c_void>();
            let mut operation = ptr::null_mut();
            let mut error = CuaDriverBuffer::empty();
            let status = unsafe {
                ffi::invoke(
                    self.raw_handle(),
                    name.as_ptr(),
                    name.len(),
                    arguments.as_ptr(),
                    arguments.len(),
                    Some(rust_completion),
                    context,
                    &mut operation,
                    &mut error,
                )
            };
            if status != CuaDriverStatus::Ok {
                unsafe { drop(Box::from_raw(context.cast::<CallbackContext>())) };
                status_result(status, &mut error, name)?;
                return Err(DriverError::Protocol {
                    reason: format!("{name} ABI invocation failed without an error"),
                });
            }
            (
                receiver,
                OperationGuard {
                    operation,
                    completed: false,
                },
            )
        };
        let completed = receiver.await.map_err(|_| DriverError::Protocol {
            reason: format!("{name} ABI completion callback was dropped"),
        })?;
        guard.complete();
        if completed.status != CuaDriverStatus::Ok {
            return Err(map_status(completed.status, completed.error, name));
        }
        serde_json::from_str(&completed.result).map_err(|error| DriverError::Protocol {
            reason: format!("{name} returned invalid native JSON: {error}"),
        })
    }

    pub(crate) async fn invoke_from_trusted_adapter(
        &self,
        name: &str,
        arguments: Value,
    ) -> Result<Value, DriverError> {
        let runtime = {
            let handle = *self.handle.lock().unwrap();
            if handle.is_null() {
                return Err(DriverError::Shutdown);
            }
            unsafe {
                handle
                    .cast::<CuaDriverHandle>()
                    .as_ref()
                    .expect("live ABI handle is non-null")
                    .runtime
                    .clone()
            }
        };
        if !arguments.is_object() {
            return Err(DriverError::InvalidArguments {
                tool: name.into(),
                reason: "arguments must be a JSON object".into(),
            });
        }

        let (receiver, guard) = {
            let (sender, receiver) = oneshot::channel();
            let context = Box::into_raw(Box::new(CallbackContext { sender })).cast::<c_void>();
            let operation = match start_invoke_completion(
                runtime,
                name.to_owned(),
                arguments,
                InvokeMode::TrustedAdapter,
                rust_completion,
                context,
            ) {
                Ok(operation) => operation,
                Err(error) => {
                    unsafe { drop(Box::from_raw(context.cast::<CallbackContext>())) };
                    return Err(map_status(error.status, error.message, name));
                }
            };
            (
                receiver,
                OperationGuard {
                    operation: operation.cast::<ffi::Operation>(),
                    completed: false,
                },
            )
        };
        let completed = receiver.await.map_err(|_| DriverError::Protocol {
            reason: format!("{name} trusted-adapter completion callback was dropped"),
        })?;
        guard.complete();
        if completed.status != CuaDriverStatus::Ok {
            return Err(map_status(completed.status, completed.error, name));
        }
        serde_json::from_str(&completed.result).map_err(|error| DriverError::Protocol {
            reason: format!("{name} returned invalid trusted-adapter JSON: {error}"),
        })
    }

    pub(crate) async fn shutdown(&self) -> Result<(), DriverError> {
        let (receiver, guard) = {
            let (sender, receiver) = oneshot::channel();
            let context = Box::into_raw(Box::new(CallbackContext { sender })).cast::<c_void>();
            let mut operation = ptr::null_mut();
            let mut error = CuaDriverBuffer::empty();
            let status = unsafe {
                ffi::shutdown(
                    self.raw_handle(),
                    Some(rust_completion),
                    context,
                    &mut operation,
                    &mut error,
                )
            };
            if status != CuaDriverStatus::Ok {
                unsafe { drop(Box::from_raw(context.cast::<CallbackContext>())) };
                status_result(status, &mut error, "shutdown embedded runtime")?;
                return Err(DriverError::Protocol {
                    reason: "shutdown ABI invocation failed without an error".into(),
                });
            }
            (
                receiver,
                OperationGuard {
                    operation,
                    completed: false,
                },
            )
        };
        let completed = receiver.await.map_err(|_| DriverError::Protocol {
            reason: "shutdown ABI completion callback was dropped".into(),
        })?;
        guard.complete();
        if completed.status == CuaDriverStatus::Ok {
            Ok(())
        } else {
            Err(map_status(
                completed.status,
                completed.error,
                "shutdown embedded runtime",
            ))
        }
    }
}

impl Drop for NativeAbiDriver {
    fn drop(&mut self) {
        let handle = self.handle.get_mut().unwrap();
        unsafe { ffi::destroy(handle) };
    }
}

unsafe impl Send for NativeAbiSession {}
unsafe impl Sync for NativeAbiSession {}

impl NativeAbiSession {
    pub(crate) fn close(&self) {
        let mut handle = self.handle.lock().unwrap();
        unsafe { ffi::session_destroy(&mut *handle) };
    }

    pub(crate) async fn invoke(&self, name: &str, arguments: Value) -> Result<Value, DriverError> {
        let arguments = serde_json::to_vec(&arguments).map_err(|error| DriverError::Protocol {
            reason: format!("serialize {name} session ABI arguments: {error}"),
        })?;
        let (receiver, guard) = {
            // Keep the session-handle lock through the synchronous FFI call.
            // `close()` is exported on the generated-language object and may
            // run concurrently from another thread/finalizer; reading the
            // pointer and then dropping the lock would let close free it
            // before `session_invoke` has retained the underlying session.
            let handle = self.handle.lock().unwrap();
            if handle.is_null() {
                return Err(DriverError::Shutdown);
            }
            let (sender, receiver) = oneshot::channel();
            let context = Box::into_raw(Box::new(CallbackContext { sender })).cast::<c_void>();
            let mut operation = ptr::null_mut();
            let mut error = CuaDriverBuffer::empty();
            let status = unsafe {
                ffi::session_invoke(
                    *handle,
                    name.as_ptr(),
                    name.len(),
                    arguments.as_ptr(),
                    arguments.len(),
                    Some(rust_completion),
                    context,
                    &mut operation,
                    &mut error,
                )
            };
            if status != CuaDriverStatus::Ok {
                unsafe { drop(Box::from_raw(context.cast::<CallbackContext>())) };
                status_result(status, &mut error, name)?;
                return Err(DriverError::Protocol {
                    reason: format!("{name} session ABI invocation failed without an error"),
                });
            }
            (
                receiver,
                OperationGuard {
                    operation,
                    completed: false,
                },
            )
        };
        let completed = receiver.await.map_err(|_| DriverError::Protocol {
            reason: format!("{name} session ABI completion callback was dropped"),
        })?;
        guard.complete();
        if completed.status != CuaDriverStatus::Ok {
            return Err(map_status(completed.status, completed.error, name));
        }
        serde_json::from_str(&completed.result).map_err(|error| DriverError::Protocol {
            reason: format!("{name} returned invalid session JSON: {error}"),
        })
    }
}

impl Drop for NativeAbiSession {
    fn drop(&mut self) {
        let handle = self.handle.get_mut().unwrap();
        unsafe { ffi::session_destroy(handle) };
    }
}

fn status_result(
    status: CuaDriverStatus,
    error: &mut CuaDriverBuffer,
    operation: &str,
) -> Result<(), DriverError> {
    let message = unsafe { copy_and_free_buffer(error) };
    if status == CuaDriverStatus::Ok {
        Ok(())
    } else {
        Err(map_status(status, message, operation))
    }
}

fn map_status(status: CuaDriverStatus, message: String, operation: &str) -> DriverError {
    match status {
        CuaDriverStatus::Shutdown => DriverError::Shutdown,
        CuaDriverStatus::RuntimeConflict => DriverError::RuntimeAlreadyExists,
        CuaDriverStatus::InvalidArgument | CuaDriverStatus::NullPointer => {
            DriverError::Configuration { reason: message }
        }
        _ => DriverError::Protocol {
            reason: format!("{operation}: {message}"),
        },
    }
}

fn runtime_create_failure(error: RuntimeCreateError) -> AbiFailure {
    match error {
        RuntimeCreateError::AlreadyExists => AbiFailure::new(
            CuaDriverStatus::RuntimeConflict,
            "runtime_already_exists: one direct Cua Driver runtime is already active in this process",
        ),
        RuntimeCreateError::Authorization(reason) => {
            AbiFailure::new(CuaDriverStatus::InvalidArgument, reason)
        }
        RuntimeCreateError::Unavailable(reason) => {
            AbiFailure::new(CuaDriverStatus::RuntimeUnavailable, reason)
        }
    }
}

fn map_runtime_create_error(error: RuntimeCreateError) -> DriverError {
    match error {
        RuntimeCreateError::AlreadyExists => DriverError::RuntimeAlreadyExists,
        RuntimeCreateError::Authorization(reason) => DriverError::Configuration { reason },
        RuntimeCreateError::Unavailable(reason) => DriverError::Protocol { reason },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn abi_layout_and_status_values_are_stable() {
        assert_eq!(std::mem::size_of::<CuaDriverAbiVersion>(), 12);
        assert_eq!(CuaDriverStatus::Ok as i32, 0);
        assert_eq!(CuaDriverStatus::Panic as i32, 7);
        assert_eq!(CuaDriverStatus::RuntimeConflict as i32, 8);
        assert!(cua_driver_abi_is_compatible_v1(1, 0));
        assert!(cua_driver_abi_is_compatible_v1(1, 1));
        assert!(!cua_driver_abi_is_compatible_v1(1, 2));
        assert!(!cua_driver_abi_is_compatible_v1(2, 0));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn abi_executor_threads_use_per_monitor_v2() {
        let executor = abi_executor().unwrap();
        let (worker_is_per_monitor_v2, blocking_is_per_monitor_v2) =
            executor.runtime.block_on(async {
                let worker = executor
                    .runtime
                    .spawn(async { platform_windows::dpi::current_thread_uses_per_monitor_v2() })
                    .await
                    .unwrap();
                let blocking = executor
                    .runtime
                    .spawn_blocking(platform_windows::dpi::current_thread_uses_per_monitor_v2)
                    .await
                    .unwrap();
                (worker, blocking)
            });

        assert!(worker_is_per_monitor_v2);
        assert!(blocking_is_per_monitor_v2);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn abi_executor_refuses_a_rejected_windows_dpi_initializer() {
        let executor = OnceLock::new();
        let error = initialized_abi_executor(&executor, || {
            build_windows_abi_executor(Arc::new(|| Err("simulated PMv2 rejection".to_owned())))
        })
        .unwrap_err();

        assert_eq!(error.status, CuaDriverStatus::RuntimeUnavailable);
        assert!(error
            .message
            .contains("initialize Windows physical-pixel context"));
        assert!(error.message.contains("simulated PMv2 rejection"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn abi_late_blocking_thread_failure_prevents_work_and_reaches_completion() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let reject_new_threads = Arc::new(AtomicBool::new(false));
        let reject = reject_new_threads.clone();
        let cell = OnceLock::new();
        let executor = initialized_abi_executor(&cell, || {
            build_windows_abi_executor(Arc::new(move || {
                if reject.load(Ordering::SeqCst) {
                    Err("late blocking worker DPI rejection".into())
                } else {
                    platform_windows::dpi::use_per_monitor_v2_for_current_thread()
                }
            }))
        })
        .unwrap();

        // Hold the prewarmed blocking worker so the operation must create a
        // NEW worker after successful runtime initialization. No timing sleeps.
        let (occupied, ready) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel();
        let held = executor.runtime.spawn_blocking(move || {
            occupied.send(()).unwrap();
            released.recv_timeout(Duration::from_secs(10)).unwrap();
        });
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        reject_new_threads.store(true, Ordering::SeqCst);

        let body_ran = Arc::new(AtomicBool::new(false));
        let body = body_ran.clone();
        let (sender, receiver) = oneshot::channel();
        let context = Box::into_raw(Box::new(CallbackContext { sender })).cast::<c_void>();
        let mut operation = spawn_completion(
            executor.handle().clone(),
            async move {
                // Deliberately swallow the native task error like an optional
                // screenshot fallback. ABI completion must still refuse success.
                let _ = platform_windows::dpi::spawn_blocking(move || {
                    body.store(true, Ordering::SeqCst);
                })
                .await;
                Ok("must not report physical-pixel success".into())
            },
            rust_completion,
            context,
        )
        .unwrap();
        let completed = executor
            .runtime
            .block_on(async { tokio::time::timeout(Duration::from_secs(5), receiver).await });
        // Core recording calls native capture hooks directly. Refuse before
        // obtaining geometry or PNG bytes even without the tool-task wrapper.
        let capture_errors = executor
            .runtime
            .block_on(executor.runtime.spawn_blocking(|| {
                (
                    platform_windows::capture::screenshot_display_bytes()
                        .unwrap_err()
                        .to_string(),
                    platform_windows::capture::screenshot_window_bytes_with_occlusion(0)
                        .unwrap_err()
                        .to_string(),
                )
            }));
        let _ = release.send(());
        executor.runtime.block_on(held).unwrap();
        unsafe { cua_driver_operation_release_v1(&mut operation) };

        let completed = completed.unwrap().unwrap();
        assert_eq!(completed.status, CuaDriverStatus::RuntimeUnavailable);
        assert!(completed.result.is_empty());
        assert!(completed
            .error
            .contains("late blocking worker DPI rejection"));
        assert!(!body_ran.load(Ordering::SeqCst));
        let (display_error, window_error) = capture_errors.unwrap();
        assert!(display_error.contains("late blocking worker DPI rejection"));
        assert!(window_error.contains("late blocking worker DPI rejection"));
        // A later ABI submission cannot revive the poisoned executor.
        let error = initialized_abi_executor(&cell, || unreachable!())
            .unwrap()
            .check_ready()
            .unwrap_err();
        assert_eq!(error.status, CuaDriverStatus::RuntimeUnavailable);
        assert!(!initialized_abi_executor(&cell, || unreachable!())
            .unwrap()
            .is_ready());
        assert!(!embedded_runtime_is_available(
            true,
            initialized_abi_executor(&cell, || unreachable!()),
        ));
        let session_error =
            admit_trusted_session(initialized_abi_executor(&cell, || unreachable!())).unwrap_err();
        assert_eq!(session_error.status, CuaDriverStatus::RuntimeUnavailable);
        assert!(session_error
            .message
            .contains("late blocking worker DPI rejection"));
        // Teardown must still run after failure, without admitting GUI work.
        let (sender, receiver) = oneshot::channel();
        let context = Box::into_raw(Box::new(CallbackContext { sender })).cast::<c_void>();
        let mut cleanup = spawn_cleanup_completion(
            executor.handle().clone(),
            async { Ok("cleanup completed".into()) },
            rust_completion,
            context,
        )
        .unwrap();
        let completed = executor
            .runtime
            .block_on(async { tokio::time::timeout(Duration::from_secs(5), receiver).await });
        unsafe { cua_driver_operation_release_v1(&mut cleanup) };
        let completed = completed.unwrap().unwrap();
        assert_eq!(completed.status, CuaDriverStatus::Ok);
        assert_eq!(completed.result, "cleanup completed");
        // Failure is executor-local; it does not poison the global healthy one.
        abi_executor().unwrap();
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn abi_child_thread_failure_stops_work_and_async_resumption() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let reject_new_threads = Arc::new(AtomicBool::new(false));
        let reject = reject_new_threads.clone();
        let executor = build_windows_abi_executor(Arc::new(move || {
            if reject.load(Ordering::SeqCst) {
                Err("late owned thread DPI rejection".into())
            } else {
                platform_windows::dpi::use_per_monitor_v2_for_current_thread()
            }
        }))
        .unwrap();
        let child_ran = Arc::new(AtomicBool::new(false));
        let child = child_ran.clone();
        let resumed = Arc::new(AtomicBool::new(false));
        let resumed_body = resumed.clone();
        let (sender, receiver) = oneshot::channel();
        let context = Box::into_raw(Box::new(CallbackContext { sender })).cast::<c_void>();
        let mut operation = spawn_completion(
            executor.handle().clone(),
            async move {
                reject_new_threads.store(true, Ordering::SeqCst);
                let result = std::thread::spawn(platform_windows::dpi::owned_thread(move || {
                    child.store(true, Ordering::SeqCst);
                }))
                .join()
                .unwrap();
                assert!(result
                    .unwrap_err()
                    .contains("late owned thread DPI rejection"));
                // Force another poll after the owner failed, then prove the
                // remainder of the async operation is never entered.
                tokio::task::yield_now().await;
                resumed_body.store(true, Ordering::SeqCst);
                Ok("must not resume".into())
            },
            rust_completion,
            context,
        )
        .unwrap();
        let completed = executor
            .runtime
            .block_on(async { tokio::time::timeout(Duration::from_secs(5), receiver).await });
        unsafe { cua_driver_operation_release_v1(&mut operation) };
        let completed = completed.unwrap().unwrap();
        assert_eq!(completed.status, CuaDriverStatus::RuntimeUnavailable);
        assert!(completed.result.is_empty());
        assert!(!child_ran.load(Ordering::SeqCst));
        assert!(!resumed.load(Ordering::SeqCst));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn abi_late_blocking_worker_and_child_thread_use_physical_pixels() {
        let executor = build_abi_executor().unwrap();
        let (occupied, ready) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel();
        let held = executor.runtime.spawn_blocking(move || {
            occupied.send(()).unwrap();
            released.recv_timeout(Duration::from_secs(10)).unwrap();
        });
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        let result = executor.runtime.block_on(executor.runtime.spawn(async {
            platform_windows::dpi::spawn_blocking(|| {
                let blocking = platform_windows::dpi::current_thread_uses_per_monitor_v2();
                let child = std::thread::spawn(platform_windows::dpi::owned_thread(
                    platform_windows::dpi::current_thread_uses_per_monitor_v2,
                ))
                .join()
                .unwrap()
                .unwrap();
                (blocking, child)
            })
            .await
        }));
        let _ = release.send(());
        executor.runtime.block_on(held).unwrap();
        assert_eq!(result.unwrap().unwrap(), (true, true));
    }

    #[test]
    fn generated_header_exposes_the_exported_v1_contract() {
        let header = include_str!("../../../include/cua_driver_abi.h");
        assert!(header.contains("Do not edit"));
        for declaration in [
            "#define CUA_DRIVER_ABI_MAJOR 1",
            "#define CUA_DRIVER_ABI_MINOR 1",
            "#define CUA_DRIVER_ABI_PATCH 0",
            "CUA_DRIVER_STATUS_OK = 0",
            "CUA_DRIVER_STATUS_INVALID_ARGUMENT = 1",
            "CUA_DRIVER_STATUS_NULL_POINTER = 2",
            "CUA_DRIVER_STATUS_RUNTIME_UNAVAILABLE = 3",
            "CUA_DRIVER_STATUS_SHUTDOWN = 4",
            "CUA_DRIVER_STATUS_CANCELLED = 5",
            "CUA_DRIVER_STATUS_INTERNAL = 6",
            "CUA_DRIVER_STATUS_PANIC = 7",
            "CUA_DRIVER_STATUS_RUNTIME_CONFLICT = 8",
            "cua_driver_abi_version_v1",
            "cua_driver_abi_is_compatible_v1",
            "cua_driver_buffer_free_v1",
            "cua_driver_create_v1",
            "cua_driver_destroy_v1",
            "cua_driver_is_available_v1",
            "cua_driver_metadata_json_v1",
            "cua_driver_list_tools_json_v1",
            "cua_driver_invoke_v1",
            "cua_driver_session_create_v1",
            "cua_driver_session_destroy_v1",
            "cua_driver_session_invoke_v1",
            "cua_driver_shutdown_v1",
            "cua_driver_operation_cancel_v1",
            "cua_driver_operation_release_v1",
        ] {
            assert!(header.contains(declaration), "header omitted {declaration}");
        }
    }

    #[test]
    fn panic_is_contained_as_status() {
        let mut error = CuaDriverBuffer::empty();
        let status = with_ffi_guard(&mut error, || -> Result<(), AbiFailure> {
            panic!("contained test panic")
        });
        assert_eq!(status, CuaDriverStatus::Panic);
        let message = unsafe { copy_and_free_buffer(&mut error) };
        assert!(message.contains("no panic crossed"));
    }

    #[test]
    fn owned_buffers_and_handles_are_idempotently_released() {
        let _runtime_test = crate::runtime::TEST_RUNTIME_LOCK.lock().unwrap();
        let mut buffer = CuaDriverBuffer::from_string("owned".into());
        unsafe {
            cua_driver_buffer_free_v1(&mut buffer);
            cua_driver_buffer_free_v1(&mut buffer);
        }
        assert!(buffer.data.is_null());

        let mut handle = ptr::null_mut();
        let mut error = CuaDriverBuffer::empty();
        let status = unsafe { cua_driver_create_v1(ptr::null(), 0, &mut handle, &mut error) };
        assert_eq!(status, CuaDriverStatus::Ok);
        unsafe {
            cua_driver_destroy_v1(&mut handle);
            cua_driver_destroy_v1(&mut handle);
        }
        assert!(handle.is_null());
    }

    #[test]
    fn abi_can_own_two_runtime_handles_concurrently() {
        let _runtime_test = crate::runtime::TEST_RUNTIME_LOCK.lock().unwrap();
        let mut first = ptr::null_mut();
        let mut second = ptr::null_mut();
        let mut first_error = CuaDriverBuffer::empty();
        let mut second_error = CuaDriverBuffer::empty();
        assert_eq!(
            unsafe { cua_driver_create_v1(ptr::null(), 0, &mut first, &mut first_error) },
            CuaDriverStatus::Ok
        );
        assert_eq!(
            unsafe { cua_driver_create_v1(ptr::null(), 0, &mut second, &mut second_error) },
            CuaDriverStatus::Ok
        );
        assert!(!first.is_null());
        assert!(!second.is_null());
        assert_ne!(first, second);
        unsafe {
            cua_driver_destroy_v1(&mut first);
            cua_driver_destroy_v1(&mut second);
        }
        assert!(first.is_null());
        assert!(second.is_null());
    }

    #[tokio::test]
    async fn cancellation_completes_once_and_release_is_idempotent() {
        let (sender, receiver) = oneshot::channel();
        let context = Box::into_raw(Box::new(CallbackContext { sender })).cast::<c_void>();
        let mut operation = spawn_completion(
            abi_executor().unwrap().handle().clone(),
            async {
                tokio::time::sleep(Duration::from_secs(30)).await;
                Ok("never".into())
            },
            rust_completion,
            context,
        )
        .unwrap();
        unsafe {
            cua_driver_operation_cancel_v1(operation);
        }
        let completed = tokio::time::timeout(Duration::from_secs(1), receiver)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(completed.status, CuaDriverStatus::Cancelled);
        unsafe {
            cua_driver_operation_release_v1(&mut operation);
            cua_driver_operation_release_v1(&mut operation);
        }
        assert!(operation.is_null());
    }
}

//! Bounded external-process containment for the optional perception worker.
//!
//! Containment is reported as platform capabilities rather than one misleading
//! cross-platform boolean. Each implementation must install every boundary it
//! advertises, or fail before worker code executes. [`spawn`] refuses to launch
//! a worker on any target whose reported capabilities do not meet
//! [`ContainmentCapabilities::meets_contract`], so a partially contained
//! platform is an explicit `unsupported_platform` result rather than a weaker
//! silent mode.
//!
//! Platform coverage:
//!
//! - Linux: process group, `PR_SET_PDEATHSIG`, `PR_SET_NO_NEW_PRIVS`,
//!   `close_range`, `RLIMIT_AS`/`RLIMIT_CPU`/`RLIMIT_NOFILE`, a Landlock
//!   ruleset that confines reads, executes and writes to an exact allowlist,
//!   and a seccomp filter that denies every socket, `io_uring`, process
//!   creation, `ptrace` and cross-process memory access.
//! - macOS: process group, a trusted `/bin/sh` parent-death sentinel, inherited
//!   descriptor closure, `RLIMIT_CPU`/`RLIMIT_NOFILE`, a supervisor-sampled
//!   memory ceiling, and a deny-by-default Seatbelt profile applied through
//!   `sandbox-exec` that confines reads, writes, execution, Mach lookups and
//!   IPC and denies network, fork and cross-process authority.
//! - Windows: an AppContainer (LowBox) identity with no capabilities, an
//!   explicit `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` carrying only the protocol
//!   pipes, private security descriptors on the job, process, thread, pipes,
//!   desktop and working directory, a private desktop, and a Job Object with
//!   kill-on-close, memory, CPU-time, active-process and UI restrictions. The
//!   worker is created suspended and resumed only after every boundary is in
//!   place. Filesystem ACLs are read and updated through verified no-follow
//!   handles so reparse-point swaps cannot redirect a grant.
//!
//! Network denial, read confinement and write confinement are deliberately not
//! configurable. The only tunable surface is [`ContainmentLimits`], including
//! extra readable and writable paths a caller must opt into explicitly.
//!
//! [`run_contained_hook`] reuses the same boundary for a one-shot auxiliary
//! process — an installed extension's health or self-test hook — so such a hook
//! never runs with more authority than an ordinary parse.

use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use cua_driver_contract::{VisualParseError, VisualParseErrorCode};
use tokio::io::AsyncReadExt as _;

use super::error;

#[cfg(unix)]
mod unix;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as platform;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as platform;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as platform;

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod unsupported;
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
use unsupported as platform;

/// The parent end of the worker's stdin pipe. Unix uses the child's inherited
/// pipe directly; Windows uses a private named pipe so the parent side can be
/// asynchronous while the child receives an ordinary synchronous stdio handle.
pub(crate) type WorkerStdin = platform::WorkerStdin;
/// The parent end of the worker's stdout pipe.
pub(crate) type WorkerStdout = platform::WorkerStdout;

/// A 16 GiB memory ceiling. Linux applies this to virtual address space, where
/// inference stacks commonly reserve far more than they commit; Windows applies
/// it to total committed memory in the Job Object; macOS has no kernel
/// primitive for it and the supervisor samples the worker instead.
const DEFAULT_MAX_MEMORY_BYTES: u64 = 16 * 1024 * 1024 * 1024;
/// 15 minutes of CPU time. A warm worker accumulates CPU across every reused
/// parse, so this is a session ceiling rather than a per-request one; the
/// per-request bound is [`super::WarmWorkerPolicy::inference_timeout`].
const DEFAULT_MAX_CPU_TIME: Duration = Duration::from_secs(15 * 60);
/// The worker runs alone. Unix denies process creation outright through seccomp
/// and Seatbelt; Windows caps the Job Object's active-process count instead,
/// because a Job Object has no equivalent creation hook.
const DEFAULT_MAX_PROCESSES: u32 = 1;
/// Prevents descriptor exhaustion on Unix. Windows Job Objects have no
/// equivalent handle-count ceiling, which capability reporting makes explicit.
const DEFAULT_MAX_OPEN_FILES: u32 = 256;

/// How a memory ceiling is actually enforced on the current target.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryLimitEnforcement {
    /// The kernel refuses allocations past the ceiling.
    Kernel,
    /// The supervisor samples the worker's footprint and kills the process tree
    /// once the ceiling is exceeded. An allocation burst that both starts and
    /// completes between two samples is not prevented.
    SupervisorSampling,
    /// No ceiling is installed. The worker is not memory bounded.
    None,
}

/// Truthful boundaries installed by the current target implementation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContainmentCapabilities {
    pub process_tree_cleanup: bool,
    pub parent_death: bool,
    pub parent_eof: bool,
    pub inherited_handle_closure: bool,
    pub memory_limit: MemoryLimitEnforcement,
    pub cpu_limit: bool,
    /// The worker cannot create another process at all. This replaces the
    /// earlier per-UID task ceiling, which bounded neither the worker tree nor
    /// anything else useful.
    pub process_creation_denial: bool,
    pub file_descriptor_limit: bool,
    pub filesystem_read_isolation: bool,
    pub filesystem_write_isolation: bool,
    pub network_isolation: bool,
    /// The worker cannot read or write another process's memory, attach a
    /// debugger, or steal another process's descriptors.
    pub process_memory_isolation: bool,
    /// The worker cannot capture the desktop, synthesize input, install input
    /// hooks, or read the clipboard.
    pub desktop_input_isolation: bool,
}

impl ContainmentCapabilities {
    /// Every boundary the perception worker contract requires before untrusted
    /// worker code may run.
    ///
    /// `file_descriptor_limit` is deliberately absent: Windows Job Objects
    /// expose no per-job handle ceiling, and descriptor exhaustion is bounded
    /// there by the job's memory limit instead.
    pub const fn meets_contract(&self) -> bool {
        self.process_tree_cleanup
            && self.parent_death
            && self.parent_eof
            && self.inherited_handle_closure
            && self.cpu_limit
            && self.process_creation_denial
            && self.filesystem_read_isolation
            && self.filesystem_write_isolation
            && self.network_isolation
            && self.process_memory_isolation
            && self.desktop_input_isolation
            && !matches!(self.memory_limit, MemoryLimitEnforcement::None)
    }
}

/// Capabilities are compile-target facts. Host facilities are still checked at
/// launch and cause a stable fail-closed error when unavailable.
pub const fn capabilities() -> ContainmentCapabilities {
    #[cfg(target_os = "linux")]
    {
        ContainmentCapabilities {
            process_tree_cleanup: true,
            parent_death: true,
            parent_eof: true,
            inherited_handle_closure: true,
            memory_limit: MemoryLimitEnforcement::Kernel,
            cpu_limit: true,
            process_creation_denial: true,
            file_descriptor_limit: true,
            filesystem_read_isolation: true,
            filesystem_write_isolation: true,
            network_isolation: true,
            process_memory_isolation: true,
            desktop_input_isolation: true,
        }
    }
    #[cfg(target_os = "macos")]
    {
        ContainmentCapabilities {
            process_tree_cleanup: true,
            parent_death: true,
            parent_eof: true,
            inherited_handle_closure: true,
            memory_limit: MemoryLimitEnforcement::SupervisorSampling,
            cpu_limit: true,
            process_creation_denial: true,
            file_descriptor_limit: true,
            filesystem_read_isolation: true,
            filesystem_write_isolation: true,
            network_isolation: true,
            process_memory_isolation: true,
            desktop_input_isolation: true,
        }
    }
    #[cfg(windows)]
    {
        ContainmentCapabilities {
            process_tree_cleanup: true,
            parent_death: true,
            parent_eof: true,
            inherited_handle_closure: true,
            memory_limit: MemoryLimitEnforcement::Kernel,
            cpu_limit: true,
            process_creation_denial: true,
            file_descriptor_limit: false,
            filesystem_read_isolation: true,
            filesystem_write_isolation: true,
            network_isolation: true,
            process_memory_isolation: true,
            desktop_input_isolation: true,
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        ContainmentCapabilities {
            process_tree_cleanup: false,
            parent_death: false,
            parent_eof: false,
            inherited_handle_closure: false,
            memory_limit: MemoryLimitEnforcement::None,
            cpu_limit: false,
            process_creation_denial: false,
            file_descriptor_limit: false,
            filesystem_read_isolation: false,
            filesystem_write_isolation: false,
            network_isolation: false,
            process_memory_isolation: false,
            desktop_input_isolation: false,
        }
    }
}

/// Explicit, testable resource ceilings applied to the worker process tree.
#[derive(Clone, Debug)]
pub struct ContainmentLimits {
    /// Memory ceiling in bytes. Linux applies it to each process's virtual
    /// address space, Windows to the Job Object's total committed memory, and
    /// macOS through supervisor sampling. [`capabilities`] reports which.
    pub max_memory_bytes: u64,
    /// Maximum CPU time each worker process may accumulate. The soft limit is
    /// this value and the hard limit a short grace period above it, so a worker
    /// that exhausts its CPU budget dies with a distinguishable signal.
    pub max_cpu_time: Duration,
    /// Maximum number of concurrently live processes in the Windows Job Object.
    /// Unix denies process creation outright and ignores this count.
    pub max_processes: u32,
    /// Maximum number of open file descriptors on Unix. Windows reports that it
    /// cannot enforce an equivalent per-job handle count.
    pub max_open_files: u32,
    /// Directories the worker may write to in addition to its private working
    /// directory. Every entry widens the sandbox, so callers opt in per path.
    pub additional_writable_paths: Vec<PathBuf>,
    /// Directories the worker may read in addition to its own bundle, runtime
    /// and model paths and the target's immutable system runtime directories.
    /// Every entry widens the sandbox, so callers opt in per path.
    pub additional_readable_paths: Vec<PathBuf>,
    /// Windows-only file whose exclusive lock serializes temporary worker ACLs
    /// with extension lifecycle hardening. Other platforms ignore this path.
    pub windows_acl_lease_path: Option<PathBuf>,
    /// Windows AppContainer recovery journal. Installed workers derive this
    /// from the lease; extension hooks provide it while an outer owner holds
    /// that same lease.
    pub windows_acl_profile_journal_path: Option<PathBuf>,
    /// Require Windows teardown to finish before the containing call returns.
    /// Extension hooks use this while their caller already owns the lifecycle
    /// ACL lease, avoiding a nested lock while forbidding deferred revocation.
    pub windows_require_synchronous_cleanup: bool,
    /// Exact executable files needed only by scripted test fixtures. Shipped
    /// native workers execute only their configured worker binary.
    #[cfg(test)]
    pub additional_executable_paths: Vec<PathBuf>,
}

impl Default for ContainmentLimits {
    fn default() -> Self {
        Self {
            max_memory_bytes: DEFAULT_MAX_MEMORY_BYTES,
            max_cpu_time: DEFAULT_MAX_CPU_TIME,
            max_processes: DEFAULT_MAX_PROCESSES,
            max_open_files: DEFAULT_MAX_OPEN_FILES,
            additional_writable_paths: Vec::new(),
            additional_readable_paths: Vec::new(),
            windows_acl_lease_path: None,
            windows_acl_profile_journal_path: None,
            windows_require_synchronous_cleanup: false,
            #[cfg(test)]
            additional_executable_paths: Vec::new(),
        }
    }
}

#[cfg(windows)]
pub fn windows_acl_profile_journal_path(lease_path: &Path) -> PathBuf {
    platform::acl_profile_journal_path(lease_path)
}

#[cfg(windows)]
pub fn recover_windows_acl_profile_journal(journal_path: &Path) -> Result<(), VisualParseError> {
    platform::recover_acl_profile_journal(journal_path)
}

impl ContainmentLimits {
    pub(crate) fn validate(&self) -> Result<(), VisualParseError> {
        if self.max_memory_bytes == 0
            || self.max_cpu_time.as_secs() == 0
            || self.max_processes == 0
            || self.max_open_files == 0
        {
            return Err(error(
                VisualParseErrorCode::ResourceLimitExceeded,
                "perception worker containment limits must be non-zero",
                false,
                None,
            ));
        }
        Ok(())
    }

    fn max_cpu_seconds(&self) -> u64 {
        self.max_cpu_time.as_secs().max(1)
    }
}

/// The exact, canonical, symlink-free filesystem boundary handed to a platform
/// sandbox. Directory roots are canonical and executable entries are exact
/// canonical files.
struct FilesystemBoundary {
    /// Directories the worker may read from.
    readable: Vec<PathBuf>,
    /// Directories the worker may write to.
    writable: Vec<PathBuf>,
    /// Exact files the worker may execute or map as executable code.
    #[cfg_attr(windows, allow(dead_code))]
    executables: Vec<PathBuf>,
}

/// Derive the worker's filesystem boundary from its launch configuration.
///
/// Read roots come from the worker executable, every absolute path argument
/// (the model manifest and the inference runtime library are passed this way),
/// the private working directory and the caller's explicit opt-ins. Nothing
/// else is readable, so the caller's home directory, credential stores, browser
/// profiles, desktop IPC sockets and the temporary root all stay outside the
/// sandbox.
fn filesystem_boundary(
    executable: &Path,
    args: &[String],
    working_directory: &Path,
    limits: &ContainmentLimits,
) -> Result<FilesystemBoundary, VisualParseError> {
    let working_directory = canonical_directory(working_directory)?;
    let mut readable = vec![working_directory.clone()];
    let mut writable = vec![working_directory];
    #[cfg(not(test))]
    let executables = vec![canonical_file(executable)?];
    #[cfg(test)]
    let mut executables = vec![canonical_file(executable)?];

    readable.push(derived_read_root(executable)?);
    for argument in args {
        let path = Path::new(argument);
        if !path.is_absolute() {
            continue;
        }
        // A configured path that does not exist cannot be a launch or inference
        // input, so it never widens the sandbox.
        if path.symlink_metadata().is_err() {
            continue;
        }
        readable.push(derived_read_root(path)?);
    }
    for path in &limits.additional_readable_paths {
        readable.push(canonical_directory(path)?);
    }
    #[cfg(test)]
    for path in &limits.additional_executable_paths {
        executables.push(canonical_file(path)?);
    }
    for path in &limits.additional_writable_paths {
        let path = canonical_directory(path)?;
        readable.push(path.clone());
        writable.push(path);
    }

    Ok(FilesystemBoundary {
        readable: collapse_roots(readable),
        writable: collapse_roots(writable),
        executables,
    })
}

fn canonical_file(path: &Path) -> Result<PathBuf, VisualParseError> {
    let path = std::fs::canonicalize(path).map_err(|cause| {
        containment_error(
            "failed to resolve a perception worker executable",
            os_detail(&cause),
        )
    })?;
    if !path.is_file() {
        return Err(containment_error(
            "a perception worker executable is not a file",
            None,
        ));
    }
    Ok(path)
}

/// Resolve `path` to the exact directory the worker needs, rejecting a root
/// that would widen the sandbox beyond the worker's own installation.
fn derived_read_root(path: &Path) -> Result<PathBuf, VisualParseError> {
    let resolved = std::fs::canonicalize(path).map_err(|cause| {
        containment_error(
            "failed to resolve a perception worker path for the read sandbox",
            os_detail(&cause),
        )
    })?;
    let root = if resolved.is_dir() {
        resolved
    } else {
        resolved
            .parent()
            .ok_or_else(|| {
                containment_error(
                    "a perception worker path has no directory for the read sandbox",
                    None,
                )
            })?
            .to_path_buf()
    };
    reject_broad_read_root(&root)?;
    Ok(root)
}

/// A derived read root must be specific to the worker installation. A shared
/// system directory, a home directory or a filesystem root would hand the
/// worker unrelated user data, so those are refused rather than silently
/// widening the sandbox.
fn reject_broad_read_root(path: &Path) -> Result<(), VisualParseError> {
    let named = path
        .components()
        .filter(|component| matches!(component, Component::Normal(_)))
        .count();
    let parent_is_home_root = path.parent().is_some_and(|parent| {
        matches!(
            parent.to_str(),
            Some(
                "/Users"
                    | "/home"
                    | "/private/Users"
                    | "/var/folders"
                    | "/private/var/folders"
                    | "/root"
            )
        ) || parent
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case("Users"))
    });
    let is_home = home_directory().is_some_and(|home| home == path);
    let denied = BROAD_READ_ROOTS
        .iter()
        .any(|entry| path.as_os_str().eq_ignore_ascii_case(*entry));
    if named < 2 || parent_is_home_root || is_home || denied {
        return Err(containment_error(
            "a perception worker path resolves to a directory too broad to grant the worker",
            None,
        ));
    }
    Ok(())
}

/// Shared directories that must never become a worker read root even though
/// they are deep enough to pass the component check.
const BROAD_READ_ROOTS: &[&str] = &[
    "/usr/bin",
    "/usr/sbin",
    "/usr/lib",
    "/usr/lib64",
    "/usr/libexec",
    "/usr/local",
    "/usr/share",
    "/var/tmp",
    "/var/folders",
    "/private/tmp",
    "/private/var",
    "/System/Library",
    "/Library/Keychains",
    "/Library/Application Support",
    "/etc/ssl",
];

fn home_directory() -> Option<PathBuf> {
    let name = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let value = std::env::var_os(name)?;
    std::fs::canonicalize(value).ok()
}

fn canonical_directory(path: &Path) -> Result<PathBuf, VisualParseError> {
    let resolved = std::fs::canonicalize(path).map_err(|cause| {
        containment_error(
            "failed to resolve a perception worker sandbox directory",
            os_detail(&cause),
        )
    })?;
    if !resolved.is_dir() {
        return Err(containment_error(
            "a perception worker sandbox path is not a directory",
            None,
        ));
    }
    Ok(resolved)
}

/// Drop duplicates and any root already covered by an ancestor, so a platform
/// installs the smallest equivalent rule set.
fn collapse_roots(mut roots: Vec<PathBuf>) -> Vec<PathBuf> {
    roots.sort();
    roots.dedup();
    let mut collapsed: Vec<PathBuf> = Vec::with_capacity(roots.len());
    for root in roots {
        if collapsed.iter().any(|kept| root.starts_with(kept)) {
            continue;
        }
        collapsed.push(root);
    }
    collapsed
}

/// The raw operating-system result of a worker exit, before the guard folds in
/// supervisor state such as a sampled memory kill.
struct RawExit {
    success: bool,
    /// The operating system itself ended the process for exhausting a
    /// containment ceiling.
    resource_limited: bool,
    description: String,
}

/// How the worker process finished.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum WorkerExit {
    Success,
    /// The worker died or exited unsuccessfully for a reason other than a
    /// containment ceiling.
    Failure(String),
    /// A containment ceiling — CPU time or memory — ended the worker. The
    /// caller reports this as `resource_limit_exceeded` rather than a crash so
    /// an agent does not retry a request that cannot succeed.
    ResourceLimit(String),
}

/// A launched worker process plus the platform handles that bound it.
///
/// Field order is load-bearing: `guard` is declared first so it drops first,
/// tearing down the whole process tree before the process handle is reaped and
/// before the protocol pipes close.
pub(crate) struct ContainedChild {
    guard: platform::Guard,
    process: platform::Process,
    stdin: Option<WorkerStdin>,
    stdout: Option<WorkerStdout>,
}

impl ContainedChild {
    pub(crate) fn take_stdin(&mut self) -> Option<WorkerStdin> {
        self.stdin.take()
    }

    pub(crate) fn take_stdout(&mut self) -> Option<WorkerStdout> {
        self.stdout.take()
    }

    /// Wait for the worker to exit and classify how it ended.
    pub(crate) async fn wait(&mut self) -> std::io::Result<WorkerExit> {
        let raw = self.process.wait().await?;
        // The process table entry is gone, so a later process-group signal
        // could reach a recycled group id. Platforms that cannot prove the
        // group is still occupied stop signalling it here.
        #[cfg(windows)]
        self.guard.note_reaped()?;
        #[cfg(not(windows))]
        self.guard.note_reaped();
        Ok(if raw.success {
            WorkerExit::Success
        } else if raw.resource_limited || self.guard.memory_ceiling_exceeded() {
            WorkerExit::ResourceLimit(raw.description)
        } else {
            WorkerExit::Failure(raw.description)
        })
    }
}

/// Launch the worker under full platform containment.
///
/// Returns [`VisualParseErrorCode::UnsupportedPlatform`] when the target or host
/// lacks a required containment facility and
/// [`VisualParseErrorCode::WorkerLaunchFailed`] when an available facility
/// cannot be configured or the contained process cannot be launched. Neither
/// case executes worker code.
pub(crate) async fn spawn(
    executable: &Path,
    args: &[String],
    working_directory: &Path,
    limits: &ContainmentLimits,
) -> Result<ContainedChild, VisualParseError> {
    limits.validate()?;
    if !capabilities().meets_contract() {
        return Err(unsupported_error(
            "this platform cannot install every perception worker boundary, so no worker is launched",
        ));
    }
    let boundary = filesystem_boundary(executable, args, working_directory, limits)?;
    // Launch through the same canonical names the platform policy contains.
    // This is load-bearing on macOS, where `/var` is a symlink to
    // `/private/var` and Seatbelt evaluates the path used by `exec`.
    let executable = canonical_file(executable)?;
    let working_directory = canonical_directory(working_directory)?;
    let args = args
        .iter()
        .map(|argument| {
            let path = Path::new(argument);
            if path.is_absolute() && path.symlink_metadata().is_ok() {
                std::fs::canonicalize(path)
                    .map(|path| path.display().to_string())
                    .map_err(|cause| {
                        containment_error(
                            "failed to resolve a perception worker argument for launch",
                            os_detail(&cause),
                        )
                    })
            } else {
                Ok(argument.clone())
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    platform::spawn(&executable, &args, &working_directory, &boundary, limits).await
}

/// How a one-shot contained process finished.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HookOutcome {
    Succeeded,
    /// The hook ran and ended unsuccessfully for a reason other than a
    /// containment ceiling.
    Failed(String),
    /// A containment ceiling — CPU time or memory — ended the hook.
    ResourceLimited(String),
    /// The hook outlived its deadline, so its process tree was killed and
    /// reaped before this returned.
    TimedOut,
}

/// How long the killed process tree is given to be reaped after a deadline, so
/// a one-shot caller leaves no zombie behind.
const TERMINATION_GRACE: Duration = Duration::from_secs(5);

/// Run one auxiliary process — an installed extension's health or self-test
/// hook — under exactly the boundary [`spawn`] installs for a perception parse.
///
/// The hook is not a protocol peer, so its request pipe is closed immediately
/// and it observes end of file just as a null stdin would; its standard output
/// is read and discarded so a chatty hook can neither block on a full pipe nor
/// reach the Driver's own output; its standard error is discarded by the
/// platform command shape. The private working directory is created here and
/// removed when this returns, and it is the only path the hook may write.
///
/// Containment that cannot be installed is an error rather than a weaker
/// launch, so no hook instruction runs before every boundary is in place.
pub async fn run_contained_hook(
    executable: &Path,
    args: &[String],
    limits: &ContainmentLimits,
    timeout: Duration,
) -> Result<HookOutcome, VisualParseError> {
    if timeout.is_zero() {
        return Err(error(
            VisualParseErrorCode::ResourceLimitExceeded,
            "a contained hook deadline must be non-zero",
            false,
            None,
        ));
    }
    let working_directory = tempfile::Builder::new()
        .prefix("cua-extension-hook-")
        .tempdir()
        .map_err(|cause| {
            containment_error(
                "failed to create a private contained hook directory",
                os_detail(&cause),
            )
        })?;
    let mut contained = spawn(executable, args, working_directory.path(), limits).await?;
    // Closing the request pipe is what makes the hook observe end of file.
    drop(contained.take_stdin());
    let mut stdout = contained.take_stdout();
    let exit = tokio::time::timeout(timeout, async {
        let (_, exit) = tokio::join!(discard_output(stdout.as_mut()), contained.wait());
        exit
    })
    .await;
    match exit {
        Ok(Ok(WorkerExit::Success)) => Ok(HookOutcome::Succeeded),
        Ok(Ok(WorkerExit::Failure(detail))) => Ok(HookOutcome::Failed(detail)),
        Ok(Ok(WorkerExit::ResourceLimit(detail))) => Ok(HookOutcome::ResourceLimited(detail)),
        Ok(Err(cause)) => Err(error(
            VisualParseErrorCode::WorkerCrashed,
            "failed to supervise the contained hook",
            false,
            os_detail(&cause),
        )),
        Err(_) => {
            terminate(contained).await;
            Ok(HookOutcome::TimedOut)
        }
    }
}

/// Drain the hook's output into nothing. A hook that fills its pipe would
/// otherwise block until its deadline instead of exiting.
async fn discard_output(stdout: Option<&mut WorkerStdout>) {
    let Some(stdout) = stdout else {
        return;
    };
    let mut sink = [0_u8; 8 * 1024];
    while matches!(stdout.read(&mut sink).await, Ok(count) if count > 0) {}
}

/// Tear a contained process tree down through its platform guard and reap it.
///
/// Destructuring rather than dropping keeps the documented order explicit: the
/// guard kills the tree first, and only then is the process handle awaited.
async fn terminate(contained: ContainedChild) {
    let ContainedChild {
        guard,
        mut process,
        stdin,
        stdout,
    } = contained;
    // Matches the field order an ordinary drop would follow: the guard tears
    // the tree down first, the process handle is reaped next, and the protocol
    // pipes are released last.
    drop(guard);
    let _ = tokio::time::timeout(TERMINATION_GRACE, process.wait()).await;
    drop(stdin);
    drop(stdout);
}

/// A launch failure that is deterministic for this host: retrying cannot make
/// the containment facility appear.
fn containment_error(message: &'static str, detail: Option<String>) -> VisualParseError {
    error(
        VisualParseErrorCode::WorkerLaunchFailed,
        message,
        false,
        detail,
    )
}

/// A platform or host that cannot install the full containment contract.
fn unsupported_error(message: &'static str) -> VisualParseError {
    error(
        VisualParseErrorCode::UnsupportedPlatform,
        message,
        false,
        None,
    )
}

/// Error detail is limited to an operating-system status. Paths, arguments and
/// environment values never reach the caller through a containment failure.
#[allow(dead_code)]
fn os_detail(cause: &std::io::Error) -> Option<String> {
    Some(match cause.raw_os_error() {
        Some(code) => format!("os error {code}"),
        None => "os error unavailable".to_owned(),
    })
}

#[allow(dead_code)]
fn spawn_error(cause: std::io::Error) -> VisualParseError {
    error(
        VisualParseErrorCode::WorkerLaunchFailed,
        "failed to launch the contained perception worker",
        false,
        os_detail(&cause),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_supported_target_meets_the_containment_contract() {
        // A target that cannot install one of these boundaries must reach the
        // fail-closed branch in `spawn` instead of launching a worker.
        let reported = capabilities();
        #[cfg(any(target_os = "linux", target_os = "macos", windows))]
        assert!(
            reported.meets_contract(),
            "a supported target reported an incomplete boundary set: {reported:?}"
        );
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        assert!(!reported.meets_contract());
    }

    #[test]
    fn macos_reports_its_memory_ceiling_as_sampled_rather_than_kernel_enforced() {
        // Darwin has no public per-process address-space or footprint limit for
        // a spawned child, so the ceiling must not be advertised as a kernel
        // guarantee.
        #[cfg(target_os = "macos")]
        assert_eq!(
            capabilities().memory_limit,
            MemoryLimitEnforcement::SupervisorSampling
        );
        #[cfg(any(target_os = "linux", windows))]
        assert_eq!(capabilities().memory_limit, MemoryLimitEnforcement::Kernel);
    }

    #[test]
    fn broad_directories_are_refused_as_worker_read_roots() {
        for candidate in [
            "/",
            "/usr",
            "/tmp",
            "/etc",
            "/usr/lib",
            "/usr/local",
            "/Users/someone",
            "/home/someone",
            "/private/var",
            "/System/Library",
        ] {
            assert!(
                reject_broad_read_root(Path::new(candidate)).is_err(),
                "{candidate} was accepted as a worker read root"
            );
        }
        for candidate in [
            "/opt/cua/extensions/cua-perception/bin",
            "/Users/someone/.cua/extensions/cua-perception/0.1.0",
            "/tmp/.tmpA1b2c3",
        ] {
            assert!(
                reject_broad_read_root(Path::new(candidate)).is_ok(),
                "{candidate} was refused as a worker read root"
            );
        }
    }

    #[test]
    fn collapsed_roots_drop_paths_an_ancestor_already_covers() {
        let collapsed = collapse_roots(vec![
            PathBuf::from("/opt/cua/models"),
            PathBuf::from("/opt/cua"),
            PathBuf::from("/opt/cua"),
            PathBuf::from("/opt/cuativity"),
        ]);
        assert_eq!(
            collapsed,
            vec![PathBuf::from("/opt/cua"), PathBuf::from("/opt/cuativity")]
        );
    }

    #[test]
    fn the_worker_bundle_and_model_paths_become_the_only_derived_read_roots() {
        let bundle = tempfile::Builder::new()
            .prefix("cua-perception-bundle-")
            .tempdir()
            .unwrap();
        let working = tempfile::Builder::new()
            .prefix("cua-perception-work-")
            .tempdir()
            .unwrap();
        let executable = bundle.path().join("bin");
        std::fs::create_dir(&executable).unwrap();
        let executable = executable.join("worker");
        std::fs::write(&executable, b"#!/bin/sh\n").unwrap();
        let models = bundle.path().join("models");
        std::fs::create_dir(&models).unwrap();
        let manifest = models.join("manifest.json");
        std::fs::write(&manifest, b"{}").unwrap();

        let boundary = filesystem_boundary(
            &executable,
            &[
                "--manifest".to_owned(),
                manifest.display().to_string(),
                "--threads".to_owned(),
                "2".to_owned(),
            ],
            working.path(),
            &ContainmentLimits::default(),
        )
        .unwrap();

        let canonical_models = std::fs::canonicalize(&models).unwrap();
        let canonical_bin = std::fs::canonicalize(executable.parent().unwrap()).unwrap();
        let canonical_working = std::fs::canonicalize(working.path()).unwrap();
        assert!(boundary.readable.contains(&canonical_models));
        assert!(boundary.readable.contains(&canonical_bin));
        assert!(boundary.readable.contains(&canonical_working));
        assert_eq!(boundary.writable, vec![canonical_working]);
        // The caller's home directory and the temporary root are never granted.
        assert!(!boundary
            .readable
            .iter()
            .any(|root| root == Path::new("/tmp")
                || root == Path::new("/private/tmp")
                || home_directory().is_some_and(|home| *root == home)));
    }
}

//! Bounded external-process containment for the optional perception worker.
//!
//! Containment is reported as platform capabilities rather than one misleading
//! cross-platform boolean. Each implementation must install every boundary it
//! advertises, or fail before worker code executes.
//!
//! Platform coverage:
//!
//! - Windows: a private Job Object with kill-on-close, an active-process limit,
//!   a job memory limit and a per-process CPU-time limit. The child is created
//!   suspended so no descendant can be spawned before job assignment. Job
//!   Objects do not provide filesystem or network isolation, and this module
//!   does not claim otherwise.
//! - Linux: process group, `PR_SET_PDEATHSIG`, `PR_SET_NO_NEW_PRIVS`,
//!   `RLIMIT_AS`/`RLIMIT_CPU`/`RLIMIT_NPROC`, a Landlock ruleset restricting
//!   writes, and a seccomp filter denying non-local socket families.
//! - macOS: process group, CPU/process/file-descriptor limits and a Seatbelt
//!   profile applied through `sandbox-exec` that denies network and restricts
//!   writes. Darwin does not provide a usable hard address-space limit for this
//!   worker; that limitation is reported explicitly.
//!
//! Network denial and write restriction are deliberately not configurable. The
//! only tunable surface is [`ContainmentLimits`], including a list of extra
//! writable paths a caller must opt into explicitly.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::process::{Child, Command};

use cua_driver_contract::{VisualParseError, VisualParseErrorCode};

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

/// A 16 GiB memory ceiling. Linux applies this to virtual address space, where
/// inference stacks commonly reserve far more than they commit; Windows
/// applies it to total committed memory in the Job Object. macOS reports that
/// it cannot enforce this ceiling and skips it.
const DEFAULT_MAX_MEMORY_BYTES: u64 = 16 * 1024 * 1024 * 1024;
/// 15 minutes of CPU time. A warm worker accumulates CPU across every reused
/// parse, so this is a session ceiling rather than a per-request one; the
/// per-request bound is [`super::WarmWorkerPolicy::inference_timeout`].
const DEFAULT_MAX_CPU_TIME: Duration = Duration::from_secs(15 * 60);
/// Bounds the worker process tree well below a typical host ceiling. Linux also
/// denies creation of additional processes while allowing runtime threads.
const DEFAULT_MAX_PROCESSES: u32 = 512;
/// Prevents descriptor exhaustion on Unix. Windows Job Objects have no
/// equivalent handle-count ceiling, which capability reporting makes explicit.
const DEFAULT_MAX_OPEN_FILES: u32 = 256;

/// Truthful boundaries installed by the current target implementation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContainmentCapabilities {
    pub process_tree_cleanup: bool,
    pub parent_death: bool,
    pub parent_eof: bool,
    pub inherited_handle_closure: bool,
    pub memory_limit: bool,
    pub cpu_limit: bool,
    pub process_limit: bool,
    pub file_descriptor_limit: bool,
    pub filesystem_write_isolation: bool,
    pub network_isolation: bool,
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
            memory_limit: true,
            cpu_limit: true,
            process_limit: true,
            file_descriptor_limit: true,
            filesystem_write_isolation: true,
            network_isolation: true,
        }
    }
    #[cfg(target_os = "macos")]
    {
        ContainmentCapabilities {
            process_tree_cleanup: true,
            parent_death: false,
            parent_eof: true,
            inherited_handle_closure: false,
            memory_limit: false,
            cpu_limit: true,
            process_limit: true,
            file_descriptor_limit: true,
            filesystem_write_isolation: true,
            network_isolation: true,
        }
    }
    #[cfg(windows)]
    {
        ContainmentCapabilities {
            process_tree_cleanup: true,
            parent_death: true,
            parent_eof: true,
            inherited_handle_closure: false,
            memory_limit: true,
            cpu_limit: true,
            process_limit: true,
            file_descriptor_limit: false,
            filesystem_write_isolation: false,
            network_isolation: false,
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        ContainmentCapabilities {
            process_tree_cleanup: false,
            parent_death: false,
            parent_eof: false,
            inherited_handle_closure: false,
            memory_limit: false,
            cpu_limit: false,
            process_limit: false,
            file_descriptor_limit: false,
            filesystem_write_isolation: false,
            network_isolation: false,
        }
    }
}

/// Explicit, testable resource ceilings applied to the worker process tree.
#[derive(Clone, Debug)]
pub struct ContainmentLimits {
    /// Memory ceiling in bytes. Linux applies it to each process's virtual
    /// address space and Windows to the Job Object's total committed memory.
    /// macOS reports `memory_limit: false` and does not claim this boundary.
    pub max_memory_bytes: u64,
    /// Maximum CPU time each worker process may accumulate.
    pub max_cpu_time: Duration,
    /// Maximum number of concurrently live processes in the worker tree.
    pub max_processes: u32,
    /// Maximum number of open file descriptors on Unix. Windows reports that
    /// it cannot enforce an equivalent per-job handle count.
    pub max_open_files: u32,
    /// Directories the worker may write to in addition to its private working
    /// directory. Every entry widens the sandbox, so callers opt in per path.
    pub additional_writable_paths: Vec<PathBuf>,
}

impl Default for ContainmentLimits {
    fn default() -> Self {
        Self {
            max_memory_bytes: DEFAULT_MAX_MEMORY_BYTES,
            max_cpu_time: DEFAULT_MAX_CPU_TIME,
            max_processes: DEFAULT_MAX_PROCESSES,
            max_open_files: DEFAULT_MAX_OPEN_FILES,
            additional_writable_paths: Vec::new(),
        }
    }
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

/// A launched worker process plus the platform handle that bounds it.
///
/// Field order is load-bearing: `guard` is declared first so it drops first,
/// tearing down the whole process tree before the process handle is reaped.
pub(crate) struct ContainedChild {
    #[allow(dead_code)]
    guard: platform::Guard,
    pub(crate) child: Child,
}

/// Launch the worker under full platform containment.
///
/// Returns [`VisualParseErrorCode::UnsupportedPlatform`] when the target or host
/// lacks a required containment facility and
/// [`VisualParseErrorCode::WorkerLaunchFailed`] when an available facility
/// cannot be configured or the contained process cannot be launched. Neither
/// case executes worker code.
pub(crate) fn spawn(
    executable: &Path,
    args: &[String],
    working_directory: &Path,
    limits: &ContainmentLimits,
) -> Result<ContainedChild, VisualParseError> {
    limits.validate()?;
    let (child, guard) = platform::spawn(executable, args, working_directory, limits)?;
    Ok(ContainedChild { guard, child })
}

/// Stdio and environment shape shared by every platform: no inherited
/// environment, piped stdin so the worker observes parent EOF, piped stdout for
/// the framed protocol, discarded stderr, and a kill on handle drop.
fn base_command(program: &Path) -> Command {
    let mut command = Command::new(program);
    command
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    command
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

/// A platform that has no containment implementation at all.
#[allow(dead_code)]
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
    fn capabilities_do_not_overstate_target_boundaries() {
        let reported = capabilities();
        #[cfg(target_os = "linux")]
        assert_eq!(
            reported,
            ContainmentCapabilities {
                process_tree_cleanup: true,
                parent_death: true,
                parent_eof: true,
                inherited_handle_closure: true,
                memory_limit: true,
                cpu_limit: true,
                process_limit: true,
                file_descriptor_limit: true,
                filesystem_write_isolation: true,
                network_isolation: true,
            }
        );
        #[cfg(target_os = "macos")]
        assert_eq!(
            reported,
            ContainmentCapabilities {
                process_tree_cleanup: true,
                parent_death: false,
                parent_eof: true,
                inherited_handle_closure: false,
                memory_limit: false,
                cpu_limit: true,
                process_limit: true,
                file_descriptor_limit: true,
                filesystem_write_isolation: true,
                network_isolation: true,
            }
        );
        #[cfg(windows)]
        assert_eq!(
            reported,
            ContainmentCapabilities {
                process_tree_cleanup: true,
                parent_death: true,
                parent_eof: true,
                inherited_handle_closure: false,
                memory_limit: true,
                cpu_limit: true,
                process_limit: true,
                file_descriptor_limit: false,
                filesystem_write_isolation: false,
                network_isolation: false,
            }
        );
    }
}

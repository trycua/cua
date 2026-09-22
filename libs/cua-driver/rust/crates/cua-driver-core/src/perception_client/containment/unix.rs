//! Containment pieces shared by the Unix platforms.

use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::process::Command;

use super::RawExit;

/// `getrlimit`/`setrlimit` take `__rlimit_resource_t` on glibc and `c_int`
/// everywhere else this crate builds.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
type RlimitResource = libc::__rlimit_resource_t;
#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
type RlimitResource = libc::c_int;

/// The worker keeps this much CPU time past its soft ceiling before the kernel
/// escalates from `SIGXCPU` to `SIGKILL`. The gap is what lets the supervisor
/// report a CPU exhaustion as `resource_limit_exceeded` instead of a crash.
const CPU_LIMIT_GRACE_SECONDS: u64 = 5;

/// Stdio and environment shape shared by every Unix platform: no inherited
/// environment, piped stdin so the worker observes parent EOF, piped stdout for
/// the framed protocol, discarded stderr, and a kill on handle drop.
///
/// `TMPDIR` points at the private working directory because writes are confined
/// to it; a runtime that spills an allocation arena into the system temporary
/// directory would otherwise fail.
pub(super) fn base_command(program: &Path, working_directory: &Path) -> Command {
    let mut command = Command::new(program);
    command
        .env_clear()
        .env("TMPDIR", working_directory)
        .env("TMP", working_directory)
        .env("TEMP", working_directory)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    command
}

/// Apply the CPU-time and file-descriptor ceilings, plus an address-space
/// ceiling on the targets whose kernel enforces one.
///
/// Called between `fork` and `exec`, so it uses only raw syscalls and performs
/// no allocation. `RLIMIT_NPROC` is deliberately absent: Linux and Darwin both
/// count it per real UID rather than per process tree, so it bounded nothing
/// useful while breaking the worker on any desktop whose user already owns more
/// tasks than the ceiling. Process creation is denied outright instead — by the
/// seccomp filter on Linux and by the deny-by-default Seatbelt profile on
/// macOS.
pub(super) fn apply_resource_limits(
    max_address_space_bytes: Option<u64>,
    max_cpu_seconds: u64,
    max_open_files: u32,
) -> std::io::Result<()> {
    if let Some(bytes) = max_address_space_bytes {
        #[cfg(target_os = "linux")]
        set_limit(libc::RLIMIT_AS, bytes, bytes)?;
        #[cfg(not(target_os = "linux"))]
        let _ = bytes;
    }
    set_limit(
        libc::RLIMIT_CPU,
        max_cpu_seconds,
        max_cpu_seconds.saturating_add(CPU_LIMIT_GRACE_SECONDS),
    )?;
    set_limit(
        libc::RLIMIT_NOFILE,
        u64::from(max_open_files),
        u64::from(max_open_files),
    )
}

fn set_limit(resource: RlimitResource, soft: u64, hard: u64) -> std::io::Result<()> {
    let mut current = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    if unsafe { libc::getrlimit(resource, &mut current) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // An unprivileged process cannot raise its inherited hard limit, and the
    // stricter of the two is the safe choice, so clamp instead of failing.
    let clamp = |value: u64| -> libc::rlim_t {
        let requested = value as libc::rlim_t;
        if current.rlim_max == libc::RLIM_INFINITY {
            requested
        } else {
            current.rlim_max.min(requested)
        }
    };
    let limit = libc::rlimit {
        rlim_cur: clamp(soft),
        rlim_max: clamp(hard),
    };
    if unsafe { libc::setrlimit(resource, &limit) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Mark every descriptor above the protocol stdio pipes close-on-exec, so no
/// file, socket or device the Driver had open survives into the worker.
///
/// `retain` stays open: macOS uses it for the parent-death sentinel pipe. Runs
/// between `fork` and `exec`, so it allocates nothing.
///
/// Linux uses `close_range(2)` instead, which is a single syscall and needs no
/// descriptor-table sweep.
#[cfg(target_os = "macos")]
pub(super) fn close_inherited_descriptors(retain: Option<libc::c_int>) -> std::io::Result<()> {
    // Read the ceiling before the descriptor limit is lowered, and cap the
    // sweep so an inherited `RLIM_INFINITY` cannot turn this into an unbounded
    // loop.
    const MAXIMUM_SWEEP: libc::c_int = 65_536;
    let ceiling = unsafe { libc::getdtablesize() }.clamp(3, MAXIMUM_SWEEP);
    for descriptor in 3..ceiling {
        if Some(descriptor) == retain {
            continue;
        }
        unsafe {
            libc::close(descriptor);
        }
    }
    if let Some(retain) = retain {
        // The sentinel descriptor must survive `exec`, so its inherited
        // close-on-exec flag is cleared explicitly.
        if unsafe { libc::fcntl(retain, libc::F_SETFD, 0) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Kills the worker's process group so descendants cannot outlive cancellation,
/// timeout, runtime shutdown, client drop or idle cleanup.
pub(super) struct ProcessGroupGuard {
    pid: Option<u32>,
    reaped: bool,
    /// Whether the group must still be signalled after the leader was reaped.
    ///
    /// Once the leader is reaped its process-group id can be recycled, so
    /// signalling it blindly could reach an unrelated group. Linux sets this to
    /// `false` because seccomp denies `fork`, `setsid` and `setpgid`, which
    /// proves the group is empty. macOS sets it to `true` because its
    /// parent-death sentinel is still a live member, which keeps the group id
    /// reserved and the signal correctly addressed.
    kill_group_after_reap: bool,
    memory_ceiling_exceeded: Arc<AtomicBool>,
}

impl ProcessGroupGuard {
    pub(super) fn new(pid: Option<u32>, kill_group_after_reap: bool) -> Self {
        Self {
            pid,
            reaped: false,
            kill_group_after_reap,
            memory_ceiling_exceeded: Arc::new(AtomicBool::new(false)),
        }
    }

    /// A flag a supervisor task sets when it kills the worker for exceeding its
    /// memory ceiling, so the exit is reported as a resource limit.
    pub(super) fn memory_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.memory_ceiling_exceeded)
    }

    pub(super) fn note_reaped(&mut self) {
        self.reaped = true;
    }

    pub(super) fn memory_ceiling_exceeded(&self) -> bool {
        self.memory_ceiling_exceeded.load(Ordering::Acquire)
    }
}

impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        if self.reaped && !self.kill_group_after_reap {
            return;
        }
        if let Some(pid) = self.pid {
            if let Ok(pid) = i32::try_from(pid) {
                // The worker is its own process-group leader, so the negative
                // pid reaches every descendant that did not change group.
                unsafe {
                    libc::kill(-pid, libc::SIGKILL);
                }
            }
        }
    }
}

/// The Unix worker process. The child owns the protocol pipes directly, so the
/// supervisor only has to reap it and classify how it ended.
pub(super) struct Process {
    child: tokio::process::Child,
}

impl Process {
    pub(super) fn new(child: tokio::process::Child) -> Self {
        Self { child }
    }

    pub(super) async fn wait(&mut self) -> std::io::Result<RawExit> {
        use std::os::unix::process::ExitStatusExt as _;

        let status = self.child.wait().await?;
        Ok(RawExit {
            success: status.success(),
            // `SIGXCPU` is only ever raised by the soft CPU ceiling installed
            // above; the hard ceiling that follows it is a plain `SIGKILL`.
            resource_limited: status.signal() == Some(libc::SIGXCPU),
            description: status.to_string(),
        })
    }
}

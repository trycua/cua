//! Containment pieces shared by the Unix platforms.

/// `getrlimit`/`setrlimit` take `__rlimit_resource_t` on glibc and `c_int`
/// everywhere else this crate builds.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
type RlimitResource = libc::__rlimit_resource_t;
#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
type RlimitResource = libc::c_int;

/// Apply CPU-time, process-count and file-descriptor ceilings. Linux also gets
/// an address-space ceiling; macOS skips it because Darwin cannot enforce a
/// usable hard memory limit for this worker.
///
/// Called between `fork` and `exec`, so it uses only raw syscalls and performs
/// no allocation.
pub(super) fn apply_resource_limits(
    max_address_space_bytes: u64,
    max_cpu_seconds: u64,
    max_processes: u32,
    max_open_files: u32,
) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    set_limit(libc::RLIMIT_AS, max_address_space_bytes)?;
    #[cfg(target_os = "macos")]
    let _ = max_address_space_bytes;
    set_limit(libc::RLIMIT_CPU, max_cpu_seconds)?;
    set_limit(libc::RLIMIT_NPROC, u64::from(max_processes))?;
    set_limit(libc::RLIMIT_NOFILE, u64::from(max_open_files))
}

fn set_limit(resource: RlimitResource, value: u64) -> std::io::Result<()> {
    let mut current = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    if unsafe { libc::getrlimit(resource, &mut current) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let requested = value as libc::rlim_t;
    // An unprivileged process cannot raise its inherited hard limit, and the
    // stricter of the two is the safe choice, so clamp instead of failing.
    let ceiling = if current.rlim_max == libc::RLIM_INFINITY {
        requested
    } else {
        current.rlim_max.min(requested)
    };
    let limit = libc::rlimit {
        rlim_cur: ceiling,
        rlim_max: ceiling,
    };
    if unsafe { libc::setrlimit(resource, &limit) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Kills the worker's process group so descendants cannot outlive cancellation,
/// timeout, runtime shutdown, client drop or idle cleanup.
pub(super) struct ProcessGroupGuard {
    pid: Option<u32>,
}

impl ProcessGroupGuard {
    pub(super) fn new(pid: Option<u32>) -> Self {
        Self { pid }
    }
}

impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
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

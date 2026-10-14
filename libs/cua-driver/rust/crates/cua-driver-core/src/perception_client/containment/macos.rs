//! macOS worker containment: a deny-by-default Seatbelt profile, a trusted
//! parent-death sentinel, inherited descriptor closure, resource limits and a
//! supervisor-sampled memory ceiling.
//!
//! The profile is applied by launching the worker through
//! `/usr/bin/sandbox-exec`, which is the only supported way for a parent to
//! impose a Seatbelt profile on a child it does not control. `sandbox_init`
//! cannot be used from a `pre_exec` hook: it allocates and talks to `sandboxd`,
//! neither of which is async-signal-safe after `fork`. A profile that fails to
//! compile makes `sandbox-exec` exit before it execs anything, so a malformed
//! profile can never yield an unsandboxed worker, and a missing `sandbox-exec`
//! fails the launch outright.
//!
//! Darwin has no `PR_SET_PDEATHSIG`, and a `kqueue` watch cannot be installed
//! across `exec`. The worker is therefore launched underneath a fixed
//! `/bin/sh` script that holds one extra descriptor: the read end of a pipe
//! whose write end never leaves the Driver. The script backgrounds a read on
//! that descriptor, closes its own copy, and then `exec`s `sandbox-exec`, so
//! the shell's pid — which is also the process-group id — becomes the worker.
//! When the Driver dies for any reason, including `SIGKILL`, the pipe reaches
//! end of file and the sentinel kills the whole process group.
//!
//! Darwin also has no usable hard address-space limit: `RLIMIT_AS` is an alias
//! for `RLIMIT_RSS`, which XNU does not enforce. Rather than advertise an
//! unenforced ceiling, the supervisor samples the worker's resident footprint
//! and kills the tree when it exceeds the bound. [`super::capabilities`]
//! reports that as [`super::MemoryLimitEnforcement::SupervisorSampling`].

use std::io::Write as _;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::OpenOptionsExt as _;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cua_driver_contract::VisualParseError;

use super::unix::{
    apply_resource_limits, base_command, close_inherited_descriptors, ProcessGroupGuard,
};
use super::{
    containment_error, os_detail, spawn_error, unsupported_error, ContainedChild,
    ContainmentLimits, FilesystemBoundary,
};

pub(super) type Process = super::unix::Process;
pub(super) type WorkerStdin = tokio::process::ChildStdin;
pub(super) type WorkerStdout = tokio::process::ChildStdout;

const SHELL: &str = "/bin/sh";
const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";
const PROFILE_FILE: &str = ".cua-perception-sandbox.sb";

/// The descriptor the sentinel reads. Chosen because `pre_exec` runs after the
/// standard library has already placed the protocol pipes on 0, 1 and 2.
const SENTINEL_DESCRIPTOR: RawFd = 3;

/// Backgrounds a blocking read on the sentinel descriptor, drops its own copy
/// of that descriptor so the worker never sees it, and replaces itself with
/// `sandbox-exec`. `$$` stays the pid of the original shell across both the
/// subshell and the `exec`, and that pid is the process-group id, so the kill
/// reaches the worker and anything it left behind.
const PARENT_DEATH_SENTINEL: &str = concat!(
    "{ read -r cua_parent_gone <&3; kill -9 -$$; } 0</dev/null 1>/dev/null 2>&1 &\n",
    "exec 3<&-\n",
    "exec \"$@\"\n",
);

/// How often the supervisor samples the worker's footprint.
const MEMORY_SAMPLE_INTERVAL: Duration = Duration::from_millis(250);

/// Immutable system directories the dynamic loader and Objective-C runtime
/// read. None of them expose user data.
const SYSTEM_READ_TREES: &[&str] = &[
    "/usr/lib",
    "/System/Library",
    "/System/Volumes/Preboot/Cryptexes/OS",
    "/private/var/db/dyld",
];

/// The only devices the worker may open. `/dev` as a whole is never granted.
const DEVICE_NODES: &[&str] = &[
    "/dev/null",
    "/dev/zero",
    "/dev/random",
    "/dev/urandom",
    "/dev/dtracehelper",
];

/// The Mach services a non-GUI process needs to start and log. Everything else
/// — `com.apple.windowserver.active`, `com.apple.tccd`, `com.apple.pasteboard.1`
/// and `com.apple.SecurityServer` among them — stays denied by the profile's
/// default, which is what keeps the worker away from the Driver's own screen
/// recording, accessibility and Keychain grants.
const MACH_SERVICES: &[&str] = &[
    "com.apple.system.opendirectoryd.libinfo",
    "com.apple.system.DirectoryService.libinfo_v1",
    "com.apple.system.notification_center",
    "com.apple.system.logger",
    "com.apple.logd",
    "com.apple.diagnosticd",
];

/// Owns the worker's process group, the sentinel pipe and the memory sampler.
///
/// Field order is load-bearing. [`Drop::drop`] runs first and stops the
/// sampler; then `group` drops and kills the tree, which reaches the sentinel
/// before it can observe end of file; only then does `sentinel` close. A
/// sentinel that fired after the group was reaped could otherwise signal a
/// recycled process-group id.
pub(super) struct Guard {
    group: ProcessGroupGuard,
    /// Held only for its `Drop`: closing this end is what makes the sentinel
    /// observe end of file when the Driver goes away.
    _sentinel: OwnedFd,
    memory_watch: tokio::task::JoinHandle<()>,
}

impl Guard {
    pub(super) fn note_reaped(&mut self) {
        self.group.note_reaped();
    }

    pub(super) fn memory_ceiling_exceeded(&self) -> bool {
        self.group.memory_ceiling_exceeded()
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.memory_watch.abort();
    }
}

pub(super) async fn spawn(
    executable: &Path,
    args: &[String],
    working_directory: &Path,
    boundary: &FilesystemBoundary,
    limits: &ContainmentLimits,
) -> Result<ContainedChild, VisualParseError> {
    for required in [SHELL, SANDBOX_EXEC] {
        if !Path::new(required).is_file() {
            return Err(unsupported_error(
                "this macOS build cannot contain the perception worker: sandbox-exec or /bin/sh is unavailable",
            ));
        }
    }
    // `/bin/sh` is what is actually spawned, so a missing worker would
    // otherwise surface as a crashed worker instead of a failed launch.
    if !executable.is_file() {
        return Err(spawn_error(std::io::Error::from_raw_os_error(libc::ENOENT)));
    }
    let profile_path = write_profile(working_directory, boundary)?;
    let (sentinel_read, sentinel_write) = sentinel_pipe()?;

    let mut command = base_command(Path::new(SHELL), working_directory);
    command
        .arg("-c")
        .arg(PARENT_DEATH_SENTINEL)
        .arg("cua-perception-supervisor")
        .arg(SANDBOX_EXEC)
        .arg("-f")
        .arg(&profile_path)
        .arg(executable)
        .args(args);
    command.current_dir(working_directory);
    command.process_group(0);

    let sentinel_source = sentinel_read.as_raw_fd();
    if sentinel_source < SENTINEL_DESCRIPTOR {
        return Err(containment_error(
            "the perception worker sentinel pipe landed on a reserved descriptor",
            None,
        ));
    }
    let max_cpu_seconds = limits.max_cpu_seconds();
    let max_open_files = limits.max_open_files;
    // Runs between fork and exec using only raw syscalls. The limits and the
    // descriptor table survive the exec into `/bin/sh`, its exec of
    // `sandbox-exec`, and that program's exec of the worker itself.
    unsafe {
        command.as_std_mut().pre_exec(move || {
            if sentinel_source != SENTINEL_DESCRIPTOR
                && libc::dup2(sentinel_source, SENTINEL_DESCRIPTOR) < 0
            {
                return Err(std::io::Error::last_os_error());
            }
            close_inherited_descriptors(Some(SENTINEL_DESCRIPTOR))?;
            apply_resource_limits(None, max_cpu_seconds, max_open_files)
        });
    }

    let mut child = command.spawn().map_err(spawn_error)?;
    drop(sentinel_read);
    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let pid = child.id();
    // The sentinel subshell stays a live member of the group, so the group id
    // cannot be recycled and remains safe to signal even after the leader has
    // been reaped.
    let group = ProcessGroupGuard::new(pid, true);
    let Some(sample_pid) = pid.and_then(|pid| i32::try_from(pid).ok()) else {
        return Err(containment_error(
            "failed to identify the perception worker for memory enforcement",
            None,
        ));
    };
    if resident_bytes(sample_pid).is_none() {
        unsafe {
            libc::kill(-sample_pid, libc::SIGKILL);
        }
        return Err(containment_error(
            "failed to install the perception worker memory sampler",
            None,
        ));
    }
    let memory_watch = watch_memory(sample_pid, limits.max_memory_bytes, group.memory_flag());
    Ok(ContainedChild {
        guard: Guard {
            group,
            _sentinel: sentinel_write,
            memory_watch,
        },
        process: Process::new(child),
        stdin,
        stdout,
    })
}

/// Create the sentinel pipe with both ends close-on-exec, so no other Driver
/// child can hold the write end open and keep a dead Driver's worker alive.
fn sentinel_pipe() -> Result<(OwnedFd, OwnedFd), VisualParseError> {
    let mut ends = [0 as libc::c_int; 2];
    if unsafe { libc::pipe(ends.as_mut_ptr()) } != 0 {
        return Err(containment_error(
            "failed to create the perception worker parent-death sentinel pipe",
            os_detail(&std::io::Error::last_os_error()),
        ));
    }
    let read = unsafe { OwnedFd::from_raw_fd(ends[0]) };
    let write = unsafe { OwnedFd::from_raw_fd(ends[1]) };
    for end in [&read, &write] {
        if unsafe { libc::fcntl(end.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } != 0 {
            return Err(containment_error(
                "failed to isolate the perception worker parent-death sentinel pipe",
                os_detail(&std::io::Error::last_os_error()),
            ));
        }
    }
    Ok((read, write))
}

/// Sample the worker's resident footprint and kill the tree once it exceeds the
/// ceiling. Darwin offers no kernel primitive a parent can install on a child
/// for this, so the boundary is reported as sampled rather than enforced.
fn watch_memory(
    pid: libc::c_int,
    ceiling: u64,
    exceeded: Arc<AtomicBool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(MEMORY_SAMPLE_INTERVAL).await;
            let Some(resident) = resident_bytes(pid) else {
                // ESRCH proves the worker is gone. Any other sampling failure
                // leaves the ceiling unobservable, so kill the group rather
                // than silently continuing without enforcement.
                if unsafe { libc::kill(pid, 0) } == 0
                    || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
                {
                    unsafe {
                        libc::kill(-pid, libc::SIGKILL);
                    }
                }
                return;
            };
            if resident > ceiling {
                exceeded.store(true, Ordering::Release);
                unsafe {
                    libc::kill(-pid, libc::SIGKILL);
                }
                return;
            }
        }
    })
}

/// `struct proc_taskinfo` from `<libproc.h>`. The layout is stable ABI.
#[repr(C)]
#[derive(Clone, Copy)]
struct ProcTaskInfo {
    virtual_size: u64,
    resident_size: u64,
    total_user: u64,
    total_system: u64,
    threads_user: u64,
    threads_system: u64,
    policy: i32,
    faults: i32,
    pageins: i32,
    cow_faults: i32,
    messages_sent: i32,
    messages_received: i32,
    syscalls_mach: i32,
    syscalls_unix: i32,
    context_switches: i32,
    thread_count: i32,
    running_threads: i32,
    priority: i32,
}

/// `PROC_PIDTASKINFO` from `<sys/proc_info.h>`.
const PROC_PIDTASKINFO: libc::c_int = 4;

extern "C" {
    fn proc_pidinfo(
        pid: libc::c_int,
        flavor: libc::c_int,
        arg: u64,
        buffer: *mut libc::c_void,
        buffersize: libc::c_int,
    ) -> libc::c_int;
}

fn resident_bytes(pid: libc::c_int) -> Option<u64> {
    let mut info = ProcTaskInfo {
        virtual_size: 0,
        resident_size: 0,
        total_user: 0,
        total_system: 0,
        threads_user: 0,
        threads_system: 0,
        policy: 0,
        faults: 0,
        pageins: 0,
        cow_faults: 0,
        messages_sent: 0,
        messages_received: 0,
        syscalls_mach: 0,
        syscalls_unix: 0,
        context_switches: 0,
        thread_count: 0,
        running_threads: 0,
        priority: 0,
    };
    let size = std::mem::size_of::<ProcTaskInfo>() as libc::c_int;
    let written = unsafe {
        proc_pidinfo(
            pid,
            PROC_PIDTASKINFO,
            0,
            std::ptr::addr_of_mut!(info).cast(),
            size,
        )
    };
    (written == size).then_some(info.resident_size)
}

/// Write the Seatbelt profile into the private working directory and hand
/// `sandbox-exec` a path rather than the policy text, so the profile and the
/// worker's temporary path never appear in `ps` output.
fn write_profile(
    working_directory: &Path,
    boundary: &FilesystemBoundary,
) -> Result<PathBuf, VisualParseError> {
    let path = working_directory.join(PROFILE_FILE);
    let profile = build_profile(boundary)?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(|cause| {
            containment_error(
                "failed to create the macOS perception worker sandbox profile",
                os_detail(&cause),
            )
        })?;
    file.write_all(profile.as_bytes()).map_err(|cause| {
        containment_error(
            "failed to write the macOS perception worker sandbox profile",
            os_detail(&cause),
        )
    })?;
    Ok(path)
}

/// Deny-by-default Seatbelt profile.
///
/// Reads, executes and writes are confined to the exact worker bundle, runtime,
/// model and working-directory paths plus the immutable system runtime
/// directories the loader needs. Network, fork, cross-process inspection,
/// IOKit, and every Mach service outside a short logging allowlist are denied
/// by the default rule.
fn build_profile(boundary: &FilesystemBoundary) -> Result<String, VisualParseError> {
    let mut profile = String::from(
        "(version 1)\n\
         (deny default)\n\
         ; Explicit for auditability; the default rule already denies these.\n\
         (deny network*)\n\
         (deny process-fork)\n\
         (deny mach-register)\n\
         (allow sysctl-read)\n\
         (allow process-info* (target self))\n\
         (allow signal (target self))\n",
    );

    profile.push_str("(allow mach-lookup\n");
    for service in MACH_SERVICES {
        profile.push_str("  (global-name \"");
        profile.push_str(&escape(service));
        profile.push_str("\")\n");
    }
    profile.push_str(")\n");
    profile.push_str(
        "(allow ipc-posix-shm-read-data (ipc-posix-name \"apple.shm.notification_center\"))\n",
    );

    let mut metadata_paths = vec![PathBuf::from("/"), PathBuf::from("/var")];
    let mut add_ancestors = |path: &Path| {
        let mut parent = path.parent();
        while let Some(ancestor) = parent {
            metadata_paths.push(ancestor.to_path_buf());
            parent = ancestor.parent();
        }
    };
    for path in boundary
        .readable
        .iter()
        .chain(&boundary.writable)
        .chain(&boundary.executables)
    {
        add_ancestors(path);
    }
    for path in SYSTEM_READ_TREES.iter().chain(DEVICE_NODES) {
        add_ancestors(Path::new(path));
    }
    metadata_paths.sort();
    metadata_paths.dedup();
    let metadata = metadata_paths
        .iter()
        .map(|path| literal_rule(path))
        .collect::<Result<Vec<_>, _>>()?;
    push_rule(&mut profile, "file-read-metadata", metadata.iter());

    // A literal directory rule permits traversal and directory inspection but
    // cannot expose any file below it. Some runtimes inspect `/` at startup.
    let mut readable = vec!["  (literal \"/\")\n".to_owned()];
    for path in &boundary.readable {
        readable.push(subpath_rule(path)?);
    }
    for path in SYSTEM_READ_TREES {
        if Path::new(path).exists() {
            readable.push(subpath_rule(Path::new(path))?);
        }
    }
    let devices: Vec<String> = DEVICE_NODES
        .iter()
        .filter(|node| Path::new(node).exists())
        .map(|node| format!("  (literal \"{}\")\n", escape(node)))
        .collect();

    push_rule(&mut profile, "file-read*", readable.iter().chain(&devices));
    let executable: Vec<String> = boundary
        .executables
        .iter()
        .map(|path| literal_rule(path))
        .collect::<Result<_, _>>()?;
    let mut mappable = executable.clone();
    for path in SYSTEM_READ_TREES {
        if Path::new(path).exists() {
            mappable.push(subpath_rule(Path::new(path))?);
        }
    }
    push_rule(&mut profile, "file-map-executable", mappable.iter());
    push_rule(&mut profile, "process-exec*", executable.iter());
    push_rule(&mut profile, "file-ioctl", devices.iter());
    push_rule(&mut profile, "file-write-data", devices.iter());

    let mut writable = Vec::new();
    for path in &boundary.writable {
        writable.push(subpath_rule(path)?);
    }
    push_rule(&mut profile, "file-write*", writable.iter());
    Ok(profile)
}

fn push_rule<'a>(profile: &mut String, operation: &str, entries: impl Iterator<Item = &'a String>) {
    let mut opened = false;
    for entry in entries {
        if !opened {
            profile.push_str("(allow ");
            profile.push_str(operation);
            profile.push('\n');
            opened = true;
        }
        profile.push_str(entry);
    }
    if opened {
        profile.push_str(")\n");
    }
}

fn subpath_rule(path: &Path) -> Result<String, VisualParseError> {
    let text = path.to_str().ok_or_else(|| {
        containment_error(
            "a path for the macOS worker sandbox is not valid UTF-8",
            None,
        )
    })?;
    Ok(format!("  (subpath \"{}\")\n", escape(text)))
}

fn literal_rule(path: &Path) -> Result<String, VisualParseError> {
    let text = path.to_str().ok_or_else(|| {
        containment_error(
            "a path for the macOS worker sandbox is not valid UTF-8",
            None,
        )
    })?;
    Ok(format!("  (literal \"{}\")\n", escape(text)))
}

/// Escape a path for a Seatbelt double-quoted string literal so a crafted path
/// cannot terminate the literal and inject profile syntax.
fn escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if matches!(character, '\\' | '"') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile_for(readable: &[&str], writable: &[&str]) -> String {
        build_profile(&FilesystemBoundary {
            readable: readable.iter().map(PathBuf::from).collect(),
            writable: writable.iter().map(PathBuf::from).collect(),
            executables: readable.iter().map(PathBuf::from).collect(),
        })
        .unwrap()
    }

    #[test]
    fn the_profile_denies_capture_input_and_credential_authority() {
        let profile = profile_for(&["/opt/cua/worker"], &["/tmp/.tmpworker"]);
        assert!(profile.starts_with("(version 1)\n(deny default)\n"));
        // The broad grants that previously re-granted the Driver's own TCC
        // authority to the worker must not reappear.
        for forbidden in [
            "(allow mach-lookup)",
            "(allow iokit-open)",
            "(allow file-read*)\n",
            "(allow process-exec*)\n",
            "com.apple.windowserver",
            "com.apple.tccd",
            "com.apple.pasteboard",
            "com.apple.SecurityServer",
        ] {
            assert!(
                !profile.contains(forbidden),
                "the profile still contains {forbidden}:\n{profile}"
            );
        }
        assert!(profile.contains("(subpath \"/opt/cua/worker\")"));
        assert!(profile.contains("(deny process-fork)"));
    }

    #[test]
    fn writable_paths_never_widen_beyond_the_requested_roots() {
        let profile = profile_for(&["/opt/cua/worker"], &["/tmp/.tmpworker"]);
        let writes = profile
            .split("(allow file-write*\n")
            .nth(1)
            .expect("the profile grants writes");
        assert!(writes.starts_with("  (subpath \"/tmp/.tmpworker\")\n)\n"));
    }

    #[test]
    fn a_crafted_path_cannot_inject_profile_syntax() {
        let profile = profile_for(&["/opt/\") (allow default) (subpath \"/"], &["/tmp/w"]);
        assert!(profile.contains("(subpath \"/opt/\\\") (allow default) (subpath \\\"/\")"));
        assert!(!profile.contains("(allow default)\n"));
    }

    #[test]
    fn the_sentinel_script_kills_the_group_and_hides_its_descriptor() {
        assert!(PARENT_DEATH_SENTINEL.contains("read -r cua_parent_gone <&3"));
        assert!(PARENT_DEATH_SENTINEL.contains("kill -9 -$$"));
        assert!(PARENT_DEATH_SENTINEL.contains("exec 3<&-"));
        assert!(PARENT_DEATH_SENTINEL.trim_end().ends_with("exec \"$@\""));
    }

    /// Darwin has no `PR_SET_PDEATHSIG`, so the parent-death claim rests
    /// entirely on this sentinel. Closing the Driver's end of the pipe stands in
    /// for the Driver dying; a worker that ignores termination signals must
    /// still be gone afterwards.
    #[test]
    fn closing_the_sentinel_pipe_kills_a_worker_that_ignores_signals() {
        use std::os::unix::process::CommandExt as _;

        let (read, write) = sentinel_pipe().unwrap();
        let source = read.as_raw_fd();
        let mut command = std::process::Command::new(SHELL);
        command
            .arg("-c")
            .arg(PARENT_DEATH_SENTINEL)
            .arg("cua-perception-supervisor")
            .arg(SHELL)
            .arg("-c")
            .arg("trap '' TERM INT HUP; while :; do sleep 1; done")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .process_group(0);
        unsafe {
            command.pre_exec(move || {
                if source != SENTINEL_DESCRIPTOR && libc::dup2(source, SENTINEL_DESCRIPTOR) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                close_inherited_descriptors(Some(SENTINEL_DESCRIPTOR))
            });
        }
        let mut child = command.spawn().unwrap();
        drop(read);

        // The worker is alive and unreachable by an ordinary termination
        // signal before the Driver's end of the sentinel pipe closes.
        std::thread::sleep(Duration::from_millis(250));
        assert!(child.try_wait().unwrap().is_none());

        drop(write);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(!status.success(), "the sentinel did not kill the worker");
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the worker outlived its Driver"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

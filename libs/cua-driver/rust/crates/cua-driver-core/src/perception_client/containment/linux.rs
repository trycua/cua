//! Linux worker containment: process group, parent-death signal, resource
//! limits, `PR_SET_NO_NEW_PRIVS`, a Landlock ruleset that confines writes, and
//! a seccomp filter that denies non-local socket families.
//!
//! Landlock and seccomp are the two unprivileged kernel facilities that a
//! parent can install on a child without root, a namespace or a helper binary.
//! Landlock alone is not enough for the network half of the contract: network
//! restriction only arrives in Landlock ABI 4 and covers TCP bind/connect only,
//! so the seccomp filter is what actually denies the worker every non-local
//! address family. Both are prepared in the parent so that a kernel without
//! them fails the launch with a stable error instead of degrading silently.

use std::ffi::CString;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};

use cua_driver_contract::VisualParseError;
use tokio::process::Child;

use super::unix::{apply_resource_limits, ProcessGroupGuard};
use super::{
    base_command, containment_error, os_detail, spawn_error, unsupported_error, ContainmentLimits,
};

pub(super) type Guard = ProcessGroupGuard;

const SYS_LANDLOCK_CREATE_RULESET: libc::c_long = 444;
const SYS_LANDLOCK_ADD_RULE: libc::c_long = 445;
const SYS_LANDLOCK_RESTRICT_SELF: libc::c_long = 446;
const SYS_CLOSE_RANGE: libc::c_long = 436;
const CLOSE_RANGE_CLOEXEC: u32 = 1 << 2;

const LANDLOCK_CREATE_RULESET_VERSION: u32 = 1 << 0;
const LANDLOCK_RULE_PATH_BENEATH: u32 = 1;

const LANDLOCK_ACCESS_FS_WRITE_FILE: u64 = 1 << 1;
const LANDLOCK_ACCESS_FS_REMOVE_DIR: u64 = 1 << 4;
const LANDLOCK_ACCESS_FS_REMOVE_FILE: u64 = 1 << 5;
const LANDLOCK_ACCESS_FS_MAKE_CHAR: u64 = 1 << 6;
const LANDLOCK_ACCESS_FS_MAKE_DIR: u64 = 1 << 7;
const LANDLOCK_ACCESS_FS_MAKE_REG: u64 = 1 << 8;
const LANDLOCK_ACCESS_FS_MAKE_SOCK: u64 = 1 << 9;
const LANDLOCK_ACCESS_FS_MAKE_FIFO: u64 = 1 << 10;
const LANDLOCK_ACCESS_FS_MAKE_BLOCK: u64 = 1 << 11;
const LANDLOCK_ACCESS_FS_MAKE_SYM: u64 = 1 << 12;
/// Landlock ABI 2.
const LANDLOCK_ACCESS_FS_REFER: u64 = 1 << 13;
/// Landlock ABI 3.
const LANDLOCK_ACCESS_FS_TRUNCATE: u64 = 1 << 14;

/// Every write-shaped filesystem right available in Landlock ABI 1. Read rights
/// are deliberately left unhandled so the ruleset never restricts reads: the
/// worker must still load its runtime and model artifacts from outside its
/// working directory.
const LANDLOCK_WRITE_ACCESS_V1: u64 = LANDLOCK_ACCESS_FS_WRITE_FILE
    | LANDLOCK_ACCESS_FS_REMOVE_DIR
    | LANDLOCK_ACCESS_FS_REMOVE_FILE
    | LANDLOCK_ACCESS_FS_MAKE_CHAR
    | LANDLOCK_ACCESS_FS_MAKE_DIR
    | LANDLOCK_ACCESS_FS_MAKE_REG
    | LANDLOCK_ACCESS_FS_MAKE_SOCK
    | LANDLOCK_ACCESS_FS_MAKE_FIFO
    | LANDLOCK_ACCESS_FS_MAKE_BLOCK
    | LANDLOCK_ACCESS_FS_MAKE_SYM;

const SECCOMP_SET_MODE_FILTER: libc::c_ulong = 1;
const SECCOMP_GET_ACTION_AVAIL: libc::c_ulong = 2;

const SECCOMP_RET_KILL_PROCESS: u32 = 0x8000_0000;
const SECCOMP_RET_KILL_THREAD: u32 = 0x0000_0000;
const SECCOMP_RET_ERRNO: u32 = 0x0005_0000;
const SECCOMP_RET_ALLOW: u32 = 0x7fff_0000;

const BPF_LD: u16 = 0x00;
const BPF_W: u16 = 0x00;
const BPF_ABS: u16 = 0x20;
const BPF_ALU: u16 = 0x04;
const BPF_AND: u16 = 0x50;
const BPF_JMP: u16 = 0x05;
const BPF_JEQ: u16 = 0x10;
const BPF_K: u16 = 0x00;
const BPF_RET: u16 = 0x06;

/// Offsets into `struct seccomp_data`.
const SECCOMP_DATA_NR: u32 = 0;
const SECCOMP_DATA_ARCH: u32 = 4;
/// Low half of `args[0]` on a little-endian target.
const SECCOMP_DATA_ARG0_LOW: u32 = 16;

#[cfg(target_arch = "x86_64")]
const AUDIT_ARCH: u32 = 0xC000_003E;
#[cfg(target_arch = "aarch64")]
const AUDIT_ARCH: u32 = 0xC000_00B7;

#[repr(C)]
struct LandlockRulesetAttr {
    handled_access_fs: u64,
}

// The kernel declares `landlock_path_beneath_attr` as packed; an unpacked
// layout would add four bytes of tail padding and be rejected.
#[repr(C, packed)]
struct LandlockPathBeneathAttr {
    allowed_access: u64,
    parent_fd: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SockFilter {
    code: u16,
    jt: u8,
    jf: u8,
    k: u32,
}

#[repr(C)]
struct SockFprog {
    len: u16,
    filter: *const SockFilter,
}

pub(super) fn spawn(
    executable: &Path,
    args: &[String],
    working_directory: &Path,
    limits: &ContainmentLimits,
) -> Result<(Child, Guard), VisualParseError> {
    // Both rulesets are built in the parent so an unsupported kernel or
    // architecture is reported explicitly rather than as an opaque
    // `pre_exec` failure after the point of no return.
    let ruleset = build_landlock_ruleset(working_directory, &limits.additional_writable_paths)?;
    let filter = build_seccomp_filter()?;

    let mut command = base_command(executable);
    command.args(args).current_dir(working_directory);
    command.process_group(0);

    let max_memory_bytes = limits.max_memory_bytes;
    let max_cpu_seconds = limits.max_cpu_seconds();
    let max_processes = limits.max_processes;
    let max_open_files = limits.max_open_files;
    // Runs between fork and exec. Every call below is a raw syscall, nothing
    // allocates, and both the ruleset descriptor and the filter program were
    // materialized before the fork.
    unsafe {
        command.as_std_mut().pre_exec(move || {
            let parent = libc::getppid();
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            // The parent may have exited between the fork and the prctl, in
            // which case the death signal was already missed.
            if libc::getppid() != parent {
                libc::raise(libc::SIGKILL);
            }
            apply_resource_limits(
                max_memory_bytes,
                max_cpu_seconds,
                max_processes,
                max_open_files,
            )?;
            if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::syscall(
                SYS_LANDLOCK_RESTRICT_SELF,
                ruleset.as_raw_fd(),
                0 as libc::c_ulong,
            ) != 0
            {
                return Err(std::io::Error::last_os_error());
            }
            // Mark every non-stdio descriptor close-on-exec. This includes
            // descriptors inherited from the host and closes the otherwise
            // serious bypass where an already-open file or socket survives
            // into the worker past Landlock/seccomp installation.
            if libc::syscall(SYS_CLOSE_RANGE, 3_u32, u32::MAX, CLOSE_RANGE_CLOEXEC) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            let program = SockFprog {
                len: filter.len() as u16,
                filter: filter.as_ptr(),
            };
            if libc::syscall(
                libc::SYS_seccomp,
                SECCOMP_SET_MODE_FILTER,
                0 as libc::c_ulong,
                &program as *const SockFprog,
            ) != 0
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let child = command.spawn().map_err(spawn_error)?;
    let guard = ProcessGroupGuard::new(child.id());
    Ok((child, guard))
}

fn landlock_abi() -> Option<u32> {
    let abi = unsafe {
        libc::syscall(
            SYS_LANDLOCK_CREATE_RULESET,
            std::ptr::null::<LandlockRulesetAttr>(),
            0 as libc::size_t,
            LANDLOCK_CREATE_RULESET_VERSION as libc::c_ulong,
        )
    };
    u32::try_from(abi).ok().filter(|abi| *abi >= 1)
}

fn build_landlock_ruleset(
    working_directory: &Path,
    additional_writable_paths: &[PathBuf],
) -> Result<OwnedFd, VisualParseError> {
    let Some(abi) = landlock_abi() else {
        return Err(unsupported_error(
            "this Linux kernel does not provide the Landlock filesystem containment the perception worker requires",
        ));
    };
    if abi < 3 {
        return Err(unsupported_error(
            "this Linux kernel cannot restrict file truncation for the perception worker",
        ));
    }
    let mut handled = LANDLOCK_WRITE_ACCESS_V1;
    handled |= LANDLOCK_ACCESS_FS_REFER | LANDLOCK_ACCESS_FS_TRUNCATE;

    let attribute = LandlockRulesetAttr {
        handled_access_fs: handled,
    };
    let ruleset = unsafe {
        libc::syscall(
            SYS_LANDLOCK_CREATE_RULESET,
            &attribute as *const LandlockRulesetAttr,
            std::mem::size_of::<LandlockRulesetAttr>() as libc::size_t,
            0 as libc::c_ulong,
        )
    };
    let ruleset = RawFd::try_from(ruleset)
        .ok()
        .filter(|fd| *fd >= 0)
        .ok_or_else(|| {
            containment_error(
                "failed to create the Landlock ruleset for the perception worker",
                os_detail(&std::io::Error::last_os_error()),
            )
        })?;
    let ruleset = unsafe { OwnedFd::from_raw_fd(ruleset) };

    add_rule(&ruleset, working_directory, handled, true)?;
    for path in additional_writable_paths {
        add_rule(&ruleset, path, handled, true)?;
    }
    // Native inference runtimes open GPU and accelerator character devices
    // read-write. Opening an existing node is allowed; creating, replacing or
    // removing entries under `/dev` is not.
    let device_access = handled & (LANDLOCK_ACCESS_FS_WRITE_FILE | LANDLOCK_ACCESS_FS_TRUNCATE);
    add_rule(&ruleset, Path::new("/dev"), device_access, false)?;
    Ok(ruleset)
}

fn add_rule(
    ruleset: &OwnedFd,
    path: &Path,
    allowed_access: u64,
    required: bool,
) -> Result<(), VisualParseError> {
    let Some(parent) = open_path(path, required)? else {
        return Ok(());
    };
    let attribute = LandlockPathBeneathAttr {
        allowed_access,
        parent_fd: parent.as_raw_fd(),
    };
    let added = unsafe {
        libc::syscall(
            SYS_LANDLOCK_ADD_RULE,
            ruleset.as_raw_fd(),
            LANDLOCK_RULE_PATH_BENEATH as libc::c_ulong,
            &attribute as *const LandlockPathBeneathAttr,
            0 as libc::c_ulong,
        )
    };
    if added != 0 {
        return Err(containment_error(
            "failed to grant a writable path in the Landlock ruleset for the perception worker",
            os_detail(&std::io::Error::last_os_error()),
        ));
    }
    Ok(())
}

fn open_path(path: &Path, required: bool) -> Result<Option<OwnedFd>, VisualParseError> {
    let raw = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        containment_error(
            "a writable path for the Linux worker sandbox contains an interior NUL",
            None,
        )
    })?;
    let fd = unsafe { libc::open(raw.as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
    if fd < 0 {
        if !required {
            return Ok(None);
        }
        return Err(containment_error(
            "failed to open a writable path for the Linux worker sandbox",
            os_detail(&std::io::Error::last_os_error()),
        ));
    }
    Ok(Some(unsafe { OwnedFd::from_raw_fd(fd) }))
}

/// Deny every socket family except `AF_UNIX`, which the worker's own runtime may
/// use for local IPC. `socket` is the only way to obtain a fresh network
/// endpoint, and the worker inherits no descriptors beyond its stdio pipes, so
/// denying creation denies the network.
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
fn build_seccomp_filter() -> Result<Vec<SockFilter>, VisualParseError> {
    Err(unsupported_error(
        "the perception worker has no seccomp network denial for this Linux architecture",
    ))
}

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
fn build_seccomp_filter() -> Result<Vec<SockFilter>, VisualParseError> {
    if !seccomp_action_available(SECCOMP_RET_ERRNO) {
        return Err(unsupported_error(
            "this Linux kernel does not provide the seccomp network denial the perception worker requires",
        ));
    }
    // A foreign audit architecture means the syscall numbers below do not
    // describe the calling ABI, so the safe response is to stop the process
    // rather than let an unfiltered syscall through. `args[0]` is read as its
    // low half, which is the domain argument on these little-endian targets.
    let mismatch = if seccomp_action_available(SECCOMP_RET_KILL_PROCESS) {
        SECCOMP_RET_KILL_PROCESS
    } else {
        SECCOMP_RET_KILL_THREAD
    };
    let denied = SECCOMP_RET_ERRNO | (libc::EPERM as u32 & 0xffff);
    let unavailable = SECCOMP_RET_ERRNO | (libc::ENOSYS as u32 & 0xffff);
    let mut filter = vec![
        load(SECCOMP_DATA_ARCH),
        jump_equal(AUDIT_ARCH, 1, 0),
        ret(mismatch),
        load(SECCOMP_DATA_NR),
        // The worker itself must remain the process-group leader so the guard
        // can always reap it. Process creation is denied below; native runtime
        // threads remain available through CLONE_THREAD.
        jump_equal(libc::SYS_setsid as u32, 0, 1),
        ret(denied),
        jump_equal(libc::SYS_setpgid as u32, 0, 1),
        ret(denied),
        // Do not let the worker clear the parent-death signal installed just
        // before this filter. Other prctl operations (for example thread names)
        // remain available.
        jump_equal(libc::SYS_prctl as u32, 0, 4),
        load(SECCOMP_DATA_ARG0_LOW),
        jump_equal(libc::PR_SET_PDEATHSIG as u32, 0, 1),
        ret(denied),
        load(SECCOMP_DATA_NR),
    ];
    #[cfg(target_arch = "x86_64")]
    filter.extend([
        jump_equal(libc::SYS_fork as u32, 0, 1),
        ret(denied),
        jump_equal(libc::SYS_vfork as u32, 0, 1),
        ret(denied),
    ]);
    filter.extend([
        // Returning ENOSYS for clone3 makes pthread implementations fall back
        // to clone, whose flags seccomp can inspect directly.
        jump_equal(libc::SYS_clone3 as u32, 0, 1),
        ret(unavailable),
        jump_equal(libc::SYS_clone as u32, 0, 4),
        load(SECCOMP_DATA_ARG0_LOW),
        and(libc::CLONE_THREAD as u32),
        jump_equal(libc::CLONE_THREAD as u32, 1, 0),
        ret(denied),
        load(SECCOMP_DATA_NR),
        jump_equal(libc::SYS_socket as u32, 2, 0),
        jump_equal(libc::SYS_socketpair as u32, 1, 0),
        ret(SECCOMP_RET_ALLOW),
        load(SECCOMP_DATA_ARG0_LOW),
        jump_equal(libc::AF_UNIX as u32, 1, 0),
        ret(SECCOMP_RET_ERRNO | (libc::EAFNOSUPPORT as u32 & 0xffff)),
        ret(SECCOMP_RET_ALLOW),
    ]);
    Ok(filter)
}

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
fn seccomp_action_available(action: u32) -> bool {
    unsafe {
        libc::syscall(
            libc::SYS_seccomp,
            SECCOMP_GET_ACTION_AVAIL,
            0 as libc::c_ulong,
            &action as *const u32,
        ) == 0
    }
}

fn load(offset: u32) -> SockFilter {
    SockFilter {
        code: BPF_LD | BPF_W | BPF_ABS,
        jt: 0,
        jf: 0,
        k: offset,
    }
}

fn jump_equal(value: u32, jt: u8, jf: u8) -> SockFilter {
    SockFilter {
        code: BPF_JMP | BPF_JEQ | BPF_K,
        jt,
        jf,
        k: value,
    }
}

fn and(value: u32) -> SockFilter {
    SockFilter {
        code: BPF_ALU | BPF_AND | BPF_K,
        jt: 0,
        jf: 0,
        k: value,
    }
}

fn ret(action: u32) -> SockFilter {
    SockFilter {
        code: BPF_RET | BPF_K,
        jt: 0,
        jf: 0,
        k: action,
    }
}

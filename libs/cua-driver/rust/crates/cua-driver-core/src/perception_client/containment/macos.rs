//! macOS worker containment: process group, resource limits and a Seatbelt
//! profile that denies network and restricts writes to the private working
//! directory.
//!
//! The profile is applied by launching the worker through `/usr/bin/sandbox-exec`,
//! which is the only supported way for a parent to impose a Seatbelt profile on
//! a child it does not control. `sandbox_init` cannot be used from a `pre_exec`
//! hook: it allocates and talks to `sandboxd`, neither of which is
//! async-signal-safe after `fork`. When `sandbox-exec` is unavailable this
//! module fails closed rather than launching an unconstrained worker.

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};

use cua_driver_contract::VisualParseError;
use tokio::process::Child;

use super::unix::{apply_resource_limits, ProcessGroupGuard};
use super::{base_command, containment_error, spawn_error, unsupported_error, ContainmentLimits};

pub(super) type Guard = ProcessGroupGuard;

const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

pub(super) fn spawn(
    executable: &Path,
    args: &[String],
    working_directory: &Path,
    limits: &ContainmentLimits,
) -> Result<(Child, Guard), VisualParseError> {
    let sandbox_exec = Path::new(SANDBOX_EXEC);
    if !sandbox_exec.is_file() {
        return Err(unsupported_error(
            "this macOS build cannot sandbox the perception worker: sandbox-exec is unavailable",
        ));
    }
    // `sandbox-exec` is what is actually spawned, so a missing worker would
    // otherwise surface as a crashed worker instead of a failed launch.
    if !executable.is_file() {
        return Err(spawn_error(std::io::Error::from_raw_os_error(libc::ENOENT)));
    }
    let profile = build_profile(working_directory, &limits.additional_writable_paths)?;

    let mut command = base_command(sandbox_exec);
    command.arg("-p").arg(&profile).arg(executable).args(args);
    command.current_dir(working_directory);
    command.process_group(0);

    let max_memory_bytes = limits.max_memory_bytes;
    let max_cpu_seconds = limits.max_cpu_seconds();
    let max_processes = limits.max_processes;
    let max_open_files = limits.max_open_files;
    // Runs between fork and exec using only raw syscalls. Limits survive the
    // exec into `sandbox-exec` and its exec of the worker itself.
    unsafe {
        command.as_std_mut().pre_exec(move || {
            apply_resource_limits(
                max_memory_bytes,
                max_cpu_seconds,
                max_processes,
                max_open_files,
            )
        });
    }

    let child = command.spawn().map_err(spawn_error)?;
    let guard = ProcessGroupGuard::new(child.id());
    Ok((child, guard))
}

/// Deny-by-default Seatbelt profile.
///
/// Reads stay broad because the worker must load its own runtime, model
/// artifacts and the dyld shared cache, none of which live under the private
/// working directory. Writes are confined to the working directory, the
/// explicitly opted-in extra paths and a small set of character devices that
/// process startup requires.
fn build_profile(
    working_directory: &Path,
    additional_writable_paths: &[PathBuf],
) -> Result<String, VisualParseError> {
    let mut profile = String::from(
        "(version 1)\n\
         (deny default)\n\
         (deny network*)\n\
         (allow process-exec*)\n\
         (allow process-info* (target self))\n\
         (allow signal (target self))\n\
         (allow sysctl-read)\n\
         (allow mach-lookup)\n\
         (allow ipc-posix-shm)\n\
         (allow iokit-open)\n\
         (allow file-read*)\n\
         (allow file-map-executable)\n\
         (allow file-ioctl)\n\
         (allow file-write-data\n\
         \x20 (literal \"/dev/null\")\n\
         \x20 (literal \"/dev/zero\")\n\
         \x20 (literal \"/dev/random\")\n\
         \x20 (literal \"/dev/urandom\")\n\
         \x20 (literal \"/dev/dtracehelper\"))\n\
         (allow file-write*\n",
    );
    let extras = additional_writable_paths.iter().map(PathBuf::as_path);
    for path in std::iter::once(working_directory).chain(extras) {
        // Seatbelt matches the resolved path, and the per-user temporary
        // directory reaches the worker through the `/var` symlink.
        let resolved = std::fs::canonicalize(path).map_err(|cause| {
            containment_error(
                "failed to resolve a writable path for the macOS worker sandbox",
                super::os_detail(&cause),
            )
        })?;
        let text = resolved.to_str().ok_or_else(|| {
            containment_error(
                "a writable path for the macOS worker sandbox is not valid UTF-8",
                None,
            )
        })?;
        profile.push_str("  (subpath \"");
        profile.push_str(&escape(text));
        profile.push_str("\")\n");
    }
    profile.push_str(")\n");
    Ok(profile)
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

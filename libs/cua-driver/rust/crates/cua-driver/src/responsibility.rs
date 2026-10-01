#[cfg(target_os = "macos")]
use std::ffi::CString;

#[cfg(target_os = "macos")]
use std::os::unix::ffi::{OsStrExt, OsStringExt};

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn responsibility_spawnattrs_setdisclaim(
        attr: *mut libc::posix_spawnattr_t,
        disclaim: libc::c_int,
    ) -> libc::c_int;
}

#[cfg(target_os = "macos")]
pub fn already_disclaimed() -> bool {
    std::env::var_os(cua_driver_core::RESPONSIBILITY_DISCLAIMED_ENV).is_some()
}

/// Split out from [`reexec_disclaimed_if_needed`] so the decision is
/// testable without spawning. Embedded mode must skip the disclaim:
/// disclaiming would make the driver its own responsible process and
/// break TCC inheritance from the host.
#[cfg(target_os = "macos")]
fn should_skip_disclaim(embedded: bool, already_disclaimed: bool, inside_bundle: bool) -> bool {
    embedded || already_disclaimed || inside_bundle
}

#[cfg(target_os = "macos")]
pub fn reexec_disclaimed_if_needed() {
    if should_skip_disclaim(
        cua_driver_core::embedded_mode(),
        already_disclaimed(),
        crate::bundle::is_executable_inside_cuadriver_app(),
    ) {
        return;
    }

    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            tracing::debug!("responsibility disclaim skipped: current_exe failed: {e}");
            return;
        }
    };

    let exe_c = match CString::new(exe.as_os_str().as_bytes()) {
        Ok(exe) => exe,
        Err(e) => {
            tracing::debug!("responsibility disclaim skipped: executable path contains NUL: {e}");
            return;
        }
    };

    let argv: Vec<CString> = match std::env::args_os()
        .map(|arg| CString::new(arg.as_bytes()))
        .collect::<Result<_, _>>()
    {
        Ok(argv) => argv,
        Err(e) => {
            tracing::debug!("responsibility disclaim skipped: argv contains NUL: {e}");
            return;
        }
    };
    let mut argv_ptrs: Vec<*mut libc::c_char> = argv
        .iter()
        .map(|arg| arg.as_ptr() as *mut libc::c_char)
        .collect();
    argv_ptrs.push(std::ptr::null_mut());

    let env: Vec<CString> = match std::env::vars_os()
        .map(|(key, value)| {
            let mut entry = key.into_vec();
            entry.push(b'=');
            entry.extend(value.into_vec());
            CString::new(entry)
        })
        .chain(std::iter::once(CString::new(format!(
            "{}=1",
            cua_driver_core::RESPONSIBILITY_DISCLAIMED_ENV
        ))))
        .collect::<Result<_, _>>()
    {
        Ok(env) => env,
        Err(e) => {
            tracing::debug!("responsibility disclaim skipped: environment contains NUL: {e}");
            return;
        }
    };
    let mut env_ptrs: Vec<*mut libc::c_char> = env
        .iter()
        .map(|entry| entry.as_ptr() as *mut libc::c_char)
        .collect();
    env_ptrs.push(std::ptr::null_mut());

    let mut attr = std::mem::MaybeUninit::<libc::posix_spawnattr_t>::uninit();
    let init_result = unsafe { libc::posix_spawnattr_init(attr.as_mut_ptr()) };
    if init_result != 0 {
        tracing::debug!(
            "responsibility disclaim skipped: posix_spawnattr_init failed: {init_result}"
        );
        return;
    }
    let mut attr = unsafe { attr.assume_init() };

    let disclaim_result = unsafe { responsibility_spawnattrs_setdisclaim(&mut attr, 1) };
    if disclaim_result != 0 {
        tracing::debug!(
            "responsibility disclaim skipped: responsibility_spawnattrs_setdisclaim failed: {disclaim_result}"
        );
        unsafe {
            libc::posix_spawnattr_destroy(&mut attr);
        }
        return;
    }

    let mut child_pid: libc::pid_t = 0;
    let spawn_result = unsafe {
        libc::posix_spawn(
            &mut child_pid,
            exe_c.as_ptr(),
            std::ptr::null(),
            &attr,
            argv_ptrs.as_ptr(),
            env_ptrs.as_ptr(),
        )
    };
    unsafe {
        libc::posix_spawnattr_destroy(&mut attr);
    }

    if spawn_result != 0 {
        tracing::debug!("responsibility disclaim skipped: posix_spawn failed: {spawn_result}");
        return;
    }

    // The disclaimed child is the real `serve` process. Block on it and mirror
    // its exit status so the launch keeps its original foreground semantics:
    // callers that wait on `serve`, forward
    // terminal signals to the foreground process group, or read `$?` still see
    // the same behavior. The child shares our process group (no
    // POSIX_SPAWN_SETPGROUP), so Ctrl-C reaches it directly.
    let mut status: libc::c_int = 0;
    loop {
        let waited = unsafe { libc::waitpid(child_pid, &mut status, 0) };
        if waited == -1 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            // Can't reap the child (already reaped, or ECHILD). The child owns
            // the daemon either way; exit cleanly rather than fall through and
            // double-run `serve` in this process.
            tracing::debug!("responsibility disclaim: waitpid failed: {err}");
            std::process::exit(0);
        }
        break;
    }

    let code = if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status)
    } else if libc::WIFSIGNALED(status) {
        128 + libc::WTERMSIG(status)
    } else {
        0
    };
    std::process::exit(code);
}

#[cfg(not(target_os = "macos"))]
pub fn reexec_disclaimed_if_needed() {}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn embedded_mode_skips_disclaim_reexec() {
        assert!(should_skip_disclaim(true, false, false));
        // A bare standalone binary must still disclaim.
        assert!(!should_skip_disclaim(false, false, false));
        assert!(should_skip_disclaim(false, true, false));
        assert!(should_skip_disclaim(false, false, true));
    }

    #[test]
    fn already_disclaimed_reflects_env_var() {
        let name = cua_driver_core::RESPONSIBILITY_DISCLAIMED_ENV;
        let original = std::env::var_os(name);

        std::env::remove_var(name);
        assert!(!already_disclaimed());

        std::env::set_var(name, "1");
        assert!(already_disclaimed());

        match original {
            Some(value) => std::env::set_var(name, value),
            None => std::env::remove_var(name),
        }
    }
}

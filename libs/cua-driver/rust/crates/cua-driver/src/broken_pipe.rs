//! Clean exit when a finite CLI command's reader closes its output early.
//!
//! The CLI prints through `println!`/`eprintln!`, which panic with
//! `failed printing to stdout: <io error>` when the reader of a pipe has gone
//! away (for example `cua-driver status | head -3`). Rust ignores `SIGPIPE`, so
//! the write returns `EPIPE` on Unix, and Windows reports `ERROR_NO_DATA` /
//! `ERROR_BROKEN_PIPE`; both map to [`std::io::ErrorKind::BrokenPipe`].
//!
//! Restoring the default `SIGPIPE` disposition process-wide would also kill
//! the CLI silently when an internal socket or worker pipe closes, hiding the
//! diagnostics those paths already report. Instead, finite commands install a
//! panic hook that recognises only a broken pipe on the process's own stdout or
//! stderr and terminates the way a conventional pipeline stage does: on Unix
//! the process dies from `SIGPIPE` (status 141 in shells), and on Windows it
//! exits with the same status code 141. Every other panic keeps the previous
//! hook's behavior.
//!
//! Long-lived transports (`mcp`, `serve`, and bare invocation, which runs MCP)
//! do not install the hook, so their existing per-task panic handling is
//! unchanged.

use std::io::ErrorKind;

/// Exit status a shell reports for a process terminated by `SIGPIPE`.
pub const BROKEN_PIPE_EXIT_CODE: i32 = 128 + 13;

/// Install the broken-pipe hook when argv names a finite command.
pub fn install_for_finite_command_from_argv() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if crate::cli::is_long_lived_transport_command(&args) {
        return;
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let message = info
            .payload()
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| info.payload().downcast_ref::<&str>().copied());
        if message.is_some_and(is_broken_pipe_print_panic) {
            terminate_for_broken_pipe();
        }
        previous(info);
    }));
}

/// True only for the standard library's stdout/stderr print panic whose
/// underlying OS error is a broken pipe on the current platform.
pub fn is_broken_pipe_print_panic(message: &str) -> bool {
    let Some(error) = message
        .strip_prefix("failed printing to stdout: ")
        .or_else(|| message.strip_prefix("failed printing to stderr: "))
    else {
        return false;
    };
    let Some(code) = error
        .trim_end()
        .strip_suffix(')')
        .and_then(|rest| rest.rsplit_once("(os error "))
        .and_then(|(_, code)| code.parse::<i32>().ok())
    else {
        return false;
    };
    std::io::Error::from_raw_os_error(code).kind() == ErrorKind::BrokenPipe
}

/// Map a child's exit status to the status a shell would report, so the
/// telemetry wrapper preserves a signal death such as `SIGPIPE` as `128 + n`
/// instead of collapsing it to a generic failure.
pub fn shell_exit_code(status: std::process::ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }
    1
}

fn terminate_for_broken_pipe() -> ! {
    #[cfg(unix)]
    // SAFETY: restoring the default disposition and raising a signal on the
    // current process are async-signal-safe libc calls with no Rust invariants.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
        libc::raise(libc::SIGPIPE);
    }
    std::process::exit(BROKEN_PIPE_EXIT_CODE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn print_panic(stream: &str, error: std::io::Error) -> String {
        format!("failed printing to {stream}: {error}")
    }

    #[test]
    fn recognises_the_platform_broken_pipe_print_panic() {
        #[cfg(unix)]
        let codes = [libc::EPIPE];
        // ERROR_BROKEN_PIPE and ERROR_NO_DATA ("The pipe is being closed.").
        #[cfg(windows)]
        let codes = [109, 232];
        for code in codes {
            let error = std::io::Error::from_raw_os_error(code);
            assert!(is_broken_pipe_print_panic(&print_panic("stdout", error)));
            let error = std::io::Error::from_raw_os_error(code);
            assert!(is_broken_pipe_print_panic(&print_panic("stderr", error)));
        }
    }

    #[test]
    fn keeps_other_print_failures_and_panics_visible() {
        #[cfg(unix)]
        let other = libc::ENOSPC;
        #[cfg(windows)]
        let other = 112; // ERROR_DISK_FULL
        let error = std::io::Error::from_raw_os_error(other);
        assert!(!is_broken_pipe_print_panic(&print_panic("stdout", error)));
        let custom = std::io::Error::new(ErrorKind::BrokenPipe, "synthetic");
        assert!(!is_broken_pipe_print_panic(&print_panic("stdout", custom)));
        assert!(!is_broken_pipe_print_panic(
            "called `Result::unwrap()` on an `Err` value: Broken pipe (os error 32)"
        ));
        assert!(!is_broken_pipe_print_panic("failed printing to stdout"));
    }

    #[cfg(unix)]
    #[test]
    fn wrapper_reports_signal_deaths_like_a_shell() {
        use std::os::unix::process::ExitStatusExt;
        let status = std::process::ExitStatus::from_raw(libc::SIGPIPE);
        assert_eq!(shell_exit_code(status), BROKEN_PIPE_EXIT_CODE);
        let status = std::process::ExitStatus::from_raw(3 << 8);
        assert_eq!(shell_exit_code(status), 3);
    }
}

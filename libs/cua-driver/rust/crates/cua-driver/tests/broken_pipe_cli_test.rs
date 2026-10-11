//! A finite CLI command whose reader closes early (`cua-driver ... | head`)
//! must stop like a normal pipeline stage instead of panicking with
//! `failed printing to stdout: Broken pipe`.

use std::process::{Command, ExitStatus, Stdio};
use tempfile::TempDir;

/// Shell-visible status for a process terminated by SIGPIPE; Windows uses the
/// same exit code.
const BROKEN_PIPE_EXIT_CODE: i32 = 141;

fn run_with_closed_stdout(wrapped_child: bool) -> (ExitStatus, String) {
    let home = TempDir::new().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_cua-driver"));
    // `dump-docs` is desktop-free and prints far more than a pipe buffer, so
    // the write fails even if the reader closes after the first chunk.
    command
        .args(["dump-docs", "--pretty"])
        .env("CUA_DRIVER_RS_HOME", home.path())
        .env("CUA_DRIVER_TELEMETRY_HOME", home.path())
        .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if wrapped_child {
        command.env("CUA_DRIVER_CLI_TELEMETRY_CHILD", "1");
    } else {
        command.env_remove("CUA_DRIVER_CLI_TELEMETRY_CHILD");
    }
    let mut child = command.spawn().expect("spawn cua-driver");
    drop(child.stdout.take());
    let output = child.wait_with_output().expect("wait for cua-driver");
    (
        output.status,
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn assert_quiet(stderr: &str) {
    assert!(
        !stderr.contains("panicked") && !stderr.contains("failed printing"),
        "closed stdout must not panic:\n{stderr}"
    );
}

#[test]
fn finite_command_stops_cleanly_when_its_reader_closes() {
    let (status, stderr) = run_with_closed_stdout(true);
    assert_quiet(&stderr);
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(libc::SIGPIPE), "{status:?}");
    }
    #[cfg(windows)]
    assert_eq!(status.code(), Some(BROKEN_PIPE_EXIT_CODE), "{status:?}");
}

#[test]
fn telemetry_wrapper_reports_the_broken_pipe_status_like_a_shell() {
    let (status, stderr) = run_with_closed_stdout(false);
    assert_quiet(&stderr);
    assert_eq!(status.code(), Some(BROKEN_PIPE_EXIT_CODE), "{status:?}");
}

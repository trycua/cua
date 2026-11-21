// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `serve --owner-pid PID`: the daemon runs only while its owner does.

#![cfg(any(unix, target_os = "windows"))]

use cua_driver_testkit::{spawn_in_job, IsolatedStateRoot};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

const BOUND: Duration = Duration::from_secs(20);
const REFUSAL: &str = "no running process has this PID";

/// Kills and reaps the child if the test ends while it still runs.
struct Reaped(Child);

impl Drop for Reaped {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Daemon {
    state: IsolatedStateRoot,
    #[cfg(unix)]
    _endpoint_dir: tempfile::TempDir,
    socket: String,
    pid_file: PathBuf,
}

impl Daemon {
    fn new(name: &str) -> Self {
        let state = IsolatedStateRoot::new().unwrap();
        #[cfg(unix)]
        let endpoint_dir = cua_driver_testkit::socket_tempdir().unwrap();
        #[cfg(unix)]
        let socket = endpoint_dir
            .path()
            .join(format!("{name}.sock"))
            .display()
            .to_string();
        #[cfg(target_os = "windows")]
        let socket = format!(r"\\.\pipe\cua-owner-pid-{name}-{}", std::process::id());
        let pid_file = state.path().join("daemon.pid");
        Self {
            state,
            #[cfg(unix)]
            _endpoint_dir: endpoint_dir,
            socket,
            pid_file,
        }
    }

    fn serve(&self, owner_pid: u32) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cua-driver"));
        command
            .args(["serve", "--socket", &self.socket, "--pid-file"])
            .arg(&self.pid_file)
            .args(["--owner-pid", &owner_pid.to_string()])
            .args(["--no-overlay", "--no-permissions-gate"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .envs(self.state.env())
            .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "false")
            .env("CUA_DRIVER_RS_UPDATE_CHECK", "false");
        command
    }

    /// Runs a `serve` that is expected to refuse its owner at startup.
    fn refused(&self, owner_pid: u32) -> (ExitStatus, String) {
        let mut serve = self.serve(owner_pid);
        serve.stderr(Stdio::piped());
        let mut daemon = Reaped(spawn_in_job(&mut serve).expect("spawn cua-driver serve"));
        let status = exit_within(&mut daemon.0, BOUND)
            .unwrap_or_else(|| panic!("serve accepted owner {owner_pid} and kept running"));
        let mut stderr = String::new();
        std::io::Read::read_to_string(daemon.0.stderr.as_mut().unwrap(), &mut stderr).unwrap();
        (status, stderr)
    }
}

/// A process that runs for `seconds` and then exits.
fn spawn_owner(seconds: u32) -> Child {
    let mut command = if cfg!(windows) {
        let mut ping = Command::new("ping");
        ping.args(["-n", &(seconds + 1).to_string(), "127.0.0.1"]);
        ping
    } else {
        let mut sleep = Command::new("sleep");
        sleep.arg(seconds.to_string());
        sleep
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    spawn_in_job(&mut command).expect("spawn owner process")
}

fn exit_within(child: &mut Child, limit: Duration) -> Option<ExitStatus> {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(status) = child.try_wait().expect("poll child") {
            return Some(status);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Blocks until `child` exits, leaving it unreaped.
fn wait_without_reaping(child: &Child) {
    #[cfg(unix)]
    {
        // SAFETY: an all-zero siginfo_t is a valid out buffer.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // SAFETY: WNOWAIT leaves the child waitable for `Child::wait`.
        let waited = unsafe {
            libc::waitid(
                libc::P_PID,
                child.id() as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOWAIT,
            )
        };
        assert_eq!(waited, 0, "waitid: {}", std::io::Error::last_os_error());
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::io::AsRawHandle as _;
        use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
        use windows::Win32::System::Threading::{WaitForSingleObject, INFINITE};
        // SAFETY: `child` owns this live process handle for the whole call.
        let waited = unsafe { WaitForSingleObject(HANDLE(child.as_raw_handle()), INFINITE) };
        assert_eq!(waited, WAIT_OBJECT_0);
    }
}

#[test]
fn serve_shuts_down_and_removes_its_pid_file_once_its_owner_exits() {
    let daemon = Daemon::new("exits");
    let mut owner = Reaped(spawn_owner(120));
    let mut serve = daemon.serve(owner.0.id());
    serve.stderr(if std::env::var_os("CUA_TEST_DRIVER_STDERR").is_some() {
        Stdio::inherit()
    } else {
        Stdio::null()
    });
    let mut running = Reaped(spawn_in_job(&mut serve).expect("spawn cua-driver serve"));

    let deadline = Instant::now() + BOUND;
    while std::fs::read_to_string(&daemon.pid_file)
        .unwrap_or_default()
        .trim()
        .is_empty()
    {
        if let Some(status) = running.0.try_wait().expect("poll serve") {
            panic!("serve exited while its owner ran: {status}");
        }
        assert!(
            Instant::now() < deadline,
            "serve wrote no pid file within {BOUND:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        running.0.try_wait().expect("poll serve").is_none(),
        "serve exited while its owner ran"
    );

    owner.0.kill().expect("kill the owner");
    owner.0.wait().expect("reap the owner");
    let status = exit_within(&mut running.0, BOUND)
        .unwrap_or_else(|| panic!("serve outlived its owner by {BOUND:?}"));
    assert!(status.success(), "serve exited with {status}");
    assert!(!daemon.pid_file.exists(), "serve left its pid file behind");
}

#[test]
fn serve_refuses_an_owner_pid_whose_process_is_gone() {
    let daemon = Daemon::new("gone");
    let mut owner = spawn_owner(0);
    let pid = owner.id();
    owner.wait().expect("reap the owner");
    // Closes the Windows process handle, so the process object is gone too.
    drop(owner);

    let (status, stderr) = daemon.refused(pid);
    assert_eq!(status.code(), Some(1), "{stderr}");
    assert!(stderr.contains(REFUSAL), "{stderr}");
}

#[test]
fn serve_refuses_an_owner_that_exited_but_is_not_reaped() {
    let daemon = Daemon::new("unreaped");
    let mut owner = Reaped(spawn_owner(0));
    wait_without_reaping(&owner.0);

    let (status, stderr) = daemon.refused(owner.0.id());
    owner.0.wait().expect("reap the owner");
    assert_eq!(status.code(), Some(1), "{stderr}");
    assert!(stderr.contains(REFUSAL), "{stderr}");
}

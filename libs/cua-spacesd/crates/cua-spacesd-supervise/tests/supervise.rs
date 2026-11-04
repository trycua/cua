// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The supervisor against real processes. Each case runs `run` in a child
//! copy of this test binary (its signal handlers are process-wide): the
//! helper test below builds the `Config` from `SUP_*` variables.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cua_spacesd_supervise::{run, Config, OnExit, Server, Workload};

/// Entry point of the child copy; a no-op in the normal test run.
#[test]
fn helper_run_supervisor() {
    let Ok(argv) = std::env::var("SUP_WORKLOAD") else {
        return;
    };
    let shell = std::env::var("SUP_SHELL").unwrap_or_else(|_| "/bin/sh".into());
    let workload = (!argv.is_empty()).then(|| Workload {
        argv: vec![shell, "-c".into(), argv.clone()],
        user: std::env::var("SUP_USER").ok(),
        workdir: std::env::var("SUP_WORKDIR").ok().map(PathBuf::from),
        env_remove: vec!["SUP_SECRET".into()],
    });
    let server = std::env::var("SUP_SERVER").ok().map(|s| Server {
        program: "/bin/sh".into(),
        args: vec!["-c".into(), s],
        env_defaults: vec![("SUP_DEFAULTED".into(), "yes".into())],
    });
    let on_exit = if std::env::var("SUP_SERVE").is_ok() {
        OnExit::Serve
    } else {
        OnExit::Exit
    };
    let code = run(Config {
        workload,
        server,
        on_exit,
        stop_grace: Duration::from_secs(2),
    });
    std::process::exit(code);
}

struct Case {
    /// The supervisor's standard error is a pipe (owned by whoever runs it).
    pipe_stderr: bool,
    /// The shell that runs `workload` (default `/bin/sh`).
    shell: Option<String>,
    workload: String,
    server: Option<String>,
    serve: bool,
    user: Option<String>,
    workdir: Option<PathBuf>,
}

fn case(workload: &str) -> Case {
    Case {
        pipe_stderr: false,
        shell: None,
        workload: workload.into(),
        server: None,
        serve: false,
        user: None,
        workdir: None,
    }
}

fn start(c: &Case) -> std::process::Child {
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args([
        "--exact",
        "helper_run_supervisor",
        "--nocapture",
        "--test-threads=1",
    ])
    .env("SUP_WORKLOAD", &c.workload)
    .env("SUP_SECRET", "the-token")
    .stdout(Stdio::null())
    .stderr(Stdio::null());
    if let Some(s) = &c.server {
        cmd.env("SUP_SERVER", s);
    }
    if c.serve {
        cmd.env("SUP_SERVE", "1");
    }
    if let Some(sh) = &c.shell {
        cmd.env("SUP_SHELL", sh);
    }
    if c.pipe_stderr {
        cmd.stderr(Stdio::piped());
    }
    if let Some(u) = &c.user {
        cmd.env("SUP_USER", u);
    }
    if let Some(d) = &c.workdir {
        cmd.env("SUP_WORKDIR", d);
    }
    cmd.spawn().unwrap()
}

fn wait_for(path: &Path) {
    let end = Instant::now() + Duration::from_secs(10);
    while !path.exists() {
        assert!(Instant::now() < end, "{} never appeared", path.display());
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_code(child: &mut std::process::Child) -> i32 {
    let end = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(s) = child.try_wait().unwrap() {
            use std::os::unix::process::ExitStatusExt;
            return s.code().unwrap_or_else(|| 128 + s.signal().unwrap_or(0));
        }
        if Instant::now() > end {
            let _ = child.kill();
            panic!("the supervisor did not end");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn signal(child: &std::process::Child, sig: libc::c_int) {
    unsafe {
        libc::kill(child.id() as i32, sig);
    }
}

#[test]
fn the_commands_exit_status_becomes_the_supervisors() {
    assert_eq!(wait_code(&mut start(&case("exit 7"))), 7);
    assert_eq!(wait_code(&mut start(&case("exit 0"))), 0);
}

#[test]
fn a_signalled_command_ends_with_128_plus_the_signal() {
    assert_eq!(wait_code(&mut start(&case("kill -9 $$"))), 137);
}

#[test]
fn a_command_that_cannot_start_ends_with_127() {
    let tmp = tempdir();
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", "helper_missing_program", "--nocapture"])
        .env("SUP_MISSING", tmp.to_str().unwrap())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    assert_eq!(wait_code(&mut cmd.spawn().unwrap()), 127);
}

#[test]
fn helper_missing_program() {
    if std::env::var("SUP_MISSING").is_err() {
        return;
    }
    let code = run(Config {
        workload: Some(Workload {
            argv: vec!["/no/such/program".into()],
            ..Default::default()
        }),
        server: None,
        on_exit: OnExit::Exit,
        stop_grace: Duration::from_secs(1),
    });
    std::process::exit(code);
}

fn tempdir() -> PathBuf {
    let d = std::env::temp_dir().join(format!("sup-{}-{}", std::process::id(), rand_suffix()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn rand_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

#[test]
fn termination_signals_reach_the_command_and_its_status_is_kept() {
    let d = tempdir();
    let ready = d.join("ready");
    let mut s = start(&case(&format!(
        "trap 'exit 42' TERM; touch {}; while :; do sleep 0.05; done",
        ready.display()
    )));
    wait_for(&ready);
    signal(&s, libc::SIGTERM);
    assert_eq!(wait_code(&mut s), 42);
}

#[test]
fn other_signals_are_forwarded_too() {
    let d = tempdir();
    let ready = d.join("ready");
    let mut s = start(&case(&format!(
        "trap 'exit 33' USR1; touch {}; while :; do sleep 0.05; done",
        ready.display()
    )));
    wait_for(&ready);
    signal(&s, libc::SIGUSR1);
    assert_eq!(wait_code(&mut s), 33);
}

#[test]
fn window_and_alarm_signals_reach_the_command() {
    for (name, sig, code) in [("WINCH", libc::SIGWINCH, 45), ("ALRM", libc::SIGALRM, 46)] {
        let d = tempdir();
        let ready = d.join("ready");
        let mut s = start(&case(&format!(
            "trap 'exit {code}' {name}; touch {}; while :; do sleep 0.05; done",
            ready.display()
        )));
        wait_for(&ready);
        signal(&s, sig);
        assert_eq!(wait_code(&mut s), code, "SIG{name}");
    }
}

/// Real-time signals (an image may stop on `SIGRTMIN+3`, as systemd does)
/// are forwarded too: bash can trap them by name.
#[cfg(target_os = "linux")]
#[test]
fn real_time_signals_reach_the_command() {
    let d = tempdir();
    let ready = d.join("ready");
    let mut c = case(&format!(
        "trap 'exit 47' RTMIN+3; touch {}; while :; do sleep 0.05; done",
        ready.display()
    ));
    c.shell = Some("/bin/bash".into());
    if !std::path::Path::new("/bin/bash").exists() {
        eprintln!("skipped: needs bash");
        return;
    }
    let mut s = start(&c);
    wait_for(&ready);
    signal(&s, libc::SIGRTMIN() + 3);
    assert_eq!(wait_code(&mut s), 47);
}

#[test]
fn the_server_runs_alongside_and_stops_with_the_command() {
    let d = tempdir();
    let up = d.join("server-up");
    let gone = d.join("server-term");
    let mut c = case(&format!(
        "while [ ! -e {} ]; do sleep 0.05; done; exit 5",
        up.display()
    ));
    c.server = Some(format!(
        "trap 'touch {}; exit 0' TERM; touch {}; while :; do sleep 0.05; done",
        gone.display(),
        up.display()
    ));
    let mut s = start(&c);
    assert_eq!(wait_code(&mut s), 5);
    assert!(gone.exists(), "cua-spacesd was told to stop");
}

#[test]
fn a_server_that_dies_is_started_again() {
    let d = tempdir();
    let log = d.join("starts");
    let mut c = case("sleep 3; exit 0");
    c.server = Some(format!("echo x >> {}; exit 1", log.display()));
    let mut s = start(&c);
    assert_eq!(wait_code(&mut s), 0);
    let starts = std::fs::read_to_string(&log).unwrap().lines().count();
    assert!(starts >= 3, "restarted with a backoff, got {starts} starts");
}

#[test]
fn a_stuck_server_is_killed_after_the_grace() {
    let d = tempdir();
    let up = d.join("server-up");
    // The command ends only once the server ignores TERM.
    let mut c = case(&format!(
        "while [ ! -e {} ]; do sleep 0.02; done; exit 0",
        up.display()
    ));
    c.server = Some(format!(
        "trap '' TERM; touch {}; while :; do sleep 0.05; done",
        up.display()
    ));
    let t = Instant::now();
    let mut s = start(&c);
    assert_eq!(wait_code(&mut s), 0);
    assert!(
        t.elapsed() >= Duration::from_secs(2),
        "waited for the grace"
    );
}

#[test]
fn the_serve_policy_outlives_the_command_and_ends_zero_on_term() {
    let d = tempdir();
    let up = d.join("server-up");
    let mut c = case("exit 3");
    c.serve = true;
    c.server = Some(format!(
        "trap 'exit 0' TERM; touch {}; while :; do sleep 0.05; done",
        up.display()
    ));
    let mut s = start(&c);
    wait_for(&up);
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        s.try_wait().unwrap().is_none(),
        "still serving after exit 3"
    );
    signal(&s, libc::SIGTERM);
    assert_eq!(wait_code(&mut s), 0);
}

#[test]
fn the_command_does_not_see_the_token_and_the_server_gets_defaults() {
    let d = tempdir();
    let out = d.join("env");
    let sv = d.join("server-env");
    // The command waits for the server's report, then reports its own.
    let mut c = case(&format!(
        "while [ ! -s {} ]; do sleep 0.02; done; [ -z \"$SUP_SECRET\" ] && echo hidden > {}; exit 0",
        sv.display(),
        out.display()
    ));
    c.server = Some(format!(
        "echo \"$SUP_SECRET/$SUP_DEFAULTED\" > {}.tmp && mv {}.tmp {}; sleep 5",
        sv.display(),
        sv.display(),
        sv.display()
    ));
    let mut s = start(&c);
    assert_eq!(wait_code(&mut s), 0);
    assert_eq!(std::fs::read_to_string(&out).unwrap().trim(), "hidden");
    assert_eq!(
        std::fs::read_to_string(&sv).unwrap().trim(),
        "the-token/yes",
        "the server keeps the token and gets the defaults"
    );
}

#[test]
fn the_working_directory_is_the_images() {
    let d = tempdir();
    let out = d.join("pwd");
    let mut c = case(&format!("pwd -P > {}", out.display()));
    c.workdir = Some(d.canonicalize().unwrap());
    assert_eq!(wait_code(&mut start(&c)), 0);
    assert_eq!(
        std::fs::read_to_string(&out).unwrap().trim(),
        d.canonicalize().unwrap().to_str().unwrap()
    );
}

/// Orphans are reparented to the supervisor (a subreaper) and reaped.
#[cfg(target_os = "linux")]
#[test]
fn orphaned_grandchildren_are_reaped_not_left_as_zombies() {
    let d = tempdir();
    let out = d.join("zombies");
    // The grandchild outlives its parent shell, exits, and must not linger.
    let script = format!(
        "(sleep 0.3 &) ; sleep 1; \
         n=$(ps -eo ppid=,stat= | awk -v p=$PPID '$1==p && $2 ~ /^Z/' | wc -l); \
         echo $n > {}; exit 0",
        out.display()
    );
    assert_eq!(wait_code(&mut start(&case(&script))), 0);
    assert_eq!(std::fs::read_to_string(&out).unwrap().trim(), "0");
}

/// As root: the command runs as the image's user, with its home.
#[cfg(target_os = "linux")]
#[test]
fn the_command_runs_as_the_images_user_when_root() {
    if unsafe { libc::geteuid() } != 0 {
        eprintln!("skipped: needs root");
        return;
    }
    let d = tempdir();
    std::fs::set_permissions(&d, std::os::unix::fs::PermissionsExt::from_mode(0o777)).unwrap();
    let out = d.join("id");
    let mut c = case(&format!(
        "echo \"$(id -u):$(id -g):$HOME\" > {}",
        out.display()
    ));
    c.user = Some("65534:65534".into());
    assert_eq!(wait_code(&mut start(&c)), 0);
    // HOME is the passwd entry's, when the image has one for the uid.
    let passwd = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
    let home = cua_spacesd_supervise::user::resolve("65534", &passwd, "")
        .ok()
        .and_then(|i| i.home)
        .unwrap_or_else(|| std::env::var("HOME").unwrap_or_default());
    assert_eq!(
        std::fs::read_to_string(&out).unwrap().trim(),
        format!("65534:65534:{home}")
    );
}

/// An image that logs to `/dev/stderr` (nginx) as a non-root user: the pipe
/// behind it is handed to the user, as a container runtime does.
#[cfg(target_os = "linux")]
#[test]
fn a_command_running_as_a_user_can_write_to_dev_stderr() {
    if unsafe { libc::geteuid() } != 0 {
        eprintln!("skipped: needs root");
        return;
    }
    let mut c = case("echo logged > /dev/stderr");
    c.user = Some("65534:65534".into());
    c.pipe_stderr = true;
    let mut s = start(&c);
    let mut seen = String::new();
    std::io::Read::read_to_string(s.stderr.as_mut().unwrap(), &mut seen).unwrap();
    assert_eq!(wait_code(&mut s), 0, "{seen}");
    assert!(seen.contains("logged"), "{seen}");
}

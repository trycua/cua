#![cfg(unix)]
//! A driver child that stops answering is reported with what it said, and an
//! observation that failed is never read as an absent window.

use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use cua_driver_testkit::McpDriver;
use serde_json::json;

const SENTINEL: &str = "incompatible daemon: testkit stub sentinel";

/// Answers `initialize`, logs each `tools/call` to `<socket dir>/calls`, and
/// then either exits with [`SENTINEL`] on stderr (when `<socket dir>/exit`
/// exists) or answers `list_windows` with no windows.
const STUB_DRIVER: &str = r#"#!/bin/sh
dir=$(dirname "$3")
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id" ;;
    *'"method":"tools/call"'*)
      echo call >> "$dir/calls"
      if [ -e "$dir/exit" ]; then
        echo "cua-driver-rs: invalid daemon response: SENTINEL" >&2
        exit 1
      fi
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"0 windows"}],"structuredContent":{"windows":[]}}}\n' "$id" ;;
  esac
done
"#;

static STUB_BIN: LazyLock<PathBuf> = LazyLock::new(|| {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join("testkit-stub-driver.sh");
    std::fs::write(&path, STUB_DRIVER.replace("SENTINEL", SENTINEL)).expect("write stub driver");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("mark stub driver executable");
    std::env::set_var("CUA_TEST_DRIVER_BIN", &path);
    path
});

struct StubSession {
    dir: tempfile::TempDir,
    _daemon: UnixListener,
    driver: McpDriver,
}

impl StubSession {
    fn spawn(exit_on_call: bool) -> Self {
        LazyLock::force(&STUB_BIN);
        let dir = tempfile::tempdir().expect("stub session dir");
        if exit_on_call {
            std::fs::write(dir.path().join("exit"), "").expect("select exit mode");
        }
        let socket = dir.path().join("daemon.sock");
        let daemon = UnixListener::bind(&socket).expect("bind stand-in daemon socket");
        let driver = McpDriver::spawn_daemon_proxy_unrecorded(socket.to_str().unwrap())
            .expect("spawn stub driver through CUA_TEST_DRIVER_BIN");
        Self {
            dir,
            _daemon: daemon,
            driver,
        }
    }

    fn list_windows_calls(&self) -> usize {
        std::fs::read_to_string(self.dir.path().join("calls"))
            .map(|calls| calls.lines().count())
            .unwrap_or(0)
    }
}

#[test]
fn a_child_that_exits_is_reported_as_exited_with_its_reason_not_as_a_timeout() {
    let mut session = StubSession::spawn(true);
    let reply = session
        .driver
        .call_raw("list_windows", json!({ "pid": 1 }));
    let error = reply["error"].as_str().expect("an error reply");
    assert!(
        error.starts_with("driver exited during list_windows"),
        "{error}"
    );
    assert!(!error.contains("TIMEOUT"), "{error}");
    assert!(error.contains(SENTINEL), "{error}");
}

#[test]
fn find_window_fails_on_the_first_failed_observation_with_the_childs_reason() {
    let mut session = StubSession::spawn(true);
    let started = Instant::now();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        session.driver.find_window(1, "CuaTestHarness AppKit")
    }));
    let elapsed = started.elapsed();
    let panic = outcome.expect_err("an errored list_windows must fail, not report an absent window");
    let message = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&str>().copied())
        .unwrap_or_default();
    assert!(message.contains(SENTINEL), "{message}");
    assert!(elapsed < McpDriver::FIND_WINDOW_DEADLINE, "{elapsed:?}");
    assert_eq!(session.list_windows_calls(), 1);
}

#[test]
fn a_healthy_empty_list_windows_is_polled_until_the_deadline() {
    let mut session = StubSession::spawn(false);
    let deadline = Duration::from_millis(700);
    let started = Instant::now();
    assert_eq!(
        session
            .driver
            .find_window_within(1, "CuaTestHarness AppKit", deadline),
        None
    );
    assert!(started.elapsed() >= deadline, "{:?}", started.elapsed());
    assert!(
        session.list_windows_calls() >= 2,
        "polled {} time(s)",
        session.list_windows_calls()
    );
}

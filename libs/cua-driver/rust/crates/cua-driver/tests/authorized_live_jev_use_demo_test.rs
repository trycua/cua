//! Review-gated visual-only demo skeleton. No live Jev adapter is linked here.

#[cfg(any(target_os = "windows", target_os = "linux"))]
use std::path::{Path, PathBuf};
#[cfg(any(target_os = "windows", target_os = "linux"))]
use std::process::{Command, Stdio};
#[cfg(any(target_os = "windows", target_os = "linux"))]
use std::time::{Duration, Instant};

#[cfg(any(target_os = "windows", target_os = "linux"))]
use cua_driver_testkit::{spawn_in_job, Driver, FixtureJournal, McpDriver};

#[cfg(any(target_os = "windows", target_os = "linux"))]
const FIXTURE_TITLE: &str = "Cua Visual-Only Canvas Fixture";

fn require_callable_live_adapter() -> Result<(), &'static str> {
    Err(
        "authorized demo cannot run: no committed callable live Jev adapter marker; no live API request was made",
    )
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/fixtures/apps/cross-platform/visual-only-canvas/main.py")
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn fixture_command(journal_url: &str) -> Command {
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("py");
        command.arg("-3");
        command
    };
    #[cfg(target_os = "linux")]
    let mut command = Command::new("python3");
    command
        .arg(fixture_path())
        .args(["--journal-url", journal_url])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    command
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn spawn_driver() -> McpDriver {
    McpDriver::spawn_named("authorized-live-jev-use-demo")
        .expect("source-built cua-driver is required for the authorized demo")
}

#[test]
fn orchestration_refuses_to_imply_a_live_adapter() {
    let error = require_callable_live_adapter().expect_err("must fail closed");
    assert!(error.contains("committed callable live Jev adapter marker"));
    assert!(error.contains("no live API request was made"));
}

#[test]
#[cfg(any(target_os = "windows", target_os = "linux"))]
#[ignore = "requires protected-environment assets and a separately reviewed live adapter"]
fn authorized_visual_only_demo() {
    match require_callable_live_adapter() {
        Ok(()) => {}
        Err(error) => panic!("{error}"),
    }

    let journal = FixtureJournal::start();
    let fixture = spawn_in_job(&mut fixture_command(journal.url())).expect("start canvas fixture");
    let pid = i64::from(fixture.id());
    let deadline = Instant::now() + Duration::from_secs(10);
    while journal.snapshot()["ready"].as_bool() != Some(true) {
        assert!(
            Instant::now() < deadline,
            "canvas fixture did not publish its loopback oracle"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    let mut driver = spawn_driver();
    driver.reaper().push(fixture);
    let (window_id, _) = driver
        .find_window(pid, FIXTURE_TITLE)
        .expect("find visual-only fixture window");
    let state = driver.call(
        "get_window_state",
        serde_json::json!({"pid": pid, "window_id": window_id, "capture_mode": "ax"}),
    );
    let ax = state.tree_text();
    for painted_label in ["EMBER", "TIDE", "MOSS", "CHOOSE A SIGNAL"] {
        assert!(
            !ax.contains(painted_label),
            "painted label leaked into the semantic action surface: {painted_label}"
        );
    }
    driver.start_behavior_recording();

    panic!(
        "authorized demo stopped before interaction: reviewed live Jev adapter is not linked; no live API request was made"
    );
}

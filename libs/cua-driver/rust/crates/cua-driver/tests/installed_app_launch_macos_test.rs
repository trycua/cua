//! macOS real-app launch/focus checks.
//!
//! These are Rust ports of the old Python focus-steal parity coverage for
//! built-in macOS apps. The canonical macOS lane runs them in a logged-in,
//! TCC-authorized desktop session.
//!
//! Run:
//!   cargo test -p cua-driver --test installed_app_launch_macos_test -- --ignored --nocapture --test-threads=1

#![cfg(target_os = "macos")]

use std::process::Command;
use std::thread::sleep;
use std::time::{Duration, Instant};

use cua_driver_testkit::e2e::{
    execute_case, recording_evidence, CaseSpec, Delivery, DriverRoute, Observation, OracleKind,
    Scope, Targeting,
};
use cua_driver_testkit::{Driver, McpDriver};

const FINDER_BUNDLE: &str = "com.apple.finder";
const CALCULATOR_BUNDLE: &str = "com.apple.calculator";
const TEXTEDIT_BUNDLE: &str = "com.apple.TextEdit";

fn osascript(script: &str) -> Option<String> {
    let out = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .inspect_err(|e| eprintln!("[launch-focus] osascript spawn failed: {e}"))
        .ok()?;
    if !out.status.success() {
        eprintln!(
            "[launch-focus] osascript failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn frontmost_bundle_id() -> Option<String> {
    osascript(
        r#"tell application "System Events" to bundle identifier of first application process whose frontmost is true"#,
    )
    .filter(|s| !s.is_empty())
}

fn activate_bundle(bundle_id: &str) {
    let _ = osascript(&format!(r#"tell application id "{bundle_id}" to activate"#));
}

fn wait_for_frontmost(bundle_id: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if frontmost_bundle_id().as_deref() == Some(bundle_id) {
            return true;
        }
        sleep(Duration::from_millis(100));
    }
    false
}

fn ensure_finder_frontmost() {
    for _ in 0..3 {
        activate_bundle(FINDER_BUNDLE);
        if wait_for_frontmost(FINDER_BUNDLE, Duration::from_secs(2)) {
            return;
        }
        sleep(Duration::from_millis(500));
    }
    panic!("could not make Finder frontmost for installed-app launch validation");
}

fn kill_app_process(process_name: &str) {
    let _ = Command::new("pkill").args(["-x", process_name]).output();
    sleep(Duration::from_millis(800));
}

fn launch_and_assert_frontmost_unchanged(driver: &mut McpDriver, bundle_id: &str, label: &str) {
    let before = frontmost_bundle_id();
    assert_eq!(
        before.as_deref(),
        Some(FINDER_BUNDLE),
        "{label}: expected Finder frontmost before launch, got {before:?}"
    );

    let launch = driver.call("launch_app", serde_json::json!({ "bundle_id": bundle_id }));
    assert!(
        !launch.is_error(),
        "{label}: launch_app({bundle_id}) failed: {}",
        launch.text()
    );
    assert!(
        launch.structured()["pid"].as_i64().unwrap_or(0) > 0,
        "{label}: launch_app({bundle_id}) returned no pid: {:?}",
        launch.raw
    );

    sleep(Duration::from_millis(1200));
    let after = frontmost_bundle_id();
    assert_eq!(
        after.as_deref(),
        Some(FINDER_BUNDLE),
        "{label}: launch_app({bundle_id}) stole focus; frontmost after launch was {after:?}"
    );
}

fn run_launch_case(cell_id: &str, bundle_id: &str, process_name: &str, label: &str) {
    let case = CaseSpec::delivered(
        cell_id,
        label,
        "appkit",
        "launch_app",
        Targeting::NotApplicable,
        Delivery::Background,
        Scope::Desktop,
        DriverRoute::Composite,
        vec![OracleKind::Focus, OracleKind::Protocol],
    );
    execute_case(case, |evidence| {
        kill_app_process(process_name);
        ensure_finder_frontmost();
        let mut driver = McpDriver::spawn_macos_daemon_proxy_named(cell_id)
            .expect("start installed macOS daemon proxy");
        *evidence = recording_evidence(driver.recording_dir());
        driver.start_behavior_recording();
        ensure_finder_frontmost();
        launch_and_assert_frontmost_unchanged(&mut driver, bundle_id, label);
        Observation::delivered(
            vec![OracleKind::Focus, OracleKind::Protocol],
            Default::default(),
        )
    });
}

#[test]
#[ignore]
fn textedit_launch_preserves_finder_frontmost() {
    run_launch_case(
        "macos-textedit-launch-background",
        TEXTEDIT_BUNDLE,
        "TextEdit",
        "textedit",
    );
}

#[test]
#[ignore]
fn calculator_launch_preserves_finder_frontmost() {
    run_launch_case(
        "macos-calculator-launch-background",
        CALCULATOR_BUNDLE,
        "Calculator",
        "calculator",
    );
}

//! macOS real-app launch/focus checks.
//!
//! These are Rust ports of the old Python focus-steal parity coverage for
//! built-in macOS apps. The canonical macOS lane runs them in a logged-in,
//! TCC-authorized desktop session.
//!
//! Run:
//!   cargo test -p cua-driver-e2e --test installed_app_launch_macos_test -- --ignored --nocapture --test-threads=1

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

fn set_process_visible(process_name: &str, visible: bool) {
    let _ = osascript(&format!(
        r#"tell application "System Events" to set visible of application process "{process_name}" to {visible}"#
    ));
}

fn process_visible(process_name: &str) -> Option<bool> {
    osascript(&format!(
        r#"tell application "System Events" to visible of application process "{process_name}""#
    ))
    .and_then(|value| match value.as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    })
}

/// Make Finder frontmost and keep it there for a settle window. Hosted macOS
/// runners can hand the front to another process just after `activate`
/// succeeded, so one sample is a race: re-activate with a bounded deadline
/// until Finder has held the front for `FRONT_SETTLE` in a row.
fn ensure_finder_frontmost() {
    const FRONT_SETTLE: Duration = Duration::from_millis(1500);
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut stable_since: Option<Instant> = None;
    while Instant::now() < deadline {
        if frontmost_bundle_id().as_deref() == Some(FINDER_BUNDLE) {
            let since = *stable_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= FRONT_SETTLE {
                return;
            }
            sleep(Duration::from_millis(100));
        } else {
            stable_since = None;
            activate_bundle(FINDER_BUNDLE);
            sleep(Duration::from_millis(300));
        }
    }
    panic!("could not make Finder stay frontmost for installed-app launch validation");
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

#[test]
#[ignore]
fn textedit_launch_reopens_hidden_running_app_without_focus_steal() {
    let case = CaseSpec::delivered(
        "macos-textedit-reopen-hidden",
        "reopen a hidden running app",
        "appkit",
        "launch_app",
        Targeting::NotApplicable,
        Delivery::Background,
        Scope::Desktop,
        DriverRoute::Composite,
        vec![OracleKind::Focus, OracleKind::Protocol],
    );
    execute_case(case, |evidence| {
        kill_app_process("TextEdit");
        ensure_finder_frontmost();
        let mut driver = McpDriver::spawn_macos_daemon_proxy_named("macos-textedit-reopen-hidden")
            .expect("start installed macOS daemon proxy");
        *evidence = recording_evidence(driver.recording_dir());
        driver.start_behavior_recording();

        launch_and_assert_frontmost_unchanged(&mut driver, TEXTEDIT_BUNDLE, "initial launch");
        set_process_visible("TextEdit", false);
        assert_eq!(
            process_visible("TextEdit"),
            Some(false),
            "TextEdit should be hidden before the reopen request"
        );
        ensure_finder_frontmost();

        launch_and_assert_frontmost_unchanged(&mut driver, TEXTEDIT_BUNDLE, "reopen");
        sleep(Duration::from_millis(1200));
        assert_eq!(
            process_visible("TextEdit"),
            Some(true),
            "launch_app should make an already-running hidden app visible"
        );
        kill_app_process("TextEdit");

        Observation::delivered(
            vec![OracleKind::Focus, OracleKind::Protocol],
            Default::default(),
        )
    });
}

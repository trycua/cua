//! Exact-window certification for macOS `bring_to_front`.
//!
//! The assertions deliberately do not consume cua-driver's `list_windows` or
//! internal verifier. System Events independently reports the frontmost process
//! and AX focused window, while a direct CoreGraphics query provides the
//! WindowServer front-to-back order.
//!
//! Run with:
//! `cargo test -p cua-driver --test bring_to_front_macos_test -- --ignored --nocapture --test-threads=1`

#![cfg(target_os = "macos")]

use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use core_foundation::{
    array::{CFArray, CFArrayRef},
    base::{CFGetTypeID, CFTypeRef, TCFType},
    boolean::CFBoolean,
    dictionary::CFDictionary,
    number::CFNumber,
    string::CFString,
};
use cua_driver_testkit::{harness_app, Driver, McpDriver};

const MAIN_TITLE: &str = "CuaTestHarness AppKit";
const SECONDARY_TITLE: &str = "CuaTestHarness AppKit Secondary";
const OCCLUDER_TITLE: &str = "CuaTestHarness SwiftUI";
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGWindowListCopyWindowInfo(option: u32, relative_to_window: u32) -> CFArrayRef;
}

#[derive(Debug, Clone)]
struct OracleWindow {
    id: u32,
    pid: u32,
    layer: i32,
}

struct Fixture {
    pid: u32,
    report: tempfile::NamedTempFile,
}

struct Occluder {
    pid: u32,
}

impl Drop for Occluder {
    fn drop(&mut self) {
        let _ = Command::new("kill")
            .args(["-TERM", &self.pid.to_string()])
            .status();
    }
}

fn harness_exe() -> std::path::PathBuf {
    std::env::var("HARNESS_APPKIT_APP")
        .map(std::path::PathBuf::from)
        .ok()
        .filter(|path| path.exists())
        .unwrap_or_else(|| harness_app("harness-appkit", "CuaTestHarness.AppKit.app"))
        .join("Contents/MacOS/CuaTestHarness.AppKit")
}

fn occluder_exe() -> std::path::PathBuf {
    std::env::var("HARNESS_SWIFTUI_APP")
        .map(std::path::PathBuf::from)
        .ok()
        .filter(|path| path.exists())
        .unwrap_or_else(|| harness_app("harness-swiftui", "CuaTestHarness.SwiftUI.app"))
        .join("Contents/MacOS/CuaTestHarness.SwiftUI")
}

fn launch(driver: &mut McpDriver, mode: Option<&str>) -> Fixture {
    let exe = harness_exe();
    assert!(exe.exists(), "required AppKit harness missing at {exe:?}");
    let report = tempfile::NamedTempFile::new().expect("create AppKit window report");
    let mut command = Command::new(exe);
    command.stdout(Stdio::null()).stderr(Stdio::null());
    command.env("CUA_HARNESS_WINDOW_REPORT", report.path());
    if let Some(mode) = mode {
        command.env("CUA_HARNESS_BRING_TO_FRONT_MODE", mode);
    }
    let child = cua_driver_testkit::spawn_in_job(&mut command).expect("launch AppKit harness");
    let pid = child.id();
    driver.reaper().push(child);
    Fixture { pid, report }
}

fn running_executable_pids(exe: &std::path::Path) -> HashSet<u32> {
    let output = Command::new("pgrep")
        .args([
            "-f",
            "-x",
            "--",
            exe.to_str().expect("UTF-8 SwiftUI fixture path"),
        ])
        .output()
        .expect("query SwiftUI fixture processes");
    if !output.status.success() {
        return HashSet::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .collect()
}

fn launch_occluder(_driver: &mut McpDriver) -> Occluder {
    let exe = occluder_exe();
    assert!(
        exe.exists(),
        "required distinct-bundle SwiftUI occluder missing at {exe:?}"
    );
    let app = exe
        .ancestors()
        .nth(3)
        .expect("SwiftUI executable is inside an app bundle");
    let previous = running_executable_pids(&exe);
    let opened = Command::new("open")
        .args(["-n", "-g"])
        .arg(app)
        .status()
        .expect("launch SwiftUI occluder through LaunchServices");
    assert!(opened.success(), "LaunchServices rejected SwiftUI occluder");

    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        if let Some(pid) = running_executable_pids(&exe).difference(&previous).next() {
            return Occluder { pid: *pid };
        }
        assert!(
            Instant::now() < deadline,
            "LaunchServices SwiftUI process did not appear"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn wait_for_ordinary_window(pid: u32) -> u32 {
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        if let Some(window) = cg_windows()
            .into_iter()
            .find(|window| window.pid == pid && window.layer == 0)
        {
            return window.id;
        }
        assert!(
            Instant::now() < deadline,
            "ordinary window for pid {pid} did not appear"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn wait_for_window_ids(fixture: &Fixture, names: &[&str]) -> HashMap<String, u32> {
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        let report = std::fs::read_to_string(fixture.report.path()).unwrap_or_default();
        let ids: HashMap<String, u32> = report
            .lines()
            .filter_map(|line| line.split_once('='))
            .filter_map(|(name, id)| id.parse().ok().map(|id| (name.to_owned(), id)))
            .collect();
        let windows = cg_windows();
        if names.iter().all(|name| {
            ids.get(*name).is_some_and(|id| {
                windows
                    .iter()
                    .any(|window| window.pid == fixture.pid && window.id == *id)
            })
        }) {
            return ids;
        }
        assert!(
            Instant::now() < deadline,
            "windows {names:?} for pid {} did not appear; report={report:?}; observed={windows:?}",
            fixture.pid
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn cg_windows() -> Vec<OracleWindow> {
    // On-screen only | exclude desktop elements. CoreGraphics returns front to back.
    let raw = unsafe { CGWindowListCopyWindowInfo(1 | 16, 0) };
    assert!(!raw.is_null(), "CGWindowListCopyWindowInfo returned null");
    let array: CFArray<CFTypeRef> = unsafe { CFArray::wrap_under_create_rule(raw) };
    let mut windows = Vec::new();
    for item in array.iter() {
        let item = *item;
        if unsafe { CFGetTypeID(item) } != CFDictionary::<*const c_void, *const c_void>::type_id() {
            continue;
        }
        let dictionary: CFDictionary<*const c_void, *const c_void> =
            unsafe { CFDictionary::wrap_under_get_rule(item as _) };
        let number = |key: &str| -> i64 {
            let key = CFString::new(key);
            dictionary
                .find(key.as_concrete_TypeRef() as *const c_void)
                .and_then(|value| unsafe {
                    let value = *value;
                    (CFGetTypeID(value) == CFNumber::type_id())
                        .then(|| CFNumber::wrap_under_get_rule(value as _).to_i64())
                        .flatten()
                })
                .unwrap_or_default()
        };
        let on_screen = {
            let key = CFString::new("kCGWindowIsOnscreen");
            dictionary
                .find(key.as_concrete_TypeRef() as *const c_void)
                .is_some_and(|value| unsafe {
                    let value = *value;
                    CFGetTypeID(value) == CFBoolean::type_id()
                        && bool::from(CFBoolean::wrap_under_get_rule(value as _))
                })
        };
        if on_screen {
            windows.push(OracleWindow {
                id: number("kCGWindowNumber") as u32,
                pid: number("kCGWindowOwnerPID") as u32,
                layer: number("kCGWindowLayer") as i32,
            });
        }
    }
    windows
}

fn frontmost_pid() -> u32 {
    let output = Command::new("osascript")
        .args([
            "-e",
            "tell application \"System Events\" to unix id of first process whose frontmost is true",
        ])
        .output()
        .expect("query frontmost process");
    assert!(
        output.status.success(),
        "frontmost oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .expect("frontmost oracle pid")
}

fn focused_window_title(pid: u32) -> String {
    let script = format!(
        "tell application \"System Events\" to tell (first process whose unix id is {pid}) to get name of value of attribute \"AXFocusedWindow\""
    );
    let output = Command::new("osascript")
        .args(["-e", &script])
        .output()
        .expect("query AX focused window");
    assert!(
        output.status.success(),
        "focused-window oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn activate_and_raise(pid: u32, title: &str) {
    let script = format!(
        "tell application \"System Events\" to tell (first process whose unix id is {pid})\nset frontmost to true\nperform action \"AXRaise\" of window \"{title}\"\nend tell"
    );
    let output = Command::new("osascript")
        .args(["-e", &script])
        .output()
        .expect("activate fixture window");
    assert!(
        output.status.success(),
        "fixture activation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::thread::sleep(Duration::from_millis(250));
}

fn layer_zero_front() -> OracleWindow {
    cg_windows()
        .into_iter()
        .find(|window| window.layer == 0)
        .expect("at least one ordinary on-screen window")
}

#[test]
#[ignore]
fn exact_secondary_window_is_verified_and_prior_app_can_recover_focus() {
    let mut driver = McpDriver::spawn_macos_daemon_proxy_named("macos-bring-to-front-exact")
        .expect("start installed macOS daemon proxy");
    let target = launch(&mut driver, Some("two-windows"));
    let target_windows = wait_for_window_ids(&target, &["main", "secondary"]);
    let secondary = target_windows["secondary"];
    // Use a distinct bundle identity for the prior app. Raw-launching a second
    // copy of the AppKit fixture lets AX and WindowServer select that process's
    // window while NSWorkspace continues to report the other same-bundle
    // instance as frontmost, which is not representative app recovery.
    let occluder = launch_occluder(&mut driver);
    let occluder_main = wait_for_ordinary_window(occluder.pid);

    activate_and_raise(target.pid, MAIN_TITLE);
    activate_and_raise(occluder.pid, OCCLUDER_TITLE);
    assert_ne!(
        layer_zero_front().id,
        secondary,
        "precondition: secondary already frontmost"
    );

    let result = driver.call(
        "bring_to_front",
        serde_json::json!({"pid": target.pid, "window_id": secondary}),
    );
    assert!(
        !result.is_error(),
        "exact bring_to_front failed: {}; raw={}",
        result.text(),
        result.raw
    );
    assert_eq!(result.structured()["activated"], true);
    assert_eq!(result.structured()["request_accepted"], true);
    assert_eq!(result.structured()["process_activated"], true);
    assert_eq!(result.structured()["exact_window_effect"]["verified"], true);
    assert_eq!(
        frontmost_pid(),
        target.pid,
        "independent frontmost-process oracle"
    );
    assert_eq!(
        focused_window_title(target.pid),
        SECONDARY_TITLE,
        "independent AX key-window oracle"
    );
    assert_eq!(
        layer_zero_front().id,
        secondary,
        "independent CGWindow z-order oracle"
    );

    assert_ne!(focused_window_title(target.pid), MAIN_TITLE);

    let recovered = driver.call(
        "bring_to_front",
        serde_json::json!({"pid": occluder.pid, "window_id": occluder_main}),
    );
    assert!(
        !recovered.is_error(),
        "prior app recovery failed: {}",
        recovered.text()
    );
    assert_eq!(frontmost_pid(), occluder.pid);
    assert_eq!(layer_zero_front().id, occluder_main);
}

#[test]
#[ignore]
fn modal_sheet_never_yields_false_parent_success() {
    let mut driver = McpDriver::spawn_macos_daemon_proxy_named("macos-bring-to-front-sheet")
        .expect("start installed macOS daemon proxy");
    let target = launch(&mut driver, Some("sheet"));
    let windows = wait_for_window_ids(&target, &["main", "secondary", "sheet"]);
    let parent = windows["main"];
    let result = driver.call(
        "bring_to_front",
        serde_json::json!({"pid": target.pid, "window_id": parent}),
    );
    assert!(
        result.is_error(),
        "modal parent falsely reported success: {}",
        result.raw
    );
    assert_eq!(result.structured()["activated"], false);
    assert_ne!(focused_window_title(target.pid), MAIN_TITLE);
}

#[test]
#[ignore]
fn floating_panel_is_typed_nonordinary_without_focus_theft() {
    let mut driver = McpDriver::spawn_macos_daemon_proxy_named("macos-bring-to-front-floating")
        .expect("start installed macOS daemon proxy");
    let target = launch(&mut driver, Some("floating"));
    let windows = wait_for_window_ids(&target, &["main", "secondary", "floating"]);
    let floating_id = windows["floating"];
    let floating = cg_windows()
        .into_iter()
        .find(|window| window.pid == target.pid && window.id == floating_id)
        .expect("floating panel in CoreGraphics oracle");
    assert_ne!(
        floating.layer, 0,
        "fixture floating panel unexpectedly ordinary"
    );
    let occluder = launch(&mut driver, None);
    wait_for_window_ids(&occluder, &["main"]);
    activate_and_raise(occluder.pid, MAIN_TITLE);

    let result = driver.call(
        "bring_to_front",
        serde_json::json!({"pid": target.pid, "window_id": floating.id}),
    );
    assert!(
        result.is_error(),
        "floating panel falsely reported success: {}",
        result.raw
    );
    assert_eq!(
        result.structured()["code"],
        "bring_to_front_window_not_ordinary"
    );
    assert_eq!(result.structured()["request_accepted"], false);
    assert_eq!(
        frontmost_pid(),
        occluder.pid,
        "validation stole focus before refusing"
    );
}

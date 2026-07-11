//! Integration test against the CuaTestHarness.Gtk3 (PyGObject / GTK3) app —
//! the Linux peer of `harness_appkit_test` / `harness_swiftui_test` (macOS) and
//! `harness_wpf_test` / `harness_winui3_test` (Windows). Brings Linux to parity
//! with the other platforms' controlled-harness coverage (alongside the real-app
//! Nix GUI scenarios).
//!
//! ## The Linux id contract: NAME, not id
//! cua-driver's Linux `get_window_state` renders each element as
//! `[idx] <role> "<name>"` from AT-SPI — there is NO `id=` field like Windows
//! (UIA AutomationId) or macOS (AX identifier). So the harness sets each
//! actionable control's AT-SPI **accessible name** to the scenario aid
//! (`btn-increment`, …), and this test matches on name via
//! `ax::element_index_containing` (substring), not `ax::has_id`.
//!
//! Requires a real Linux desktop with AT-SPI running + a display (Xwayland is
//! fine — the launcher forces `GDK_BACKEND=x11`), GTK3 + PyGObject, and the app
//! built via `tests/fixtures/build/linux.sh`. `#[ignore]`; run explicitly:
//!   cargo test -p cua-driver --test harness_gtk3_test -- --ignored --nocapture --test-threads=1

#![cfg(target_os = "linux")]

use std::process::{Command, Stdio};
use std::time::Duration;

use cua_driver_testkit::e2e::{
    execute_case, native_background_case, native_readonly_case, recording_evidence, DriverRoute,
    Evidence, Observation, OracleKind, Targeting,
};
use cua_driver_testkit::observer::TargetWindow;
use cua_driver_testkit::sentinel::run_with_background_oracles;
use cua_driver_testkit::{ax, harness_app, Driver, McpDriver};

fn harness_exe() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("HARNESS_GTK3_EXE") {
        let pb = std::path::PathBuf::from(p);
        if pb.exists() {
            return pb;
        }
    }
    harness_app("harness-gtk3", "CuaTestHarness.Gtk3")
}

/// Launch the GTK3 harness, tying its lifetime to the driver's reaper, and
/// return (pid, main window_id).
fn launch(driver: &mut McpDriver) -> (u32, u64) {
    let exe = harness_exe();
    assert!(exe.exists(), "required GTK3 harness is missing: {exe:?}");
    driver
        .reaper()
        .spawn(
            Command::new(&exe)
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit()),
        )
        .unwrap_or_else(|error| panic!("launch GTK3 harness {exe:?}: {error}"));
    // GTK app cold-start + window map + AT-SPI registration.
    std::thread::sleep(Duration::from_millis(1500));

    // Resolve the harness window by title. find_window needs the pid; the
    // launcher execs python3 in-place so the spawned pid IS the GTK process.
    // We don't have that pid directly here (reaper owns the child), so discover
    // by title across list_windows.
    let deadline = std::time::Instant::now() + Duration::from_secs(12);
    while std::time::Instant::now() < deadline {
        let r = driver.call("list_windows", serde_json::json!({}));
        if let Some(wins) = r.structured()["windows"].as_array() {
            for w in wins {
                if w["title"]
                    .as_str()
                    .unwrap_or("")
                    .contains("CuaTestHarness GTK3")
                {
                    let pid = w["pid"].as_u64().unwrap_or(0) as u32;
                    let wid = w["window_id"].as_u64().unwrap_or(0);
                    if pid != 0 && wid != 0 {
                        driver.reaper().track_pid(pid);
                        return (pid, wid);
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(400));
    }
    panic!("required GTK3 harness window never appeared");
}

fn snapshot(driver: &mut McpDriver, pid: u32, wid: u64) -> String {
    driver
        .call(
            "get_window_state",
            serde_json::json!({ "pid": pid as i64, "window_id": wid }),
        )
        .text()
        .to_string()
}

fn ax_empty(text: &str) -> bool {
    ax::looks_empty(text)
}

fn run_case(
    case: cua_driver_testkit::e2e::CaseSpec,
    test: impl FnOnce(u32, u64, &mut McpDriver) -> Observation,
) {
    let cell_id = case.cell_id.clone();
    let delivery = case.delivery;
    execute_case(case, |evidence| {
        let mut driver = McpDriver::spawn_named(&cell_id).expect("start source-built Linux driver");
        *evidence = recording_evidence(driver.recording_dir());
        let (pid, wid) = launch(&mut driver);
        if delivery != cua_driver_testkit::e2e::Delivery::Background {
            driver.start_behavior_recording();
        }
        test(pid, wid, &mut driver)
    });
}

fn run_background_case(action: &str, test: impl FnOnce(u32, u64, &mut McpDriver)) {
    run_case(
        native_background_case("gtk3", action, Targeting::Ax, DriverRoute::LinuxAtSpiAction),
        |pid, wid, driver| {
            let (_, passed) = run_with_background_oracles(
                driver,
                TargetWindow {
                    pid,
                    native_id: wid,
                },
                |driver| test(pid, wid, driver),
            )
            .unwrap_or_else(|error| panic!("background desktop contract failed: {error}"));
            Observation::delivered_with_fixture_state(passed)
        },
    );
}

#[test]
#[ignore]
fn harness_gtk3_smoke() {
    run_case(
        native_readonly_case(
            "gtk3",
            "ax_tree",
            Targeting::Ax,
            DriverRoute::AxRead,
            vec![OracleKind::AxState],
        ),
        |pid, wid, driver| {
            println!("gtk3 harness pid={pid} wid={wid}");
            let text = snapshot(driver, pid, wid);
            assert!(!ax_empty(&text), "required GTK3 AT-SPI tree is empty");

            // Actionable controls expose their aid as the AT-SPI accessible name.
            for name in [
                "btn-increment",
                "btn-reset",
                "txt-input",
                "btn-open-popover",
                "btn-exit",
            ] {
                assert!(
                    text.contains(name),
                    "missing control named {name:?} in GTK3 AT-SPI tree"
                );
            }
            // Marker label + counter label carry their text as the accessible name.
            assert!(
                text.contains("HARNESS_TEXT_MARKER_v1"),
                "text_body marker not in tree"
            );
            assert!(
                text.contains("counter=0"),
                "initial counter label not in tree"
            );
            assert!(
                text.contains("popover_open=False"),
                "initial popover state not in tree"
            );

            Observation::delivered(vec![OracleKind::AxState], Evidence::default())
        },
    );
}

#[test]
#[ignore]
fn harness_gtk3_counter_click() {
    run_background_case("left_click", |pid, wid, driver| {
        let pre = snapshot(driver, pid, wid);
        assert!(!ax_empty(&pre), "required GTK3 AT-SPI tree is empty");
        let idx = ax::element_index_containing(&pre, "btn-increment")
            .expect("btn-increment not found in GTK3 tree");
        let click = driver.call(
            "click",
            serde_json::json!({
                "pid": pid as i64, "window_id": wid, "element_index": idx,
                "delivery_mode": "background"
            }),
        );
        assert!(
            !click.is_error(),
            "GTK3 counter click failed: {}",
            click.text()
        );
        println!("click [{idx}] btn-increment: {}", click.text());
        std::thread::sleep(Duration::from_millis(400));

        let post = snapshot(driver, pid, wid);
        assert!(
            post.contains("counter=1"),
            "counter did not advance after clicking btn-increment. counter lines: {}",
            post.lines()
                .filter(|l| l.contains("counter="))
                .collect::<Vec<_>>()
                .join(" / ")
        );
    });
}

#[test]
#[ignore]
fn harness_gtk3_popover() {
    run_background_case("popover_open", |pid, wid, driver| {
        let pre = snapshot(driver, pid, wid);
        assert!(!ax_empty(&pre), "required GTK3 AT-SPI tree is empty");
        assert!(
            pre.contains("popover_open=False"),
            "popover state not initially closed"
        );

        let idx = ax::element_index_containing(&pre, "btn-open-popover")
            .expect("btn-open-popover not found in GTK3 tree");
        let open = driver.call(
            "click",
            serde_json::json!({
                "pid": pid as i64, "window_id": wid, "element_index": idx,
                "delivery_mode": "background"
            }),
        );
        assert!(
            !open.is_error(),
            "GTK3 popover open failed: {}",
            open.text()
        );
        std::thread::sleep(Duration::from_millis(500));

        // GtkPopover may surface as a separate top-level; check the main window
        // first, then any new window of this pid.
        let post = snapshot(driver, pid, wid);
        assert!(
            post.contains("popover_open=True"),
            "popover state did not flip open after click. Popover lines: {}",
            post.lines()
                .filter(|l| l.contains("popover_open="))
                .collect::<Vec<_>>()
                .join(" / ")
        );
        let mut found = post.contains("POPOVER_MARKER_v1");
        if !found {
            let r = driver.call("list_windows", serde_json::json!({}));
            if let Some(wins) = r.structured()["windows"].as_array() {
                for w in wins {
                    if w["pid"].as_u64() == Some(pid as u64) {
                        if let Some(other) = w["window_id"].as_u64() {
                            if other != wid
                                && snapshot(driver, pid, other).contains("POPOVER_MARKER_v1")
                            {
                                found = true;
                                break;
                            }
                        }
                    }
                }
            }
        }
        assert!(
            found,
            "popover body marker POPOVER_MARKER_v1 not found after open"
        );
    });
}

//! Evidence-bearing native GTK3 harness catalog for Linux.
//!
//! GTK exposes accessibility through ATK -> AT-SPI. The Linux snapshot has no
//! AutomationId/AXIdentifier field, so actionable controls publish their
//! scenario id as their accessible name and this test addresses them by name.
//!
//! Requires a Linux desktop, AT-SPI, GTK3, PyGObject, and the fixture built by
//! `tests/fixtures/build/linux.sh`. The canonical lane runs this target with:
//!   cargo test -p cua-driver --test harness_gtk3_test -- --ignored --nocapture --test-threads=1

#![cfg(target_os = "linux")]

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cua_driver_testkit::e2e::{
    execute_case, native_background_case, native_foreground_case, native_readonly_case,
    recording_evidence, CaseSpec, Delivery, DisplayServer, DriverRoute, Evidence, Observation,
    OracleKind, RefusalCode, Targeting,
};
use cua_driver_testkit::observer::TargetWindow;
use cua_driver_testkit::sentinel::run_with_background_oracles;
use cua_driver_testkit::{ax, harness_app, Driver, McpDriver, ToolResponse};

fn harness_exe() -> std::path::PathBuf {
    if let Ok(path) = std::env::var("HARNESS_GTK3_EXE") {
        let path = std::path::PathBuf::from(path);
        if path.exists() {
            return path;
        }
    }
    harness_app("harness-gtk3", "CuaTestHarness.Gtk3")
}

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

    let deadline = Instant::now() + Duration::from_secs(12);
    while Instant::now() < deadline {
        let response = driver.call("list_windows", serde_json::json!({}));
        if let Some(windows) = response.structured()["windows"].as_array() {
            for window in windows {
                if window["title"]
                    .as_str()
                    .unwrap_or("")
                    .contains("CuaTestHarness GTK3")
                {
                    let pid = window["pid"].as_u64().unwrap_or(0) as u32;
                    let window_id = window["window_id"].as_u64().unwrap_or(0);
                    if pid != 0 && window_id != 0 {
                        driver.reaper().track_pid(pid);
                        std::thread::sleep(Duration::from_millis(500));
                        return (pid, window_id);
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(400));
    }
    panic!("required GTK3 harness window never appeared");
}

fn snapshot(driver: &mut McpDriver, pid: u32, window_id: u64) -> ToolResponse {
    driver.call(
        "get_window_state",
        serde_json::json!({ "pid": pid as i64, "window_id": window_id }),
    )
}

fn element_index(state: &ToolResponse, name: &str) -> u64 {
    ax::element_index_containing(state.tree_text(), name).unwrap_or_else(|| {
        panic!(
            "{name:?} not found in GTK3 AT-SPI tree:\n{}",
            state.tree_text()
        )
    })
}

fn element_rect(
    driver: &mut McpDriver,
    pid: u32,
    window_id: u64,
    state: &ToolResponse,
    name: &str,
) -> (f64, f64, f64, f64) {
    let index = element_index(state, name);
    let elements = state.structured()["elements"]
        .as_array()
        .expect("PX targeting requires structured GTK3 elements");
    let target = elements
        .iter()
        .find(|element| element["element_index"].as_u64() == Some(index))
        .and_then(|element| element["frame"].as_object())
        .unwrap_or_else(|| panic!("GTK3 element [{index}] {name:?} has no frame"));
    let windows = driver.call("list_windows", serde_json::json!({ "pid": pid as i64 }));
    let window = windows.structured()["windows"]
        .as_array()
        .and_then(|windows| {
            windows
                .iter()
                .find(|window| window["window_id"].as_u64() == Some(window_id))
        })
        .unwrap_or_else(|| panic!("GTK3 window {window_id} disappeared before PX targeting"));

    let window_width = window["width"].as_f64().unwrap_or(0.0);
    let window_height = window["height"].as_f64().unwrap_or(0.0);
    let screenshot_width = state.structured()["screenshot_width"]
        .as_f64()
        .expect("PX targeting requires screenshot_width");
    let screenshot_height = state.structured()["screenshot_height"]
        .as_f64()
        .expect("PX targeting requires screenshot_height");
    assert!(
        window_width > 0.0 && window_height > 0.0,
        "GTK3 top-level frame has invalid geometry: {window:?}"
    );
    let scale_x = screenshot_width / window_width;
    let scale_y = screenshot_height / window_height;
    let x = (target["x"].as_f64().unwrap_or(0.0) - window["x"].as_f64().unwrap_or(0.0)) * scale_x;
    let y = (target["y"].as_f64().unwrap_or(0.0) - window["y"].as_f64().unwrap_or(0.0)) * scale_y;
    let width = target["w"].as_f64().unwrap_or(0.0) * scale_x;
    let height = target["h"].as_f64().unwrap_or(0.0) * scale_y;
    assert!(
        width > 0.0
            && height > 0.0
            && x + width / 2.0 >= 0.0
            && y + height / 2.0 >= 0.0
            && x + width / 2.0 < screenshot_width
            && y + height / 2.0 < screenshot_height,
        "GTK3 PX target {name:?} is outside the capture: rect=({x},{y},{width},{height}) capture=({screenshot_width},{screenshot_height})"
    );
    (x, y, width, height)
}

fn wait_for_state(driver: &mut McpDriver, pid: u32, window_id: u64, expected: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(4);
    let mut last = String::new();
    while Instant::now() < deadline {
        last = snapshot(driver, pid, window_id).tree_text().to_owned();
        if last.contains(expected) {
            return last;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let relevant = last
        .lines()
        .filter(|line| line.contains('=') || line.contains("MARKER"))
        .collect::<Vec<_>>()
        .join(" / ");
    panic!("fixture state never contained {expected:?}; final state: {relevant}");
}

fn state_number(text: &str, key: &str) -> Option<u64> {
    let start = text.find(key)? + key.len();
    let digits = text[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>();
    digits.parse().ok()
}

fn wait_for_positive_state(driver: &mut McpDriver, pid: u32, window_id: u64, key: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(4);
    let mut last = String::new();
    while Instant::now() < deadline {
        last = snapshot(driver, pid, window_id).tree_text().to_owned();
        if state_number(&last, key).is_some_and(|value| value > 0) {
            return last;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    panic!("fixture state {key:?} never became positive; final tree:\n{last}");
}

fn assert_popover_marker(driver: &mut McpDriver, pid: u32, main_window_id: u64) {
    let deadline = Instant::now() + Duration::from_secs(4);
    while Instant::now() < deadline {
        if snapshot(driver, pid, main_window_id)
            .tree_text()
            .contains("POPOVER_MARKER_v1")
        {
            return;
        }
        let windows = driver.call("list_windows", serde_json::json!({}));
        if let Some(windows) = windows.structured()["windows"].as_array() {
            for window in windows {
                let window_id = window["window_id"].as_u64().unwrap_or(0);
                if window["pid"].as_u64() == Some(pid as u64)
                    && window_id != 0
                    && window_id != main_window_id
                    && snapshot(driver, pid, window_id)
                        .tree_text()
                        .contains("POPOVER_MARKER_v1")
                {
                    return;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    panic!("GTK3 popover opened but POPOVER_MARKER_v1 was not exposed");
}

fn run_case(case: CaseSpec, test: impl FnOnce(u32, u64, &mut McpDriver) -> Observation) {
    let cell_id = case.cell_id.clone();
    let delivery = case.delivery;
    execute_case(case, |evidence| {
        let mut driver = McpDriver::spawn_named(&cell_id).expect("start source-built Linux driver");
        *evidence = recording_evidence(driver.recording_dir());
        let (pid, window_id) = launch(&mut driver);
        if delivery != Delivery::Background {
            driver.start_behavior_recording();
        }
        test(pid, window_id, &mut driver)
    });
}

#[derive(Clone, Copy, Debug)]
enum Operation {
    AxClick {
        target: &'static str,
        expected: &'static str,
    },
    AxTypeText {
        target: &'static str,
        text: &'static str,
        expected: &'static str,
    },
    AxSetValue {
        target: &'static str,
        value: &'static str,
        expected: &'static str,
    },
    PxClick {
        target: &'static str,
        button: &'static str,
        count: u8,
        expected: &'static str,
    },
    PxTypeText {
        target: &'static str,
        text: &'static str,
        expected: &'static str,
    },
    PressKey {
        key: &'static str,
        expected: &'static str,
    },
    Hotkey {
        expected: &'static str,
    },
    Scroll {
        target: &'static str,
        pixel: bool,
        state_key: &'static str,
    },
    Drag {
        target: &'static str,
        state_key: &'static str,
    },
    Popover {
        target: &'static str,
        expected: &'static str,
    },
}

#[derive(Clone, Copy, Debug)]
struct CatalogRow {
    action: &'static str,
    targeting: Targeting,
    delivery: Delivery,
    route: DriverRoute,
    operation: Operation,
}

fn assert_refusal_without_mutation(
    response: &ToolResponse,
    before: &str,
    pid: u32,
    window_id: u64,
    driver: &mut McpDriver,
) {
    assert!(
        response.is_error(),
        "expected background_unavailable refusal"
    );
    assert_eq!(
        response.structured()["code"].as_str(),
        Some("background_unavailable"),
        "unexpected refusal: {}",
        response.text()
    );
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        snapshot(driver, pid, window_id).tree_text(),
        before,
        "refused GTK3 action mutated fixture state"
    );
}

fn invoke_operation(
    row: CatalogRow,
    pid: u32,
    window_id: u64,
    driver: &mut McpDriver,
    expect_refusal: bool,
) -> bool {
    let mode = match row.delivery {
        Delivery::Background => "background",
        Delivery::Foreground => "foreground",
        Delivery::NotApplicable => unreachable!("catalog operations require delivery"),
    };
    let pre = snapshot(driver, pid, window_id);
    assert!(
        !ax::looks_empty(pre.tree_text()),
        "required GTK3 AT-SPI tree is empty"
    );

    let (response, expected) = match row.operation {
        Operation::AxClick { target, expected } => {
            let index = element_index(&pre, target);
            (
                driver.call(
                    "click",
                    serde_json::json!({
                        "pid": pid as i64, "window_id": window_id,
                        "element_index": index, "delivery_mode": mode
                    }),
                ),
                expected,
            )
        }
        Operation::AxTypeText {
            target,
            text,
            expected,
        } => {
            let index = element_index(&pre, target);
            (
                driver.call(
                    "type_text",
                    serde_json::json!({
                        "pid": pid as i64, "window_id": window_id,
                        "element_index": index, "text": text, "delivery_mode": mode
                    }),
                ),
                expected,
            )
        }
        Operation::AxSetValue {
            target,
            value,
            expected,
        } => {
            let index = element_index(&pre, target);
            (
                driver.call(
                    "set_value",
                    serde_json::json!({
                        "pid": pid as i64, "window_id": window_id,
                        "element_index": index, "value": value
                    }),
                ),
                expected,
            )
        }
        Operation::PxClick {
            target,
            button,
            count,
            expected,
        } => {
            let (x, y, width, height) = element_rect(driver, pid, window_id, &pre, target);
            let tool = if count == 2 {
                "double_click"
            } else if button == "right" {
                "right_click"
            } else {
                "click"
            };
            let mut args = serde_json::json!({
                "pid": pid as i64, "window_id": window_id,
                "x": x + width / 2.0, "y": y + height / 2.0,
                "delivery_mode": mode
            });
            if tool == "click" {
                args["button"] = serde_json::json!(button);
                args["count"] = serde_json::json!(count);
            }
            (driver.call(tool, args), expected)
        }
        Operation::PxTypeText {
            target,
            text,
            expected,
        } => {
            let (x, y, width, height) = element_rect(driver, pid, window_id, &pre, target);
            (
                driver.call(
                    "type_text",
                    serde_json::json!({
                        "pid": pid as i64, "window_id": window_id,
                        "x": x + width / 2.0, "y": y + height / 2.0,
                        "text": text, "delivery_mode": mode
                    }),
                ),
                expected,
            )
        }
        Operation::PressKey { key, expected } => (
            driver.call(
                "press_key",
                serde_json::json!({
                    "pid": pid as i64, "window_id": window_id,
                    "key": key, "delivery_mode": mode
                }),
            ),
            expected,
        ),
        Operation::Hotkey { expected } => (
            driver.call(
                "hotkey",
                serde_json::json!({
                    "pid": pid as i64, "window_id": window_id,
                    "keys": ["ctrl", "shift", "k"], "delivery_mode": mode
                }),
            ),
            expected,
        ),
        Operation::Scroll {
            target,
            pixel,
            state_key,
        } => {
            let mut args = serde_json::json!({
                "pid": pid as i64, "window_id": window_id,
                "direction": "down", "amount": 6, "delivery_mode": mode
            });
            if pixel {
                let (x, y, width, height) = element_rect(driver, pid, window_id, &pre, target);
                args["x"] = serde_json::json!(x + width / 2.0);
                args["y"] = serde_json::json!(y + height / 2.0);
            } else {
                args["element_index"] = serde_json::json!(element_index(&pre, target));
            }
            let response = driver.call("scroll", args);
            if expect_refusal {
                assert_refusal_without_mutation(&response, pre.tree_text(), pid, window_id, driver);
                return true;
            }
            assert!(
                !response.is_error(),
                "GTK3 scroll failed: {}",
                response.text()
            );
            wait_for_positive_state(driver, pid, window_id, state_key);
            return false;
        }
        Operation::Drag { target, state_key } => {
            let (x, y, width, height) = element_rect(driver, pid, window_id, &pre, target);
            let response = driver.call(
                "drag",
                serde_json::json!({
                    "pid": pid as i64, "window_id": window_id,
                    "from_x": x + width * 0.05, "from_y": y + height / 2.0,
                    "to_x": x + width * 0.90, "to_y": y + height / 2.0,
                    "duration_ms": 500, "steps": 30, "delivery_mode": mode
                }),
            );
            if expect_refusal {
                assert_refusal_without_mutation(&response, pre.tree_text(), pid, window_id, driver);
                return true;
            }
            assert!(
                !response.is_error(),
                "GTK3 drag failed: {}; raw={}",
                response.text(),
                response.raw
            );
            wait_for_positive_state(driver, pid, window_id, state_key);
            return false;
        }
        Operation::Popover { target, expected } => {
            let index = element_index(&pre, target);
            let response = driver.call(
                "click",
                serde_json::json!({
                    "pid": pid as i64, "window_id": window_id,
                    "element_index": index, "delivery_mode": mode
                }),
            );
            assert!(
                !response.is_error(),
                "GTK3 popover click failed: {}",
                response.text()
            );
            wait_for_state(driver, pid, window_id, expected);
            assert_popover_marker(driver, pid, window_id);
            return false;
        }
    };

    if expect_refusal {
        assert_refusal_without_mutation(&response, pre.tree_text(), pid, window_id, driver);
        return true;
    }

    assert!(
        !response.is_error(),
        "GTK3 {} {:?}/{:?} failed: {}",
        row.action,
        row.targeting,
        row.delivery,
        response.text()
    );
    wait_for_state(driver, pid, window_id, expected);
    false
}

fn row_expects_refusal(row: CatalogRow) -> bool {
    if row.delivery != Delivery::Background {
        return false;
    }
    let inject_mode = std::env::var_os("CUA_INJECT_SOCKET").is_some();
    if !inject_mode
        && matches!(
            row.operation,
            Operation::PressKey { .. } | Operation::Hotkey { .. }
        )
    {
        return true;
    }
    let focus_bound_pointer = matches!(
        row.operation,
        Operation::PxClick {
            button: "right",
            ..
        } | Operation::PxClick { count: 2, .. }
            | Operation::Scroll { pixel: true, .. }
            | Operation::Drag { .. }
    );
    if DisplayServer::current() == DisplayServer::X11
        && (focus_bound_pointer || matches!(row.operation, Operation::PxTypeText { .. }))
        && !platform_linux::input::real_pointer_input_available()
    {
        return true;
    }
    DisplayServer::current() == DisplayServer::Wayland
        && !inject_mode
        && (focus_bound_pointer || matches!(row.operation, Operation::PxTypeText { .. }))
}

fn run_catalog_row(row: CatalogRow) {
    let expect_refusal = row_expects_refusal(row);
    let route = if DisplayServer::current() == DisplayServer::Wayland
        && (row.targeting == Targeting::Px
            || matches!(
                row.operation,
                Operation::PressKey { .. } | Operation::Hotkey { .. }
            )) {
        if std::env::var_os("CUA_INJECT_SOCKET").is_some() {
            DriverRoute::LinuxCuaCompositorInject
        } else {
            DriverRoute::LinuxWaylandVirtualPointer
        }
    } else {
        row.route
    };
    let case = match row.delivery {
        Delivery::Background => native_background_case("gtk3", row.action, row.targeting, route),
        Delivery::Foreground => native_foreground_case("gtk3", row.action, row.targeting, route),
        Delivery::NotApplicable => unreachable!("catalog operations require delivery"),
    };
    let case = if expect_refusal {
        case.expecting_refusal(vec![RefusalCode::BackgroundUnavailable])
    } else {
        case
    };
    run_case(case, |pid, window_id, driver| match row.delivery {
        Delivery::Background => {
            let mut refused = false;
            let (_, passed) = run_with_background_oracles(
                driver,
                TargetWindow {
                    pid,
                    native_id: window_id,
                },
                |driver| {
                    refused = invoke_operation(row, pid, window_id, driver, expect_refusal);
                },
            )
            .unwrap_or_else(|error| panic!("background desktop contract failed: {error}"));
            if refused {
                let mut passed = passed;
                passed.push(OracleKind::FixtureState);
                Observation::refused(
                    RefusalCode::BackgroundUnavailable,
                    passed,
                    "GTK3 focus-bound background input was refused",
                    Evidence::default(),
                )
            } else {
                Observation::delivered_with_fixture_state(passed)
            }
        }
        Delivery::Foreground => {
            assert!(!invoke_operation(row, pid, window_id, driver, false));
            Observation::delivered_with_fixture_state(Vec::new())
        }
        Delivery::NotApplicable => unreachable!(),
    });
}

#[test]
#[ignore]
fn harness_gtk3_ax_tree() {
    run_case(
        native_readonly_case(
            "gtk3",
            "ax_tree",
            Targeting::Ax,
            DriverRoute::AxRead,
            vec![OracleKind::AxState],
        ),
        |pid, window_id, driver| {
            let state = snapshot(driver, pid, window_id);
            let text = state.tree_text();
            assert!(!ax::looks_empty(text), "required GTK3 AT-SPI tree is empty");
            for name in [
                "btn-increment",
                "txt-input",
                "btn-clicktarget",
                "sld-value",
                "chk-agree",
                "scroll-tall",
                "btn-open-popover",
                "btn-exit",
            ] {
                assert!(text.contains(name), "missing GTK3 control named {name:?}");
            }
            for state in [
                "HARNESS_TEXT_MARKER_v1",
                "counter=0",
                "mirror=",
                "last_action=none",
                "last_key=none",
                "last_hotkey=none",
                "slider_value=0",
                "agreed=False",
                "scroll_offset=0",
                "popover_open=False",
            ] {
                assert!(text.contains(state), "missing GTK3 fixture state {state:?}");
            }
            Observation::delivered(vec![OracleKind::AxState], Evidence::default())
        },
    );
}

macro_rules! catalog_test {
    ($name:ident, $row:expr) => {
        #[test]
        #[ignore]
        fn $name() {
            run_catalog_row($row);
        }
    };
}

catalog_test!(
    harness_gtk3_left_click_ax_background,
    CatalogRow {
        action: "left_click",
        targeting: Targeting::Ax,
        delivery: Delivery::Background,
        route: DriverRoute::LinuxAtSpiAction,
        operation: Operation::AxClick {
            target: "btn-increment",
            expected: "counter=1"
        },
    }
);
catalog_test!(
    harness_gtk3_left_click_ax_foreground,
    CatalogRow {
        action: "left_click",
        targeting: Targeting::Ax,
        delivery: Delivery::Foreground,
        route: DriverRoute::LinuxAtSpiAction,
        operation: Operation::AxClick {
            target: "btn-increment",
            expected: "counter=1"
        },
    }
);
catalog_test!(
    harness_gtk3_left_click_px_background,
    CatalogRow {
        action: "left_click",
        targeting: Targeting::Px,
        delivery: Delivery::Background,
        route: DriverRoute::LinuxXSendEvent,
        operation: Operation::PxClick {
            target: "btn-clicktarget",
            button: "left",
            count: 1,
            expected: "last_action=click  clicks=1"
        },
    }
);
catalog_test!(
    harness_gtk3_left_click_px_foreground,
    CatalogRow {
        action: "left_click",
        targeting: Targeting::Px,
        delivery: Delivery::Foreground,
        route: DriverRoute::LinuxXTest,
        operation: Operation::PxClick {
            target: "btn-clicktarget",
            button: "left",
            count: 1,
            expected: "last_action=click  clicks=1"
        },
    }
);
catalog_test!(
    harness_gtk3_right_click_px_background,
    CatalogRow {
        action: "right_click",
        targeting: Targeting::Px,
        delivery: Delivery::Background,
        route: DriverRoute::LinuxXSendEvent,
        operation: Operation::PxClick {
            target: "btn-clicktarget",
            button: "right",
            count: 1,
            expected: "last_action=right_click  clicks=0"
        },
    }
);
catalog_test!(
    harness_gtk3_right_click_px_foreground,
    CatalogRow {
        action: "right_click",
        targeting: Targeting::Px,
        delivery: Delivery::Foreground,
        route: DriverRoute::LinuxXTest,
        operation: Operation::PxClick {
            target: "btn-clicktarget",
            button: "right",
            count: 1,
            expected: "last_action=right_click  clicks=0"
        },
    }
);
catalog_test!(
    harness_gtk3_double_click_px_background,
    CatalogRow {
        action: "double_click",
        targeting: Targeting::Px,
        delivery: Delivery::Background,
        route: DriverRoute::LinuxXSendEvent,
        operation: Operation::PxClick {
            target: "btn-clicktarget",
            button: "left",
            count: 2,
            expected: "last_action=double_click  clicks=2"
        },
    }
);
catalog_test!(
    harness_gtk3_double_click_px_foreground,
    CatalogRow {
        action: "double_click",
        targeting: Targeting::Px,
        delivery: Delivery::Foreground,
        route: DriverRoute::LinuxXTest,
        operation: Operation::PxClick {
            target: "btn-clicktarget",
            button: "left",
            count: 2,
            expected: "last_action=double_click  clicks=2"
        },
    }
);
catalog_test!(
    harness_gtk3_type_text_ax_background,
    CatalogRow {
        action: "type_text",
        targeting: Targeting::Ax,
        delivery: Delivery::Background,
        route: DriverRoute::LinuxAtSpiValue,
        operation: Operation::AxTypeText {
            target: "txt-input",
            text: "ax-typed",
            expected: "mirror=ax-typed"
        },
    }
);
catalog_test!(
    harness_gtk3_type_text_ax_foreground,
    CatalogRow {
        action: "type_text",
        targeting: Targeting::Ax,
        delivery: Delivery::Foreground,
        route: DriverRoute::LinuxAtSpiValue,
        operation: Operation::AxTypeText {
            target: "txt-input",
            text: "ax-typed",
            expected: "mirror=ax-typed"
        },
    }
);
catalog_test!(
    harness_gtk3_type_text_px_background,
    CatalogRow {
        action: "type_text",
        targeting: Targeting::Px,
        delivery: Delivery::Background,
        route: DriverRoute::LinuxXSendEvent,
        operation: Operation::PxTypeText {
            target: "txt-input",
            text: "px-typed",
            expected: "mirror=px-typed"
        },
    }
);
catalog_test!(
    harness_gtk3_type_text_px_foreground,
    CatalogRow {
        action: "type_text",
        targeting: Targeting::Px,
        delivery: Delivery::Foreground,
        route: DriverRoute::LinuxXTest,
        operation: Operation::PxTypeText {
            target: "txt-input",
            text: "px-typed",
            expected: "mirror=px-typed"
        },
    }
);
catalog_test!(
    harness_gtk3_set_value_ax_background,
    CatalogRow {
        action: "set_value",
        targeting: Targeting::Ax,
        delivery: Delivery::Background,
        route: DriverRoute::LinuxAtSpiValue,
        operation: Operation::AxSetValue {
            target: "txt-input",
            value: "set-value",
            expected: "mirror=set-value"
        },
    }
);
catalog_test!(
    harness_gtk3_set_value_ax_foreground,
    CatalogRow {
        action: "set_value",
        targeting: Targeting::Ax,
        delivery: Delivery::Foreground,
        route: DriverRoute::LinuxAtSpiValue,
        operation: Operation::AxSetValue {
            target: "txt-input",
            value: "set-value",
            expected: "mirror=set-value"
        },
    }
);
catalog_test!(
    harness_gtk3_check_ax_background,
    CatalogRow {
        action: "checkbox_toggle",
        targeting: Targeting::Ax,
        delivery: Delivery::Background,
        route: DriverRoute::LinuxAtSpiAction,
        operation: Operation::AxClick {
            target: "chk-agree",
            expected: "agreed=True"
        },
    }
);
catalog_test!(
    harness_gtk3_check_ax_foreground,
    CatalogRow {
        action: "checkbox_toggle",
        targeting: Targeting::Ax,
        delivery: Delivery::Foreground,
        route: DriverRoute::LinuxAtSpiAction,
        operation: Operation::AxClick {
            target: "chk-agree",
            expected: "agreed=True"
        },
    }
);
catalog_test!(
    harness_gtk3_slider_set_value_ax_background,
    CatalogRow {
        action: "slider_set_value",
        targeting: Targeting::Ax,
        delivery: Delivery::Background,
        route: DriverRoute::LinuxAtSpiValue,
        operation: Operation::AxSetValue {
            target: "sld-value",
            value: "64",
            expected: "slider_value=64"
        },
    }
);
catalog_test!(
    harness_gtk3_slider_set_value_ax_foreground,
    CatalogRow {
        action: "slider_set_value",
        targeting: Targeting::Ax,
        delivery: Delivery::Foreground,
        route: DriverRoute::LinuxAtSpiValue,
        operation: Operation::AxSetValue {
            target: "sld-value",
            value: "64",
            expected: "slider_value=64"
        },
    }
);
catalog_test!(
    harness_gtk3_press_key_px_background,
    CatalogRow {
        action: "press_key",
        targeting: Targeting::Px,
        delivery: Delivery::Background,
        route: DriverRoute::LinuxXSendEvent,
        operation: Operation::PressKey {
            key: "f5",
            expected: "last_key=f5  key_presses=1"
        },
    }
);
catalog_test!(
    harness_gtk3_press_key_px_foreground,
    CatalogRow {
        action: "press_key",
        targeting: Targeting::Px,
        delivery: Delivery::Foreground,
        route: DriverRoute::LinuxXTest,
        operation: Operation::PressKey {
            key: "f5",
            expected: "last_key=f5  key_presses=1"
        },
    }
);
catalog_test!(
    harness_gtk3_hotkey_px_background,
    CatalogRow {
        action: "hotkey",
        targeting: Targeting::Px,
        delivery: Delivery::Background,
        route: DriverRoute::LinuxXSendEvent,
        operation: Operation::Hotkey {
            expected: "last_hotkey=ctrl+shift+k  hotkeys=1"
        },
    }
);
catalog_test!(
    harness_gtk3_hotkey_px_foreground,
    CatalogRow {
        action: "hotkey",
        targeting: Targeting::Px,
        delivery: Delivery::Foreground,
        route: DriverRoute::LinuxXTest,
        operation: Operation::Hotkey {
            expected: "last_hotkey=ctrl+shift+k  hotkeys=1"
        },
    }
);
catalog_test!(
    harness_gtk3_scroll_ax_background,
    CatalogRow {
        action: "scroll",
        targeting: Targeting::Ax,
        delivery: Delivery::Background,
        route: DriverRoute::LinuxAtSpiAction,
        operation: Operation::Scroll {
            target: "scroll-tall-vertical",
            pixel: false,
            state_key: "scroll_offset="
        },
    }
);
catalog_test!(
    harness_gtk3_scroll_ax_foreground,
    CatalogRow {
        action: "scroll",
        targeting: Targeting::Ax,
        delivery: Delivery::Foreground,
        route: DriverRoute::LinuxAtSpiAction,
        operation: Operation::Scroll {
            target: "scroll-tall-vertical",
            pixel: false,
            state_key: "scroll_offset="
        },
    }
);
catalog_test!(
    harness_gtk3_scroll_px_background,
    CatalogRow {
        action: "scroll",
        targeting: Targeting::Px,
        delivery: Delivery::Background,
        route: DriverRoute::LinuxXSendEvent,
        operation: Operation::Scroll {
            target: "scroll-tall-viewport",
            pixel: true,
            state_key: "scroll_offset="
        },
    }
);
catalog_test!(
    harness_gtk3_scroll_px_foreground,
    CatalogRow {
        action: "scroll",
        targeting: Targeting::Px,
        delivery: Delivery::Foreground,
        route: DriverRoute::LinuxXTest,
        operation: Operation::Scroll {
            target: "scroll-tall-viewport",
            pixel: true,
            state_key: "scroll_offset="
        },
    }
);
catalog_test!(
    harness_gtk3_drag_px_background,
    CatalogRow {
        action: "drag",
        targeting: Targeting::Px,
        delivery: Delivery::Background,
        route: DriverRoute::LinuxXSendEvent,
        operation: Operation::Drag {
            target: "sld-value",
            state_key: "slider_value="
        },
    }
);
catalog_test!(
    harness_gtk3_drag_px_foreground,
    CatalogRow {
        action: "drag",
        targeting: Targeting::Px,
        delivery: Delivery::Foreground,
        route: DriverRoute::LinuxXTest,
        operation: Operation::Drag {
            target: "sld-value",
            state_key: "slider_value="
        },
    }
);
catalog_test!(
    harness_gtk3_child_window_ax_background,
    CatalogRow {
        action: "child_window",
        targeting: Targeting::Ax,
        delivery: Delivery::Background,
        route: DriverRoute::LinuxAtSpiAction,
        operation: Operation::Popover {
            target: "btn-open-popover",
            expected: "popover_open=True"
        },
    }
);
catalog_test!(
    harness_gtk3_child_window_ax_foreground,
    CatalogRow {
        action: "child_window",
        targeting: Targeting::Ax,
        delivery: Delivery::Foreground,
        route: DriverRoute::LinuxAtSpiAction,
        operation: Operation::Popover {
            target: "btn-open-popover",
            expected: "popover_open=True"
        },
    }
);

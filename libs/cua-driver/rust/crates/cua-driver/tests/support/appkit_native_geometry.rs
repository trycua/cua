use super::*;
use std::time::Instant;

fn fixture_state(
    directory: &Path,
    ready: impl Fn(&serde_json::Value) -> bool,
) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(bytes) = std::fs::read(directory.join("state.json")) {
            let state = serde_json::from_slice(&bytes).expect("native geometry fixture state");
            if ready(&state) {
                return state;
            }
            assert!(Instant::now() < deadline, "fixture state deadline: {state}");
        } else {
            assert!(Instant::now() < deadline, "fixture did not publish state");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn save_response(directory: &Path, name: &str, response: &ToolResponse) {
    std::fs::write(
        directory.join(name),
        serde_json::to_vec_pretty(&response.raw).unwrap(),
    )
    .unwrap();
}

fn web_target(snapshot: &ToolResponse) -> String {
    snapshot.structured()["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|element| {
            element["role"] == "AXButton" && element["label"] == "Geometry reveal target"
        })
        .and_then(|element| element["element_token"].as_str())
        .expect("native WebKit reveal target")
        .to_owned()
}

#[test]
#[ignore]
fn harness_appkit_native_geometry_mismatch_refuses_pixel_without_side_effects() {
    run_native_geometry_mismatch(false);
}

#[test]
#[ignore]
fn harness_appkit_native_geometry_mismatch_refuses_foreground_without_side_effects() {
    run_native_geometry_mismatch(true);
}

fn run_native_geometry_mismatch(foreground: bool) {
    let mut case = if foreground {
        native_foreground_case(
            "appkit",
            "native_geometry_mismatch",
            Targeting::Px,
            DriverRoute::MacosCgEventHid,
        )
    } else {
        native_background_case(
            "appkit",
            "native_geometry_mismatch",
            Targeting::Px,
            DriverRoute::MacosCgEventPid,
        )
    }
    .expecting_refusal(vec![RefusalCode::NativeWindowGeometryMismatch]);
    case.oracles = vec![
        OracleKind::FixtureState,
        OracleKind::Focus,
        OracleKind::ZOrder,
        OracleKind::NoLeakedInput,
        OracleKind::Cursor,
    ];
    let delivery_mode = if foreground {
        "foreground"
    } else {
        "background"
    };
    let cell = case.cell_id.clone();
    execute_case(case, |evidence| {
        let mut driver = McpDriver::spawn_macos_daemon_proxy_named(&cell)
            .expect("start installed macOS daemon proxy");
        *evidence = recording_evidence(driver.recording_dir());
        let directory = driver
            .recording_dir()
            .expect("native evidence directory")
            .join("native-geometry");
        std::fs::create_dir_all(&directory).unwrap();
        let app = Command::new(harness_exe())
            .env("CUA_APPKIT_GEOMETRY_DIR", &directory)
            .stdout(std::fs::File::create(directory.join("fixture.stdout")).unwrap())
            .stderr(std::fs::File::create(directory.join("fixture.stderr")).unwrap())
            .spawn()
            .expect("launch native geometry fixture");
        let harness = Harness {
            pid: app.id(),
            _app: app,
        };
        let initial = fixture_state(&directory, |state| {
            state["window_id"].is_u64() && state["web_scroll_y"].as_f64().is_some_and(|y| y >= 0.0)
        });
        let wid = initial["window_id"].as_u64().unwrap();
        assert_eq!(initial["pid"], harness.pid);
        let aligned = snapshot_elements(&mut driver, harness.pid, wid);
        save_response(&directory, "aligned.json", &aligned);
        assert!(!aligned.is_error(), "aligned snapshot: {}", aligned.text());
        assert_eq!(
            aligned.structured()["native_window_geometry"]["status"],
            "aligned"
        );
        let scale = aligned.structured()["screenshot_width"].as_f64().unwrap() / 360.0;
        let calibration = driver.call(
            "click",
            serde_json::json!({
                "pid": harness.pid, "window_id": wid,
                "x": 300.0 * scale, "y": 130.0 * scale,
                "button": "middle", "delivery_mode": "foreground"
            }),
        );
        save_response(&directory, "calibration.json", &calibration);
        assert!(
            !calibration.is_error(),
            "calibrate native input: {}",
            calibration.text()
        );
        let reveal_snapshot = snapshot_elements(&mut driver, harness.pid, wid);
        let reveal_calibration = driver.call(
            "scroll",
            serde_json::json!({
                "pid": harness.pid, "window_id": wid,
                "element_token": web_target(&reveal_snapshot),
                "direction": "down", "amount": 1
            }),
        );
        save_response(&directory, "reveal-calibration.json", &reveal_calibration);
        assert!(
            !reveal_calibration.is_error(),
            "calibrate reveal: {}",
            reveal_calibration.text()
        );
        let calibrated = fixture_state(&directory, |state| {
            state["web_scroll_y"].as_f64().is_some_and(|y| y > 0.0)
                && state["input_events"].as_array().is_some_and(|events| {
                    events
                        .iter()
                        .any(|event| event["type"] == 25 && event["window_id"] == wid)
                        && events
                            .iter()
                            .any(|event| event["type"] == 26 && event["window_id"] == wid)
                        && events
                            .iter()
                            .any(|event| event["type"] == 22 && event["window_id"] == wid)
                })
        });
        std::fs::write(
            directory.join("calibrated.json"),
            serde_json::to_vec_pretty(&calibrated).unwrap(),
        )
        .unwrap();
        let before_reset = snapshot_elements(&mut driver, harness.pid, wid);
        let reset = driver.call(
            "click",
            serde_json::json!({
                "pid": harness.pid, "window_id": wid,
                "element_token": element_token_by_id(&before_reset, "geometry-reset-web")
            }),
        );
        assert!(!reset.is_error(), "reset scroll probe: {}", reset.text());
        fixture_state(&directory, |state| state["web_scroll_y"] == 0.0);
        let before_toggle = snapshot_elements(&mut driver, harness.pid, wid);
        let toggle = driver.call(
            "click",
            serde_json::json!({
                "pid": harness.pid, "window_id": wid,
                "element_token": element_token_by_id(&before_toggle, "geometry-mismatch")
            }),
        );
        assert!(
            !toggle.is_error(),
            "enable native disagreement: {}",
            toggle.text()
        );
        let target = TargetWindow {
            pid: harness.pid,
            native_id: wid,
        };
        let (response, mut passed) = run_with_background_oracles(&mut driver, target, |driver| {
            let snapshot = driver.call(
                "get_window_state",
                serde_json::json!({
                    "pid": harness.pid, "window_id": wid,
                    "screenshot_out_file": directory.join("mismatched.png")
                }),
            );
            save_response(&directory, "mismatched.json", &snapshot);
            assert!(
                !snapshot.is_error(),
                "mismatched snapshot: {}",
                snapshot.text()
            );
            let geometry = &snapshot.structured()["native_window_geometry"];
            assert_eq!(geometry["status"], "mismatched");
            assert_eq!(geometry["logical"]["width"], 560.0);
            assert_eq!(geometry["compositor"]["width"], 360.0);
            assert_eq!(snapshot.structured()["window_id"], wid);
            assert_eq!(snapshot.structured()["screenshot_frame_valid"], true);
            let png = std::fs::read(directory.join("mismatched.png")).unwrap();
            assert!(png.len() > 24 && png.starts_with(b"\x89PNG\r\n\x1a\n"));
            assert_eq!(
                snapshot.structured()["screenshot_file_path"],
                directory.join("mismatched.png").to_str().unwrap()
            );
            assert_eq!(
                snapshot.structured()["screenshot_width"],
                u32::from_be_bytes(png[16..20].try_into().unwrap())
            );
            assert_eq!(
                snapshot.structured()["screenshot_height"],
                u32::from_be_bytes(png[20..24].try_into().unwrap())
            );
            assert!(has_id(snapshot.tree_text(), "geometry-increment"));
            let increment = driver.call(
                "click",
                serde_json::json!({
                    "pid": harness.pid, "window_id": wid,
                    "element_token": element_token_by_id(&snapshot, "geometry-increment")
                }),
            );
            save_response(&directory, "semantic.json", &increment);
            assert!(
                !increment.is_error(),
                "semantic action: {}",
                increment.text()
            );
            assert_eq!(increment.action_route(), Some("accessibility"));
            fixture_state(&directory, |state| state["counter"] == 1);
            let response = driver.call(
                "click",
                serde_json::json!({
                    "pid": harness.pid, "window_id": wid,
                    "x": 300.0 * scale, "y": 130.0 * scale,
                    "delivery_mode": delivery_mode
                }),
            );
            save_response(&directory, "refusal.json", &response);
            assert!(
                response.is_error(),
                "pixel request must refuse: {}",
                response.text()
            );
            assert_eq!(
                response.structured()["code"],
                "native_window_geometry_mismatch"
            );
            assert_eq!(response.action_effect(), Some("refused"));
            assert_eq!(response.action_delivery_mode(), None);
            let scroll = driver.call(
                "scroll",
                serde_json::json!({
                    "pid": harness.pid, "window_id": wid,
                    "element_token": web_target(&snapshot),
                    "direction": "down", "amount": 1, "delivery_mode": delivery_mode
                }),
            );
            save_response(&directory, "scroll-refusal.json", &scroll);
            assert_eq!(
                scroll.structured()["code"],
                "native_window_geometry_mismatch"
            );
            assert_eq!(scroll.action_effect(), Some("refused"));
            assert_eq!(scroll.action_delivery_mode(), None);
            let after_scroll = fixture_state(&directory, |state| state["counter"] == 1);
            std::fs::write(
                directory.join("after-scroll.json"),
                serde_json::to_vec_pretty(&after_scroll).unwrap(),
            )
            .unwrap();
            assert_eq!(
                after_scroll["web_scroll_y"], 0.0,
                "refused wheel fallback revealed its element"
            );
            std::thread::sleep(Duration::from_millis(750));
            response
        })
        .expect("refusal must preserve background desktop oracles");
        let final_state = fixture_state(&directory, |state| state["counter"] == 1);
        assert_eq!(final_state["input_events"], calibrated["input_events"]);
        passed.push(OracleKind::FixtureState);
        Observation::refused(
            RefusalCode::NativeWindowGeometryMismatch,
            passed,
            response.text(),
            Evidence::default(),
        )
    });
}

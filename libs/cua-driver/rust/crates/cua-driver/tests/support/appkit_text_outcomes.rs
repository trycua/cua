use super::*;

fn fixture_state(directory: &Path) -> serde_json::Value {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(bytes) = std::fs::read(directory.join("state.json")) {
            return serde_json::from_slice(&bytes).expect("text fixture state");
        }
        assert!(
            std::time::Instant::now() < deadline,
            "text fixture did not publish state"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
#[ignore]
fn harness_appkit_text_outcomes_match_independent_state() {
    for (field, effect, route) in [
        ("native", "confirmed", DriverRoute::MacosAxValue),
        ("stale", "unverifiable", DriverRoute::MacosAxValue),
        ("unsupported", "confirmed", DriverRoute::MacosCgEventPid),
    ] {
        let case = native_background_case(
            "appkit",
            &format!("type_text_{field}_readback"),
            Targeting::Ax,
            route,
        );
        let label = case.cell_id.clone();
        execute_case(case, |evidence| {
            let mut driver = McpDriver::spawn_macos_daemon_proxy_named(&label)
                .expect("text outcome test requires a TCC-authorized installed daemon");
            *evidence = recording_evidence(driver.recording_dir());
            let output = driver
                .recording_dir()
                .expect("set CUA_E2E_RECORDINGS_ROOT")
                .to_path_buf();
            let directory = tempfile::tempdir().unwrap();
            let child = Command::new(harness_exe())
                .env("CUA_APPKIT_TEXT_OUTCOMES_DIR", directory.path())
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap();
            let harness = Harness {
                pid: child.id(),
                _app: child,
            };
            let ready = fixture_state(directory.path());
            let window = ready["window_id"].as_u64().unwrap();
            assert_eq!(ready["pid"].as_u64(), Some(harness.pid as u64));
            let (_, passed) = run_with_background_oracles(
                &mut driver,
                TargetWindow {
                    pid: harness.pid,
                    native_id: window,
                },
                |driver| {
                    let snapshot = snapshot_elements(driver, harness.pid, window);
                    let token = element_token_by_id(&snapshot, field);
                    let response = driver.call(
                        "type_text",
                        serde_json::json!({
                            "pid": harness.pid, "window_id": window, "element_token": token,
                            "text": "marker", "delivery_mode": "background"
                        }),
                    );
                    let state = fixture_state(directory.path());
                    std::fs::write(
                        output.join("text-outcome-response.json"),
                        serde_json::to_vec_pretty(&response.raw).unwrap(),
                    )
                    .unwrap();
                    std::fs::write(
                        output.join("text-outcome-state.json"),
                        serde_json::to_vec_pretty(&state).unwrap(),
                    )
                    .unwrap();
                    assert!(!response.is_error(), "{}", response.text());
                    let result = response.structured();
                    assert_eq!(result["effect"], effect);
                    assert_eq!(result["delivery"]["mode"], "background");
                    assert!(result.get("escalation").is_none());
                    if field == "stale" {
                        assert!(result["delivery"].get("delivered_count").is_none());
                    } else {
                        assert_eq!(result["delivery"]["delivered_count"], 6);
                    }
                    assert_eq!(state["fields"][field]["value"], "marker");
                    assert_eq!(state["unrelated"], "marker");
                    if field == "unsupported" {
                        assert_eq!(result["route"], "synthetic_events");
                        assert_eq!(state["fields"][field]["writes"], 0);
                        assert_eq!(state["fields"][field]["keys"], 6);
                    } else {
                        assert_eq!(result["route"], "accessibility");
                        assert_eq!(state["fields"][field]["writes"], 1);
                        assert_eq!(state["fields"][field]["keys"], 0);
                    }
                },
            )
            .expect("background text fixture oracles");
            Observation::delivered_with_fixture_state(passed)
        });
    }
}

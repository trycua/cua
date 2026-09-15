use super::*;
use std::time::Instant;

#[test]
#[ignore]
fn harness_appkit_pixel_focus_preserves_selection_and_latency() {
    run_case(
        native_foreground_case(
            "appkit",
            "pixel-focus-latency",
            Targeting::Px,
            DriverRoute::MacosCgEventPid,
        ),
        |pid, wid, driver| {
            let initial = snapshot_elements(driver, pid, wid);
            let (x, y, width, height) = element_pixel_frame(&initial, "txt-input");
            let point = serde_json::json!({
                "pid":pid, "window_id":wid,
                "x":x + width / 2.0, "y":y + height / 2.0,
                "delivery_mode":"foreground"
            });
            let output = driver
                .recording_dir()
                .unwrap()
                .join("pixel-focus-latency.json");
            let mut measurements = Vec::new();
            for trial in 0..14 {
                let reset_snapshot = snapshot_elements(driver, pid, wid);
                let reset = driver.call(
                    "set_value",
                    serde_json::json!({
                        "pid":pid, "window_id":wid,
                        "element_token":element_token_by_id(&reset_snapshot, "txt-input"),
                        "value":""
                    }),
                );
                assert!(!reset.is_error(), "reset: {}", reset.text());
                let expected = format!("focus-{trial}");
                let steps = [
                    ("type_text", serde_json::json!({"text":"original"})),
                    ("press_key", serde_json::json!({"key":"a"})),
                    ("hotkey", serde_json::json!({"keys":["cmd", "a"]})),
                    ("type_text", serde_json::json!({"text":expected})),
                ];
                let started = Instant::now();
                let mut actions = Vec::new();
                for (tool, extra) in steps {
                    let mut args = point.clone();
                    args.as_object_mut()
                        .unwrap()
                        .extend(extra.as_object().unwrap().clone());
                    let action_started = Instant::now();
                    let result = driver.call(tool, args);
                    let elapsed_ms = action_started.elapsed().as_secs_f64() * 1000.0;
                    assert!(!result.is_error(), "{tool}: {}", result.text());
                    actions.push(serde_json::json!({
                        "tool":tool, "elapsed_ms":elapsed_ms, "response":result.raw
                    }));
                }
                let observed = snapshot_elements(driver, pid, wid);
                let index = element_index_by_id(observed.tree_text(), "txt-input").unwrap();
                let field = observed.structured()["elements"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|element| element["element_index"].as_u64() == Some(index))
                    .unwrap();
                measurements.push(serde_json::json!({
                    "trial":trial, "warmup":trial < 2,
                    "verified_elapsed_ms":started.elapsed().as_secs_f64() * 1000.0,
                    "actions":actions, "observed_value":field["value"]
                }));
                std::fs::write(&output, serde_json::to_vec_pretty(&measurements).unwrap()).unwrap();
                assert_eq!(
                    field["value"], expected,
                    "pixel focus must preserve select-all before replacement"
                );
            }
            Observation::delivered(vec![OracleKind::FixtureState], Evidence::default())
        },
    );
}

use cua_driver_core::action_record::{
    ActionEffect, ActionExecutionRecord, ActionTransport, ActualDelivery, RequestedDelivery,
};
use cua_driver_core::recording::{
    set_ax_snapshot_fn, set_element_bounds_fn, set_screenshot_fn, RecordingSession,
};
use serde_json::{json, Value};

#[test]
fn macos_semantic_activation_without_a_point_preserves_truth_without_a_marker() {
    let png = cua_driver_core::image_utils::encode_rgba_to_png(&[255; 16], 2, 2).unwrap();
    set_screenshot_fn(move |_, _| Some(png.clone()));
    set_ax_snapshot_fn(|_, _| Some(br#"{"fixture":"semantic-activation"}"#.to_vec()));
    set_element_bounds_fn(|_, _, _| None);

    let directory = tempfile::tempdir().unwrap();
    let recording = RecordingSession::new();
    recording
        .start(directory.path().to_str().unwrap(), false, None)
        .unwrap();
    let args = json!({
        "pid": 32166,
        "window_id": 2467,
        "element_index": 12,
        "snapshot_id": "s00000232",
        "action": "press",
        "delivery_mode": "foreground"
    });
    let action = ActionExecutionRecord::builder(
        ActionEffect::Unverifiable,
        ActionTransport::MacosAxAction,
        RequestedDelivery::Foreground,
    )
    .actual_delivery(ActualDelivery::Foreground)
    .build()
    .unwrap();
    let pending = recording.begin_turn("click", &args, 0).unwrap();
    recording.finish_turn_with_outcome(pending, "Performed AXPress", Some(&action), false);
    recording.stop_owner(None).unwrap();

    let turn = directory.path().join("turn-00001");
    let read_json = |name: &str| -> Value {
        serde_json::from_slice(&std::fs::read(turn.join(name)).unwrap()).unwrap()
    };
    let recorded = read_json("action.json");
    let evidence = read_json("evidence.json");
    assert_eq!(recorded["action_truth"]["transport"], "macos_ax_action");
    assert_eq!(recorded["action_truth"]["route"], "accessibility");
    assert_eq!(recorded["action_truth"]["effect"], "unverifiable");
    assert_eq!(recorded["action_truth"]["actual_delivery"], "foreground");
    assert_eq!(recorded["result_error"], false);
    assert!(recorded.get("click_point").is_none());
    assert!(turn.join("before.png").is_file());
    assert!(turn.join("after.png").is_file());
    assert_eq!(evidence["before"]["screenshot"]["status"], "captured");
    assert_eq!(evidence["after"]["screenshot"]["status"], "captured");
    assert!(!turn.join("click.png").exists());
    assert_eq!(
        evidence["click"],
        json!({
            "status": "not_applicable",
            "classification": "semantic_action_without_point",
            "source_image": "before.png"
        }),
        "a semantic activation with captured screenshots must not be reported as a failed spatial marker capture"
    );
}

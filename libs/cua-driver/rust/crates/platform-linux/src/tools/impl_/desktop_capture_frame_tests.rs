use super::{capture_admission_error, normalize_desktop_capture_for_action_frame};

fn png(width: u32, height: u32) -> Vec<u8> {
    let rgba = vec![0x7f; (width * height * 4) as usize];
    cua_driver_core::image_utils::encode_rgba_to_png(&rgba, width, height).expect("encode fixture")
}

#[test]
fn native_wayland_capture_is_resized_to_the_reported_action_frame() {
    let (normalized, width, height, scale) =
        normalize_desktop_capture_for_action_frame(png(3200, 2000), 1600, 1000)
            .expect("normalize 2x capture");

    assert_eq!((width, height), (1600, 1000));
    assert_eq!(
        cua_driver_core::image_utils::png_dimensions(&normalized).unwrap(),
        (1600, 1000)
    );
    assert_eq!(scale, 2.0);
}

#[test]
fn nonuniform_capture_mapping_fails_instead_of_distorting_coordinates() {
    let error = normalize_desktop_capture_for_action_frame(png(3200, 2000), 1600, 1200)
        .expect_err("nonuniform mapping must fail closed");
    assert!(error.to_string().contains("cannot be mapped uniformly"));
}

#[test]
fn capture_refusal_exposes_specific_code_and_refused_effect() {
    let refusal = capture_admission_error(anyhow::Error::new(
        cua_driver_core::capture_runtime::CaptureActionError::NativeActionFrameMismatch,
    ));
    let structured = refusal.structured_content.unwrap();
    assert_eq!(structured["code"], "capture_frame_mismatch");
    assert_eq!(structured["effect"], "refused");
}

#[test]
fn desktop_window_records_and_text_use_the_captured_frame_origin() {
    let frame = crate::wayland::hyprland::DesktopFrame {
        x: -3840,
        y: -1080,
        width: 7680,
        height: 3240,
        scale: 1.0,
        outputs: vec![],
    };
    let mut windows = vec![crate::x11::WindowInfo {
        xid: 42,
        pid: Some(123),
        title: "left".into(),
        app_name: "fixture".into(),
        is_on_screen: true,
        z_index: None,
        x: -3838,
        y: -1060,
        width: 800,
        height: 600,
    }];
    super::rebase_desktop_windows(&mut windows, &frame);
    let record = super::window_record_json(&windows[0]);
    assert_eq!(
        record["bounds"],
        serde_json::json!({"x":2,"y":20,"width":800,"height":600})
    );
    assert_eq!(record["x"], 2);
    assert_eq!(record["y"], 20);
    assert_eq!(record["window_id"], 42);
    assert!(super::desktop_window_lines(&windows, 1.0).contains("800x600 at (2,20)"));
    assert!(super::desktop_window_lines(&windows, 2.0).contains("400x300 at (1,10)"));
}

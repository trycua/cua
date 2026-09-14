use cua_driver_core::native_window_geometry::{
    assess, observe_capture, GeometryAssessment, GeometrySample, NativeWindowRect, PointTolerance,
};

#[test]
fn a_mismatch_does_not_discard_images_or_replace_capture_errors() {
    let sample = GeometrySample {
        logical: NativeWindowRect::new(200.0, 100.0, 230.0, 408.0),
        compositor: NativeWindowRect::new(10.0, 250.0, 31.0, 102.0),
    };
    let png = cua_driver_core::image_utils::encode_rgba_to_png(&[255; 16], 2, 2).unwrap();
    for capture in [Ok(png), Err("capture denied")] {
        let (retained, assessment) = observe_capture(
            || sample,
            || capture.clone(),
            PointTolerance::new(1.0).unwrap(),
        );
        assert_eq!(retained, capture);
        assert!(matches!(assessment, GeometryAssessment::Mismatched { .. }));
    }
}

#[test]
fn unavailable_geometry_preserves_existing_route_refusals() {
    let mut snapshot = serde_json::json!({
        "background_input": {
            "routes": [{"route": "window_pointer", "status": "refused", "reason": "minimized_or_hidden_window"}]
        }
    });
    let routes = snapshot["background_input"].clone();
    GeometryAssessment::Unavailable.project_snapshot(&mut snapshot);
    assert_eq!(snapshot["background_input"], routes);
    assert_eq!(
        snapshot["native_window_geometry"],
        serde_json::json!({"status": "unavailable"})
    );
}

#[test]
fn geometry_changes_during_capture_preserve_the_capture_but_do_not_prove_a_frame() {
    let logical = NativeWindowRect::new(200.0, 100.0, 230.0, 408.0);
    let native_geometry = std::cell::Cell::new(GeometrySample {
        logical,
        compositor: logical,
    });
    let png = cua_driver_core::image_utils::encode_rgba_to_png(&[255; 16], 2, 2).unwrap();
    let (retained, assessment) = observe_capture(
        || native_geometry.get(),
        || {
            native_geometry.set(GeometrySample {
                logical,
                compositor: NativeWindowRect::new(10.0, 250.0, 31.0, 102.0),
            });
            png.clone()
        },
        PointTolerance::new(1.0).unwrap(),
    );
    assert_eq!(retained, png);
    assert_eq!(assessment, GeometryAssessment::Unstable);
}

#[test]
fn projection_does_not_invent_a_background_capability_report() {
    let sample = GeometrySample {
        logical: NativeWindowRect::new(200.0, 100.0, 230.0, 408.0),
        compositor: NativeWindowRect::new(10.0, 250.0, 31.0, 102.0),
    };
    let mut snapshot = serde_json::json!({"window_id": 7});
    assess(sample, sample, PointTolerance::new(1.0).unwrap()).project_snapshot(&mut snapshot);
    assert!(snapshot.get("background_input").is_none());
}

#[test]
fn mismatch_projection_preserves_observation_and_semantic_routes() {
    let sample = GeometrySample {
        logical: NativeWindowRect::new(200.0, 100.0, 230.0, 408.0),
        compositor: NativeWindowRect::new(10.0, 250.0, 31.0, 102.0),
    };
    let assessment = assess(sample, sample, PointTolerance::new(1.0).unwrap());
    let mut snapshot = serde_json::json!({
        "snapshot_id": "s00000001",
        "elements": [{"element_token": "s00000001:0"}],
        "screenshot_frame_valid": true,
        "screenshot_width": 31,
        "screenshot_height": 102,
        "background_input": {
            "routes": [
                {"route": "accessibility", "status": "available"},
                {"route": "window_pointer", "status": "available"},
                {"route": "pid_keyboard", "status": "available"}
            ],
            "observation": {"one_shot_capture": "available", "frame_freshness": "unknown"}
        }
    });
    let mut expected = snapshot.clone();
    expected["native_window_geometry"] = serde_json::json!({
        "status": "mismatched",
        "logical": {"x": 200.0, "y": 100.0, "width": 230.0, "height": 408.0},
        "compositor": {"x": 10.0, "y": 250.0, "width": 31.0, "height": 102.0}
    });
    expected["background_input"]["routes"][1] = serde_json::json!({
        "route": "window_pointer", "status": "refused", "reason": "native_window_geometry_mismatch"
    });
    assessment.project_snapshot(&mut snapshot);
    assert_eq!(snapshot, expected);
}

#[test]
fn stable_calculator_thumbnail_is_a_confirmed_geometry_mismatch() {
    let sample = GeometrySample {
        logical: NativeWindowRect::new(200.0, 100.0, 230.0, 408.0),
        compositor: NativeWindowRect::new(10.0, 250.0, 31.0, 102.0),
    };
    let assessment = assess(sample, sample, PointTolerance::new(1.0).unwrap());
    assert!(matches!(assessment, GeometryAssessment::Mismatched { .. }));
    assert!(assessment.blocks_pointer());
}

#[test]
fn genuinely_small_window_on_a_negative_origin_display_is_aligned() {
    let frame = NativeWindowRect::new(-1920.0, -20.0, 90.0, 102.0);
    let sample = GeometrySample {
        logical: frame,
        compositor: frame,
    };
    let assessment = assess(sample, sample, PointTolerance::new(1.0).unwrap());
    assert!(matches!(assessment, GeometryAssessment::Aligned { .. }));
    assert!(!assessment.blocks_pointer());
}

#[test]
fn a_capture_during_a_stage_transition_is_unstable_not_a_confirmed_mismatch() {
    let logical = NativeWindowRect::new(200.0, 100.0, 230.0, 408.0);
    let before = GeometrySample {
        logical,
        compositor: logical,
    };
    let after = GeometrySample {
        logical,
        compositor: NativeWindowRect::new(10.0, 250.0, 31.0, 102.0),
    };
    for (first, last) in [(before, after), (after, before)] {
        let assessment = assess(first, last, PointTolerance::new(1.0).unwrap());
        assert_eq!(assessment, GeometryAssessment::Unstable);
        assert!(!assessment.blocks_pointer());
    }
}

#[test]
fn sampling_jitter_obeys_the_callers_point_tolerance() {
    let before = GeometrySample {
        logical: NativeWindowRect::new(-10.0, 20.0, 90.0, 102.0),
        compositor: NativeWindowRect::new(-10.0, 20.0, 90.0, 102.0),
    };
    let after = GeometrySample {
        logical: NativeWindowRect::new(-9.5, 20.0, 90.0, 102.0),
        compositor: NativeWindowRect::new(-9.0, 20.0, 90.0, 102.0),
    };
    assert!(matches!(
        assess(before, after, PointTolerance::new(1.0).unwrap()),
        GeometryAssessment::Aligned { .. }
    ));
    assert_eq!(
        assess(before, after, PointTolerance::new(0.25).unwrap()),
        GeometryAssessment::Unstable
    );
}

#[test]
fn disagreement_must_hold_on_both_sides_of_the_capture() {
    let logical = NativeWindowRect::new(10.0, 20.0, 90.0, 102.0);
    let before = GeometrySample {
        logical,
        compositor: NativeWindowRect::new(11.0, 20.0, 90.0, 102.0),
    };
    let after = GeometrySample {
        logical,
        compositor: NativeWindowRect::new(11.5, 20.0, 90.0, 102.0),
    };
    for (first, last) in [(before, after), (after, before)] {
        assert_eq!(
            assess(first, last, PointTolerance::new(1.0).unwrap()),
            GeometryAssessment::Unstable
        );
    }
}

#[test]
fn missing_native_evidence_is_unavailable_without_a_new_pointer_veto() {
    let frame = NativeWindowRect::new(10.0, 20.0, 90.0, 102.0);
    let complete = GeometrySample {
        logical: frame,
        compositor: frame,
    };
    for missing in [
        GeometrySample::default(),
        GeometrySample {
            logical: None,
            compositor: frame,
        },
        GeometrySample {
            logical: frame,
            compositor: None,
        },
    ] {
        for (before, after) in [(missing, complete), (complete, missing)] {
            let assessment = assess(before, after, PointTolerance::new(1.0).unwrap());
            assert_eq!(assessment, GeometryAssessment::Unavailable);
            assert!(!assessment.blocks_pointer());
        }
    }
}

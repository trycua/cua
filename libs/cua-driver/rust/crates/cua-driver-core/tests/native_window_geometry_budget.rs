use cua_driver_core::native_window_geometry::{
    geometry_call_timeout, observe_capture_budgeted, GeometryAssessment, GeometrySample,
    NativeWindowRect, PointTolerance,
};
use std::cell::Cell;
use std::time::Duration;

fn sample() -> GeometrySample {
    GeometrySample {
        logical: NativeWindowRect::new(-100.0, 10.0, 230.0, 408.0),
        compositor: NativeWindowRect::new(-100.0, 10.0, 31.0, 102.0),
    }
}

#[test]
fn both_samples_spend_one_budget_without_charging_capture() {
    let mut allowances = Vec::new();
    let (captured, assessment) = observe_capture_budgeted(
        |remaining| {
            allowances.push(remaining);
            (sample(), Duration::from_millis(4))
        },
        || {
            std::thread::sleep(Duration::from_millis(20));
            vec![137, 80, 78, 71]
        },
        Duration::from_millis(10),
        PointTolerance::new(1.0).unwrap(),
    );
    assert_eq!(
        allowances,
        [Duration::from_millis(10), Duration::from_millis(6)]
    );
    assert_eq!(captured, [137, 80, 78, 71]);
    assert!(assessment.blocks_pointer());
}

#[test]
fn late_second_sample_cannot_confirm_mismatch() {
    let mut calls = 0;
    let (captured, assessment) = observe_capture_budgeted(
        |remaining| {
            calls += 1;
            assert_eq!(
                remaining,
                Duration::from_millis(if calls == 1 { 250 } else { 125 })
            );
            (
                sample(),
                Duration::from_millis(if calls == 1 { 125 } else { 126 }),
            )
        },
        || "retained screenshot and tree",
        Duration::from_millis(250),
        PointTolerance::new(1.0).unwrap(),
    );
    assert_eq!(calls, 2);
    assert_eq!(captured, "retained screenshot and tree");
    assert_eq!(assessment, GeometryAssessment::Unavailable);
    assert!(!assessment.blocks_pointer());
}

#[test]
fn exhausted_budget_skips_further_metadata_but_preserves_capture_error() {
    for elapsed in [
        Duration::from_millis(250),
        Duration::from_millis(251),
        Duration::MAX,
    ] {
        let calls = Cell::new(0);
        let (captured, assessment) = observe_capture_budgeted(
            |_| {
                calls.set(calls.get() + 1);
                (sample(), elapsed)
            },
            || Err::<(), _>("capture failed independently"),
            Duration::from_millis(250),
            PointTolerance::new(1.0).unwrap(),
        );
        assert_eq!(calls.get(), 1);
        assert_eq!(captured, Err("capture failed independently"));
        assert_eq!(assessment, GeometryAssessment::Unavailable);
    }
}

#[test]
fn zero_budget_never_samples_and_still_captures_once() {
    let captures = Cell::new(0);
    let (_, assessment) = observe_capture_budgeted(
        |_| panic!("metadata must not run without an allowance"),
        || captures.set(captures.get() + 1),
        Duration::ZERO,
        PointTolerance::new(1.0).unwrap(),
    );
    assert_eq!(captures.get(), 1);
    assert_eq!(assessment, GeometryAssessment::Unavailable);
}

#[test]
fn independent_requests_do_not_reuse_exhausted_budget_or_assessment() {
    for aligned in [false, true, false] {
        let (_, assessment) = observe_capture_budgeted(
            |_| {
                let mut value = sample();
                if aligned {
                    value.compositor = value.logical;
                }
                (value, Duration::from_millis(1))
            },
            || (),
            Duration::from_millis(2),
            PointTolerance::new(1.0).unwrap(),
        );
        assert_eq!(assessment.blocks_pointer(), !aligned);
    }
}

#[test]
fn each_ax_timeout_reserves_remaining_calls_and_obeys_the_cap() {
    let cap = Duration::from_millis(25);
    for (remaining, calls, expected) in [(250, 1, 25), (50, 2, 25), (20, 2, 10), (9, 3, 3)] {
        assert_eq!(
            geometry_call_timeout(Duration::from_millis(remaining), calls, cap),
            Some(Duration::from_millis(expected))
        );
    }
    for (remaining, calls, cap) in [
        (Duration::ZERO, 1, cap),
        (Duration::from_millis(50), 0, cap),
        (Duration::from_nanos(1), 2, cap),
        (Duration::from_millis(50), 1, Duration::ZERO),
    ] {
        assert_eq!(geometry_call_timeout(remaining, calls, cap), None);
    }
}

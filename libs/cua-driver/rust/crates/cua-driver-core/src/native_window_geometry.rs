use serde::Serialize;
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct NativeWindowRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl NativeWindowRect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Option<Self> {
        let components = [x, y, width, height];
        (components.iter().all(|value| value.is_finite()) && width > 0.0 && height > 0.0).then_some(
            Self {
                x,
                y,
                width,
                height,
            },
        )
    }

    pub fn matches_within(self, other: Self, tolerance: PointTolerance) -> bool {
        [self.x, self.y, self.width, self.height]
            .iter()
            .zip([other.x, other.y, other.width, other.height])
            .all(|(left, right)| (left - right).abs() <= tolerance.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointTolerance(f64);

impl PointTolerance {
    pub fn new(points: f64) -> Option<Self> {
        (points.is_finite() && points >= 0.0).then_some(Self(points))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GeometrySample {
    pub logical: Option<NativeWindowRect>,
    pub compositor: Option<NativeWindowRect>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum GeometryAssessment {
    Aligned {
        logical: NativeWindowRect,
        compositor: NativeWindowRect,
    },
    Mismatched {
        logical: NativeWindowRect,
        compositor: NativeWindowRect,
    },
    Unstable,
    Unavailable,
}

impl GeometryAssessment {
    pub fn blocks_pointer(self) -> bool {
        matches!(self, Self::Mismatched { .. })
    }

    pub fn project_snapshot(self, snapshot: &mut Value) {
        snapshot["native_window_geometry"] = json!(self);
        if self.blocks_pointer() {
            if let Some(routes) = snapshot
                .get_mut("background_input")
                .and_then(|report| report.get_mut("routes"))
                .and_then(Value::as_array_mut)
            {
                for route in routes {
                    if route["route"] == "window_pointer" && route["status"] == "available" {
                        route["status"] = json!("refused");
                        route["reason"] = json!("native_window_geometry_mismatch");
                    }
                }
            }
        }
    }
}

pub fn observe_capture<T>(
    mut sample: impl FnMut() -> GeometrySample,
    capture: impl FnOnce() -> T,
    tolerance: PointTolerance,
) -> (T, GeometryAssessment) {
    let before = sample();
    let captured = capture();
    let after = sample();
    (captured, assess(before, after, tolerance))
}

pub fn assess(
    before: GeometrySample,
    after: GeometrySample,
    tolerance: PointTolerance,
) -> GeometryAssessment {
    let (Some(before_logical), Some(before_compositor), Some(logical), Some(compositor)) = (
        before.logical,
        before.compositor,
        after.logical,
        after.compositor,
    ) else {
        return GeometryAssessment::Unavailable;
    };
    if !before_logical.matches_within(logical, tolerance)
        || !before_compositor.matches_within(compositor, tolerance)
    {
        return GeometryAssessment::Unstable;
    }
    match (
        before_logical.matches_within(before_compositor, tolerance),
        logical.matches_within(compositor, tolerance),
    ) {
        (true, true) => GeometryAssessment::Aligned {
            logical,
            compositor,
        },
        (false, false) => GeometryAssessment::Mismatched {
            logical,
            compositor,
        },
        _ => GeometryAssessment::Unstable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f64, y: f64, width: f64, height: f64) -> NativeWindowRect {
        NativeWindowRect::new(x, y, width, height).unwrap()
    }

    #[test]
    fn ordinary_and_genuinely_small_windows_match() {
        let tolerance = PointTolerance::new(0.0).unwrap();
        for frame in [
            rect(12.0, 20.0, 230.0, 408.0),
            rect(-90.0, -102.0, 90.0, 102.0),
        ] {
            assert!(frame.matches_within(frame, tolerance));
        }
    }

    #[test]
    fn thumbnail_need_not_shrink_both_dimensions() {
        let logical = rect(0.0, 0.0, 90.0, 102.0);
        let thumbnail = rect(0.0, 0.0, 47.0, 105.0);
        assert!(!logical.matches_within(thumbnail, PointTolerance::new(0.0).unwrap()));
    }

    #[test]
    fn each_component_obeys_the_supplied_tolerance_in_both_directions() {
        let components = [10.0, 20.0, 90.0, 102.0];
        let original = rect(components[0], components[1], components[2], components[3]);
        let tolerance = PointTolerance::new(1.0).unwrap();
        for index in 0..4 {
            for delta in [-1.25, -1.0, 1.0, 1.25] {
                let mut changed = components;
                changed[index] += delta;
                let changed = rect(changed[0], changed[1], changed[2], changed[3]);
                let expected = delta.abs() <= 1.0;
                assert_eq!(original.matches_within(changed, tolerance), expected);
                assert_eq!(changed.matches_within(original, tolerance), expected);
            }
        }
    }

    #[test]
    fn invalid_native_values_cannot_become_rectangles() {
        for index in 0..4 {
            for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
                let mut values = [0.0, 0.0, 90.0, 102.0];
                values[index] = invalid;
                assert!(
                    NativeWindowRect::new(values[0], values[1], values[2], values[3]).is_none()
                );
            }
        }
        for extent in [0.0, -1.0] {
            assert!(NativeWindowRect::new(0.0, 0.0, extent, 102.0).is_none());
            assert!(NativeWindowRect::new(0.0, 0.0, 90.0, extent).is_none());
        }
    }

    #[test]
    fn tolerance_must_be_finite_and_nonnegative() {
        for invalid in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(PointTolerance::new(invalid).is_none());
        }
        assert!(PointTolerance::new(0.0).is_some());
    }

    #[test]
    fn calculator_thumbnail_does_not_match_logical_window() {
        let logical = rect(0.0, 0.0, 230.0, 408.0);
        let thumbnail = rect(0.0, 0.0, 31.0, 102.0);
        assert!(!logical.matches_within(thumbnail, PointTolerance::new(0.0).unwrap()));
    }
}

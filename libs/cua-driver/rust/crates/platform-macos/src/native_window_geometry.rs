use core_foundation::base::{CFType, TCFType};
use cua_driver_core::native_window_geometry::{
    GeometryAssessment, GeometrySample, NativeWindowRect, PointTolerance,
};
use std::time::{Duration, Instant};

use crate::ax::bindings::{
    ax_get_window_id, copy_ax_windows, element_screen_rect, AXUIElementCreateApplication,
    AXUIElementRef, AXUIElementSetMessagingTimeout,
};

const METADATA_BUDGET: Duration = Duration::from_millis(250);
const MAX_WINDOW_READS: usize = 64;
const POINT_TOLERANCE: f64 = 1.0;

pub(crate) fn observe_capture<T>(
    pid: i32,
    window_id: u32,
    capture: impl FnOnce() -> T,
) -> (T, GeometryAssessment) {
    let mut remaining = METADATA_BUDGET;
    cua_driver_core::native_window_geometry::observe_capture(
        || {
            let started = Instant::now();
            let sample = sample(pid, window_id, started + remaining);
            let elapsed = started.elapsed();
            let in_budget = elapsed <= remaining;
            remaining = remaining.saturating_sub(elapsed);
            if in_budget {
                sample
            } else {
                GeometrySample::default()
            }
        },
        capture,
        PointTolerance::new(POINT_TOLERANCE).unwrap(),
    )
}

fn sample(pid: i32, window_id: u32, deadline: Instant) -> GeometrySample {
    if Instant::now() >= deadline {
        return GeometrySample::default();
    }
    let Some(window) = crate::windows::window_info_by_id(window_id).filter(|w| w.pid == pid) else {
        return GeometrySample::default();
    };
    let bounds = window.bounds;
    let compositor = NativeWindowRect::new(bounds.x, bounds.y, bounds.width, bounds.height);
    let logical = logical_rect(pid, window_id, deadline);
    GeometrySample {
        logical,
        compositor,
    }
}

fn set_timeout(element: AXUIElementRef, deadline: Instant, calls: u32) -> bool {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return false;
    }
    let seconds = (remaining / calls).as_secs_f32().min(0.025);
    seconds > 0.0 && unsafe { AXUIElementSetMessagingTimeout(element, seconds) == 0 }
}

fn logical_rect(pid: i32, window_id: u32, deadline: Instant) -> Option<NativeWindowRect> {
    unsafe {
        let raw_app = AXUIElementCreateApplication(pid);
        if raw_app.is_null() {
            return None;
        }
        let app = CFType::wrap_under_create_rule(raw_app.cast());
        if !set_timeout(app.as_CFTypeRef().cast_mut().cast(), deadline, 1) {
            return None;
        }
        let windows: Vec<CFType> = copy_ax_windows(app.as_CFTypeRef().cast_mut().cast())
            .into_iter()
            .map(|window| CFType::wrap_under_create_rule(window.cast()))
            .collect();
        for window in windows.iter().take(MAX_WINDOW_READS) {
            let element = window.as_CFTypeRef().cast_mut().cast();
            if !set_timeout(element, deadline, 1) {
                return None;
            }
            if ax_get_window_id(element) != Some(window_id) {
                continue;
            }
            if !set_timeout(element, deadline, 2) {
                return None;
            }
            let [x, y, width, height] = element_screen_rect(element)?;
            return (Instant::now() < deadline)
                .then(|| NativeWindowRect::new(x, y, width, height))
                .flatten();
        }
    }
    None
}

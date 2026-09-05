//! Cursor guard adapted from contributor PR #3466; compositor observations
//! remain independent of Driver geometry, capture and input implementations.

use crate::observer::{DesktopSnapshot, TargetZ};

type Point = (f64, f64);
type Region = (f64, f64, f64, f64);

fn canary_destination(original: Point, regions: &[Region]) -> Result<Point, String> {
    if !original.0.is_finite() || !original.1.is_finite() {
        return Err("Hyprland cursor is not finite".into());
    }
    // Regions are compositor logical coordinates, already scaled/rotated by
    // the independent observer. Never use an enclosing desktop bounding box:
    // it can include gaps between outputs and discard negative origins.
    for &(x, y, width, height) in regions {
        if ![x, y, width, height, x + width, y + height]
            .iter()
            .all(|value| value.is_finite())
            || width <= 0.0
            || height <= 0.0
        {
            return Err("Hyprland monitor region is invalid".into());
        }
    }
    let &(x, y, width, height) = regions
        .iter()
        .find(|&&(x, y, width, height)| {
            original.0 >= x && original.0 < x + width && original.1 >= y && original.1 < y + height
        })
        .ok_or("Hyprland original cursor is outside active logical monitors")?;
    for fraction in [0.25, 0.75] {
        let candidate = (
            (x + width * fraction).round(),
            (y + height * fraction).round(),
        );
        if candidate.0 >= x
            && candidate.0 < x + width
            && candidate.1 >= y
            && candidate.1 < y + height
            && (candidate.0 - original.0).hypot(candidate.1 - original.1) > 8.0
        {
            return Ok(candidate);
        }
    }
    Err("Hyprland monitor has no distinct interior cursor canary".into())
}

trait CalibrationIo {
    fn snapshot(&mut self) -> Result<DesktopSnapshot, String>;
    fn regions(&mut self) -> Result<Vec<Region>, String>;
    fn move_cursor(&mut self, point: Point) -> Result<(), String>;
    fn restore_focus(&mut self) -> Result<(), String>;
}

fn at_destination(actual: Option<Point>, requested: Point) -> bool {
    actual.is_some_and(|point| {
        point.0.is_finite()
            && point.1.is_finite()
            && (point.0 - requested.0).abs() <= 1.0
            && (point.1 - requested.1).abs() <= 1.0
    })
}

fn same_posture(left: &DesktopSnapshot, right: &DesktopSnapshot) -> bool {
    left.foreground == right.foreground
        && left.input_focus == right.input_focus
        && left.target_z == right.target_z
}

fn calibrate_io(io: &mut impl CalibrationIo) -> Result<(), String> {
    let before = io.snapshot()?;
    if before.target_z != TargetZ::Foreground
        || before.foreground.is_none()
        || before.foreground != before.input_focus
    {
        return Err("Hyprland cursor calibration requires a focused sentinel".into());
    }
    let original = before
        .cursor_pos
        .ok_or("Hyprland cursor position unavailable")?;
    if !original.0.is_finite() || !original.1.is_finite() {
        return Err("Hyprland original cursor is not finite".into());
    }
    let result = (|| {
        let regions = io.regions()?;
        let destination = canary_destination(original, &regions)?;
        let read_only = io.snapshot()?;
        if before != read_only {
            return Err("Hyprland read-only cursor samples changed desktop state".into());
        }
        io.move_cursor(destination)?;
        let moved = io.snapshot()?;
        if !at_destination(moved.cursor_pos, destination) {
            return Err(format!(
                "Hyprland cursor canary missed requested destination {destination:?}: {:?}",
                moved.cursor_pos
            ));
        }
        Ok(())
    })();

    // Execute every restoration step even after failed input or observation.
    // Preserve both the original error and cleanup errors; never certify a
    // successful move with an unverified restoration.
    let cursor_restore = io.move_cursor(original);
    let focus_restore = io.restore_focus();
    let restored = io.snapshot();
    let mut errors: Vec<String> = [result, cursor_restore, focus_restore]
        .into_iter()
        .filter_map(Result::err)
        .collect();
    match restored {
        Ok(restored) => {
            if restored.cursor_pos != Some(original) || !same_posture(&before, &restored) {
                errors.push(format!("Hyprland cursor/focus/z-order restoration mismatch: {before:?} -> {restored:?}"));
            }
        }
        Err(error) => errors.push(format!(
            "Hyprland restoration could not be verified: {error}"
        )),
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

#[cfg(target_os = "linux")]
pub(super) fn is_session() -> bool {
    (super::is_wayland_session() || std::env::var_os("WAYLAND_DISPLAY").is_some())
        && (std::env::var("CUA_E2E_WAYLAND_SESSION")
            .is_ok_and(|session| session.eq_ignore_ascii_case("hyprland"))
            || std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some())
}

#[cfg(target_os = "linux")]
pub(super) fn activate(
    driver: &mut impl crate::Driver,
    target: crate::observer::TargetWindow,
) -> Result<(), String> {
    use crate::observer::{DesktopObserver, NativeObserver};
    // Resolve exact identity independently before Driver activation. A stale
    // native id or ambiguous pid must not focus another client.
    crate::observer::linux::hyprland_target_address(target).map_err(|error| error.to_string())?;
    let observer = DesktopObserver::new(NativeObserver::new(), target);
    observer.snapshot().map_err(|error| error.to_string())?;
    let response = driver.call(
        "bring_to_front",
        serde_json::json!({
            "pid": target.pid, "window_id": target.native_id,
        }),
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let after = observer.snapshot().map_err(|error| error.to_string())?;
        if response.is_error() {
            return Err(format!(
                "Hyprland Driver activation failed: {}",
                response.text()
            ));
        }
        if after.target_z == TargetZ::Foreground
            && after.foreground == Some(target.native_id)
            && after.input_focus == Some(target.native_id)
        {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!(
                "Hyprland Driver activation did not focus target: {after:?}"
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

#[cfg(target_os = "linux")]
pub(super) fn calibrate(
    driver: &mut impl crate::Driver,
    target: crate::observer::TargetWindow,
) -> Result<(), String> {
    struct NativeIo<'a, D> {
        driver: &'a mut D,
        target: crate::observer::TargetWindow,
    }
    impl<D: crate::Driver> CalibrationIo for NativeIo<'_, D> {
        fn snapshot(&mut self) -> Result<DesktopSnapshot, String> {
            use crate::observer::{DesktopObserver, NativeObserver};
            // Bracket the explicit Driver action with fresh Driver perception,
            // while retaining compositor queries as the independent oracle.
            let state = self.driver.call("get_desktop_state", serde_json::json!({}));
            if state.is_error() {
                return Err(format!(
                    "Hyprland calibration perception failed: {}",
                    state.text()
                ));
            }
            let snapshot = DesktopObserver::new(NativeObserver::new(), self.target)
                .snapshot()
                .map_err(|error| error.to_string())?;
            let cursor = crate::observer::linux::hyprland_cursor_position()
                .map_err(|error| error.to_string())?;
            if snapshot.cursor_pos != Some(cursor) {
                return Err("Hyprland cursor sample was unstable".into());
            }
            Ok(snapshot)
        }
        fn regions(&mut self) -> Result<Vec<Region>, String> {
            crate::observer::linux::hyprland_monitor_regions().map_err(|error| error.to_string())
        }
        fn move_cursor(&mut self, point: Point) -> Result<(), String> {
            let response = self.driver.call(
                "move_cursor",
                serde_json::json!({
                    "x": point.0, "y": point.1, "scope": "desktop",
                }),
            );
            if response.is_error() {
                Err(format!(
                    "Hyprland explicit desktop cursor move failed: {}",
                    response.text()
                ))
            } else {
                std::thread::sleep(std::time::Duration::from_millis(100));
                Ok(())
            }
        }
        fn restore_focus(&mut self) -> Result<(), String> {
            activate(self.driver, self.target)
        }
    }
    calibrate_io(&mut NativeIo { driver, target })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    fn state(cursor: Point) -> DesktopSnapshot {
        DesktopSnapshot {
            foreground: Some(7),
            input_focus: Some(7),
            target_z: TargetZ::Foreground,
            cursor_pos: Some(cursor),
        }
    }
    struct FakeIo {
        snapshots: VecDeque<Result<DesktopSnapshot, String>>,
        moves: Vec<Point>,
        focus_restored: bool,
        fail_move: bool,
        fail_restore: bool,
    }
    impl CalibrationIo for FakeIo {
        fn snapshot(&mut self) -> Result<DesktopSnapshot, String> {
            self.snapshots.pop_front().unwrap()
        }
        fn regions(&mut self) -> Result<Vec<Region>, String> {
            Ok(vec![(0.0, 0.0, 100.0, 100.0)])
        }
        fn move_cursor(&mut self, point: Point) -> Result<(), String> {
            self.moves.push(point);
            if self.fail_move || (self.fail_restore && self.moves.len() == 2) {
                Err("injected move failure".into())
            } else {
                Ok(())
            }
        }
        fn restore_focus(&mut self) -> Result<(), String> {
            self.focus_restored = true;
            Ok(())
        }
    }
    fn io(samples: Vec<Result<DesktopSnapshot, String>>) -> FakeIo {
        FakeIo {
            snapshots: samples.into(),
            moves: vec![],
            focus_restored: false,
            fail_move: false,
            fail_restore: false,
        }
    }
    #[test]
    fn destination_uses_real_scaled_monitor_with_negative_origin() {
        let regions = [
            (-1280.0, -720.0, 1280.0, 720.0),
            (200.0, 0.0, 1920.0, 1080.0),
        ];
        assert_eq!(
            canary_destination((-640.0, -360.0), &regions).unwrap(),
            (-960.0, -540.0)
        );
        assert!(canary_destination((100.0, 100.0), &regions).is_err());
    }
    #[test]
    fn destination_rejects_invalid_or_tiny_regions() {
        for region in [
            (0.0, 0.0, f64::NAN, 100.0),
            (0.0, 0.0, 1.0, 1.0),
            (0.0, 0.0, -1.0, 100.0),
        ] {
            assert!(canary_destination((0.0, 0.0), &[region]).is_err());
        }
    }
    #[test]
    fn calibration_requires_destination_and_verified_restore() {
        let original = state((50.0, 50.0));
        let mut io = io(vec![
            Ok(original.clone()),
            Ok(original.clone()),
            Ok(state((25.0, 25.0))),
            Ok(original),
        ]);
        assert!(calibrate_io(&mut io).is_ok());
        assert_eq!(io.moves, [(25.0, 25.0), (50.0, 50.0)]);
        assert!(io.focus_restored);
    }
    #[test]
    fn arbitrary_motion_or_query_error_never_calibrates_and_always_restores() {
        for moved in [Ok(state((60.0, 60.0))), Err("query failed".into())] {
            let original = state((50.0, 50.0));
            let mut io = io(vec![
                Ok(original.clone()),
                Ok(original.clone()),
                moved,
                Ok(original),
            ]);
            assert!(calibrate_io(&mut io).is_err());
            assert_eq!(io.moves.last(), Some(&(50.0, 50.0)));
            assert!(io.focus_restored);
        }
    }
    #[test]
    fn negative_sample_failure_still_restores() {
        let original = state((50.0, 50.0));
        for negative in [Ok(state((51.0, 50.0))), Err("query failed".into())] {
            let mut io = io(vec![Ok(original.clone()), negative, Ok(original.clone())]);
            assert!(calibrate_io(&mut io).is_err());
            assert_eq!(io.moves, [(50.0, 50.0)]);
            assert!(io.focus_restored);
        }
    }
    #[test]
    fn failed_move_still_attempts_all_cleanup() {
        let original = state((50.0, 50.0));
        let mut io = io(vec![
            Ok(original.clone()),
            Ok(original.clone()),
            Ok(original),
        ]);
        io.fail_move = true;
        assert!(calibrate_io(&mut io).is_err());
        assert_eq!(io.moves.len(), 2);
        assert!(io.focus_restored);
    }
    #[test]
    fn restoration_query_posture_or_move_failure_is_fatal() {
        let original = state((50.0, 50.0));
        let mut changed = original.clone();
        changed.foreground = Some(8);
        for restored in [
            Err("restore query failed".into()),
            Ok(changed),
            Ok(state((40.0, 40.0))),
            Ok(state((50.5, 50.0))),
        ] {
            let mut io = io(vec![
                Ok(original.clone()),
                Ok(original.clone()),
                Ok(state((25.0, 25.0))),
                restored,
            ]);
            assert!(calibrate_io(&mut io).is_err());
        }
        let mut io = io(vec![
            Ok(original.clone()),
            Ok(original.clone()),
            Ok(state((25.0, 25.0))),
            Ok(original),
        ]);
        io.fail_restore = true;
        assert!(calibrate_io(&mut io).is_err());
    }
}

//! The pointer gate that checks clicks and drags against niri's current
//! layout. [`prepare_target`] is the one entry point for pointer input;
//! [`PointerGate::holds`] rechecks before every later event.

use super::*;

/// Recent window-local to layout translations kept for the pointer gate,
/// oldest evicted first. Tool bodies translate coordinates with [`aim`] and
/// call the pointer route separately, so this cache is what carries the
/// translated rectangle to [`gate_pointer`] until those bodies can pass it
/// (the tool split that follows this adapter).
const MAX_AIMS: usize = 64;

struct Aim {
    window: u64,
    point: (i32, i32),
    rect: Rect,
}

fn aims() -> &'static Mutex<Vec<Aim>> {
    static AIMS: OnceLock<Mutex<Vec<Aim>>> = OnceLock::new();
    AIMS.get_or_init(|| Mutex::new(Vec::new()))
}

fn placed_or_refuse(window: &Window) -> Result<&Placed> {
    window.placement.as_ref().map_err(|unplaced| {
        foreground_unavailable(format!("niri window {}: {}", window.id, unplaced.reason()))
    })
}

/// The refusal for window-coordinate input on `window_id` when niri shows it
/// tiled, so a caller without a screenshot of it learns why it has none.
pub fn tiled_refusal(window_id: u64) -> Option<anyhow::Error> {
    let scene = ipc().scene().ok()?;
    let window = scene.window(window_id).ok()?;
    match &window.placement {
        Err(Unplaced::Tiled) => placed_or_refuse(&window).err(),
        _ => None,
    }
}

/// Translate window coordinates into the global layout. A floating window's
/// coordinates are window-local; the geometry used is remembered so the
/// pointer gate can prove it still holds. A window without a place (a tiled
/// one) refuses instead of passing `(x, y)` through as output coordinates.
pub fn aim(window_id: u64, x: i32, y: i32) -> Result<(i32, i32)> {
    let scene = ipc().scene().map_err(input_unavailable)?;
    single_output_pointer(&scene)?;
    let window = scene.window(window_id).map_err(input_unavailable)?;
    let (point, rect) = aim_at(&window, x, y)?;
    if let Ok(mut aims) = aims().lock() {
        aims.retain(|aim| (aim.window, aim.point) != (window_id, point));
        if aims.len() >= MAX_AIMS {
            aims.remove(0);
        }
        aims.push(Aim {
            window: window_id,
            point,
            rect,
        });
    }
    Ok(point)
}

/// Window-addressed pointer input needs a single output: with several, the
/// virtual pointer's absolute motion spans their bounding box, and nothing
/// has shown that niri maps it the way the driver computes it.
pub(super) fn single_output_pointer(scene: &Scene) -> Result<()> {
    match scene.laid_out().len() {
        0 | 1 => Ok(()),
        outputs => Err(foreground_unavailable(format!(
            "niri window-coordinate pointer input supports a single output; this layout has {outputs}"
        ))),
    }
}

pub(super) fn aim_at(window: &Window, x: i32, y: i32) -> Result<((i32, i32), Rect)> {
    let rect = placed_or_refuse(window)?.rect;
    Ok(((rect.x.saturating_add(x), rect.y.saturating_add(y)), rect))
}

/// A pointer action admitted on the current layout. [`Self::holds`] reads
/// the layout again before every event, so a window that moved, or a pointer
/// space that changed, refuses the next one. Two equal snapshots cannot see a
/// move away and back between them, nor an animation niri does not report.
pub struct PointerGate {
    ipc: AttestedIpc,
    /// The window and the rectangle its points were admitted on.
    window: Option<(u64, Rect)>,
    points: Vec<(i32, i32)>,
    /// The pointer space the action's session scales motions by.
    space: (i32, i32, u32, u32),
}

impl PointerGate {
    /// Recheck the admitted action right before an event that acts on the
    /// target: the first motion, the first press, and each wheel tick.
    pub fn holds(&mut self) -> Result<()> {
        if let Some((id, rect)) = self.window {
            let window = self
                .ipc
                .scene()
                .and_then(|scene| scene.window(id))
                .map_err(input_unavailable)?;
            pointer_admission(&window, rect, &self.points, |_| None)?;
        }
        let current =
            pointer_space_of(&self.ipc.query(Request::Outputs)?).map_err(input_unavailable)?;
        super::super::pointer_space_holds(self.space, current)
    }
}

/// What a niri pointer action addresses.
pub enum Target {
    /// A window by id; `activate` focuses it once the action is admitted.
    Window { id: u64, activate: bool },
    /// Pixels of the single output, as `get_desktop_state` shows it.
    Desktop,
    /// Nothing to check beyond the session and the pointer space.
    Unaddressed,
}

/// The one entry point for niri pointer input: refuse points the virtual
/// pointer in `space` cannot reach (`motion_absolute`
/// clamps into it, so a moved point lands elsewhere), admit `target` on the
/// current layout, and recheck it right before the first event. Call
/// [`PointerGate::holds`] before every later one.
pub fn prepare_target(
    target: Target,
    space: (i32, i32, u32, u32),
    points: &[(i32, i32)],
) -> Result<Option<PointerGate>> {
    let (origin_x, origin_y, width, height) = space;
    if let Some((x, y)) = points
        .iter()
        .copied()
        .find(|&(x, y)| !super::super::pointer_reaches(origin_x, origin_y, width, height, x, y))
    {
        bail!("foreground_unavailable: point ({x}, {y}) is outside the niri pointer space");
    }
    let mut gate = PointerGate {
        ipc: ipc(),
        window: None,
        points: points.to_vec(),
        space,
    };
    let scene = gate.ipc.scene().map_err(input_unavailable)?;
    match target {
        Target::Window { id, activate } => {
            let rect = gate_pointer(&scene, id, points)?;
            if activate {
                activate_in(&mut gate.ipc, id, None, FOCUS_CONFIRM_TIMEOUT)
                    .map_err(input_unavailable)?;
            }
            gate.window = Some((id, rect));
        }
        Target::Desktop => desktop_admission(&scene, points)?,
        Target::Unaddressed => return Ok(None),
    }
    gate.holds()?;
    Ok(Some(gate))
}

/// Admit window-addressed pointer input on a floating window: the target
/// must be placed where its coordinates were computed and every point must
/// land on it rather than on another floating window. A window without a
/// place (a tiled one) refuses. Returns the rectangle admitted.
fn gate_pointer(scene: &Scene, window_id: u64, points: &[(i32, i32)]) -> Result<Rect> {
    single_output_pointer(scene)?;
    let window = scene.window(window_id).map_err(input_unavailable)?;
    let before = placed_or_refuse(&window)?.rect;
    pointer_admission(&window, before, points, |point| {
        aims()
            .lock()
            .ok()?
            .iter()
            .rev()
            .find(|aim| (aim.window, aim.point) == (window_id, point))
            .map(|aim| aim.rect)
    })?;
    Ok(before)
}

fn pointer_admission(
    window: &Window,
    before: Rect,
    points: &[(i32, i32)],
    aimed: impl Fn((i32, i32)) -> Option<Rect>,
) -> Result<()> {
    let placed = placed_or_refuse(window)?;
    for &(x, y) in points {
        let expected = aimed((x, y)).unwrap_or(before);
        if placed.rect != expected {
            return Err(foreground_unavailable(format!(
                "niri window {} moved from {expected:?} to {:?} after its coordinates were computed; take a new screenshot",
                window.id, placed.rect
            )));
        }
        if !placed.covers(x, y) {
            return Err(foreground_unavailable(format!(
                "point ({x}, {y}) is outside niri window {} at {:?}",
                window.id, placed.rect
            )));
        }
        if !placed.area.contains(x, y) {
            return Err(foreground_unavailable(format!(
                "point ({x}, {y}) is past the edge of niri output {}, where window {} is not shown",
                placed.output, window.id
            )));
        }
        if !placed.hits(x, y) {
            return Err(foreground_unavailable(format!(
                "point ({x}, {y}) lies under another floating window, and niri reports no stacking order"
            )));
        }
    }
    Ok(())
}

/// Admit desktop-scope points: pixels of the single output, the frame a
/// `get_desktop_state` screenshot shows.
pub(super) fn desktop_admission(scene: &Scene, points: &[(i32, i32)]) -> Result<()> {
    let (output, logical) = scene.single_output().map_err(input_unavailable)?;
    let area = Rect {
        x: logical.x,
        y: logical.y,
        width: logical.width,
        height: logical.height,
    };
    match points.iter().find(|&&(x, y)| !area.contains(x, y)) {
        Some((x, y)) => Err(foreground_unavailable(format!(
            "point ({x}, {y}) is past the edge of niri output {}, which the get_desktop_state screenshot shows",
            output.name
        ))),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_that_moved_after_admission_refuses_the_next_event() {
        let admitted = one_output(vec![floating(7, 1, (100.0, 60.0), (300, 200))]);
        let rect = gate_pointer(&admitted, 7, &[(150, 100)]).unwrap();
        let moved = one_output(vec![floating(7, 1, (120.0, 60.0), (300, 200))]);
        assert_eq!(
            pointer_admission(&placed_window(&moved, 7), rect, &[(150, 100)], |_| None)
                .unwrap_err()
                .to_string(),
            "foreground_unavailable: niri window 7 moved from Rect { x: 100, y: 60, width: 300, height: 200 } to Rect { x: 120, y: 60, width: 300, height: 200 } after its coordinates were computed; take a new screenshot"
        );
        let covered = one_output(vec![
            floating(7, 1, (100.0, 60.0), (300, 200)),
            floating(8, 1, (140.0, 90.0), (50, 50)),
        ]);
        assert!(
            pointer_admission(&placed_window(&covered, 7), rect, &[(150, 100)], |_| None).is_err()
        );
    }

    // ── Pointer admission ───────────────────────────────────────────────────

    #[test]
    fn window_coordinate_pointer_input_refuses_several_outputs() {
        let two_outputs = scene(
            vec![floating(7, 1, (100.0, 60.0), (300, 200))],
            vec![
                workspace(1, "DP-1", true, None),
                workspace(2, "DP-2", true, None),
            ],
            vec![
                output("DP-1", 0, 0, (1920, 1080), 1.0, "Normal"),
                output("DP-2", 1920, 0, (1920, 1080), 1.0, "Normal"),
            ],
            false,
        );
        assert_eq!(
            single_output_pointer(&two_outputs).unwrap_err().to_string(),
            "foreground_unavailable: niri window-coordinate pointer input supports a single output; this layout has 2"
        );
        let one = one_output(vec![floating(7, 1, (100.0, 60.0), (300, 200))]);
        assert!(single_output_pointer(&one).is_ok());
    }

    #[test]
    fn floating_window_coordinates_add_the_window_origin() {
        let scene = one_output(vec![floating(7, 1, (100.0, 60.0), (300, 200))]);
        let (point, rect) = aim_at(&scene.window(7).unwrap(), 10, 20).unwrap();
        assert_eq!(point, (110, 80));
        assert_eq!(
            rect,
            Rect {
                x: 100,
                y: 60,
                width: 300,
                height: 200
            }
        );
    }

    fn placed_window(scene: &Scene, id: u64) -> Window {
        scene.window(id).unwrap()
    }

    #[test]
    fn pointer_admission_refuses_moved_windows_and_ambiguous_points() {
        let scene = one_output(vec![
            floating(7, 1, (100.0, 60.0), (300, 200)),
            floating(8, 1, (350.0, 200.0), (200, 100)),
        ]);
        let window = placed_window(&scene, 7);
        let rect = window.placement.as_ref().unwrap().rect;
        assert!(pointer_admission(&window, rect, &[(150, 100)], |_| None).is_ok());
        assert!(pointer_admission(&window, rect, &[(150, 100)], |_| Some(rect)).is_ok());
        let earlier = Rect { x: 0, ..rect };
        assert_eq!(
            pointer_admission(&window, rect, &[(150, 100)], |_| Some(earlier)).unwrap_err().to_string(),
            "foreground_unavailable: niri window 7 moved from Rect { x: 0, y: 60, width: 300, height: 200 } to Rect { x: 100, y: 60, width: 300, height: 200 } after its coordinates were computed; take a new screenshot"
        );
        assert_eq!(
            pointer_admission(&window, rect, &[(360, 210)], |_| None).unwrap_err().to_string(),
            "foreground_unavailable: point (360, 210) lies under another floating window, and niri reports no stacking order"
        );
        assert_eq!(
            pointer_admission(&window, rect, &[(150, 100), (5, 5)], |_| None).unwrap_err().to_string(),
            "foreground_unavailable: point (5, 5) is outside niri window 7 at Rect { x: 100, y: 60, width: 300, height: 200 }"
        );
        // Taller than the 853x480 output: its bottom is not shown.
        let scene = one_output(vec![floating(9, 1, (100.0, 60.0), (300, 500))]);
        let tall = placed_window(&scene, 9);
        let tall_rect = tall.placement.as_ref().unwrap().rect;
        assert_eq!(
            pointer_admission(&tall, tall_rect, &[(150, 500)], |_| None).unwrap_err().to_string(),
            "foreground_unavailable: point (150, 500) is past the edge of niri output winit, where window 9 is not shown"
        );
        let tiled = one_output(vec![tiled(5, 1, (402.0, 448.0))])
            .window(5)
            .unwrap();
        assert_eq!(
            pointer_admission(&tiled, rect, &[(150, 100)], |_| None)
                .unwrap_err()
                .to_string(),
            "foreground_unavailable: niri window 5: niri does not report where a tiled window is; use get_desktop_state and a desktop-scope action (needs the display grant), or float the window"
        );
    }

    #[test]
    fn a_tiled_window_refuses_window_capture_and_window_coordinates() {
        // Even the focused active tiled window: its neighbour shares the
        // output, and niri cannot say which pixels are whose.
        let scene = tiled_scene(Some(5));
        assert_eq!(
            capture_plan_in(&scene, 5, Some(105)),
            Err("niri does not report where a tiled window is; use get_desktop_state and a desktop-scope action (needs the display grant), or float the window")
        );
        let window = scene.window(5).unwrap();
        assert_eq!(
            aim_at(&window, 10, 20).unwrap_err().to_string(),
            "foreground_unavailable: niri window 5: niri does not report where a tiled window is; use get_desktop_state and a desktop-scope action (needs the display grant), or float the window"
        );
    }

    #[test]
    fn desktop_pixels_are_admitted_only_on_the_single_output() {
        let scene = tiled_scene(Some(5));
        desktop_admission(&scene, &[(2500, 400), (2100, 100)]).unwrap();
        assert_eq!(
            desktop_admission(&scene, &[(10, 100)])
                .unwrap_err()
                .to_string(),
            "foreground_unavailable: point (10, 100) is past the edge of niri output winit, which the get_desktop_state screenshot shows"
        );
    }
}

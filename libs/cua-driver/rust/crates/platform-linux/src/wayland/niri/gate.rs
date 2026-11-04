//! The pointer gate that checks clicks and drags against niri's current
//! layout. [`prepare_target`] is the one entry point for pointer input;
//! [`PointerGate::holds`] rechecks before every later event.

use super::*;
use crate::interaction::policy::{self, Miss, Point};

/// A layout point for pointer input and, when it was translated from window
/// coordinates on niri, the window rectangle that translation used. The
/// pointer gate refuses the point if the window no longer has that rectangle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AimedPoint {
    pub point: (i32, i32),
    pub rect: Option<Rect>,
}

impl From<(i32, i32)> for AimedPoint {
    fn from(point: (i32, i32)) -> Self {
        Self { point, rect: None }
    }
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
/// coordinates are window-local; the rectangle used travels with the point so
/// the pointer gate can prove it still holds. A window without a place (a
/// tiled one) refuses instead of passing `(x, y)` through as output
/// coordinates.
pub fn aim(window_id: u64, x: i32, y: i32) -> Result<AimedPoint> {
    let scene = ipc().scene().map_err(input_unavailable)?;
    single_output_pointer(&scene)?;
    let window = scene.window(window_id).map_err(input_unavailable)?;
    aim_at(&window, x, y)
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

pub(super) fn aim_at(window: &Window, x: i32, y: i32) -> Result<AimedPoint> {
    let rect = placed_or_refuse(window)?.rect;
    Ok(AimedPoint {
        point: (rect.x.saturating_add(x), rect.y.saturating_add(y)),
        rect: Some(rect),
    })
}

/// A pointer action admitted on the current layout. [`Self::holds`] reads
/// the layout again before every event, so a window that moved, or a pointer
/// space that changed, refuses the next one. Two equal snapshots cannot see a
/// move away and back between them, nor an animation niri does not report.
pub struct PointerGate<C = fn(Instant) -> Result<UnixStream>> {
    ipc: Ipc<C>,
    window: Option<u64>,
    /// The points, each carrying the rectangle it was admitted on.
    points: Vec<AimedPoint>,
    /// The pointer space the action's session scales motions by.
    space: (i32, i32, u32, u32),
}

impl<C: FnMut(Instant) -> Result<UnixStream>> PointerGate<C> {
    /// Recheck the admitted action right before an event that acts on the
    /// target: the first motion, the first press, and each wheel tick. A
    /// window target must also still hold keyboard focus, read back from
    /// niri: a lock or layer-shell surface can take it without moving it.
    pub fn holds(&mut self) -> Result<()> {
        if let Some(id) = self.window {
            let scene = self.ipc.scene().map_err(input_unavailable)?;
            let window = scene.window(id).map_err(input_unavailable)?;
            pointer_admission(&window, &self.points)?;
            let pid = scene
                .windows
                .iter()
                .find(|w| w.id == id)
                .and_then(|w| w.pid);
            confirm_focus(
                &mut self.ipc,
                id,
                pid,
                interaction::operation_deadline(FOCUS_CONFIRM_TIMEOUT),
            )?;
        }
        let current =
            pointer_space_of(&self.ipc.query(Request::Outputs)?).map_err(input_unavailable)?;
        super::super::pointer_space_holds(self.space, current)
    }
}

/// What a niri pointer action addresses.
pub enum Target {
    /// A window by id, focused once the action is admitted.
    Window(u64),
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
    points: &[AimedPoint],
) -> Result<Option<PointerGate>> {
    let (origin_x, origin_y, width, height) = space;
    if let Some((x, y)) = points
        .iter()
        .map(|aimed| aimed.point)
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
    let scene = match target {
        Target::Window(id) => settled_window(|deadline| gate.ipc.scene_until(deadline), id),
        _ => gate.ipc.scene(),
    }
    .map_err(input_unavailable)?;
    match target {
        Target::Window(id) => {
            let rect = gate_pointer(&scene, id, points)?;
            activate_in(&mut gate.ipc, id, None, FOCUS_CONFIRM_TIMEOUT)
                .map_err(input_unavailable)?;
            gate.window = Some(id);
            gate.points = admitted_on(points, rect);
        }
        Target::Desktop => desktop_admission(&scene, &gate.points)?,
        Target::Unaddressed => return Ok(None),
    }
    gate.holds()?;
    Ok(Some(gate))
}

fn settled_window(scene: impl FnMut(Instant) -> Result<Scene>, id: u64) -> Result<Scene> {
    settled(
        scene,
        |scene| scene.window(id).map(|window| window.placement).ok(),
        || {
            foreground_unavailable(format!(
                "niri window {id} is still moving; take a new screenshot"
            ))
        },
    )
}

/// Admit window-addressed pointer input on a floating window: the target
/// must still have the rectangle each point was translated against, and
/// every point must land on it rather than on another floating window. A
/// point given in layout coordinates carries no rectangle to compare. A
/// window without a place (a tiled one) refuses. Returns the rectangle
/// admitted.
fn gate_pointer(scene: &Scene, window_id: u64, points: &[AimedPoint]) -> Result<Rect> {
    single_output_pointer(scene)?;
    let window = scene.window(window_id).map_err(input_unavailable)?;
    pointer_admission(&window, points)?;
    Ok(placed_or_refuse(&window)?.rect)
}

/// `points` as admitted on `rect`: later rechecks compare every point with it.
fn admitted_on(points: &[AimedPoint], rect: Rect) -> Vec<AimedPoint> {
    points
        .iter()
        .map(|aimed| AimedPoint {
            rect: Some(rect),
            ..*aimed
        })
        .collect()
}

fn pointer_admission(window: &Window, points: &[AimedPoint]) -> Result<()> {
    let placed = placed_or_refuse(window)?;
    for &AimedPoint {
        point: (x, y),
        rect,
    } in points
    {
        let moved_from = rect.filter(|expected| {
            !policy::fresh(expected, &interaction::facts::Fact::Known(placed.rect))
        });
        let point = Point {
            fresh: moved_from.is_none(),
            on_window: placed.covers(x, y),
            shown: placed.area.contains(x, y),
            unobscured: placed.hits(x, y),
        };
        let reason = match policy::reach(&point) {
            Ok(()) => continue,
            Err(Miss::Stale) => format!(
                "niri window {} moved from {:?} to {:?} after its coordinates were computed; take a new screenshot",
                window.id,
                moved_from.unwrap_or(placed.rect),
                placed.rect
            ),
            Err(Miss::OffWindow) => format!(
                "point ({x}, {y}) is outside niri window {} at {:?}",
                window.id, placed.rect
            ),
            Err(Miss::Hidden) => format!(
                "point ({x}, {y}) is past the edge of niri output {}, where window {} is not shown",
                placed.output, window.id
            ),
            Err(Miss::Obscured) => format!(
                "point ({x}, {y}) lies under another floating window, and niri reports no stacking order"
            ),
        };
        return Err(foreground_unavailable(reason));
    }
    Ok(())
}

/// Admit desktop-scope points: pixels of the single output, the frame a
/// `get_desktop_state` screenshot shows.
pub(super) fn desktop_admission(scene: &Scene, points: &[AimedPoint]) -> Result<()> {
    let (output, logical) = scene.single_output().map_err(input_unavailable)?;
    let area = Rect {
        x: logical.x,
        y: logical.y,
        width: logical.width,
        height: logical.height,
    };
    match points
        .iter()
        .map(|aimed| aimed.point)
        .find(|&(x, y)| !area.contains(x, y))
    {
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
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    fn aimed_on(point: (i32, i32), rect: Rect) -> AimedPoint {
        AimedPoint {
            point,
            rect: Some(rect),
        }
    }

    #[test]
    fn a_window_that_moved_after_admission_refuses_the_next_event() {
        let admitted = one_output(vec![floating(7, 1, (100.0, 60.0), (300, 200))]);
        let rect = gate_pointer(&admitted, 7, &[(150, 100).into()]).unwrap();
        let points = admitted_on(&[(150, 100).into()], rect);
        let moved = one_output(vec![floating(7, 1, (120.0, 60.0), (300, 200))]);
        assert_eq!(
            pointer_admission(&placed_window(&moved, 7), &points)
                .unwrap_err()
                .to_string(),
            "foreground_unavailable: niri window 7 moved from Rect { x: 100, y: 60, width: 300, height: 200 } to Rect { x: 120, y: 60, width: 300, height: 200 } after its coordinates were computed; take a new screenshot"
        );
        let covered = one_output(vec![
            floating(7, 1, (100.0, 60.0), (300, 200)),
            floating(8, 1, (140.0, 90.0), (50, 50)),
        ]);
        assert!(pointer_admission(&placed_window(&covered, 7), &points).is_err());
    }

    #[test]
    fn focus_taken_without_any_move_refuses_the_next_event() {
        let mut target = floating(7, 1, (100.0, 60.0), (300, 200));
        target["is_focused"] = json!(true);
        let fake = Arc::new(Mutex::new(Fake {
            windows: vec![target],
            workspaces: vec![workspace(1, "winit", true, Some(7))],
            outputs: vec![output("winit", 0, 0, (1280, 720), 1.5, "Flipped180")],
            ..Fake::default()
        }));
        let admitted = one_output(fake.lock().unwrap().windows.clone());
        let rect = gate_pointer(&admitted, 7, &[(150, 100).into()]).unwrap();
        let mut gate = PointerGate {
            ipc: fake_ipc(&fake),
            window: Some(7),
            points: admitted_on(&[(150, 100).into()], rect),
            space: admitted.pointer_space().unwrap(),
        };
        gate.holds().unwrap();
        fake.lock().unwrap().windows[0]["is_focused"] = json!(false);
        assert_eq!(
            gate.holds().unwrap_err().to_string(),
            "foreground_unavailable: niri did not confirm focus on window 7 (focused: none)"
        );
    }

    #[test]
    fn window_pointer_input_waits_for_a_settled_window_and_refuses_one_that_keeps_moving() {
        let at = |x: f64| Ok(one_output(vec![floating(7, 1, (x, 60.0), (300, 200))]));
        let mut reads = 0;
        let settling = settled_window(
            |_| {
                reads += 1;
                at(if reads == 1 { 90.0 } else { 100.0 })
            },
            7,
        );
        assert_eq!(
            placed_window(&settling.unwrap(), 7)
                .placement
                .unwrap()
                .rect
                .x,
            100
        );
        let mut x = 0.0;
        let moving = settled_window(
            |_| {
                x += 10.0;
                at(x)
            },
            7,
        );
        assert_eq!(
            moving.err().unwrap().to_string(),
            "foreground_unavailable: niri window 7 is still moving; take a new screenshot"
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
        let aimed = aim_at(&scene.window(7).unwrap(), 10, 20).unwrap();
        assert_eq!(
            aimed,
            aimed_on(
                (110, 80),
                Rect {
                    x: 100,
                    y: 60,
                    width: 300,
                    height: 200
                }
            )
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
        assert!(pointer_admission(&window, &[(150, 100).into()]).is_ok());
        assert!(pointer_admission(&window, &[aimed_on((150, 100), rect)]).is_ok());
        let earlier = Rect { x: 0, ..rect };
        assert_eq!(
            pointer_admission(&window, &[aimed_on((150, 100), earlier)]).unwrap_err().to_string(),
            "foreground_unavailable: niri window 7 moved from Rect { x: 0, y: 60, width: 300, height: 200 } to Rect { x: 100, y: 60, width: 300, height: 200 } after its coordinates were computed; take a new screenshot"
        );
        assert_eq!(
            pointer_admission(&window, &[(360, 210).into()]).unwrap_err().to_string(),
            "foreground_unavailable: point (360, 210) lies under another floating window, and niri reports no stacking order"
        );
        assert_eq!(
            pointer_admission(&window, &[(150, 100).into(), (5, 5).into()]).unwrap_err().to_string(),
            "foreground_unavailable: point (5, 5) is outside niri window 7 at Rect { x: 100, y: 60, width: 300, height: 200 }"
        );
        // Taller than the 853x480 output: its bottom is not shown.
        let scene = one_output(vec![floating(9, 1, (100.0, 60.0), (300, 500))]);
        let tall = placed_window(&scene, 9);
        assert_eq!(
            pointer_admission(&tall, &[(150, 500).into()]).unwrap_err().to_string(),
            "foreground_unavailable: point (150, 500) is past the edge of niri output winit, where window 9 is not shown"
        );
        let tiled = one_output(vec![tiled(5, 1, (402.0, 448.0))])
            .window(5)
            .unwrap();
        assert_eq!(
            pointer_admission(&tiled, &[aimed_on((150, 100), rect)])
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
        desktop_admission(&scene, &[(2500, 400).into(), (2100, 100).into()]).unwrap();
        assert_eq!(
            desktop_admission(&scene, &[(10, 100).into()])
                .unwrap_err()
                .to_string(),
            "foreground_unavailable: point (10, 100) is past the edge of niri output winit, which the get_desktop_state screenshot shows"
        );
    }
}

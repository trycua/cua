//! Desktop frames and the pointer gates that check clicks and drags against
//! niri's layout. [`prepare_target`] is the one entry point for pointer
//! input; [`PointerGate::holds`] rechecks before every later event.

use super::*;

/// Recent window-local to layout translations kept for the pointer gate,
/// oldest evicted first. Tool bodies translate coordinates with [`aim`] and
/// call the pointer route separately, so this cache is what carries the
/// translated rectangle to [`gate_pointer`] until those bodies can pass it
/// (the tool split that follows this adapter).
const MAX_AIMS: usize = 64;
/// How long a `get_desktop_state` screenshot stays usable for desktop-scope
/// pointer input. niri scrolls a view without reporting it (see [`Shown`]),
/// so an unchanged layout proves less the older the screenshot is.
const FRAME_MAX_AGE: Duration = Duration::from_secs(60);

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
    window.placement.as_ref().map_err(|unplaced| match unplaced {
        Unplaced::Tiled => foreground_unavailable(format!(
            "niri does not report where tiled window {} is; use get_desktop_state and a desktop-scope action (needs the display grant), or float the window",
            window.id
        )),
        unplaced => {
            foreground_unavailable(format!("niri window {}: {}", window.id, unplaced.reason()))
        }
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

/// A window-addressed pointer action admitted on a settled layout. Each
/// confirmation reads the layout history again, so a change after admission
/// refuses the next event.
pub struct PointerGate {
    history: &'static History,
    mark: Mark,
    expected: Expected,
    ipc: AttestedIpc,
    /// The `get_desktop_state` frame a desktop-scope gate admitted against.
    desktop: Option<DesktopFrame>,
    /// The pointer space the action's session scales motions by.
    space: (i32, i32, u32, u32),
}

impl PointerGate {
    /// Refuse a desktop-scope drag from layout point `(x, y)` whose press
    /// would stop it (see [`DesktopFrame::drag_start`]). A window gate
    /// focused its window already. Call before the press.
    pub fn drag_start(&self, x: i32, y: i32) -> Result<()> {
        match &self.desktop {
            Some(frame) => frame.drag_start(x, y, self.expected),
            None => Ok(()),
        }
    }

    /// Refuse when the layout changed after admission. Call right before the
    /// first motion and again right before every press or wheel event.
    pub fn confirm(&mut self) -> Result<()> {
        if self
            .history
            .changed_since(self.mark, self.expected)
            .map_err(input_unavailable)?
        {
            return Err(foreground_unavailable(
                "the niri layout changed after the pointer target was checked; take a new screenshot",
            ));
        }
        Ok(())
    }

    /// Recheck the admitted action right before an event that acts on the
    /// target: the first motion, the first press, and each wheel tick. niri
    /// sends no event when outputs change, so the pointer space the session
    /// scales motions by is reread too, before the layout history: niri
    /// answers only after handling what it received before, so a window
    /// that closed by then is in the history read next.
    pub fn holds(&mut self) -> Result<()> {
        let current =
            pointer_space_of(&self.ipc.query(Request::Outputs)?).map_err(input_unavailable)?;
        self.confirm()?;
        super::super::pointer_space_holds(self.space, current)
    }
}

/// What a niri pointer action addresses.
pub enum Target<'a> {
    /// A window by id; `activate` focuses it once the action is admitted.
    Window { id: u64, activate: bool },
    /// Pixels of the `get_desktop_state` screenshot named by the capture id,
    /// or of the latest one.
    Desktop(Option<&'a str>),
    /// Nothing to check beyond the session and the pointer space.
    Unaddressed,
}

/// The one entry point for niri pointer input: refuse a locked session and
/// points the virtual pointer in `space` cannot reach (`motion_absolute`
/// clamps into it, so a moved point lands elsewhere), admit `target` (see
/// [`gate_pointer`] and [`desktop_gate`]), and recheck it right before the
/// first event. Call [`PointerGate::holds`] before every later one.
pub fn prepare_target(
    target: Target,
    space: (i32, i32, u32, u32),
    points: &[(i32, i32)],
) -> Result<Option<PointerGate>> {
    refuse_if_locked().map_err(input_unavailable)?;
    let (origin_x, origin_y, width, height) = space;
    if let Some((x, y)) = points
        .iter()
        .copied()
        .find(|&(x, y)| !super::super::pointer_reaches(origin_x, origin_y, width, height, x, y))
    {
        bail!("foreground_unavailable: point ({x}, {y}) is outside the niri pointer space");
    }
    let mut gate = match target {
        Target::Window { id, activate } => gate_pointer(id, points, activate, space)?,
        Target::Desktop(capture_id) => desktop_gate(points, capture_id, space)?,
        Target::Unaddressed => return Ok(None),
    };
    gate.holds()?;
    Ok(Some(gate))
}

/// Gate window-addressed pointer input on a floating window: wait until the
/// layout is quiet, then require the target to be placed where its
/// coordinates were computed and every point to land on it rather than on
/// another floating window. A window without a place (a tiled one) refuses.
/// With `activate`, focus the window only once all of that holds, so a
/// refused action leaves focus alone.
fn gate_pointer(
    window_id: u64,
    points: &[(i32, i32)],
    activate: bool,
    space: (i32, i32, u32, u32),
) -> Result<PointerGate> {
    let mut ipc = ipc();
    let scene = ipc.scene().map_err(input_unavailable)?;
    single_output_pointer(&scene)?;
    let window = scene.window(window_id).map_err(input_unavailable)?;
    let before = placed_or_refuse(&window)?.rect;
    let mark = settled().map_err(input_unavailable)?;
    let window = ipc
        .scene()
        .and_then(|scene| scene.window(window_id))
        .map_err(input_unavailable)?;
    pointer_admission(&window, before, points, |point| {
        aims()
            .lock()
            .ok()?
            .iter()
            .rev()
            .find(|aim| (aim.window, aim.point) == (window_id, point))
            .map(|aim| aim.rect)
    })?;
    // Changes queued while the admission scene was read refuse here, before
    // focus moves.
    let mut gate = PointerGate {
        history: &HISTORY,
        mark,
        expected: Expected::default(),
        ipc,
        desktop: None,
        space,
    };
    gate.confirm()?;
    if activate {
        gate.expected = Expected {
            window: Some(window_id),
            workspace: window.workspace,
        };
        activate_in(&mut gate.ipc, window_id, None, FOCUS_CONFIRM_TIMEOUT)
            .map_err(input_unavailable)?;
    }
    Ok(gate)
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

// ── Desktop frames ───────────────────────────────────────────────────────────

/// What one output shows, as far as niri reports it: the output, its active
/// workspace with that workspace's active window and every window's layout,
/// the focused window and the overview. niri can also scroll the view and
/// so move every tiled window without changing any of this or sending any
/// event: the `center-column`, `center-visible-columns` and `center-window`
/// actions, and a touchpad or mouse view gesture that ends on the same
/// column. Nothing in IPC detects those; [`FRAME_MAX_AGE`] bounds them.
#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    output: IpcOutput,
    workspace: u64,
    active: Option<u64>,
    focused: Option<u64>,
    overview_open: bool,
    windows: Vec<(u64, Option<IpcLayout>)>,
}

impl Scene {
    fn shown(&self, output: &str) -> Option<Shown> {
        let workspace = self
            .workspaces
            .iter()
            .find(|workspace| workspace.is_active && workspace.output.as_deref() == Some(output))?;
        let mut windows: Vec<_> = self
            .windows
            .iter()
            .filter(|window| window.workspace_id == Some(workspace.id))
            .map(|window| (window.id, window.layout.clone()))
            .collect();
        windows.sort_by_key(|(id, _)| *id);
        Some(Shown {
            output: self.outputs.get(output)?.clone(),
            workspace: workspace.id,
            active: workspace.active_window_id,
            focused: self
                .windows
                .iter()
                .find(|window| window.is_focused)
                .map(|window| window.id),
            overview_open: self.overview_open,
            windows,
        })
    }
}

/// The pixel frame of a niri `get_desktop_state` screenshot. Desktop-scope
/// pointer points are its pixels, the single output's logical pixels. niri
/// does not report where tiled windows are, so a point is admitted only
/// while niri's event stream reports no layout change after `mark`, taken
/// once the layout had settled before the capture, the output still shows
/// what `shown` recorded, and the screenshot is at most [`FRAME_MAX_AGE`]
/// old. That proves only that niri reported no change: a view scroll niri
/// does not report (see [`Shown`]) or an animation longer than
/// [`SETTLE_QUIET`] still moves the pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct DesktopFrame {
    area: Rect,
    shown: Shown,
    mark: Mark,
    /// When the capture started.
    taken: Instant,
}

impl DesktopFrame {
    fn output(&self) -> &str {
        &self.shown.output.name
    }

    fn changed(&self, why: &str) -> anyhow::Error {
        foreground_unavailable(format!(
            "{why} after the get_desktop_state screenshot, so its pixels may be elsewhere now; take a new screenshot"
        ))
    }

    pub(super) fn holds(&self, scene: &Scene, history: &History) -> Result<()> {
        if self.taken.elapsed() > FRAME_MAX_AGE {
            return Err(foreground_unavailable(format!(
                "the get_desktop_state screenshot is over {} s old, and niri does not report every scroll of its view; take a new screenshot",
                FRAME_MAX_AGE.as_secs()
            )));
        }
        if scene.shown(self.output()).as_ref() != Some(&self.shown) {
            return Err(self.changed(&format!("niri output {} changed", self.output())));
        }
        if history
            .changed_since(self.mark, Expected::default())
            .map_err(input_unavailable)?
        {
            return Err(self.changed("the niri layout changed"));
        }
        Ok(())
    }

    /// Admit desktop-scope input at layout `points`, and say which focus
    /// change the input may make itself. A press on a floating window
    /// focuses and raises it without moving anything, so focus moving to
    /// the one floating window under every point is expected; focus moving
    /// to a tiled window can scroll the view, so it counts as a change.
    pub(super) fn admit(
        &self,
        scene: &Scene,
        points: &[(i32, i32)],
        history: &History,
    ) -> Result<Expected> {
        self.holds(scene, history)?;
        let mut under = HashSet::new();
        for &(x, y) in points {
            if !self.area.contains(x, y) {
                return Err(foreground_unavailable(format!(
                    "point ({x}, {y}) is past the edge of niri output {}, which the get_desktop_state screenshot shows",
                    self.output()
                )));
            }
            under.insert(match self.floating_under(x, y).as_slice() {
                [id] => Some(*id),
                _ => None,
            });
        }
        Ok(match under.into_iter().collect::<Vec<_>>().as_slice() {
            [Some(id)] => Expected {
                window: Some(*id),
                workspace: Some(self.shown.workspace),
            },
            _ => Expected::default(),
        })
    }
}

impl DesktopFrame {
    /// The floating windows whose tiles cover layout point `(x, y)`.
    fn floating_under(&self, x: i32, y: i32) -> Vec<u64> {
        let logical = self.shown.output.logical.as_ref();
        self.shown
            .windows
            .iter()
            .filter_map(|(id, layout)| Some((*id, floating_tile(logical?, layout.as_ref()?)?)))
            .filter(|(_, tile)| tile.contains(f64::from(x), f64::from(y)))
            .map(|(id, _)| id)
            .collect()
    }

    /// Refuse a drag whose press at `(x, y)` would move focus in a way
    /// [`Self::admit`] counts as a layout change: the motions after the
    /// press would then stop, and the app would get the press and release
    /// as a click. `expected` is the focus change [`Self::admit`] allowed
    /// for the drag's points. A press on the one floating window there is
    /// safe while that window has focus, or while the drag stays on it, so
    /// `admit` expects the focus the press gives it. Off every floating
    /// window, the press is on a tiled window or on
    /// no window, which is safe while a tiled window here has focus, or
    /// while nothing has focus and the workspace holds no tiled window (the
    /// press lands on empty space). niri does not say where tiled windows
    /// are, so a start on another tiled window than the focused one is not
    /// caught.
    fn drag_start(&self, x: i32, y: i32, expected: Expected) -> Result<()> {
        let focused = self.shown.focused;
        let tiled = |layout: &Option<IpcLayout>| {
            layout
                .as_ref()
                .is_some_and(|layout| layout.pos_in_scrolling_layout.is_some())
        };
        let keeps_focus = match self.floating_under(x, y).as_slice() {
            [id] => focused == Some(*id) || expected.window == Some(*id),
            [] => match focused {
                None => !self.shown.windows.iter().any(|(_, layout)| tiled(layout)),
                Some(focused) => self
                    .shown
                    .windows
                    .iter()
                    .any(|(id, layout)| *id == focused && tiled(layout)),
            },
            several => focused.is_some_and(|id| several.contains(&id)),
        };
        if keeps_focus {
            return Ok(());
        }
        Err(foreground_unavailable(
            "the drag starts where niri has not focused, and its press would focus that window and stop the drag after the press, which the app would see as a click; click there first, take a new get_desktop_state screenshot, then drag",
        ))
    }
}

/// The latest `get_desktop_state` screenshot's frame and capture id.
struct LatestDesktop {
    frame: DesktopFrame,
    capture: String,
}

fn latest_desktop() -> std::sync::MutexGuard<'static, Option<LatestDesktop>> {
    static LATEST: Mutex<Option<LatestDesktop>> = Mutex::new(None);
    LATEST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Take a desktop screenshot through `capture` on a settled layout, with
/// the frame its pixels use. The layout must stay unchanged through the
/// capture.
pub fn capture_desktop<T>(capture: impl FnOnce() -> Result<T>) -> Result<(T, DesktopFrame)> {
    let mut ipc = ipc();
    capture_desktop_in(
        || ipc.scene(),
        settled,
        |mark| HISTORY.changed_since(mark, Expected::default()),
        capture,
    )
}

pub(super) fn capture_desktop_in<T>(
    mut scene: impl FnMut() -> Result<Scene>,
    settle: impl FnOnce() -> Result<Mark>,
    changed_since: impl FnOnce(Mark) -> Result<bool>,
    capture: impl FnOnce() -> Result<T>,
) -> Result<(T, DesktopFrame)> {
    let taken = Instant::now();
    let mut frame_now = || -> Result<(Rect, Shown)> {
        let scene = scene()?;
        let (output, logical) = scene.single_output()?;
        let area = Rect {
            x: logical.x,
            y: logical.y,
            width: logical.width,
            height: logical.height,
        };
        let shown = scene
            .shown(&output.name)
            .context("niri shows no workspace on its output")?;
        Ok((area, shown))
    };
    let before = frame_now()?;
    let mark = settle()?;
    let (area, shown) = frame_now()?;
    if (area, &shown) != (before.0, &before.1) {
        bail!("the niri layout changed while settling; call get_desktop_state again");
    }
    let captured = capture()?;
    if changed_since(mark)? {
        bail!("the niri layout changed during get_desktop_state; call it again");
    }
    // niri has no output-change event: compare the output itself.
    if frame_now()? != (area, shown.clone()) {
        bail!("the niri output changed during get_desktop_state; call it again");
    }
    Ok((
        captured,
        DesktopFrame {
            area,
            shown,
            mark,
            taken,
        },
    ))
}

/// Publish a `get_desktop_state` screenshot through `publish`, which
/// returns its capture id, and make `frame` the latest one in the same
/// step, so concurrent calls leave the latest published screenshot and the
/// latest frame from one call.
pub fn adopt_desktop(
    frame: DesktopFrame,
    publish: impl FnOnce() -> anyhow::Result<String>,
) -> anyhow::Result<String> {
    let mut latest = latest_desktop();
    let capture = publish()?;
    *latest = Some(LatestDesktop {
        frame,
        capture: capture.clone(),
    });
    Ok(capture)
}

/// Gate desktop-scope pointer input at layout `points`: they are pixels of
/// the latest `get_desktop_state` screenshot, which `capture_id`, when
/// given, must name, and its frame must still hold (see [`DesktopFrame`]).
fn desktop_gate(
    points: &[(i32, i32)],
    capture_id: Option<&str>,
    space: (i32, i32, u32, u32),
) -> Result<PointerGate> {
    let latest = latest_desktop()
        .as_ref()
        .map(|latest| (latest.frame.clone(), latest.capture.clone()));
    let mut ipc = ipc();
    let scene = ipc.scene().map_err(input_unavailable)?;
    let (frame, expected) = desktop_admission(latest, capture_id, &scene, points, &HISTORY)?;
    Ok(PointerGate {
        history: &HISTORY,
        mark: frame.mark,
        expected,
        ipc,
        desktop: Some(frame),
        space,
    })
}

fn desktop_admission(
    latest: Option<(DesktopFrame, String)>,
    capture_id: Option<&str>,
    scene: &Scene,
    points: &[(i32, i32)],
    history: &History,
) -> Result<(DesktopFrame, Expected)> {
    let Some((frame, latest_capture)) = latest else {
        return Err(foreground_unavailable(
            "niri does not report where tiled windows are, so desktop-scope points are pixels of a get_desktop_state screenshot; take one first",
        ));
    };
    if let Some(capture_id) = capture_id.filter(|id| *id != latest_capture) {
        return Err(foreground_unavailable(format!(
            "capture {capture_id} is not the latest get_desktop_state screenshot, and only that one's pixels are checked against niri's layout; use the latest screenshot"
        )));
    }
    let expected = frame.admit(scene, points, history)?;
    Ok((frame, expected))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    fn offline_ipc() -> AttestedIpc {
        Ipc {
            connect: |_| bail!("no niri in this test"),
            per_attempt: QUERY_TIMEOUT,
            total: QUERY_TOTAL_TIMEOUT,
            backoff: QUERY_RETRY_BACKOFF,
            lines: None,
        }
    }

    #[test]
    fn a_layout_change_after_positioning_refuses_the_press() {
        let history = history();
        let mark = history
            .settle(
                || {
                    Ok(event_stream(vec![(
                        Duration::from_millis(250),
                        json!({"WindowLayoutsChanged": {"changes": []}}),
                    )]))
                },
                Duration::from_millis(100),
                Duration::from_secs(1),
            )
            .unwrap();
        let mut gate = PointerGate {
            history,
            mark,
            expected: Expected::default(),
            ipc: offline_ipc(),
            desktop: None,
            space: (0, 0, 0, 0),
        };
        // Still quiet before the first motion; a window moves over the point
        // while the pointer is positioned, so the press must refuse.
        gate.confirm().unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(
            gate.confirm().unwrap_err().to_string(),
            "foreground_unavailable: the niri layout changed after the pointer target was checked; take a new screenshot"
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
            "foreground_unavailable: niri does not report where tiled window 5 is; use get_desktop_state and a desktop-scope action (needs the display grant), or float the window"
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
            "foreground_unavailable: niri does not report where tiled window 5 is; use get_desktop_state and a desktop-scope action (needs the display grant), or float the window"
        );
    }

    #[test]
    fn a_desktop_capture_yields_the_frame_of_its_settled_layout() {
        let mark = Mark {
            generation: 3,
            at: 9,
        };
        let ((), frame) = capture_desktop_in(
            || Ok(tiled_scene(Some(5))),
            || Ok(mark),
            unchanged,
            || Ok(()),
        )
        .unwrap();
        assert_eq!(frame.mark, mark);
        assert_eq!(
            frame.area,
            Rect {
                x: 1920,
                y: 0,
                width: 853,
                height: 480
            }
        );
        assert_eq!(Some(frame.shown), tiled_scene(Some(5)).shown("winit"));
        // A layout change niri reported during the capture refuses it.
        assert_eq!(
            capture_desktop_in(
                || Ok(tiled_scene(Some(5))),
                || Ok(mark),
                |_| Ok(true),
                || Ok(())
            )
            .unwrap_err()
            .to_string(),
            "the niri layout changed during get_desktop_state; call it again"
        );
    }

    #[test]
    fn desktop_pixels_are_admitted_on_the_output_while_its_frame_holds() {
        let history = history();
        let frame = desktop_frame(history);
        let scene = tiled_scene(Some(5));
        // A tiled area: focus moving there may scroll the view.
        let expected = frame.admit(&scene, &[(2500, 400)], history).unwrap();
        assert_eq!(expected.window, None);
        // Floating window 7 covers layout x 2020..2320, y 60..260: a press
        // there may focus and raise it, which moves nothing.
        let expected = frame.admit(&scene, &[(2100, 100)], history).unwrap();
        assert_eq!((expected.window, expected.workspace), (Some(7), Some(1)));
        assert_eq!(
            frame
                .admit(&scene, &[(10, 100)], history)
                .unwrap_err()
                .to_string(),
            "foreground_unavailable: point (10, 100) is past the edge of niri output winit, which the get_desktop_state screenshot shows"
        );
    }

    #[test]
    fn a_desktop_drag_refuses_a_press_that_would_move_focus_off_its_start() {
        let history = history();
        let refusal = "foreground_unavailable: the drag starts where niri has not focused, and its press would focus that window and stop the drag after the press, which the app would see as a click; click there first, take a new get_desktop_state screenshot, then drag";
        // The drag gate's admission, then its start check, as `drag` runs
        // them. Floating window 7 covers layout x 2020..2320, y 60..260.
        let drag = |frame: &DesktopFrame, scene: &Scene, from: (i32, i32), to: (i32, i32)| {
            frame
                .admit(scene, &[from, to], history)
                .and_then(|expected| frame.drag_start(from.0, from.1, expected))
                .map_err(|error| error.to_string())
        };
        let (on_7, also_on_7, tiled) = ((2100, 100), (2200, 200), (2500, 400));
        // Tiled window 5 has focus: a start off the floating window is on
        // it, or on another tiled window niri cannot tell apart.
        let frame = desktop_frame(history);
        let scene = tiled_scene(Some(5));
        drag(&frame, &scene, tiled, (2600, 400)).unwrap();
        // Within floating window 7, its press only focuses and raises it.
        drag(&frame, &scene, on_7, also_on_7).unwrap();
        // Out of window 7, the focus its press takes counts as a change, so
        // the drag would stop after the press.
        let refused = drag(&frame, &scene, on_7, tiled).unwrap_err();
        assert_eq!(refused, refusal);
        assert!(refusal_suggestion(&refused)
            .unwrap()
            .starts_with("Click the drag's start point first"));
        // Floating window 7 has focus: a start on a tiled window would move
        // focus there and may scroll the view; one on window 7 moves none.
        let scene = tiled_scene(Some(7));
        let frame = DesktopFrame {
            shown: scene.shown("winit").unwrap(),
            ..frame
        };
        assert_eq!(
            drag(&frame, &scene, tiled, (2600, 400)).unwrap_err(),
            refusal
        );
        drag(&frame, &scene, on_7, tiled).unwrap();
        // Nothing has focus: a start off the floating window is on a tiled
        // window the press would focus.
        let scene = tiled_scene(None);
        let unfocused = DesktopFrame {
            shown: scene.shown("winit").unwrap(),
            ..frame.clone()
        };
        assert_eq!(
            drag(&unfocused, &scene, tiled, (2600, 400)).unwrap_err(),
            refusal
        );
        // Nothing has focus and the workspace holds no tiled window: the
        // press lands on empty space and focuses nothing.
        let mut scene = tiled_scene(None);
        scene.windows.retain(|window| window.id == 7);
        let empty = DesktopFrame {
            shown: scene.shown("winit").unwrap(),
            ..frame
        };
        drag(&empty, &scene, tiled, (2600, 400)).unwrap();
    }

    #[test]
    fn desktop_pixels_refuse_once_the_layout_changed() {
        let history = history();
        let frame = desktop_frame(history);
        let changed = "foreground_unavailable: niri output winit changed after the get_desktop_state screenshot, so its pixels may be elsewhere now; take a new screenshot";
        // Focus moved to the tiled window beside it: niri may have scrolled.
        let mut focus = tiled_scene(Some(6));
        focus.workspaces[0].active_window_id = Some(6);
        assert_eq!(
            frame.holds(&focus, history).unwrap_err().to_string(),
            changed
        );
        // A column resized or opened shifts the columns after it.
        let mut resized = tiled_scene(Some(5));
        resized.windows[1].layout.as_mut().unwrap().tile_size = (500.0, 448.0);
        assert_eq!(
            frame.holds(&resized, history).unwrap_err().to_string(),
            changed
        );
        let mut overview = tiled_scene(Some(5));
        overview.overview_open = true;
        assert!(frame.holds(&overview, history).is_err());
        // A window on the hidden workspace changes nothing shown.
        let mut hidden = tiled_scene(Some(5));
        hidden.windows[3].layout.as_mut().unwrap().tile_size = (500.0, 448.0);
        frame.holds(&hidden, history).unwrap();
    }

    #[test]
    fn a_change_niri_reported_after_the_capture_refuses_desktop_pixels() {
        let captured = desktop_frame(history());
        let history = history();
        let mark = history
            .settle(
                || {
                    Ok(event_stream(vec![(
                        Duration::from_millis(100),
                        json!({"WindowFocusChanged": {"id": 6}}),
                    )]))
                },
                Duration::from_millis(50),
                Duration::from_secs(1),
            )
            .unwrap();
        let frame = DesktopFrame { mark, ..captured };
        let scene = tiled_scene(Some(5));
        frame.holds(&scene, history).unwrap();
        // Focus went elsewhere and back: the scene matches again, but niri
        // may have scrolled its view in between.
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(
            frame.holds(&scene, history).unwrap_err().to_string(),
            "foreground_unavailable: the niri layout changed after the get_desktop_state screenshot, so its pixels may be elsewhere now; take a new screenshot"
        );
    }

    #[test]
    fn a_desktop_frame_expires_because_niri_does_not_report_every_view_scroll() {
        let history = history();
        let scene = tiled_scene(Some(5));
        let frame = desktop_frame(history);
        let aged = |age: Duration| DesktopFrame {
            taken: Instant::now().checked_sub(age).unwrap(),
            ..frame.clone()
        };
        aged(Duration::from_secs(59))
            .holds(&scene, history)
            .unwrap();
        assert_eq!(
            aged(Duration::from_secs(61))
                .holds(&scene, history)
                .unwrap_err()
                .to_string(),
            "foreground_unavailable: the get_desktop_state screenshot is over 60 s old, and niri does not report every scroll of its view; take a new screenshot"
        );
    }

    #[test]
    fn desktop_input_needs_the_latest_get_desktop_state_screenshot() {
        let history = history();
        let scene = tiled_scene(Some(5));
        let point = [(2500, 400)];
        assert_eq!(
            desktop_admission(None, None, &scene, &point, history)
                .unwrap_err()
                .to_string(),
            "foreground_unavailable: niri does not report where tiled windows are, so desktop-scope points are pixels of a get_desktop_state screenshot; take one first"
        );
        let latest = || Some((desktop_frame(history), "c2".to_owned()));
        assert_eq!(
            desktop_admission(latest(), Some("c1"), &scene, &point, history)
                .unwrap_err()
                .to_string(),
            "foreground_unavailable: capture c1 is not the latest get_desktop_state screenshot, and only that one's pixels are checked against niri's layout; use the latest screenshot"
        );
        desktop_admission(latest(), Some("c2"), &scene, &point, history).unwrap();
        desktop_admission(latest(), None, &scene, &point, history).unwrap();
    }

    #[test]
    fn the_desktop_screenshot_published_last_is_the_one_whose_frame_is_adopted() {
        use std::sync::{mpsc, Arc};
        let frame = desktop_frame(history());
        // The capture registry's latest desktop screenshot.
        let store = Arc::new(Mutex::new(Vec::new()));
        let (published, wait_published) = mpsc::channel();
        let (release, wait_release) = mpsc::channel::<()>();
        let first = std::thread::spawn({
            let (frame, store) = (frame.clone(), store.clone());
            move || {
                adopt_desktop(frame, || {
                    store.lock().unwrap().push("a");
                    published.send(()).unwrap();
                    wait_release.recv().unwrap();
                    Ok("a".to_owned())
                })
                .unwrap();
            }
        });
        wait_published.recv().unwrap();
        let second = std::thread::spawn({
            let store = store.clone();
            move || {
                adopt_desktop(frame, || {
                    store.lock().unwrap().push("b");
                    Ok("b".to_owned())
                })
                .unwrap();
            }
        });
        // The second call would publish and adopt now, between the first
        // call's publish and adopt, unless adoption holds it off.
        std::thread::sleep(Duration::from_millis(100));
        release.send(()).unwrap();
        first.join().unwrap();
        second.join().unwrap();
        assert_eq!(
            latest_desktop()
                .as_ref()
                .map(|latest| latest.capture.clone()),
            store.lock().unwrap().last().map(|id: &&str| id.to_string())
        );
    }
}

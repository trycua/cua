//! Geometry: window placement in layout coordinates, built from one IPC
//! snapshot.

use super::*;

// ── Geometry ─────────────────────────────────────────────────────────────────

/// A rectangle in niri's global logical layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    fn right(&self) -> i64 {
        i64::from(self.x) + i64::from(self.width)
    }

    fn bottom(&self) -> i64 {
        i64::from(self.y) + i64::from(self.height)
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        i64::from(x) >= i64::from(self.x)
            && i64::from(y) >= i64::from(self.y)
            && i64::from(x) < self.right()
            && i64::from(y) < self.bottom()
    }

    pub(super) fn inside(&self, outer: &Rect) -> bool {
        self.x >= outer.x
            && self.y >= outer.y
            && self.right() <= outer.right()
            && self.bottom() <= outer.bottom()
    }

    pub(super) fn from_f64(x: f64, y: f64, width: f64, height: f64) -> Option<Self> {
        let (x, y, width, height) = (x.round(), y.round(), width.round(), height.round());
        let in_i32 = |v: f64| (f64::from(i32::MIN)..=f64::from(i32::MAX)).contains(&v);
        (in_i32(x) && in_i32(y) && width >= 1.0 && height >= 1.0 && in_i32(width) && in_i32(height))
            .then_some(Self {
                x: x as i32,
                y: y as i32,
                width: width as u32,
                height: height as u32,
            })
            .filter(|rect| valid_dimensions(rect.width, rect.height))
    }
}

/// What a tiled window's window capture and window-coordinate pointer input
/// refuse with: no pixel can be proven to be the window's.
const TILED_REFUSAL: &str = "niri does not report where a tiled window is; use get_desktop_state and a desktop-scope action (needs the display grant), or float the window";

/// Why niri reports no on-screen place for a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unplaced {
    /// In the scrolling layout: niri does not report where the view is.
    Tiled,
    /// On an inactive workspace, on no output, or the overview is open.
    NotShowing,
    /// A tiled window as large as the output, likely fullscreen, hides the
    /// workspace's floating windows.
    Covered,
    /// This niri release reports no window layout.
    NoLayout,
}

impl Unplaced {
    pub fn reason(self) -> &'static str {
        match self {
            Self::Tiled => TILED_REFUSAL,
            Self::NotShowing => {
                "the window is not showing: its workspace is not active on an output, or the niri overview is open"
            }
            Self::Covered => {
                "a tiled window as large as the output, likely fullscreen, covers the workspace's floating windows"
            }
            Self::NoLayout => "this niri release does not report window layout (niri 25.08 or later is required)",
        }
    }
}

/// A floating window's place on screen, from one compositor snapshot.
#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    /// The window's geometry in the global logical layout, rounded to whole
    /// logical pixels.
    pub rect: Rect,
    /// The exact logical origin niri reports: niri aligns windows to
    /// physical pixels, so at a fractional scale it is fractional.
    pub origin: (f64, f64),
    pub output: String,
    /// The output's logical rectangle: niri shows no part of the window
    /// outside it.
    pub area: Rect,
    /// The exact tiles of the workspace's other floating windows. IPC
    /// reports no stacking order, so any of them may be above this window.
    pub others: Vec<Exact>,
}

/// A rectangle in exact, possibly fractional, logical coordinates: at a
/// fractional scale niri aligns windows to physical pixels, and rounding to
/// whole logical pixels can hide a one-pixel overlap.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Exact {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Exact {
    pub(super) fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.width && y < self.y + self.height
    }

    fn intersects(&self, other: &Exact) -> bool {
        self.x < other.x + other.width
            && other.x < self.x + self.width
            && self.y < other.y + other.height
            && other.y < self.y + self.height
    }
}

impl Placed {
    /// The window's exact geometry.
    pub fn exact(&self) -> Exact {
        Exact {
            x: self.origin.0,
            y: self.origin.1,
            width: f64::from(self.rect.width),
            height: f64::from(self.rect.height),
        }
    }

    pub fn overlapped(&self) -> bool {
        let exact = self.exact();
        self.others.iter().any(|other| other.intersects(&exact))
    }

    /// Whether a layout point lies inside the exact window geometry rather
    /// than only inside its rounded rectangle.
    pub fn covers(&self, x: i32, y: i32) -> bool {
        self.exact().contains(f64::from(x), f64::from(y))
    }

    /// Whether a layout point surely lands on this window.
    pub fn hits(&self, x: i32, y: i32) -> bool {
        self.covers(x, y)
            && !self
                .others
                .iter()
                .any(|other| other.contains(f64::from(x), f64::from(y)))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    pub id: u64,
    pub pid: Option<u32>,
    pub title: String,
    pub app_id: String,
    pub workspace: Option<u64>,
    pub focused: bool,
    /// Focus recency among all windows, higher is more recent and the
    /// focused window highest. niri reports no stacking order; this is the
    /// best available "which of a process's windows is in use".
    pub focus_rank: usize,
    /// niri shows it: a placed floating window, or the active tiled window of
    /// an active workspace while the overview is closed.
    pub on_screen: bool,
    /// The window's logical size, which niri reports for tiled windows too.
    /// Identity only: a tiled window has no position, so this is never input
    /// or capture geometry.
    pub size: Option<(u32, u32)>,
    pub placement: Result<Placed, Unplaced>,
}

pub(super) struct Scene {
    pub(super) windows: Vec<IpcWindow>,
    pub(super) workspaces: Vec<IpcWorkspace>,
    pub(super) outputs: BTreeMap<String, IpcOutput>,
    pub(super) overview_open: bool,
}

impl Scene {
    fn logical(&self, workspace: &IpcWorkspace) -> Option<&IpcLogical> {
        let output = self.outputs.get(workspace.output.as_ref()?)?;
        output.logical.as_ref()
    }

    fn workspace(&self, window: &IpcWindow) -> Option<&IpcWorkspace> {
        let id = window.workspace_id?;
        self.workspaces.iter().find(|workspace| workspace.id == id)
    }

    fn placement(&self, window: &IpcWindow) -> Result<Placed, Unplaced> {
        let layout = window.layout.as_ref().ok_or(Unplaced::NoLayout)?;
        if layout.pos_in_scrolling_layout.is_some() {
            return Err(Unplaced::Tiled);
        }
        if self.overview_open {
            return Err(Unplaced::NotShowing);
        }
        let workspace = self
            .workspace(window)
            .filter(|workspace| workspace.is_active)
            .ok_or(Unplaced::NotShowing)?;
        let logical = self.logical(workspace).ok_or(Unplaced::NotShowing)?;
        if self.covered(workspace, logical) {
            return Err(Unplaced::Covered);
        }
        let (tile_x, tile_y) = layout
            .tile_pos_in_workspace_view
            .ok_or(Unplaced::NotShowing)?;
        let (offset_x, offset_y) = layout.window_offset_in_tile;
        let (width, height) = layout.window_size;
        let origin = (
            f64::from(logical.x) + tile_x + offset_x,
            f64::from(logical.y) + tile_y + offset_y,
        );
        let rect = Rect::from_f64(origin.0, origin.1, f64::from(width), f64::from(height))
            .ok_or(Unplaced::NotShowing)?;
        let others = self
            .windows
            .iter()
            .filter(|other| other.id != window.id && other.workspace_id == window.workspace_id)
            .filter_map(|other| floating_tile(logical, other.layout.as_ref()?))
            .collect();
        Ok(Placed {
            rect,
            origin,
            output: workspace.output.clone().unwrap_or_default(),
            area: Rect {
                x: logical.x,
                y: logical.y,
                width: logical.width,
                height: logical.height,
            },
            others,
        })
    }

    /// niri hides a workspace's floating windows while its active column is
    /// fullscreen. IPC has no fullscreen flag, so an active tiled tile as
    /// large as the output (give or take a pixel) counts as covering them.
    /// This is refusal evidence only, never proof that a window is visible.
    fn covered(&self, workspace: &IpcWorkspace, logical: &IpcLogical) -> bool {
        self.windows
            .iter()
            .filter(|window| Some(window.id) == workspace.active_window_id)
            .filter_map(|window| window.layout.as_ref())
            .any(|layout| {
                layout.pos_in_scrolling_layout.is_some()
                    && layout.tile_size.0 >= f64::from(logical.width) - 1.0
                    && layout.tile_size.1 >= f64::from(logical.height) - 1.0
            })
    }

    fn on_screen(&self, window: &IpcWindow, placement: &Result<Placed, Unplaced>) -> bool {
        match placement {
            Ok(_) => true,
            Err(Unplaced::Tiled) => {
                !self.overview_open
                    && self.workspace(window).is_some_and(|workspace| {
                        workspace.is_active && workspace.active_window_id == Some(window.id)
                    })
            }
            Err(_) => false,
        }
    }

    pub(super) fn windows(&self) -> Result<Vec<Window>> {
        let mut seen = HashSet::new();
        let mut windows = Vec::with_capacity(self.windows.len());
        for window in &self.windows {
            if !seen.insert(window.id) {
                bail!("duplicate niri window id {}", window.id);
            }
            let placement = self.placement(window);
            windows.push(Window {
                id: window.id,
                pid: window
                    .pid
                    .and_then(|pid| u32::try_from(pid).ok())
                    .filter(|pid| *pid > 0),
                title: window.title.clone().unwrap_or_default(),
                app_id: window.app_id.clone().unwrap_or_default(),
                workspace: window.workspace_id,
                focused: window.is_focused,
                focus_rank: self
                    .windows
                    .iter()
                    .filter(|other| {
                        (other.is_focused, other.focus_timestamp)
                            < (window.is_focused, window.focus_timestamp)
                    })
                    .count(),
                on_screen: self.on_screen(window, &placement),
                size: window.layout.as_ref().and_then(|layout| {
                    let (width, height) = layout.window_size;
                    Some((u32::try_from(width).ok()?, u32::try_from(height).ok()?))
                }),
                placement,
            });
        }
        Ok(windows)
    }

    pub(super) fn window(&self, id: u64) -> Result<Window> {
        self.windows()?
            .into_iter()
            .find(|window| window.id == id)
            .with_context(|| format!("niri window {id} no longer exists; refresh list_windows"))
    }

    /// Outputs niri lays out (enabled ones have a logical rectangle).
    pub(super) fn laid_out(&self) -> Vec<(&IpcOutput, &IpcLogical)> {
        self.outputs
            .values()
            .filter_map(|output| Some((output, output.logical.as_ref()?)))
            .collect()
    }

    /// Absolute virtual-pointer motion from an unbound pointer maps over the
    /// bounding box of every output's pointer rectangle.
    pub(super) fn pointer_space(&self) -> Result<(i32, i32, u32, u32)> {
        pointer_space_of(&self.outputs)
    }

    /// The one laid-out output. Display capture copies a single output, so
    /// output-pixel routes need a layout where that output is the only one.
    pub(super) fn single_output(&self) -> Result<(&IpcOutput, &IpcLogical)> {
        match self.laid_out().as_slice() {
            [(output, logical)] => Ok((*output, logical.checked()?)),
            [] => bail!("niri has no enabled output"),
            outputs => bail!(
                "niri window capture and desktop input support a single output; this layout has {}",
                outputs.len()
            ),
        }
    }
}

pub(super) fn pointer_space_of(
    outputs: &BTreeMap<String, IpcOutput>,
) -> Result<(i32, i32, u32, u32)> {
    let mut rects = Vec::new();
    for output in outputs.values() {
        rects.extend(output.pointer_rect()?);
    }
    bounding_box(&rects).context("niri has no enabled output")
}

pub(super) fn floating_tile(logical: &IpcLogical, layout: &IpcLayout) -> Option<Exact> {
    if layout.pos_in_scrolling_layout.is_some() {
        return None;
    }
    let (x, y) = layout.tile_pos_in_workspace_view?;
    let tile = Exact {
        x: f64::from(logical.x) + x,
        y: f64::from(logical.y) + y,
        width: layout.tile_size.0,
        height: layout.tile_size.1,
    };
    [tile.x, tile.y, tile.width, tile.height]
        .iter()
        .all(|v| v.is_finite())
        .then_some(tile)
}

/// The rectangle around `rects`, or `None` when it spans more than
/// `i32::MAX`: pointer motion converts the span to `i32`.
pub(super) fn bounding_box(rects: &[Rect]) -> Option<(i32, i32, u32, u32)> {
    let min_x = rects.iter().map(|r| r.x).min()?;
    let min_y = rects.iter().map(|r| r.y).min()?;
    let max_x = rects.iter().map(Rect::right).max()?;
    let max_y = rects.iter().map(Rect::bottom).max()?;
    let span = |len: i64| u32::try_from(i32::try_from(len).ok()?).ok();
    Some((
        min_x,
        min_y,
        span(max_x - i64::from(min_x))?,
        span(max_y - i64::from(min_y))?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    #[test]
    fn unknown_output_transform_refuses_pointer_space() {
        let scene = scene(
            vec![],
            vec![workspace(1, "winit", true, None)],
            vec![output("winit", 0, 0, (1280, 720), 1.5, "Diagonal")],
            false,
        );
        assert_eq!(
            scene.pointer_space().unwrap_err().to_string(),
            "unknown niri output transform \"Diagonal\""
        );
    }

    #[test]
    fn duplicate_window_ids_refuse_the_snapshot() {
        let scene = one_output(vec![
            floating(7, 1, (0.0, 0.0), (10, 10)),
            floating(7, 1, (50.0, 0.0), (10, 10)),
        ]);
        assert_eq!(
            scene.windows().unwrap_err().to_string(),
            "duplicate niri window id 7"
        );
    }

    #[test]
    fn floating_window_geometry_adds_output_origin_tile_and_offset() {
        let mut window = floating(7, 1, (100.25, 60.0), (300, 200));
        window["layout"]["window_offset_in_tile"] = json!([4.0, 4.0]);
        let scene = scene(
            vec![window],
            vec![workspace(1, "DP-2", true, None)],
            vec![output("DP-2", 1920, 0, (2560, 1440), 1.25, "Normal")],
            false,
        );
        assert_eq!(
            placement(&scene, 7).unwrap().rect,
            Rect {
                x: 2024,
                y: 64,
                width: 300,
                height: 200
            }
        );
    }

    #[test]
    fn tiled_windows_get_no_geometry() {
        let scene = one_output(vec![tiled(5, 1, (402.0, 448.0))]);
        assert_eq!(placement(&scene, 5), Err(Unplaced::Tiled));
        assert!(
            !scene.window(5).unwrap().on_screen,
            "an inactive tiled window is not proven on screen"
        );
    }

    #[test]
    fn active_tiled_window_is_on_screen_without_a_place() {
        let scene = scene(
            vec![tiled(5, 1, (402.0, 448.0))],
            vec![workspace(1, "winit", true, Some(5))],
            vec![output("winit", 0, 0, (1280, 720), 1.5, "Normal")],
            false,
        );
        let window = scene.window(5).unwrap();
        assert!(window.on_screen);
        assert_eq!(window.placement, Err(Unplaced::Tiled));
        // The size still identifies it, for AT-SPI root correlation.
        assert_eq!(window.size, Some((402, 448)));
    }

    #[test]
    fn floating_window_on_an_inactive_workspace_is_not_showing() {
        let scene = one_output(vec![floating(7, 2, (100.0, 60.0), (300, 200))]);
        assert_eq!(placement(&scene, 7), Err(Unplaced::NotShowing));
        assert!(!scene.window(7).unwrap().on_screen);
    }

    #[test]
    fn open_overview_hides_every_place() {
        let scene = scene(
            vec![floating(7, 1, (100.0, 60.0), (300, 200))],
            vec![workspace(1, "winit", true, None)],
            vec![output("winit", 0, 0, (1280, 720), 1.5, "Normal")],
            true,
        );
        assert_eq!(placement(&scene, 7), Err(Unplaced::NotShowing));
    }

    #[test]
    fn output_sized_active_tile_covers_floating_windows() {
        // Logical output 853x480; a fullscreen tile reports about that size.
        for (tile, covered) in [
            ((853.0, 480.0), true),
            ((852.4, 479.2), true),
            ((402.0, 448.0), false),
        ] {
            let scene = scene(
                vec![floating(7, 1, (100.0, 60.0), (300, 200)), tiled(5, 1, tile)],
                vec![workspace(1, "winit", true, Some(5))],
                vec![output("winit", 0, 0, (1280, 720), 1.5, "Normal")],
                false,
            );
            assert_eq!(
                placement(&scene, 7) == Err(Unplaced::Covered),
                covered,
                "{tile:?}"
            );
        }
    }

    #[test]
    fn layoutless_niri_refuses_geometry() {
        let mut window = floating(7, 1, (100.0, 60.0), (300, 200));
        window.as_object_mut().unwrap().remove("layout");
        assert_eq!(
            placement(&one_output(vec![window]), 7),
            Err(Unplaced::NoLayout)
        );
    }

    #[test]
    fn any_floating_overlap_is_ambiguous_whatever_the_focus_order() {
        let stamps = [
            (
                json!({"secs": 1, "nanos": 0}),
                json!({"secs": 2, "nanos": 0}),
            ),
            (
                json!({"secs": 2, "nanos": 0}),
                json!({"secs": 1, "nanos": 0}),
            ),
            (
                json!({"secs": 1, "nanos": 0}),
                json!({"secs": 1, "nanos": 0}),
            ),
            (Value::Null, Value::Null),
        ];
        for (target_stamp, other_stamp) in stamps {
            for target_focused in [false, true] {
                let mut target = floating(7, 1, (100.0, 60.0), (300, 200));
                target["focus_timestamp"] = target_stamp.clone();
                target["is_focused"] = json!(target_focused);
                // A dialog (child) over its parent looks the same in IPC.
                let mut other = floating(8, 1, (350.0, 200.0), (200, 100));
                other["focus_timestamp"] = other_stamp.clone();
                let placed = placement(&one_output(vec![target, other]), 7).unwrap();
                assert!(placed.overlapped());
                assert!(
                    !placed.hits(360, 210),
                    "a point under the other tile is ambiguous"
                );
                assert!(placed.hits(110, 70));
            }
        }
    }

    #[test]
    fn floating_windows_on_other_workspaces_do_not_overlap() {
        let placed = placement(
            &one_output(vec![
                floating(7, 1, (100.0, 60.0), (300, 200)),
                floating(8, 2, (100.0, 60.0), (300, 200)),
                tiled(5, 1, (402.0, 448.0)),
            ]),
            7,
        )
        .unwrap();
        assert!(!placed.overlapped());
    }

    #[test]
    fn pointer_space_rounds_each_output_up_and_spans_all_outputs() {
        let single = |mode, scale| {
            scene(
                vec![],
                vec![],
                vec![output("winit", 0, 0, mode, scale, "Normal")],
                false,
            )
            .pointer_space()
            .unwrap()
        };
        assert_eq!(single((1280, 720), 1.0), (0, 0, 1280, 720));
        assert_eq!(single((1280, 720), 1.25), (0, 0, 1024, 576));
        assert_eq!(single((1280, 720), 1.5), (0, 0, 854, 480));
        assert_eq!(single((1280, 720), 2.0), (0, 0, 640, 360));
        let pair = scene(
            vec![],
            vec![],
            vec![
                output("eDP-1", 0, 0, (1280, 720), 1.5, "Normal"),
                output("DP-2", 853, -100, (1920, 1080), 1.0, "Normal"),
            ],
            false,
        );
        assert_eq!(pair.pointer_space().unwrap(), (0, -100, 2773, 1080));
        let rotated = scene(
            vec![],
            vec![],
            vec![output("DP-2", 0, 0, (1920, 1080), 1.0, "90")],
            false,
        );
        assert_eq!(rotated.pointer_space().unwrap(), (0, 0, 1080, 1920));
    }

    #[test]
    fn a_pointer_space_wider_than_i32_refuses() {
        // Pointer motion clamps into `width as i32 - 1`, which panics once
        // the span passes i32::MAX.
        let far = scene(
            vec![],
            vec![],
            vec![
                output("DP-1", i32::MIN, 0, (1920, 1080), 1.0, "Normal"),
                output("DP-2", i32::MAX - 1920, 0, (1920, 1080), 1.0, "Normal"),
            ],
            false,
        );
        assert!(far.pointer_space().is_err());
        let edge = Rect {
            x: 0,
            y: 0,
            width: i32::MAX as u32,
            height: 1,
        };
        assert_eq!(bounding_box(&[edge]), Some((0, 0, i32::MAX as u32, 1)));
    }

    #[test]
    fn disabled_outputs_are_left_out_of_pointer_space() {
        let mut off = output("HDMI-A-1", 0, 0, (1920, 1080), 1.0, "Normal");
        off["logical"] = Value::Null;
        off["current_mode"] = Value::Null;
        let scene = scene(
            vec![],
            vec![],
            vec![output("winit", 0, 0, (1280, 720), 1.5, "Normal"), off],
            false,
        );
        assert_eq!(scene.pointer_space().unwrap(), (0, 0, 854, 480));
    }
}

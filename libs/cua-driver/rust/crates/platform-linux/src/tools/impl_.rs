//! Real Linux tool implementations (compiled only on Linux).

use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolRegistry},
    tool_args::ArgsExt,
    window_target::{PidOnlyWindowTargetGuard, WindowTargetCandidate, WindowTargetCandidates},
};
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use crate::atspi::Snapshots;
pub(crate) use crate::input::action::spawn_blocking_bounded;
pub(crate) use crate::wayland::AimedPoint;
use cursor_overlay::CursorRegistry;

use super::{
    bring_to_front::BringToFrontTool,
    check_permissions::CheckPermissionsTool,
    click::ClickTool,
    cursor_tools::{
        GetAgentCursorStateV2Tool, SetAgentCursorEnabledV2Tool, SetAgentCursorMotionV2Tool,
        SetAgentCursorThemeTool,
    },
    double_click::DoubleClickTool,
    drag::DragTool,
    get_accessibility_tree::GetAccessibilityTreeTool,
    get_config::GetConfigTool,
    get_cursor_position::GetCursorPositionTool,
    get_desktop_state::GetDesktopStateTool,
    get_screen_size::GetScreenSizeTool,
    get_window_state::GetWindowStateTool,
    hotkey::HotkeyTool,
    invoke_menu::InvokeMenuTool,
    kill_app::KillAppTool,
    launch_app::LaunchAppTool,
    list_apps::ListAppsTool,
    list_windows::ListWindowsTool,
    mouse_button_down::MouseButtonDownTool,
    mouse_button_up::MouseButtonUpTool,
    mouse_drag::MouseDragTool,
    move_cursor::MoveCursorTool,
    parallel_mouse_drag::ParallelMouseDragTool,
    press_key::PressKeyTool,
    right_click::RightClickTool,
    scroll::ScrollTool,
    set_config::SetConfigTool,
    set_value::SetValueTool,
    set_window_frame::SetWindowFrameTool,
    type_text::TypeTextTool,
    type_text_chars::TypeTextCharsTool,
    zoom::ZoomTool,
};

fn window_target_candidates_for_pid(
    windows: impl IntoIterator<Item = crate::x11::WindowInfo>,
    pid: u32,
) -> Vec<WindowTargetCandidate> {
    windows
        .into_iter()
        .filter(|window| window.pid == Some(pid))
        .map(|window| WindowTargetCandidate {
            window_id: window.xid,
            transient_for: (!crate::wayland::is_wayland())
                .then(|| crate::x11::transient_for(window.xid))
                .flatten(),
            title: window.title,
            app_name: Some(window.app_name),
            is_on_screen: window.is_on_screen,
        })
        .collect()
}

fn pid_window_target_candidates(pid: i64) -> Vec<WindowTargetCandidate> {
    let Ok(pid) = u32::try_from(pid) else {
        return Vec::new();
    };
    window_target_candidates_for_pid(crate::wayland::list_windows_dispatch(Some(pid)), pid)
}

/// Topmost on-screen window of `pid` covering a desktop-frame point.
fn desktop_point_window_resolver() -> cua_driver_core::window_target::DesktopPointWindowResolver {
    Arc::new(move |pid, x, y| {
        let pid = u32::try_from(pid).ok()?;
        // Window geometry is in layout coordinates; on Hyprland the desktop
        // frame starts at the top-left powered output, not the layout origin.
        let (x, y) = if crate::wayland::is_wayland() && crate::wayland::hyprland::is_session() {
            let frame = crate::wayland::hyprland::desktop_frame().ok()?;
            (x + f64::from(frame.x), y + f64::from(frame.y))
        } else if crate::wayland::is_wayland() && crate::wayland::niri::is_session() {
            let frame = crate::wayland::niri::desktop_snapshot().ok()?.frame;
            (x + f64::from(frame.x), y + f64::from(frame.y))
        } else {
            (x, y)
        };
        topmost_window_at(&crate::wayland::list_windows_dispatch(Some(pid)), pid, x, y)
    })
}

/// The pid's topmost on-screen window containing screen point `(sx, sy)`:
/// the highest `z_index` (bottom-to-top stacking position) wins, so an
/// app's own dialog beats the main window it covers. Windows without a
/// stacking position rank lowest.
fn topmost_window_at(
    windows: &[crate::x11::WindowInfo],
    pid: u32,
    sx: f64,
    sy: f64,
) -> Option<u64> {
    windows
        .iter()
        .filter(|w| w.pid == Some(pid) && w.is_on_screen && w.width > 0 && w.height > 0)
        .filter(|w| {
            sx >= f64::from(w.x)
                && sy >= f64::from(w.y)
                && sx < f64::from(w.x) + f64::from(w.width)
                && sy < f64::from(w.y) + f64::from(w.height)
        })
        .max_by_key(|w| w.z_index.map(|z| z as i64).unwrap_or(-1))
        .map(|w| w.xid)
}

/// The window a pid-only keyboard action means in a multi-window app: the
/// pid's window that holds the core focus, else its topmost transient dialog,
/// else the WM's active window when it is the pid's, else the largest mapped
/// toplevel (see [`crate::x11::pick_pid_window`]). A dialog the app just
/// opened therefore receives the following `type_text` / `press_key` even
/// while the main window is still the WM's active window.
fn pid_fallback_window_resolver() -> cua_driver_core::window_target::PidFallbackWindowResolver {
    Arc::new(|pid| {
        let pid = u32::try_from(pid).ok()?;
        if crate::wayland::is_wayland() && crate::wayland::niri::is_session() {
            return crate::wayland::niri::window_in_use(pid);
        }
        let windows: Vec<crate::x11::WindowInfo> = crate::wayland::list_windows_dispatch(Some(pid))
            .into_iter()
            .filter(|w| w.pid == Some(pid))
            .collect();
        if crate::wayland::is_wayland() {
            return windows
                .into_iter()
                .filter(|w| w.is_on_screen)
                .max_by_key(|w| w.z_index.unwrap_or(0))
                .map(|w| w.xid);
        }
        let ids: Vec<u64> = windows.iter().map(|w| w.xid).collect();
        crate::x11::pick_pid_window(
            &windows,
            crate::x11::focused_window_among(&ids),
            crate::x11::transient_for,
            crate::x11::active_window(),
        )
    })
}

#[cfg(test)]
mod pid_window_resolver_tests {
    use super::*;

    fn win(xid: u64, z: Option<usize>, on_screen: bool, w: u32, h: u32) -> crate::x11::WindowInfo {
        crate::x11::WindowInfo {
            xid,
            pid: Some(7),
            app_name: "gimp".into(),
            title: format!("w{xid}"),
            is_on_screen: on_screen,
            z_index: z,
            x: 0,
            y: 0,
            width: w,
            height: h,
        }
    }

    #[test]
    fn point_resolver_prefers_the_topmost_window_by_stacking_position() {
        // Main window listed last (as `_NET_CLIENT_LIST_STACKING` bottom-up
        // enumeration does when a dialog sits above it); the dialog wins.
        let dialog = crate::x11::WindowInfo {
            z_index: Some(5),
            ..win(2, None, true, 400, 300)
        };
        let main = crate::x11::WindowInfo {
            z_index: Some(3),
            ..win(1, None, true, 1920, 1080)
        };
        assert_eq!(
            topmost_window_at(&[main.clone(), dialog.clone()], 7, 100.0, 100.0),
            Some(2)
        );
        assert_eq!(topmost_window_at(&[dialog, main], 7, 100.0, 100.0), Some(2));
    }

    #[test]
    fn point_resolver_ignores_windows_without_a_stacking_position_when_one_has_it() {
        let ranked = win(2, Some(0), true, 400, 300);
        let unranked = win(1, None, true, 1920, 1080);
        assert_eq!(
            topmost_window_at(&[ranked, unranked], 7, 10.0, 10.0),
            Some(2)
        );
    }

    #[test]
    fn pid_only_resolver_orders_focus_then_dialog_then_active_then_largest() {
        let main = win(1, Some(1), true, 1920, 1080);
        let dialog = win(2, Some(2), true, 400, 300);
        let dock = win(3, Some(0), true, 200, 900);
        let hidden = win(4, None, false, 3000, 3000);
        let windows = vec![main, dialog, dock, hidden];
        let transient = |xid: u64| (xid == 2).then_some(1u64);
        // 1. core focus inside the pid wins outright (even over a dialog).
        assert_eq!(
            crate::x11::pick_pid_window(&windows, Some(3), transient, Some(1)),
            Some(3)
        );
        // 2. no focus in the pid: its topmost transient dialog.
        assert_eq!(
            crate::x11::pick_pid_window(&windows, None, transient, Some(1)),
            Some(2)
        );
        // 3. no dialog: the WM's active window when it is the pid's.
        assert_eq!(
            crate::x11::pick_pid_window(&windows, None, |_| None, Some(3)),
            Some(3)
        );
        // 4. otherwise the largest mapped toplevel (never the unmapped one).
        assert_eq!(
            crate::x11::pick_pid_window(&windows, None, |_| None, None),
            Some(1)
        );
        assert_eq!(
            crate::x11::pick_pid_window(&windows, None, |_| None, Some(99)),
            Some(1)
        );
        // A focused window that is not the pid's is ignored.
        assert_eq!(
            crate::x11::pick_pid_window(&windows, Some(42), |_| None, None),
            Some(1)
        );
    }

    #[test]
    fn pid_only_resolver_skips_unmapped_dialogs() {
        let main = win(1, Some(1), true, 800, 600);
        let unmapped_dialog = win(2, Some(2), false, 400, 300);
        let windows = vec![main, unmapped_dialog];
        assert_eq!(
            crate::x11::pick_pid_window(&windows, None, |xid| (xid == 2).then_some(1u64), None),
            Some(1)
        );
    }
}

type PidWindowGuardParts = (
    WindowTargetCandidates,
    cua_driver_core::window_target::DesktopPointWindowResolver,
    cua_driver_core::window_target::PidFallbackWindowResolver,
    cua_driver_core::window_target::SnapshotWindowResolver,
    cua_driver_core::window_target::TokenPidResolver,
);

fn pid_window_guarded<T: Tool + 'static>(
    tool: T,
    (candidates, point_resolver, fallback_resolver, snapshot_resolver, token_pid_resolver): &PidWindowGuardParts,
) -> Box<dyn Tool> {
    Box::new(
        PidOnlyWindowTargetGuard::new(Box::new(tool), candidates.clone())
            .with_point_resolver(point_resolver.clone())
            .with_fallback_resolver(fallback_resolver.clone())
            .with_snapshot_resolver(snapshot_resolver.clone())
            .with_token_pid_resolver(token_pid_resolver.clone()),
    )
}

/// The pid an `element_token` was minted for, so a token-only action (no
/// `pid`) reaches its tool with the pid the token carries.
fn token_pid_resolver(state: Arc<ToolState>) -> cua_driver_core::window_target::TokenPidResolver {
    Arc::new(move |args| state.snapshots.pid_for_token(args))
}

/// The window a `snapshot_id` was published for (the element cache lane of
/// that pid), so pid-only element actions follow the snapshot to its popup
/// or dialog.
fn snapshot_window_resolver(
    state: Arc<ToolState>,
) -> cua_driver_core::window_target::SnapshotWindowResolver {
    Arc::new(move |pid, handle| {
        let pid = i32::try_from(pid).ok()?;
        let id = cua_driver_core::element_token::parse_snapshot_handle(handle)?;
        state.snapshots.window_for_snapshot(pid, id)
    })
}

// ── DriverConfig + ZoomRegistry ─────────────────────────────────────────────

#[derive(Clone)]
pub struct DriverConfig {
    pub capture_mode: String,
    pub max_image_dimension: u32,
}

impl Default for DriverConfig {
    fn default() -> Self {
        Self {
            capture_mode: "ax".into(),
            max_image_dimension: 1568,
        }
    }
}

/// Load `DriverConfig` from `~/.cua-driver/config.json`, falling back to
/// defaults for any missing/malformed keys. Called once at `ToolState`
/// construction (i.e. on every fresh `cua-driver call` process) so that a
/// prior `set_config capture_mode=vision` survives across stateless one-shot
/// invocations — matching the macOS daemon's startup load. See #2008.
pub fn load_driver_config() -> DriverConfig {
    let mut cfg = DriverConfig::default();
    if let Some(v) =
        pip_preview::read_config_value("capture_mode").and_then(|v| v.as_str().map(str::to_owned))
    {
        cfg.capture_mode = v;
    }
    if let Some(v) = pip_preview::read_config_value("max_image_dimension").and_then(|v| v.as_u64())
    {
        if let Ok(v32) = u32::try_from(v) {
            cfg.max_image_dimension = v32;
        }
    }
    cfg
}

use cua_driver_core::snapshot_store::ZoomContext;

pub struct ToolState {
    pub snapshots: Arc<Snapshots>,
    pub cursor_registry: Arc<CursorRegistry>,
    pub capture_service: Arc<cua_driver_core::capture_runtime::CaptureService>,
    pub mouse_hold: std::sync::Mutex<std::collections::HashMap<String, MouseHoldState>>,
    pub config: Arc<RwLock<DriverConfig>>,
}

impl ToolState {}

#[derive(Clone, Debug)]
pub struct MouseHoldState {
    pub pid: u32,
    pub xid: u64,
    pub button: u8,
    pub x: f64,
    pub y: f64,
}

impl ToolState {
    /// Test fixture: production registries share one capture service.
    #[cfg(test)]
    pub fn new() -> Arc<Self> {
        Self::new_with_capture_service(Arc::new(
            cua_driver_core::capture_runtime::CaptureService::default(),
        ))
    }

    fn new_with_capture_service(
        capture_service: Arc<cua_driver_core::capture_runtime::CaptureService>,
    ) -> Arc<Self> {
        let snapshots = Arc::new(Snapshots::new());
        cua_driver_core::snapshot_store::register_runtime_store(&snapshots);
        Arc::new(Self {
            snapshots,
            cursor_registry: Arc::new(CursorRegistry::new()),
            capture_service,
            mouse_hold: std::sync::Mutex::new(Default::default()),
            config: Arc::new(RwLock::new(load_driver_config())),
        })
    }

    pub(crate) fn zoom_context(
        &self,
        args: &Value,
        pid: u32,
        window_id: Option<u64>,
    ) -> Result<ZoomContext, ToolResult> {
        self.snapshots.zoom(
            pid as i32,
            window_id,
            args.get("_session_id").and_then(Value::as_str),
        )
    }
}

pub(crate) fn screenshot_scale(
    state: &ToolState,
    args: &Value,
    pid: u32,
    window_id: Option<u64>,
) -> Result<f64, ToolResult> {
    state
        .snapshots
        .screenshot_scale(pid as i32, window_id, args)
        .map_err(|missing| {
            // A tiled niri window never has a screenshot: say why instead of
            // asking for the get_window_state read that cannot give one.
            window_id
                .filter(|_| crate::wayland::is_wayland() && crate::wayland::niri::is_session())
                .and_then(crate::wayland::niri::tiled_refusal)
                .map_or(missing, input_error_result)
        })
}

/// Return the lowercase basename of an executable token, stripping any
/// leading shell-wrapper words and quoting. `env FOO=1 /usr/bin/firefox %U`
/// → `firefox`. `firefox %u` → `firefox`.
pub(crate) fn exec_basename(s: &str) -> String {
    // Take the first whitespace-separated token that looks like a binary,
    // skipping `env`-style prefixes and `K=V` assignments.
    for tok in s.split_whitespace() {
        if tok == "env" {
            continue;
        }
        if tok.contains('=') && !tok.starts_with('/') && !tok.starts_with('-') {
            // `FOO=bar` env-var assignment — skip.
            continue;
        }
        let cleaned = tok.trim_matches(|c| c == '"' || c == '\'');
        let basename = std::path::Path::new(cleaned)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(cleaned);
        return basename.to_ascii_lowercase();
    }
    String::new()
}

/// Build the structured `list_windows` record for one Linux window.
///
/// Emits the canonical cross-platform shape — geometry nested under a
/// `bounds: {x,y,width,height}` object plus `app_name` / `is_on_screen`,
/// matching the macOS and Windows backends (#2017) — while KEEPING the
/// historical flat `x/y/width/height` fields inline as a legacy alias so
/// existing Linux callers don't break. Fully additive; no field removed,
/// no schema_version bump.
///
pub(crate) fn window_record_json(w: &crate::x11::WindowInfo) -> Value {
    json!({
        "window_id": w.xid,
        "pid": w.pid,
        "app_name": w.app_name,
        "title": w.title,
        // Canonical cross-platform geometry (macOS/Windows parity).
        "bounds": { "x": w.x, "y": w.y, "width": w.width, "height": w.height },
        "is_on_screen": w.is_on_screen,
        "z_index": w.z_index,
        // Legacy alias: flat fields kept inline for pre-existing callers.
        "x": w.x, "y": w.y,
        "width": w.width, "height": w.height,
    })
}

pub(crate) fn visible_live_desktop_windows(
    mut windows: Vec<crate::x11::WindowInfo>,
) -> Vec<crate::x11::WindowInfo> {
    windows.retain(|window| {
        window.is_on_screen && window.pid.is_none_or(crate::proc_fs::is_process_live)
    });
    windows
}

#[cfg(test)]
mod list_windows_tests {
    use super::*;

    #[test]
    fn record_has_bounds_and_flat_legacy_fields() {
        let w = crate::x11::WindowInfo {
            xid: 42,
            pid: Some(1234),
            app_name: "example-app".to_owned(),
            title: "Example".to_owned(),
            is_on_screen: true,
            z_index: Some(3),
            x: 10,
            y: 20,
            width: 300,
            height: 400,
        };
        let rec = window_record_json(&w);

        // Canonical cross-platform shape: nested `bounds` object.
        let bounds = rec
            .get("bounds")
            .expect("record must carry a `bounds` object");
        assert_eq!(bounds["x"], json!(10));
        assert_eq!(bounds["y"], json!(20));
        assert_eq!(bounds["width"], json!(300));
        assert_eq!(bounds["height"], json!(400));

        // Legacy alias: flat fields must still be present.
        assert_eq!(rec["x"], json!(10));
        assert_eq!(rec["y"], json!(20));
        assert_eq!(rec["width"], json!(300));
        assert_eq!(rec["height"], json!(400));

        // Cross-platform companions.
        assert_eq!(rec["app_name"], json!("example-app"));
        assert_eq!(rec["is_on_screen"], json!(true));
        assert_eq!(rec["z_index"], json!(3));
        assert_eq!(rec["window_id"], json!(42));
        assert_eq!(rec["title"], json!("Example"));
    }

    #[test]
    fn unavailable_wayland_order_serializes_as_null() {
        let w = crate::x11::WindowInfo {
            xid: 43,
            pid: Some(1234),
            app_name: "native-wayland-app".to_owned(),
            title: "Example".to_owned(),
            is_on_screen: true,
            z_index: None,
            x: 0,
            y: 0,
            width: 300,
            height: 400,
        };

        assert_eq!(window_record_json(&w)["z_index"], Value::Null);
    }

    #[test]
    fn desktop_discovery_retains_only_visible_windows_with_live_or_unknown_owners() {
        let window = |xid, pid, is_on_screen| crate::x11::WindowInfo {
            xid,
            pid,
            app_name: "native-wayland-app".to_owned(),
            title: "Example".to_owned(),
            is_on_screen,
            z_index: None,
            x: 10,
            y: 20,
            width: 300,
            height: 400,
        };
        let windows = visible_live_desktop_windows(vec![
            window(1, Some(std::process::id()), true),
            window(2, Some(std::process::id()), false),
            window(3, Some(u32::MAX), true),
            window(4, None, true),
            window(5, None, false),
        ]);

        assert_eq!(
            windows.iter().map(|w| w.xid).collect::<Vec<_>>(),
            vec![1, 4]
        );
        let record = window_record_json(&windows[0]);
        assert_eq!(
            record["bounds"],
            json!({"x": 10, "y": 20, "width": 300, "height": 400})
        );
        assert_eq!(record["app_name"], json!("native-wayland-app"));
        assert_eq!(record["is_on_screen"], json!(true));
        assert_eq!(record["z_index"], Value::Null);
    }

    #[test]
    fn chromium_launch_detection_uses_executable_basename() {
        assert!(chromium_family_program("/usr/bin/google-chrome-stable"));
        assert!(chromium_family_program("CuaTestHarness.Electron"));
        assert!(chromium_family_program("chromium-browser"));
        assert!(!chromium_family_program("/usr/bin/gnome-text-editor"));
    }
}

/// Roles a popup's rows carry: menu entries, and the list / tree / table
/// rows of a combo list, a completer or a chooser popover.
fn popup_item_role(role: &str) -> bool {
    let role = role.trim().to_ascii_lowercase();
    role.contains("menu item")
        || role == "menu"
        || role.contains("list item")
        || role.contains("tree item")
        || role.contains("table cell")
        || role == "cell"
        || role == "item"
        || role == "option"
}

/// The entries drawn inside a popup of `width`x`height` (frames are
/// popup-local here). When an indexed container (Qt exposes the combo list
/// under the combo box inside the dialog) fills the popup, its subtree is
/// the popup's content; otherwise the item-role elements inside the box.
/// Falls back to the input when nothing matches, so a popup that is not a
/// menu (a GTK popover without its own frame) still returns what the walk
/// found.
pub(crate) fn popup_menu_elements(
    elements: Vec<serde_json::Value>,
    (origin_x, origin_y): (i32, i32),
    width: u32,
    height: u32,
) -> Vec<serde_json::Value> {
    // `frame` is in screen coordinates; compare it with the popup's screen
    // rectangle.
    let frame_inside = |entry: &serde_json::Value| {
        let frame = &entry["frame"];
        let x = frame["x"].as_i64().unwrap_or(i64::MIN) - i64::from(origin_x);
        let y = frame["y"].as_i64().unwrap_or(i64::MIN) - i64::from(origin_y);
        frame.is_object()
            && x >= -2
            && y >= -2
            && x + frame["w"].as_i64().unwrap_or(0) <= width as i64 + 2
            && y + frame["h"].as_i64().unwrap_or(0) <= height as i64 + 2
    };
    let role_of =
        |entry: &serde_json::Value| entry["role"].as_str().unwrap_or("").to_ascii_lowercase();
    // A container whose frame covers most of the popup: the list behind a
    // combo box / completer, the menu behind a context menu.
    let popup_area = u64::from(width) * u64::from(height);
    let container = elements
        .iter()
        .filter(|entry| frame_inside(entry))
        .filter(|entry| {
            let role = role_of(entry);
            matches!(
                role.as_str(),
                "list"
                    | "list box"
                    | "menu"
                    | "tree"
                    | "tree table"
                    | "table"
                    | "panel"
                    | "scroll pane"
                    | "filler"
            )
        })
        .filter(|entry| {
            let frame = &entry["frame"];
            let area = frame["w"].as_u64().unwrap_or(0) * frame["h"].as_u64().unwrap_or(0);
            popup_area > 0 && area * 10 >= popup_area * 6
        })
        .max_by_key(|entry| {
            let frame = &entry["frame"];
            frame["w"].as_u64().unwrap_or(0) * frame["h"].as_u64().unwrap_or(0)
        })
        .and_then(|entry| entry["element_index"].as_u64());
    if let Some(container) = container {
        let parent_of: std::collections::HashMap<u64, u64> = elements
            .iter()
            .filter_map(|entry| {
                Some((
                    entry["element_index"].as_u64()?,
                    entry["parent_index"].as_u64()?,
                ))
            })
            .collect();
        let descends = |mut idx: u64| {
            for _ in 0..64 {
                match parent_of.get(&idx) {
                    Some(parent) if *parent == container => return true,
                    Some(parent) => idx = *parent,
                    None => return false,
                }
            }
            false
        };
        let subtree: Vec<serde_json::Value> = elements
            .iter()
            .filter(|entry| {
                entry["element_index"]
                    .as_u64()
                    .is_some_and(|idx| idx != container && descends(idx))
            })
            .cloned()
            .collect();
        if !subtree.is_empty() {
            return subtree;
        }
    }
    let items: Vec<serde_json::Value> = elements
        .iter()
        .filter(|entry| popup_item_role(&role_of(entry)) && frame_inside(entry))
        .cloned()
        .collect();
    // A popup is either a menu or a list: when list / tree / table rows are
    // drawn inside it, menubar entries whose extents happen to fit the box
    // (Qt reports the main window's menubar items inside a chooser popup's
    // rectangle) are not its content.
    let rows: Vec<serde_json::Value> = items
        .iter()
        .filter(|entry| !role_of(entry).contains("menu"))
        .cloned()
        .collect();
    if !rows.is_empty() {
        rows
    } else if items.is_empty() {
        elements
    } else {
        items
    }
}

/// Elements that have a frame (visible, hit-testable on the screenshot)
/// come first, in walk order; frameless ones follow. A client that caps the
/// result then still shows the document, toolbar and sidebar controls.
pub(crate) fn framed_elements_first(elements: Vec<serde_json::Value>) -> Vec<serde_json::Value> {
    let (framed, frameless): (Vec<_>, Vec<_>) = elements
        .into_iter()
        .partition(|entry| entry.get("frame").is_some());
    framed.into_iter().chain(frameless).collect()
}

/// Same-pid windows mapped over a target window: transient dialogs (with
/// their modality) and override-redirect popups, plus whether any of them
/// overlaps the target's rectangle (then its own drawable is not what the
/// user sees).
#[derive(Default)]
pub(crate) struct WindowOverlays {
    pub(crate) dialogs: Vec<serde_json::Value>,
    pub(crate) popups: Vec<serde_json::Value>,
    pub(crate) covers_window: bool,
    pub(crate) window_rect: Option<(i32, i32, u32, u32)>,
}

impl WindowOverlays {
    /// One sentence per overlay naming the call that acts on it.
    pub(crate) fn follow_up(&self, pid: u32) -> Option<String> {
        let mut parts = Vec::new();
        for dialog in &self.dialogs {
            let id = dialog["window_id"].as_u64().unwrap_or(0);
            let owning_pid = dialog.get("owning_pid").and_then(|v| v.as_u64());
            parts.push(format!(
                "dialog \"{}\" (window_id {id}, transient of window {}{}{}) is open over this \
                 window: call get_window_state(pid={pid}, window_id={id}) to index it and act \
                 there (pid-only keys go to it).",
                dialog["title"].as_str().unwrap_or(""),
                dialog["transient_for"].as_u64().unwrap_or(0),
                if dialog["modal"].as_bool() == Some(true) {
                    ", modal"
                } else {
                    ""
                },
                owning_pid
                    .map(|owner| format!(
                        ", owned by a different process pid {owner}, not pid {pid}"
                    ))
                    .unwrap_or_default(),
            ));
        }
        for popup in &self.popups {
            let id = popup["window_id"].as_u64().unwrap_or(0);
            parts.push(format!(
                "popup (window_id {id}, bounds x={} y={} {}x{}) is open: call \
                 get_window_state(pid={pid}, window_id={id}) to index its items and click them \
                 by element_token.",
                popup["bounds"]["x"],
                popup["bounds"]["y"],
                popup["bounds"]["width"],
                popup["bounds"]["height"]
            ));
        }
        (!parts.is_empty()).then(|| parts.join(" "))
    }
}

pub(crate) fn rects_intersect(a: (i32, i32, u32, u32), b: (i32, i32, u32, u32)) -> bool {
    a.0 < b.0 + b.2 as i32
        && a.0 + a.2 as i32 > b.0
        && a.1 < b.1 + b.3 as i32
        && a.1 + a.3 as i32 > b.1
}

pub(crate) fn chromium_family_program(program: &str) -> bool {
    let basename = std::path::Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(program)
        .to_ascii_lowercase();
    ["chrome", "chromium", "electron", "brave", "edge"]
        .iter()
        .any(|needle| basename.contains(needle))
}

// ── shared helpers ────────────────────────────────────────────────────────────

/// Resolve a token element's center in window-local coordinates.
///
/// Returns `(xid, window_local_x, window_local_y)`.
/// Looks up the element's bounds (see [`crate::atspi::token_bounds`]), finds
/// the owning window (via `xid_hint` or the first window for `pid`), then
/// converts screen-absolute → window-local coords via compositor metadata on
/// Wayland or XTranslateCoordinates on X11.
pub(crate) fn resolve_element_local_coords(
    pid: u32,
    token: &crate::atspi::TokenElement,
    xid_hint: Option<u64>,
) -> anyhow::Result<(u64, f64, f64)> {
    let (bx, by, bw, bh) = crate::atspi::token_bounds(pid, xid_hint, token)?;
    let screen_cx = bx as f64 + bw as f64 / 2.0;
    let screen_cy = by as f64 + bh as f64 / 2.0;

    let xid = if let Some(x) = xid_hint {
        x
    } else if crate::wayland::wayland_input_enabled() {
        crate::wayland::list_windows_dispatch(Some(pid))
            .into_iter()
            .next()
            .map(|window| window.xid)
            .ok_or_else(|| anyhow::anyhow!("No Wayland windows for pid {pid}"))?
    } else {
        crate::x11::list_windows(Some(pid))
            .into_iter()
            .next()
            .map(|w| w.xid)
            .ok_or_else(|| anyhow::anyhow!("No windows for pid {pid}"))?
    };

    if crate::wayland::wayland_input_enabled() {
        let (window_x, window_y, window_width, window_height) =
            crate::wayland::window_geometry(xid)
                .ok_or_else(|| anyhow::anyhow!("No Wayland geometry for window {xid}"))?;
        if window_width == 0 || window_height == 0 {
            anyhow::bail!("Wayland window {xid} has no usable geometry");
        }
        return Ok((
            xid,
            screen_cx - window_x as f64,
            screen_cy - window_y as f64,
        ));
    }

    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::ConnectionExt as _;
    use x11rb::rust_connection::RustConnection;
    let (conn, screen_num) = RustConnection::connect(None)?;
    let root = conn.setup().roots[screen_num].root;
    let reply = conn
        .translate_coordinates(xid as u32, root, 0, 0)?
        .reply()?;
    let local_x = screen_cx - reply.dst_x as f64;
    let local_y = screen_cy - reply.dst_y as f64;
    Ok((xid, local_x, local_y))
}

/// An element whose AT-SPI extents do not fall inside its own toplevel: GTK4
/// reports empty extents for some widgets (a text view centre resolves to
/// (0,0), the GNOME hot corner), and a real pointer click there would hit
/// whatever is under that screen point. Refused before any input is sent.
#[derive(Debug)]
struct ElementBoundsUnusable {
    idx: usize,
    local: (f64, f64),
    window: (u32, u32),
}

impl std::fmt::Display for ElementBoundsUnusable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "element [{}] resolves to window-local ({:.0}, {:.0}), outside its {}x{} window; \
             the toolkit reported unusable AT-SPI extents, so no pointer input was sent",
            self.idx, self.local.0, self.local.1, self.window.0, self.window.1
        )
    }
}
impl std::error::Error for ElementBoundsUnusable {}

/// `resolve_element_local_coords` plus the sanity check that the centre lies
/// inside the window it belongs to. Blocking.
pub(crate) fn checked_element_local_coords(
    pid: u32,
    token: &crate::atspi::TokenElement,
    xid_hint: Option<u64>,
) -> anyhow::Result<(u64, f64, f64)> {
    let (xid, lx, ly) = resolve_element_local_coords(pid, token, xid_hint)?;
    if crate::wayland::wayland_input_enabled() {
        return Ok((xid, lx, ly));
    }
    use x11rb::protocol::xproto::ConnectionExt as _;
    use x11rb::rust_connection::RustConnection;
    let (conn, _) = RustConnection::connect(None)?;
    let geom = conn.get_geometry(xid as u32)?.reply()?;
    let (w, h) = (u32::from(geom.width), u32::from(geom.height));
    if lx < 0.0 || ly < 0.0 || lx >= f64::from(w) || ly >= f64::from(h) {
        return Err(ElementBoundsUnusable {
            idx: token.index,
            local: (lx, ly),
            window: (w, h),
        }
        .into());
    }
    Ok((xid, lx, ly))
}

pub(crate) fn element_screen_center(
    pid: u32,
    token: &crate::atspi::TokenElement,
    xid: Option<u64>,
) -> anyhow::Result<(f64, f64)> {
    element_screen_center_and_rect(pid, token, xid).map(|(center, _)| center)
}

/// The element's screen centre plus its AT-SPI extents as the overlay's
/// `[x, y, width, height]`, both from one bounds lookup (same screen space).
pub(crate) fn element_screen_center_and_rect(
    pid: u32,
    token: &crate::atspi::TokenElement,
    xid: Option<u64>,
) -> anyhow::Result<((f64, f64), [f64; 4])> {
    let (bx, by, bw, bh) = crate::atspi::token_bounds(pid, xid, token)?;
    let rect = [bx as f64, by as f64, bw as f64, bh as f64];
    Ok(((rect[0] + rect[2] / 2.0, rect[1] + rect[3] / 2.0), rect))
}

/// Shared schema for the optional `coordinate_frame` of pointer tools.
pub(crate) fn coordinate_frame_schema() -> Value {
    json!({
        "type": "string",
        "enum": ["window", "desktop"],
        "description": "Frame of x/y (and from_x/from_y/to_x/to_y). Default \"window\": \
            window-local screenshot pixels as returned by get_window_state. \"desktop\": \
            full-screen pixels as returned by get_desktop_state, translated to the target \
            window. Passing scope:\"desktop\" together with pid/window_id means the same thing."
    })
}

pub(crate) fn coordinate_frame_is_desktop(args: &Value) -> bool {
    args.get("coordinate_frame").and_then(Value::as_str) == Some("desktop")
}

pub(crate) fn window_scope_unproven_refusal(tool: &str, xid: u64, reason: &str) -> ToolResult {
    let message = format!(
        "{tool}: the element's membership in window_id {xid} is unproven ({reason}); \
         call get_window_state again"
    );
    ToolResult::error(message.clone()).with_structured(json!({
        "status": "refused",
        "refusal": { "code": "window_scope_unproven", "message": message }
    }))
}

/// Prove from a fresh walk that the token's observed element is still in the
/// token's window, then carry that element to dispatch. `Ok(None)` without a
/// token. Refuses before any dispatch. X11, Hyprland and compositors without
/// a GNOME Shell need no proof.
pub(crate) async fn prove_token_element(
    pid: u32,
    resolved: &cua_driver_core::element_token::ResolvedElement<
        crate::atspi::snapshot::CachedElement,
    >,
    tool: &str,
) -> Result<Option<crate::atspi::TokenElement>, ToolResult> {
    let cua_driver_core::element_token::ResolvedElement::Element {
        window_id,
        element_index,
        element,
    } = resolved
    else {
        return Ok(None);
    };
    let (xid, identity) = (*window_id, element.identity.clone());
    let proof = tokio::task::spawn_blocking(move || {
        crate::atspi::require_window_element(
            pid,
            xid,
            crate::atspi::WindowElement::Identity(identity),
        )
    })
    .await
    .unwrap_or_else(|error| Err(anyhow::anyhow!("task error: {error}")));
    token_element_for(tool, xid, *element_index, element.clone(), proof).map(Some)
}

pub(crate) fn token_element_for(
    tool: &str,
    xid: u64,
    index: usize,
    observed: crate::atspi::snapshot::CachedElement,
    proof: anyhow::Result<crate::atspi::WindowProof>,
) -> Result<crate::atspi::TokenElement, ToolResult> {
    proof
        .map(|proof| crate::atspi::TokenElement::new(index, observed, proof))
        .map_err(|error| window_scope_unproven_refusal(tool, xid, &error.to_string()))
}

/// The object an element token observed no longer answers: the token is
/// stale. Nothing reached that object, and it is never retargeted by index
/// after a window proof.
pub(crate) fn stale_observed_element(
    tool: &str,
    idx: usize,
    error: &anyhow::Error,
) -> Option<ToolResult> {
    error
        .is::<crate::atspi::native::CachedElementGone>()
        .then(|| {
            ToolResult::error(format!(
                "{tool}: stale_element_token: observed AT-SPI object [{idx}] is no longer \
                 present ({error}); re-snapshot with get_window_state"
            ))
            .with_structured(json!({ "code": "stale_element_token", "effect": "none" }))
        })
}

pub(crate) fn window_local_to_screen(xid: u64, x: f64, y: f64) -> anyhow::Result<(f64, f64)> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::ConnectionExt as _;
    use x11rb::rust_connection::RustConnection;

    let (conn, screen_num) = RustConnection::connect(None)?;
    let root = conn.setup().roots[screen_num].root;
    let reply = conn
        .translate_coordinates(xid as u32, root, 0, 0)?
        .reply()?;
    Ok((reply.dst_x as f64 + x, reply.dst_y as f64 + y))
}

/// Either `coordinate_frame:"desktop"` or `scope:"desktop"` marks `x,y` as
/// desktop (screen) pixels; both are translated exactly once by the caller.
pub(crate) fn desktop_frame_requested(args: &Value) -> bool {
    coordinate_frame_is_desktop(args)
        || args.get("scope").and_then(Value::as_str) == Some("desktop")
}

/// Convert desktop (screen) coordinates to `xid`'s window-local frame so the
/// rest of the pointer pipeline (which is window-local by contract) is reused.
pub(crate) fn desktop_to_window_local(xid: u64, x: f64, y: f64) -> anyhow::Result<(f64, f64)> {
    // A niri window id is no X11 window: never ask XWayland about it.
    if crate::wayland::is_wayland() && crate::wayland::niri::is_session() {
        return Err(crate::wayland::niri::desktop_frame_window_refusal(xid));
    }
    let (ox, oy) = window_local_to_screen(xid, 0.0, 0.0)?;
    Ok((x - ox, y - oy))
}

/// A failed desktop-to-window mapping; a coded refusal keeps its structure.
pub(crate) fn desktop_frame_unmapped(xid: u64, error: anyhow::Error) -> ToolResult {
    if crate::input::foreground::error_code(&error).is_some() {
        return input_error_result(error);
    }
    ToolResult::error(format!(
        "desktop-frame coordinates could not be mapped into window {xid}: {error}"
    ))
}

#[cfg(test)]
#[test]
fn a_niri_desktop_frame_window_refusal_is_structured() {
    let refused =
        desktop_frame_unmapped(31, crate::wayland::niri::desktop_frame_window_refusal(31));
    let structured = refused.structured_content.unwrap();
    assert_eq!(structured["code"], "foreground_unavailable");
    assert_eq!(structured["effect"], "none");
    assert!(structured["suggestion"]
        .as_str()
        .unwrap()
        .starts_with("Drop pid and window_id"));
}

/// The `steps` of a `drag` or `mouse_drag`, held to the advertised 1..=200
/// before the button goes down: every step is a motion sent with the
/// button held.
pub(crate) fn drag_steps(args: &Value) -> usize {
    args.u64_or("steps", 20).clamp(1, 200) as usize
}

#[cfg(test)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_timed_out_call_holds_the_desktop_until_its_late_cleanup_is_done() {
    use cua_driver_core::tool::DesktopCall;
    // niri: call 1 focused its window, then waited in niri past its budget.
    // Its worker gives focus back afterwards. Desktop typing that started in
    // between would have focus moved away under it.
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let call_1 = {
        let events = events.clone();
        DesktopCall::acquire()
            .await
            .unwrap()
            .scope(spawn_blocking_bounded(
                "call 1",
                std::time::Duration::from_millis(50),
                move || {
                    std::thread::sleep(std::time::Duration::from_millis(300));
                    // The next key is refused, but focus still goes back.
                    assert!(crate::input::action::admit(|| ()).is_err());
                    events.lock().unwrap().push("call 1 gave focus back");
                    Ok(())
                },
            ))
            .await
            .unwrap()
    };
    assert!(call_1.is_err());
    DesktopCall::acquire()
        .await
        .unwrap()
        .scope(async { events.lock().unwrap().push("call 2 typed") })
        .await;
    assert_eq!(
        *events.lock().unwrap(),
        ["call 1 gave focus back", "call 2 typed"]
    );
}

#[cfg(test)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn input_that_cannot_be_cancelled_never_overlaps_the_next_desktop_action() {
    use cua_driver_core::tool::DesktopCall;
    // A wtype child does not read the call's action, and an abandoned XTest
    // call still sends its releases: a worker whose call timed out, or whose
    // caller went away, can still send input. That input must not reach
    // whatever the next desktop action targets.
    for budget in [50, 60_000] {
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let worker_events = events.clone();
        let call = DesktopCall::acquire().await.unwrap();
        let call_1 = async move {
            call.scope(spawn_blocking_bounded(
                "call 1",
                std::time::Duration::from_millis(budget),
                move || {
                    std::thread::sleep(std::time::Duration::from_millis(300));
                    worker_events.lock().unwrap().push("call 1 typed");
                    Ok(())
                },
            ))
            .await
        };
        // A 50 ms budget times out; with 60 s the caller is dropped first.
        let _ = tokio::time::timeout(std::time::Duration::from_millis(100), call_1).await;
        DesktopCall::acquire()
            .await
            .unwrap()
            .scope(async { events.lock().unwrap().push("call 2 typed") })
            .await;
        assert_eq!(
            *events.lock().unwrap(),
            ["call 1 typed", "call 2 typed"],
            "budget {budget} ms"
        );
    }
}

#[cfg(test)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancelled_x11_press_is_recorded_before_the_next_call_on_its_cursor() {
    use cua_driver_core::tool::DesktopCall;
    let state = ToolState::new();
    let hold = |xid| MouseHoldState {
        pid: 1,
        xid,
        button: 1,
        x: 0.0,
        y: 0.0,
    };
    // Call 1: XTest is slow, and its client cancels the call.
    let call = DesktopCall::acquire().await.unwrap();
    let first = {
        let state = state.clone();
        async move {
            call.scope(press_and_record_x11_hold(
                state,
                "default".to_owned(),
                hold(1),
                || {
                    std::thread::sleep(std::time::Duration::from_millis(300));
                    Ok(())
                },
            ))
            .await
        }
    };
    let _ = tokio::time::timeout(std::time::Duration::from_millis(50), first).await;
    // Call 2 on the same cursor checks for a hold once it has the desktop.
    let _call = DesktopCall::acquire().await.unwrap();
    let held = state
        .mouse_hold
        .lock()
        .unwrap()
        .get("default")
        .map(|hold| hold.xid);
    assert_eq!(
        held,
        Some(1),
        "call 2 would find no hold, press window 2, and have call 1 overwrite its hold"
    );
}

#[cfg(test)]
#[tokio::test]
async fn a_late_x11_drag_press_is_followed_by_its_release() {
    // The X server answers the press only after the call timed out. The
    // drag then sends its own release, which arrives first.
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let pressed = press_x11_drag_start(
        std::time::Duration::from_millis(50),
        {
            let events = events.clone();
            move || {
                std::thread::sleep(std::time::Duration::from_millis(200));
                events.lock().unwrap().push("press");
                Ok(())
            }
        },
        {
            let events = events.clone();
            move || {
                events.lock().unwrap().push("release");
                Ok(())
            }
        },
    )
    .await
    .unwrap();
    assert!(pressed.is_err());
    events.lock().unwrap().push("release");
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    let events = events.lock().unwrap();
    assert!(events.contains(&"press"), "{events:?}");
    assert_eq!(events.last(), Some(&"release"), "{events:?}");
}

#[cfg(test)]
#[test]
fn drag_steps_stay_within_the_advertised_bounds() {
    // Without the bound, steps = u32::MAX holds the button through about
    // 4.3 billion motions.
    assert_eq!(drag_steps(&json!({ "steps": u32::MAX })), 200);
    assert_eq!(drag_steps(&json!({ "steps": 0 })), 1);
    assert_eq!(drag_steps(&json!({ "steps": 20 })), 20);
}

/// The press of an X11 step-by-step drag. A press that goes down after its
/// call ended is released here, in the same worker: the drag's own release
/// runs in another worker on its own X connection and can reach the app
/// first, which would leave the app's last event a press.
pub(crate) async fn press_x11_drag_start(
    budget: std::time::Duration,
    press: impl FnOnce() -> anyhow::Result<()> + Send + 'static,
    release: impl FnOnce() -> anyhow::Result<()> + Send + 'static,
) -> Result<anyhow::Result<()>, tokio::task::JoinError> {
    spawn_blocking_bounded("drag", budget, move || {
        press()?;
        if crate::input::action::admit(|| ()).is_err() {
            release()?;
        }
        Ok(())
    })
    .await
}

/// Press an X11 button and record its hold as soon as it is down, so a caller
/// that goes away before the await resumes still leaves a hold that
/// mouse_button_up can release. The worker keeps the call's desktop lease
/// through both, so the next call on the cursor sees this hold instead of
/// pressing and recording its own, which this one would then overwrite.
pub(crate) async fn press_and_record_x11_hold(
    state: Arc<ToolState>,
    cursor_id: String,
    hold: MouseHoldState,
    press: impl FnOnce() -> anyhow::Result<()> + Send + 'static,
) -> Result<anyhow::Result<()>, tokio::task::JoinError> {
    spawn_blocking_bounded("mouse_button_down", foreground_budget(0), move || {
        press()?;
        state.mouse_hold.lock().unwrap().insert(cursor_id, hold);
        Ok(())
    })
    .await
}

/// Upper bound for one foreground input transaction: focus confirmation plus
/// the injection itself. Typing sleeps per character, so scale with `chars`.
pub(crate) fn foreground_budget(chars: usize) -> std::time::Duration {
    std::time::Duration::from_millis(12_000 + (chars as u64) * 40)
}

/// The budget of a `type_text` call that types `chars` characters. niri's
/// keyboard gives itself its own deadline, which starts once the window is
/// focused and stops the call with how far it got; the call allows that
/// plus the foreground setup, so that deadline ends it first.
pub(crate) fn typing_budget(chars: usize, niri: bool) -> std::time::Duration {
    let budget = foreground_budget(chars);
    if niri {
        budget.max(foreground_budget(0) + crate::wayland::niri_keyboard_budget(chars))
    } else {
        budget
    }
}

#[cfg(test)]
#[test]
fn niri_typing_outlasts_the_keyboard_deadline() {
    // niri's keyboard allows itself a deadline per key and reports how far
    // it got when that passes. A call budget that ran out first would cut
    // the text off with a bare timeout, and a retry would type its start
    // again.
    for chars in [0, 1, 150, 1_000, 100_000] {
        assert!(
            typing_budget(chars, true) > crate::wayland::niri_keyboard_budget(chars),
            "{chars} characters"
        );
        assert_eq!(typing_budget(chars, false), foreground_budget(chars));
    }
}

/// [`foreground_budget`] for `chars` characters with `delay_ms` between them.
/// The delay comes from the caller unbounded.
pub(crate) fn paced_typing_budget(chars: usize, delay_ms: u64) -> std::time::Duration {
    foreground_budget(chars)
        + std::time::Duration::from_millis(delay_ms.saturating_mul(chars as u64))
}

#[cfg(test)]
#[test]
fn any_typing_delay_gives_a_budget() {
    assert_eq!(
        paced_typing_budget(2, 30),
        foreground_budget(2) + std::time::Duration::from_millis(60)
    );
    // Sizing the call must not panic, however long the caller's delay.
    let longest = paced_typing_budget(3, u64::MAX);
    assert!(longest >= std::time::Duration::from_millis(u64::MAX));
    crate::input::action::Action::new(longest);
}

/// Map an input-path error to a tool error, attaching the structured
/// foreground `code` (`foreground_unavailable` / `foreground_timeout`) when the
/// message carries one so the caller can branch instead of parsing prose.
pub(crate) fn input_error_result(e: anyhow::Error) -> ToolResult {
    let text = e.to_string();
    if crate::input::is_uinput_unavailable(&e) {
        // The focus-free real-input route exists but this process cannot open
        // /dev/uinput: an honest refusal the operator can act on, not a bare
        // I/O error.
        let mut refusal = with_uinput_hint(crate::input::delivery::background_unavailable_error(
            crate::input::delivery::BackgroundUnavailable::FocusedInputOnly,
        ));
        if let Some(structured) = refusal.structured_content.as_mut() {
            structured["cause"] = json!(crate::input::UINPUT_UNAVAILABLE_CODE);
            structured["detail"] = json!(text);
        }
        return refusal;
    }
    match crate::input::foreground::error_code(&e) {
        Some(code) => ToolResult::error(text.clone()).with_structured(json!({
            "code": code,
            "detail": text,
            "effect": "none",
            "suggestion": if code == crate::input::foreground::CODE_TIMEOUT {
                "Take a screenshot (get_desktop_state) to confirm the desktop is responsive, then retry once."
            } else {
                crate::wayland::niri::refusal_suggestion(&text).unwrap_or(
                    "Call bring_to_front on the window (or its modal dialog), confirm with get_window_state, then retry.",
                )
            },
        })),
        None => linux_input_error(e),
    }
}

#[cfg(test)]
#[test]
fn a_niri_refusal_suggests_its_own_remedy_instead_of_bring_to_front() {
    let refused = input_error_result(anyhow::anyhow!(
        "foreground_unavailable: the niri layout changed after the get_desktop_state screenshot, so its pixels may be elsewhere now; take a new screenshot"
    ));
    let structured = refused.structured_content.unwrap();
    assert_eq!(structured["code"], "foreground_unavailable");
    assert!(structured["suggestion"]
        .as_str()
        .unwrap()
        .starts_with("Take a new screenshot"));
}

/// Structured payload for a completed foreground transaction.
pub(crate) fn foreground_structured(
    path: &str,
    report: crate::input::ForegroundReport,
    extra: serde_json::Map<String, Value>,
) -> Value {
    let mut v = json!({
        "path": path,
        "verified": false,
        "delivery_mode": "foreground",
        "effect": "unverifiable",
    });
    apply_foreground_report(&mut v, &report);
    for (k, val) in extra {
        v[k] = val;
    }
    v
}

/// Fold a foreground transaction's post-check into a result: the report
/// itself, `focus_after` at the top level, and an `evidence` item so the
/// observation survives the public-record reduction. A window of the target
/// process that appeared or closed is `window_change` evidence and an
/// `effect: confirmed` wherever the focus sits afterwards (the OK button of a
/// dialog destroys the very window that held the focus, and closing the last
/// window of a process leaves the focus to the window manager); a change
/// among unattributed popups only counts while the focus stayed in the
/// process (`ForegroundReport::window_change_confirms`). Otherwise, focus that
/// left the process is a suspected no-op and anything else stays
/// unverifiable with a `native_api_result` item.
pub(crate) fn apply_foreground_report(v: &mut Value, report: &crate::input::ForegroundReport) {
    v["foreground"] = report.to_json();
    v["focus_after"] = json!(report.focus_after.as_str());
    let confirming_change = report
        .window_change
        .as_ref()
        .filter(|_| report.window_change_confirms());
    let evidence = match confirming_change {
        Some(change) => {
            v["effect"] = json!("confirmed");
            v["verified"] = json!(true);
            json!({
                "kind": "window_change",
                "detail": format!("focus_after={}; {change}", report.focus_after.as_str()),
            })
        }
        None if report.focus_after == crate::input::FocusAfter::Elsewhere => {
            v["effect"] = json!("suspected_noop");
            v["warning"] = json!(
                "input focus left the target process before the post-check; the input \
                 may have reached another window. Verify with a screenshot."
            );
            return;
        }
        None => json!({
            "kind": "native_api_result",
            "detail": format!(
                "real input delivered to the activated window (focus_after={}); {}",
                report.focus_after.as_str(),
                match &report.window_change {
                    Some(change) => format!("{change}, not attributed to the target process"),
                    None => "no window change observed".to_owned(),
                }
            ),
        }),
    };
    let mut items = v
        .get("evidence")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    items.push(evidence);
    v["evidence"] = json!(items);
}

/// Typed refusal for a pointer tool whose arguments name neither an element
/// nor a complete point. Before this check a call with only a pid was
/// executed at window-local (0, 0) — the top-left corner, i.e. the File menu.
pub(crate) fn invalid_pointer_arguments(tool: &str, detail: &str) -> ToolResult {
    let accepted = [
        "pid + element_token from get_window_state",
        "pid/window_id + x + y (window-local pixels of window_id; coordinate_frame:\"desktop\" for screen pixels)",
        "x + y + scope:\"desktop\" (screen pixels, no pid)",
    ];
    ToolResult::error(format!(
        "{tool}: invalid arguments: {detail}. Accepted forms: {}. No input was sent.",
        accepted.join("; ")
    ))
    .with_structured(json!({
        "code": "invalid_arguments",
        "tool": tool,
        "effect": "refused",
        "detail": detail,
        "accepted_forms": accepted,
    }))
}

/// `Some(refusal)` when a pointer tool reached its coordinate path without
/// both `x` and `y` (and, by construction, without an element).
pub(crate) fn require_point_args(tool: &str, args: &Value) -> Option<ToolResult> {
    let present = |name: &str| args.get(name).is_some_and(|v| v.is_number());
    match (present("x"), present("y")) {
        (true, true) => None,
        (false, false) => Some(invalid_pointer_arguments(
            tool,
            "no element_token and no x/y were given (a `target_id` is not an \
             element handle)",
        )),
        (true, false) => Some(invalid_pointer_arguments(tool, "x was given without y")),
        (false, true) => Some(invalid_pointer_arguments(tool, "y was given without x")),
    }
}

#[cfg(test)]
mod pointer_argument_tests {
    use super::*;

    #[test]
    fn pid_only_pointer_call_is_refused_not_clicked_at_origin() {
        let refusal = require_point_args("click", &json!({"target_id": "s1:147", "pid": 1867}))
            .expect("refused");
        assert_eq!(refusal.is_error, Some(true));
        let structured = refusal.structured_content.expect("structured");
        assert_eq!(structured["code"], "invalid_arguments");
        assert_eq!(structured["effect"], "refused");
        assert!(structured["accepted_forms"].as_array().unwrap().len() == 3);
    }

    #[test]
    fn complete_point_passes() {
        assert!(require_point_args(
            "click",
            &json!({"pid": 1, "window_id": 2, "x": 3, "y": 4.5})
        )
        .is_none());
        assert!(require_point_args("click", &json!({"x": 1, "y": null})).is_some());
        assert!(require_point_args("click", &json!({"y": 1})).is_some());
    }

    #[test]
    fn foreground_report_becomes_evidence() {
        let report = crate::input::ForegroundReport {
            already_active: false,
            retried_activation: false,
            confirm_ms: 12,
            focus_after: crate::input::FocusAfter::SamePid,
            window_change: Some("appeared: window 9 \"Brightness-Contrast\"".into()),
            window_change_owned: true,
        };
        let v = foreground_structured("x11_xtest_fg", report, serde_json::Map::new());
        assert_eq!(v["effect"], "confirmed");
        assert_eq!(v["focus_after"], "same_pid");
        assert_eq!(v["evidence"][0]["kind"], "window_change");
        let quiet = crate::input::ForegroundReport {
            already_active: true,
            retried_activation: false,
            confirm_ms: 0,
            focus_after: crate::input::FocusAfter::Target,
            window_change: None,
            window_change_owned: false,
        };
        let v = foreground_structured("x11_xtest_fg", quiet, serde_json::Map::new());
        assert_eq!(v["effect"], "unverifiable");
        assert_eq!(v["evidence"][0]["kind"], "native_api_result");
        let closed = crate::input::ForegroundReport {
            already_active: true,
            retried_activation: false,
            confirm_ms: 0,
            focus_after: crate::input::FocusAfter::Unknown,
            window_change: Some("closed: window 3 \"Format Cells\"".into()),
            window_change_owned: true,
        };
        let v = foreground_structured("x11_xtest_fg", closed, serde_json::Map::new());
        assert_eq!(v["effect"], "confirmed");
        assert_eq!(v["evidence"][0]["kind"], "window_change");
        // A popup that names no pid is listed in the target's window set (a VCL
        // menu carries none), but it may be another client's: once the focus
        // left the target process its change is no confirmation.
        let unattributed = |focus_after| crate::input::ForegroundReport {
            already_active: true,
            retried_activation: false,
            confirm_ms: 0,
            focus_after,
            window_change: Some("appeared: popup window 7 (200x300 at 1,2)".into()),
            window_change_owned: false,
        };
        let v = foreground_structured(
            "x11_xtest_fg",
            unattributed(crate::input::FocusAfter::Elsewhere),
            serde_json::Map::new(),
        );
        assert_eq!(v["effect"], "suspected_noop");
        let v = foreground_structured(
            "x11_xtest_fg",
            unattributed(crate::input::FocusAfter::Unknown),
            serde_json::Map::new(),
        );
        assert_eq!(v["effect"], "unverifiable");
        assert_eq!(v["evidence"][0]["kind"], "native_api_result");
        let v = foreground_structured(
            "x11_xtest_fg",
            unattributed(crate::input::FocusAfter::SamePid),
            serde_json::Map::new(),
        );
        assert_eq!(v["effect"], "confirmed");
        let lost = crate::input::ForegroundReport {
            already_active: true,
            retried_activation: false,
            confirm_ms: 0,
            focus_after: crate::input::FocusAfter::Elsewhere,
            window_change: None,
            window_change_owned: false,
        };
        let v = foreground_structured("x11_xtest_fg", lost, serde_json::Map::new());
        assert_eq!(v["effect"], "suspected_noop");
    }
}

pub(crate) fn parse_mouse_button(name: &str) -> u8 {
    match name {
        "right" => 3,
        "middle" => 2,
        _ => 1,
    }
}

/// Escalation hint for a non-AX / suspected-no-op surface — the cross-platform
/// `escalation` field mirrored from macOS. The recommended next rung depends on
/// the session type: X11 can pixel-target a specific window in the background,
/// so the deliberate move is an element px action — click by pixel (x,y) off the
/// screenshot already in this response (`px`). Native Wayland CANNOT
/// background-target an unfocused window (libei injects to the compositor's input
/// focus — see `input::delivery`), and the pixel path itself routes through the
/// same coordinate-free AT-SPI actuation that just no-op'd, so the move there is
/// to bring the window to the foreground first (`foreground`).
pub(crate) fn non_ax_escalation() -> Value {
    if crate::wayland::is_wayland() {
        json!({
            "recommended": "foreground",
            "reason": "non-AX surface on Wayland: a specific unfocused window can't be \
                       pixel-targeted in the background (libei injects to the compositor's \
                       focus). bring_to_front, then click by pixel off the screenshot \
                       with delivery_mode:\"foreground\"."
        })
    } else {
        json!({
            "recommended": "px",
            "reason": "non-AX surface — act by pixel (x,y) off the screenshot \
                       in this response (an element px action)."
        })
    }
}

/// True when `pid` is a Chromium-based embedder — a Chrome/Chromium browser or
/// any Electron/CEF app. On these surfaces an AT-SPI `EditableText.insertText`
/// can succeed at the bridge while the Chromium *renderer* never observes it,
/// so the AT-SPI "ax" rung must not be trusted as a confirmed insert (see
/// [`type_text_ax_result`]).
///
/// This is the Linux analogue of macOS `ElectronJs::is_electron` (which checks
/// for a bundled Electron Framework). Linux has no bundle, so the signal is
/// Chromium's multiprocess fingerprint: the embedder forks sandboxed helpers
/// whose argv carries `--type=renderer` / `--type=zygote` / `--type=gpu-process`.
/// Native GTK/Qt apps never spawn such helpers, so this is conservative (very
/// low false-positive). The single-process fallback also matches the embedder's
/// own argv. Reads `/proc`; cheap and only invoked on the rare AT-SPI confirm.
pub(crate) fn is_chromium_embedder(pid: u32) -> bool {
    fn argv_is_chromium_helper(p: u32) -> bool {
        fs::read(format!("/proc/{p}/cmdline"))
            .map(|raw| crate::at_point_policy::cmdline_is_chromium_helper(&raw))
            .unwrap_or(false)
    }
    // Single-process / the embedder itself carrying a Chromium switch.
    if argv_is_chromium_helper(pid) {
        return true;
    }
    // Build PPid → children across /proc, then BFS the descendants of `pid`
    // looking for a Chromium helper. Same /proc-walk shape as
    // `terminal_descendant_ttys`.
    let mut children: std::collections::HashMap<u32, Vec<u32>> = std::collections::HashMap::new();
    let entries = match fs::read_dir("/proc") {
        Ok(e) => e,
        Err(_) => return false,
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let child: u32 = match name.to_string_lossy().parse() {
            Ok(p) => p,
            Err(_) => continue,
        };
        let status = match fs::read_to_string(format!("/proc/{child}/status")) {
            Ok(s) => s,
            Err(_) => continue,
        };
        if let Some(ppid) = status
            .lines()
            .find(|l| l.starts_with("PPid:"))
            .and_then(|l| l[5..].trim().parse::<u32>().ok())
        {
            children.entry(ppid).or_default().push(child);
        }
    }
    let mut queue = std::collections::VecDeque::from([pid]);
    let mut seen = std::collections::HashSet::new();
    while let Some(cur) = queue.pop_front() {
        if !seen.insert(cur) {
            continue;
        }
        if let Some(kids) = children.get(&cur) {
            for &kid in kids {
                if argv_is_chromium_helper(kid) {
                    return true;
                }
                queue.push_back(kid);
            }
        }
    }
    false
}

pub(crate) fn is_webkitgtk_embedder(pid: u32) -> bool {
    fn argv_is_webkit_helper(pid: u32) -> bool {
        fs::read(format!("/proc/{pid}/cmdline"))
            .map(|raw| {
                let cmdline = String::from_utf8_lossy(&raw);
                cmdline.contains("WebKitWebProcess") || cmdline.contains("WebKitNetworkProcess")
            })
            .unwrap_or(false)
    }

    let mut children: std::collections::HashMap<u32, Vec<u32>> = std::collections::HashMap::new();
    let Ok(entries) = fs::read_dir("/proc") else {
        return false;
    };
    for entry in entries.flatten() {
        let Ok(child) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(status) = fs::read_to_string(format!("/proc/{child}/status")) else {
            continue;
        };
        if let Some(ppid) = status
            .lines()
            .find(|line| line.starts_with("PPid:"))
            .and_then(|line| line[5..].trim().parse::<u32>().ok())
        {
            children.entry(ppid).or_default().push(child);
        }
    }
    let mut queue = std::collections::VecDeque::from([pid]);
    let mut seen = std::collections::HashSet::new();
    while let Some(current) = queue.pop_front() {
        if !seen.insert(current) {
            continue;
        }
        if argv_is_webkit_helper(current) {
            return true;
        }
        if let Some(descendants) = children.get(&current) {
            queue.extend(descendants.iter().copied());
        }
    }
    false
}

pub(crate) fn maps_indicate_gtk(maps: &str) -> bool {
    maps.contains("libgtk-3.so") || maps.contains("libgtk-4.so")
}

/// Toolkits known to discard synthetic (`send_event`) X11 pointer events:
/// GTK3/4 (XInput2 only), LibreOffice VCL (all plugins), Qt5/6 (xcb, XI2).
pub(crate) fn maps_indicate_synthetic_pointer_dropped(maps: &str) -> bool {
    maps_indicate_gtk(maps)
        || maps.contains("libvcl")
        || maps.contains("libmergedlo")
        || maps.contains("libQt5Gui")
        || maps.contains("libQt6Gui")
}

pub(crate) fn synthetic_pointer_is_dropped(pid: u32) -> bool {
    fs::read_to_string(format!("/proc/{pid}/maps"))
        .map(|maps| maps_indicate_synthetic_pointer_dropped(&maps))
        .unwrap_or(false)
}

fn is_gtk_process(pid: u32) -> bool {
    fs::read_to_string(format!("/proc/{pid}/maps"))
        .map(|maps| maps_indicate_gtk(&maps))
        .unwrap_or(false)
}

pub(crate) fn unavailable_webkit_background(
    pid: u32,
    delivery: crate::input::delivery::DeliveryMode,
) -> Option<ToolResult> {
    (!delivery.is_foreground()
        && is_webkitgtk_embedder(pid)
        && !crate::wayland::is_inject_mode()
        && !crate::input::real_pointer_input_available())
    .then(|| {
        crate::input::delivery::background_unavailable_error(
            crate::input::delivery::BackgroundUnavailable::WebKitSyntheticInput,
        )
    })
}

pub(crate) fn unavailable_webkit_keyboard_background(
    pid: u32,
    delivery: crate::input::delivery::DeliveryMode,
) -> Option<ToolResult> {
    (!delivery.is_foreground() && is_webkitgtk_embedder(pid) && !crate::wayland::is_inject_mode())
        .then(|| {
            crate::input::delivery::background_unavailable_error(
                crate::input::delivery::BackgroundUnavailable::FocusedInputOnly,
            )
        })
}

/// Operator-facing hint attached to a background keyboard refusal on X11: the
/// focus-free route exists (XI2 MPX virtual master keyboard) but needs a
/// writable `/dev/uinput`, which is a one-time host setup step.
const UINPUT_KEYBOARD_HINT: &str = "Background keyboard delivery on X11 needs a virtual master \
    keyboard fed by a uinput device. Grant this process access to /dev/uinput \
    (modprobe uinput; chmod 0666 /dev/uinput, or add the driver's user to the \
    'input' group and restart it) and retry. Xvfb/Xtigervnc cannot hot-add \
    input devices, so the route is unavailable there.";

fn with_uinput_hint(mut refusal: ToolResult) -> ToolResult {
    refusal
        .content
        .push(cua_driver_core::protocol::Content::text(
            UINPUT_KEYBOARD_HINT,
        ));
    if let Some(structured) = refusal.structured_content.as_mut() {
        structured["hint"] = json!(UINPUT_KEYBOARD_HINT);
    }
    refusal
}

/// Honest refusal for a background keyboard action that has no focus-free
/// actuator. Carries the uinput hint whenever the MPX route is missing only
/// because `/dev/uinput` is not writable (the fixable case).
pub(crate) fn background_keyboard_refusal(atspi_ok: bool) -> ToolResult {
    let mut refusal = crate::input::delivery::background_unavailable_error(
        crate::input::delivery::BackgroundUnavailable::FocusedInputOnly,
    );
    let bus = crate::health_report::START_ATSPI_BUS_HINT;
    let hint = format!("AT-SPI is unavailable. {bus} Or use delivery_mode \"foreground\".");
    refusal
        .content
        .extend((!atspi_ok).then(|| cua_driver_core::protocol::Content::text(hint)));
    if !crate::wayland::is_wayland() && !crate::input::uinput_accessible() {
        with_uinput_hint(refusal)
    } else {
        refusal
    }
}

pub(crate) fn atspi_up() -> bool {
    let probe = std::thread::spawn(crate::health_report::probe_a11y_bus);
    crate::wayland::is_wayland() || probe.join().unwrap_or(false)
}

pub(crate) fn unavailable_gtk_keyboard_background(
    pid: u32,
    delivery: crate::input::delivery::DeliveryMode,
) -> Option<ToolResult> {
    (!delivery.is_foreground()
        && is_gtk_process(pid)
        && !crate::wayland::is_inject_mode()
        && !crate::input::real_keyboard_input_available())
    .then(|| background_keyboard_refusal(atspi_up()))
}

/// Outcome of one background/foreground keyboard delivery on X11, funnelled
/// through `spawn_blocking`'s success type so the tool can report the route.
pub(crate) enum KeyRoute {
    /// Activated the window and injected via XTest.
    Foreground(crate::input::ForegroundReport),
    /// Real key events through the session's XI2 virtual master keyboard.
    Mpx(crate::input::KeyboardDeliveryReport),
    /// Legacy synthetic XSendEvent (core-only toolkits, Tk, xterm).
    Synthetic,
    /// Written to the terminal's pty master.
    Terminal,
}

/// Background key delivery that honours a keyboard grab held by the target's
/// own popup (a Qt combo list / completer, a GTK or VCL menu): the X server
/// drops core key events from any other master keyboard to a client that
/// holds an active keyboard grab, so the virtual master keyboard route would
/// be silently lost; the core keyboard (XTest) reaches the grab holder, which
/// is the target itself, without any focus change.
pub(crate) fn background_virtual_key(
    cursor_id: &str,
    pid: u32,
    xid: u64,
    key: &str,
    modifiers: &[&str],
) -> anyhow::Result<crate::input::KeyboardDeliveryReport> {
    if let Some(popup) = crate::input::popup_of_pid(pid) {
        match crate::input::send_popup_keyboard_key(popup.window, key, modifiers) {
            Err(error) if error.is::<crate::input::PopupOwnerGone>() => {}
            delivered => return delivered,
        }
        return crate::input::send_keys_under_popup_grab(pid, popup, || {
            crate::input::send_key_xtest(key, modifiers)
        });
    }
    crate::input::send_virtual_keyboard_key(cursor_id, xid, key, modifiers)
}

/// Text counterpart of [`background_virtual_key`].
pub(crate) fn background_virtual_text(
    cursor_id: &str,
    pid: u32,
    xid: u64,
    text: &str,
) -> anyhow::Result<crate::input::KeyboardDeliveryReport> {
    if let Some(popup) = crate::input::popup_of_pid(pid) {
        match crate::input::send_popup_keyboard_text(popup.window, text) {
            Err(error) if error.is::<crate::input::PopupOwnerGone>() => {}
            delivered => return delivered,
        }
        return crate::input::send_keys_under_popup_grab(pid, popup, || {
            crate::input::send_type_text_xtest(text)
        });
    }
    crate::input::send_virtual_keyboard_text(cursor_id, xid, text)
}

pub(crate) fn is_popup_keyboard_grab(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<crate::input::PopupKeyboardGrab>()
        .is_some()
}

/// Deliver a background key/chord: prefer the focus-free real-input route
/// (MPX virtual master keyboard) when the host supports it, otherwise the
/// legacy synthetic XSendEvent. A uinput permission failure is surfaced as is
/// (the tool turns it into the refusal + hint); any other MPX failure falls
/// back to XSendEvent for the toolkits that still accept it, and is reported
/// as an error for the ones known to drop it.
pub(crate) fn background_key_route(
    cursor_id: &str,
    pid: u32,
    xid: u64,
    px_target: Option<(i32, i32)>,
    key: &str,
    modifiers: &[&str],
    element: Option<&crate::atspi::TokenElement>,
) -> anyhow::Result<KeyRoute> {
    if crate::input::real_keyboard_input_available() {
        // The element GrabFocus and the delivery run under one focus guard:
        // GTK can raise/activate the toplevel for either, and the guard
        // restores the user's focus and reports it (see `focus_guard`).
        let delivered = crate::input::focus_guard::guarded(Some(pid), || {
            grab_focus_for_background(pid, element)?;
            background_virtual_key(cursor_id, pid, xid, key, modifiers)
        });
        match delivered {
            Ok((mut report, guard)) => {
                report.focus_guard = guard;
                return Ok(KeyRoute::Mpx(report));
            }
            Err(error) if crate::input::is_uinput_unavailable(&error) => return Err(error),
            Err(error) if is_popup_keyboard_grab(&error) => return Err(error),
            Err(error)
                if is_grab_focus_failure(&error)
                    || is_gtk_process(pid)
                    || synthetic_pointer_is_dropped(pid) =>
            {
                return Err(error.context("virtual master keyboard delivery failed"));
            }
            Err(error) => tracing::warn!("MPX keyboard fell back to XSendEvent: {error}"),
        }
    }
    if let Some(element) = element {
        if !crate::atspi::focus_token(pid, element)? {
            anyhow::bail!(
                "AT-SPI Component.GrabFocus returned false for element {}",
                element.index
            );
        }
    }
    match px_target {
        Some((x, y)) => crate::input::send_key_at(xid, x, y, key, modifiers),
        None => crate::input::send_key(xid, key, modifiers),
    }
    .map(|()| KeyRoute::Synthetic)
}

const GRAB_FOCUS_FAILURE_PREFIX: &str = "grab_focus_failed";

/// AT-SPI `Component.GrabFocus` on the addressed element, for the background
/// keyboard routes. Failures carry a prefix so the caller never mistakes them
/// for a delivery failure worth a synthetic-event fallback.
fn grab_focus_for_background(
    pid: u32,
    element: Option<&crate::atspi::TokenElement>,
) -> anyhow::Result<()> {
    let Some(element) = element else {
        return Ok(());
    };
    let index = element.index;
    match crate::atspi::focus_token(pid, element) {
        Ok(true) => Ok(()),
        Ok(false) => anyhow::bail!(
            "{GRAB_FOCUS_FAILURE_PREFIX}: AT-SPI Component.GrabFocus returned false for element {index}"
        ),
        Err(error) => Err(error.context(format!(
            "{GRAB_FOCUS_FAILURE_PREFIX}: AT-SPI Component.GrabFocus for element {index}"
        ))),
    }
}

fn is_grab_focus_failure(error: &anyhow::Error) -> bool {
    error.to_string().starts_with(GRAB_FOCUS_FAILURE_PREFIX)
}

/// Merge a background focus-guard report into a finished tool result: the
/// structured `focus_changed` / `focus_restored` / `grab_held_by` fields plus
/// one sentence of text when anything moved.
pub(crate) fn attach_focus_guard(
    mut result: ToolResult,
    report: Option<&crate::input::FocusGuardReport>,
) -> ToolResult {
    let Some(report) = report else {
        return result;
    };
    let summary = report.summary();
    if !summary.is_empty() {
        result
            .content
            .push(cua_driver_core::protocol::Content::text(
                summary.trim().to_owned(),
            ));
    }
    let mut structured = result
        .structured_content
        .take()
        .unwrap_or_else(|| json!({}));
    for (key, value) in report.to_json().as_object().into_iter().flatten() {
        structured[key] = value.clone();
    }
    if let Some(item) = report.evidence_item() {
        push_evidence(&mut structured, item);
    }
    result.structured_content = Some(structured);
    result
}

/// Append an `evidence[]` item to a structured payload (the public action
/// contract keeps `evidence`, while flat diagnostic fields are reduced away).
pub(crate) fn push_evidence(structured: &mut Value, item: Value) {
    match structured.get_mut("evidence").and_then(Value::as_array_mut) {
        Some(items) => items.push(item),
        None => structured["evidence"] = json!([item]),
    }
}

/// Tool result for a completed keyboard route (`press_key` / `hotkey`).
pub(crate) fn key_route_result(action: &str, route: KeyRoute, mode_label: &str) -> ToolResult {
    match route {
        KeyRoute::Foreground(report) => ToolResult::text(format!(
            "{action} as a real key event (delivery_mode=foreground, focus_after={}); \
             not verified — confirm with a screenshot.",
            report.focus_after.as_str()
        ))
        .with_structured(foreground_structured(
            "key_events_fg",
            report,
            serde_json::Map::new(),
        )),
        KeyRoute::Mpx(report) => {
            let mut structured = report.to_json();
            structured["verified"] = json!(false);
            structured["delivery_mode"] = json!(mode_label);
            structured["effect"] = json!("unverifiable");
            let mut text = format!(
                "{action} as real key events {} (delivery_mode={mode_label}); not verified — \
                 confirm with a screenshot.",
                report.route_phrase()
            );
            let closed_target = report
                .focus_guard
                .as_ref()
                .is_some_and(|guard| guard.closed_window.is_some());
            let opened_window = report
                .focus_guard
                .as_ref()
                .and_then(|guard| guard.same_app_window.as_ref())
                .filter(|_| report.delivery_confirmed);
            if closed_target && report.delivery_confirmed {
                // The key closed the window that held the focus (Escape /
                // Return on a dialog): the lost virtual focus is the effect,
                // and the window change is evidence the contract publishes.
                structured["effect"] = json!("confirmed");
                structured["verified"] = json!(true);
                text.push_str(" The window that held the focus closed after the key.");
            } else if let Some(window) = opened_window {
                // The key mapped a new top-level of the target (F4 -> Position
                // and Size, ctrl+1 -> Format Cells): the window change is the
                // effect; name it and the call that targets it.
                structured["effect"] = json!("confirmed");
                structured["verified"] = json!(true);
                structured["window_opened"] = json!({
                    "window_id": window.window,
                    "title": window.title,
                    "focused": window.focused,
                });
                text.push_str(&format!(
                    " The key opened window {} \"{}\"; call get_window_state(pid, window_id={}) \
                     to index it and act there.",
                    window.window, window.title, window.window
                ));
            } else if !report.virtual_focus_held || !report.delivery_confirmed {
                structured["effect"] = json!("suspected_noop");
                text.push_str(
                    " Warning: the target lost the virtual keyboard's focus or the server \
                     did not confirm the last key; the keys may not have landed.",
                );
            }
            text.push_str(&report.delivery_notes());
            ToolResult::text(text).with_structured(structured)
        }
        KeyRoute::Synthetic | KeyRoute::Terminal => {
            ToolResult::text(format!("{action} (delivery_mode={mode_label})."))
                .with_structured(json!({ "verified": false, "delivery_mode": mode_label }))
        }
    }
}

pub(crate) fn unavailable_gtk_pointer_background(
    pid: u32,
    delivery: crate::input::delivery::DeliveryMode,
) -> Option<ToolResult> {
    (!delivery.is_foreground()
        && is_gtk_process(pid)
        && !crate::wayland::is_inject_mode()
        && !crate::input::real_pointer_input_available())
    .then(|| {
        crate::input::delivery::background_unavailable_error(
            crate::input::delivery::BackgroundUnavailable::FocusedInputOnly,
        )
    })
}

pub(crate) fn unavailable_wayland_focused_input_background(
    delivery: crate::input::delivery::DeliveryMode,
    focus_free_inject_supported: bool,
) -> Option<ToolResult> {
    (crate::wayland::wayland_input_enabled()
        && !(focus_free_inject_supported && crate::wayland::is_inject_mode())
        && !delivery.is_foreground())
    .then(|| {
        crate::input::delivery::background_unavailable_error(
            crate::input::delivery::BackgroundUnavailable::FocusedInputOnly,
        )
    })
}

#[derive(Debug)]
pub(crate) enum CoordinateContext {
    Zoom(ZoomContext),
    Screenshot(f64),
}

/// Chromium's X11 renderer drops synthetic input sent to an occluded,
/// unfocused toplevel. Returning success here would be a silent loss, so all
/// input tools expose the same typed refusal and leave foreground activation
/// as the explicit escalation.
pub(crate) fn unavailable_chromium_background(
    pid: u32,
    delivery: crate::input::delivery::DeliveryMode,
) -> Option<ToolResult> {
    if delivery.is_foreground() || !is_chromium_embedder(pid) {
        return None;
    }
    // Chromium is an XInput2 client: real (non-synthetic) events from the MPX
    // virtual master pointer/keyboard reach its renderer like the private
    // compositor's do, so only the synthetic-XSendEvent-only host must refuse.
    let focus_free_real_input =
        crate::wayland::is_inject_mode() || crate::input::real_pointer_input_available();
    if chromium_background_must_refuse(delivery.is_foreground(), focus_free_real_input, true) {
        Some(crate::input::delivery::background_unavailable_error(
            crate::input::delivery::BackgroundUnavailable::ChromiumInput,
        ))
    } else {
        None
    }
}

pub(crate) fn chromium_background_must_refuse(
    foreground: bool,
    focus_free_inject_mode: bool,
    chromium: bool,
) -> bool {
    chromium && !foreground && !focus_free_inject_mode
}

/// Screen-absolute center of a window (top-left from translate_coordinates plus
/// half its geometry). Used to position the no-focus-steal scroll over the
/// window's content. Blocking — call inside spawn_blocking.
pub(crate) fn window_screen_center(xid: u64) -> anyhow::Result<(i32, i32)> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::ConnectionExt as _;
    use x11rb::rust_connection::RustConnection;

    let (conn, screen_num) = RustConnection::connect(None)?;
    let root = conn.setup().roots[screen_num].root;
    let geom = conn.get_geometry(xid as u32)?.reply()?;
    let trans = conn
        .translate_coordinates(xid as u32, root, 0, 0)?
        .reply()?;
    Ok((
        trans.dst_x as i32 + geom.width as i32 / 2,
        trans.dst_y as i32 + geom.height as i32 / 2,
    ))
}

/// How a background pointer action was delivered on X11, with the cheap
/// post-checks the real-pointer route can make.
#[derive(Debug)]
pub(crate) enum PointerRoute {
    /// Real button events from the session's MPX virtual master pointer.
    Mpx(crate::input::PointerEffect),
    /// A background pixel click resolved to an accessible under the point and
    /// fired its action (or selected it through its container).
    Atspi(crate::atspi::AtPointHit),
    /// Target-addressed synthetic `XSendEvent` (core-only toolkits).
    Synthetic,
    /// Activated the window first and used XTest (delivery_mode=foreground).
    Foreground,
    /// Native Wayland pointer route.
    Wayland,
}

impl PointerRoute {
    /// Result `path`; the Wayland routes keep their own tool-level labels.
    pub(crate) fn path(&self) -> Option<&'static str> {
        match self {
            Self::Mpx(_) => Some(crate::input::MPX_POINTER_PATH),
            Self::Atspi(_) => Some("x11_atspi"),
            Self::Synthetic => Some("x11_xsendevent"),
            Self::Foreground => Some("x11_xtest_fg"),
            Self::Wayland => None,
        }
    }

    /// Structured fields shared by click / double_click / right_click / drag:
    /// `path`, `verified`, `effect`, and for the MPX route the post-checks
    /// (`focus_unchanged`, `region_diff_pct`). `verified:true` + `effect:
    /// "landed"` only when the screen around the point visibly reacted.
    pub(crate) fn structured(&self, mode_label: &str) -> Value {
        let mut v = json!({
            "verified": false,
            "effect": "unverifiable",
            "delivery_mode": mode_label,
        });
        if let Some(path) = self.path() {
            v["path"] = json!(path);
        }
        if let Self::Atspi(hit) = self {
            v["hit"] = json!({
                "role": hit.role,
                "name": hit.name,
                "action": hit.action,
                "path": hit.path,
            });
            if let Some(selected) = &hit.selected {
                v["selected"] = json!(selected);
            }
            // The read-back of the container's selection state is real
            // accessibility evidence; a fired action is only a native call
            // whose effect the caller still confirms.
            let evidence = if hit.selection_verified {
                v["verified"] = json!(true);
                v["effect"] = json!("confirmed");
                json!({
                    "kind": "accessibility_readback",
                    "detail": hit.describe(),
                })
            } else {
                json!({
                    "kind": "native_api_result",
                    "detail": hit.describe(),
                })
            };
            v["evidence"] = json!([evidence]);
        }
        if let Self::Mpx(effect) = self {
            v["focus_unchanged"] = json!(effect.focus_unchanged);
            v["screen_point"] = json!([effect.x, effect.y]);
            if let Some((wx, wy)) = effect.window_point {
                v["window_point"] = json!([wx, wy]);
            }
            if let Some(pct) = effect.region_diff_pct {
                v["region_diff_pct"] = json!((pct * 100.0).round() / 100.0);
            }
            v["popups_appeared"] = json!(effect.popups_appeared);
            if let Some(popup) = effect.popups.last() {
                v["popup"] = popup.to_json();
            }
            if !effect.popups.is_empty() {
                v["popups"] = json!(effect
                    .popups
                    .iter()
                    .map(|p| p.to_json())
                    .collect::<Vec<_>>());
            }
            // Evidence the action record can publish. A popup (menu, popover,
            // combo list) appearing is a window change the public contract
            // accepts for `effect: confirmed`; a screen-region change is
            // recorded but, per the contract, cannot promote the effect on
            // its own — the text still reports it.
            let mut evidence = Vec::new();
            if effect.popups_appeared > 0 {
                evidence.push(json!({
                    "kind": "window_change",
                    "detail": format!(
                        "{} appeared after the {} at ({}, {}); get_window_state(pid, window_id=<popup window_id>) lists its items",
                        popup_list(&effect.popups),
                        self.path().unwrap_or("pointer"),
                        effect.x,
                        effect.y
                    ),
                }));
            }
            if let Some(pct) = effect
                .region_diff_pct
                .filter(|pct| *pct >= crate::input::PointerEffect::LANDED_THRESHOLD_PCT)
            {
                evidence.push(json!({
                    "kind": "screenshot_comparison",
                    "detail": format!("{pct:.2}% of the screen region around ({}, {}) changed", effect.x, effect.y),
                }));
            }
            // The background focus guard ran around the press train: report
            // what the application moved and whether it was restored. An own
            // window it opened is a window change the contract publishes.
            if let Some(guard) = &effect.focus_guard {
                for (key, value) in guard.to_json().as_object().into_iter().flatten() {
                    v[key] = value.clone();
                }
                evidence.extend(guard.evidence_item());
            }
            if let Some(foreign) = &effect.foreign_window {
                v["foreign_window"] = foreign.to_json();
                if foreign.changed() {
                    evidence.push(json!({
                        "kind": "window_change",
                        "detail": foreign.describe(),
                    }));
                }
            }
            if let Some(cover) = &effect.retargeted_to {
                v["retargeted_to"] = json!({
                    "window_id": cover.window,
                    "title": cover.title,
                });
                evidence.push(json!({
                    "kind": "window_change",
                    "detail": format!(
                        "the point was under the application's own window {} \"{}\"; the press went to it",
                        cover.window, cover.title
                    ),
                }));
            }
            if !evidence.is_empty() {
                v["evidence"] = json!(evidence);
            }
            if effect.landed() || effect.foreign_window.as_ref().is_some_and(|f| f.changed()) {
                v["verified"] = json!(true);
                v["effect"] = json!("confirmed");
            }
        }
        v
    }

    /// Human-readable tail for the result text.
    pub(crate) fn text_suffix(&self, mode_label: &str) -> String {
        let mut text = self.text_suffix_inner(mode_label);
        if let Self::Mpx(effect) = self {
            if let Some(popup) = effect.popups.last() {
                text.push_str(&format!(
                    " {} opened (popup: window_id={}, bounds x={} y={} {}x{}); call \
                     get_window_state(pid={}, window_id={}) to index its items and click them \
                     by element_token, or click(window_id={}, x, y) with popup-local pixels.",
                    popup.describe(),
                    popup.window,
                    popup.x,
                    popup.y,
                    popup.width,
                    popup.height,
                    popup
                        .pid
                        .map(|p| p.to_string())
                        .unwrap_or_else(|| "<pid>".to_owned()),
                    popup.window,
                    popup.window
                ));
            }
            if let Some(cover) = &effect.retargeted_to {
                text.push_str(&format!(
                    " The point was under this application's own window {} \"{}\" \
                     (retargeted_to), so the press went to that window.",
                    cover.window, cover.title
                ));
            }
            if let Some(foreign) = &effect.foreign_window {
                text.push(' ');
                text.push_str(&foreign.describe());
            }
            if let Some(guard) = &effect.focus_guard {
                text.push_str(&guard.summary());
            }
        }
        text
    }

    fn text_suffix_inner(&self, mode_label: &str) -> String {
        match self {
            Self::Atspi(hit) if hit.selected.is_some() => format!(
                "(delivery_mode={mode_label}, path=x11_atspi); {}{}",
                hit.describe(),
                if hit.selection_verified {
                    " — the item is selected."
                } else {
                    "; confirm with a screenshot."
                }
            ),
            Self::Atspi(hit) => format!(
                "(delivery_mode={mode_label}, path=x11_atspi); {}; the AT-SPI action was \
                 dispatched, not verified — confirm with a screenshot.",
                hit.describe()
            ),
            Self::Mpx(effect) if effect.landed() => format!(
                "(delivery_mode={mode_label}, path={}, focus {}); {}the screen around the \
                 point changed ({:.1}% of the region) — the action landed.",
                crate::input::MPX_POINTER_PATH,
                if effect.focus_unchanged {
                    "untouched"
                } else {
                    "restored"
                },
                if effect.popups_appeared > 0 {
                    format!("{} popup window(s) appeared and ", effect.popups_appeared)
                } else {
                    String::new()
                },
                effect.region_diff_pct.unwrap_or(0.0)
            ),
            Self::Mpx(effect) => format!(
                "(delivery_mode={mode_label}, path={}, focus {}); real button events were \
                 delivered but the screen around the point did not visibly change \
                 ({}) — confirm with a screenshot.",
                crate::input::MPX_POINTER_PATH,
                if effect.focus_unchanged {
                    "untouched"
                } else {
                    "restored"
                },
                effect
                    .region_diff_pct
                    .map(|pct| format!("{pct:.2}% of the region"))
                    .unwrap_or_else(|| "region capture unavailable".to_owned())
            ),
            _ => format!(
                "(delivery_mode={mode_label}{}); not verified — confirm with a screenshot.",
                self.path()
                    .map(|p| format!(", path={p}"))
                    .unwrap_or_default()
            ),
        }
    }
}

/// The focus-free real-pointer route existed but the press did not go
/// through, on a toolkit that would silently drop the synthetic fallback.
#[derive(Debug)]
struct BackgroundPointerFailed {
    reason: String,
}

impl std::fmt::Display for BackgroundPointerFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "background pointer delivery failed: the virtual master pointer route did not \
             go through ({}) and this toolkit drops synthetic pointer events, so no click \
             was delivered",
            self.reason
        )
    }
}
impl std::error::Error for BackgroundPointerFailed {}

/// X11 no-focus-steal pixel click. On a real Xorg host the MPX uinput pointer
/// lands a *true* button event on XInput2 toolkits (GTK3/4, VCL, Qt,
/// Chromium) that silently drop synthetic `XSendEvent` pointers — so right /
/// middle / double clicks actually register. It refuses (typed
/// [`crate::input::TargetOccluded`]) when another toplevel covers the point.
/// On Xvfb / Xtigervnc / unsupported servers (`real_pointer_input_available()`
/// returns false) it falls back to the legacy `XSendEvent` path so headless
/// tests and core-only toolkits keep working; when the MPX attempt itself
/// fails, the fallback is only taken for toolkits that accept synthetic
/// events, otherwise a typed [`BackgroundPointerFailed`] is returned. `lx`,`ly`
/// are window-local; screen-absolute coords for the warp are derived here.
/// Blocking — call inside spawn_blocking.
/// `popup window A "x" (..), popup window B (..)` for evidence text.
fn popup_list(popups: &[crate::input::PopupWindow]) -> String {
    if popups.is_empty() {
        return "a popup window".to_owned();
    }
    popups
        .iter()
        .map(crate::input::PopupWindow::describe)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Every pixel action reports the point it resolved, success or refusal, so
/// a coordinate-frame mistake is visible: `window_point` is what the caller
/// passed (window-local pixels after zoom / resize translation),
/// `screen_point` is where that lands on the screen.
pub(crate) fn with_pixel_points(
    mut result: ToolResult,
    screen: Option<(i32, i32)>,
    window: (i32, i32),
) -> ToolResult {
    let mut structured = result
        .structured_content
        .take()
        .unwrap_or_else(|| json!({}));
    structured["window_point"] = json!([window.0, window.1]);
    let mut note = format!(" [window_point=({}, {})", window.0, window.1);
    if let Some((sx, sy)) = screen {
        structured["screen_point"] = json!([sx, sy]);
        note.push_str(&format!(" screen_point=({sx}, {sy})"));
    }
    note.push(']');
    result.structured_content = Some(structured);
    if let Some(cua_driver_core::protocol::Content::Text { text, .. }) = result.content.first_mut()
    {
        text.push_str(&note);
    } else {
        result
            .content
            .push(cua_driver_core::protocol::Content::text(
                note.trim().to_owned(),
            ));
    }
    result
}

pub(crate) fn x11_pixel_click_no_focus_steal(
    cursor_id: &str,
    xid: u64,
    lx: i32,
    ly: i32,
    button: u8,
    count: usize,
) -> anyhow::Result<PointerRoute> {
    if crate::input::real_pointer_input_available() {
        if let Ok((sx, sy)) = window_local_to_screen(xid, lx as f64, ly as f64) {
            // The press train has its own save/restore fast path; the focus
            // guard around it catches what the application does afterwards
            // (a menu grab, a dialog it maps) and reports it.
            let target_pid = crate::x11::window_pid(xid);
            let (outcome, guard) = crate::input::focus_guard::guarded_settled(target_pid, || {
                Ok(crate::input::send_virtual_pointer_click(
                    cursor_id,
                    &crate::input::VirtualPointerClick {
                        target_window: xid,
                        x: sx.round() as i32,
                        y: sy.round() as i32,
                        button,
                        count,
                    },
                ))
            })?;
            match outcome {
                Ok(mut effect) => {
                    effect.focus_guard = guard;
                    effect.window_point = Some((lx, ly));
                    return Ok(PointerRoute::Mpx(effect));
                }
                Err(error) if crate::input::is_uinput_unavailable(&error) => return Err(error),
                Err(error)
                    if error
                        .downcast_ref::<crate::input::TargetOccluded>()
                        .is_some()
                        || error
                            .downcast_ref::<crate::input::PointOutsideWindow>()
                            .is_some() =>
                {
                    return Err(error)
                }
                // The synthetic fallback is a silent no-op on GTK/VCL/Qt:
                // report the real-input failure instead of a click that
                // changed nothing.
                Err(error)
                    if crate::x11::window_pid(xid).is_some_and(synthetic_pointer_is_dropped) =>
                {
                    tracing::warn!("MPX click failed on a toolkit that drops synthetic pointer events: {error:#}");
                    return Err(BackgroundPointerFailed {
                        reason: format!("{error:#}"),
                    }
                    .into());
                }
                Err(e) => tracing::warn!("MPX click fell back to XSendEvent: {e:#}"),
            }
        }
    }
    crate::input::send_click(xid, lx, ly, count, button).map(|()| PointerRoute::Synthetic)
}

/// Typed tool error for a Linux input failure: uinput permission, an occluded
/// target, or a real-pointer delivery that did not go through. Everything
/// else keeps its full error chain in the text.
pub(crate) fn linux_input_error(error: anyhow::Error) -> ToolResult {
    if crate::input::is_uinput_unavailable(&error) {
        return ToolResult::error(error.to_string()).with_structured(json!({
            "code": crate::input::UINPUT_UNAVAILABLE_CODE,
        }));
    }
    if let Some(occluded) = error.downcast_ref::<crate::input::TargetOccluded>() {
        let hint = format!(
            "The point is under window {}{}{}, which belongs to a different application, so \
             a background pointer press there would hit that window instead of the target; \
             no input was sent. Either act on that window (pass its window_id{} and click / \
             press keys there, or dismiss it), click the target by element_token (an AT-SPI \
             action that needs no pointer), or pick a point of the target that is not \
             covered (list_windows gives bounds and z_index).",
            occluded.covering_window,
            if occluded.covering_title.is_empty() {
                String::new()
            } else {
                format!(" \"{}\"", occluded.covering_title)
            },
            occluded
                .covering_pid
                .map(|pid| format!(" of pid {pid}"))
                .unwrap_or_default(),
            occluded
                .covering_pid
                .map(|pid| format!(" with pid {pid}"))
                .unwrap_or_default(),
        );
        return ToolResult::error(format!("{occluded}. {hint}")).with_structured(json!({
            "code": "target_occluded",
            "effect": "refused",
            "path": crate::input::MPX_POINTER_PATH,
            "target_window": occluded.target_window,
            "covering_window": occluded.covering_window,
            "covering_title": occluded.covering_title,
            "covering_pid": occluded.covering_pid,
            "screen_point": [occluded.x, occluded.y],
            "hint": hint,
        }));
    }
    if let Some(grab) = error.downcast_ref::<crate::input::PopupKeyboardGrab>() {
        let hint = format!(
            "Keys for pid {} cannot be delivered while its {} is open: the popup's keyboard \
             grab drops keys from the virtual keyboard, and the core keyboard would go to the \
             focused application instead. Dismiss the popup first (click outside it, or \
             click an item in it with pid/window_id={}), or click the popup's item by \
             element_token after get_window_state(pid={}, window_id={}); then retry.",
            grab.pid,
            grab.popup.describe(),
            grab.popup.window,
            grab.pid,
            grab.popup.window
        );
        return ToolResult::error(format!("{grab}. {hint}")).with_structured(json!({
            "code": "popup_keyboard_grab",
            "effect": "refused",
            "verified": false,
            "popup": grab.popup.to_json(),
            "focus_owner_pid": grab.focus_owner,
            "hint": hint,
        }));
    }
    if let Some(outside) = error.downcast_ref::<crate::input::PointOutsideWindow>() {
        let (bx, by, bw, bh) = outside.bounds;
        let hint = format!(
            "x/y are window-local pixels of window {} (0..{bw} x 0..{bh}, its top-left is at \
             screen ({bx}, {by})). Pass window-local coordinates from get_window_state's \
             screenshot, or screen pixels with coordinate_frame:\"desktop\".",
            outside.target_window
        );
        return ToolResult::error(format!("{outside}. {hint}")).with_structured(json!({
            "code": "point_outside_window",
            "effect": "refused",
            "path": crate::input::MPX_POINTER_PATH,
            "target_window": outside.target_window,
            "screen_point": [outside.screen_x, outside.screen_y],
            "window_point": [outside.window_x, outside.window_y],
            "window_bounds": { "x": bx, "y": by, "width": bw, "height": bh },
            "hint": hint,
        }));
    }
    if let Some(unusable) = error.downcast_ref::<ElementBoundsUnusable>() {
        let hint = "Re-snapshot with get_window_state and click by pixel (x/y) from the \
             screenshot, or pick a child element that reports a frame.";
        return ToolResult::error(format!("{unusable}. {hint}")).with_structured(json!({
            "code": "element_bounds_unavailable",
            "effect": "none",
            "element_index": unusable.idx,
            "hint": hint,
        }));
    }
    if let Some(failed) = error.downcast_ref::<BackgroundPointerFailed>() {
        let hint = "Click by element_token (AT-SPI action) or retry with \
             delivery_mode:\"foreground\".";
        return ToolResult::error(format!("{failed}. {hint}")).with_structured(json!({
            "code": "background_pointer_failed",
            "effect": "none",
            "path": crate::input::MPX_POINTER_PATH,
            "reason": failed.reason,
            "hint": hint,
        }));
    }
    ToolResult::error(format!("{error:#}"))
}

pub(crate) fn isolated_hyprland_background(delivery: crate::input::delivery::DeliveryMode) -> bool {
    !delivery.is_foreground() && crate::wayland::hyprland_input::enabled()
}

pub(crate) fn hyprland_foreground(delivery: crate::input::delivery::DeliveryMode) -> bool {
    delivery.is_foreground()
        && crate::wayland::wayland_input_enabled()
        && crate::wayland::hyprland::is_session()
}

/// AT-SPI roles whose `activate`/`press` action does not move the widget
/// focus into them, so a following keystroke would land elsewhere; a real
/// pointer click at their centre is what a user does.
pub(crate) fn element_needs_real_click(role: &str) -> bool {
    crate::atspi::is_focus_taking_role(role)
}

/// Menubar entries and the items of an open menu.
pub(crate) fn element_is_menu_role(role: &str) -> bool {
    matches!(
        role.trim().to_ascii_lowercase().as_str(),
        "menu" | "menu item" | "check menu item" | "radio menu item"
    )
}

/// A chord the window manager, not the application, acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WmChord {
    /// Alt+F4: close the target window (`_NET_CLOSE_WINDOW`).
    CloseWindow,
    /// Super+*, Alt+Tab, Ctrl+Alt+*, Alt+F7/F8/F10/space/Escape: a WM binding
    /// with no window-addressed equivalent.
    Unavailable,
}

/// mutter's key bindings are passive grabs on the core keyboard. A chord from
/// the session's virtual master keyboard (background delivery) reaches the
/// client window, never the WM, so `alt+F4` / `super+a` / `alt+tab` silently
/// do nothing. Recognise them so the tool can act (close the window through
/// EWMH) or refuse with a typed reason instead of reporting `unverifiable`.
fn wm_chord_kind(key: &str, modifiers: &[String]) -> Option<WmChord> {
    let mods: Vec<String> = modifiers
        .iter()
        .map(|m| m.trim().to_ascii_lowercase())
        .collect();
    let has = |names: &[&str]| mods.iter().any(|m| names.contains(&m.as_str()));
    let alt = has(&["alt", "alt_l", "alt_r", "option"]);
    let ctrl = has(&["ctrl", "control", "ctrl_l", "ctrl_r"]);
    let shift = has(&["shift", "shift_l", "shift_r"]);
    let super_ = has(&[
        "super", "super_l", "super_r", "meta", "win", "cmd", "command", "hyper",
    ]);
    let key_lower = key.trim().to_ascii_lowercase();
    if super_ {
        return Some(WmChord::Unavailable);
    }
    if alt && ctrl {
        return Some(WmChord::Unavailable);
    }
    if alt && !ctrl && key_lower == "f4" {
        return Some(WmChord::CloseWindow);
    }
    if alt
        && !ctrl
        && !shift
        && matches!(
            key_lower.as_str(),
            "tab" | "f5" | "f7" | "f8" | "f10" | "space" | "escape" | "esc"
        )
    {
        return Some(WmChord::Unavailable);
    }
    if alt && shift && key_lower == "tab" {
        return Some(WmChord::Unavailable);
    }
    None
}

/// Background delivery of a WM-level chord: close the window for Alt+F4,
/// refuse the rest (`wm_chord_unavailable`). `None` for ordinary chords.
pub(crate) async fn wm_chord_background(
    pid: u32,
    xid: u64,
    key: &str,
    modifiers: &[String],
    delivery: crate::input::delivery::DeliveryMode,
) -> Option<ToolResult> {
    if delivery.is_foreground()
        || crate::wayland::wayland_input_enabled()
        || crate::wayland::is_inject_mode()
    {
        return None;
    }
    let chord = wm_chord_kind(key, modifiers)?;
    let display = format!("{}+{}", modifiers.join("+"), key);
    match chord {
        WmChord::CloseWindow => Some(close_window_background(pid, xid, &display).await),
        WmChord::Unavailable => {
            let hint = format!(
                "{display} is a window-manager binding (mutter's passive grab on the core \
                 keyboard); a virtual-keyboard chord in background mode reaches the window, \
                 not the WM, so it would do nothing. Use the driver instead: launch_app / \
                 list_windows / bring_to_front to switch windows, hotkey alt+F4 (or \
                 kill_app) to close one, or retry with delivery_mode:\"foreground\"."
            );
            Some(
                ToolResult::error(format!("Refused {display} on pid {pid}: {hint}"))
                    .with_structured(json!({
                        "code": "wm_chord_unavailable",
                        "effect": "refused",
                        "verified": false,
                        "delivery_mode": "background",
                        "chord": display,
                        "hint": hint,
                    })),
            )
        }
    }
}

/// Alt+F4 in background mode: ask the WM to close the window
/// (`_NET_CLOSE_WINDOW` -> `WM_DELETE_WINDOW`) and watch it go.
async fn close_window_background(pid: u32, xid: u64, display: &str) -> ToolResult {
    let before: Vec<crate::x11::WindowInfo> =
        tokio::task::spawn_blocking(move || crate::x11::list_windows(Some(pid)))
            .await
            .unwrap_or_default();
    let title = before
        .iter()
        .find(|w| w.xid == xid)
        .map(|w| w.title.clone())
        .unwrap_or_default();
    let sent = tokio::task::spawn_blocking(move || crate::x11::close_window(xid, pid)).await;
    match sent {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            return ToolResult::error(format!(
                "{display} on pid {pid}: could not request the window manager to close \
                 window {xid}: {error:#}"
            ))
            .with_structured(json!({
                "code": "close_window_failed",
                "effect": "none",
                "detail": error.to_string(),
            }))
        }
        Err(error) => return ToolResult::error(format!("Task error: {error}")),
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(2000);
    let mut closed = false;
    while std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let viewable =
            tokio::task::spawn_blocking(move || crate::x11::window_is_viewable(xid)).await;
        if matches!(viewable, Ok(false)) {
            closed = true;
            break;
        }
    }
    let after: Vec<crate::x11::WindowInfo> =
        tokio::task::spawn_blocking(move || crate::x11::list_windows(Some(pid)))
            .await
            .unwrap_or_default();
    let new_windows: Vec<&crate::x11::WindowInfo> = after
        .iter()
        .filter(|w| w.is_on_screen && !before.iter().any(|old| old.xid == w.xid))
        .collect();
    let quoted = if title.is_empty() {
        String::new()
    } else {
        format!(" \"{title}\"")
    };
    // The close request is an XSendEvent client message to the root (the
    // transport the action contract knows as x11_xsendevent).
    let mut structured = json!({
        "path": "x11_xsendevent",
        "mechanism": "_NET_CLOSE_WINDOW",
        "delivery_mode": "background",
        "chord": display,
        "target_window": xid,
        "window_closed": closed,
    });
    if closed {
        structured["verified"] = json!(true);
        structured["effect"] = json!("confirmed");
        structured["closed_window"] = json!(xid);
        structured["evidence"] = json!([{
            "kind": "window_change",
            "detail": format!("window {xid}{quoted} of pid {pid} closed after _NET_CLOSE_WINDOW"),
        }]);
        let mut text = format!(
            "Pressed {display} on pid {pid} as a window-manager close request \
             (_NET_CLOSE_WINDOW -> WM_DELETE_WINDOW; a virtual-keyboard Alt+F4 never reaches \
             mutter): window {xid}{quoted} closed."
        );
        if let Some(w) = new_windows.first() {
            text.push_str(&format!(
                " The application then opened window {} \"{}\".",
                w.xid, w.title
            ));
            structured["opened_window"] = json!({ "window_id": w.xid, "title": w.title });
        }
        ToolResult::text(text).with_structured(structured)
    } else {
        structured["verified"] = json!(false);
        structured["effect"] = json!("unverifiable");
        let mut text = format!(
            "Pressed {display} on pid {pid} as a window-manager close request \
             (_NET_CLOSE_WINDOW -> WM_DELETE_WINDOW): window {xid}{quoted} is still mapped \
             after 2 s."
        );
        if let Some(w) = new_windows.first() {
            text.push_str(&format!(
                " The application opened window {} \"{}\" instead (a save / confirm \
                 dialog?); act on it with get_window_state / click.",
                w.xid, w.title
            ));
            structured["opened_window"] = json!({ "window_id": w.xid, "title": w.title });
            structured["evidence"] = json!([{
                "kind": "window_change",
                "detail": format!("the application opened window {} \"{}\" after the close request", w.xid, w.title),
            }]);
        } else {
            text.push_str(" Confirm with list_windows; kill_app force-closes the application.");
        }
        ToolResult::text(text).with_structured(structured)
    }
}

/// `press_key` accepts `"alt+F4"` / `"ctrl+shift+t"` style keys: everything
/// before the last `+` is a modifier. A bare `"+"` stays the plus key.
pub(crate) fn split_key_combo(key: &str) -> (Vec<String>, String) {
    if key.len() <= 1 || !key.contains('+') {
        return (Vec::new(), key.to_owned());
    }
    let mut parts: Vec<&str> = key.split('+').collect();
    let last = parts.pop().unwrap_or("");
    let last = if last.is_empty() { "+" } else { last };
    let modifiers: Vec<String> = parts
        .into_iter()
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect();
    if !modifiers.is_empty() && modifiers.iter().all(|m| is_modifier(m)) {
        (modifiers, last.to_owned())
    } else {
        (Vec::new(), key.to_owned())
    }
}

pub(crate) fn isolated_hyprland_refusal(detail: impl Into<String>) -> ToolResult {
    let detail = detail.into();
    ToolResult::error(format!("background_unavailable: {detail}")).with_structured(json!({
        "ok": false, "code": "background_unavailable", "reason": "unsupported_operation",
        "detail": detail, "route": "synthetic_events", "verified": false, "effect": "refused"
    }))
}

pub(crate) fn isolated_hyprland_result(result: anyhow::Result<Value>) -> ToolResult {
    hyprland_input_result(result, false)
}

pub(crate) fn hyprland_input_result(result: anyhow::Result<Value>, foreground: bool) -> ToolResult {
    use cua_driver_core::action_record::{
        ActionEffect, ActionExecutionRecord, ActionTransport, ActualDelivery, RequestedDelivery,
    };
    let record = |effect| {
        ActionExecutionRecord::builder(
            effect,
            if foreground {
                ActionTransport::LinuxHyprlandForegroundInput
            } else {
                ActionTransport::LinuxHyprlandIsolatedInput
            },
            if foreground {
                RequestedDelivery::Foreground
            } else {
                RequestedDelivery::Background
            },
        )
    };
    let actual = if foreground {
        ActualDelivery::Foreground
    } else {
        ActualDelivery::Background
    };
    let unavailable = if foreground {
        "foreground_unavailable"
    } else {
        "background_unavailable"
    };
    match result {
        Ok(value) if value["ok"] == true => ToolResult::text(
            "Dispatched exact-target Hyprland input; application effect is unverifiable.",
        )
        .with_action_record(
            record(ActionEffect::Unverifiable)
                .actual_delivery(actual)
                .build()
                .expect("isolated input record is valid"),
        ),
        Ok(mut value) => {
            if foreground && value["code"] == "foreground_partial_unknown" {
                return hyprland_input_result(
                    Err(crate::wayland::hyprland_input::unknown_dispatch(
                        anyhow::anyhow!(
                            "{}",
                            value["detail"]
                                .as_str()
                                .unwrap_or("foreground acquisition may have changed focus")
                        ),
                        value["delivery"]["delivered_count"]
                            .as_u64()
                            .and_then(|count| u32::try_from(count).ok())
                            .unwrap_or(0),
                    )),
                    true,
                );
            }
            let code = value["code"]
                .as_str()
                .unwrap_or("protocol_error")
                .to_owned();
            let detail = value["detail"]
                .as_str()
                .unwrap_or("input refused")
                .to_owned();
            let refusal = if code == "stale_geometry" {
                "stale_observation"
            } else {
                unavailable
            };
            value["reason"] = json!(code);
            value["code"] = json!(refusal);
            value["route"] = json!(if foreground {
                "global_input"
            } else {
                "synthetic_events"
            });
            value["verified"] = json!(false);
            let delivered = value["delivery"]["delivered_count"]
                .as_u64()
                .and_then(|count| u32::try_from(count).ok())
                .filter(|count| *count > 0);
            let outcome = if value["effect"] == "partial"
                && delivered.is_some_and(|count| foreground || count == 1)
            {
                record(ActionEffect::Partial)
                    .actual_delivery(actual)
                    .delivered_count(delivered.unwrap())
            } else {
                value["effect"] = json!("refused");
                value
                    .as_object_mut()
                    .expect("protocol reply is an object")
                    .remove("delivery");
                record(ActionEffect::Refused)
            }
            .detail(&detail)
            .build()
            .expect("isolated input outcome is valid");
            ToolResult::error(format!("{refusal} ({code}): {detail}"))
                .with_structured(value)
                .with_action_record(outcome)
        }
        Err(error) if error.is::<crate::wayland::hyprland_input::LaneBusy>() => {
            hyprland_input_result(
                Ok(json!({
                    "ok": false, "code": "lane_busy", "detail": error.to_string()
                })),
                foreground,
            )
        }
        Err(error) if error.is::<crate::wayland::hyprland_input::ActionCancelled>() => {
            hyprland_input_result(
                Ok(json!({
                    "ok": false, "code": "cancelled", "detail": error.to_string()
                })),
                foreground,
            )
        }
        Err(error) => {
            let count = error
                .downcast_ref::<crate::wayland::hyprland_input::DispatchUnknown>()
                .map(|error| error.acknowledged_phases)
                .filter(|count| *count > 0);
            let effect = if count.is_some() {
                ActionEffect::Partial
            } else {
                ActionEffect::Unverifiable
            };
            let mut outcome = record(effect)
                .actual_delivery(ActualDelivery::Unknown)
                .detail(error.to_string());
            if let Some(count) = count {
                outcome = outcome.delivered_count(count);
            }
            let outcome = outcome
                .build()
                .expect("unknown delivery preserves acknowledged progress");
            let mut value = serde_json::to_value(outcome.public_result().unwrap()).unwrap();
            value["ok"] = json!(false);
            value["code"] = json!(unavailable);
            value["reason"] = json!("transport_or_protocol_error");
            value["detail"] = json!(error.to_string());
            ToolResult::error(format!("{unavailable}: {error}"))
                .with_structured(value)
                .with_action_record(outcome)
        }
    }
}

pub(crate) fn isolated_hyprland_task_error(
    error: tokio::task::JoinError,
    acknowledged: bool,
) -> ToolResult {
    isolated_hyprland_result(Err(crate::wayland::hyprland_input::unknown_dispatch(
        error.into(),
        u32::from(acknowledged),
    )))
}

pub(crate) fn spawn_isolated_hyprland(
    args: &Value,
    work: impl FnOnce(crate::wayland::hyprland_input::ActionCancellation) -> anyhow::Result<Value>
        + Send
        + 'static,
) -> Result<
    (
        crate::wayland::hyprland_input::CancelOnDrop,
        tokio::task::JoinHandle<anyhow::Result<Value>>,
    ),
    ToolResult,
> {
    use cua_driver_core::session;
    let owner = named_session_cursor_key(args)
        .ok_or_else(|| isolated_hyprland_refusal("authenticated lifecycle required"))?;
    let transport_owner = args
        .get("_transport_session_id")
        .and_then(Value::as_str)
        .unwrap_or(&owner);
    let snapshot = session::session_snapshot(&owner, transport_owner, std::time::Duration::ZERO)
        .ok_or_else(|| isolated_hyprland_refusal("admitted lifecycle required"))?;
    // Retain admission before spawning: aborting the async caller must not
    // complete session teardown while its native worker still owns input.
    let lifecycle = session::begin_session_dispatch(
        &owner,
        snapshot.public_label.as_deref(),
        transport_owner,
        snapshot.implicit,
        snapshot.transport,
        snapshot.client_kind,
    )
    .map_err(isolated_hyprland_refusal)?;
    let (guard, cancellation) = crate::wayland::hyprland_input::ActionCancellation::invocation();
    let dispatch = tokio::task::spawn_blocking(move || {
        let _lifecycle = lifecycle;
        work(cancellation)
    });
    Ok((guard, dispatch))
}

pub(crate) async fn isolated_hyprland_action(
    args: &Value,
    pid: u32,
    xid: u64,
    action: crate::wayland::hyprland_input::Action,
) -> ToolResult {
    let owner = named_session_cursor_key(args);
    let (_cancellation, dispatch) = match spawn_isolated_hyprland(args, move |cancellation| {
        crate::wayland::hyprland_input::execute(owner, pid, xid, action, cancellation)
    }) {
        Ok(dispatch) => dispatch,
        Err(refusal) => return refusal,
    };
    match dispatch.await {
        Ok(result) => isolated_hyprland_result(result),
        Err(error) => isolated_hyprland_task_error(error, false),
    }
}

pub(crate) fn foreground_hyprland_refusal(detail: impl Into<String>) -> ToolResult {
    hyprland_input_result(
        Ok(json!({
            "ok": false, "code": "unsupported_operation", "detail": detail.into()
        })),
        true,
    )
}

pub(crate) async fn foreground_hyprland_action(
    args: &Value,
    pid: u32,
    xid: u64,
    action: crate::wayland::hyprland_input::Action,
) -> ToolResult {
    if !crate::wayland::hyprland_input::enabled() {
        return foreground_hyprland_refusal("production Hyprland input plugin is unavailable");
    }
    let owner = named_session_cursor_key(args);
    let (_cancellation, dispatch) = match spawn_isolated_hyprland(args, move |cancellation| {
        crate::wayland::hyprland_input::execute_foreground(owner, pid, xid, action, cancellation)
    }) {
        Ok(dispatch) => dispatch,
        Err(_) => return foreground_hyprland_refusal("authenticated admitted lifecycle required"),
    };
    hyprland_input_result(
        match dispatch.await {
            Ok(result) => result,
            Err(error) => Err(crate::wayland::hyprland_input::unknown_dispatch(
                error.into(),
                0,
            )),
        },
        true,
    )
}

pub(crate) fn foreground_hyprland_key(
    key: String,
    mut modifiers: Vec<String>,
) -> Result<crate::wayland::hyprland_input::Action, ToolResult> {
    let parts: Vec<&str> = key.split('+').collect();
    let key = if parts.len() > 1 && key != "+" {
        if parts[..parts.len() - 1]
            .iter()
            .any(|part| !is_modifier(part))
            || parts.last() == Some(&"")
        {
            return Err(foreground_hyprland_refusal("invalid modified key chord"));
        }
        modifiers.extend(
            parts[..parts.len() - 1]
                .iter()
                .map(|part| part.to_lowercase()),
        );
        parts.last().unwrap().to_string()
    } else {
        key
    };
    Ok(crate::wayland::hyprland_input::Action::Key { key, modifiers })
}

pub(crate) async fn focus_hyprland_foreground(
    state: &Arc<ToolState>,
    args: &Value,
    pid: u32,
    xid: u64,
    element: Option<crate::atspi::TokenElement>,
    pixel: Option<(f64, f64)>,
) -> Result<(), ToolResult> {
    if !crate::wayland::hyprland_input::enabled() {
        return Err(foreground_hyprland_refusal(
            "production Hyprland input plugin is unavailable",
        ));
    }
    if let Some((x, y)) = pixel {
        return focus_by_pixel(
            state,
            pid,
            Some(xid),
            (x, y),
            true,
            args,
            args.bool_or("from_zoom", false),
        )
        .await;
    }
    if let Some(element) = element {
        let activation = foreground_hyprland_action(
            args,
            pid,
            xid,
            crate::wayland::hyprland_input::Action::Activate,
        )
        .await;
        if activation.is_error == Some(true) {
            return Err(activation);
        }
        match tokio::task::spawn_blocking(move || crate::atspi::focus_token(pid, &element)).await {
            Ok(Ok(true)) => {}
            _ => return Err(foreground_hyprland_refusal("AT-SPI child focus failed")),
        }
    }
    Ok(())
}

#[cfg(test)]
#[tokio::test]
async fn isolated_worker_retains_lifecycle_until_native_work_exits() {
    use cua_driver_core::session::{self, SessionClientKind, SessionTransport};
    let sid = "isolated-worker-lifecycle-test";
    let owner = "isolated-worker-transport-test";
    let caller = session::begin_session_dispatch(
        sid,
        None,
        owner,
        true,
        SessionTransport::McpStdio,
        SessionClientKind::Mcp,
    )
    .unwrap();
    let (started, ready) = tokio::sync::oneshot::channel();
    let (release, released) = std::sync::mpsc::channel();
    let (cancellation, dispatch) = spawn_isolated_hyprland(
        &json!({"_session_id":sid,"_transport_session_id":owner}),
        move |_| {
            started.send(()).unwrap();
            released.recv().unwrap();
            Ok(json!({"ok":true}))
        },
    )
    .unwrap();
    ready.await.unwrap();
    drop(cancellation);
    drop(caller);
    assert!(session::end_session_for_owner(sid, owner));
    assert!(session::is_session_ending(sid));
    assert!(!session::is_session_ended(sid));
    release.send(()).unwrap();
    dispatch.await.unwrap().unwrap();
    assert!(session::is_session_ended(sid));
}

#[cfg(test)]
#[test]
fn isolated_background_routes_do_not_reprobe_availability_before_primary_fallback() {
    // Structural regression for the availability-loss race: each tool keeps
    // the decision that bypassed background refusal until its returning
    // native branch. A second probe here could select primary-seat input.
    for (source, start) in [
        (include_str!("click.rs"), "impl Tool for ClickTool {"),
        (include_str!("scroll.rs"), "impl Tool for ScrollTool {"),
        (include_str!("drag.rs"), "impl Tool for DragTool {"),
    ] {
        let body = source.rsplit_once(start).unwrap().1;
        let invoke = body.split_once("async fn invoke").unwrap().1;
        assert_eq!(
            invoke
                .matches("isolated_hyprland_background(delivery)")
                .count(),
            1,
            "{start}"
        );
        let native = invoke.split_once("if isolated_background {").unwrap().1;
        assert!(native.contains("return match dispatch.await"), "{start}");
    }
}

#[cfg(test)]
#[test]
fn background_keyboard_refusal_names_missing_atspi_bus() {
    let plain = crate::input::delivery::background_unavailable_error(
        crate::input::delivery::BackgroundUnavailable::FocusedInputOnly,
    );
    let text = |refusal: &ToolResult| -> String {
        refusal
            .content
            .iter()
            .filter_map(|content| match content {
                cua_driver_core::protocol::Content::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    };
    let (missing, reachable) = (
        background_keyboard_refusal(false),
        background_keyboard_refusal(true),
    );
    for needle in [
        "AT-SPI is unavailable",
        "at-spi-bus-launcher",
        "delivery_mode \"foreground\"",
    ] {
        assert!(
            text(&missing).contains(needle),
            "{needle} missing from {}",
            text(&missing)
        );
    }
    assert!(!text(&reachable).contains("AT-SPI is unavailable"));
    for refusal in [missing, reachable] {
        let structured = refusal.structured_content.unwrap();
        let expected = plain.structured_content.as_ref().unwrap();
        assert_eq!(structured["code"], expected["code"]);
        assert_eq!(structured["escalation"], expected["escalation"]);
    }
}

#[cfg(test)]
#[test]
fn persistent_wayland_pointer_coordinates_include_window_origin() {
    let output = persistent_pointer_coordinates(true, 7, 10, 20, |window_id, x, y| {
        assert_eq!(window_id, 7);
        Ok((x + 100, y + 200).into())
    });
    assert_eq!(output.unwrap().point, (110, 220));

    let x11 = persistent_pointer_coordinates(false, 7, 10, 20, |_, _, _| {
        panic!("X11 coordinates must remain window-local")
    });
    assert_eq!(x11.unwrap().point, (10, 20));
}

#[cfg(test)]
#[test]
fn session_end_releases_only_its_hold_before_discarding_state() {
    let state = ToolState::new();
    let session_id = "ending-session";
    let hold = MouseHoldState {
        pid: 42,
        xid: 7,
        button: 1,
        x: 10.0,
        y: 20.0,
    };
    let other_hold = MouseHoldState {
        pid: 43,
        xid: 8,
        button: 3,
        x: 30.0,
        y: 40.0,
    };
    {
        let mut holds = state.mouse_hold.lock().unwrap();
        holds.insert(session_id.to_owned(), hold.clone());
        holds.insert("other-session".to_owned(), other_hold.clone());
    }

    let mut released = Vec::new();
    release_mouse_hold_for_session(&state, session_id, |cursor_id, held| {
        assert!(state.mouse_hold.lock().unwrap().contains_key(cursor_id));
        released.push((cursor_id.to_owned(), held.button));
        Ok(())
    })
    .unwrap();

    assert_eq!(released, vec![(session_id.to_owned(), hold.button)]);
    let mut holds = state.mouse_hold.lock().unwrap();
    assert!(!holds.contains_key(session_id));
    assert_eq!(
        holds.get("other-session").unwrap().button,
        other_hold.button
    );
    assert!(holds.insert(session_id.to_owned(), hold).is_none());
}

#[cfg(test)]
#[test]
fn failed_session_end_release_remains_retryable_and_bounded() {
    let state = ToolState::new();
    let session_id = "failed-release-session";
    let hold = MouseHoldState {
        pid: 42,
        xid: 7,
        button: 1,
        x: 10.0,
        y: 20.0,
    };
    state
        .mouse_hold
        .lock()
        .unwrap()
        .insert(session_id.to_owned(), hold.clone());

    let mut attempts = 0;
    let failure = release_mouse_hold_for_session(&state, session_id, |_, _| {
        attempts += 1;
        anyhow::bail!("compositor disconnected")
    })
    .expect_err("a failed compositor release must fail session cleanup");
    assert!(failure.contains("compositor disconnected"));
    assert_eq!(
        attempts, 1,
        "one cleanup pass must make one bounded attempt"
    );
    assert_eq!(
        state
            .mouse_hold
            .lock()
            .unwrap()
            .get(session_id)
            .unwrap()
            .button,
        hold.button,
        "failed cleanup must retain the hold for the next session-end retry"
    );

    release_mouse_hold_for_session(&state, session_id, |cursor_id, held| {
        attempts += 1;
        assert_eq!(cursor_id, session_id);
        assert_eq!(held.button, hold.button);
        Ok(())
    })
    .unwrap();
    assert_eq!(attempts, 2);
    assert!(state
        .mouse_hold
        .lock()
        .unwrap()
        .insert(session_id.to_owned(), hold)
        .is_none());
}

#[cfg(test)]
#[test]
fn isolated_hyprland_refused_partial_and_unknown_outcomes_stay_distinct() {
    let busy = isolated_hyprland_result(Err(crate::wayland::hyprland_input::LaneBusy.into()));
    let content = busy.structured_content.as_ref().unwrap();
    assert_eq!(content["reason"], "lane_busy");
    assert_eq!(content["effect"], "refused");
    assert!(content.get("delivery").is_none());
    assert!(busy
        .action_record
        .unwrap()
        .public_result()
        .unwrap()
        .delivery
        .is_none());

    let refused = isolated_hyprland_result(Ok(
        json!({"ok":false,"code":"stale_target","detail":"stale_target"}),
    ));
    assert_eq!(
        refused.structured_content.as_ref().unwrap()["effect"],
        "refused"
    );
    let public = refused.action_record.unwrap().public_result().unwrap();
    assert!(public.delivery.is_none());
    public.validate_invariants().unwrap();

    for foreground in [false, true] {
        let stale = hyprland_input_result(
            Ok(json!({"ok":false,"code":"stale_geometry","detail":"stale_geometry"})),
            foreground,
        );
        let value = stale.structured_content.as_ref().unwrap();
        assert_eq!(value["code"], "stale_observation");
        assert_eq!(value["reason"], "stale_geometry");
        assert_eq!(value["effect"], "refused");
        assert!(stale
            .action_record
            .unwrap()
            .public_result()
            .unwrap()
            .delivery
            .is_none());
    }

    let partial = isolated_hyprland_result(Ok(
        json!({"ok":false,"code":"cancelled","detail":"cancelled",
        "effect":"partial","delivery":{"mode":"background","delivered_count":1}}),
    ));
    let public = partial.action_record.unwrap().public_result().unwrap();
    assert_eq!(public.effect, cua_driver_contract::ActionEffect::Partial);
    assert_eq!(public.delivery.unwrap().delivered_count, Some(1));

    let unknown = isolated_hyprland_result(Err(anyhow::anyhow!("connection lost")));
    let public = unknown.action_record.unwrap().public_result().unwrap();
    assert_eq!(
        public.effect,
        cua_driver_contract::ActionEffect::Unverifiable
    );
    assert_eq!(
        public.delivery.as_ref().unwrap().mode,
        cua_driver_contract::ActionDeliveryMode::Unknown
    );
    assert_eq!(public.delivery.unwrap().delivered_count, None);

    let unknown_after_start = isolated_hyprland_result(Err(
        crate::wayland::hyprland_input::unknown_dispatch(anyhow::anyhow!("connection lost"), 1),
    ));
    let public = unknown_after_start
        .action_record
        .unwrap()
        .public_result()
        .unwrap();
    public.validate_invariants().unwrap();
    assert_eq!(public.effect, cua_driver_contract::ActionEffect::Partial);
    let delivery = public.delivery.unwrap();
    assert_eq!(
        delivery.mode,
        cua_driver_contract::ActionDeliveryMode::Unknown
    );
    assert_eq!(delivery.delivered_count, Some(1));
}

#[cfg(test)]
#[test]
fn experimental_pending_grant_is_an_error_with_operator_context() {
    let result = isolated_hyprland_result(Ok(json!({
        "ok": false, "code": "permission_required", "detail": "external approval pending",
        "epoch": "epoch", "challenge": "challenge", "target": "target", "revision": 3
    })));
    assert_eq!(result.is_error, Some(true));
    let value = result.structured_content.unwrap();
    assert_eq!(value["code"], "background_unavailable");
    assert_eq!(value["reason"], "permission_required");
    assert_eq!(value["challenge"], "challenge");
    assert_eq!(value["target"], "target");
    assert_eq!(value["revision"], 3);
    assert_eq!(value["verified"], false);
}

#[cfg(test)]
#[test]
fn experimental_dispatch_does_not_claim_application_success() {
    let result = isolated_hyprland_result(Ok(json!({
        "ok": true, "effect": "unverifiable", "route": "synthetic_events"
    })));
    assert_ne!(result.is_error, Some(true));
    let public = result.action_record.unwrap().public_result().unwrap();
    public.validate_invariants().unwrap();
    let value = serde_json::to_value(public).unwrap();
    assert_eq!(value["effect"], "unverifiable");
    assert_eq!(value["route"], "synthetic_events");
    assert!(value.get("verified").is_none());
}

#[cfg(test)]
#[test]
fn foreground_hyprland_reports_foreground_and_preserves_partial_progress() {
    use cua_driver_core::action_record::{ActionTransport, RequestedDelivery};
    for (reply, expected_effect, count) in [
        (json!({"ok":true}), "unverifiable", None),
        (
            json!({"ok":false,"code":"stale_target","detail":"stale"}),
            "refused",
            None,
        ),
        (
            json!({"ok":false,"code":"cancelled","detail":"cancelled","effect":"partial",
            "delivery":{"mode":"foreground","delivered_count":3}}),
            "partial",
            Some(3),
        ),
    ] {
        let result = hyprland_input_result(Ok(reply), true);
        let record = result.action_record.unwrap();
        assert_eq!(
            record.transport,
            ActionTransport::LinuxHyprlandForegroundInput
        );
        assert_eq!(record.requested_delivery, RequestedDelivery::Foreground);
        let public = record.public_result().unwrap();
        public.validate_invariants().unwrap();
        let value = serde_json::to_value(public).unwrap();
        assert_eq!(value["route"], "global_input");
        assert_eq!(value["effect"], expected_effect);
        if expected_effect == "refused" {
            assert!(value.get("delivery").is_none());
        } else {
            assert_eq!(value["delivery"]["mode"], "foreground");
            assert_eq!(value["delivery"]["delivered_count"].as_u64(), count);
        }
    }
    for reply in [
        Err(crate::wayland::hyprland_input::unknown_dispatch(
            anyhow::anyhow!("lost"),
            2,
        )),
        Ok(json!({"ok":false,"code":"foreground_partial_unknown","detail":"focus changed"})),
    ] {
        let unknown = hyprland_input_result(reply, true);
        let value =
            serde_json::to_value(unknown.action_record.unwrap().public_result().unwrap()).unwrap();
        assert_eq!(value["delivery"]["mode"], "unknown");
        assert_ne!(value["effect"], "refused");
    }
}

#[cfg(test)]
#[test]
fn foreground_hyprland_chords_preserve_all_modifiers() {
    let action = foreground_hyprland_key("Ctrl+Shift+H".into(), vec!["alt".into()]).unwrap();
    match action {
        crate::wayland::hyprland_input::Action::Key { key, modifiers } => {
            assert_eq!(key, "H");
            assert_eq!(modifiers, ["alt", "ctrl", "shift"]);
        }
        _ => panic!("expected a key chord"),
    }
    assert!(foreground_hyprland_key("bogus+H".into(), vec![]).is_err());
}

#[cfg(test)]
#[test]
fn uinput_failure_has_a_stable_structured_code() {
    let result = linux_input_error(crate::input::uinput_unavailable("synthetic failure"));
    assert_eq!(result.is_error, Some(true));
    assert_eq!(
        result
            .structured_content
            .as_ref()
            .and_then(|value| value.get("code"))
            .and_then(Value::as_str),
        Some(crate::input::UINPUT_UNAVAILABLE_CODE)
    );
}

pub(crate) fn mouse_button_name(button: u8) -> &'static str {
    match button {
        3 => "right",
        2 => "middle",
        _ => "left",
    }
}

pub(crate) fn resolve_cursor_key(args: &Value) -> String {
    cua_driver_core::tool_args::resolve_session_key(args)
}

/// Return the cursor key only for a lifecycle-owned session. Cursor positioning
/// for keyboard actions deliberately does not opt anonymous calls or the
/// legacy cursor_id-only path into session semantics. The proxy-minted
/// `_session_id` is trusted lifecycle state and must behave like a public named
/// session for cursor ownership.
pub(crate) fn named_session_cursor_key(args: &Value) -> Option<String> {
    cursor_overlay::named_session_cursor_key(args)
}

pub(crate) fn mouse_hold_json(cursor_id: &str, hold: Option<&MouseHoldState>) -> Value {
    match hold {
        Some(hold) => json!({
            "cursor_id": cursor_id,
            "held": true,
            "pid": hold.pid,
            "window_id": hold.xid,
            "button": mouse_button_name(hold.button),
            "x": hold.x,
            "y": hold.y,
        }),
        None => json!({
            "cursor_id": cursor_id,
            "held": false,
            "pid": Value::Null,
            "window_id": Value::Null,
            "button": Value::Null,
            "x": Value::Null,
            "y": Value::Null,
        }),
    }
}

pub(crate) fn held_target_mismatch(
    args: &Value,
    cursor_id: &str,
    hold: &MouseHoldState,
) -> Option<ToolResult> {
    match args.opt_u32("pid") {
        Ok(Some(pid)) if pid != hold.pid => {
            return Some(
                ToolResult::error(format!(
                    "Cursor '{cursor_id}' is holding a button for pid {}, not pid {pid}.",
                    hold.pid
                ))
                .with_structured(mouse_hold_json(cursor_id, Some(hold))),
            );
        }
        Err(err) => return Some(err.with_structured(mouse_hold_json(cursor_id, Some(hold)))),
        _ => {}
    }

    match args.opt_u64("window_id") {
        Some(xid) if xid != hold.xid => Some(
            ToolResult::error(format!(
                "Cursor '{cursor_id}' is holding a button for window_id {}, not {xid}.",
                hold.xid
            ))
            .with_structured(mouse_hold_json(cursor_id, Some(hold))),
        ),
        _ => None,
    }
}

pub(crate) fn clear_mouse_hold_after_release(state: &ToolState, cursor_id: &str) {
    state.mouse_hold.lock().unwrap().remove(cursor_id);
}

pub(crate) fn persistent_pointer_coordinates(
    is_wayland: bool,
    window_id: u64,
    x: i32,
    y: i32,
    window_local_to_output: impl FnOnce(u64, i32, i32) -> anyhow::Result<AimedPoint>,
) -> anyhow::Result<AimedPoint> {
    if is_wayland {
        window_local_to_output(window_id, x, y)
    } else {
        Ok((x, y).into())
    }
}

fn release_mouse_hold_for_session<Release>(
    state: &ToolState,
    session_id: &str,
    mut release: Release,
) -> Result<(), String>
where
    Release: FnMut(&str, &MouseHoldState) -> anyhow::Result<()>,
{
    let hold = state.mouse_hold.lock().unwrap().get(session_id).cloned();
    let Some(hold) = hold else {
        return Ok(());
    };

    release(session_id, &hold).map_err(|error| error.to_string())?;
    state.mouse_hold.lock().unwrap().remove(session_id);
    Ok(())
}

pub(crate) fn overlay_snap_to_for(cursor_id: &str, sx: f64, sy: f64, heading: Option<f64>) {
    crate::overlay::send_command_for(
        cursor_id.to_owned(),
        cursor_overlay::OverlayCommand::SnapTo {
            x: sx,
            y: sy,
            heading_radians: heading,
        },
    );
}

/// [`overlay_move_to_for`] with the targeted element's screen rect
/// `[x, y, width, height]` (same space as `sx`/`sy`).
pub(crate) fn overlay_move_to_target_for(
    cursor_id: &str,
    sx: f64,
    sy: f64,
    heading: Option<f64>,
    target: Option<[f64; 4]>,
) {
    crate::overlay::send_command_for(
        cursor_id.to_owned(),
        cursor_overlay::OverlayCommand::MoveTo {
            x: sx,
            y: sy,
            end_heading_radians: heading.unwrap_or(std::f64::consts::FRAC_PI_4),
            target,
        },
    );
}

pub(crate) async fn overlay_glide_to_for(cursor_id: &str, sx: f64, sy: f64) {
    overlay_glide_to_target_for(cursor_id, sx, sy, None).await;
}

/// [`overlay_glide_to_for`] with the targeted element's screen rect
/// `[x, y, width, height]` (same space as `sx`/`sy`); `None` for pixel actions.
async fn overlay_glide_to_target_for(cursor_id: &str, sx: f64, sy: f64, target: Option<[f64; 4]>) {
    // Input always revives its agent cursor. Hiding it is useful while idle,
    // but a hidden cursor must not make pointer or keyboard control invisible.
    crate::overlay::send_command_for(
        cursor_id.to_owned(),
        cursor_overlay::OverlayCommand::SetEnabled(true),
    );
    // Every Wayland backend owns its animation loop. Send one destination and
    // let the Linux overlay layer select the semantic helper, layer-shell, or
    // older helper fallback without an interpolated stream fighting it.
    if crate::wayland::is_wayland() {
        overlay_move_to_target_for(cursor_id, sx, sy, None, target);
        return;
    }
    let pos = crate::overlay::current_position_for(cursor_id);
    if pos.is_none() {
        crate::overlay::send_command_for(
            cursor_id.to_owned(),
            cursor_overlay::OverlayCommand::ClickPulse { x: sx, y: sy },
        );
        return;
    }
    crate::overlay::animate_cursor_to_target_for(cursor_id.to_owned(), sx, sy, target).await;
}

/// The compositor snapshot a desktop-scope action is addressed in. One
/// snapshot converts the action's points, places the overlay and sizes the
/// virtual pointer, so they cannot disagree about the monitor layout.
pub(crate) async fn desktop_input_space() -> anyhow::Result<crate::wayland::DesktopInputSpace> {
    tokio::task::spawn_blocking(|| {
        crate::interaction::admit_lock(
            crate::interaction::policy::Scope::DesktopInput,
            crate::interaction::facts::Fact::Unknown("desktop input addresses no window".into()),
        )?;
        if !crate::wayland::wayland_input_enabled() {
            return Ok(crate::wayland::DesktopInputSpace::default());
        }
        crate::wayland::DesktopInputSpace::current()
    })
    .await
    .map_err(|error| anyhow::anyhow!("task error: {error}"))?
}

/// Keep the logical cursor position in sync with every visibly targeted
/// pointer action. Overlay delivery is intentionally best-effort: registry
/// state is still updated when no renderer is running or its queue is closed.
pub(crate) async fn reveal_pointer_action_for(
    state: &ToolState,
    cursor_id: &str,
    sx: f64,
    sy: f64,
    click_pulse: bool,
) {
    reveal_pointer_action_target_for(state, cursor_id, sx, sy, click_pulse, None).await;
}

/// [`reveal_pointer_action_for`] with the targeted element's screen rect
/// `[x, y, width, height]` (same space as `sx`/`sy`).
pub(crate) async fn reveal_pointer_action_target_for(
    state: &ToolState,
    cursor_id: &str,
    sx: f64,
    sy: f64,
    click_pulse: bool,
    target: Option<[f64; 4]>,
) {
    if !sx.is_finite() || !sy.is_finite() {
        return;
    }
    state.cursor_registry.set_enabled(cursor_id, true);
    state.cursor_registry.update_position(cursor_id, sx, sy);
    emit_cursor_hook(cursor_id, sx, sy, false);
    overlay_glide_to_target_for(cursor_id, sx, sy, target).await;
    if click_pulse {
        emit_cursor_hook(cursor_id, sx, sy, true);
        crate::overlay::send_command_for(
            cursor_id.to_owned(),
            cursor_overlay::OverlayCommand::ClickPulse { x: sx, y: sy },
        );
    }
}

/// Report a commanded cursor move (or a press) to the embedder's cursor
/// hook, with the same guards as macOS's `CursorRegistry::emit_cursor_event`:
/// nothing is built unless a hook is registered, and an ended session never
/// reaches it. Emitted from the logical cursor write path, so it works with
/// no overlay renderer (a headless Xvfb session).
pub(crate) fn emit_cursor_hook(cursor_id: &str, x: f64, y: f64, pressed: bool) {
    if !cua_driver_core::cursor_hook::cursor_hook_enabled() {
        return;
    }
    if cursor_id.is_empty() || cua_driver_core::session::is_session_ended(cursor_id) {
        return;
    }
    cua_driver_core::cursor_hook::push_cursor_event(
        cua_driver_core::cursor_hook::CursorHookEvent {
            cursor_id: cursor_id.to_owned(),
            x,
            y,
            pressed,
        },
    );
}

pub(crate) fn keyboard_window_center(xid: u64) -> Option<(f64, f64)> {
    if xid == 0 {
        return None;
    }
    if crate::wayland::is_wayland() {
        return crate::wayland::window_geometry(xid).and_then(|(x, y, width, height)| {
            (width > 0 && height > 0).then_some((
                f64::from(x) + f64::from(width) / 2.0,
                f64::from(y) + f64::from(height) / 2.0,
            ))
        });
    }
    window_screen_center(xid)
        .ok()
        .map(|(x, y)| (f64::from(x), f64::from(y)))
}

fn current_pointer_position() -> Option<(f64, f64)> {
    if crate::wayland::is_wayland() {
        return crate::wayland::last_synth_cursor_pos().map(|(x, y)| (f64::from(x), f64::from(y)));
    }

    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::ConnectionExt as _;
    use x11rb::rust_connection::RustConnection;

    let (connection, screen_num) = RustConnection::connect(None).ok()?;
    let root = connection.setup().roots[screen_num].root;
    let reply = connection.query_pointer(root).ok()?.reply().ok()?;
    Some((f64::from(reply.root_x), f64::from(reply.root_y)))
}

pub(crate) fn explicit_keyboard_cursor_target(
    pid: u32,
    xid: u64,
    element: Option<&crate::atspi::TokenElement>,
    pixel_target: Option<(f64, f64)>,
) -> Option<(f64, f64)> {
    if let Some(element) = element {
        let (sx, sy) = element_screen_center(pid, element, (xid != 0).then_some(xid)).ok()?;
        return Some((sx, sy));
    }

    let (x, y) = pixel_target?;
    if crate::wayland::wayland_input_enabled() {
        return crate::wayland::window_geometry(xid)
            .map(|(wx, wy, _, _)| (f64::from(wx) + x.round(), f64::from(wy) + y.round()));
    }
    window_local_to_screen(xid, x, y).ok()
}

/// Position and reveal a named session's cursor before keyboard/value input.
/// `preserve_legacy_element_visual` retains the existing element-only feedback
/// for type_text/set_value anonymous calls without adding session-style fallback
/// placement to them. Geometry and overlay failures are observational and never
/// affect the tool's actual input result.
/// Tell the cursor hook where an anonymous keyboard action types, before the
/// keystrokes: the element or pixel target, else the window's center. The
/// overlay is left alone (anonymous calls keep their legacy visuals); only an
/// embedder publishing presence learns the position, so a viewer sees the
/// agent's cursor go to the field it is about to type into.
pub(crate) async fn announce_keyboard_target(
    args: &Value,
    pid: u32,
    xid: u64,
    element: Option<crate::atspi::TokenElement>,
    pixel_target: Option<(f64, f64)>,
) {
    if !cua_driver_core::cursor_hook::cursor_hook_enabled() {
        return;
    }
    let cursor_id = resolve_cursor_key(args);
    let target = tokio::task::spawn_blocking(move || {
        explicit_keyboard_cursor_target(pid, xid, element.as_ref(), pixel_target)
            .or_else(|| (xid != 0).then(|| keyboard_window_center(xid)).flatten())
    })
    .await
    .ok()
    .flatten();
    if let Some((x, y)) = target {
        emit_cursor_hook(&cursor_id, x, y, false);
    }
}

pub(crate) async fn position_named_session_keyboard_cursor(
    state: &ToolState,
    args: &Value,
    pid: u32,
    xid: u64,
    element: Option<crate::atspi::TokenElement>,
    pixel_target: Option<(f64, f64)>,
    preserve_legacy_element_visual: bool,
) {
    let named_cursor_id = named_session_cursor_key(args);
    let cursor_id = match named_cursor_id {
        Some(ref cursor_id) => cursor_id.clone(),
        None if preserve_legacy_element_visual && element.is_some() => resolve_cursor_key(args),
        None => {
            announce_keyboard_target(args, pid, xid, element, pixel_target).await;
            return;
        }
    };

    let remembered = named_cursor_id.as_ref().and_then(|_| {
        state
            .cursor_registry
            .get(&cursor_id)
            .and_then(|cursor| cursor.x.zip(cursor.y))
    });
    let explicit = tokio::task::spawn_blocking(move || {
        explicit_keyboard_cursor_target(pid, xid, element.as_ref(), pixel_target)
    })
    .await
    .ok()
    .flatten();

    let fallback = if named_cursor_id.is_some()
        && explicit.is_none()
        && cursor_overlay::keyboard_cursor_target(None, remembered, None, None).is_none()
    {
        tokio::task::spawn_blocking(move || {
            let center = keyboard_window_center(xid);
            let pointer = center.is_none().then(current_pointer_position).flatten();
            (center, pointer)
        })
        .await
        .unwrap_or((None, None))
    } else {
        (None, None)
    };

    let Some((sx, sy)) =
        cursor_overlay::keyboard_cursor_target(explicit, remembered, fallback.0, fallback.1)
    else {
        return;
    };
    if xid != 0 {
        crate::overlay::send_command_for(
            cursor_id.clone(),
            cursor_overlay::OverlayCommand::PinAbove(xid),
        );
    }
    reveal_pointer_action_for(state, &cursor_id, sx, sy, false).await;
}

fn process_name(pid: u32) -> Option<String> {
    let cmdline = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let first = String::from_utf8_lossy(&cmdline)
        .split('\0')
        .next()
        .unwrap_or("")
        .trim()
        .to_owned();
    if !first.is_empty() {
        return std::path::Path::new(&first)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .or(Some(first));
    }

    let status = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    status
        .lines()
        .find(|l| l.starts_with("Name:"))
        .map(|l| l[5..].trim().to_owned())
}

pub(crate) fn is_terminal_process(pid: u32) -> bool {
    // Canonical list lives in `crate::terminal::TERMINAL_PROCESS_NAMES`
    // — keeps the additive contract centralised. Adding a new terminal
    // here means appending one string in `crate::terminal`.
    match process_name(pid).as_deref() {
        Some(name) => crate::terminal::is_terminal_process_name(name),
        None => false,
    }
}

fn terminal_descendant_ttys(pid: u32) -> Vec<PathBuf> {
    let mut parent_to_children: std::collections::HashMap<u32, Vec<u32>> =
        std::collections::HashMap::new();
    let proc_dir = std::path::Path::new("/proc");
    let entries = match fs::read_dir(proc_dir) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };

    for entry in entries.flatten() {
        let pid_str = entry.file_name();
        let pid_str = pid_str.to_string_lossy();
        let child_pid: u32 = match pid_str.parse() {
            Ok(pid) => pid,
            Err(_) => continue,
        };
        let status = match fs::read_to_string(proc_dir.join(&*pid_str).join("status")) {
            Ok(status) => status,
            Err(_) => continue,
        };
        let parent_pid = status
            .lines()
            .find(|l| l.starts_with("PPid:"))
            .and_then(|l| l[5..].trim().parse::<u32>().ok());
        if let Some(parent_pid) = parent_pid {
            parent_to_children
                .entry(parent_pid)
                .or_default()
                .push(child_pid);
        }
    }

    let mut descendants = Vec::new();
    let mut queue = std::collections::VecDeque::from([pid]);
    while let Some(current) = queue.pop_front() {
        if let Some(children) = parent_to_children.get(&current) {
            for &child in children {
                descendants.push(child);
                queue.push_back(child);
            }
        }
    }
    descendants.sort_unstable();

    let mut ttys = Vec::new();
    for child in descendants {
        let tty = match fs::read_link(format!("/proc/{child}/fd/0")) {
            Ok(path) => path,
            Err(_) => continue,
        };
        if tty.starts_with("/dev/pts/") {
            ttys.push(tty);
        }
    }
    ttys
}

fn terminal_tty_for_window(pid: u32, xid: u64) -> Option<PathBuf> {
    if !is_terminal_process(pid) {
        return None;
    }
    let mut windows = crate::x11::list_windows(Some(pid));
    windows.sort_by_key(|w| w.xid);
    let window_index = windows.iter().position(|w| w.xid == xid)?;
    let ttys = terminal_descendant_ttys(pid);
    ttys.get(window_index).cloned()
}

/// Type into a terminal window without touching X focus. Resolves the window's
/// pty, then borrows the emulator's master fd and writes to it (see
/// `crate::tty`). Returns `Ok(false)` when the target isn't a terminal we can
/// reach this way so the caller falls back to the generic XSendEvent path.
pub(crate) fn inject_terminal_input(pid: u32, xid: u64, text: &str) -> anyhow::Result<bool> {
    let Some(tty) = terminal_tty_for_window(pid, xid) else {
        return Ok(false);
    };
    // tty is `/dev/pts/<N>`; the emulator (pid) holds the master for the same N.
    let Some(ptn) = tty
        .file_name()
        .and_then(|s| s.to_str())
        .and_then(|s| s.parse::<u32>().ok())
    else {
        return Ok(false);
    };
    crate::tty::inject_via_master(pid, ptn, text)
}

pub(crate) fn capture_admission_error(error: anyhow::Error) -> ToolResult {
    let code = cua_driver_core::capture_runtime::admission_error_code(&error);
    ToolResult::error(format!("capture-bound click refused: {error}"))
        .with_structured(json!({ "code": code, "effect": "refused" }))
}

// ── px-focus helper (keyboard family) ───────────────────────────────────────

/// px-focus for the keyboard family (type_text / press_key / hotkey): pixel-click
/// at (x,y) to establish real renderer focus before a keystroke — the *element px
/// action* form of a keyboard tool. Reuses ClickTool's exact coordinate
/// translation + delivery_mode so it lands on the same pixel a px-click would.
/// `Ok(())` on success; `Err(ToolResult)` short-circuits the caller.
///
/// Retains the parent's authenticated session admission for the nested click.
/// `point` is the window-local pixel to click for focus.
pub(crate) async fn focus_by_pixel(
    state: &Arc<ToolState>,
    pid: u32,
    window_id: Option<u64>,
    (x, y): (f64, f64),
    foreground: bool,
    parent_args: &Value,
    from_zoom: bool,
) -> Result<(), ToolResult> {
    let mut click_args = json!({
        "pid": pid, "x": x, "y": y,
        "delivery_mode": if foreground { "foreground" } else { "background" },
    });
    if let Some(wid) = window_id {
        click_args["window_id"] = json!(wid);
    }
    for field in [
        "session",
        "_session_id",
        "_transport_session_id",
        "cursor_id",
        "scope",
        "coordinate_frame",
    ] {
        if let Some(value) = parent_args.get(field) {
            click_args[field] = value.clone();
        }
    }
    if from_zoom {
        click_args["from_zoom"] = json!(true);
    }
    let focus = ClickTool {
        state: state.clone(),
    }
    .invoke(click_args)
    .await;
    if focus.is_error == Some(true) {
        return Err(focus);
    }
    // Brief settle so the renderer registers focus before the keystrokes.
    tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    Ok(())
}

/// Establish widget-local focus inside a nested-compositor target without
/// changing the compositor's focused toplevel. AX uses Component.GrabFocus;
/// PX sends a private per-surface left click at the requested local point.
pub(crate) async fn focus_nested_inject_target(
    pid: u32,
    window_id: u64,
    element: Option<crate::atspi::TokenElement>,
    pixel: Option<(f64, f64)>,
) -> Result<(), ToolResult> {
    if let Some(element) = element {
        let index = element.index;
        return match tokio::task::spawn_blocking(move || crate::atspi::focus_token(pid, &element))
            .await
        {
            Ok(Ok(true)) => Ok(()),
            Ok(Ok(false)) => Err(ToolResult::error(format!(
                "AT-SPI Component.GrabFocus returned false for element {index}"
            ))),
            Ok(Err(error)) => Err(stale_observed_element("focus", index, &error)
                .unwrap_or_else(|| ToolResult::error(error.to_string()))),
            Err(error) => Err(ToolResult::error(format!("Task error: {error}"))),
        };
    }
    if let Some((x, y)) = pixel {
        return match tokio::task::spawn_blocking(move || {
            crate::wayland::inject_click(pid, window_id, x, y, 1, 1)
        })
        .await
        {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(ToolResult::error(error.to_string())),
            Err(error) => Err(ToolResult::error(format!("Task error: {error}"))),
        };
    }
    Ok(())
}

pub(crate) fn is_modifier(k: &str) -> bool {
    matches!(
        k.to_lowercase().as_str(),
        "ctrl"
            | "control"
            | "shift"
            | "alt"
            | "super"
            | "meta"
            | "cmd"
            | "command"
            | "win"
            | "windows"
    )
}

/// Numeric-aware equality for the `set_value` read-back ("40" == "40.0").
pub(crate) fn values_agree(wanted: &str, observed: &str) -> bool {
    let wanted = wanted.trim();
    let observed = observed.trim();
    if wanted == observed {
        return true;
    }
    match (wanted.parse::<f64>(), observed.parse::<f64>()) {
        (Ok(a), Ok(b)) => (a - b).abs() <= 1e-6 * a.abs().max(1.0),
        _ => false,
    }
}

/// Read the true X11 root-window size in pixels: (width, height).
/// Shared by `get_screen_size` and `get_desktop_state`.
pub(crate) fn x11_screen_size() -> anyhow::Result<(u32, u32)> {
    use x11rb::connection::Connection;
    use x11rb::rust_connection::RustConnection;
    let (conn, screen_num) = RustConnection::connect(None)
        .map_err(|e| anyhow::anyhow!("{e}{}", crate::no_display_hint()))?;
    let setup = conn.setup();
    let screen = &setup.roots[screen_num];
    let w = screen.width_in_pixels as u32;
    let h = screen.height_in_pixels as u32;
    // WSLg / headless XWayland quirk: the X server connects but the
    // root screen advertises a 0-px geometry until a real output is
    // attached. Returning {width:0,height:0} here would propagate a
    // success with zero dimensions to the client, which then either
    // divides by zero when scaling or feeds the value into `int(...)`
    // after the missing key collapses to None. Fail loudly with an
    // actionable, typed error instead (never emit a 0/null where the
    // client expects a usable int). See issue #2005.
    if w == 0 || h == 0 {
        anyhow::bail!(
            "X11 connected but reports a 0x0 root screen — no usable \
             display geometry.{}",
            crate::no_display_hint()
        );
    }
    Ok((w, h))
}

/// Put the desktop image in the exact coordinate frame consumed by desktop
/// actions. Native Wayland capture buffers may use backing pixels while the
/// compositor's pointer protocol and reported screen geometry use logical
/// pixels. Returning the backing image alongside logical dimensions violates
/// the screenshot-to-action contract and makes every vision-grounded action
/// miss by the output scale.
pub(crate) fn normalize_desktop_capture_for_action_frame(
    png: Vec<u8>,
    action_width: u32,
    action_height: u32,
) -> anyhow::Result<(Vec<u8>, u32, u32, f64)> {
    if action_width == 0 || action_height == 0 {
        anyhow::bail!("desktop action frame is empty: {action_width}x{action_height}");
    }

    let (capture_width, capture_height) = crate::capture::png_dimensions_pub(&png)?;
    let scale_x = f64::from(capture_width) / f64::from(action_width);
    let scale_y = f64::from(capture_height) / f64::from(action_height);
    if (scale_x - scale_y).abs() > 0.01 {
        anyhow::bail!(
            "desktop capture {capture_width}x{capture_height} cannot be mapped uniformly to \
             action frame {action_width}x{action_height} (scale {scale_x:.4}x{scale_y:.4})"
        );
    }

    if capture_width == action_width && capture_height == action_height {
        return Ok((png, action_width, action_height, 1.0));
    }

    let image = image::load_from_memory_with_format(&png, image::ImageFormat::Png)?;
    let resized = image.resize_exact(
        action_width,
        action_height,
        image::imageops::FilterType::Lanczos3,
    );
    let mut encoded = std::io::Cursor::new(Vec::new());
    resized.write_to(&mut encoded, image::ImageFormat::Png)?;
    Ok((encoded.into_inner(), action_width, action_height, scale_x))
}

#[cfg(test)]
#[tokio::test]
async fn check_permissions_accepts_the_status_prompt_argument() {
    let result = build_registry_with_provider(false, None)
        .invoke("check_permissions", json!({ "prompt": false }))
        .await;
    let wire = serde_json::to_value(&result).unwrap();
    assert!(wire["structuredContent"].get("x11").is_some(), "{wire}");
}

// ── registry ─────────────────────────────────────────────────────────────────

pub fn build_registry_with_provider(
    compat: bool,
    provider: Option<std::sync::Arc<dyn cua_driver_core::consent::ProtectedConsentProvider>>,
) -> ToolRegistry {
    let mut r = ToolRegistry::new_with_protected_consent_provider(provider);
    let state = ToolState::new_with_capture_service(r.capture_service());
    if crate::wayland::is_wayland() && crate::wayland::hyprland::is_session() {
        let recording_state = Arc::downgrade(&state);
        r.recording
            .set_pixel_point_fn(move |args, window_id, pid, mut x, mut y| {
                let state = recording_state.upgrade()?;
                if let Some(pid) = pid {
                    let pid = u32::try_from(pid).ok()?;
                    // Match ClickTool's screenshot and zoom conversion before
                    // translating to the recording's full-output image.
                    if args.bool_or("from_zoom", false) {
                        (x, y) = state
                            .zoom_context(args, pid, args.opt_u64("window_id"))
                            .ok()?
                            .zoom_to_window(x, y);
                    } else if let Ok(ratio) =
                        screenshot_scale(&state, args, pid, args.opt_u64("window_id"))
                    {
                        x *= ratio;
                        y *= ratio;
                    }
                }
                crate::recording_hooks::hyprland_pixel_recording_point(window_id, pid, x, y)
            });
    }
    let cursor_outcome_reader = {
        let cursor_registry = state.cursor_registry.clone();
        cua_driver_core::session::register_scoped_cursor_outcome_reader(std::sync::Arc::new(
            move |session_id| {
                let state = cursor_registry.get(session_id);
                let motion_customized = state.is_some()
                    && crate::overlay::current_motion_for(session_id)
                        != cursor_overlay::MotionConfig::default();
                let active_cursor_count = cursor_registry
                    .all_states()
                    .iter()
                    .filter(|state| state.config.cursor_id != "default")
                    .count()
                    .max(1);
                match state {
                    Some(state) => cua_driver_core::session::bounded_cursor_outcome(
                        true,
                        state.config.enabled,
                        crate::overlay::is_visible_for_session(session_id),
                        Some(state.config.theme_id.as_str()),
                        motion_customized,
                        active_cursor_count,
                    ),
                    None => cua_driver_core::session::bounded_cursor_outcome(
                        false,
                        false,
                        false,
                        None,
                        false,
                        active_cursor_count,
                    ),
                }
            },
        ))
    };
    let session_end_hook = {
        let cursor_registry = state.cursor_registry.clone();
        let state_for_session_end = state.clone();
        cua_driver_core::session::register_scoped_fallible_session_end_hook(
            "linux_input",
            move |session_id| {
                if crate::wayland::is_wayland() {
                    release_mouse_hold_for_session(
                        &state_for_session_end,
                        session_id,
                        |cursor_id, held| {
                            crate::wayland::persistent_vptr::release_at_session_end(
                                cursor_id,
                                held.button,
                            )
                        },
                    )?;
                    // A press the tool never recorded (its recording failed
                    // and its release was not confirmed) is released too.
                    crate::wayland::persistent_vptr::release_unrecorded_at_session_end(session_id)
                        .map_err(|error| error.to_string())?;
                } else {
                    clear_mouse_hold_after_release(&state_for_session_end, session_id);
                }

                state_for_session_end
                    .snapshots
                    .retire_session_screenshots(session_id);
                cursor_registry.remove(session_id);
                crate::overlay::remove_cursor(session_id.to_owned());
                crate::input::forget_master_pointer(session_id);
                crate::wayland::hyprland_input::cleanup_session(session_id);
                Ok(())
            },
        )
    };
    let session_revive_hook =
        cua_driver_core::session::register_scoped_session_revive_hook(move |session_id| {
            crate::overlay::revive_cursor(session_id.to_owned());
        });
    r.retain_cursor_outcome_reader(cursor_outcome_reader);
    r.retain_session_end_hook(session_end_hook);
    r.retain_session_revive_hook(session_revive_hook);
    if let Some(runtime_scope) = cua_driver_core::tool::current_dispatch_runtime_scope() {
        let prefix = format!("__cua_runtime_{runtime_scope}:");
        let cursor_registry = state.cursor_registry.clone();
        let capture_service = state.capture_service.clone();
        r.retain_runtime_cleanup(move || {
            crate::capture_action_frame::retire_runtime(&capture_service);
            crate::wayland::hyprland_input::cleanup_runtime(&prefix);
            for cursor in cursor_registry
                .all_states()
                .into_iter()
                .filter(|cursor| cursor.config.cursor_id.starts_with(&prefix))
            {
                cursor_registry.remove(&cursor.config.cursor_id);
                crate::overlay::remove_cursor(cursor.config.cursor_id);
            }
        });
    }
    r.register(Box::new(ListAppsTool));
    r.register(Box::new(ListWindowsTool));
    r.register(Box::new(GetWindowStateTool {
        state: state.clone(),
    }));
    r.register(Box::new(
        cua_driver_core::expectation::VerifyStateTool::new(std::sync::Arc::new(
            cua_driver_core::expectation::ToolObservationProvider::new(
                std::sync::Arc::new(ListWindowsTool),
                std::sync::Arc::new(GetWindowStateTool {
                    state: state.clone(),
                }),
            ),
        )),
    ));
    r.register(Box::new(LaunchAppTool));
    r.register(Box::new(KillAppTool));
    let pid_window_candidates: PidWindowGuardParts = (
        Arc::new(pid_window_target_candidates),
        desktop_point_window_resolver(),
        pid_fallback_window_resolver(),
        snapshot_window_resolver(state.clone()),
        token_pid_resolver(state.clone()),
    );
    r.register(pid_window_guarded(BringToFrontTool, &pid_window_candidates));
    r.register(Box::new(SetWindowFrameTool));
    r.register(Box::new(InvokeMenuTool));
    r.register(pid_window_guarded(
        ClickTool {
            state: state.clone(),
        },
        &pid_window_candidates,
    ));
    r.register(pid_window_guarded(
        DoubleClickTool {
            state: state.clone(),
        },
        &pid_window_candidates,
    ));
    r.register(pid_window_guarded(
        RightClickTool {
            state: state.clone(),
        },
        &pid_window_candidates,
    ));
    r.register(pid_window_guarded(
        DragTool {
            state: state.clone(),
        },
        &pid_window_candidates,
    ));
    r.register(pid_window_guarded(
        MouseButtonDownTool {
            state: state.clone(),
        },
        &pid_window_candidates,
    ));
    r.register(pid_window_guarded(
        MouseDragTool {
            state: state.clone(),
        },
        &pid_window_candidates,
    ));
    r.register(pid_window_guarded(
        MouseButtonUpTool {
            state: state.clone(),
        },
        &pid_window_candidates,
    ));
    r.register(Box::new(ParallelMouseDragTool {
        state: state.clone(),
    }));
    r.register(pid_window_guarded(
        TypeTextTool {
            state: state.clone(),
        },
        &pid_window_candidates,
    ));
    r.register(pid_window_guarded(
        PressKeyTool {
            state: state.clone(),
        },
        &pid_window_candidates,
    ));
    r.register(pid_window_guarded(
        HotkeyTool {
            state: state.clone(),
        },
        &pid_window_candidates,
    ));
    r.register(pid_window_guarded(
        SetValueTool {
            state: state.clone(),
        },
        &pid_window_candidates,
    ));
    r.register(pid_window_guarded(
        ScrollTool {
            state: state.clone(),
        },
        &pid_window_candidates,
    ));
    cua_driver_core::clipboard::register_clipboard_tools(
        &mut r,
        Arc::new(crate::clipboard::LinuxClipboard::new()),
    );
    // `screenshot` removed - see the matching comment in
    // platform-windows/src/tools/impl_.rs::build_registry. Canonical
    // screenshot path is `get_window_state` (it always returns a screenshot now).
    let _ = compat;
    r.register(Box::new(GetScreenSizeTool));
    r.register(Box::new(GetDesktopStateTool {
        state: state.clone(),
    }));
    r.register(Box::new(GetCursorPositionTool));
    r.register(Box::new(MoveCursorTool {
        state: state.clone(),
    }));
    r.register(Box::new(SetAgentCursorEnabledV2Tool {
        state: state.clone(),
    }));
    r.register(Box::new(SetAgentCursorMotionV2Tool));
    r.register(Box::new(GetAgentCursorStateV2Tool {
        state: state.clone(),
    }));
    r.register(Box::new(SetAgentCursorThemeTool {
        state: state.clone(),
    }));
    r.register(Box::new(CheckPermissionsTool));
    // `health_report` — single-call cross-platform driver diagnostics.
    // Stable schema_version="1" contract for downstream consumers. Linux skips
    // tcc_* and bundle_identity with "not applicable on Linux".
    r.register(Box::new(
        cua_driver_core::health_report::HealthReportTool::new(std::sync::Arc::new(
            crate::health_report::LinuxHealthProvider,
        )),
    ));
    r.register(Box::new(GetConfigTool {
        state: state.clone(),
    }));
    r.register(Box::new(SetConfigTool {
        state: state.clone(),
    }));
    r.register(Box::new(GetAccessibilityTreeTool));
    r.register(Box::new(ZoomTool {
        state: state.clone(),
    }));
    // `type_text_chars` is a deprecated invoke-time alias for `type_text`.
    // Keep it out of tools/list, matching the macOS and Windows registries.
    let _: &TypeTextCharsTool = &TypeTextCharsTool;
    // Cross-platform `page` tool definition lives in mcp-server; Linux plugs
    // in its AT-SPI + CDP backend here.
    r.register(Box::new(cua_driver_core::page::PageTool::new(Arc::new(
        super::page::LinuxPageBackend::new(),
    ))));
    let browser_engine = cua_driver_core::browser::BrowserEngine::new_with_runtime_services(
        Arc::new(crate::browser_platform::LinuxBrowserPlatform::new(
            state.cursor_registry.clone(),
        )),
        r.approval_broker(),
        r.protected_resource_ownership(),
    );
    cua_driver_core::browser::register_browser_tools(&browser_engine, &mut r);
    r.register_recording_tools();
    r.register_session_tools();
    r
}

#[cfg(test)]
mod pid_window_target_tests {
    use super::*;
    use cua_driver_core::window_target::{resolve_pid_window_target, PidWindowTargetResolution};

    fn window(xid: u64, pid: u32) -> crate::x11::WindowInfo {
        crate::x11::WindowInfo {
            xid,
            pid: Some(pid),
            app_name: "editor".into(),
            title: format!("Document {xid}"),
            is_on_screen: true,
            z_index: Some(1),
            x: 0,
            y: 0,
            width: 640,
            height: 480,
        }
    }

    #[test]
    fn same_pid_sibling_windows_are_ambiguous() {
        let candidates =
            window_target_candidates_for_pid([window(7, 42), window(8, 42), window(9, 99)], 42);
        assert!(matches!(
            resolve_pid_window_target(candidates),
            PidWindowTargetResolution::Ambiguous(windows)
                if windows.iter().map(|window| window.window_id).collect::<Vec<_>>() == [7, 8]
        ));
    }
}

#[cfg(test)]
mod desktop_capture_frame_tests {
    use super::{capture_admission_error, normalize_desktop_capture_for_action_frame};

    fn png(width: u32, height: u32) -> Vec<u8> {
        let rgba = vec![0x7f; (width * height * 4) as usize];
        cua_driver_core::image_utils::encode_rgba_to_png(&rgba, width, height)
            .expect("encode fixture")
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
}

#[cfg(test)]
mod background_keyboard_route_tests {
    use super::{element_needs_real_click, split_key_combo, wm_chord_kind, WmChord};

    #[test]
    fn key_combos_split_into_modifiers_and_key() {
        assert_eq!(
            split_key_combo("alt+F4"),
            (vec!["alt".to_owned()], "F4".to_owned())
        );
        assert_eq!(
            split_key_combo("ctrl+shift+t"),
            (vec!["ctrl".to_owned(), "shift".to_owned()], "t".to_owned())
        );
        assert_eq!(split_key_combo("+"), (vec![], "+".to_owned()));
        assert_eq!(
            split_key_combo("ctrl++"),
            (vec!["ctrl".to_owned()], "+".to_owned())
        );
        assert_eq!(split_key_combo("Return"), (vec![], "Return".to_owned()));
        // Not a modifier prefix: left untouched for the keysym resolver.
        assert_eq!(split_key_combo("a+b"), (vec![], "a+b".to_owned()));
    }

    #[test]
    fn wm_chords_are_recognised() {
        let m = |xs: &[&str]| xs.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(
            wm_chord_kind("F4", &m(&["alt"])),
            Some(WmChord::CloseWindow)
        );
        assert_eq!(
            wm_chord_kind("f4", &m(&["Alt_L"])),
            Some(WmChord::CloseWindow)
        );
        assert_eq!(
            wm_chord_kind("Tab", &m(&["alt"])),
            Some(WmChord::Unavailable)
        );
        assert_eq!(
            wm_chord_kind("Tab", &m(&["alt", "shift"])),
            Some(WmChord::Unavailable)
        );
        assert_eq!(
            wm_chord_kind("a", &m(&["super"])),
            Some(WmChord::Unavailable)
        );
        assert_eq!(
            wm_chord_kind("t", &m(&["ctrl", "alt"])),
            Some(WmChord::Unavailable)
        );
        assert_eq!(
            wm_chord_kind("F4", &m(&["ctrl", "alt"])),
            Some(WmChord::Unavailable)
        );
        // Application chords stay with the application.
        assert_eq!(wm_chord_kind("c", &m(&["ctrl"])), None);
        assert_eq!(wm_chord_kind("F4", &m(&["ctrl"])), None);
        assert_eq!(wm_chord_kind("f", &m(&["alt"])), None);
        assert_eq!(wm_chord_kind("F4", &m(&[])), None);
        assert_eq!(wm_chord_kind("s", &m(&["ctrl", "shift"])), None);
    }

    #[test]
    fn focus_taking_roles_need_a_real_click() {
        for role in ["spin button", "Text", "entry", "slider", "combo box"] {
            assert!(element_needs_real_click(role), "{role}");
        }
        for role in ["menu item", "push button", "menu", "check box", "page tab"] {
            assert!(!element_needs_real_click(role), "{role}");
        }
    }
}

#[cfg(test)]
mod visibility_tests {
    use super::*;

    #[test]
    fn framed_elements_lead_and_keep_their_relative_order() {
        let elements = vec![
            json!({"element_index": 0, "role": "menu"}),
            json!({"element_index": 1, "role": "push button", "frame": {"x": 1, "y": 1, "w": 2, "h": 2}}),
            json!({"element_index": 2, "role": "menu"}),
            json!({"element_index": 3, "role": "paragraph", "frame": {"x": 1, "y": 9, "w": 2, "h": 2}}),
        ];
        let ordered: Vec<u64> = framed_elements_first(elements)
            .iter()
            .map(|e| e["element_index"].as_u64().unwrap())
            .collect();
        assert_eq!(ordered, vec![1, 3, 0, 2]);
    }

    #[test]
    fn a_frameless_popup_returns_only_the_menu_entries_drawn_inside_it() {
        let elements = vec![
            json!({"element_index": 1, "role": "push button", "frame": {"x": 5, "y": 5, "w": 20, "h": 20}}),
            json!({"element_index": 2, "role": "menu item", "label": "Paragraph...", "frame": {"x": 2, "y": 30, "w": 180, "h": 22}}),
            json!({"element_index": 3, "role": "menu item", "label": "elsewhere", "frame": {"x": 400, "y": 30, "w": 180, "h": 22}}),
            json!({"element_index": 4, "role": "menu", "label": "Format"}),
        ];
        let kept: Vec<u64> = popup_menu_elements(elements.clone(), (0, 0), 200, 400)
            .iter()
            .map(|e| e["element_index"].as_u64().unwrap())
            .collect();
        assert_eq!(kept, vec![2]);
        assert_eq!(
            popup_menu_elements(elements[..1].to_vec(), (0, 0), 200, 400).len(),
            1
        );
    }

    #[test]
    fn a_combo_popup_returns_the_rows_of_the_list_that_fills_it() {
        // Qt: the combo list lives under the combo box inside the dialog; the
        // main window's menubar entries are elsewhere on the screen.
        let elements = vec![
            json!({"element_index": 1, "role": "menu", "label": "Audio", "frame": {"x": -600, "y": -300, "w": 50, "h": 20}}),
            json!({"element_index": 7, "role": "list", "frame": {"x": 1, "y": 1, "w": 198, "h": 118}}),
            json!({"element_index": 8, "role": "list item", "label": "Desktop", "parent_index": 7, "frame": {"x": 2, "y": 2, "w": 190, "h": 22}}),
            json!({"element_index": 9, "role": "list item", "label": "user", "parent_index": 7, "frame": {"x": 2, "y": 26, "w": 190, "h": 22}}),
            json!({"element_index": 12, "role": "push button", "label": "Open", "frame": {"x": 20, "y": 20, "w": 60, "h": 20}}),
        ];
        let kept: Vec<u64> = popup_menu_elements(elements, (0, 0), 200, 120)
            .iter()
            .map(|e| e["element_index"].as_u64().unwrap())
            .collect();
        assert_eq!(kept, vec![8, 9]);
    }

    #[test]
    fn a_list_popup_without_an_indexed_container_keeps_its_item_rows_only() {
        let elements = vec![
            json!({"element_index": 1, "role": "menu", "label": "Audio", "frame": {"x": 1, "y": 1, "w": 50, "h": 20}}),
            json!({"element_index": 8, "role": "list item", "label": "Desktop", "frame": {"x": 2, "y": 2, "w": 190, "h": 22}}),
            json!({"element_index": 9, "role": "tree item", "label": "user", "frame": {"x": 2, "y": 26, "w": 190, "h": 22}}),
            json!({"element_index": 12, "role": "push button", "label": "Open", "frame": {"x": 20, "y": 20, "w": 60, "h": 20}}),
        ];
        let kept: Vec<u64> = popup_menu_elements(elements, (0, 0), 200, 120)
            .iter()
            .map(|e| e["element_index"].as_u64().unwrap())
            .collect();
        assert_eq!(kept, vec![8, 9]);
    }

    #[test]
    fn menu_roles_take_the_real_press() {
        for role in ["menu", "Menu Item", "check menu item", "radio menu item"] {
            assert!(element_is_menu_role(role), "{role}");
        }
        for role in ["menu bar", "push button", "text"] {
            assert!(!element_is_menu_role(role), "{role}");
        }
    }

    #[test]
    fn overlays_name_the_follow_up_call_per_dialog_and_popup() {
        let overlays = WindowOverlays {
            dialogs: vec![json!({
                "window_id": 71, "title": "Position and Size", "transient_for": 3, "modal": true,
                "bounds": {"x": 100, "y": 100, "width": 400, "height": 300}
            })],
            popups: vec![json!({
                "window_id": 72, "title": "", "pid": 5,
                "bounds": {"x": 10, "y": 30, "width": 200, "height": 400}
            })],
            covers_window: true,
            window_rect: Some((0, 0, 800, 600)),
        };
        let note = overlays.follow_up(5).unwrap();
        assert!(
            note.contains(
                "dialog \"Position and Size\" (window_id 71, transient of window 3, modal)"
            ),
            "{note}"
        );
        assert!(
            note.contains("get_window_state(pid=5, window_id=71)"),
            "{note}"
        );
        assert!(
            note.contains("popup (window_id 72, bounds x=10 y=30 200x400)"),
            "{note}"
        );
        assert!(rects_intersect((0, 0, 800, 600), (100, 100, 400, 300)));
        assert!(!rects_intersect((0, 0, 800, 600), (800, 0, 10, 10)));
    }

    #[test]
    fn a_cross_process_dialog_names_its_real_owning_pid_in_the_follow_up() {
        // GIMP's "Export Image as JPEG" dialog spawned as a separate process
        // (owning_pid 999) but is transient-for the target pid (5)'s window.
        let overlays = WindowOverlays {
            dialogs: vec![json!({
                "window_id": 90, "title": "Export Image as JPEG", "transient_for": 10,
                "modal": false,
                "bounds": {"x": 0, "y": 0, "width": 300, "height": 200},
                "owning_pid": 999,
            })],
            popups: vec![],
            covers_window: false,
            window_rect: Some((0, 0, 800, 600)),
        };
        let note = overlays.follow_up(5).unwrap();
        assert!(
            note.contains("owned by a different process pid 999, not pid 5"),
            "{note}"
        );
        assert!(
            note.contains("get_window_state(pid=5, window_id=90)"),
            "{note}"
        );
    }
}

#[cfg(test)]
mod cursor_hook_emission_tests {
    use super::emit_cursor_hook;
    use cua_driver_core::cursor_hook::{set_cursor_hook_fn, CursorHookEvent};
    use std::sync::{Arc, Mutex};

    /// Linux pointer actions report the agent cursor to an embedder's hook
    /// (cua-spacesd turns them into presence), moves and presses alike, and
    /// an anonymous cursor id never reaches it.
    #[test]
    fn pointer_actions_reach_the_cursor_hook() {
        let seen: Arc<Mutex<Vec<CursorHookEvent>>> = Arc::default();
        let sink = seen.clone();
        if !set_cursor_hook_fn(move |e| sink.lock().unwrap().push(e)) {
            // Another test in this binary owns the process-wide hook.
            return;
        }
        emit_cursor_hook("agent-7", 10.0, 20.0, false);
        emit_cursor_hook("agent-7", 10.0, 20.0, true);
        emit_cursor_hook("", 1.0, 1.0, false);
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(
            (
                seen[0].cursor_id.as_str(),
                seen[0].x,
                seen[0].y,
                seen[0].pressed
            ),
            ("agent-7", 10.0, 20.0, false)
        );
        assert!(seen[1].pressed);
    }
}

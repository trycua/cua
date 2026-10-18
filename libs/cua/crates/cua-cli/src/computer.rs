//! Computer actions over a sandbox's cua-spacesd, shared by `cua do` and
//! `cua mcp`: screenshots with window zoom and coordinate mapping, pointer,
//! keyboard, clipboard, windows (`WindowsService`) and accessibility
//! (`AccessibilityService`).

use cua_sdk::{CuaError, SpacesdClient};
use cua_spacesd_client::{KeySpec, pb};
use std::sync::Arc;

/// Long-edge cap for screenshots handed to agents (as the Python CLI).
pub const MAX_LENGTH: u32 = 1200;

/// Maps an env error (a gone window or element is `NotFound`).
pub fn env_err(e: cua_spacesd_client::Error) -> CuaError {
    match e {
        cua_spacesd_client::Error::TargetUnavailable(_)
        | cua_spacesd_client::Error::StaleHandle(_) => CuaError::NotFound(e.to_string()),
        e => e.into(),
    }
}

/// A screenshot and the mapping from its pixels to screen points.
#[derive(Clone, Debug)]
pub struct Shot {
    /// PNG bytes.
    pub png: Vec<u8>,
    /// Image pixels per logical point.
    pub scale: f64,
    /// Top-left of the captured area in screen points.
    pub origin: (f64, f64),
}

/// `origin + pixel / scale` (scale 0 is treated as 1).
pub fn map_point(p: (f64, f64), scale: f64, origin: (f64, f64)) -> (f64, f64) {
    let s = if scale > 0.0 { scale } else { 1.0 };
    (origin.0 + p.0 / s, origin.1 + p.1 / s)
}

/// A computer: one spacesd.
#[derive(Clone)]
pub struct Computer {
    env: Arc<SpacesdClient>,
}

impl Computer {
    /// Wraps an SDK env client.
    pub fn new(env: Arc<SpacesdClient>) -> Self {
        Self { env }
    }

    fn c(&self) -> &cua_spacesd_client::SpacesdClient {
        self.env.inner()
    }

    /// Screenshot of the primary display or one window, capped at
    /// `max_dimension` pixels on the long edge (0: native).
    pub async fn screenshot(
        &self,
        window: Option<pb::WindowRef>,
        max_dimension: u32,
    ) -> Result<Shot, CuaError> {
        let r = self
            .c()
            .computer()
            .screenshot(pb::ScreenshotRequest {
                source: window.map(pb::screenshot_request::Source::Window),
                region: None,
                format: pb::ImageFormat::Png as i32,
                quality: 0,
                max_dimension,
                include_cursor: false,
            })
            .await
            .map_err(|s| env_err(s.into()))?
            .into_inner();
        let b = r.logical_bounds.unwrap_or_default();
        Ok(Shot {
            png: r.image,
            scale: if r.scale > 0.0 { r.scale } else { 1.0 },
            origin: (b.x, b.y),
        })
    }

    /// Clicks with `button` (`left`, `right`, `middle`) `count` times.
    pub async fn click(&self, x: f64, y: f64, button: &str, count: u32) -> Result<(), CuaError> {
        self.c()
            .click_with(x, y, mouse_button(button)?, count)
            .await
            .map_err(env_err)?;
        Ok(())
    }

    /// Moves the pointer.
    pub async fn move_to(&self, x: f64, y: f64) -> Result<(), CuaError> {
        self.c().move_to(x, y).await.map_err(env_err)?;
        Ok(())
    }

    /// Presses or releases a mouse button (at the current position when no
    /// point is given).
    pub async fn mouse_button(
        &self,
        down: bool,
        at: Option<(f64, f64)>,
        button: &str,
    ) -> Result<(), CuaError> {
        use pb::pointer_request::Action;
        let position = at.map(|(x, y)| pb::Point { x, y });
        let button = mouse_button(button)? as i32;
        let action = if down {
            Action::Down(pb::PointerDown { position, button })
        } else {
            Action::Up(pb::PointerUp { position, button })
        };
        self.c().pointer(None, action).await.map_err(env_err)?;
        Ok(())
    }

    /// Scrolls `clicks` lines in a direction, optionally at a point.
    pub async fn scroll(
        &self,
        direction: &str,
        clicks: i64,
        at: Option<(f64, f64)>,
    ) -> Result<(), CuaError> {
        let n = clicks as f64;
        let (dx, dy) = match direction {
            "up" => (0.0, -n),
            "down" => (0.0, n),
            "left" => (-n, 0.0),
            "right" => (n, 0.0),
            d => {
                return Err(CuaError::InvalidArgument(format!(
                    "scroll direction must be up, down, left or right, not {d:?}"
                )));
            }
        };
        self.c()
            .pointer(
                None,
                pb::pointer_request::Action::Scroll(pb::PointerScroll {
                    position: at.map(|(x, y)| pb::Point { x, y }),
                    delta_x: dx,
                    delta_y: dy,
                    unit: pb::ScrollUnit::Line as i32,
                }),
            )
            .await
            .map_err(env_err)?;
        Ok(())
    }

    /// Drags with the left button.
    pub async fn drag(&self, from: (f64, f64), to: (f64, f64)) -> Result<(), CuaError> {
        self.c().drag(from, to).await.map_err(env_err)?;
        Ok(())
    }

    /// Types text.
    pub async fn type_text(&self, text: &str) -> Result<(), CuaError> {
        self.c().type_text(text).await.map_err(env_err)?;
        Ok(())
    }

    /// Presses one key.
    pub async fn press(&self, key: &str) -> Result<(), CuaError> {
        KeySpec::parse(key).map_err(|e| CuaError::InvalidArgument(e.to_string()))?;
        self.c().press(key).await.map_err(env_err)?;
        Ok(())
    }

    /// Presses a chord.
    pub async fn hotkey(&self, keys: &[String]) -> Result<(), CuaError> {
        let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
        self.c().hotkey(&refs).await.map_err(|e| match e {
            cua_spacesd_client::Error::Protocol(m) => CuaError::InvalidArgument(m),
            e => env_err(e),
        })?;
        Ok(())
    }

    /// Holds or releases one key.
    pub async fn key_state(&self, key: &str, down: bool) -> Result<(), CuaError> {
        use pb::keyboard_request::Action;
        let input =
            match KeySpec::parse(key).map_err(|e| CuaError::InvalidArgument(e.to_string()))? {
                KeySpec::Named(k) => pb::key_input::Key::Named(k as i32),
                KeySpec::Char(c) => pb::key_input::Key::Character(c),
            };
        let key = Some(pb::KeyInput { key: Some(input) });
        let action = if down {
            Action::Down(pb::KeyboardDown { key })
        } else {
            Action::Up(pb::KeyboardUp { key })
        };
        self.c().keyboard(None, action).await.map_err(env_err)?;
        Ok(())
    }

    /// Clipboard text.
    pub async fn clipboard(&self) -> Result<Option<String>, CuaError> {
        self.c().get_clipboard().await.map_err(env_err)
    }

    /// Sets clipboard text.
    pub async fn set_clipboard(&self, text: &str) -> Result<(), CuaError> {
        self.c().set_clipboard(text).await.map_err(env_err)?;
        Ok(())
    }

    /// Pointer position in screen points.
    pub async fn cursor(&self) -> Result<(f64, f64), CuaError> {
        self.c().cursor_position().await.map_err(env_err)
    }

    /// Primary display size in logical points.
    pub async fn screen_size(&self) -> Result<(f64, f64), CuaError> {
        let d = self.c().displays().await.map_err(env_err)?;
        let p = d
            .iter()
            .find(|d| d.primary)
            .or(d.first())
            .ok_or_else(|| CuaError::NotFound("the sandbox reports no displays".into()))?;
        if let Some(b) = p.bounds {
            return Ok((b.width, b.height));
        }
        let n = p.native_size.unwrap_or_default();
        let s = if p.scale_factor > 0.0 {
            p.scale_factor
        } else {
            1.0
        };
        Ok((n.width as f64 / s, n.height as f64 / s))
    }

    /// Runs a shell line.
    pub async fn shell(
        &self,
        line: &str,
        timeout_ms: Option<u32>,
    ) -> Result<cua_sdk::ProcessOutput, CuaError> {
        self.env.sh(line.to_string(), timeout_ms).await
    }

    // ----------------------------------------------------------- windows

    /// Windows whose app name or title contains `filter` (case-insensitive;
    /// empty: all), excluding internal helper windows.
    pub async fn windows(&self, filter: &str) -> Result<Vec<pb::WindowInfo>, CuaError> {
        let all = self
            .c()
            .windows()
            .list_windows(pb::ListWindowsRequest { filter: None })
            .await
            .map_err(|s| env_err(s.into()))?
            .into_inner()
            .windows;
        let f = filter.to_lowercase();
        Ok(all
            .into_iter()
            .filter(|w| !SKIP_TITLES.contains(&w.title.as_str()))
            .filter(|w| {
                f.is_empty()
                    || w.title.to_lowercase().contains(&f)
                    || w.app
                        .as_ref()
                        .is_some_and(|a| a.name.to_lowercase().contains(&f))
            })
            .collect())
    }

    /// The window with id `id` (its current ref, including the epoch).
    pub async fn window(&self, id: &str) -> Result<pb::WindowInfo, CuaError> {
        self.windows("")
            .await?
            .into_iter()
            .find(|w| window_id(w) == id)
            .ok_or_else(|| CuaError::NotFound(format!("no window {id}")))
    }

    /// The focused window, if any.
    pub async fn focused_window(&self) -> Result<Option<pb::WindowInfo>, CuaError> {
        Ok(self.windows("").await?.into_iter().find(|w| w.focused))
    }

    /// Activates, minimizes, maximizes, restores or closes a window.
    pub async fn window_op(&self, id: &str, op: &str) -> Result<(), CuaError> {
        let w = self.window(id).await?.r#ref;
        let ws = self.c().windows();
        let mut ws = ws;
        let r = match op {
            "activate" | "focus" => ws
                .activate_window(pb::ActivateWindowRequest { window: w })
                .await
                .map(|_| ()),
            "minimize" => ws
                .minimize_window(pb::MinimizeWindowRequest { window: w })
                .await
                .map(|_| ()),
            "maximize" => ws
                .maximize_window(pb::MaximizeWindowRequest { window: w })
                .await
                .map(|_| ()),
            "restore" => ws
                .restore_window(pb::RestoreWindowRequest { window: w })
                .await
                .map(|_| ()),
            "close" => ws
                .close_window(pb::CloseWindowRequest {
                    window: w,
                    force: false,
                })
                .await
                .map(|_| ()),
            other => {
                return Err(CuaError::InvalidArgument(format!(
                    "unknown window operation {other}"
                )));
            }
        };
        r.map_err(|s| env_err(s.into()))
    }

    /// Moves and/or resizes a window.
    pub async fn set_bounds(
        &self,
        id: &str,
        position: Option<(f64, f64)>,
        size: Option<(f64, f64)>,
    ) -> Result<pb::WindowInfo, CuaError> {
        let w = self.window(id).await?.r#ref;
        let r = self
            .c()
            .windows()
            .set_window_bounds(pb::SetWindowBoundsRequest {
                window: w,
                position: position.map(|(x, y)| pb::Point { x, y }),
                width: size.map(|s| s.0),
                height: size.map(|s| s.1),
            })
            .await
            .map_err(|s| env_err(s.into()))?;
        Ok(r.into_inner().window.unwrap_or_default())
    }

    /// Opens a URL or path with the default handler.
    pub async fn open(&self, target: &str) -> Result<(), CuaError> {
        let t = if target.contains("://") {
            pb::open_request::Target::Url(target.into())
        } else {
            pb::open_request::Target::Path(target.into())
        };
        self.c()
            .windows()
            .open(pb::OpenRequest {
                target: Some(t),
                with_app: None,
                delivery: 0,
            })
            .await
            .map_err(|s| env_err(s.into()))?;
        Ok(())
    }

    /// Waits (at most `timeout`) until the desktop can take input: a
    /// display, a window manager where the platform has one, and
    /// cua-driver input (`Health`'s `desktop` component). A guest without a
    /// desktop at all is left to fail on the action itself.
    pub async fn ensure_desktop(&self, timeout: std::time::Duration) -> Result<(), CuaError> {
        match self.c().wait_desktop_ready(timeout).await {
            Ok(_) | Err(cua_spacesd_client::Error::FeatureUnsupported { .. }) => Ok(()),
            Err(e) => Err(env_err(e)),
        }
    }

    /// Launches an app like [`Computer::launch`], then waits (at most
    /// `focus_wait`) for its first new window and activates it, so input
    /// that follows reaches it. Returns the pid and the window, if one
    /// appeared.
    pub async fn launch_and_focus(
        &self,
        app: &str,
        args: Vec<String>,
        focus_wait: std::time::Duration,
    ) -> Result<(u32, Option<pb::WindowInfo>), CuaError> {
        let before: std::collections::HashSet<String> = self
            .windows("")
            .await
            .unwrap_or_default()
            .iter()
            .map(window_id)
            .collect();
        let pid = self.launch(app, args).await?;
        let deadline = tokio::time::Instant::now() + focus_wait;
        loop {
            let new = self
                .windows("")
                .await
                .unwrap_or_default()
                .into_iter()
                .find(|w| !before.contains(&window_id(w)) && !window_id(w).is_empty());
            if let Some(w) = new {
                if !w.focused {
                    // Best effort: the window is there even if it cannot be
                    // raised; the next input reports its own delivery.
                    let _ = self.window_op(&window_id(&w), "activate").await;
                }
                return Ok((pid, Some(w)));
            }
            if tokio::time::Instant::now() >= deadline {
                return Ok((pid, None));
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    }

    /// Launches an app by name, id or executable.
    pub async fn launch(&self, app: &str, args: Vec<String>) -> Result<u32, CuaError> {
        let r = self
            .c()
            .windows()
            .launch_app(pb::LaunchAppRequest {
                app: Some(pb::AppSpec {
                    app: Some(pb::app_spec::App::Name(app.into())),
                }),
                args,
                ..Default::default()
            })
            .await
            .map_err(|s| env_err(s.into()))?;
        Ok(r.into_inner().pid)
    }

    // ------------------------------------------------------ accessibility

    /// The accessibility tree of a window (or the focused one).
    pub async fn a11y_tree(
        &self,
        window: Option<&str>,
        max_depth: u32,
    ) -> Result<pb::GetTreeResponse, CuaError> {
        let window = match window {
            Some(id) => self.window(id).await?.r#ref,
            None => None,
        };
        Ok(self
            .c()
            .accessibility()
            .get_tree(pb::GetTreeRequest {
                window,
                max_depth,
                include_hidden: false,
                max_nodes: 2000,
            })
            .await
            .map_err(|s| env_err(s.into()))?
            .into_inner())
    }

    /// Finds elements.
    pub async fn a11y_find(
        &self,
        window: Option<&str>,
        name_contains: &str,
        role: &str,
    ) -> Result<pb::FindResponse, CuaError> {
        let window = match window {
            Some(id) => self.window(id).await?.r#ref,
            None => None,
        };
        Ok(self
            .c()
            .accessibility()
            .find(pb::FindRequest {
                window,
                query: Some(pb::AccessibilityQuery {
                    role: role.into(),
                    name_contains: name_contains.into(),
                    ..Default::default()
                }),
                max_results: 100,
            })
            .await
            .map_err(|s| env_err(s.into()))?
            .into_inner())
    }

    /// Performs an accessibility action (`press`, `focus`, `set_value`, ...).
    pub async fn a11y_act(
        &self,
        snapshot: &str,
        element: &str,
        action: &str,
        value: &str,
    ) -> Result<(), CuaError> {
        let a = pb::AccessibilityAction::from_str_name(&format!(
            "ACCESSIBILITY_ACTION_{}",
            action.to_ascii_uppercase().replace('-', "_")
        ))
        .ok_or_else(|| CuaError::InvalidArgument(format!("unknown action {action}")))?;
        self.c()
            .accessibility()
            .act(pb::ActRequest {
                element: Some(pb::ElementRef {
                    snapshot_id: snapshot.into(),
                    element_id: element.into(),
                }),
                action: a as i32,
                value: value.into(),
                custom_action: String::new(),
                delivery: 0,
            })
            .await
            .map_err(|s| env_err(s.into()))?;
        Ok(())
    }
}

/// Internal helper windows hidden from listings.
const SKIP_TITLES: &[&str] = &["Chrome Legacy Window"];

/// A window's id.
pub fn window_id(w: &pb::WindowInfo) -> String {
    w.r#ref.as_ref().map(|r| r.id.clone()).unwrap_or_default()
}

/// `x,y,w,h` of a window.
pub fn bbox(w: &pb::WindowInfo) -> String {
    let b = w.bounds.unwrap_or_default();
    format!("{},{},{},{}", b.x, b.y, b.width, b.height)
}

/// One `cua do window ls` row: id, owning app (from cua-driver's window
/// info; `?` when the driver reports none), title, bounds, and ` *` for
/// the focused window.
pub fn window_line(w: &pb::WindowInfo) -> String {
    let app = w
        .app
        .as_ref()
        .map(|a| a.name.trim())
        .filter(|n| !n.is_empty())
        .unwrap_or("?");
    format!(
        "  {}  {app}  {}  [{}]{}",
        window_id(w),
        w.title,
        bbox(w),
        if w.focused { " *" } else { "" }
    )
}

/// A window as JSON.
pub fn window_json(w: &pb::WindowInfo) -> serde_json::Value {
    let b = w.bounds.unwrap_or_default();
    serde_json::json!({
        "id": window_id(w),
        "title": w.title,
        "app": w.app.as_ref().map(|a| a.name.clone()),
        "pid": w.app.as_ref().map(|a| a.pid),
        "position": [b.x, b.y],
        "size": [b.width, b.height],
        "focused": w.focused,
        "state": pb::WindowState::try_from(w.state)
            .map(|s| s.as_str_name().trim_start_matches("WINDOW_STATE_").to_lowercase())
            .unwrap_or_default(),
    })
}

/// A node as JSON.
pub fn node_json(n: &pb::AccessibilityNode) -> serde_json::Value {
    let b = n.bounds.unwrap_or_default();
    serde_json::json!({
        "id": n.element_id,
        "parent": n.parent_id,
        "depth": n.depth,
        "role": n.role,
        "name": n.name,
        "value": n.value,
        "bounds": [b.x, b.y, b.width, b.height],
    })
}

fn mouse_button(b: &str) -> Result<pb::MouseButton, CuaError> {
    Ok(match b {
        "" | "left" => pb::MouseButton::Left,
        "right" => pb::MouseButton::Right,
        "middle" => pb::MouseButton::Middle,
        o => {
            return Err(CuaError::InvalidArgument(format!(
                "button must be left, right or middle, not {o:?}"
            )));
        }
    })
}

/// Splits `cmd+c`, `ctrl-shift-s` into keys.
pub fn split_keys(s: &str) -> Vec<String> {
    s.replace('-', "+")
        .split('+')
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_pixels_through_scale_and_origin() {
        assert_eq!(
            map_point((600.0, 300.0), 0.5, (100.0, 50.0)),
            (1300.0, 650.0)
        );
        assert_eq!(map_point((10.0, 20.0), 0.0, (0.0, 0.0)), (10.0, 20.0));
    }

    #[test]
    fn splits_hotkeys() {
        assert_eq!(split_keys("cmd+c"), ["cmd", "c"]);
        assert_eq!(split_keys("ctrl-shift-s"), ["ctrl", "shift", "s"]);
    }

    #[test]
    fn window_rows_name_the_owning_app() {
        let mut w = pb::WindowInfo {
            r#ref: Some(pb::WindowRef {
                id: "42".into(),
                epoch: 1,
            }),
            title: "Untitled".into(),
            app: Some(pb::AppInfo {
                name: "TextEdit".into(),
                app_id: "com.apple.TextEdit".into(),
                pid: 7,
            }),
            bounds: Some(pb::Rect {
                x: 1.0,
                y: 2.0,
                width: 300.0,
                height: 200.0,
            }),
            focused: true,
            ..Default::default()
        };
        assert_eq!(window_line(&w), "  42  TextEdit  Untitled  [1,2,300,200] *");
        w.app = None;
        w.focused = false;
        assert_eq!(window_line(&w), "  42  ?  Untitled  [1,2,300,200]");
    }
}

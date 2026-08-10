//! Hyprland-specific identity resolution for per-toplevel capture.
//!
//! Standard foreign-toplevel protocols do not expose process identity or an
//! opaque handle accepted by Hyprland's toplevel-export protocol. Hyprland's
//! user-owned IPC supplies that missing correlation. Capture is authorized only
//! when one mapped client owned by the requested PID matches the compositor
//! title and app-id observed on the Wayland connection.

use anyhow::{bail, Context, Result};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use std::collections::HashSet;
use std::process::Command;

#[derive(Clone, Debug, Default, Deserialize)]
struct Workspace {
    #[serde(default)]
    id: i64,
}

#[derive(Clone, Debug, Deserialize)]
struct Client {
    address: String,
    mapped: bool,
    #[serde(default)]
    hidden: bool,
    pid: i64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    class: String,
    #[serde(default)]
    at: [i32; 2],
    #[serde(default)]
    size: [i32; 2],
    #[serde(default)]
    workspace: Workspace,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct Monitor {
    #[serde(default, rename = "activeWorkspace")]
    active_workspace: Workspace,
    #[serde(default)]
    x: i32,
    #[serde(default)]
    y: i32,
    #[serde(default)]
    width: u32,
    #[serde(default)]
    height: u32,
    #[serde(default)]
    scale: f64,
    #[serde(default)]
    transform: u8,
}

/// Logical compositor coordinate space accepted by Hyprland virtual pointers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OutputLayout {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Compositor-owned Hyprland metadata for one mapped toplevel.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Window {
    pub address: u64,
    pub pid: u32,
    pub title: String,
    pub app_id: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub workspace: i64,
    pub visible: bool,
}

pub fn is_session() -> bool {
    std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some()
        && std::env::var("XDG_CURRENT_DESKTOP")
            .ok()
            .is_some_and(|desktop| desktop.to_ascii_lowercase().contains("hyprland"))
}

fn windows_from_clients(clients: &[Client], active_workspaces: &HashSet<i64>) -> Vec<Window> {
    clients
        .iter()
        .filter(|client| client.mapped)
        .filter_map(|client| {
            let address = parse_address(&client.address)?;
            let pid = u32::try_from(client.pid).ok().filter(|pid| *pid != 0)?;
            Some(Window {
                address,
                pid,
                title: client.title.clone(),
                app_id: client.class.clone(),
                x: client.at[0],
                y: client.at[1],
                width: u32::try_from(client.size[0]).unwrap_or_default(),
                height: u32::try_from(client.size[1]).unwrap_or_default(),
                workspace: client.workspace.id,
                visible: !client.hidden && active_workspaces.contains(&client.workspace.id),
            })
        })
        .collect()
}

/// Read all mapped Hyprland clients with compositor-owned process, geometry,
/// workspace, and visibility metadata.
pub fn list_windows() -> Result<Vec<Window>> {
    if !is_session() {
        bail!("not a Hyprland session");
    }
    let monitors: Vec<Monitor> = hyprctl_json("monitors")?;
    let active_workspaces = monitors
        .into_iter()
        .map(|monitor| monitor.active_workspace.id)
        .filter(|workspace| *workspace != 0)
        .collect::<HashSet<_>>();
    Ok(windows_from_clients(&clients()?, &active_workspaces))
}

/// Return the logical bounding rectangle of all Hyprland outputs. Hyprland's
/// monitor positions and client coordinates are logical, while monitor mode
/// dimensions are physical and must be divided by scale. Virtual-pointer
/// absolute coordinates are normalized across this complete layout, not one
/// arbitrarily selected `wl_output`.
pub fn output_layout() -> Result<OutputLayout> {
    if !is_session() {
        bail!("not a Hyprland session");
    }
    let monitors: Vec<Monitor> = hyprctl_json("monitors")?;
    layout_from_monitors(&monitors).context("Hyprland reported no valid monitor layout")
}

/// Map one physical output capture to Hyprland's logical coordinate size.
/// Returns `None` when dimensions do not identify exactly one output, so callers
/// never guess on mirrored or otherwise ambiguous layouts.
pub fn logical_output_size_for_capture(width: u32, height: u32) -> Option<(u32, u32)> {
    if !is_session() {
        return None;
    }
    let monitors: Vec<Monitor> = hyprctl_json("monitors").ok()?;
    logical_output_size_from_monitors(&monitors, width, height)
}

/// Position the real Hyprland seat cursor in compositor-logical coordinates.
/// Hyprland's compositor dispatcher is authoritative across mixed-scale and
/// multi-monitor layouts; button and axis events still use the standard
/// wlroots virtual-pointer protocol.
pub fn move_cursor(x: i32, y: i32) -> Result<()> {
    if !is_session() {
        bail!("not a Hyprland session");
    }
    let binary = hyprctl_binary();
    let output = Command::new(binary)
        .args(["dispatch", "movecursor", &x.to_string(), &y.to_string()])
        .output()
        .context("launch hyprctl dispatch movecursor")?;
    if !output.status.success() || !output.stdout.starts_with(b"ok") {
        bail!("hyprctl dispatch movecursor failed");
    }
    Ok(())
}

fn monitor_physical_and_logical_size(monitor: &Monitor) -> Option<((u32, u32), (u32, u32))> {
    if monitor.width == 0
        || monitor.height == 0
        || !monitor.scale.is_finite()
        || monitor.scale <= 0.0
    {
        return None;
    }
    let physical = if monitor.transform % 2 == 0 {
        (monitor.width, monitor.height)
    } else {
        (monitor.height, monitor.width)
    };
    let logical = (
        (f64::from(physical.0) / monitor.scale).round() as u32,
        (f64::from(physical.1) / monitor.scale).round() as u32,
    );
    (logical.0 > 0 && logical.1 > 0).then_some((physical, logical))
}

fn logical_output_size_from_monitors(
    monitors: &[Monitor],
    width: u32,
    height: u32,
) -> Option<(u32, u32)> {
    let matches = monitors
        .iter()
        .filter_map(monitor_physical_and_logical_size)
        .filter_map(|(physical, logical)| (physical == (width, height)).then_some(logical))
        .collect::<Vec<_>>();
    (matches.len() == 1).then_some(matches[0])
}

fn layout_from_monitors(monitors: &[Monitor]) -> Option<OutputLayout> {
    let rectangles = monitors.iter().filter_map(|monitor| {
        if monitor.width == 0
            || monitor.height == 0
            || !monitor.scale.is_finite()
            || monitor.scale <= 0.0
        {
            return None;
        }
        let (_, (logical_width, logical_height)) = monitor_physical_and_logical_size(monitor)?;
        let width = i64::from(logical_width);
        let height = i64::from(logical_height);
        (width > 0 && height > 0).then_some((
            i64::from(monitor.x),
            i64::from(monitor.y),
            i64::from(monitor.x) + width,
            i64::from(monitor.y) + height,
        ))
    });

    let (min_x, min_y, max_x, max_y) =
        rectangles.fold(None, |bounds: Option<(i64, i64, i64, i64)>, rect| {
            Some(match bounds {
                None => rect,
                Some((min_x, min_y, max_x, max_y)) => (
                    min_x.min(rect.0),
                    min_y.min(rect.1),
                    max_x.max(rect.2),
                    max_y.max(rect.3),
                ),
            })
        })?;
    Some(OutputLayout {
        x: i32::try_from(min_x).ok()?,
        y: i32::try_from(min_y).ok()?,
        width: u32::try_from(max_x - min_x).ok()?,
        height: u32::try_from(max_y - min_y).ok()?,
    })
}

pub fn window_for_address(address: u64) -> Option<Window> {
    list_windows()
        .ok()?
        .into_iter()
        .find(|window| window.address == address)
}

/// Correlate an accessibility observation to one compositor client. PID is
/// mandatory; title and app-id disambiguate sibling windows, and a sole
/// PID-owned client is the final safe fallback.
pub fn matching_window<'a>(
    windows: &'a [Window],
    pid: u32,
    title: &str,
    app_id: &str,
) -> Option<&'a Window> {
    let owned = windows
        .iter()
        .filter(|window| window.pid == pid)
        .collect::<Vec<_>>();
    let title_matches = owned
        .iter()
        .copied()
        .filter(|window| !title.is_empty() && window.title == title)
        .collect::<Vec<_>>();
    if let [window] = title_matches.as_slice() {
        return Some(*window);
    }
    let app_matches = owned
        .iter()
        .copied()
        .filter(|window| !app_id.is_empty() && window.app_id == app_id)
        .collect::<Vec<_>>();
    if let [window] = app_matches.as_slice() {
        return Some(*window);
    }
    match owned.as_slice() {
        [window] => Some(*window),
        _ => None,
    }
}

pub fn window_for_pid(pid: u32) -> Option<Window> {
    let matching = list_windows()
        .ok()?
        .into_iter()
        .filter(|window| window.pid == pid)
        .collect::<Vec<_>>();
    match matching.as_slice() {
        [window] => Some(window.clone()),
        _ => None,
    }
}

pub fn window_for_title(title: &str) -> Option<Window> {
    let matching = list_windows()
        .ok()?
        .into_iter()
        .filter(|window| !title.is_empty() && window.title == title)
        .collect::<Vec<_>>();
    match matching.as_slice() {
        [window] => Some(window.clone()),
        _ => None,
    }
}

pub fn window_for_app_id(app_id: &str) -> Option<Window> {
    let matching = list_windows()
        .ok()?
        .into_iter()
        .filter(|window| !app_id.is_empty() && window.app_id == app_id)
        .collect::<Vec<_>>();
    match matching.as_slice() {
        [window] => Some(window.clone()),
        _ => None,
    }
}

/// Resolve one exact Hyprland compositor address for a Wayland observation.
/// Ambiguous title/app-id matches fail closed rather than selecting a sibling.
pub fn resolve_capture_address(
    window_id: u64,
    target_pid: Option<u32>,
    title: &str,
    app_id: &str,
) -> Result<u64> {
    if !is_session() {
        bail!("not a Hyprland session");
    }
    let target_pid = target_pid.context("Hyprland capture requires a verified target PID")?;
    resolve_from_clients(&clients()?, window_id, target_pid, title, app_id)
}

fn resolve_from_clients(
    clients: &[Client],
    window_id: u64,
    target_pid: u32,
    title: &str,
    app_id: &str,
) -> Result<u64> {
    let mut owned: Vec<(u64, &Client)> = clients
        .iter()
        .filter(|client| client.mapped && !client.hidden && client.pid == i64::from(target_pid))
        .filter_map(|client| parse_address(&client.address).map(|address| (address, client)))
        .collect();

    if let Some((address, _)) = owned.iter().find(|(address, _)| *address == window_id) {
        return Ok(*address);
    }

    owned.retain(|(_, client)| {
        let title_matches = !title.is_empty() && client.title == title;
        let app_matches = !app_id.is_empty() && client.class == app_id;
        if !title.is_empty() && !app_id.is_empty() {
            title_matches && app_matches
        } else {
            title_matches || app_matches
        }
    });

    match owned.as_slice() {
        [(address, _)] => Ok(*address),
        [] => bail!("no mapped Hyprland client owned by PID {target_pid} matched title/app-id"),
        matches => bail!(
            "Hyprland capture identity is ambiguous: {} PID-owned clients matched title/app-id",
            matches.len()
        ),
    }
}

fn clients() -> Result<Vec<Client>> {
    hyprctl_json("clients")
}

fn hyprctl_binary() -> &'static str {
    if std::path::Path::new("/usr/bin/hyprctl").is_file() {
        "/usr/bin/hyprctl"
    } else {
        "hyprctl"
    }
}

fn hyprctl_json<T: DeserializeOwned>(query: &str) -> Result<T> {
    let output = Command::new(hyprctl_binary())
        .args(["-j", query])
        .output()
        .with_context(|| format!("launch hyprctl -j {query}"))?;
    if !output.status.success() || output.stdout.is_empty() {
        bail!("hyprctl -j {query} failed");
    }
    serde_json::from_slice(&output.stdout).with_context(|| format!("parse hyprctl {query} JSON"))
}

fn parse_address(address: &str) -> Option<u64> {
    u64::from_str_radix(address.trim_start_matches("0x"), 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(address: &str, pid: i64, title: &str, class: &str) -> Client {
        Client {
            address: address.to_owned(),
            mapped: true,
            hidden: false,
            pid,
            title: title.to_owned(),
            class: class.to_owned(),
            at: [10, 20],
            size: [800, 600],
            workspace: Workspace { id: 1 },
        }
    }

    fn monitor(x: i32, y: i32, width: u32, height: u32, scale: f64) -> Monitor {
        Monitor {
            x,
            y,
            width,
            height,
            scale,
            ..Monitor::default()
        }
    }

    #[test]
    fn parses_full_hyprland_pointer_address() {
        assert_eq!(parse_address("0x55b5cd9af330"), Some(0x55b5cd9af330));
        assert_eq!(parse_address("invalid"), None);
    }

    #[test]
    fn identity_requires_pid_title_and_app_id() {
        let clients = [
            client("0x1111", 42, "Target", "fixture"),
            client("0x2222", 43, "Target", "fixture"),
            client("0x3333", 42, "Other", "fixture"),
        ];
        assert_eq!(
            resolve_from_clients(&clients, 0xff00, 42, "Target", "fixture").unwrap(),
            0x1111
        );
    }

    #[test]
    fn ambiguous_pid_owned_siblings_fail_closed() {
        let clients = [
            client("0x1111", 42, "Target", "fixture"),
            client("0x2222", 42, "Target", "fixture"),
        ];
        let error = resolve_from_clients(&clients, 0xff00, 42, "Target", "fixture")
            .expect_err("duplicate identities must not be guessed");
        assert!(error.to_string().contains("ambiguous"));
    }

    #[test]
    fn exact_hyprland_address_wins_within_verified_pid() {
        let clients = [
            client("0x1111", 42, "First", "fixture"),
            client("0x2222", 42, "Second", "fixture"),
        ];
        assert_eq!(
            resolve_from_clients(&clients, 0x2222, 42, "stale", "stale").unwrap(),
            0x2222
        );
    }

    #[test]
    fn compositor_metadata_preserves_geometry_and_workspace_visibility() {
        let mut visible = client("0x1111", 42, "Target", "fixture");
        visible.at = [-20, 30];
        visible.size = [940, 780];
        visible.workspace.id = 7;
        let mut hidden = client("0x2222", 43, "Hidden", "fixture");
        hidden.workspace.id = 8;

        let windows = windows_from_clients(&[visible, hidden], &HashSet::from([7]));
        assert_eq!(
            windows[0],
            Window {
                address: 0x1111,
                pid: 42,
                title: "Target".to_owned(),
                app_id: "fixture".to_owned(),
                x: -20,
                y: 30,
                width: 940,
                height: 780,
                workspace: 7,
                visible: true,
            }
        );
        assert!(!windows[1].visible);
    }

    #[test]
    fn accessibility_matching_uses_pid_then_unique_title() {
        let windows = windows_from_clients(
            &[
                client("0x1111", 42, "Main", "fixture"),
                client("0x2222", 42, "Child", "fixture"),
                client("0x3333", 43, "Main", "fixture"),
            ],
            &HashSet::from([1]),
        );
        assert_eq!(
            matching_window(&windows, 42, "Main", "fixture").map(|window| window.address),
            Some(0x1111)
        );
        assert!(matching_window(&windows, 42, "Unknown", "fixture").is_none());
        assert!(matching_window(&windows, 44, "Main", "fixture").is_none());
    }

    #[test]
    fn malformed_process_or_size_metadata_fails_closed() {
        let mut invalid_pid = client("0x1111", -1, "Bad", "fixture");
        invalid_pid.size = [800, 600];
        let mut invalid_size = client("0x2222", 42, "Target", "fixture");
        invalid_size.size = [-1, 600];

        let windows = windows_from_clients(&[invalid_pid, invalid_size], &HashSet::from([1]));
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].width, 0);
    }

    #[test]
    fn output_layout_uses_logical_scaled_bounds() {
        let monitors = [
            monitor(384, 288, 1920, 1080, 1.25),
            monitor(1920, 0, 3840, 2160, 1.5),
        ];
        assert_eq!(
            layout_from_monitors(&monitors),
            Some(OutputLayout {
                x: 384,
                y: 0,
                width: 4096,
                height: 1440,
            })
        );
    }

    #[test]
    fn output_layout_translates_negative_and_rotated_outputs() {
        let mut rotated = monitor(-1080, -200, 1920, 1080, 1.0);
        rotated.transform = 1;
        assert_eq!(
            layout_from_monitors(&[rotated, monitor(0, 0, 2560, 1440, 1.0)]),
            Some(OutputLayout {
                x: -1080,
                y: -200,
                width: 3640,
                height: 1920,
            })
        );
    }

    #[test]
    fn physical_capture_size_maps_to_one_logical_output() {
        let monitors = [
            monitor(384, 288, 1920, 1080, 1.25),
            monitor(1920, 0, 3840, 2160, 1.5),
        ];
        assert_eq!(
            logical_output_size_from_monitors(&monitors, 1920, 1080),
            Some((1536, 864))
        );
        assert_eq!(
            logical_output_size_from_monitors(&monitors, 3840, 2160),
            Some((2560, 1440))
        );
        assert_eq!(
            logical_output_size_from_monitors(
                &[
                    monitor(0, 0, 1920, 1080, 1.0),
                    monitor(1920, 0, 1920, 1080, 1.0)
                ],
                1920,
                1080,
            ),
            None
        );
    }
}

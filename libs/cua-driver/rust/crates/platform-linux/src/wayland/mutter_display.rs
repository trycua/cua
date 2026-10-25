//! Content-free display metadata from GNOME's compositor (Mutter), for `get_screen_size` on a
//! GNOME Wayland session that has no X11 `DISPLAY`.
//!
//! Consent scopes whole-desktop and unfiltered listing calls to the display geometry, read
//! through `get_screen_size`. Without a native source here that read opens X11 and fails, so
//! every such call is refused on GNOME Wayland (trycua/cua#1922). Mutter publishes its monitor
//! layout on `org.gnome.Mutter.DisplayConfig`; this reads that layout and nothing else.
//!
//! As with Hyprland's IPC, the answer must come from the compositor this process draws on: the
//! bus name's owner must be the peer of the `WAYLAND_DISPLAY` socket, or the report is refused.

use std::collections::HashMap;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use zbus::zvariant::OwnedValue;

const DEST: &str = "org.gnome.Mutter.DisplayConfig";
const PATH: &str = "/org/gnome/Mutter/DisplayConfig";
const CALL_TIMEOUT: Duration = Duration::from_secs(2);

type Props = HashMap<String, OwnedValue>;
type Spec = (String, String, String, String);
type Mode = (String, i32, i32, f64, f64, Vec<f64>, Props);
type Monitor = (Spec, Vec<Mode>, Props);
type Logical = (i32, i32, f64, u32, bool, Vec<Spec>, Props);
type State = (u32, Vec<Monitor>, Vec<Logical>, Props);

/// One logical monitor as Mutter reports it, reduced to what the frame needs.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LogicalMonitor {
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub scale: f64,
    /// Width and height of the monitor's current mode, in physical pixels, before rotation.
    pub mode: (i32, i32),
    /// `wl_output` transform: odd values (90°, 270°, and their flipped forms) swap the axes.
    pub transform: u32,
}

/// The desktop frame (bounding box of every logical monitor, in layout coordinates), the largest
/// scale, and a per-monitor report in the shape `get_screen_size` already uses for Hyprland.
pub(crate) fn frame_from_layout(
    logical_layout: bool,
    monitors: &[LogicalMonitor],
) -> Result<(u32, u32, f64, serde_json::Value)> {
    if monitors.is_empty() {
        bail!("GNOME reports no active monitor");
    }
    let mut boxes = Vec::with_capacity(monitors.len());
    for m in monitors {
        if m.mode.0 <= 0 || m.mode.1 <= 0 || !m.scale.is_finite() || m.scale <= 0.0 {
            bail!("GNOME reports an unusable mode for monitor {}", m.name);
        }
        let (w, h) = if m.transform % 2 == 1 {
            (m.mode.1, m.mode.0)
        } else {
            m.mode
        };
        let (w, h) = if logical_layout {
            (
                (f64::from(w) / m.scale).round() as i64,
                (f64::from(h) / m.scale).round() as i64,
            )
        } else {
            (i64::from(w), i64::from(h))
        };
        boxes.push((m, i64::from(m.x), i64::from(m.y), w, h));
    }
    let min_x = boxes.iter().map(|b| b.1).min().unwrap_or(0);
    let min_y = boxes.iter().map(|b| b.2).min().unwrap_or(0);
    let max_x = boxes.iter().map(|b| b.1 + b.3).max().unwrap_or(0);
    let max_y = boxes.iter().map(|b| b.2 + b.4).max().unwrap_or(0);
    let width = u32::try_from(max_x - min_x).context("desktop frame width")?;
    let height = u32::try_from(max_y - min_y).context("desktop frame height")?;
    if width == 0 || height == 0 {
        bail!("GNOME reports an empty desktop frame");
    }
    let scale = monitors.iter().map(|m| m.scale).fold(0.0_f64, f64::max);
    let report = serde_json::Value::Array(
        boxes
            .iter()
            .map(|(m, x, y, w, h)| {
                serde_json::json!({
                    "name": m.name,
                    "width": w,
                    "height": h,
                    "scale": m.scale,
                    "powered": true,
                    "frame_x": x - min_x,
                    "frame_y": y - min_y,
                })
            })
            .collect(),
    );
    Ok((width, height, scale, report))
}

fn logical_monitors(state: &State) -> Result<(bool, Vec<LogicalMonitor>)> {
    let (_, monitors, logical, props) = state;
    // layout-mode 1 is logical (positions and sizes in scaled pixels), 2 physical. Absent means
    // the compositor does not support choosing, which Mutter documents as logical.
    let layout_logical = match props.get("layout-mode") {
        None => true,
        Some(value) => u32::try_from(value).map_err(|_| anyhow!("unreadable layout-mode"))? != 2,
    };
    let current = |connector: &str| {
        monitors
            .iter()
            .find(|(spec, _, _)| spec.0 == connector)
            .and_then(|(_, modes, _)| {
                modes.iter().find(|mode| {
                    mode.6
                        .get("is-current")
                        .and_then(|v| bool::try_from(v).ok())
                        .unwrap_or(false)
                })
            })
            .map(|mode| (mode.1, mode.2))
    };
    let mut out = Vec::with_capacity(logical.len());
    for (x, y, scale, transform, _primary, specs, _) in logical {
        let spec = specs
            .first()
            .context("a logical monitor without a monitor")?;
        let mode =
            current(&spec.0).with_context(|| format!("monitor {} has no current mode", spec.0))?;
        out.push(LogicalMonitor {
            name: spec.0.clone(),
            x: *x,
            y: *y,
            scale: *scale,
            mode,
            transform: *transform,
        });
    }
    Ok((layout_logical, out))
}

fn wayland_socket() -> Result<PathBuf> {
    let display = std::env::var_os("WAYLAND_DISPLAY").context("missing WAYLAND_DISPLAY")?;
    let display = PathBuf::from(display);
    if display.is_absolute() {
        return Ok(display);
    }
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").context("missing XDG_RUNTIME_DIR")?;
    let runtime = PathBuf::from(runtime);
    if !runtime.is_absolute() {
        bail!("XDG_RUNTIME_DIR must be absolute");
    }
    Ok(runtime.join(display))
}

/// The desktop frame of the GNOME session this process draws on, attested to its compositor.
pub fn screen_report() -> Result<(u32, u32, f64, serde_json::Value)> {
    let socket = UnixStream::connect(wayland_socket()?).context("connect to the Wayland socket")?;
    let compositor = super::hyprland::peer_pid(socket.as_raw_fd())?;
    // Own thread and runtime: callers may already be inside Tokio (see kwin_helper).
    let (owner, state) = std::thread::spawn(|| -> Result<(u32, State)> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        runtime.block_on(async {
            let connection = tokio::time::timeout(CALL_TIMEOUT, zbus::Connection::session())
                .await
                .context("session bus timed out")??;
            let dbus = zbus::fdo::DBusProxy::new(&connection).await?;
            let name = zbus::names::BusName::try_from(DEST)?;
            let owner =
                tokio::time::timeout(CALL_TIMEOUT, dbus.get_connection_unix_process_id(name))
                    .await
                    .context("DisplayConfig owner lookup timed out")??;
            let proxy = zbus::Proxy::new(&connection, DEST, PATH, DEST).await?;
            let state: State =
                tokio::time::timeout(CALL_TIMEOUT, proxy.call("GetCurrentState", &()))
                    .await
                    .context("GetCurrentState timed out")??;
            Ok((owner, state))
        })
    })
    .join()
    .map_err(|_| anyhow!("GNOME display query thread panicked"))??;
    if i64::from(owner) != i64::from(compositor) {
        bail!("org.gnome.Mutter.DisplayConfig is not served by this session's compositor");
    }
    let (layout_logical, monitors) = logical_monitors(&state)?;
    frame_from_layout(layout_logical, &monitors)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(
        name: &str,
        x: i32,
        y: i32,
        scale: f64,
        mode: (i32, i32),
        transform: u32,
    ) -> LogicalMonitor {
        LogicalMonitor {
            name: name.into(),
            x,
            y,
            scale,
            mode,
            transform,
        }
    }

    #[test]
    fn one_scaled_monitor_reports_its_logical_size_and_scale() {
        let (w, h, scale, report) =
            frame_from_layout(true, &[monitor("eDP-1", 0, 0, 2.0, (2880, 1800), 0)]).unwrap();
        assert_eq!((w, h, scale), (1440, 900, 2.0));
        assert_eq!(report[0]["frame_x"], 0);
        assert_eq!(report[0]["width"], 1440);
    }

    #[test]
    fn two_monitors_span_one_frame_with_offsets_from_its_corner() {
        let (w, h, scale, report) = frame_from_layout(
            true,
            &[
                monitor("DP-1", -1920, 0, 1.0, (1920, 1080), 0),
                monitor("eDP-1", 0, 200, 2.0, (2880, 1800), 0),
            ],
        )
        .unwrap();
        assert_eq!((w, h, scale), (3360, 1100, 2.0));
        assert_eq!(
            (report[1]["frame_x"].as_i64(), report[1]["frame_y"].as_i64()),
            (Some(1920), Some(200))
        );
    }

    #[test]
    fn a_rotated_monitor_swaps_its_axes() {
        let (w, h, _, _) =
            frame_from_layout(true, &[monitor("DP-1", 0, 0, 1.0, (1920, 1080), 1)]).unwrap();
        assert_eq!((w, h), (1080, 1920));
    }

    #[test]
    fn physical_layout_keeps_mode_pixels() {
        let (w, h, _, _) =
            frame_from_layout(false, &[monitor("eDP-1", 0, 0, 2.0, (2880, 1800), 0)]).unwrap();
        assert_eq!((w, h), (2880, 1800));
    }

    #[test]
    fn no_monitor_or_an_unusable_mode_is_refused() {
        assert!(frame_from_layout(true, &[]).is_err());
        assert!(frame_from_layout(true, &[monitor("X", 0, 0, 0.0, (10, 10), 0)]).is_err());
        assert!(frame_from_layout(true, &[monitor("X", 0, 0, 1.0, (0, 10), 0)]).is_err());
    }
}

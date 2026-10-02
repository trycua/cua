//! Linux [`PointerShapeBackend`]: what cursor the guest would show at a
//! point, for multiplayer presence. Tables and per-session support live in
//! [`crate::pointer_shape_map`].
//!
//! - X11: top-level window under the point from `_NET_CLIENT_LIST_STACKING`,
//!   resize bands on the WM frame, AT-SPI element chain in screen
//!   coordinates, XFixes cursor name for the real cursor, XTest motion for
//!   the probe warp.
//! - Hyprland: client frames from `hyprctl clients`, AT-SPI in window
//!   coordinates, `cursorpos` and `movecursor` for the pointer, no real
//!   cursor readout (so no probe).
//! - Other Wayland compositors: nothing (see the limitation text).

use std::sync::Mutex;
use std::time::{Duration, Instant};

use cua_driver_core::cursor_shape::SystemCursorShape;
use cua_driver_core::pointer_shape::{
    edge_resize_shape, BackendNames, HitTest, PointerShapeBackend, ScreenRect,
};
use x11rb::connection::Connection;
use x11rb::protocol::xfixes::ConnectionExt as _;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, MapState, Window};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

use crate::atspi::native::hit::{chain_at_point, HitCoords};
use crate::pointer_shape_map::{shape_for_atspi_chain, shape_for_xcursor_name};

/// AT-SPI descent budget per hit-test.
const HIT_BUDGET: Duration = Duration::from_millis(80);
/// Cached leaf hits are trusted this long.
const CACHE_TTL: Duration = Duration::from_millis(1000);
const CACHE_MAX: usize = 64;
/// Resize band for windows without WM borders (client-side decorations).
const MIN_BAND: f64 = 4.0;
/// Corner zone along each edge.
const CORNER: f64 = 16.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Session {
    X11,
    Hyprland,
    OtherWayland,
}

fn session() -> Session {
    if crate::wayland::is_wayland() {
        if crate::wayland::hyprland::is_session() {
            Session::Hyprland
        } else {
            Session::OtherWayland
        }
    } else {
        Session::X11
    }
}

/// A top-level window under a point.
#[derive(Debug, Clone)]
struct TopLevel {
    id: u64,
    pid: Option<u32>,
    title: String,
    /// Client area.
    client: ScreenRect,
    /// Client area plus WM decorations.
    frame: ScreenRect,
    /// Resize band width inside `frame`.
    band: f64,
    /// Whether edges resize (normal, not maximized or fullscreen).
    resizable: bool,
}

struct CacheEntry {
    window: u64,
    element: ScreenRect,
    hit: HitTest,
    at: Instant,
}

pub struct LinuxPointerShape {
    session: Session,
    x11: Mutex<Option<(RustConnection, usize)>>,
    cache: Mutex<Vec<CacheEntry>>,
}

impl LinuxPointerShape {
    fn new(session: Session) -> Self {
        Self {
            session,
            x11: Mutex::new(None),
            cache: Mutex::new(Vec::new()),
        }
    }

    /// Run `f` on the backend's own X11 connection, reconnecting once when
    /// the server went away.
    fn with_x11<T>(&self, f: impl Fn(&RustConnection, Window) -> Option<T>) -> Option<T> {
        let mut guard = self.x11.lock().unwrap_or_else(|e| e.into_inner());
        for _ in 0..2 {
            if guard.is_none() {
                let (conn, screen) = RustConnection::connect(None).ok()?;
                // XFixes requires the version handshake before any request.
                let _ = conn.xfixes_query_version(5, 0).ok()?.reply();
                *guard = Some((conn, screen));
            }
            let (conn, screen) = guard.as_ref()?;
            let root = conn.setup().roots.get(*screen)?.root;
            if let Some(v) = f(conn, root) {
                return Some(v);
            }
            // A dead connection fails every request; retry on a fresh one.
            if conn
                .get_input_focus()
                .ok()
                .and_then(|c| c.reply().ok())
                .is_none()
            {
                *guard = None;
                continue;
            }
            return None;
        }
        None
    }

    fn x11_top_level_at(&self, x: f64, y: f64) -> Option<TopLevel> {
        self.with_x11(|conn, root| {
            let windows = crate::x11::get_window_list(conn, root).ok()?;
            let atom = |n: &str| crate::x11::get_atom(conn, n).ok();
            let frame_extents = atom("_NET_FRAME_EXTENTS");
            let wm_state = atom("_NET_WM_STATE");
            let max_v = atom("_NET_WM_STATE_MAXIMIZED_VERT");
            let max_h = atom("_NET_WM_STATE_MAXIMIZED_HORZ");
            let fullscreen = atom("_NET_WM_STATE_FULLSCREEN");
            let hidden = atom("_NET_WM_STATE_HIDDEN");
            let wtype = atom("_NET_WM_WINDOW_TYPE");
            let normal = atom("_NET_WM_WINDOW_TYPE_NORMAL");
            let dialog = atom("_NET_WM_WINDOW_TYPE_DIALOG");
            // Stacking order is bottom to top.
            for &xid in windows.iter().rev() {
                let attrs = conn.get_window_attributes(xid).ok()?.reply().ok();
                if attrs.map(|a| a.map_state) != Some(MapState::VIEWABLE) {
                    continue;
                }
                let Some(geom) = conn.get_geometry(xid).ok().and_then(|c| c.reply().ok()) else {
                    continue;
                };
                let Some(origin) = conn
                    .translate_coordinates(xid, root, 0, 0)
                    .ok()
                    .and_then(|c| c.reply().ok())
                else {
                    continue;
                };
                let client = ScreenRect::new(
                    f64::from(origin.dst_x),
                    f64::from(origin.dst_y),
                    f64::from(geom.width),
                    f64::from(geom.height),
                );
                let cardinals = |prop: Option<u32>, ty: AtomEnum| -> Vec<u32> {
                    prop.and_then(|p| conn.get_property(false, xid, p, ty, 0, 64).ok())
                        .and_then(|c| c.reply().ok())
                        .and_then(|r| r.value32().map(|v| v.collect()))
                        .unwrap_or_default()
                };
                let ext = cardinals(frame_extents, AtomEnum::CARDINAL);
                let (l, r, t, b) = match ext.as_slice() {
                    [l, r, t, b, ..] => {
                        (f64::from(*l), f64::from(*r), f64::from(*t), f64::from(*b))
                    }
                    _ => (0.0, 0.0, 0.0, 0.0),
                };
                let frame = ScreenRect::new(
                    client.x - l,
                    client.y - t,
                    client.width + l + r,
                    client.height + t + b,
                );
                // The resize band reaches a few pixels outside the frame.
                if !frame
                    .inflate(if l + r + b == 0.0 { MIN_BAND } else { 0.0 })
                    .contains(x, y)
                {
                    continue;
                }
                let states = cardinals(wm_state, AtomEnum::ATOM);
                if hidden.is_some_and(|h| states.contains(&h)) {
                    continue;
                }
                let types = cardinals(wtype, AtomEnum::ATOM);
                let is_normal = types.is_empty()
                    || types
                        .iter()
                        .any(|t| Some(*t) == normal || Some(*t) == dialog);
                let maximized = (max_v.is_some_and(|m| states.contains(&m))
                    && max_h.is_some_and(|m| states.contains(&m)))
                    || fullscreen.is_some_and(|f| states.contains(&f));
                let border = [l, r, b].into_iter().fold(f64::INFINITY, f64::min);
                let band = if border.is_finite() && border > 0.0 {
                    border.max(MIN_BAND)
                } else {
                    MIN_BAND
                };
                return Some(TopLevel {
                    id: u64::from(xid),
                    pid: crate::x11::get_window_pid(conn, xid).ok().flatten(),
                    title: crate::x11::get_window_title(conn, xid).unwrap_or_default(),
                    client,
                    frame,
                    band,
                    resizable: is_normal && !maximized,
                });
            }
            // Nothing under the point: the root (desktop without a window).
            None
        })
    }

    fn hyprland_top_level_at(&self, x: f64, y: f64) -> Option<TopLevel> {
        let mut windows = crate::wayland::hyprland::list_windows().ok()?;
        windows.retain(|w| w.visible);
        windows.sort_by_key(|w| w.focus_order);
        windows.into_iter().find_map(|w| {
            let client = ScreenRect::new(
                f64::from(w.x),
                f64::from(w.y),
                f64::from(w.width),
                f64::from(w.height),
            );
            client.inflate(MIN_BAND).contains(x, y).then_some(TopLevel {
                id: w.address,
                pid: Some(w.pid),
                title: w.title,
                client,
                frame: client,
                band: MIN_BAND,
                resizable: true,
            })
        })
    }

    fn cached(&self, window: u64, x: f64, y: f64) -> Option<HitTest> {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.retain(|e| e.at.elapsed() < CACHE_TTL);
        cache
            .iter()
            .find(|e| e.window == window && e.element.contains(x, y))
            .map(|e| e.hit.clone())
    }

    fn remember(&self, window: u64, element: ScreenRect, hit: &HitTest) {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if cache.len() >= CACHE_MAX {
            cache.remove(0);
        }
        cache.push(CacheEntry {
            window,
            element,
            hit: hit.clone(),
            at: Instant::now(),
        });
    }

    fn hit_in(&self, top: &TopLevel, x: f64, y: f64) -> Option<HitTest> {
        if top.resizable {
            let outside = if top.frame == top.client {
                MIN_BAND
            } else {
                0.0
            };
            if let Some(shape) = edge_resize_shape(top.frame, x, y, top.band, outside, CORNER) {
                return Some(HitTest {
                    shape,
                    role: "edge".into(),
                    window: Some(top.frame),
                    element: None,
                });
            }
        }
        if !top.client.contains(x, y) {
            // Title bar or other WM decoration.
            return Some(HitTest {
                shape: SystemCursorShape::Default,
                role: "decoration".into(),
                window: Some(top.frame),
                element: None,
            });
        }
        if let Some(hit) = self.cached(top.id, x, y) {
            return Some(hit);
        }
        let pid = top.pid?;
        let (coords, px, py, dx, dy) = match self.session {
            Session::X11 => (HitCoords::Screen, x, y, 0.0, 0.0),
            _ => (
                HitCoords::Window,
                x - top.client.x,
                y - top.client.y,
                top.client.x,
                top.client.y,
            ),
        };
        let chain = chain_at_point(
            pid,
            px.round() as i32,
            py.round() as i32,
            coords,
            Some(top.title.as_str()),
            HIT_BUDGET,
        )
        .ok()??;
        let samples: Vec<_> = chain.iter().map(|n| n.sample.clone()).collect();
        let (shape, decided) = shape_for_atspi_chain(&samples);
        let rect = |i: usize| {
            chain[i].extents.map(|(ex, ey, w, h)| {
                ScreenRect::new(
                    f64::from(ex) + dx,
                    f64::from(ey) + dy,
                    f64::from(w),
                    f64::from(h),
                )
            })
        };
        let deciding = decided.unwrap_or(chain.len() - 1);
        let hit = HitTest {
            shape,
            role: samples[deciding].role.clone(),
            window: Some(top.frame),
            element: rect(deciding),
        };
        // Cache by the deepest node's extents: nothing under the point was
        // deeper, while a larger ancestor may hold differently shaped
        // children elsewhere.
        if let Some(leaf) = rect(chain.len() - 1).filter(|r| r.area() > 0.0) {
            self.remember(top.id, leaf, &hit);
        }
        Some(hit)
    }

    fn xfixes_shape(&self) -> SystemCursorShape {
        self.with_x11(|conn, _| {
            let reply = conn.xfixes_get_cursor_image_and_name().ok()?.reply().ok()?;
            Some(shape_for_xcursor_name(&String::from_utf8_lossy(
                &reply.name,
            )))
        })
        .unwrap_or(SystemCursorShape::Unknown)
    }
}

impl PointerShapeBackend for LinuxPointerShape {
    fn names(&self) -> BackendNames {
        match self.session {
            Session::X11 => BackendNames {
                hit_test: "atspi",
                system: "xfixes",
                probe: "xtest",
            },
            Session::Hyprland => BackendNames {
                hit_test: "atspi",
                system: "",
                probe: "",
            },
            Session::OtherWayland => BackendNames::default(),
        }
    }

    fn hit_test(&self, x: f64, y: f64) -> Option<HitTest> {
        let top = match self.session {
            Session::X11 => self.x11_top_level_at(x, y),
            Session::Hyprland => self.hyprland_top_level_at(x, y),
            Session::OtherWayland => return None,
        };
        match top {
            Some(top) => self.hit_in(&top, x, y),
            None => Some(HitTest::nothing()),
        }
    }

    fn system_shape(&self) -> SystemCursorShape {
        match self.session {
            Session::X11 => self.xfixes_shape(),
            _ => SystemCursorShape::Unknown,
        }
    }

    fn pointer_position(&self) -> Option<(f64, f64)> {
        match self.session {
            Session::X11 => self.with_x11(|conn, root| {
                let p = conn.query_pointer(root).ok()?.reply().ok()?;
                Some((f64::from(p.root_x), f64::from(p.root_y)))
            }),
            Session::Hyprland => crate::wayland::hyprland::cursor_position().ok(),
            Session::OtherWayland => None,
        }
    }

    fn warp_pointer(&self, x: f64, y: f64) -> bool {
        match self.session {
            Session::X11 => self
                .with_x11(|conn, root| {
                    const MOTION_NOTIFY: u8 = 6;
                    conn.xtest_fake_input(
                        MOTION_NOTIFY,
                        0,
                        0,
                        root,
                        x.round() as i16,
                        y.round() as i16,
                        0,
                    )
                    .ok()?;
                    conn.flush().ok()?;
                    // Round trip so the move is applied before the caller reads.
                    conn.get_input_focus().ok()?.reply().ok()?;
                    Some(())
                })
                .is_some(),
            Session::Hyprland => crate::wayland::hyprland::move_cursor(x, y).is_ok(),
            Session::OtherWayland => false,
        }
    }

    fn limitation(&self) -> Option<String> {
        match self.session {
            Session::X11 => None,
            Session::Hyprland => Some(
                "Wayland does not expose the compositor's cursor shape to clients; shapes come from the accessibility hit-test only.".into(),
            ),
            Session::OtherWayland => Some(
                "This Wayland compositor exposes neither window geometry nor the cursor shape to clients; every cursor is drawn as the arrow.".into(),
            ),
        }
    }
}

/// Install the backend for the current session (and the XFixes real-cursor
/// probe on X11). False when no display is reachable or one is installed.
pub fn install() -> bool {
    let session = session();
    if session == Session::X11 {
        let Ok((conn, _)) = RustConnection::connect(None) else {
            return false;
        };
        drop(conn);
        cua_driver_core::cursor_shape::set_cursor_shape_probe(|| {
            match cua_driver_core::pointer_shape::pointer_shape_backend() {
                Some(b) => b.system_shape(),
                None => SystemCursorShape::Unknown,
            }
        });
    }
    cua_driver_core::pointer_shape::set_pointer_shape_backend(LinuxPointerShape::new(session))
}

#[cfg(test)]
mod tests {
    //! Live checks against a real X server with an AT-SPI session. Opt-in:
    //! they move the X pointer of `$DISPLAY`, so they run only inside a
    //! throwaway container display (`CUA_POINTER_SHAPE_LIVE=1`), never on a
    //! developer desktop. Fixture layout comes from env vars the harness
    //! sets: `CUA_PS_ENTRY`, `CUA_PS_BUTTON`, `CUA_PS_TERMINAL` as `x,y`.
    use super::*;

    fn live() -> bool {
        std::env::var("CUA_POINTER_SHAPE_LIVE").as_deref() == Ok("1")
    }

    fn point(var: &str) -> (f64, f64) {
        let v = std::env::var(var).unwrap_or_else(|_| panic!("{var} unset"));
        let (x, y) = v.split_once(',').expect("x,y");
        (x.trim().parse().unwrap(), y.trim().parse().unwrap())
    }

    #[test]
    #[ignore = "needs a throwaway X display with fixtures; set CUA_POINTER_SHAPE_LIVE=1"]
    fn live_x11_hit_test_system_shape_and_probe() {
        if !live() {
            return;
        }
        let backend = LinuxPointerShape::new(Session::X11);
        assert_eq!(backend.names().system, "xfixes");
        let (ex, ey) = point("CUA_PS_ENTRY");
        let (bx, by) = point("CUA_PS_BUTTON");
        let (tx, ty) = point("CUA_PS_TERMINAL");

        let started = Instant::now();
        let entry = backend.hit_test(ex, ey).expect("hit-test over the entry");
        let first_ms = started.elapsed().as_millis();
        eprintln!("entry: {entry:?} ({first_ms} ms)");
        assert_eq!(entry.shape, SystemCursorShape::Text);
        let started = Instant::now();
        let again = backend.hit_test(ex, ey).unwrap();
        eprintln!(
            "entry again: {:?} ({} us)",
            again.shape,
            started.elapsed().as_micros()
        );
        eprintln!("points entry=({ex},{ey}) button=({bx},{by}) terminal=({tx},{ty})");
        let button = backend.hit_test(bx, by).expect("hit-test over the button");
        eprintln!("button: {button:?}");
        assert_eq!(button.shape, SystemCursorShape::Pointer);
        let term = backend
            .hit_test(tx, ty)
            .expect("hit-test over the terminal");
        eprintln!("terminal: {term:?}");
        assert_eq!(term.shape, SystemCursorShape::Text);
        if let Some(frame) = entry.window {
            let edge = backend.hit_test(frame.x + frame.width - 1.0, frame.y + frame.height / 2.0);
            eprintln!("right edge of the entry's window: {edge:?}");
        }

        // The real cursor through the probe, with an exact restore.
        let home = (7.0, 7.0);
        assert!(backend.warp_pointer(home.0, home.1));
        assert_eq!(backend.pointer_position(), Some(home));
        let out = cua_driver_core::pointer_shape::probe_by_warp(
            &backend,
            (tx, ty),
            cua_driver_core::pointer_shape::ProbeConfig {
                dwell: Duration::from_millis(80),
                ..Default::default()
            },
            &|| true,
        );
        eprintln!("probe over the terminal: {out:?}");
        assert_eq!(
            out,
            cua_driver_core::pointer_shape::ProbeOutcome::Shape(SystemCursorShape::Text)
        );
        assert_eq!(backend.pointer_position(), Some(home), "restored exactly");
        let out = cua_driver_core::pointer_shape::probe_by_warp(
            &backend,
            (bx, by),
            cua_driver_core::pointer_shape::ProbeConfig {
                dwell: Duration::from_millis(80),
                ..Default::default()
            },
            &|| true,
        );
        eprintln!("probe over the button: {out:?}");
        assert_eq!(backend.pointer_position(), Some(home), "restored exactly");
        if let Ok(xterm) = std::env::var("CUA_PS_XTERM") {
            let (xx, xy) = xterm
                .split_once(',')
                .map(|(a, b)| (a.parse().unwrap(), b.parse().unwrap()))
                .unwrap();
            let started = Instant::now();
            let out = cua_driver_core::pointer_shape::probe_by_warp(
                &backend,
                (xx, xy),
                cua_driver_core::pointer_shape::ProbeConfig::default(),
                &|| true,
            );
            eprintln!(
                "probe over xterm (no AT-SPI): {out:?} in {} ms",
                started.elapsed().as_millis()
            );
            assert_eq!(
                out,
                cua_driver_core::pointer_shape::ProbeOutcome::Shape(SystemCursorShape::Text)
            );
            assert_eq!(backend.pointer_position(), Some(home), "restored exactly");
            eprintln!(
                "xterm hit-test (no AT-SPI app): {:?}",
                backend.hit_test(xx, xy)
            );
        }
    }
}

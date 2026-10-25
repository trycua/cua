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

/// The Wayland socket this process would connect to. An inherited `WAYLAND_SOCKET` takes
/// precedence over `WAYLAND_DISPLAY` for the real connection and cannot be attested here, so it is
/// refused, as in the Hyprland adapter.
fn wayland_socket(env: impl Fn(&str) -> Option<std::ffi::OsString>) -> Result<PathBuf> {
    if env("WAYLAND_SOCKET").is_some() {
        bail!("an inherited WAYLAND_SOCKET cannot be attested");
    }
    let display = PathBuf::from(env("WAYLAND_DISPLAY").context("missing WAYLAND_DISPLAY")?);
    if display.is_absolute() {
        return Ok(display);
    }
    let runtime = PathBuf::from(env("XDG_RUNTIME_DIR").context("missing XDG_RUNTIME_DIR")?);
    if !runtime.is_absolute() {
        bail!("XDG_RUNTIME_DIR must be absolute");
    }
    Ok(runtime.join(display))
}

/// A non-blocking connect to a Unix socket, bounded by `timeout` on this thread: no worker is left
/// waiting on a socket that never accepts. A full listen backlog fails at once (EAGAIN).
fn connect_bounded(path: &std::path::Path, timeout: Duration) -> Result<std::os::fd::OwnedFd> {
    use std::os::fd::{FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    let bytes = path.as_os_str().as_bytes();
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if bytes.len() >= addr.sun_path.len() {
        bail!("the Wayland socket path is too long");
    }
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (dst, src) in addr.sun_path.iter_mut().zip(bytes) {
        *dst = *src as libc::c_char;
    }
    // SAFETY: plain socket(2); the descriptor is owned immediately below.
    let raw = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            0,
        )
    };
    if raw < 0 {
        return Err(std::io::Error::last_os_error()).context("socket");
    }
    // SAFETY: `raw` is a fresh descriptor nothing else owns.
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    let len = (std::mem::size_of::<libc::sa_family_t>() + bytes.len()) as libc::socklen_t;
    // SAFETY: `addr` is a valid sockaddr_un of at least `len` bytes.
    let rc = unsafe {
        libc::connect(
            fd.as_raw_fd(),
            (&addr as *const libc::sockaddr_un).cast(),
            len,
        )
    };
    if rc == 0 {
        return Ok(fd);
    }
    let err = std::io::Error::last_os_error();
    if err.raw_os_error() != Some(libc::EINPROGRESS) {
        return Err(err).context("connect to the Wayland socket");
    }
    let mut pfd = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLOUT,
        revents: 0,
    };
    let ms = i32::try_from(timeout.as_millis()).unwrap_or(i32::MAX);
    // SAFETY: one valid pollfd.
    if unsafe { libc::poll(&mut pfd, 1, ms) } != 1 {
        bail!("the Wayland socket did not accept a connection in time");
    }
    let mut so_error: libc::c_int = 0;
    let mut so_len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
    // SAFETY: reads one c_int option into a correctly sized buffer.
    unsafe {
        libc::getsockopt(
            fd.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_ERROR,
            (&mut so_error as *mut libc::c_int).cast(),
            &mut so_len,
        )
    };
    if so_error != 0 {
        return Err(std::io::Error::from_raw_os_error(so_error))
            .context("connect to the Wayland socket");
    }
    Ok(fd)
}

/// The compositor's pid: the peer of a bounded connection to its socket.
fn compositor_pid(path: PathBuf) -> Result<libc::pid_t> {
    let fd = connect_bounded(&path, CALL_TIMEOUT)?;
    super::hyprland::peer_pid(fd.as_raw_fd())
}

/// The three bus steps of an attested read, so the order can be proven without a bus.
trait DisplayConfigBus {
    /// The unique name that owns `org.gnome.Mutter.DisplayConfig` now.
    fn owner(&self) -> Result<String>;
    fn pid(&self, unique: &str) -> Result<u32>;
    fn state(&self, unique: &str) -> Result<State>;
}

/// Reads the layout from the one connection whose pid was checked: the well-known name can change
/// hands between the check and the read, its unique owner cannot.
fn attested_state(bus: &impl DisplayConfigBus, compositor: libc::pid_t) -> Result<State> {
    let owner = bus.owner()?;
    if i64::from(bus.pid(&owner)?) != i64::from(compositor) {
        bail!("org.gnome.Mutter.DisplayConfig is not served by this session's compositor");
    }
    bus.state(&owner)
}

struct SessionBus {
    // Fields drop in order: the connection must go while its runtime still runs.
    connection: zbus::Connection,
    runtime: tokio::runtime::Runtime,
}

impl SessionBus {
    fn open() -> Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        // Timers belong to a runtime: every timeout is made inside the future it bounds.
        let connection = runtime
            .block_on(async {
                tokio::time::timeout(CALL_TIMEOUT, zbus::Connection::session()).await
            })
            .context("session bus timed out")??;
        Ok(Self {
            connection,
            runtime,
        })
    }

    fn timed<T>(
        &self,
        what: &str,
        call: impl std::future::Future<Output = zbus::Result<T>>,
    ) -> Result<T> {
        self.runtime
            .block_on(async { tokio::time::timeout(CALL_TIMEOUT, call).await })
            .with_context(|| format!("{what} timed out"))?
            .with_context(|| format!("{what} failed"))
    }

    fn dbus(&self) -> Result<zbus::fdo::DBusProxy<'static>> {
        self.timed("DBusProxy", zbus::fdo::DBusProxy::new(&self.connection))
    }
}

impl DisplayConfigBus for SessionBus {
    fn owner(&self) -> Result<String> {
        let dbus = self.dbus()?;
        let name = zbus::names::BusName::try_from(DEST)?;
        let owner = self
            .runtime
            .block_on(async { tokio::time::timeout(CALL_TIMEOUT, dbus.get_name_owner(name)).await })
            .context("GetNameOwner timed out")??;
        Ok(owner.to_string())
    }

    fn pid(&self, unique: &str) -> Result<u32> {
        let dbus = self.dbus()?;
        let name = zbus::names::BusName::try_from(unique.to_owned())?;
        Ok(self
            .runtime
            .block_on(async {
                tokio::time::timeout(CALL_TIMEOUT, dbus.get_connection_unix_process_id(name)).await
            })
            .context("GetConnectionUnixProcessID timed out")??)
    }

    fn state(&self, unique: &str) -> Result<State> {
        let proxy = self.timed(
            "DisplayConfig proxy",
            zbus::Proxy::new(&self.connection, unique.to_owned(), PATH, DEST),
        )?;
        self.timed("GetCurrentState", proxy.call("GetCurrentState", &()))
    }
}

/// The desktop frame of the GNOME session this process draws on, attested to its compositor.
pub fn screen_report() -> Result<(u32, u32, f64, serde_json::Value)> {
    let compositor = compositor_pid(wayland_socket(|name| std::env::var_os(name))?)?;
    // Own thread and runtime: callers may already be inside Tokio (see kwin_helper).
    let state = std::thread::spawn(move || attested_state(&SessionBus::open()?, compositor))
        .join()
        .map_err(|_| anyhow!("GNOME display query thread panicked"))??;
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

    /// A bus whose well-known name moves to another process after the first lookup.
    struct HandingOver {
        asked_state_of: std::cell::RefCell<Vec<String>>,
        owners: std::cell::RefCell<Vec<&'static str>>,
    }

    impl DisplayConfigBus for HandingOver {
        fn owner(&self) -> Result<String> {
            Ok(self.owners.borrow_mut().remove(0).to_owned())
        }
        fn pid(&self, unique: &str) -> Result<u32> {
            Ok(if unique == ":1.5" { 100 } else { 200 })
        }
        fn state(&self, unique: &str) -> Result<State> {
            self.asked_state_of.borrow_mut().push(unique.to_owned());
            Ok((0, vec![], vec![], HashMap::new()))
        }
    }

    #[test]
    fn the_layout_comes_from_the_owner_whose_pid_was_checked_even_if_the_name_moves() {
        let bus = HandingOver {
            asked_state_of: Default::default(),
            owners: std::cell::RefCell::new(vec![":1.5", ":1.9"]),
        };
        attested_state(&bus, 100).unwrap();
        assert_eq!(*bus.asked_state_of.borrow(), vec![":1.5".to_owned()]);
        let other = HandingOver {
            asked_state_of: Default::default(),
            owners: std::cell::RefCell::new(vec![":1.9"]),
        };
        assert!(attested_state(&other, 100).is_err());
        assert!(other.asked_state_of.borrow().is_empty());
    }

    #[test]
    fn an_inherited_wayland_socket_is_refused_and_a_named_one_resolves() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| std::ffi::OsString::from(v))
            }
        };
        assert!(wayland_socket(env(&[
            ("WAYLAND_SOCKET", "5"),
            ("WAYLAND_DISPLAY", "wayland-0"),
            ("XDG_RUNTIME_DIR", "/run/user/1000"),
        ]))
        .is_err());
        assert_eq!(
            wayland_socket(env(&[
                ("WAYLAND_DISPLAY", "wayland-0"),
                ("XDG_RUNTIME_DIR", "/run/user/1000")
            ]))
            .unwrap(),
            PathBuf::from("/run/user/1000/wayland-0")
        );
    }

    #[test]
    fn a_socket_that_never_accepts_fails_at_once_and_leaves_nothing_open() {
        use std::os::unix::net::UnixListener;
        let dir = std::env::temp_dir().join(format!("cua-mutter-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("stalled-0");
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), 0) }, 0);
        // Fill the backlog until the kernel itself says it is full; nothing ever accepts.
        let mut held = Vec::new();
        loop {
            match connect_bounded(&path, Duration::ZERO) {
                Ok(fd) => held.push(fd),
                Err(_) => break,
            }
            assert!(held.len() < 64, "the backlog never filled");
        }
        let open = || std::fs::read_dir("/proc/self/fd").unwrap().count();
        let before = open();
        let started = std::time::Instant::now();
        for _ in 0..3 {
            assert!(compositor_pid(path.clone()).is_err());
        }
        assert!(started.elapsed() < CALL_TIMEOUT);
        assert_eq!(open(), before);
        drop(held);
        let _ = std::fs::remove_file(&path);
    }
}

//! libei-based input synthesis via the `reis` crate + xdg-desktop-portal
//! RemoteDesktop.
//!
//! Reaches GNOME/Mutter (47+, mutter shipped EIS in 45, ei_text in 46)
//! and KDE/KWin (Plasma 6.0+, ei_text in 6.1) — the cross-DE input
//! complement that doesn't depend on the wlroots-only
//! `zwlr_virtual_pointer_v1` / `zwlr_virtual_keyboard_v1` protocols.
//!
//! Sequence:
//! 1. `ei::Context::connect_to_env()` — fast path when a $LIBEI_SOCKET is
//!    already exported (cua-compositor / inject mode).
//! 2. Fallback: ashpd `RemoteDesktop::create_session` →
//!    `select_devices(KEYBOARD|POINTER)` →
//!    `start(session, parent)` (user consent dialog the first time) →
//!    `connect_to_eis()` returns a Unix fd → `ei::Context::new(fd)`.
//! 3. Worker thread runs a calloop event loop on the ei::Context;
//!    handshake → connection.seat events → seat.bind(capabilities) →
//!    device.frame + emulate events.
//! 4. Public API sends commands over a crossbeam-channel and blocks
//!    until the worker reports the request was flushed to the EIS
//!    server.
//!
//! Persistence: ashpd `PersistMode::Application` caches the consent grant
//! for the lifetime of the requesting binary; restore_token written to
//! `~/.config/cua-driver/libei.token` survives reboots (the worker
//! reads it on startup).
//!
//! Coordinates: ei_pointer_absolute uses LOGICAL PIXELS inside an
//! announced ei_device.Region — collected between device creation and
//! the first device.Done event. Multi-monitor: pick the region
//! containing the target (x, y).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::thread;

use crossbeam_channel::{bounded, Receiver, Sender};
use xkbcommon::xkb;

/// Buttons the public API exposes. Mapped to evdev codes in the worker
/// thread so the libei surface only sees evdev integers.
#[derive(Debug, Clone, Copy)]
pub enum Button {
    Left,
    Right,
    Middle,
}

impl Button {
    fn to_evdev(self) -> u32 {
        match self {
            Button::Left => 0x110,    // BTN_LEFT
            Button::Right => 0x111,   // BTN_RIGHT
            Button::Middle => 0x112,  // BTN_MIDDLE
        }
    }
}

/// Commands the worker thread accepts. Each carries a reply channel so
/// the caller blocks until the EIS server has received the event.
enum Cmd {
    Click {
        x: f64,
        y: f64,
        button: Button,
        reply: Sender<anyhow::Result<()>>,
    },
    MoveAbsolute {
        x: f64,
        y: f64,
        reply: Sender<anyhow::Result<()>>,
    },
    Scroll {
        dx: f64,
        dy: f64,
        reply: Sender<anyhow::Result<()>>,
    },
    TypeText {
        text: String,
        reply: Sender<anyhow::Result<()>>,
    },
    PressKey {
        keycode: u32,
        reply: Sender<anyhow::Result<()>>,
    },
    Drag {
        from_x: f64,
        from_y: f64,
        to_x: f64,
        to_y: f64,
        steps: u32,
        button: Button,
        reply: Sender<anyhow::Result<()>>,
    },
    Shutdown,
}

static TX: OnceLock<Sender<Cmd>> = OnceLock::new();

fn tx() -> anyhow::Result<&'static Sender<Cmd>> {
    TX.get().ok_or_else(|| anyhow::anyhow!("libei worker not started; call ensure_started() first"))
}

fn restore_token_path() -> Option<PathBuf> {
    let base = dirs::config_dir()?;
    Some(base.join("cua-driver").join("libei.token"))
}

fn read_restore_token() -> Option<String> {
    let path = restore_token_path()?;
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn write_restore_token(token: &str) -> anyhow::Result<()> {
    let path = restore_token_path()
        .ok_or_else(|| anyhow::anyhow!("no config dir available for libei restore_token"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            anyhow::anyhow!("failed to create {} for restore_token: {e}", parent.display())
        })?;
    }
    std::fs::write(&path, token)
        .map_err(|e| anyhow::anyhow!("failed to write libei restore_token to {}: {e}", path.display()))
}

/// Spawn the libei worker thread (idempotent — safe to call from every
/// MCP tool invocation; subsequent calls are no-ops).
pub fn ensure_started() -> anyhow::Result<()> {
    TX.get_or_init(|| {
        let (tx, rx) = bounded::<Cmd>(64);
        thread::Builder::new()
            .name("cua-libei-worker".into())
            .spawn(move || {
                if let Err(e) = worker(rx) {
                    tracing::warn!("cua-libei-worker exited with error: {e}");
                }
            })
            .expect("spawn cua-libei-worker thread");
        tx
    });
    Ok(())
}

// ── Public API ───────────────────────────────────────────────────────────

/// Move the cursor to absolute (x, y) within the announced device region,
/// then press + release `button`. Blocks until the EIS server has
/// received the event sequence.
pub fn click(x: f64, y: f64, button: Button) -> anyhow::Result<()> {
    ensure_started()?;
    let (tx_r, rx_r) = bounded(1);
    tx()?.send(Cmd::Click { x, y, button, reply: tx_r })
        .map_err(|e| anyhow::anyhow!("libei worker channel closed: {e}"))?;
    rx_r.recv().map_err(|e| anyhow::anyhow!("libei reply closed: {e}"))?
}

/// Move the cursor to absolute (x, y) inside the device region (no
/// button press).
pub fn move_absolute(x: f64, y: f64) -> anyhow::Result<()> {
    ensure_started()?;
    let (tx_r, rx_r) = bounded(1);
    tx()?.send(Cmd::MoveAbsolute { x, y, reply: tx_r })
        .map_err(|e| anyhow::anyhow!("libei worker channel closed: {e}"))?;
    rx_r.recv().map_err(|e| anyhow::anyhow!("libei reply closed: {e}"))?
}

/// Scroll by (dx, dy) logical units. Positive y scrolls down.
pub fn scroll(dx: f64, dy: f64) -> anyhow::Result<()> {
    ensure_started()?;
    let (tx_r, rx_r) = bounded(1);
    tx()?.send(Cmd::Scroll { dx, dy, reply: tx_r })
        .map_err(|e| anyhow::anyhow!("libei worker channel closed: {e}"))?;
    rx_r.recv().map_err(|e| anyhow::anyhow!("libei reply closed: {e}"))?
}

/// Inject a UTF-8 string via the `ei_text` interface (libei 1.6+).
/// Bypasses keycode reverse-mapping — works correctly for any layout.
pub fn type_text(text: &str) -> anyhow::Result<()> {
    ensure_started()?;
    let (tx_r, rx_r) = bounded(1);
    tx()?.send(Cmd::TypeText { text: text.to_string(), reply: tx_r })
        .map_err(|e| anyhow::anyhow!("libei worker channel closed: {e}"))?;
    rx_r.recv().map_err(|e| anyhow::anyhow!("libei reply closed: {e}"))?
}

/// Press + release a key by evdev code (e.g. `KEY_ENTER` = 28). For
/// modifier combos use the keyboard interface's modifier state explicitly.
pub fn press_key(keycode: u32) -> anyhow::Result<()> {
    ensure_started()?;
    let (tx_r, rx_r) = bounded(1);
    tx()?.send(Cmd::PressKey { keycode, reply: tx_r })
        .map_err(|e| anyhow::anyhow!("libei worker channel closed: {e}"))?;
    rx_r.recv().map_err(|e| anyhow::anyhow!("libei reply closed: {e}"))?
}

/// Press `button` at (from_x, from_y), move through `steps` interpolated points
/// to (to_x, to_y), then release — a genuine button-held drag (text selection /
/// drag-and-drop / slider). Coordinates are absolute within the announced
/// device region, like [`click`].
pub fn drag(
    from_x: f64,
    from_y: f64,
    to_x: f64,
    to_y: f64,
    steps: u32,
    button: Button,
) -> anyhow::Result<()> {
    ensure_started()?;
    let (tx_r, rx_r) = bounded(1);
    tx()?
        .send(Cmd::Drag {
            from_x,
            from_y,
            to_x,
            to_y,
            steps,
            button,
            reply: tx_r,
        })
        .map_err(|e| anyhow::anyhow!("libei worker channel closed: {e}"))?;
    rx_r.recv().map_err(|e| anyhow::anyhow!("libei reply closed: {e}"))?
}

/// Cleanly stop the worker thread.
pub fn shutdown() {
    if let Some(tx) = TX.get() {
        let _ = tx.send(Cmd::Shutdown);
    }
}

// ── Worker thread ────────────────────────────────────────────────────────
//
// The worker owns the `reis::ei::Context`, the per-seat device map, and
// the negotiated input region. It runs a calloop event loop that handles
// both the EIS protocol and the inbound command channel.

/// Owns the resources that must outlive a single libei call so the portal
/// RemoteDesktop + EIS session stays alive for the whole worker-thread lifetime.
///
/// GNOME/Mutter (and KDE) tie the RemoteDesktop session — and therefore the
/// handed-off EIS fd — to the D-Bus connection that created it. The previous
/// code dropped the ashpd proxy/session (and the tokio runtime backing their
/// zbus connection) at the end of `open_eis_context`, before the calloop loop
/// even started, so the session died immediately on those compositors (#2105).
/// Holding these for the worker's lifetime keeps the connection — and the
/// session — open. The `$LIBEI_SOCKET` / cua-compositor fast path has no portal
/// session, so it carries `None`.
#[allow(dead_code)] // fields are keep-alive only; never read
enum PortalKeepAlive {
    None,
    Portal {
        _rt: tokio::runtime::Runtime,
        _proxy: ashpd::desktop::remote_desktop::RemoteDesktop,
        _session: ashpd::desktop::Session<ashpd::desktop::remote_desktop::RemoteDesktop>,
    },
}

fn worker(rx: Receiver<Cmd>) -> anyhow::Result<()> {
    // Phase 1 — acquire an EIS context (plus, on the portal path, a keep-alive
    // that owns the ashpd RemoteDesktop session + its tokio runtime).
    let (context, _portal_keepalive) = open_eis_context()
        .map_err(|e| anyhow::anyhow!("failed to obtain EIS connection: {e}"))?;

    // Phase 2 — handshake. The reis API requires we call context.handshake()
    // and flush before the event loop starts polling.
    let _handshake = context.handshake();
    let _ = context.flush();

    // Phase 3 — run the calloop event loop. We use a calloop Generic source
    // wrapping the ei::Context's underlying socket so the loop wakes up on
    // EIS messages. The command channel is polled at the top of each loop
    // iteration. `_portal_keepalive` stays bound until this fn returns — i.e.
    // across the entire loop — so the portal session is never dropped early (#2105).
    run_calloop(context, rx)
}

/// Open the EIS context. Fast path: $LIBEI_SOCKET (cua-compositor /
/// inject mode). Fallback: ashpd portal RemoteDesktop handshake.
fn open_eis_context() -> anyhow::Result<(reis::ei::Context, PortalKeepAlive)> {
    if let Some(ctx) = reis::ei::Context::connect_to_env()
        .map_err(|e| anyhow::anyhow!("env ei socket open failed: {e}"))?
    {
        // $LIBEI_SOCKET / cua-compositor fast path: no portal session to keep alive.
        return Ok((ctx, PortalKeepAlive::None));
    }

    // Portal RemoteDesktop fallback.
    use ashpd::desktop::{
        remote_desktop::{
            ConnectToEISOptions, DeviceType, RemoteDesktop, SelectDevicesOptions, StartOptions,
        },
        CreateSessionOptions, PersistMode,
    };
    use ashpd::enumflags2::BitFlags;
    use std::os::unix::net::UnixStream;

    // A multi-threaded runtime (1 worker) so the zbus connection backing the
    // RemoteDesktop session keeps being driven after this fn returns. The
    // session must stay live for the whole worker lifetime (#2105); a
    // current-thread runtime would freeze the connection the moment we stop
    // calling `block_on`, and GNOME/KDE would tear the session down.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .map_err(|e| anyhow::anyhow!("failed to build tokio runtime for ashpd: {e}"))?;

    let (fd, proxy, session) = rt.block_on(async {
        let proxy = RemoteDesktop::new()
            .await
            .map_err(|e| anyhow::anyhow!("portal RemoteDesktop proxy unreachable: {e}. Install xdg-desktop-portal-gnome / xdg-desktop-portal-kde."))?;
        let session = proxy
            .create_session(CreateSessionOptions::default())
            .await
            .map_err(|e| anyhow::anyhow!("portal create_session failed: {e}"))?;

        let mut select_opts = SelectDevicesOptions::default()
            .set_devices(BitFlags::<DeviceType>::from(DeviceType::Keyboard) | DeviceType::Pointer)
            .set_persist_mode(PersistMode::Application);
        if let Some(tok) = read_restore_token() {
            select_opts = select_opts.set_restore_token(Some(tok.as_str()));
        }

        proxy
            .select_devices(&session, select_opts)
            .await
            .map_err(|e| anyhow::anyhow!("portal select_devices failed: {e}"))?
            .response()
            .map_err(|e| anyhow::anyhow!("portal select_devices response error: {e}"))?;

        let started = proxy
            .start(&session, None, StartOptions::default())
            .await
            .map_err(|e| anyhow::anyhow!("portal start failed: {e}"))?
            .response()
            .map_err(|e| anyhow::anyhow!("portal start response error (user denied?): {e}"))?;

        // Best-effort persist of the restore_token. Without this, every
        // process restart re-prompts the user for consent — with it, ashpd
        // 0.13's PersistMode::Application + the stored token together
        // skip the dialog for the lifetime of the persistence grant
        // (typically per login session on GNOME, indefinite on KDE).
        if let Some(tok) = started.restore_token() {
            let _ = write_restore_token(tok);
        }

        let fd = proxy
            .connect_to_eis(&session, ConnectToEISOptions::default())
            .await
            .map_err(|e| anyhow::anyhow!("portal connect_to_eis failed: {e}"))?;
        anyhow::Ok((fd, proxy, session))
    })?;

    let stream = UnixStream::from(fd);
    let ctx = reis::ei::Context::new(stream)
        .map_err(|e| anyhow::anyhow!("reis ei::Context::new failed: {e}"))?;
    Ok((ctx, PortalKeepAlive::Portal { _rt: rt, _proxy: proxy, _session: session }))
}

// ── calloop dispatch ────────────────────────────────────────────────────
//
// The EIS protocol is event-driven and non-trivial — handshake → connection
// → seat → device → frame negotiation. We model the worker as a calloop
// EventLoop driving the ei::Context Generic source, with a separate
// channel source for inbound Cmd messages.
//
// For v1 we implement the handshake + seat-bind dance and provide
// click/move/type via the negotiated pointer/keyboard/text interfaces.
// Region selection picks the first announced ei_device::Region.

fn run_calloop(context: reis::ei::Context, rx: Receiver<Cmd>) -> anyhow::Result<()> {
    use calloop::{generic::Generic, EventLoop, Interest, Mode, PostAction};

    let mut event_loop: EventLoop<EisState> = EventLoop::try_new()
        .map_err(|e| anyhow::anyhow!("calloop EventLoop::try_new failed: {e}"))?;
    let handle = event_loop.handle();

    // EIS protocol source.
    let ctx_source = Generic::new(context, Interest::READ, Mode::Level);
    handle
        .insert_source(ctx_source, |_event, ctx, state: &mut EisState| {
            state.handle_eis_readable(unsafe { ctx.get_mut() })
        })
        .map_err(|e| anyhow::anyhow!("calloop insert_source(eis) failed: {e}"))?;

    let mut state = EisState::default();

    // Inbound commands can arrive before the EIS handshake has negotiated a
    // usable input device — seat → device → interface → Resumed is async and
    // only advances while `dispatch` runs, and the portal consent-and-connect
    // path (#2105) returns before the device is live. Running a command against
    // a not-yet-negotiated device fails with "no EIS device negotiated yet". So
    // queue commands and run each only once `input_ready()`; fail any that wait
    // longer than READY_TIMEOUT (handle_command then replies with the natural
    // "no device" error) so a caller blocked on the reply never hangs forever.
    let mut pending: Vec<(Cmd, std::time::Instant)> = Vec::new();
    const READY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

    loop {
        match rx.try_recv() {
            Ok(Cmd::Shutdown) => break,
            Ok(cmd) => pending.push((cmd, std::time::Instant::now())),
            Err(crossbeam_channel::TryRecvError::Empty) => {}
            Err(crossbeam_channel::TryRecvError::Disconnected) => break,
        }

        event_loop
            .dispatch(Some(std::time::Duration::from_millis(20)), &mut state)
            .map_err(|e| anyhow::anyhow!("calloop dispatch failed: {e}"))?;

        // Run queued commands once a device is negotiated; time stale ones out.
        if !pending.is_empty() {
            let ready = state.input_ready();
            let mut still = Vec::with_capacity(pending.len());
            for (cmd, queued) in pending.drain(..) {
                if ready || queued.elapsed() >= READY_TIMEOUT {
                    state.handle_command(cmd);
                } else {
                    still.push((cmd, queued));
                }
            }
            pending = still;
        }

        // The handle_command path may have queued frame() requests; flush
        // them once per loop iteration so the EIS server sees them.
        if let Some(ctx) = state.context.as_ref() {
            let _ = ctx.flush();
        }
    }

    Ok(())
}

#[derive(Default)]
struct EisState {
    context: Option<reis::ei::Context>,
    seats: HashMap<reis::ei::Seat, SeatData>,
    devices: HashMap<reis::ei::Device, DeviceData>,
    sequence: u32,
    last_serial: u32,
    /// Set once the server has sent at least one device `Resumed` — the point
    /// at which emulation is actually accepted. Commands issued before this
    /// would fail with "no EIS device negotiated yet"; the worker queues them
    /// until [`EisState::input_ready`] returns true.
    resumed: bool,
    /// `char -> (evdev keycode, needs_shift)` built from the compositor's active
    /// xkb keymap (delivered by the `ei_keyboard` device). Lets keycode typing
    /// follow the user's real layout instead of assuming US. Empty until the
    /// `Keymap` event arrives; typing then falls back to [`char_to_evdev`].
    keymap_chars: HashMap<char, (u32, bool)>,
    /// evdev keycode that produces `Shift_L` in the active keymap (else 42).
    keymap_shift: Option<u32>,
}

#[derive(Default)]
struct SeatData {
    capabilities: HashMap<String, u64>,
}

/// One announced ei_device::Region — a single monitor's pixel rectangle
/// inside the seat's logical coordinate space. Multi-monitor sessions
/// announce one Region per output; we route each absolute (x, y) to the
/// region containing it so the cursor lands on the correct monitor.
#[derive(Clone, Copy, Debug)]
struct RegionRect {
    offset_x: f32,
    offset_y: f32,
    width: f32,
    height: f32,
}

impl RegionRect {
    fn contains(&self, x: f64, y: f64) -> bool {
        let xf = x as f32;
        let yf = y as f32;
        xf >= self.offset_x
            && xf < self.offset_x + self.width
            && yf >= self.offset_y
            && yf < self.offset_y + self.height
    }
}

#[derive(Default)]
struct DeviceData {
    device_type: Option<reis::ei::device::DeviceType>,
    /// Every Region the device announces between its creation and its
    /// first Done event. Empty until the EIS server has sent at least
    /// one Region.
    regions: Vec<RegionRect>,
    interfaces: HashMap<String, reis::Object>,
}

impl EisState {
    fn handle_eis_readable(
        &mut self,
        context: &mut reis::ei::Context,
    ) -> std::io::Result<calloop::PostAction> {
        use reis::PendingRequestResult;
        if self.context.is_none() {
            self.context = Some(context.clone());
        }
        if context.read().is_err() {
            return Ok(calloop::PostAction::Remove);
        }
        while let Some(result) = context.pending_event() {
            let request = match result {
                PendingRequestResult::Request(r) => r,
                _ => continue,
            };
            self.dispatch_eis_event(request);
        }
        let _ = context.flush();
        Ok(calloop::PostAction::Continue)
    }

    fn dispatch_eis_event(&mut self, event: reis::ei::Event) {
        use reis::ei::Event;
        match event {
            Event::Handshake(handshake, ev) => {
                if let reis::ei::handshake::Event::HandshakeVersion { .. } = ev {
                    handshake.handshake_version(1);
                    handshake.name("cua-driver");
                    handshake.context_type(reis::ei::handshake::ContextType::Sender);
                    // Advertise the interface set + versions that reis 0.7's own
                    // EIS client examples use against real compositors. The
                    // previous list advertised ei_device v2 and omitted
                    // ei_pingpong, which Mutter rejects — it closes the EIS
                    // connection immediately after our handshake `finish()`
                    // (observed: one Handshake event, then read() errors). reis's
                    // handshaker also requires ei_pingpong / ei_callback /
                    // ei_connection. See reis 0.7 examples/type-text.rs.
                    for (iface, ver) in [
                        ("ei_callback", 1u32),
                        ("ei_connection", 1),
                        ("ei_pingpong", 1),
                        ("ei_seat", 1),
                        ("ei_device", 1),
                        ("ei_pointer", 1),
                        ("ei_pointer_absolute", 1),
                        ("ei_button", 1),
                        ("ei_scroll", 1),
                        ("ei_keyboard", 1),
                        ("ei_text", 1),
                    ] {
                        handshake.interface_version(iface, ver);
                    }
                    handshake.finish();
                }
            }
            Event::Connection(_conn, ev) => match ev {
                reis::ei::connection::Event::Seat { seat } => {
                    self.seats.insert(seat, SeatData::default());
                }
                // The EIS server pings to check liveness; failing to answer
                // makes it drop the connection. Mirror reis's examples.
                reis::ei::connection::Event::Ping { ping } => {
                    ping.done(0);
                }
                _ => {}
            },
            Event::Seat(seat, ev) => {
                let data = self.seats.entry(seat.clone()).or_default();
                match ev {
                    reis::ei::seat::Event::Capability { mask, interface } => {
                        data.capabilities.insert(interface, mask);
                    }
                    reis::ei::seat::Event::Done => {
                        // Bind every input capability the seat advertises
                        // (pointer / pointer_absolute / button / scroll /
                        // keyboard / text). The server then announces
                        // matching devices.
                        let mut mask = 0u64;
                        for k in [
                            "ei_pointer",
                            "ei_pointer_absolute",
                            "ei_button",
                            "ei_scroll",
                            "ei_keyboard",
                            "ei_text",
                        ] {
                            if let Some(m) = data.capabilities.get(k) {
                                mask |= *m;
                            }
                        }
                        if mask != 0 {
                            seat.bind(mask);
                        }
                    }
                    reis::ei::seat::Event::Device { device } => {
                        self.devices.insert(device, DeviceData::default());
                    }
                    _ => {}
                }
            }
            Event::Device(device, ev) => {
                let data = self.devices.entry(device.clone()).or_default();
                match ev {
                    reis::ei::device::Event::DeviceType { device_type } => {
                        data.device_type = Some(device_type);
                    }
                    reis::ei::device::Event::Interface { object } => {
                        data.interfaces.insert(object.interface().to_owned(), object);
                    }
                    reis::ei::device::Event::Region { offset_x, offset_y, width, hight, scale: _ } => {
                        // reis 0.7 keeps the misspelled "hight" field name
                        // for ABI compatibility — see the protocol comment.
                        if width > 0 && hight > 0 {
                            data.regions.push(RegionRect {
                                offset_x: offset_x as f32,
                                offset_y: offset_y as f32,
                                width: width as f32,
                                height: hight as f32,
                            });
                        }
                    }
                    reis::ei::device::Event::Resumed { serial } => {
                        self.last_serial = serial;
                        self.resumed = true;
                    }
                    _ => {}
                }
            }
            Event::Keyboard(
                _kb,
                reis::ei::keyboard::Event::Keymap {
                    keymap_type,
                    size,
                    keymap,
                },
            ) => {
                if keymap_type == reis::ei::keyboard::KeymapType::Xkb {
                    self.load_xkb_keymap(keymap, size);
                }
            }
            _ => {}
        }
    }

    /// Compile the compositor's xkb keymap (from the `ei_keyboard` `Keymap` fd)
    /// into a `char -> (evdev keycode, shift)` table + the Shift keycode, so
    /// keycode typing follows the user's real layout instead of assuming US.
    /// Best-effort: on any failure the table stays empty and typing falls back
    /// to [`char_to_evdev`].
    fn load_xkb_keymap(&mut self, fd: std::os::unix::io::OwnedFd, size: u32) {
        use std::io::{Read, Seek, SeekFrom};
        const XKB_EVDEV_OFFSET: u32 = 8;
        const SHIFT_L: u32 = 0xffe1;

        let mut file = std::fs::File::from(fd);
        let mut buf = vec![0u8; size as usize];
        let _ = file.seek(SeekFrom::Start(0));
        if file.read_exact(&mut buf).is_err() {
            return;
        }
        // The keymap buffer is NUL-terminated text.
        let text = match buf.iter().position(|&b| b == 0) {
            Some(n) => String::from_utf8_lossy(&buf[..n]).into_owned(),
            None => String::from_utf8_lossy(&buf).into_owned(),
        };
        let ctx = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let keymap = match xkb::Keymap::new_from_string(
            &ctx,
            text,
            xkb::KEYMAP_FORMAT_TEXT_V1,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        ) {
            Some(k) => k,
            None => return,
        };

        let mut chars: HashMap<char, (u32, bool)> = HashMap::new();
        let mut shift = None;
        for kc in keymap.min_keycode().raw()..=keymap.max_keycode().raw() {
            let keycode = xkb::Keycode::new(kc);
            let evdev = kc.wrapping_sub(XKB_EVDEV_OFFSET);
            // Level 0 = unshifted, 1 = shifted; scan 0 first so the unshifted
            // mapping wins when a char is reachable both ways.
            for level in 0..=1u32 {
                for sym in keymap.key_get_syms_by_level(keycode, 0, level) {
                    if shift.is_none() && sym.raw() == SHIFT_L {
                        shift = Some(evdev);
                    }
                    if let Some(ch) = sym.key_char() {
                        chars.entry(ch).or_insert((evdev, level == 1));
                    }
                }
            }
        }
        self.keymap_chars = chars;
        self.keymap_shift = shift;
    }

    /// True once the EIS handshake has negotiated a usable input device: a
    /// pointer/keyboard device that has announced its interfaces and been
    /// `Resumed` by the server. Commands run before this fail with "no EIS
    /// device negotiated yet", so `run_calloop` queues them until this holds.
    fn input_ready(&self) -> bool {
        // Gate on the absolute-pointer device: we always request Keyboard|Pointer,
        // and Mutter/KWin announce the absolute pointer alongside (typically
        // after) the keyboard, so once it exists every device a command needs
        // has arrived. Avoids running a command against a half-negotiated seat.
        self.resumed && self.device_with_interface("ei_pointer_absolute").is_some()
    }

    fn handle_command(&mut self, cmd: Cmd) {
        let result = self.run_command(&cmd);
        // Send the reply on whichever channel this command carries.
        match cmd {
            Cmd::Click { reply, .. } => { let _ = reply.send(result); }
            Cmd::MoveAbsolute { reply, .. } => { let _ = reply.send(result); }
            Cmd::Scroll { reply, .. } => { let _ = reply.send(result); }
            Cmd::TypeText { reply, .. } => { let _ = reply.send(result); }
            Cmd::PressKey { reply, .. } => { let _ = reply.send(result); }
            Cmd::Drag { reply, .. } => { let _ = reply.send(result); }
            Cmd::Shutdown => {}
        }
    }

    fn run_command(&mut self, cmd: &Cmd) -> anyhow::Result<()> {
        match cmd {
            Cmd::Click { x, y, button, .. } => {
                let (device, rel_x, rel_y) = self.pointer_device_for(*x, *y)?;
                let ptr_abs = require_device_interface::<reis::ei::PointerAbsolute>(&self.devices, &device)?;
                let btn = require_device_interface::<reis::ei::Button>(&self.devices, &device)?;

                device.start_emulating(self.sequence, self.last_serial);
                self.sequence = self.sequence.wrapping_add(1);
                ptr_abs.motion_absolute(rel_x, rel_y);
                btn.button(button.to_evdev(), reis::ei::button::ButtonState::Press);
                device.frame(self.last_serial, 0);
                btn.button(button.to_evdev(), reis::ei::button::ButtonState::Released);
                device.frame(self.last_serial, 1);
                device.stop_emulating(self.last_serial);
            }
            Cmd::Drag {
                from_x,
                from_y,
                to_x,
                to_y,
                steps,
                button,
                ..
            } => {
                let (device, from_rx, from_ry) = self.pointer_device_for(*from_x, *from_y)?;
                let ptr_abs =
                    require_device_interface::<reis::ei::PointerAbsolute>(&self.devices, &device)?;
                let btn = require_device_interface::<reis::ei::Button>(&self.devices, &device)?;
                // Map the drag end into the same region the start resolved to.
                let off_x = *from_x as f32 - from_rx;
                let off_y = *from_y as f32 - from_ry;
                let to_rx = *to_x as f32 - off_x;
                let to_ry = *to_y as f32 - off_y;

                device.start_emulating(self.sequence, self.last_serial);
                self.sequence = self.sequence.wrapping_add(1);
                ptr_abs.motion_absolute(from_rx, from_ry);
                device.frame(self.last_serial, 0);
                btn.button(button.to_evdev(), reis::ei::button::ButtonState::Press);
                device.frame(self.last_serial, 1);
                let n = (*steps).max(1);
                let mut fseq = 2u64;
                for s in 1..=n {
                    let t = s as f32 / n as f32;
                    let ix = from_rx + (to_rx - from_rx) * t;
                    let iy = from_ry + (to_ry - from_ry) * t;
                    ptr_abs.motion_absolute(ix, iy);
                    device.frame(self.last_serial, fseq);
                    fseq += 1;
                }
                btn.button(button.to_evdev(), reis::ei::button::ButtonState::Released);
                device.frame(self.last_serial, fseq);
                device.stop_emulating(self.last_serial);
            }
            Cmd::MoveAbsolute { x, y, .. } => {
                let (device, rel_x, rel_y) = self.pointer_device_for(*x, *y)?;
                let ptr_abs = require_device_interface::<reis::ei::PointerAbsolute>(&self.devices, &device)?;

                device.start_emulating(self.sequence, self.last_serial);
                self.sequence = self.sequence.wrapping_add(1);
                ptr_abs.motion_absolute(rel_x, rel_y);
                device.frame(self.last_serial, 0);
                device.stop_emulating(self.last_serial);
            }
            Cmd::Scroll { dx, dy, .. } => {
                let device = self.device_with_interface("ei_scroll")
                    .ok_or_else(|| anyhow::anyhow!("no EIS scroll device negotiated yet — wait for handshake"))?;
                let scroll = require_device_interface::<reis::ei::Scroll>(&self.devices, &device)?;

                device.start_emulating(self.sequence, self.last_serial);
                self.sequence = self.sequence.wrapping_add(1);
                scroll.scroll(*dx as f32, *dy as f32);
                device.frame(self.last_serial, 0);
                device.stop_emulating(self.last_serial);
            }
            Cmd::TypeText { text, .. } => {
                // Prefer ei_text (libei 1.6+ — a UTF-8 string, layout-correct).
                // Mutter does NOT advertise ei_text, so fall back to keycode
                // typing via ei_keyboard: map each char to a US-layout evdev
                // keycode + shift and emit press/release. Chars with no US-layout
                // keycode are skipped (a documented limitation — full layout
                // support needs an xkb keymap, like reis's type-text example).
                if let Some(device) = self.device_with_interface("ei_text") {
                    let text_iface = require_device_interface::<reis::ei::Text>(&self.devices, &device)?;
                    device.start_emulating(self.sequence, self.last_serial);
                    self.sequence = self.sequence.wrapping_add(1);
                    text_iface.utf8(text);
                    device.frame(self.last_serial, 0);
                    device.stop_emulating(self.last_serial);
                } else {
                    use reis::ei::keyboard::KeyState;
                    let device = self.device_with_interface("ei_keyboard").ok_or_else(|| {
                        anyhow::anyhow!("no EIS keyboard device negotiated yet — wait for handshake")
                    })?;
                    let kb = require_device_interface::<reis::ei::Keyboard>(&self.devices, &device)?;
                    let shift_code = self.keymap_shift.unwrap_or(42);
                    device.start_emulating(self.sequence, self.last_serial);
                    self.sequence = self.sequence.wrapping_add(1);
                    let mut fseq = 0u64;
                    for ch in text.chars() {
                        // Prefer the compositor's actual keymap (layout-correct);
                        // fall back to the US-layout table if no keymap arrived.
                        let mapped = self
                            .keymap_chars
                            .get(&ch)
                            .copied()
                            .or_else(|| char_to_evdev(ch));
                        let Some((code, shift)) = mapped else {
                            continue;
                        };
                        if shift {
                            kb.key(shift_code, KeyState::Press);
                            device.frame(self.last_serial, fseq);
                            fseq += 1;
                        }
                        kb.key(code, KeyState::Press);
                        device.frame(self.last_serial, fseq);
                        fseq += 1;
                        kb.key(code, KeyState::Released);
                        device.frame(self.last_serial, fseq);
                        fseq += 1;
                        if shift {
                            kb.key(shift_code, KeyState::Released);
                            device.frame(self.last_serial, fseq);
                            fseq += 1;
                        }
                    }
                    device.stop_emulating(self.last_serial);
                }
            }
            Cmd::PressKey { keycode, .. } => {
                let device = self.device_with_interface("ei_keyboard")
                    .ok_or_else(|| anyhow::anyhow!("no EIS keyboard device negotiated yet — wait for handshake"))?;
                let kb = require_device_interface::<reis::ei::Keyboard>(&self.devices, &device)?;

                device.start_emulating(self.sequence, self.last_serial);
                self.sequence = self.sequence.wrapping_add(1);
                kb.key(*keycode, reis::ei::keyboard::KeyState::Press);
                device.frame(self.last_serial, 0);
                kb.key(*keycode, reis::ei::keyboard::KeyState::Released);
                device.frame(self.last_serial, 1);
                device.stop_emulating(self.last_serial);
            }
            Cmd::Shutdown => {}
        }
        if let Some(ctx) = self.context.as_ref() {
            let _ = ctx.flush();
        }
        Ok(())
    }

    /// Pick a pointer device whose announced Region contains `(x, y)`,
    /// and translate the absolute screen coordinates into the device's
    /// region-local pixel coordinates.
    ///
    /// Multi-monitor seats announce one Region per output; routing each
    /// (x, y) by containment is what lets a click on monitor #2 actually
    /// land on monitor #2. When no device's region contains the point —
    /// e.g. coords just past the edge of an output, or a session with
    /// a single device that hasn't announced any regions yet — we fall
    /// back to the first pointer device with the raw coordinates, which
    /// matches the pre-multi-monitor behaviour.
    fn pointer_device_for(&self, x: f64, y: f64) -> anyhow::Result<(reis::ei::Device, f32, f32)> {
        // Absolute motion needs the device that actually carries
        // ei_pointer_absolute — Mutter/KWin split the relative and absolute
        // pointers onto separate devices, and only one of them accepts an
        // absolute warp.
        for (device, data) in self.devices.iter() {
            if !data.interfaces.contains_key("ei_pointer_absolute") {
                continue;
            }
            for region in &data.regions {
                if region.contains(x, y) {
                    let rel_x = x as f32 - region.offset_x;
                    let rel_y = y as f32 - region.offset_y;
                    return Ok((device.clone(), rel_x, rel_y));
                }
            }
        }
        // Fallback: first absolute-pointer device, raw coords (single-region).
        let device = self.device_with_interface("ei_pointer_absolute")
            .ok_or_else(|| anyhow::anyhow!("no EIS absolute-pointer device negotiated yet — wait for handshake"))?;
        Ok((device, x as f32, y as f32))
    }

    /// The negotiated device that exposes `iface` (e.g. `ei_pointer_absolute`,
    /// `ei_scroll`, `ei_keyboard`, `ei_text`). Mutter/KWin announce one device
    /// per capability group — a relative pointer, an absolute pointer, a
    /// keyboard — so a command must pick the device carrying the interface it
    /// needs, not just "the first virtual device" (which may be the relative
    /// pointer that has no `ei_pointer_absolute`).
    fn device_with_interface(&self, iface: &str) -> Option<reis::ei::Device> {
        self.devices
            .iter()
            .find(|(_, data)| data.interfaces.contains_key(iface))
            .map(|(d, _)| d.clone())
    }
}

/// Map a character to a US-layout evdev keycode + whether Shift is needed.
/// Used by the keyboard-typing fallback when the compositor's EIS exposes no
/// `ei_text` interface (e.g. Mutter). Returns `None` for characters outside the
/// US-ASCII printable set — those are skipped rather than mistyped. Full
/// layout-correct typing would need an xkb keymap (see reis `type-text` example).
fn char_to_evdev(c: char) -> Option<(u32, bool)> {
    // Letter codes follow the QWERTY scancode layout (input-event-codes.h).
    fn letter(c: char) -> u32 {
        // KEY_A..KEY_Z follow the QWERTY scancode layout, not the alphabet.
        match c {
            'a' => 30,
            'b' => 48,
            'c' => 46,
            'd' => 32,
            'e' => 18,
            'f' => 33,
            'g' => 34,
            'h' => 35,
            'i' => 23,
            'j' => 36,
            'k' => 37,
            'l' => 38,
            'm' => 50,
            'n' => 49,
            'o' => 24,
            'p' => 25,
            'q' => 16,
            'r' => 19,
            's' => 31,
            't' => 20,
            'u' => 22,
            'v' => 47,
            'w' => 17,
            'x' => 45,
            'y' => 21,
            'z' => 44,
            _ => 0,
        }
    }
    // (keycode, needs_shift). Digit/symbol codes and their shifted variants
    // follow the US QWERTY row (input-event-codes.h).
    Some(match c {
        'a'..='z' => (letter(c), false),
        'A'..='Z' => (letter(c.to_ascii_lowercase()), true),
        '1' => (2, false),
        '2' => (3, false),
        '3' => (4, false),
        '4' => (5, false),
        '5' => (6, false),
        '6' => (7, false),
        '7' => (8, false),
        '8' => (9, false),
        '9' => (10, false),
        '0' => (11, false),
        '!' => (2, true),
        '@' => (3, true),
        '#' => (4, true),
        '$' => (5, true),
        '%' => (6, true),
        '^' => (7, true),
        '&' => (8, true),
        '*' => (9, true),
        '(' => (10, true),
        ')' => (11, true),
        ' ' => (57, false),
        '\n' => (28, false),
        '\t' => (15, false),
        '-' => (12, false),
        '_' => (12, true),
        '=' => (13, false),
        '+' => (13, true),
        '[' => (26, false),
        '{' => (26, true),
        ']' => (27, false),
        '}' => (27, true),
        '\\' => (43, false),
        '|' => (43, true),
        ';' => (39, false),
        ':' => (39, true),
        '\'' => (40, false),
        '"' => (40, true),
        '`' => (41, false),
        '~' => (41, true),
        ',' => (51, false),
        '<' => (51, true),
        '.' => (52, false),
        '>' => (52, true),
        '/' => (53, false),
        '?' => (53, true),
        _ => return None,
    })
}

fn require_device_interface<T: reis::Interface>(
    devices: &HashMap<reis::ei::Device, DeviceData>,
    device: &reis::ei::Device,
) -> anyhow::Result<T> {
    device_interface::<T>(devices, device)
        .ok_or_else(|| anyhow::anyhow!("EIS device lacks required {} interface", T::NAME))
}

fn device_interface<T: reis::Interface>(
    devices: &HashMap<reis::ei::Device, DeviceData>,
    device: &reis::ei::Device,
) -> Option<T> {
    devices.get(device)?
        .interfaces
        .get(T::NAME)?
        .clone()
        .downcast()
}

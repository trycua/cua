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

fn worker(rx: Receiver<Cmd>) -> anyhow::Result<()> {
    // Phase 1 — acquire an EIS context.
    let context = open_eis_context()
        .map_err(|e| anyhow::anyhow!("failed to obtain EIS connection: {e}"))?;

    // Phase 2 — handshake. The reis API requires we call context.handshake()
    // and flush before the event loop starts polling.
    let _handshake = context.handshake();
    let _ = context.flush();

    // Phase 3 — run the calloop event loop. We use a calloop Generic source
    // wrapping the ei::Context's underlying socket so the loop wakes up on
    // EIS messages. The command channel is polled at the top of each loop
    // iteration.
    run_calloop(context, rx)
}

/// Open the EIS context. Fast path: $LIBEI_SOCKET (cua-compositor /
/// inject mode). Fallback: ashpd portal RemoteDesktop handshake.
fn open_eis_context() -> anyhow::Result<reis::ei::Context> {
    if let Some(ctx) = reis::ei::Context::connect_to_env()
        .map_err(|e| anyhow::anyhow!("env ei socket open failed: {e}"))?
    {
        return Ok(ctx);
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

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| anyhow::anyhow!("failed to build tokio runtime for ashpd: {e}"))?;

    let fd = rt.block_on(async {
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
        anyhow::Ok(fd)
    })?;

    let stream = UnixStream::from(fd);
    let ctx = reis::ei::Context::new(stream)
        .map_err(|e| anyhow::anyhow!("reis ei::Context::new failed: {e}"))?;
    Ok(ctx)
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

    // Command channel: drain at the top of each loop iteration with a
    // bounded timeout so the EIS source stays responsive.
    loop {
        // Process pending commands.
        match rx.try_recv() {
            Ok(Cmd::Shutdown) => break,
            Ok(cmd) => state.handle_command(cmd),
            Err(crossbeam_channel::TryRecvError::Empty) => {}
            Err(crossbeam_channel::TryRecvError::Disconnected) => break,
        }

        event_loop
            .dispatch(Some(std::time::Duration::from_millis(20)), &mut state)
            .map_err(|e| anyhow::anyhow!("calloop dispatch failed: {e}"))?;

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
                    for (iface, ver) in [
                        ("ei_callback", 1u32),
                        ("ei_connection", 1),
                        ("ei_seat", 1),
                        ("ei_device", 2),
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
            Event::Connection(_conn, ev) => {
                if let reis::ei::connection::Event::Seat { seat } = ev {
                    self.seats.insert(seat, SeatData::default());
                }
            }
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
                    }
                    _ => {}
                }
            }
            _ => {}
        }
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
                let device = self.any_pointer_device()
                    .ok_or_else(|| anyhow::anyhow!("no EIS pointer device negotiated yet — wait for handshake"))?;
                let scroll = require_device_interface::<reis::ei::Scroll>(&self.devices, &device)?;

                device.start_emulating(self.sequence, self.last_serial);
                self.sequence = self.sequence.wrapping_add(1);
                scroll.scroll(*dx as f32, *dy as f32);
                device.frame(self.last_serial, 0);
                device.stop_emulating(self.last_serial);
            }
            Cmd::TypeText { text, .. } => {
                let device = self.any_pointer_device()
                    .ok_or_else(|| anyhow::anyhow!("no EIS device negotiated yet — wait for handshake"))?;
                let text_iface = require_device_interface::<reis::ei::Text>(&self.devices, &device)?;

                device.start_emulating(self.sequence, self.last_serial);
                self.sequence = self.sequence.wrapping_add(1);
                text_iface.utf8(text);
                device.frame(self.last_serial, 0);
                device.stop_emulating(self.last_serial);
            }
            Cmd::PressKey { keycode, .. } => {
                let device = self.any_pointer_device()
                    .ok_or_else(|| anyhow::anyhow!("no EIS device negotiated yet — wait for handshake"))?;
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
        let is_pointer = |data: &DeviceData| matches!(
            data.device_type,
            Some(reis::ei::device::DeviceType::Virtual) | Some(reis::ei::device::DeviceType::Physical)
        );
        for (device, data) in self.devices.iter() {
            if !is_pointer(data) {
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
        // Fallback: first pointer device, raw coords (single-region behaviour).
        let device = self.any_pointer_device()
            .ok_or_else(|| anyhow::anyhow!("no EIS pointer device negotiated yet — wait for handshake"))?;
        Ok((device, x as f32, y as f32))
    }

    fn any_pointer_device(&self) -> Option<reis::ei::Device> {
        self.devices.iter()
            .find(|(_, data)| matches!(
                data.device_type,
                Some(reis::ei::device::DeviceType::Virtual) | Some(reis::ei::device::DeviceType::Physical)
            ))
            .map(|(d, _)| d.clone())
    }
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

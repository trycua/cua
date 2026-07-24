//! libei-based input synthesis via the `reis` crate + xdg-desktop-portal
//! RemoteDesktop.
//!
//! Reaches GNOME/Mutter (47+, mutter shipped EIS in 45, ei_text in 46)
//! and KDE/KWin (Plasma 6.0+, ei_text in 6.1) — the cross-DE input
//! complement that doesn't depend on the wlroots-only
//! `zwlr_virtual_pointer_v1` / `zwlr_virtual_keyboard_v1` protocols.
//!
//! Sequence:
//! 1. `ei::Context::connect_to_env()` — fast path when a compositor or test
//!    environment already exports `$LIBEI_SOCKET`.
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
//! Persistence: ashpd `PersistMode::ExplicitlyRevoked` keeps the user's consent
//! until they revoke it in desktop settings. The restore token is stored at
//! `~/.config/cua-driver/libei-persistent.token` and reused across daemon
//! restarts.
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
            Button::Left => 0x110,   // BTN_LEFT
            Button::Right => 0x111,  // BTN_RIGHT
            Button::Middle => 0x112, // BTN_MIDDLE
        }
    }
}

/// One evdev keyboard state transition in a [`key_sequence`] request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyTransition {
    Press(u32),
    Release(u32),
}

const MAX_KEY_SEQUENCE_TRANSITIONS: usize = 64;

/// EIS frame timestamps are CLOCK_MONOTONIC microseconds, not per-command
/// sequence numbers. Mutter discards transactions carrying the old 0,1,2...
/// placeholders. Keep them strictly increasing even when several frames are
/// emitted inside one microsecond.
fn event_time_us() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};

    static LAST: AtomicU64 = AtomicU64::new(0);
    let mut ts = unsafe {
        let mut value: libc::timespec = std::mem::zeroed();
        if libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut value) == 0 {
            (value.tv_sec as u64)
                .saturating_mul(1_000_000)
                .saturating_add((value.tv_nsec as u64) / 1_000)
        } else {
            0
        }
    };
    loop {
        let previous = LAST.load(Ordering::Relaxed);
        ts = ts.max(previous.saturating_add(1));
        if LAST
            .compare_exchange_weak(previous, ts, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            return ts;
        }
    }
}

/// Commands the worker thread accepts. Each carries a reply channel so
/// the caller blocks until the EIS server has received the event.
enum Cmd {
    WaitReady {
        interface: &'static str,
        reply: Sender<anyhow::Result<()>>,
    },
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
    KeySequence {
        transitions: Vec<KeyTransition>,
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
    TX.get()
        .ok_or_else(|| anyhow::anyhow!("libei worker not started; call ensure_started() first"))
}

fn wait_for_reply(rx: Receiver<anyhow::Result<()>>) -> anyhow::Result<()> {
    rx.recv_timeout(std::time::Duration::from_secs(20))
        .map_err(|error| match error {
            crossbeam_channel::RecvTimeoutError::Timeout => anyhow::anyhow!(
                "libei input backend did not become ready within 20s; the desktop portal may be waiting for Remote Desktop consent or its EIS session may be wedged"
            ),
            crossbeam_channel::RecvTimeoutError::Disconnected => {
                anyhow::anyhow!("libei worker reply channel closed")
            }
        })?
}

fn restore_token_path() -> Option<PathBuf> {
    let base = dirs::config_dir()?;
    Some(base.join("cua-driver").join("libei-persistent.token"))
}

fn read_restore_token() -> Option<String> {
    let path = restore_token_path()?;
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn write_restore_token(token: &str) -> anyhow::Result<()> {
    let path = restore_token_path()
        .ok_or_else(|| anyhow::anyhow!("no config dir available for libei restore_token"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            anyhow::anyhow!(
                "failed to create {} for restore_token: {e}",
                parent.display()
            )
        })?;
    }
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(&path)
        .map_err(|e| {
            anyhow::anyhow!(
                "failed to open libei restore_token at {}: {e}",
                path.display()
            )
        })?;
    file.write_all(token.as_bytes()).map_err(|e| {
        anyhow::anyhow!(
            "failed to write libei restore_token to {}: {e}",
            path.display()
        )
    })
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

fn wait_until_ready(interface: &'static str) -> anyhow::Result<()> {
    ensure_started()?;
    let (tx_r, rx_r) = bounded(1);
    tx()?
        .send(Cmd::WaitReady {
            interface,
            reply: tx_r,
        })
        .map_err(|e| anyhow::anyhow!("libei worker channel closed: {e}"))?;
    wait_for_reply(rx_r)
}

/// Negotiate a resumed absolute-pointer device without emitting input.
/// Callers can then restore target activation after a portal consent dialog.
pub fn wait_pointer_ready() -> anyhow::Result<()> {
    wait_until_ready("ei_pointer_absolute")
}

/// Negotiate a resumed scroll device without emitting input.
pub fn wait_scroll_ready() -> anyhow::Result<()> {
    wait_until_ready("ei_scroll")
}

/// Negotiate a resumed keyboard device without emitting input.
pub fn wait_keyboard_ready() -> anyhow::Result<()> {
    wait_until_ready("ei_keyboard")
}

// ── Public API ───────────────────────────────────────────────────────────

/// Move the cursor to absolute (x, y) within the announced device region,
/// then press + release `button`. Blocks until the EIS server has
/// received the event sequence.
pub fn click(x: f64, y: f64, button: Button) -> anyhow::Result<()> {
    ensure_started()?;
    let (tx_r, rx_r) = bounded(1);
    tx()?
        .send(Cmd::Click {
            x,
            y,
            button,
            reply: tx_r,
        })
        .map_err(|e| anyhow::anyhow!("libei worker channel closed: {e}"))?;
    wait_for_reply(rx_r)
}

/// Move the cursor to absolute (x, y) inside the device region (no
/// button press).
pub fn move_absolute(x: f64, y: f64) -> anyhow::Result<()> {
    ensure_started()?;
    let (tx_r, rx_r) = bounded(1);
    tx()?
        .send(Cmd::MoveAbsolute { x, y, reply: tx_r })
        .map_err(|e| anyhow::anyhow!("libei worker channel closed: {e}"))?;
    wait_for_reply(rx_r)
}

/// Scroll by (dx, dy) logical units. Positive y scrolls down.
pub fn scroll(dx: f64, dy: f64) -> anyhow::Result<()> {
    ensure_started()?;
    let (tx_r, rx_r) = bounded(1);
    tx()?
        .send(Cmd::Scroll {
            dx,
            dy,
            reply: tx_r,
        })
        .map_err(|e| anyhow::anyhow!("libei worker channel closed: {e}"))?;
    wait_for_reply(rx_r)
}

/// Inject a UTF-8 string via the `ei_text` interface (libei 1.6+).
/// Bypasses keycode reverse-mapping — works correctly for any layout.
pub fn type_text(text: &str) -> anyhow::Result<()> {
    ensure_started()?;
    let (tx_r, rx_r) = bounded(1);
    tx()?
        .send(Cmd::TypeText {
            text: text.to_string(),
            reply: tx_r,
        })
        .map_err(|e| anyhow::anyhow!("libei worker channel closed: {e}"))?;
    wait_for_reply(rx_r)
}

/// Press + release a key by evdev code (e.g. `KEY_ENTER` = 28). For
/// modifier combos use the keyboard interface's modifier state explicitly.
pub fn press_key(keycode: u32) -> anyhow::Result<()> {
    ensure_started()?;
    let (tx_r, rx_r) = bounded(1);
    tx()?
        .send(Cmd::PressKey {
            keycode,
            reply: tx_r,
        })
        .map_err(|e| anyhow::anyhow!("libei worker channel closed: {e}"))?;
    wait_for_reply(rx_r)
}

/// Submit an ordered sequence of evdev key press/release transitions.
///
/// The worker emits every transition in one emulation session and frames each
/// transition separately, allowing callers to hold modifiers while pressing a
/// key. Requests are capped to keep the synchronous worker responsive.
pub fn key_sequence(transitions: &[KeyTransition]) -> anyhow::Result<()> {
    validate_key_sequence(transitions)?;
    if transitions.is_empty() {
        return Ok(());
    }

    ensure_started()?;
    let (tx_r, rx_r) = bounded(1);
    tx()?
        .send(Cmd::KeySequence {
            transitions: transitions.to_vec(),
            reply: tx_r,
        })
        .map_err(|e| anyhow::anyhow!("libei worker channel closed: {e}"))?;
    wait_for_reply(rx_r)
}

fn validate_key_sequence(transitions: &[KeyTransition]) -> anyhow::Result<()> {
    if transitions.len() > MAX_KEY_SEQUENCE_TRANSITIONS {
        anyhow::bail!(
            "libei key sequence has {} transitions; maximum is {}",
            transitions.len(),
            MAX_KEY_SEQUENCE_TRANSITIONS
        );
    }
    Ok(())
}

fn emit_key_transitions(transitions: &[KeyTransition], mut emit: impl FnMut(KeyTransition, u64)) {
    for (frame, transition) in transitions.iter().copied().enumerate() {
        emit(transition, frame as u64);
    }
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
    wait_for_reply(rx_r)
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
/// session — open. The direct `$LIBEI_SOCKET` fast path has no portal
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
    let (context, _portal_keepalive) =
        open_eis_context().map_err(|e| anyhow::anyhow!("failed to obtain EIS connection: {e}"))?;

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

/// Open the EIS context. Fast path: `$LIBEI_SOCKET` supplied by the environment.
/// Fallback: ashpd portal RemoteDesktop handshake.
fn open_eis_context() -> anyhow::Result<(reis::ei::Context, PortalKeepAlive)> {
    if let Some(ctx) = reis::ei::Context::connect_to_env()
        .map_err(|e| anyhow::anyhow!("env ei socket open failed: {e}"))?
    {
        // Direct $LIBEI_SOCKET fast path: no portal session to keep alive.
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
        let connection = super::portal::fresh_session_connection().await?;
        let proxy = RemoteDesktop::with_connection(connection)
            .await
            .map_err(|e| anyhow::anyhow!("portal RemoteDesktop proxy unreachable: {e}. Install xdg-desktop-portal-gnome / xdg-desktop-portal-kde."))?;
        let session = proxy
            .create_session(CreateSessionOptions::default())
            .await
            .map_err(|e| anyhow::anyhow!("portal create_session failed: {e}"))?;

        let mut select_opts = SelectDevicesOptions::default()
            .set_devices(BitFlags::<DeviceType>::from(DeviceType::Keyboard) | DeviceType::Pointer)
            .set_persist_mode(PersistMode::ExplicitlyRevoked);
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
        // 0.13's explicitly-revoked mode and the stored token together
        // skip the dialog for the lifetime of the persistence grant
        // until the user explicitly revokes the grant in desktop settings.
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
    Ok((
        ctx,
        PortalKeepAlive::Portal {
            _rt: rt,
            _proxy: proxy,
            _session: session,
        },
    ))
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
    use calloop::{generic::Generic, EventLoop, Interest, Mode};

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

        // Run each queued command once the device IT needs is negotiated; time
        // stale ones out. Per-command (not one global gate) so a keyboard-only
        // session doesn't make keyboard commands wait 20s for a pointer device.
        if !pending.is_empty() {
            let mut still = Vec::with_capacity(pending.len());
            for (cmd, queued) in pending.drain(..) {
                if state.cmd_ready(&cmd) || queued.elapsed() >= READY_TIMEOUT {
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
    /// The latest serial emitted by the EIS connection. Request serials are
    /// connection-global (libei's `ei_get_serial`), while readiness is tracked
    /// per device below.
    last_serial: u32,
    /// Last device lifecycle event. Mutter 46 may advertise a provisional
    /// absolute pointer before seat binding, then a second live device. Waiting
    /// briefly for this stream to settle avoids dispatching to the transient one.
    last_device_event: Option<std::time::Instant>,
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
    resumed: bool,
    resumed_serial: Option<u32>,
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
                self.last_device_event = Some(std::time::Instant::now());
                if let reis::ei::device::Event::Destroyed { serial } = &ev {
                    self.last_serial = *serial;
                    self.devices.remove(&device);
                    return;
                }
                let data = self.devices.entry(device.clone()).or_default();
                match ev {
                    reis::ei::device::Event::DeviceType { device_type } => {
                        data.device_type = Some(device_type);
                    }
                    reis::ei::device::Event::Interface { object } => {
                        data.interfaces
                            .insert(object.interface().to_owned(), object);
                    }
                    reis::ei::device::Event::Region {
                        offset_x,
                        offset_y,
                        width,
                        hight,
                        scale: _,
                    } => {
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
                        data.resumed = true;
                        data.resumed_serial = Some(serial);
                        self.last_serial = serial;
                        // A RemoteDesktop session is one emulation transaction,
                        // not one transaction per click. Mutter can neutralize a
                        // device stopped in the same batch as its frame. Match
                        // libei's reference client: start on Resumed and keep it
                        // active until the device is paused or disconnected.
                        self.sequence = self.sequence.wrapping_add(1);
                        device.start_emulating(serial, self.sequence);
                    }
                    reis::ei::device::Event::Paused { serial } => {
                        data.resumed = false;
                        data.resumed_serial = None;
                        self.last_serial = serial;
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

        // Sanity-cap the size from the (trusted, but possibly buggy) compositor
        // event: real xkb keymaps are a few KiB; refuse absurd values rather
        // than attempt a multi-GB allocation.
        if size == 0 || size as usize > 16 * 1024 * 1024 {
            return;
        }
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

    /// True once the device THIS command needs has been negotiated and
    /// `Resumed`. Gating per-command (rather than on one global pointer check)
    /// means a keyboard-only session doesn't stall keyboard commands waiting for
    /// an absolute-pointer device that never arrives — and vice versa. Commands
    /// run before their device exists fail with "no EIS device negotiated yet",
    /// so `run_calloop` queues them until this holds (or READY_TIMEOUT elapses).
    fn cmd_ready(&self, cmd: &Cmd) -> bool {
        const DEVICE_QUIET_PERIOD: std::time::Duration = std::time::Duration::from_millis(200);
        if self
            .last_device_event
            .is_none_or(|last| last.elapsed() < DEVICE_QUIET_PERIOD)
        {
            return false;
        }
        match cmd {
            Cmd::WaitReady { interface, .. } => self.device_with_interface(interface).is_some(),
            Cmd::Click { .. } | Cmd::MoveAbsolute { .. } | Cmd::Drag { .. } => {
                self.device_with_interface("ei_pointer_absolute").is_some()
            }
            Cmd::Scroll { .. } => self.device_with_interface("ei_scroll").is_some(),
            Cmd::PressKey { .. } | Cmd::KeySequence { .. } => {
                self.device_with_interface("ei_keyboard").is_some()
            }
            Cmd::TypeText { .. } => {
                self.device_with_interface("ei_text").is_some()
                    || self.device_with_interface("ei_keyboard").is_some()
            }
            Cmd::Shutdown => true,
        }
    }

    fn handle_command(&mut self, cmd: Cmd) {
        let result = self.run_command(&cmd);
        // Send the reply on whichever channel this command carries.
        match cmd {
            Cmd::WaitReady { reply, .. } => {
                let _ = reply.send(result);
            }
            Cmd::Click { reply, .. } => {
                let _ = reply.send(result);
            }
            Cmd::MoveAbsolute { reply, .. } => {
                let _ = reply.send(result);
            }
            Cmd::Scroll { reply, .. } => {
                let _ = reply.send(result);
            }
            Cmd::TypeText { reply, .. } => {
                let _ = reply.send(result);
            }
            Cmd::PressKey { reply, .. } => {
                let _ = reply.send(result);
            }
            Cmd::KeySequence { reply, .. } => {
                let _ = reply.send(result);
            }
            Cmd::Drag { reply, .. } => {
                let _ = reply.send(result);
            }
            Cmd::Shutdown => {}
        }
    }

    fn run_command(&mut self, cmd: &Cmd) -> anyhow::Result<()> {
        match cmd {
            Cmd::WaitReady { interface, .. } => {
                self.device_with_interface(interface).ok_or_else(|| {
                    anyhow::anyhow!("no resumed EIS {interface} device negotiated within 20s")
                })?;
            }
            Cmd::Click { x, y, button, .. } => {
                let (device, rel_x, rel_y) = self.pointer_device_for(*x, *y)?;
                let serial = self.last_serial;
                let ptr_abs =
                    require_device_interface::<reis::ei::PointerAbsolute>(&self.devices, &device)?;
                let btn = require_device_interface::<reis::ei::Button>(&self.devices, &device)?;

                ptr_abs.motion_absolute(rel_x, rel_y);
                btn.button(button.to_evdev(), reis::ei::button::ButtonState::Press);
                device.frame(serial, event_time_us());
                btn.button(button.to_evdev(), reis::ei::button::ButtonState::Released);
                device.frame(serial, event_time_us());
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
                let serial = self.last_serial;
                let ptr_abs =
                    require_device_interface::<reis::ei::PointerAbsolute>(&self.devices, &device)?;
                let btn = require_device_interface::<reis::ei::Button>(&self.devices, &device)?;
                // Map the drag end into the SAME device's region (a single EIS
                // emulation session can't span devices). Fall back to the start
                // region's offset when no region of this device contains the end
                // (single-region device, or a point just off-screen) — this is
                // correct only within one region, but avoids the cross-monitor
                // mis-mapping of blindly reusing the start offset.
                let (to_rx, to_ry) =
                    self.region_local_on(&device, *to_x, *to_y)
                        .unwrap_or_else(|| {
                            let off_x = *from_x as f32 - from_rx;
                            let off_y = *from_y as f32 - from_ry;
                            (*to_x as f32 - off_x, *to_y as f32 - off_y)
                        });

                ptr_abs.motion_absolute(from_rx, from_ry);
                device.frame(serial, event_time_us());
                btn.button(button.to_evdev(), reis::ei::button::ButtonState::Press);
                device.frame(serial, event_time_us());
                // Bound the interpolation: run_command executes synchronously in
                // the worker loop, so a huge step count would block EIS event
                // processing (incl. Ping) and balloon the unflushed request queue.
                let n = (*steps).max(1).min(500);
                for s in 1..=n {
                    let t = s as f32 / n as f32;
                    let ix = from_rx + (to_rx - from_rx) * t;
                    let iy = from_ry + (to_ry - from_ry) * t;
                    ptr_abs.motion_absolute(ix, iy);
                    device.frame(serial, event_time_us());
                }
                btn.button(button.to_evdev(), reis::ei::button::ButtonState::Released);
                device.frame(serial, event_time_us());
            }
            Cmd::MoveAbsolute { x, y, .. } => {
                let (device, rel_x, rel_y) = self.pointer_device_for(*x, *y)?;
                let serial = self.last_serial;
                let ptr_abs =
                    require_device_interface::<reis::ei::PointerAbsolute>(&self.devices, &device)?;

                ptr_abs.motion_absolute(rel_x, rel_y);
                device.frame(serial, event_time_us());
            }
            Cmd::Scroll { dx, dy, .. } => {
                let device = self.device_with_interface("ei_scroll").ok_or_else(|| {
                    anyhow::anyhow!("no EIS scroll device negotiated yet — wait for handshake")
                })?;
                let serial = self.last_serial;
                let scroll = require_device_interface::<reis::ei::Scroll>(&self.devices, &device)?;

                scroll.scroll(*dx as f32, *dy as f32);
                device.frame(serial, event_time_us());
            }
            Cmd::TypeText { text, .. } => {
                // Prefer ei_text (libei 1.6+ — a UTF-8 string, layout-correct).
                // Mutter does NOT advertise ei_text, so fall back to keycode
                // typing via ei_keyboard: map each char to a US-layout evdev
                // keycode + shift and emit press/release. Chars with no US-layout
                // keycode are skipped (a documented limitation — full layout
                // support needs an xkb keymap, like reis's type-text example).
                if let Some(device) = self.device_with_interface("ei_text") {
                    let serial = self.last_serial;
                    let text_iface =
                        require_device_interface::<reis::ei::Text>(&self.devices, &device)?;
                    text_iface.utf8(text);
                    device.frame(serial, event_time_us());
                } else {
                    use reis::ei::keyboard::KeyState;
                    let device = self.device_with_interface("ei_keyboard").ok_or_else(|| {
                        anyhow::anyhow!(
                            "no EIS keyboard device negotiated yet — wait for handshake"
                        )
                    })?;
                    let serial = self.last_serial;
                    let kb =
                        require_device_interface::<reis::ei::Keyboard>(&self.devices, &device)?;
                    let shift_code = self.keymap_shift.unwrap_or(42);
                    let mut skipped = 0usize;
                    for ch in text.chars() {
                        // Prefer the compositor's actual keymap (layout-correct);
                        // fall back to the US-layout table if no keymap arrived.
                        let mapped = self
                            .keymap_chars
                            .get(&ch)
                            .copied()
                            .or_else(|| char_to_evdev(ch));
                        let Some((code, shift)) = mapped else {
                            // No keycode for this char in the keymap/US table
                            // (e.g. CJK/emoji via the keycode path). Skip it, but
                            // don't pretend the whole string was typed.
                            skipped += 1;
                            continue;
                        };
                        if shift {
                            kb.key(shift_code, KeyState::Press);
                            device.frame(serial, event_time_us());
                        }
                        kb.key(code, KeyState::Press);
                        device.frame(serial, event_time_us());
                        kb.key(code, KeyState::Released);
                        device.frame(serial, event_time_us());
                        if shift {
                            kb.key(shift_code, KeyState::Released);
                            device.frame(serial, event_time_us());
                        }
                    }
                    if skipped > 0 {
                        tracing::warn!(
                            "libei keycode typing skipped {skipped} char(s) with no \
                             keycode in the active keymap (e.g. non-Latin/emoji) — \
                             the ei_text path (libei 1.6+) would type these verbatim"
                        );
                    }
                }
            }
            Cmd::PressKey { keycode, .. } => {
                let device = self.device_with_interface("ei_keyboard").ok_or_else(|| {
                    anyhow::anyhow!("no EIS keyboard device negotiated yet — wait for handshake")
                })?;
                let serial = self.last_serial;
                let kb = require_device_interface::<reis::ei::Keyboard>(&self.devices, &device)?;

                kb.key(*keycode, reis::ei::keyboard::KeyState::Press);
                device.frame(serial, event_time_us());
                kb.key(*keycode, reis::ei::keyboard::KeyState::Released);
                device.frame(serial, event_time_us());
            }
            Cmd::KeySequence { transitions, .. } => {
                let device = self.device_with_interface("ei_keyboard").ok_or_else(|| {
                    anyhow::anyhow!("no EIS keyboard device negotiated yet — wait for handshake")
                })?;
                let serial = self.last_serial;
                let kb = require_device_interface::<reis::ei::Keyboard>(&self.devices, &device)?;

                emit_key_transitions(transitions, |transition, _frame| {
                    let (keycode, state) = match transition {
                        KeyTransition::Press(keycode) => {
                            (keycode, reis::ei::keyboard::KeyState::Press)
                        }
                        KeyTransition::Release(keycode) => {
                            (keycode, reis::ei::keyboard::KeyState::Released)
                        }
                    };
                    kb.key(keycode, state);
                    device.frame(serial, event_time_us());
                });
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
            if !data.resumed || !data.interfaces.contains_key("ei_pointer_absolute") {
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
        let device = self
            .device_with_interface("ei_pointer_absolute")
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "no EIS absolute-pointer device negotiated yet — wait for handshake"
                )
            })?;
        Ok((device, x as f32, y as f32))
    }

    /// Region-local coordinates for absolute `(x, y)` within one of `device`'s
    /// announced regions, or `None` when no region of that device contains the
    /// point. Used to map a drag endpoint into the same device the drag start
    /// resolved to (a single EIS emulation session can't span devices).
    fn region_local_on(&self, device: &reis::ei::Device, x: f64, y: f64) -> Option<(f32, f32)> {
        let data = self.devices.get(device)?;
        data.regions
            .iter()
            .find(|r| r.contains(x, y))
            .map(|r| (x as f32 - r.offset_x, y as f32 - r.offset_y))
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
            .filter(|(_, data)| data.resumed && data.interfaces.contains_key(iface))
            .max_by_key(|(_, data)| data.resumed_serial.unwrap_or(0))
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
    devices
        .get(device)?
        .interfaces
        .get(T::NAME)?
        .clone()
        .downcast()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_sequence_accepts_empty_and_maximum_length_requests() {
        assert!(validate_key_sequence(&[]).is_ok());

        let transitions = vec![KeyTransition::Press(29); MAX_KEY_SEQUENCE_TRANSITIONS];
        assert!(validate_key_sequence(&transitions).is_ok());
    }

    #[test]
    fn key_sequence_rejects_requests_over_the_worker_bound() {
        let transitions = vec![KeyTransition::Release(29); MAX_KEY_SEQUENCE_TRANSITIONS + 1];
        let error = validate_key_sequence(&transitions).unwrap_err();

        assert_eq!(
            error.to_string(),
            "libei key sequence has 65 transitions; maximum is 64"
        );
    }

    #[test]
    fn key_sequence_preserves_transition_order_and_assigns_frames() {
        let transitions = [
            KeyTransition::Press(29),
            KeyTransition::Press(46),
            KeyTransition::Release(46),
            KeyTransition::Release(29),
        ];
        let mut emitted = Vec::new();

        emit_key_transitions(&transitions, |transition, frame| {
            emitted.push((transition, frame));
        });

        assert_eq!(
            emitted,
            vec![
                (KeyTransition::Press(29), 0),
                (KeyTransition::Press(46), 1),
                (KeyTransition::Release(46), 2),
                (KeyTransition::Release(29), 3),
            ]
        );
    }
}

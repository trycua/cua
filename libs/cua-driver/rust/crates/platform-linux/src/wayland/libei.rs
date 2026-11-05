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
//!    until EIS acknowledges the preceding input through ei_connection.sync.
//!
//! Persistence: ashpd `PersistMode::ExplicitlyRevoked` keeps the user's consent
//! until they revoke it in desktop settings. The restore token is stored via
//! [`super::portal::RestoreToken::RemoteDesktopInput`] and reused across
//! daemon restarts.
//!
//! Coordinates: ei_pointer_absolute takes desktop-layout positions that must
//! lie inside an announced ei_device.Region — collected between device creation
//! and the first device.Done event. Multi-monitor: the region containing the
//! target (x, y) selects the device; the point is sent unchanged.

use super::region_route::{route_absolute, RegionRect};
use std::collections::HashMap;
use std::sync::OnceLock;
use std::thread;

use crossbeam_channel::{bounded, Receiver, Sender};
use xkbcommon::xkb;

use super::niri::keyboard::{chromium_receives, is_typing_key};
use super::portal::RestoreToken;

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
        duration_ms: u64,
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
    duration_ms: u64,
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
            duration_ms,
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
        if let Some(tok) = RestoreToken::RemoteDesktopInput.read() {
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
            let _ = RestoreToken::RemoteDesktopInput.write(tok);
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

        state.pending_sync.retain(|_, (reply, sent)| {
            if sent.elapsed() < std::time::Duration::from_secs(5) {
                return true;
            }
            let _ = reply.send(Err(anyhow::anyhow!(
                "EIS did not acknowledge input within 5s; delivery is unknown"
            )));
            false
        });

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
    connection: Option<reis::ei::Connection>,
    pending_sync: HashMap<reis::ei::Callback, (Sender<anyhow::Result<()>>, std::time::Instant)>,
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
    keymap_chars: HashMap<char, TypedKey>,
    /// evdev keycode that produces `Shift_L` in the active keymap (else 42).
    keymap_shift: Option<u32>,
    /// evdev keycode that produces `ISO_Level3_Shift` (AltGr), when the keymap has one.
    keymap_level3: Option<u32>,
}

#[derive(Default)]
struct SeatData {
    capabilities: HashMap<String, u64>,
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
            Event::Callback(callback, reis::ei::callback::Event::Done { .. }) => {
                if let Some((reply, _)) = self.pending_sync.remove(&callback) {
                    let _ = reply.send(Ok(()));
                }
            }
            Event::Connection(conn, ev) => {
                self.connection = Some(conn);
                match ev {
                    reis::ei::connection::Event::Seat { seat } => {
                        self.seats.insert(seat, SeatData::default());
                    }
                    // The EIS server pings to check liveness; failing to answer
                    // makes it drop the connection. Mirror reis's examples.
                    reis::ei::connection::Event::Ping { ping } => {
                        ping.done(0);
                    }
                    _ => {}
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

        let (chars, shift, level3) = keymap_char_table(&keymap);
        self.keymap_chars = chars;
        self.keymap_shift = shift;
        self.keymap_level3 = level3;
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
        let needs_sync = !matches!(cmd, Cmd::WaitReady { .. });
        let reply = match cmd {
            Cmd::WaitReady { reply, .. }
            | Cmd::Click { reply, .. }
            | Cmd::MoveAbsolute { reply, .. }
            | Cmd::Scroll { reply, .. }
            | Cmd::TypeText { reply, .. }
            | Cmd::PressKey { reply, .. }
            | Cmd::KeySequence { reply, .. }
            | Cmd::Drag { reply, .. } => reply,
            Cmd::Shutdown => return,
        };
        if result.is_ok() && needs_sync {
            if let (Some(connection), Some(context)) = (&self.connection, &self.context) {
                // A socket flush is not delivery. Hold the caller's focus guard
                // until EIS acknowledges all preceding input frames.
                let callback = connection.sync(1);
                if let Err(error) = context.flush() {
                    let _ = reply.send(Err(error.into()));
                    return;
                }
                self.pending_sync
                    .insert(callback, (reply, std::time::Instant::now()));
            } else {
                let _ = reply.send(Err(anyhow::anyhow!(
                    "EIS connection unavailable for input acknowledgement"
                )));
            }
        } else {
            let _ = reply.send(result);
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
                let device = self.pointer_device_for(*x, *y)?;
                let serial = self.last_serial;
                let ptr_abs =
                    require_device_interface::<reis::ei::PointerAbsolute>(&self.devices, &device)?;
                let btn = require_device_interface::<reis::ei::Button>(&self.devices, &device)?;

                ptr_abs.motion_absolute(*x as f32, *y as f32);
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
                duration_ms,
                button,
                ..
            } => {
                let device = self.pointer_device_for(*from_x, *from_y)?;
                let serial = self.last_serial;
                let ptr_abs =
                    require_device_interface::<reis::ei::PointerAbsolute>(&self.devices, &device)?;
                let btn = require_device_interface::<reis::ei::Button>(&self.devices, &device)?;
                // A single EIS emulation session can't span devices: the end
                // must lie in a region of the device the start resolved to.
                let regions = self
                    .devices
                    .get(&device)
                    .map(|d| d.regions.as_slice())
                    .unwrap_or_default();
                route_absolute([((), regions)], *to_x, *to_y)?;
                let (from_rx, from_ry) = (*from_x as f32, *from_y as f32);
                let (to_rx, to_ry) = (*to_x as f32, *to_y as f32);

                ptr_abs.motion_absolute(from_rx, from_ry);
                device.frame(serial, event_time_us());
                btn.button(button.to_evdev(), reis::ei::button::ButtonState::Press);
                device.frame(serial, event_time_us());
                if let Some(context) = self.context.as_ref() {
                    let _ = context.flush();
                }
                // Bound the interpolation: run_command executes synchronously in
                // the worker loop, so a huge step count would block EIS event
                // processing (incl. Ping) and balloon the unflushed request queue.
                let n = (*steps).max(1).min(500);
                let delay = drag_step_delay(*duration_ms, n);
                for s in 1..=n {
                    let t = s as f32 / n as f32;
                    let ix = from_rx + (to_rx - from_rx) * t;
                    let iy = from_ry + (to_ry - from_ry) * t;
                    ptr_abs.motion_absolute(ix, iy);
                    device.frame(serial, event_time_us());
                    if let Some(context) = self.context.as_ref() {
                        let _ = context.flush();
                    }
                    if !delay.is_zero() {
                        crate::interaction::pause(delay);
                    }
                }
                btn.button(button.to_evdev(), reis::ei::button::ButtonState::Released);
                device.frame(serial, event_time_us());
            }
            Cmd::MoveAbsolute { x, y, .. } => {
                let device = self.pointer_device_for(*x, *y)?;
                let serial = self.last_serial;
                let ptr_abs =
                    require_device_interface::<reis::ei::PointerAbsolute>(&self.devices, &device)?;

                ptr_abs.motion_absolute(*x as f32, *y as f32);
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
                // keycode + modifiers and emit press/release. A string with any
                // character the keymap cannot produce is refused before the first
                // key, so the field never receives a silently shortened string.
                let text_device = self.device_with_interface("ei_text").filter(|_| {
                    self.device_with_interface("ei_keyboard").is_none()
                        || prefers_ei_text(text, &self.keymap_chars, self.keymap_level3)
                });
                if let Some(device) = text_device {
                    let serial = self.last_serial;
                    let text_iface =
                        require_device_interface::<reis::ei::Text>(&self.devices, &device)?;
                    let keyboard = self.device_with_interface("ei_keyboard").and_then(|kbd| {
                        let kb =
                            require_device_interface::<reis::ei::Keyboard>(&self.devices, &kbd)
                                .ok()?;
                        Some((kbd, kb))
                    });
                    if let Some((kbd, kb)) = keyboard {
                        use reis::ei::keyboard::KeyState;
                        const KEY_ENTER: u32 = 28;
                        // Text and keys reach an app by separate paths, so each
                        // part gets time to land before the next one is sent.
                        const PART_SETTLE: std::time::Duration =
                            std::time::Duration::from_millis(50);
                        for part in text_parts(text) {
                            match part {
                                TextPart::Text(line) => {
                                    text_iface.utf8(&line);
                                    device.frame(serial, event_time_us());
                                }
                                TextPart::Enter => {
                                    kb.key(KEY_ENTER, KeyState::Press);
                                    kbd.frame(serial, event_time_us());
                                    kb.key(KEY_ENTER, KeyState::Released);
                                    kbd.frame(serial, event_time_us());
                                }
                            }
                            if let Some(context) = &self.context {
                                let _ = context.flush();
                            }
                            std::thread::sleep(PART_SETTLE);
                        }
                    } else {
                        text_iface.utf8(text);
                        device.frame(serial, event_time_us());
                    }
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
                    let level3_code = self.keymap_level3;
                    let plan = plan_keycode_typing(text, &self.keymap_chars, level3_code)?;
                    let press = |code: u32, state: KeyState| {
                        kb.key(code, state);
                        device.frame(serial, event_time_us());
                    };
                    for key in plan {
                        if key.shift {
                            press(shift_code, KeyState::Press);
                        }
                        if key.level3 {
                            if let Some(code) = level3_code {
                                press(code, KeyState::Press);
                            }
                        }
                        press(key.code, KeyState::Press);
                        press(key.code, KeyState::Released);
                        if key.level3 {
                            if let Some(code) = level3_code {
                                press(code, KeyState::Released);
                            }
                        }
                        if key.shift {
                            press(shift_code, KeyState::Released);
                        }
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

    /// Pick the absolute-pointer device whose announced Region contains
    /// `(x, y)`. The region only selects the device: coordinates are
    /// desktop-layout positions and are sent to EIS unchanged. When regions
    /// were announced but none contains the point this is an error, since EIS
    /// would silently drop the motion. Only when no region was announced at all
    /// does it fall back to the first absolute-pointer device.
    fn pointer_device_for(&self, x: f64, y: f64) -> anyhow::Result<reis::ei::Device> {
        // Mutter/KWin split the relative and absolute pointers onto separate
        // devices, and only the absolute one accepts an absolute warp.
        let candidates = self
            .devices
            .iter()
            .filter(|(_, data)| data.resumed && data.interfaces.contains_key("ei_pointer_absolute"))
            .map(|(device, data)| (device, data.regions.as_slice()));
        if let Some(device) = route_absolute(candidates, x, y)? {
            return Ok(device.clone());
        }
        self.device_with_interface("ei_pointer_absolute")
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "no EIS absolute-pointer device negotiated yet — wait for handshake"
                )
            })
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

fn drag_step_delay(duration_ms: u64, steps: u32) -> std::time::Duration {
    let frames = u64::from(steps.max(1).min(500));
    std::time::Duration::from_micros(
        duration_ms
            .min(10_000)
            .saturating_mul(1_000)
            .checked_div(frames)
            .unwrap_or(0),
    )
}

/// One key press that yields a character on the active layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TypedKey {
    code: u32,
    shift: bool,
    level3: bool,
}

/// Index the compositor keymap's first layout by character.
///
/// Real keymaps also put characters on keypad and media keys: `us` has `$` on
/// `KEY_DOLLAR` and `(` on `KEY_KPLEFTPAREN`. X11 apps and Chromium drop some of
/// those, and terminals can read keypad keys as commands. So only keys Chromium
/// receives count, the main block beats the rest whatever modifiers it needs, and
/// then the fewest modifiers win. A character left with no such key is absent
/// from the table, so typing refuses it rather than losing it in some apps.
fn keymap_char_table(keymap: &xkb::Keymap) -> (HashMap<char, TypedKey>, Option<u32>, Option<u32>) {
    const XKB_EVDEV_OFFSET: u32 = 8;
    const SHIFT_L: u32 = 0xffe1;
    const ISO_LEVEL3_SHIFT: u32 = 0xfe03;
    let mut chars = HashMap::<char, ((bool, u32), TypedKey)>::new();
    let mut shift = None;
    let mut level3: Option<u32> = None;
    for kc in keymap.min_keycode().raw()..=keymap.max_keycode().raw() {
        let keycode = xkb::Keycode::new(kc);
        let evdev = kc.wrapping_sub(XKB_EVDEV_OFFSET);
        for sym in keymap.key_get_syms_by_level(keycode, 0, 0) {
            if shift.is_none() && sym.raw() == SHIFT_L {
                shift = Some(evdev);
            }
            if sym.raw() == ISO_LEVEL3_SHIFT && !level3.is_some_and(chromium_receives) {
                level3 = Some(evdev);
            }
        }
        if !chromium_receives(evdev) {
            continue;
        }
        for level in 0..=3u32 {
            for sym in keymap.key_get_syms_by_level(keycode, 0, level) {
                let Some(ch) = sym.key_char() else { continue };
                let rank = (!is_typing_key(evdev), level);
                let key = TypedKey {
                    code: evdev,
                    shift: level & 1 == 1,
                    level3: level >= 2,
                };
                let slot = chars.entry(ch).or_insert((rank, key));
                if rank < slot.0 {
                    *slot = (rank, key);
                }
            }
        }
    }
    let mut chars: HashMap<char, TypedKey> =
        chars.into_iter().map(|(ch, (_, key))| (ch, key)).collect();
    // `Linefeed` has its own key, which most apps ignore. A newline means Return.
    if let Some(enter) = chars.get(&'\r').copied() {
        chars.insert('\n', enter);
    }
    (chars, shift, level3)
}

/// Resolve every character before any key is sent, so a string the keymap
/// cannot produce is refused whole instead of arriving with characters missing.
/// Control characters and a compositor that announced no keymap fall back to
/// the fixed US table; a printable character on a known keymap never does,
/// because on that layout the US key would produce a different character.
fn plan_keycode_typing(
    text: &str,
    table: &HashMap<char, TypedKey>,
    level3: Option<u32>,
) -> anyhow::Result<Vec<TypedKey>> {
    let mut plan = Vec::new();
    let mut missing = String::new();
    for ch in text.replace("\r\n", "\n").chars() {
        let key = table.get(&ch).copied().or_else(|| {
            (table.is_empty() || ch.is_control())
                .then(|| char_to_evdev(ch))
                .flatten()
                .map(|(code, shift)| TypedKey {
                    code,
                    shift,
                    level3: false,
                })
        });
        match key {
            Some(key) if !key.level3 || level3.is_some() => plan.push(key),
            _ if missing.contains(ch) => {}
            _ => missing.push(ch),
        }
    }
    anyhow::ensure!(
        missing.is_empty(),
        "the active keyboard layout has no key for {missing:?}. Nothing was typed. \
         Use set_value on the element, or clipboard_write and then hotkey ctrl+v."
    );
    Ok(plan)
}

/// Whether `text` goes through `ei_text` rather than as key presses. `ei_text` is
/// layout independent, but it reaches apps by a different path from key presses
/// and the two can swap, so text with a line break goes as keys when it can.
fn prefers_ei_text(text: &str, table: &HashMap<char, TypedKey>, level3: Option<u32>) -> bool {
    !text.contains(['\n', '\r']) || plan_keycode_typing(text, table, level3).is_err()
}

/// A piece of text for the `ei_text` route.
#[derive(Clone, Debug, PartialEq, Eq)]
enum TextPart {
    Text(String),
    Enter,
}

/// Cut `text` at its line endings. KWin turns a newline sent as text into the
/// `Linefeed` key, which X11 apps ignore, so those go as presses of Return.
fn text_parts(text: &str) -> Vec<TextPart> {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut parts = Vec::new();
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            parts.push(TextPart::Enter);
        }
        if !line.is_empty() {
            parts.push(TextPart::Text(line.to_string()));
        }
    }
    parts
}

#[cfg(test)]
pub(super) mod local_eis {
    use super::*;
    use reis::enumflags2::BitFlags;
    use reis::handshake::EisHandshaker;
    use reis::request::{DeviceCapability, EisRequest, EisRequestConverter};
    use reis::PendingRequestResult;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Debug, PartialEq)]
    pub(in crate::wayland) enum Emitted {
        Motion(String, f32, f32),
        Button(u32, bool),
        Scroll(f32, f32),
    }

    const DEVICES: [(&str, &[u32]); 2] = [("wide", &[0, 3840]), ("far", &[10_000])];

    type Log = Arc<Mutex<Vec<Emitted>>>;

    struct Server {
        ctx: reis::eis::Context,
        handshaker: Option<EisHandshaker>,
        converter: Option<EisRequestConverter>,
        devices: Vec<reis::request::Device>,
        log: Log,
    }

    impl Server {
        fn pump(&mut self) {
            if self.ctx.read().is_err() {
                return;
            }
            while let Some(PendingRequestResult::Request(request)) = self.ctx.pending_request() {
                if let Some(handshaker) = self.handshaker.as_mut() {
                    if let Some(resp) = handshaker.handle_request(request).unwrap() {
                        let converter = EisRequestConverter::new(&self.ctx, resp, 1);
                        let _ = converter.handle().add_seat(
                            Some("seat"),
                            DeviceCapability::PointerAbsolute
                                | DeviceCapability::Button
                                | DeviceCapability::Scroll,
                        );
                        self.converter = Some(converter);
                        self.handshaker = None;
                    }
                    continue;
                }
                let converter = self.converter.as_mut().unwrap();
                converter.handle_request(request).unwrap();
                let requests: Vec<_> = std::iter::from_fn(|| converter.next_request()).collect();
                for request in requests {
                    self.record(request);
                }
            }
            let _ = self.ctx.flush();
        }

        fn record(&mut self, request: EisRequest) {
            let mut log = self.log.lock().unwrap();
            match request {
                EisRequest::Bind(bind) => {
                    for (name, offsets) in DEVICES {
                        let device = bind.seat.add_device(
                            Some(name),
                            reis::eis::device::DeviceType::Virtual,
                            BitFlags::from(DeviceCapability::PointerAbsolute)
                                | DeviceCapability::Button
                                | DeviceCapability::Scroll,
                            |device| {
                                for &x in offsets {
                                    device.device().region(x, 0, 3840, 2160, 1.0);
                                }
                            },
                        );
                        device.resumed();
                        self.devices.push(device);
                    }
                }
                EisRequest::PointerMotionAbsolute(m) => log.push(Emitted::Motion(
                    m.device.name().unwrap_or_default().to_owned(),
                    m.dx_absolute,
                    m.dy_absolute,
                )),
                EisRequest::Button(b) => log.push(Emitted::Button(
                    b.button,
                    b.state == reis::eis::button::ButtonState::Press,
                )),
                EisRequest::ScrollDelta(s) => log.push(Emitted::Scroll(s.dx, s.dy)),
                _ => {}
            }
        }
    }

    struct Fixture {
        log: Log,
        gate: Mutex<()>,
    }

    fn install() -> &'static Fixture {
        use std::os::unix::net::UnixStream;
        static FIXTURE: OnceLock<Fixture> = OnceLock::new();
        FIXTURE.get_or_init(|| {
            let log = Log::default();
            let (tx, rx) = bounded::<Cmd>(64);
            assert!(TX.set(tx).is_ok(), "real libei worker already started");
            let (client_end, server_end) = UnixStream::pair().unwrap();
            let mut client = reis::ei::Context::new(client_end).unwrap();
            let ctx = reis::eis::Context::new(server_end).unwrap();
            let handshaker = EisHandshaker::new(&ctx, 1);
            let mut server = Server {
                ctx,
                handshaker: Some(handshaker),
                converter: None,
                devices: Vec::new(),
                log: log.clone(),
            };
            thread::spawn(move || {
                let mut state = EisState::default();
                let _ = client.handshake();
                let _ = client.flush();
                loop {
                    server.pump();
                    let _ = state.handle_eis_readable(&mut client);
                    let resumed = state.devices.values().filter(|d| d.resumed).count();
                    if resumed == DEVICES.len() {
                        match rx.try_recv() {
                            Ok(Cmd::Shutdown)
                            | Err(crossbeam_channel::TryRecvError::Disconnected) => break,
                            Ok(cmd) => state.handle_command(cmd),
                            Err(_) => {}
                        }
                    }
                    state
                        .pending_sync
                        .retain(|_, (_, sent)| sent.elapsed().as_secs() < 5);
                    thread::sleep(std::time::Duration::from_millis(1));
                }
            });
            Fixture {
                log,
                gate: Mutex::new(()),
            }
        })
    }

    /// Run `f` against a local EIS server and return what the server received
    /// from it. A trailing barrier motion guarantees events from commands that
    /// failed early are not still in flight when the log is read.
    pub(in crate::wayland) fn record<R>(f: impl FnOnce() -> R) -> (R, Vec<Emitted>) {
        let fixture = install();
        let _gate = fixture.gate.lock().unwrap_or_else(|e| e.into_inner());
        fixture.log.lock().unwrap().clear();
        let result = f();
        move_absolute(1.0, 1.0).unwrap();
        let mut events = std::mem::take(&mut *fixture.log.lock().unwrap());
        assert!(matches!(events.pop(), Some(Emitted::Motion(_, x, y)) if x == 1.0 && y == 1.0));
        (result, events)
    }
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
    fn drag_duration_is_distributed_across_motion_frames() {
        let delay = drag_step_delay(500, 30);
        assert_eq!(delay, std::time::Duration::from_micros(16_666));
        assert!(delay.saturating_mul(30) <= std::time::Duration::from_millis(500));
        assert_eq!(drag_step_delay(0, 30), std::time::Duration::from_millis(0));
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

    use local_eis::{record, Emitted};

    fn motion(device: &str, x: f32, y: f32) -> Emitted {
        Emitted::Motion(device.into(), x, y)
    }

    const LEFT: u32 = 0x110;

    #[test]
    fn click_on_second_monitor_sends_layout_coordinate_unchanged() {
        let (result, events) = record(|| click(6237.0, 1014.0, Button::Left));
        result.unwrap();
        assert_eq!(
            events,
            vec![
                motion("wide", 6237.0, 1014.0),
                Emitted::Button(LEFT, true),
                Emitted::Button(LEFT, false),
            ]
        );
    }

    #[test]
    fn click_routes_to_the_device_owning_the_region() {
        let (result, events) = record(|| click(11_000.0, 50.0, Button::Left));
        result.unwrap();
        assert_eq!(events[0], motion("far", 11_000.0, 50.0));
    }

    #[test]
    fn move_sends_layout_coordinate_unchanged_and_origin_literally() {
        let (result, events) = record(|| move_absolute(6237.0, 1014.0));
        result.unwrap();
        assert_eq!(events, vec![motion("wide", 6237.0, 1014.0)]);

        let (result, events) = record(|| move_absolute(0.0, 0.0));
        result.unwrap();
        assert_eq!(events, vec![motion("wide", 0.0, 0.0)]);
    }

    #[test]
    fn drag_across_regions_of_one_device_holds_button_over_unchanged_endpoints() {
        let (result, events) = record(|| drag(0.0, 0.0, 6237.0, 1014.0, 4, 0, Button::Left));
        result.unwrap();
        assert_eq!(events.first(), Some(&motion("wide", 0.0, 0.0)));
        assert_eq!(events[1], Emitted::Button(LEFT, true));
        assert_eq!(events[events.len() - 2], motion("wide", 6237.0, 1014.0));
        assert_eq!(events.last(), Some(&Emitted::Button(LEFT, false)));
        assert_eq!(events.len(), 2 + 1 + 4 + 1 - 1);
    }

    #[test]
    fn drag_endpoint_outside_selected_device_fails_before_any_event() {
        let (result, events) = record(|| drag(100.0, 100.0, 11_000.0, 100.0, 4, 0, Button::Left));
        assert_eq!(
            result.unwrap_err().to_string(),
            "libei: (11000, 100) lies outside every announced pointer region"
        );
        assert_eq!(events, vec![]);
    }

    #[test]
    fn pointer_commands_outside_every_region_fail_before_any_event() {
        let (result, events) = record(|| {
            (
                click(8000.0, 100.0, Button::Left),
                move_absolute(8000.0, 100.0),
                drag(8000.0, 100.0, 100.0, 100.0, 4, 0, Button::Left),
            )
        });
        for r in [result.0, result.1, result.2] {
            assert_eq!(
                r.unwrap_err().to_string(),
                "libei: (8000, 100) lies outside every announced pointer region"
            );
        }
        assert_eq!(events, vec![]);
    }
}

#[cfg(test)]
mod keycode_typing_tests {
    use super::*;

    // Same shape as `us(altgr-intl)`: n/N, ntilde/Ntilde on one key, and the
    // keypad and media keys the real keymap also gives `$ ( * + \\n`.
    const KEYMAP: &str = r#"xkb_keymap {
      xkb_keycodes { minimum = 8; maximum = 500;
        <AB06> = 57; <AC01> = 38; <AE04> = 13; <AE05> = 14; <AE08> = 17; <AE09> = 18; <AE12> = 21;
        <RTRN> = 36; <LFSH> = 50; <KPMU> = 63; <KPAD> = 86; <LVL3> = 92; <RALT> = 108; <LNFD> = 109;
        <I126> = 126; <I187> = 187; <I442> = 442; <I443> = 443; <I444> = 444; };
      xkb_types { virtual_modifiers LevelThree;
        type "ONE_LEVEL" { modifiers = none; level_name[Level1] = "1"; };
        type "FOUR_LEVEL" { modifiers = Shift+LevelThree;
          map[None] = Level1; map[Shift] = Level2; map[LevelThree] = Level3; map[Shift+LevelThree] = Level4;
          level_name[Level1] = "1"; level_name[Level2] = "2"; level_name[Level3] = "3"; level_name[Level4] = "4"; };
      };
      xkb_compatibility { };
      xkb_symbols {
        key <AB06> { type = "FOUR_LEVEL", [ n, N, ntilde, Ntilde ] };
        key <AC01> { type = "FOUR_LEVEL", [ a, A, aacute, Aacute ] };
        key <AE04> { type = "FOUR_LEVEL", [ 4, dollar, currency, onequarter ] };
        key <AE05> { type = "FOUR_LEVEL", [ 5, percent, EuroSign, cent ] };
        key <AE08> { type = "FOUR_LEVEL", [ 8, asterisk, onehalf, threequarters ] };
        key <AE09> { type = "FOUR_LEVEL", [ 9, parenleft, leftsinglequotemark, rightsinglequotemark ] };
        key <AE12> { type = "FOUR_LEVEL", [ equal, plus, multiply, division ] };
        key <RTRN> { type = "ONE_LEVEL", [ Return ] };
        key <LFSH> { type = "ONE_LEVEL", [ Shift_L ] };
        key <KPMU> { type = "ONE_LEVEL", [ KP_Multiply ] };
        key <KPAD> { type = "ONE_LEVEL", [ KP_Add ] };
        key <LVL3> { type = "ONE_LEVEL", [ ISO_Level3_Shift ] };
        key <RALT> { type = "ONE_LEVEL", [ ISO_Level3_Shift ] };
        key <LNFD> { type = "ONE_LEVEL", [ Linefeed ] };
        key <I126> { type = "ONE_LEVEL", [ plusminus ] };
        key <I187> { type = "ONE_LEVEL", [ parenleft ] };
        key <I442> { type = "ONE_LEVEL", [ dollar ] };
        key <I443> { type = "ONE_LEVEL", [ EuroSign ] };
        key <I444> { type = "ONE_LEVEL", [ sterling ] };
      };
    };"#;

    fn table() -> (HashMap<char, TypedKey>, Option<u32>, Option<u32>) {
        let ctx = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let km = xkb::Keymap::new_from_string(
            &ctx,
            KEYMAP.to_string(),
            xkb::KEYMAP_FORMAT_TEXT_V1,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .expect("test keymap compiles");
        keymap_char_table(&km)
    }

    #[test]
    fn altgr_characters_are_typable() {
        let (t, _, level3) = table();
        assert_eq!(level3, Some(100));
        assert_eq!(
            t.get(&'ñ').copied(),
            Some(TypedKey {
                code: 49,
                shift: false,
                level3: true
            })
        );
        assert_eq!(
            t.get(&'Ñ').copied(),
            Some(TypedKey {
                code: 49,
                shift: true,
                level3: true
            })
        );
        assert_eq!(
            t.get(&'€').copied(),
            Some(TypedKey {
                code: 6,
                shift: false,
                level3: true
            })
        );
    }

    #[test]
    fn a_character_reachable_without_altgr_never_uses_it() {
        let (t, _, _) = table();
        assert_eq!(
            t.get(&'n').copied(),
            Some(TypedKey {
                code: 49,
                shift: false,
                level3: false
            })
        );
        assert_eq!(
            t.get(&'N').copied(),
            Some(TypedKey {
                code: 49,
                shift: true,
                level3: false
            })
        );
    }

    #[test]
    fn an_untypable_character_refuses_the_whole_string_and_names_it() {
        let (t, _, level3) = table();
        let err = plan_keycode_typing("añ🙂", &t, level3).expect_err("emoji has no key");
        let msg = err.to_string();
        assert!(msg.contains('🙂'), "{msg}");
        assert!(msg.contains("Nothing was typed"), "{msg}");
    }

    #[test]
    fn a_main_key_with_modifiers_beats_any_other_key_without_them() {
        let (t, _, _) = table();
        let key = |code, shift, level3| {
            Some(TypedKey {
                code,
                shift,
                level3,
            })
        };
        assert_eq!(t.get(&'$').copied(), key(5, true, false));
        assert_eq!(t.get(&'(').copied(), key(10, true, false));
        assert_eq!(t.get(&'*').copied(), key(9, true, false));
        assert_eq!(t.get(&'+').copied(), key(13, true, false));
        assert_eq!(t.get(&'€').copied(), key(6, false, true));
    }

    #[test]
    fn a_windows_line_ending_is_one_return() {
        let (t, _, level3) = table();
        let plan = plan_keycode_typing("a\r\nn", &t, level3).expect("typable");
        assert_eq!(plan.len(), 3);
        assert_eq!(plan[1].code, 28);
    }

    #[test]
    fn a_line_break_goes_as_keys_when_the_layout_can_type_it_all() {
        let (t, _, level3) = table();
        assert!(prefers_ei_text("añ", &t, level3));
        assert!(!prefers_ei_text("a\nñ", &t, level3));
        assert!(!prefers_ei_text("a\r\nñ", &t, level3));
        assert!(prefers_ei_text("日\nñ", &t, level3));
    }

    #[test]
    fn text_is_cut_into_strings_and_returns() {
        use TextPart::{Enter, Text};
        assert_eq!(text_parts("añ"), vec![Text("añ".into())]);
        assert_eq!(
            text_parts("a\n\nb\r\nc\n"),
            vec![
                Text("a".into()),
                Enter,
                Enter,
                Text("b".into()),
                Enter,
                Text("c".into()),
                Enter
            ]
        );
        assert_eq!(text_parts(""), Vec::new());
    }

    #[test]
    fn a_newline_presses_return() {
        let (t, _, _) = table();
        let ret = t
            .get(&'\r')
            .copied()
            .expect("Return types a carriage return");
        assert_eq!(ret.code, 28);
        assert_eq!(t.get(&'\n').copied(), Some(ret));
    }

    #[test]
    fn altgr_is_the_key_every_app_receives() {
        let (_, _, level3) = table();
        assert_eq!(level3, Some(100));
    }

    #[test]
    fn a_character_only_on_a_key_apps_drop_is_refused() {
        let (t, _, level3) = table();
        for ch in ['£', '±'] {
            assert!(
                plan_keycode_typing(&ch.to_string(), &t, level3).is_err(),
                "{ch} can only be typed on a key X11 apps or Chromium drop"
            );
        }
    }

    #[test]
    fn altgr_characters_refuse_when_the_keymap_has_no_altgr_key() {
        let (t, _, _) = table();
        assert!(plan_keycode_typing("ñ", &t, None).is_err());
    }
}

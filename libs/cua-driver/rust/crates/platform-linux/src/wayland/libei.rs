// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

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
//!    Text goes out one acknowledged step at a time from the worker loop, which
//!    keeps serving EIS meanwhile.
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
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, Receiver, Sender};
use xkbcommon::xkb;

use super::kwin_keystate::{lock_keys, LockKeys};
use super::portal::RestoreToken;
use super::text_keys::{
    chromium_receives, find_key, modifier_chords, refused, text_sym, with_line_feeds, EVDEV_OFFSET,
};
use crate::input::action::Action;

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
        desktop: DesktopFacts,
        ticket: Arc<TextTicket>,
        /// The tool call this text serves: nothing goes out once it is over.
        action: Option<Arc<Action>>,
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

/// How long a caller waits for the worker to take up a command. A portal consent
/// dialog can hold it that long.
const REPLY_WAIT: Duration = Duration::from_secs(20);
const NOT_READY: &str = "libei input backend did not become ready within 20s; the desktop portal may be waiting for Remote Desktop consent or its EIS session may be wedged";

fn wait_for_reply(rx: Receiver<anyhow::Result<()>>) -> anyhow::Result<()> {
    rx.recv_timeout(REPLY_WAIT).map_err(|error| match error {
        crossbeam_channel::RecvTimeoutError::Timeout => anyhow::anyhow!(NOT_READY),
        crossbeam_channel::RecvTimeoutError::Disconnected => {
            anyhow::anyhow!("libei worker reply channel closed")
        }
    })?
}

/// A queued text between its caller and the worker, so a caller never reports a
/// failure while the text still goes out. Text takes as long as it takes, so the
/// caller gives up only when the worker has stopped moving: on text it never
/// started, or after a whole wait with no step acknowledged.
#[derive(Default)]
struct TextTicket(AtomicU64);

impl TextTicket {
    const QUEUED: u64 = 0;
    const ABANDONED: u64 = u64::MAX;

    /// The worker claims the text. False once its caller has given up.
    fn begin(&self) -> bool {
        self.0
            .compare_exchange(Self::QUEUED, 1, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    fn advance(&self) {
        let _ = self
            .0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |progress| {
                (progress != Self::ABANDONED).then(|| progress + 1)
            });
    }

    fn progress(&self) -> u64 {
        self.0.load(Ordering::Acquire)
    }

    fn is_abandoned(&self) -> bool {
        self.progress() == Self::ABANDONED
    }

    /// Stop the text. True when the worker never started it.
    fn abandon(&self) -> bool {
        self.0.swap(Self::ABANDONED, Ordering::AcqRel) == Self::QUEUED
    }
}

/// Wait for a text's reply. `wait` is how long the worker may go without moving.
fn wait_for_text(
    rx: &Receiver<anyhow::Result<()>>,
    ticket: &TextTicket,
    wait: Duration,
) -> anyhow::Result<()> {
    let mut seen = ticket.progress();
    loop {
        match rx.recv_timeout(wait) {
            Ok(result) => return result,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                anyhow::bail!("libei worker reply channel closed")
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
        }
        let progress = ticket.progress();
        if progress != seen && progress != TextTicket::QUEUED {
            seen = progress;
            continue;
        }
        if ticket.abandon() {
            return Err(refused(NOT_READY));
        }
        anyhow::bail!(
            "type_text stopped: the libei worker stopped moving, so part of the text may have \
             gone out and no more of it will. Read the field before retrying"
        );
    }
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

/// Type `text` into whatever has keyboard focus: exactly, or refused with
/// nothing typed. It goes as key presses on the active layout, or through
/// `ei_text` (libei 1.6+) when that is the only way and no X11 app has focus.
/// Returns once all of it has been sent and acknowledged, however long that takes.
pub fn type_text(text: &str) -> anyhow::Result<()> {
    ensure_started()?;
    send_text(tx()?, text, DesktopFacts::read(), REPLY_WAIT)
}

fn send_text(
    worker: &Sender<Cmd>,
    text: &str,
    desktop: DesktopFacts,
    wait: Duration,
) -> anyhow::Result<()> {
    let (tx_r, rx_r) = bounded(1);
    let ticket = Arc::new(TextTicket::default());
    worker
        .send(Cmd::TypeText {
            text: text.to_string(),
            desktop,
            ticket: ticket.clone(),
            action: crate::input::action::current(),
            reply: tx_r,
        })
        .map_err(|e| anyhow::anyhow!("libei worker channel closed: {e}"))?;
    wait_for_text(&rx_r, &ticket, wait)
}

/// What decides how a text goes out that EIS doesn't say, read by the caller so a
/// slow answer can't stall the worker.
#[derive(Clone, Copy, Debug, Default)]
struct DesktopFacts {
    /// Whether an X11 app has or may have keyboard focus.
    x11_app_may_have_focus: bool,
    /// The active layout, from the compositor itself.
    active_layout: Option<u32>,
    /// The lock keys, from the compositor itself, when it says.
    lock_keys: Option<LockKeys>,
}

impl DesktopFacts {
    fn read() -> Self {
        let keyboard_known = EIS_REPORTS_KEYBOARD.load(Ordering::Acquire);
        Self {
            x11_app_may_have_focus: x11_app_may_have_focus(),
            active_layout: (!keyboard_known)
                .then(super::kwin_helper::active_layout)
                .flatten(),
            lock_keys: (!keyboard_known).then(lock_keys).flatten(),
        }
    }
}

/// Set once EIS has reported the keyboard's state, which it then keeps current,
/// so a caller needn't ask the compositor.
static EIS_REPORTS_KEYBOARD: AtomicBool = AtomicBool::new(false);

/// The time the worker spends between the parts of `text`, at most, for sizing
/// the budget of the tool call that types it.
pub fn text_pacing(text: &str) -> Duration {
    let parts = u32::try_from(text_parts(text).len()).unwrap_or(u32::MAX);
    PART_SETTLE.saturating_mul(2).saturating_mul(parts)
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
    // longer than REPLY_WAIT without running them: their caller has given up, so
    // input sent now would be a surprise, and a retry would send it twice.
    let mut pending: Vec<(Cmd, std::time::Instant)> = Vec::new();

    loop {
        match rx.try_recv() {
            Ok(Cmd::Shutdown) => break,
            Ok(cmd) => pending.push((cmd, std::time::Instant::now())),
            Err(crossbeam_channel::TryRecvError::Empty) => {}
            Err(crossbeam_channel::TryRecvError::Disconnected) => break,
        }

        event_loop
            .dispatch(Some(state.poll_timeout()), &mut state)
            .map_err(|e| anyhow::anyhow!("calloop dispatch failed: {e}"))?;

        // Run each queued command once the device IT needs is negotiated; time
        // stale ones out. Per-command (not one global gate) so a keyboard-only
        // session doesn't make keyboard commands wait 20s for a pointer device.
        // Input stays in order, so nothing starts while a text is going out.
        if pending
            .iter()
            .any(|(cmd, _)| matches!(cmd, Cmd::TypeText { .. }))
        {
            state.probe_keyboard();
        }
        if !pending.is_empty() {
            let mut still = Vec::with_capacity(pending.len());
            for (cmd, queued) in pending.drain(..) {
                if state.text_job.is_some() {
                    still.push((cmd, queued));
                } else if queued.elapsed() >= REPLY_WAIT {
                    state.expire_command(cmd);
                } else if state.cmd_ready(&cmd) {
                    state.handle_command(cmd);
                } else {
                    still.push((cmd, queued));
                }
            }
            pending = still;
        }

        state.advance_text();

        state.pending_sync.retain(|_, (reply, sent)| {
            if sent.elapsed() < ACK_WAIT {
                return true;
            }
            let _ = reply.send(Err(anyhow::anyhow!(NOT_ACKNOWLEDGED)));
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
    /// The compositor's xkb keymap (delivered by the `ei_keyboard` device), so
    /// key typing follows the user's real layout. `None` until the `Keymap` event
    /// arrives, and typing keys is refused until it does.
    keymap: Option<xkb::Keymap>,
    /// The compositor's keyboard state, from the `ei_keyboard` `Modifiers` event.
    /// KWin sends it only when it changes, and before 6.8 not at all; Mutter sends
    /// it when the device is created.
    keyboard: Option<KeyboardState>,
    /// A Shift tap asking a compositor that reports only changes for the state.
    keyboard_probe: KeyboardProbe,
    /// The text going out now. The worker keeps serving EIS while it does.
    text_job: Option<TextJob>,
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
            Event::Handshake(handshake, reis::ei::handshake::Event::HandshakeVersion { .. }) => {
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
            Event::Callback(callback, reis::ei::callback::Event::Done { .. }) => {
                if let Some(job) = self.text_job.as_mut() {
                    job.acknowledge(&callback);
                }
                if matches!(&self.keyboard_probe, KeyboardProbe::Sent(sent, _) if *sent == callback)
                {
                    self.keyboard_probe = KeyboardProbe::Finished;
                }
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
                    keymap_type: reis::ei::keyboard::KeymapType::Xkb,
                    size,
                    keymap,
                },
            ) => {
                self.load_xkb_keymap(keymap, size);
            }
            Event::Keyboard(
                _kb,
                reis::ei::keyboard::Event::Modifiers {
                    serial,
                    depressed,
                    locked,
                    latched,
                    group,
                },
            ) => {
                self.last_serial = serial;
                self.keyboard = Some(KeyboardState {
                    depressed,
                    latched,
                    locked,
                    group,
                });
                EIS_REPORTS_KEYBOARD.store(true, Ordering::Release);
            }
            _ => {}
        }
    }

    /// Compile the compositor's xkb keymap (from the `ei_keyboard` `Keymap` fd), so
    /// key typing follows the user's real layout. On any failure there is no
    /// keymap, and typing keys is refused rather than guessed.
    fn load_xkb_keymap(&mut self, fd: std::os::unix::io::OwnedFd, size: u32) {
        use std::io::{Read, Seek, SeekFrom};
        self.keymap = None;
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
        self.keymap = xkb::Keymap::new_from_string(
            &xkb::Context::new(
                xkb::CONTEXT_NO_DEFAULT_INCLUDES | xkb::CONTEXT_NO_ENVIRONMENT_NAMES,
            ),
            text,
            xkb::KEYMAP_FORMAT_TEXT_V1,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        );
    }

    /// True once the device THIS command needs has been negotiated and
    /// `Resumed`. Gating per-command (rather than on one global pointer check)
    /// means a keyboard-only session doesn't stall keyboard commands waiting for
    /// an absolute-pointer device that never arrives — and vice versa. Commands
    /// run before their device exists fail with "no EIS device negotiated yet",
    /// so `run_calloop` queues them until this holds (or READY_TIMEOUT elapses).
    fn cmd_ready(&self, cmd: &Cmd) -> bool {
        if !self.device_is_settled() {
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
                let keyboard = self.device_with_interface("ei_keyboard").is_some();
                (self.device_with_interface("ei_text").is_some() || keyboard)
                    && (!keyboard || self.keyboard_settled())
            }
            Cmd::Shutdown => true,
        }
    }

    fn handle_command(&mut self, cmd: Cmd) {
        if let Cmd::TypeText {
            text,
            desktop,
            ticket,
            action,
            reply,
        } = cmd
        {
            self.start_text(&text, desktop, ticket, action, reply);
            return;
        }
        let result = self.run_command(&cmd);
        let needs_sync = !matches!(cmd, Cmd::WaitReady { .. });
        let Some(reply) = cmd.into_reply() else {
            return;
        };
        if result.is_ok() && needs_sync {
            if let (Some(connection), Some(context)) = (&self.connection, &self.context) {
                // A socket flush is not delivery. Hold the caller's focus guard
                // until EIS acknowledges all preceding input frames.
                let callback = connection.sync(1);
                if let Err(error) = flush_to_eis(context) {
                    let _ = reply.send(Err(error));
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

    /// A command that waited longer than its caller: fail it without running it.
    fn expire_command(&self, cmd: Cmd) {
        let detail = match &cmd {
            Cmd::WaitReady { interface, .. } => {
                format!("no resumed EIS {interface} device negotiated within 20s")
            }
            Cmd::TypeText { .. } => refused(NOT_READY).to_string(),
            _ => NOT_READY.to_string(),
        };
        if let Some(reply) = cmd.into_reply() {
            let _ = reply.send(Err(anyhow::anyhow!(detail)));
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
                let n = (*steps).clamp(1, 500);
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
            Cmd::TypeText { .. } | Cmd::Shutdown => {}
        }
        if let Some(ctx) = self.context.as_ref() {
            let _ = ctx.flush();
        }
        Ok(())
    }

    /// Whether the worker has heard all it will about the keyboard.
    fn keyboard_settled(&self) -> bool {
        match self.keyboard_probe {
            KeyboardProbe::NotSent => self.keyboard.is_some(),
            KeyboardProbe::Sent(..) => false,
            KeyboardProbe::Finished => true,
        }
    }

    /// Before the first text, tap Shift: a no-op in any app, but a change, so KWin
    /// 6.8 reports the keyboard's layout and locks. EIS acknowledges the tap after
    /// any such report. A compositor that says nothing by then never will.
    fn probe_keyboard(&mut self) {
        const SHIFT: u32 = 42;
        match &self.keyboard_probe {
            KeyboardProbe::Finished => {}
            KeyboardProbe::Sent(_, sent) => {
                if sent.elapsed() >= ACK_WAIT {
                    self.keyboard_probe = KeyboardProbe::Finished;
                }
            }
            KeyboardProbe::NotSent if self.keyboard.is_some() => {
                self.keyboard_probe = KeyboardProbe::Finished;
            }
            KeyboardProbe::NotSent if self.device_is_settled() => {
                self.keyboard_probe = self.tap_shift(SHIFT);
            }
            KeyboardProbe::NotSent => {}
        }
    }

    fn tap_shift(&self, shift: u32) -> KeyboardProbe {
        let is_shift = self.keymap.as_ref().is_some_and(|map| {
            let code = xkb::Keycode::new(shift + EVDEV_OFFSET);
            map.key_get_syms_by_level(code, 0, 0) == [xkb::Keysym::Shift_L]
        });
        let (Some(connection), Some(context), true) = (&self.connection, &self.context, is_shift)
        else {
            return KeyboardProbe::Finished;
        };
        if self.send_tap(shift).is_err() {
            return KeyboardProbe::Finished;
        }
        let callback = connection.sync(1);
        let _ = flush_to_eis(context);
        KeyboardProbe::Sent(callback, Instant::now())
    }

    fn device_is_settled(&self) -> bool {
        const DEVICE_QUIET_PERIOD: Duration = Duration::from_millis(200);
        self.last_device_event
            .is_some_and(|last| last.elapsed() >= DEVICE_QUIET_PERIOD)
    }

    fn send_tap(&self, code: u32) -> anyhow::Result<()> {
        self.send_keys(&[TypedKey {
            code,
            modifiers: Vec::new(),
        }])
    }

    fn poll_timeout(&self) -> Duration {
        const IDLE_POLL: Duration = Duration::from_millis(20);
        self.text_job
            .as_ref()
            .filter(|job| job.unacknowledged.is_none())
            .map_or(IDLE_POLL, |job| {
                job.next_step_at
                    .saturating_duration_since(Instant::now())
                    .min(IDLE_POLL)
            })
    }

    /// Refuse `text` before anything goes out, or start sending it. The worker
    /// loop sends the rest, so it keeps serving EIS and the caller keeps waiting.
    fn start_text(
        &mut self,
        text: &str,
        desktop: DesktopFacts,
        ticket: Arc<TextTicket>,
        action: Option<Arc<Action>>,
        reply: Sender<anyhow::Result<()>>,
    ) {
        if !ticket.begin() {
            let _ = reply.send(Err(refused(NOT_READY)));
            return;
        }
        match self.text_route(text, desktop) {
            Ok((steps, settle)) => {
                self.text_job = Some(TextJob {
                    total: steps.iter().map(TextStep::chars).sum(),
                    done: 0,
                    steps: steps.into(),
                    settle,
                    next_step_at: Instant::now(),
                    unacknowledged: None,
                    ticket,
                    action,
                    reply,
                });
                self.advance_text();
            }
            Err(refusal) => {
                let _ = reply.send(Err(refusal));
            }
        }
    }

    /// How `text` goes out: every character, or a refusal with nothing sent.
    ///
    /// Keys follow the layout that is active now and apps get them in order, but
    /// the layout must have every character. `ei_text` takes any character, with
    /// three limits: it reaches apps by a different path from keys, so the two can
    /// swap, which takes time between them; KWin turns a line feed into the
    /// `Linefeed` key that most apps ignore; and X11 apps lose what the layout
    /// lacks. So it is for text the keys can't type, into an app that isn't X11.
    fn text_route(
        &self,
        text: &str,
        desktop: DesktopFacts,
    ) -> anyhow::Result<(Vec<TextStep>, Duration)> {
        let text = with_line_feeds(text);
        for ch in text.chars() {
            text_sym(ch)?;
        }
        if self.device_with_interface("ei_keyboard").is_none() {
            if desktop.x11_app_may_have_focus {
                return Err(refused(
                    "the compositor offers no keyboard, and an X11 app may have focus and \
                     lose text it gets another way",
                ));
            }
            if text.contains('\n') {
                return Err(refused(
                    "the compositor offers no keyboard to press Return for a line break",
                ));
            }
            return Ok((vec![TextStep::Utf8(text.into_owned())], Duration::ZERO));
        }
        let ei_text = self.device_with_interface("ei_text").is_some();
        let keys_only = !ei_text || desktop.x11_app_may_have_focus;
        if keys_only || text.contains('\n') {
            // Return is a key too, so a line break needs a keyboard state keys can trust.
            let (map, keyboard) = match &self.keymap {
                Some(map) => (map, keyboard_now(self.keyboard, desktop, map)?),
                None => return Err(refused("the compositor announced no keyboard layout")),
            };
            match plan_keys(&text, map, keyboard) {
                Ok(keys) => {
                    let steps = keys
                        .chunks(KEYS_PER_STEP)
                        .map(|keys| TextStep::Keys(keys.to_vec()))
                        .collect();
                    return Ok((steps, Duration::ZERO));
                }
                Err(refusal) if keys_only => return Err(refusal),
                Err(_) => {}
            }
        }
        Ok((text_parts(&text), PART_SETTLE))
    }

    /// Move the text going out one step along, if its time has come.
    fn advance_text(&mut self) {
        let Some(mut job) = self.text_job.take() else {
            return;
        };
        match self.drive_text(&mut job) {
            Ok(false) => self.text_job = Some(job),
            Ok(true) => {
                let _ = job.reply.send(Ok(()));
            }
            Err(error) => {
                let error = job.stopped(error);
                let _ = job.reply.send(Err(error));
            }
        }
    }

    /// True once all of the text has gone out and been acknowledged.
    fn drive_text(&self, job: &mut TextJob) -> anyhow::Result<bool> {
        anyhow::ensure!(!job.ticket.is_abandoned(), "the caller stopped waiting");
        if let Some((_, sent, _)) = &job.unacknowledged {
            anyhow::ensure!(sent.elapsed() < ACK_WAIT, NOT_ACKNOWLEDGED);
            return Ok(false);
        }
        let Some(step) = job.steps.front() else {
            return Ok(true);
        };
        if Instant::now() < job.next_step_at {
            return Ok(false);
        }
        let callback = match &job.action {
            Some(action) => action.admit(|| self.send_text_step(step))??,
            None => self.send_text_step(step)?,
        };
        let chars = step.chars();
        job.steps.pop_front();
        job.unacknowledged = Some((callback, Instant::now(), chars));
        Ok(false)
    }

    /// Send one step and ask EIS to acknowledge it. Its keys are all released by
    /// the end of the step, so stopping between steps leaves nothing held.
    fn send_text_step(&self, step: &TextStep) -> anyhow::Result<reis::ei::Callback> {
        let serial = self.last_serial;
        match step {
            TextStep::Utf8(text) => {
                let device = self
                    .device_with_interface("ei_text")
                    .ok_or_else(|| anyhow::anyhow!("the EIS text device went away"))?;
                require_device_interface::<reis::ei::Text>(&self.devices, &device)?.utf8(text);
                device.frame(serial, event_time_us());
            }
            TextStep::Return => self.send_tap(KEY_ENTER)?,
            TextStep::Keys(keys) => self.send_keys(keys)?,
        }
        let (Some(connection), Some(context)) = (&self.connection, &self.context) else {
            anyhow::bail!("EIS connection unavailable for input acknowledgement");
        };
        let callback = connection.sync(1);
        flush_to_eis(context)?;
        Ok(callback)
    }

    fn send_keys(&self, keys: &[TypedKey]) -> anyhow::Result<()> {
        use reis::ei::keyboard::KeyState::{Press, Released};
        let device = self
            .device_with_interface("ei_keyboard")
            .ok_or_else(|| anyhow::anyhow!("the EIS keyboard device went away"))?;
        let kb = require_device_interface::<reis::ei::Keyboard>(&self.devices, &device)?;
        let serial = self.last_serial;
        let send = |code: u32, state| {
            kb.key(code, state);
            device.frame(serial, event_time_us());
        };
        for key in keys {
            key.modifiers.iter().for_each(|&m| send(m, Press));
            send(key.code, Press);
            send(key.code, Released);
            key.modifiers.iter().rev().for_each(|&m| send(m, Released));
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
    let frames = u64::from(steps.clamp(1, 500));
    std::time::Duration::from_micros(
        duration_ms
            .min(10_000)
            .saturating_mul(1_000)
            .checked_div(frames)
            .unwrap_or(0),
    )
}

impl Cmd {
    fn into_reply(self) -> Option<Sender<anyhow::Result<()>>> {
        match self {
            Cmd::WaitReady { reply, .. }
            | Cmd::Click { reply, .. }
            | Cmd::MoveAbsolute { reply, .. }
            | Cmd::Scroll { reply, .. }
            | Cmd::TypeText { reply, .. }
            | Cmd::PressKey { reply, .. }
            | Cmd::KeySequence { reply, .. }
            | Cmd::Drag { reply, .. } => Some(reply),
            Cmd::Shutdown => None,
        }
    }
}

/// Write what is queued. A full socket is no failure: the rest stays queued and
/// the worker loop writes it, so the input still goes out.
fn flush_to_eis(context: &reis::ei::Context) -> anyhow::Result<()> {
    match context.flush().map_err(std::io::Error::from) {
        Err(error) if error.kind() != std::io::ErrorKind::WouldBlock => Err(error.into()),
        _ => Ok(()),
    }
}

const ACK_WAIT: Duration = Duration::from_secs(5);
const NOT_ACKNOWLEDGED: &str = "EIS did not acknowledge input within 5s; delivery is unknown";
/// Text and keys reach an app by separate paths, so each part gets time to land
/// before the next one is sent.
const PART_SETTLE: Duration = Duration::from_millis(50);
/// Keys sent before waiting for EIS to catch up, so a long text never outruns it.
const KEYS_PER_STEP: usize = 64;
const KEY_ENTER: u32 = 28;

/// A piece of a text going out, acknowledged by EIS before the next.
#[derive(Clone, Debug, PartialEq, Eq)]
enum TextStep {
    Utf8(String),
    Return,
    Keys(Vec<TypedKey>),
}

impl TextStep {
    fn chars(&self) -> usize {
        match self {
            TextStep::Utf8(text) => text.chars().count(),
            TextStep::Return => 1,
            TextStep::Keys(keys) => keys.len(),
        }
    }
}

struct TextJob {
    steps: VecDeque<TextStep>,
    settle: Duration,
    total: usize,
    done: usize,
    next_step_at: Instant,
    /// The step EIS has not acknowledged: its callback, when it went, its characters.
    unacknowledged: Option<(reis::ei::Callback, Instant, usize)>,
    ticket: Arc<TextTicket>,
    action: Option<Arc<Action>>,
    reply: Sender<anyhow::Result<()>>,
}

impl TextJob {
    fn acknowledge(&mut self, callback: &reis::ei::Callback) {
        if let Some((_, _, chars)) = self
            .unacknowledged
            .take_if(|(waiting_for, _, _)| waiting_for == callback)
        {
            self.done += chars;
            self.next_step_at = Instant::now() + self.settle;
            self.ticket.advance();
        }
    }

    /// `error` as the caller should see it. A stop before anything went out keeps
    /// its code; a later one says how far it got, without the code that means no
    /// effect, as niri's keyboard does.
    fn stopped(&self, error: anyhow::Error) -> anyhow::Error {
        let (done, total) = (self.done, self.total);
        match &self.unacknowledged {
            None if done == 0 => error,
            None => anyhow::anyhow!("type_text stopped after {done} of {total} characters: {error:#}"),
            Some((_, _, chars)) => anyhow::anyhow!(
                "type_text stopped after {done} of {total} characters, and the next {chars} may have gone out: {error:#}"
            ),
        }
    }
}

/// A compositor that reports the keyboard only when it changes has to see a change
/// first, or text would start out on a state nobody told the worker about.
#[derive(Default)]
enum KeyboardProbe {
    #[default]
    NotSent,
    Sent(reis::ei::Callback, Instant),
    Finished,
}

/// The compositor's keyboard state, which decides what a key press types: EIS
/// keys go through the compositor's own xkb state, with its active layout and
/// its locks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct KeyboardState {
    depressed: u32,
    latched: u32,
    locked: u32,
    group: u32,
}

/// The keyboard's state right now: what EIS last said or, when it has said
/// nothing, what the compositor said when asked. A keymap with several layouts and
/// no word on which is active is refused, since any guess can type wrong letters.
/// So is a held or latched modifier key, or no word on them, since a held Shift
/// turns `a` into `A`, `1` into `!` and Return into Shift+Return.
fn keyboard_now(
    reported: Option<KeyboardState>,
    desktop: DesktopFacts,
    map: &xkb::Keymap,
) -> anyhow::Result<KeyboardState> {
    if let Some(keyboard) = reported {
        if keyboard.depressed | keyboard.latched != 0 {
            return Err(refused(MODIFIER_HELD));
        }
        return Ok(keyboard);
    }
    let group = match desktop.active_layout {
        Some(group) => group,
        None if map.num_layouts() <= 1 => 0,
        None => {
            return Err(refused(format!(
                "the keymap has {} layouts and the compositor doesn't say which is active",
                map.num_layouts()
            )))
        }
    };
    let locks = desktop.lock_keys.ok_or_else(|| {
        refused(
            "the compositor doesn't say which lock and modifier keys are on, so keys could \
             type something else; on KDE Plasma 6.3 to 6.5, `cua-driver kwin-capture on` lets \
             the driver read them",
        )
    })?;
    if locks.modifier_held {
        return Err(refused(MODIFIER_HELD));
    }
    let lock_mask = |on: bool, name: &str| match on {
        true => 1_u32.checked_shl(map.mod_get_index(name)).unwrap_or(0),
        false => 0,
    };
    Ok(KeyboardState {
        group,
        locked: lock_mask(locks.caps, xkb::MOD_NAME_CAPS) | lock_mask(locks.num, xkb::MOD_NAME_NUM),
        ..KeyboardState::default()
    })
}

const MODIFIER_HELD: &str =
    "a modifier key is held or latched, so keys would not type what they say";

/// One key press that types a character on the active layout, and the modifier
/// keys to hold around it, in press order.
#[derive(Clone, Debug, PartialEq, Eq)]
struct TypedKey {
    code: u32,
    modifiers: Vec<u32>,
}

/// Whether holding `modifiers` and pressing `code` types `sym`, worked out with
/// the same xkb state machine the compositor runs.
fn types_sym(
    map: &xkb::Keymap,
    keyboard: KeyboardState,
    modifiers: &[u32],
    code: xkb::Keycode,
    sym: xkb::Keysym,
) -> bool {
    let mut state = xkb::State::new(map);
    state.update_mask(0, 0, keyboard.locked, 0, 0, keyboard.group);
    for &modifier in modifiers {
        state.update_key(
            xkb::Keycode::new(modifier + EVDEV_OFFSET),
            xkb::KeyDirection::Down,
        );
    }
    state.serialize_layout(xkb::STATE_LAYOUT_EFFECTIVE) == keyboard.group
        && state.key_get_one_sym(code) == sym
}

/// The key that types `sym` now, on a key every app receives, and the fewest
/// modifiers to hold for it. Which ones depends on the locks: under Caps Lock, `a`
/// takes Shift. Modifier keys can depend on each other (Shift held first turns
/// some AltGr keys into `Multi_key`), so both press orders are tried.
fn typed_key(
    map: &xkb::Keymap,
    keyboard: KeyboardState,
    chords: &[Vec<u32>],
    sym: xkb::Keysym,
) -> Option<TypedKey> {
    let (code, modifiers) = find_key(map, keyboard.group, sym, |code, _| {
        chords.iter().find_map(|chord| {
            let reversed = chord.iter().rev().copied().collect();
            let order = [chord.clone(), reversed]
                .into_iter()
                .find(|order| types_sym(map, keyboard, order, code, sym))?;
            Some((u32::try_from(order.len()).ok()?, order))
        })
    })?;
    chromium_receives(code).then_some(TypedKey { code, modifiers })
}

/// Resolve every character before any key is sent, so a string the keymap cannot
/// produce right now is refused whole instead of arriving wrong or short.
/// `keyboard` holds no modifier key: [`keyboard_now`] refuses one that does.
fn plan_keys(
    text: &str,
    map: &xkb::Keymap,
    keyboard: KeyboardState,
) -> anyhow::Result<Vec<TypedKey>> {
    let chords = modifier_chords(map, keyboard.group);
    let mut known = HashMap::new();
    let mut plan = Vec::with_capacity(text.len());
    let mut missing = String::new();
    for ch in text.chars() {
        let sym = text_sym(ch)?;
        let key = known
            .entry(ch)
            .or_insert_with(|| typed_key(map, keyboard, &chords, sym));
        match key {
            Some(key) => plan.push(key.clone()),
            None if missing.contains(ch) => {}
            None => missing.push(ch),
        }
    }
    if missing.is_empty() {
        Ok(plan)
    } else {
        Err(refused(format!(
            "the active keyboard layout has no key for {missing:?}. Use set_value on the \
             element, or clipboard_write and then hotkey ctrl+v"
        )))
    }
}

/// Whether an X11 app has, or may have, keyboard focus. KWin types a character
/// the layout lacks by swapping in a keymap, pressing F24 and swapping back at
/// once. Xwayland apps read the keymap after that, so the character is lost with
/// no error, and such text must go as keys, which refuse it. So only a clear "no"
/// counts as no: no X server to ask, or one that says a Wayland app has focus. A
/// server that doesn't answer in time is a maybe, and one probe waits on it at a
/// time, so a hung server holds at most one thread and socket.
fn x11_app_may_have_focus() -> bool {
    const ANSWER_WITHIN: Duration = Duration::from_millis(250);
    static PROBING: AtomicBool = AtomicBool::new(false);
    if std::env::var_os("DISPLAY").is_none() && !x11_socket_exists() {
        return false;
    }
    if PROBING.swap(true, Ordering::AcqRel) {
        return true;
    }
    let (sender, receiver) = bounded(1);
    let probe = thread::Builder::new()
        .name("cua-libei-x11-focus".into())
        .spawn(move || {
            let answer = crate::x11::active_window_is_a_client();
            PROBING.store(false, Ordering::Release);
            let _ = sender.send(answer);
        });
    if probe.is_err() {
        PROBING.store(false, Ordering::Release);
        return true;
    }
    !matches!(receiver.recv_timeout(ANSWER_WITHIN), Ok(Some(false)))
}

/// Whether an X server has a socket here, for a driver started without `DISPLAY`.
fn x11_socket_exists() -> bool {
    std::fs::read_dir("/tmp/.X11-unix").is_ok_and(|mut sockets| {
        sockets.any(|socket| {
            socket.is_ok_and(|socket| socket.file_name().as_encoded_bytes().starts_with(b"X"))
        })
    })
}

/// Cut `text` at its line endings. KWin turns a newline sent as text into the
/// `Linefeed` key, which X11 apps ignore, so those go as presses of Return.
fn text_parts(text: &str) -> Vec<TextStep> {
    let text = with_line_feeds(text);
    let mut parts = Vec::new();
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            parts.push(TextStep::Return);
        }
        if !line.is_empty() {
            parts.push(TextStep::Utf8(line.to_string()));
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
    use std::io::Write;
    use std::os::fd::AsFd;
    use std::sync::Mutex;

    #[derive(Clone, Debug, PartialEq)]
    pub(in crate::wayland) enum Emitted {
        Motion(String, f32, f32),
        Button(u32, bool),
        Scroll(f32, f32),
        Key(u32, bool),
        Text(String),
    }

    /// What a test asks the EIS server to do. The server answers on `done` once
    /// it has written the change, which then precedes any later acknowledgement.
    pub(in crate::wayland::libei) enum Control {
        Modifiers(KeyboardState),
        /// Pause or resume the `ei_text` device.
        TextDevice {
            resumed: bool,
        },
        /// Pause or resume the `ei_keyboard` device.
        KeyboardDevice {
            resumed: bool,
        },
        /// Pause every device once the next text arrives.
        PauseOnText,
        /// Resume every device.
        ResumeAll,
        /// Stop reading from the client: EIS is stuck.
        Wedge(bool),
    }

    const DEVICES: [(&str, &[u32]); 2] = [("wide", &[0, 3840]), ("far", &[10_000])];
    /// A real keymap: `us` is layout 0 and `de` is layout 1, with AltGr.
    const KEYMAP: &str = include_str!("../../tests/fixtures/niri-us-de.xkb");

    type Log = Arc<Mutex<Vec<Emitted>>>;

    struct Server {
        ctx: reis::eis::Context,
        handshaker: Option<EisHandshaker>,
        converter: Option<EisRequestConverter>,
        devices: Vec<reis::request::Device>,
        log: Log,
        keyboard: Option<reis::request::Device>,
        text: Option<reis::request::Device>,
        pause_on_text: bool,
        reports_on_key: Option<KeyboardState>,
        wedged: bool,
    }

    impl Server {
        fn pump(&mut self) {
            if self.wedged || self.ctx.read().is_err() {
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
                                | DeviceCapability::Scroll
                                | DeviceCapability::Keyboard
                                | DeviceCapability::Text,
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
            let log_handle = self.log.clone();
            let mut log = log_handle.lock().unwrap();
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
                    let keyboard = bind.seat.add_device(
                        Some("keyboard"),
                        reis::eis::device::DeviceType::Virtual,
                        BitFlags::from(DeviceCapability::Keyboard),
                        send_keymap,
                    );
                    keyboard.resumed();
                    self.keyboard = Some(keyboard);
                    let text = bind.seat.add_device(
                        Some("text"),
                        reis::eis::device::DeviceType::Virtual,
                        BitFlags::from(DeviceCapability::Text),
                        |_| {},
                    );
                    text.resumed();
                    self.text = Some(text);
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
                EisRequest::KeyboardKey(k) => {
                    log.push(Emitted::Key(
                        k.key,
                        k.state == reis::eis::keyboard::KeyState::Press,
                    ));
                    if let Some(state) = self.reports_on_key {
                        drop(log);
                        // As KWin 6.8 does for a modifier key: held, then not.
                        let pressed = k.state == reis::eis::keyboard::KeyState::Press;
                        self.control(Control::Modifiers(KeyboardState {
                            depressed: u32::from(pressed),
                            ..state
                        }));
                        if pressed {
                            // Long enough for the client to see the key held.
                            let _ = self.ctx.flush();
                            thread::sleep(Duration::from_millis(100));
                        }
                    }
                }
                EisRequest::TextUtf8(t) => {
                    log.push(Emitted::Text(t.text));
                    if std::mem::take(&mut self.pause_on_text) {
                        self.devices
                            .iter()
                            .chain(&self.keyboard)
                            .chain(&self.text)
                            .for_each(reis::request::Device::paused);
                    }
                }
                _ => {}
            }
        }

        fn control(&mut self, control: Control) {
            match control {
                Control::Modifiers(state) => {
                    let keyboard = self.keyboard.as_ref().expect("the seat is bound");
                    let converter = self.converter.as_ref().expect("the seat is bound");
                    let interface = keyboard
                        .interface::<reis::eis::Keyboard>()
                        .expect("the device has a keyboard");
                    converter.handle().with_next_serial(|serial| {
                        interface.modifiers(
                            serial,
                            state.depressed,
                            state.locked,
                            state.latched,
                            state.group,
                        );
                    });
                }
                Control::TextDevice { resumed } => {
                    let text = self.text.as_ref().expect("the seat is bound");
                    if resumed {
                        text.resumed();
                    } else {
                        text.paused();
                    }
                }
                Control::KeyboardDevice { resumed } => {
                    let keyboard = self.keyboard.as_ref().expect("the seat is bound");
                    if resumed {
                        keyboard.resumed();
                    } else {
                        keyboard.paused();
                    }
                }
                Control::PauseOnText => self.pause_on_text = true,
                Control::ResumeAll => self
                    .devices
                    .iter()
                    .chain(&self.keyboard)
                    .chain(&self.text)
                    .for_each(reis::request::Device::resumed),
                Control::Wedge(wedged) => self.wedged = wedged,
            }
            let _ = self.ctx.flush();
        }
    }

    fn send_keymap(device: &reis::request::Device) {
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(KEYMAP.as_bytes()).unwrap();
        device
            .interface::<reis::eis::Keyboard>()
            .expect("the device has a keyboard")
            .keymap(
                reis::eis::keyboard::KeymapType::Xkb,
                u32::try_from(KEYMAP.len()).unwrap(),
                file.as_fd(),
            );
    }

    /// The real worker loop against a local EIS server, the same way `worker`
    /// runs it.
    pub(in crate::wayland::libei) struct LocalEis {
        pub(in crate::wayland::libei) tx: Sender<Cmd>,
        pub(in crate::wayland::libei) log: Log,
        controls: Sender<(Control, Sender<()>)>,
    }

    impl LocalEis {
        /// A compositor that reports `reports_on_key`, if any, at each key: KWin
        /// 6.8 reports only changes, and KWin before it never does.
        pub(in crate::wayland::libei) fn start(reports_on_key: Option<KeyboardState>) -> Self {
            use std::os::unix::net::UnixStream;
            let log = Log::default();
            let (tx, rx) = bounded::<Cmd>(64);
            let (client_end, server_end) = UnixStream::pair().unwrap();
            let client = reis::ei::Context::new(client_end).unwrap();
            let ctx = reis::eis::Context::new(server_end).unwrap();
            let handshaker = EisHandshaker::new(&ctx, 1);
            let mut server = Server {
                ctx,
                handshaker: Some(handshaker),
                converter: None,
                devices: Vec::new(),
                log: log.clone(),
                keyboard: None,
                text: None,
                pause_on_text: false,
                reports_on_key,
                wedged: false,
            };
            let (controls, requests) = bounded::<(Control, Sender<()>)>(1);
            thread::spawn(move || {
                let _ = client.handshake();
                let _ = client.flush();
                let _ = run_calloop(client, rx);
            });
            thread::spawn(move || loop {
                server.pump();
                match requests.try_recv() {
                    Ok((control, done)) => {
                        server.control(control);
                        let _ = done.send(());
                    }
                    Err(crossbeam_channel::TryRecvError::Empty) => {}
                    Err(crossbeam_channel::TryRecvError::Disconnected) => break,
                }
                thread::sleep(Duration::from_millis(1));
            });
            Self { tx, log, controls }
        }

        fn control(&self, control: Control) {
            let (done, finished) = bounded(1);
            self.controls.send((control, done)).unwrap();
            finished.recv().unwrap();
        }
    }

    struct Fixture {
        local: LocalEis,
        gate: Mutex<()>,
    }

    /// The worker every public function in this process talks to. Its compositor
    /// has reported the keyboard, as Mutter does, so tests see only their own keys.
    fn install() -> &'static Fixture {
        static FIXTURE: OnceLock<Fixture> = OnceLock::new();
        FIXTURE.get_or_init(|| {
            let local = LocalEis::start(None);
            assert!(
                TX.set(local.tx.clone()).is_ok(),
                "real libei worker already started"
            );
            wait_keyboard_ready().unwrap();
            local.control(Control::Modifiers(KeyboardState::default()));
            Fixture {
                local,
                gate: Mutex::new(()),
            }
        })
    }

    pub(in crate::wayland::libei) fn control(control: Control) {
        install().local.control(control);
    }

    /// Run `f` against a local EIS server and return what the server received
    /// from it. A trailing barrier motion guarantees events from commands that
    /// failed early are not still in flight when the log is read.
    pub(in crate::wayland) fn record<R>(f: impl FnOnce() -> R) -> (R, Vec<Emitted>) {
        let fixture = install();
        let _gate = fixture.gate.lock().unwrap_or_else(|e| e.into_inner());
        fixture.local.log.lock().unwrap().clear();
        let result = f();
        move_absolute(1.0, 1.0).unwrap();
        let mut events = std::mem::take(&mut *fixture.local.log.lock().unwrap());
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
mod typing_route_tests {
    use super::local_eis::{control, record, Control, Emitted, LocalEis};
    use super::*;

    const RIGHT_ALT: u32 = 100;
    const LEFT_SHIFT: u32 = 42;
    const Q: u32 = 16;
    const Y: u32 = 21;
    const Z: u32 = 44;
    const A: u32 = 30;
    const CAPS_LOCK_MASK: u32 = 1 << 1;
    const SHIFT_MASK: u32 = 1;
    const DE: u32 = 1;
    const WAIT: Duration = REPLY_WAIT;

    fn facts(x11_app_may_have_focus: bool) -> DesktopFacts {
        DesktopFacts {
            x11_app_may_have_focus,
            ..DesktopFacts::default()
        }
    }

    fn send(text: &str, x11_app_may_have_focus: bool) -> anyhow::Result<()> {
        send_text(tx()?, text, facts(x11_app_may_have_focus), WAIT)
    }

    fn tap(code: u32) -> [Emitted; 2] {
        [Emitted::Key(code, true), Emitted::Key(code, false)]
    }

    fn held(modifier: u32, code: u32) -> [Emitted; 4] {
        [
            Emitted::Key(modifier, true),
            Emitted::Key(code, true),
            Emitted::Key(code, false),
            Emitted::Key(modifier, false),
        ]
    }

    /// Type with `state` on the keyboard, then put the keyboard back.
    fn typed_with(
        state: KeyboardState,
        text: &str,
        x11: bool,
    ) -> (anyhow::Result<()>, Vec<Emitted>) {
        record(|| {
            control(Control::Modifiers(state));
            let result = send(text, x11);
            control(Control::Modifiers(KeyboardState::default()));
            result
        })
    }

    fn typed(text: &str, x11: bool) -> (anyhow::Result<()>, Vec<Emitted>) {
        typed_with(KeyboardState::default(), text, x11)
    }

    #[test]
    fn keys_follow_the_active_layout() {
        // `de` swaps Y and Z. The same text is different keys in each layout.
        let (result, events) = typed("y\nz", true);
        result.unwrap();
        assert_eq!(events, [tap(Y), tap(28), tap(Z)].concat());

        let (result, events) = typed_with(
            KeyboardState {
                group: DE,
                ..KeyboardState::default()
            },
            "y\nz",
            true,
        );
        result.unwrap();
        assert_eq!(events, [tap(Z), tap(28), tap(Y)].concat());
    }

    #[test]
    fn a_modifier_is_pressed_around_its_key_and_released_after_it() {
        let (result, events) = typed("Ya", true);
        result.unwrap();
        assert_eq!(
            events,
            [held(LEFT_SHIFT, Y).to_vec(), tap(A).to_vec()].concat()
        );

        // `@` is AltGr+Q in `de`.
        let (result, events) = typed_with(
            KeyboardState {
                group: DE,
                ..KeyboardState::default()
            },
            "@",
            true,
        );
        result.unwrap();
        assert_eq!(events, held(RIGHT_ALT, Q));
    }

    #[test]
    fn caps_lock_on_gets_the_exact_case() {
        let caps = KeyboardState {
            locked: CAPS_LOCK_MASK,
            ..KeyboardState::default()
        };
        let (result, events) = typed_with(caps, "aA1", true);
        result.unwrap();
        assert_eq!(
            events,
            [
                held(LEFT_SHIFT, A).to_vec(),
                tap(A).to_vec(),
                tap(2).to_vec()
            ]
            .concat()
        );
    }

    #[test]
    fn a_held_modifier_refuses_before_any_key() {
        let (result, events) = typed_with(
            KeyboardState {
                depressed: SHIFT_MASK,
                ..KeyboardState::default()
            },
            "a",
            true,
        );
        assert_eq!(
            result.unwrap_err().to_string(),
            "foreground_unavailable: a modifier key is held or latched, so keys would not type \
             what they say; nothing was typed"
        );
        assert_eq!(events, []);
    }

    #[test]
    fn text_the_layout_lacks_refuses_whole_before_any_key() {
        let (result, events) = typed("a🙂b🙂", true);
        assert_eq!(
            result.unwrap_err().to_string(),
            "foreground_unavailable: the active keyboard layout has no key for \"🙂\". Use \
             set_value on the element, or clipboard_write and then hotkey ctrl+v; nothing was typed"
        );
        assert_eq!(events, []);
    }

    #[test]
    fn a_control_character_refuses_on_both_routes() {
        for x11 in [true, false] {
            let (result, events) = typed("abc\u{8}", x11);
            assert_eq!(
                result.unwrap_err().to_string(),
                "foreground_unavailable: control character U+0008 cannot be typed; nothing was typed"
            );
            assert_eq!(events, []);
        }
    }

    #[test]
    fn a_line_goes_through_ei_text_unless_an_x11_app_may_have_focus() {
        let (result, events) = typed("añ🙂", false);
        result.unwrap();
        assert_eq!(events, [Emitted::Text("añ🙂".into())]);

        let (result, events) = typed("añ🙂", true);
        assert_eq!(
            result.unwrap_err().to_string(),
            "foreground_unavailable: the active keyboard layout has no key for \"ñ🙂\". Use \
             set_value on the element, or clipboard_write and then hotkey ctrl+v; nothing was typed"
        );
        assert_eq!(events, []);
    }

    #[test]
    fn line_breaks_go_as_keys_when_the_layout_can_type_it_all() {
        let (result, events) = typed("a\r\nb\n", false);
        result.unwrap();
        assert_eq!(
            events,
            [tap(A), tap(28), tap(48), tap(28)].concat(),
            "a CRLF is one Return, and no part went through ei_text"
        );
    }

    #[test]
    fn text_with_line_breaks_the_keys_cant_type_goes_in_parts_with_return_between() {
        let (result, events) = typed("🙂\n\nb\r\nc", false);
        result.unwrap();
        assert_eq!(
            events,
            [
                vec![Emitted::Text("🙂".into())],
                tap(28).to_vec(),
                tap(28).to_vec(),
                vec![Emitted::Text("b".into())],
                tap(28).to_vec(),
                vec![Emitted::Text("c".into())],
            ]
            .concat()
        );
    }

    #[test]
    fn without_ei_text_a_line_goes_as_keys_and_the_rest_is_refused() {
        let (results, events) = record(|| {
            control(Control::TextDevice { resumed: false });
            // A paused device counts once the worker's device quiet period ends.
            wait_keyboard_ready().unwrap();
            let results = (send("ab", false), send("🙂", false));
            control(Control::TextDevice { resumed: true });
            wait_keyboard_ready().unwrap();
            results
        });
        results.0.unwrap();
        assert_eq!(
            results.1.unwrap_err().to_string(),
            "foreground_unavailable: the active keyboard layout has no key for \"🙂\". Use \
             set_value on the element, or clipboard_write and then hotkey ctrl+v; nothing was typed"
        );
        assert_eq!(events, [tap(A), tap(48)].concat());
    }

    #[test]
    fn a_held_modifier_refuses_a_line_break_on_the_ei_text_route() {
        let (result, events) = typed_with(
            KeyboardState {
                depressed: SHIFT_MASK,
                ..KeyboardState::default()
            },
            "🙂\nb",
            false,
        );
        assert_eq!(
            result.unwrap_err().to_string(),
            format!("foreground_unavailable: {MODIFIER_HELD}; nothing was typed")
        );
        assert_eq!(events, []);
    }

    #[test]
    fn with_only_ei_text_a_line_goes_out_and_x11_or_line_breaks_are_refused() {
        let (results, events) = record(|| {
            control(Control::KeyboardDevice { resumed: false });
            wait_until_ready("ei_text").unwrap();
            let results = (send("ab", false), send("ab", true), send("a\nb", false));
            control(Control::KeyboardDevice { resumed: true });
            wait_keyboard_ready().unwrap();
            results
        });
        results.0.unwrap();
        assert_eq!(
            results.1.unwrap_err().to_string(),
            "foreground_unavailable: the compositor offers no keyboard, and an X11 app may have \
             focus and lose text it gets another way; nothing was typed"
        );
        assert_eq!(
            results.2.unwrap_err().to_string(),
            "foreground_unavailable: the compositor offers no keyboard to press Return for a \
             line break; nothing was typed"
        );
        assert_eq!(events, [Emitted::Text("ab".into())]);
    }

    #[test]
    fn a_long_text_arrives_whole_and_in_order() {
        let text = "ab".repeat(KEYS_PER_STEP * 3);
        let (result, events) = typed(&text, true);
        result.unwrap();
        let expected: Vec<_> = text
            .chars()
            .flat_map(|c| tap(if c == 'a' { A } else { 48 }))
            .collect();
        assert_eq!(events, expected);
    }

    #[test]
    fn the_worker_serves_eis_while_a_text_goes_out_and_stops_when_devices_pause() {
        let (result, events) = record(|| {
            control(Control::PauseOnText);
            let result = send("🙂\n🙂\n🙂\n🙂", false);
            resume_every_device();
            result
        });
        assert_eq!(
            result.unwrap_err().to_string(),
            "type_text stopped after 1 of 7 characters: the EIS keyboard device went away"
        );
        assert_eq!(events, [Emitted::Text("🙂".into())]);
    }

    /// Put the devices `PauseOnText` paused back, so later tests have them.
    fn resume_every_device() {
        control(Control::ResumeAll);
        wait_keyboard_ready().unwrap();
    }

    #[test]
    fn a_text_stops_when_its_tool_call_is_over() {
        let (result, events) = record(|| {
            let action = Action::new(Duration::from_secs(60));
            let _bound = crate::input::action::bind(action.clone());
            let typing = thread::spawn({
                let action = action.clone();
                move || {
                    let _bound = crate::input::action::bind(action);
                    send("🙂\n🙂\n🙂\n🙂\n🙂\n🙂", false)
                }
            });
            thread::sleep(PART_SETTLE);
            action.abandon();
            typing.join().unwrap()
        });
        // How far it got depends on timing; the rest of the message doesn't.
        let error: String = result
            .unwrap_err()
            .to_string()
            .chars()
            .filter(|c| !c.is_ascii_digit())
            .collect();
        let over = "foreground_timeout: the call ran out of time, so no further input was sent";
        assert!(
            [
                format!("type_text stopped after  of  characters: {over}"),
                format!(
                    "type_text stopped after  of  characters, and the next  may have gone out: \
                     {over}"
                ),
            ]
            .contains(&error),
            "{error}"
        );
        assert!(
            events.len() < [1, 2, 1, 2, 1, 2, 1, 2, 1, 2, 1].iter().sum::<usize>(),
            "{events:?}"
        );
    }

    #[test]
    fn a_one_line_call_returns_with_no_settle_after_it() {
        let fastest = (0..5)
            .map(|_| {
                let (result, took) = record(|| {
                    let start = Instant::now();
                    let result = send("ab", false);
                    (result, start.elapsed())
                })
                .0;
                result.unwrap();
                took
            })
            .min();
        assert!(fastest < Some(PART_SETTLE), "{fastest:?}");
    }

    #[test]
    fn a_stuck_eis_stops_the_text_and_says_how_far_it_got() {
        // A stuck EIS stops the text at its next step: nothing more is sent.
        let (result, _) = record(|| {
            control(Control::Wedge(true));
            let result = send("🙂\n🙂", false);
            control(Control::Wedge(false));
            result
        });
        assert_eq!(
            result.unwrap_err().to_string(),
            "type_text stopped after 0 of 3 characters, and the next 1 may have gone out: EIS did \
             not acknowledge input within 5s; delivery is unknown"
        );
    }

    #[test]
    fn a_text_its_caller_gave_up_on_is_not_typed() {
        let (reply, reply_rx) = bounded(1);
        let ticket = Arc::new(TextTicket::default());
        assert!(ticket.abandon(), "it never started");
        let (result, events) = record(|| {
            tx().unwrap()
                .send(Cmd::TypeText {
                    text: "a".into(),
                    desktop: facts(true),
                    ticket,
                    action: None,
                    reply,
                })
                .unwrap();
            reply_rx.recv().unwrap()
        });
        assert_eq!(
            result.unwrap_err().to_string(),
            refused(NOT_READY).to_string()
        );
        assert_eq!(events, []);
    }

    #[test]
    fn a_text_its_caller_gave_up_on_stops_at_its_next_step() {
        let (result, events) = record(|| {
            control(Control::Wedge(true));
            let result = send_text(
                tx().unwrap(),
                "🙂\n🙂",
                facts(false),
                Duration::from_millis(100),
            );
            control(Control::Wedge(false));
            // The worker takes a new text only once it is done with the old one.
            send("x", false).unwrap();
            result
        });
        assert_eq!(
            result.unwrap_err().to_string(),
            "type_text stopped: the libei worker stopped moving, so part of the text may have \
             gone out and no more of it will. Read the field before retrying"
        );
        assert_eq!(
            events,
            [Emitted::Text("🙂".into()), Emitted::Text("x".into())]
        );
    }

    #[test]
    fn a_caller_waits_as_long_as_the_text_moves_and_no_longer() {
        const STEP: Duration = Duration::from_millis(40);
        // Moving: three waits' worth of text comes back with its result.
        let (reply, rx) = bounded(1);
        let ticket = Arc::new(TextTicket::default());
        assert!(ticket.begin());
        let worker = thread::spawn({
            let ticket = ticket.clone();
            move || {
                for _ in 0..3 {
                    thread::sleep(STEP / 2);
                    ticket.advance();
                    thread::sleep(STEP / 2);
                }
                reply.send(Ok(())).unwrap();
            }
        });
        wait_for_text(&rx, &ticket, STEP * 2).unwrap();
        worker.join().unwrap();

        // Stuck after starting: the caller stops it, and says it may be partial.
        let (_reply, rx) = bounded(1);
        let ticket = TextTicket::default();
        assert!(ticket.begin());
        assert_eq!(
            wait_for_text(&rx, &ticket, STEP).unwrap_err().to_string(),
            "type_text stopped: the libei worker stopped moving, so part of the text may have \
             gone out and no more of it will. Read the field before retrying"
        );
        assert!(ticket.is_abandoned());

        // Never started: the caller stops it, and nothing was typed.
        let (_reply, rx) = bounded(1);
        let ticket = TextTicket::default();
        assert_eq!(
            wait_for_text(&rx, &ticket, STEP).unwrap_err().to_string(),
            format!("foreground_unavailable: {NOT_READY}; nothing was typed")
        );
        assert!(!ticket.begin(), "the worker must not start it now");
    }

    /// The keys a compositor that reports nothing gets for `text`.
    fn on_silent_compositor(
        local: &LocalEis,
        text: &str,
        desktop: DesktopFacts,
    ) -> (anyhow::Result<()>, Vec<Emitted>) {
        let result = send_text(&local.tx, text, desktop, WAIT);
        let events = std::mem::take(&mut *local.log.lock().unwrap());
        (result, events)
    }

    #[test]
    fn a_compositor_that_reports_nothing_is_asked_directly() {
        const SHIFT: u32 = 42;
        let local = LocalEis::start(None);
        let de = DesktopFacts {
            x11_app_may_have_focus: true,
            active_layout: Some(DE),
            lock_keys: Some(LockKeys::default()),
        };

        // The first text taps Shift, once, to see if the compositor reports it.
        let (result, events) = on_silent_compositor(&local, "y", de);
        result.unwrap();
        assert_eq!(events, [tap(SHIFT), tap(Z)].concat());
        let (result, events) = on_silent_compositor(&local, "y", de);
        result.unwrap();
        assert_eq!(events, tap(Z));

        let caps = DesktopFacts {
            lock_keys: Some(LockKeys {
                caps: true,
                ..LockKeys::default()
            }),
            ..de
        };
        let (result, events) = on_silent_compositor(&local, "z", caps);
        result.unwrap();
        assert_eq!(events, held(SHIFT, Y));

        let (result, events) = on_silent_compositor(&local, "y", facts(true));
        assert_eq!(
            result.unwrap_err().to_string(),
            "foreground_unavailable: the keymap has 2 layouts and the compositor doesn't say \
             which is active; nothing was typed"
        );
        assert_eq!(events, []);
    }

    #[test]
    fn with_the_key_states_unknown_or_a_modifier_held_no_key_goes_out() {
        const SHIFT: u32 = 42;
        let local = LocalEis::start(None);
        let unknown = DesktopFacts {
            x11_app_may_have_focus: true,
            active_layout: Some(0),
            lock_keys: None,
        };

        // A held Shift would turn digits into symbols too, so nothing goes out.
        for (text, probe) in [("a", tap(SHIFT).to_vec()), ("1 2\n", vec![])] {
            let (result, events) = on_silent_compositor(&local, text, unknown);
            assert_eq!(
                result.unwrap_err().to_string(),
                "foreground_unavailable: the compositor doesn't say which lock and modifier keys \
                 are on, so keys could type something else; on KDE Plasma 6.3 to 6.5, \
                 `cua-driver kwin-capture on` lets the driver read them; nothing was typed"
            );
            assert_eq!(events, probe);
        }

        let held = DesktopFacts {
            lock_keys: Some(LockKeys {
                modifier_held: true,
                ..LockKeys::default()
            }),
            ..unknown
        };
        let (result, events) = on_silent_compositor(&local, "1", held);
        assert_eq!(
            result.unwrap_err().to_string(),
            format!("foreground_unavailable: {MODIFIER_HELD}; nothing was typed")
        );
        assert_eq!(events, []);
    }

    #[test]
    fn a_compositor_that_reports_only_changes_reports_after_the_shift_tap() {
        const SHIFT: u32 = 42;
        let local = LocalEis::start(Some(KeyboardState {
            group: DE,
            ..KeyboardState::default()
        }));
        let (result, events) = on_silent_compositor(&local, "y", facts(true));
        result.unwrap();
        assert_eq!(events, [tap(SHIFT), tap(Z)].concat());
    }

    #[test]
    fn parts_budget_grows_with_line_breaks_only() {
        assert_eq!(text_pacing("one line"), PART_SETTLE * 2);
        assert_eq!(text_pacing("a\nb"), PART_SETTLE * 2 * 3);
    }
}

#[cfg(test)]
mod key_plan_tests {
    use super::*;

    // What real layouts do, with their real actions: a key type whose levels
    // aren't Shift and AltGr, modifier keys that only work in one order, a key
    // only on a media code, and a character on two keys.
    const KEYMAP: &str = r#"xkb_keymap {
      xkb_keycodes { minimum = 8; maximum = 500;
        <AC01> = 38; <AE01> = 10; <AE02> = 11; <AE03> = 12; <AD05> = 28;
        <LFSH> = 50; <LALT> = 64; <RALT> = 108; <CAPS> = 66;
        <KP1> = 87; <KPDV> = 106; <I443> = 443; };
      xkb_types {
        virtual_modifiers NumLock;
        type "ONE_LEVEL" { modifiers = none; level_name[Level1] = "1"; };
        type "TWO_LEVEL" { modifiers = Shift; map[Shift] = Level2;
          level_name[Level1] = "1"; level_name[Level2] = "2"; };
        type "ALPHABETIC" { modifiers = Shift+Lock; map[Shift] = Level2; map[Lock] = Level2;
          level_name[Level1] = "1"; level_name[Level2] = "2"; };
        type "FOUR_LEVEL" { modifiers = Shift+Mod5;
          map[Shift] = Level2; map[Mod5] = Level3; map[Shift+Mod5] = Level4;
          level_name[Level1] = "1"; level_name[Level2] = "2"; level_name[Level3] = "3"; level_name[Level4] = "4"; };
        type "KEYPAD" { modifiers = Shift+NumLock; map[None] = Level1; map[Shift] = Level2;
          map[NumLock] = Level2; level_name[Level1] = "1"; level_name[Level2] = "2"; };
        type "ALT_LEVEL" { modifiers = Mod1; map[Mod1] = Level2;
          level_name[Level1] = "1"; level_name[Level2] = "2"; };
        type "SHIFTED_ALTGR" { modifiers = Shift; map[Shift] = Level2;
          level_name[Level1] = "1"; level_name[Level2] = "2"; };
      };
      xkb_compatibility { };
      xkb_symbols {
        key <AC01> { type = "ALPHABETIC", [ a, A ] };
        key <AE01> { type = "FOUR_LEVEL", [ 1, exclam, onesuperior, exclamdown ] };
        key <AE02> { type = "FOUR_LEVEL", [ 2, at, twosuperior, b ] };
        key <AE03> { type = "ONE_LEVEL", [ b ] };
        key <AD05> { type = "TWO_LEVEL", [ t, T ] };
        key <LFSH> { type = "ONE_LEVEL", symbols[Group1] = [ Shift_L ], actions[Group1] = [ SetMods(modifiers = Shift) ] };
        key <LALT> { type = "ONE_LEVEL", symbols[Group1] = [ Alt_L ], actions[Group1] = [ SetMods(modifiers = Mod1) ] };
        key <RALT> { type = "SHIFTED_ALTGR", symbols[Group1] = [ ISO_Level3_Shift, Multi_key ], actions[Group1] = [ SetMods(modifiers = Mod5), NoAction() ] };
        key <CAPS> { type = "ONE_LEVEL", symbols[Group1] = [ Caps_Lock ], actions[Group1] = [ LockMods(modifiers = Lock) ] };
        key <KP1> { type = "KEYPAD", [ KP_End, 1 ] };
        key <KPDV> { type = "ALT_LEVEL", [ KP_Divide, division ] };
        key <I443> { type = "ONE_LEVEL", [ EuroSign ] };
        modifier_map Shift { <LFSH> };
        modifier_map Mod1 { <LALT> };
        modifier_map Mod5 { <RALT> };
        modifier_map Lock { <CAPS> };
      };
    };"#;

    fn keymap() -> xkb::Keymap {
        xkb::Keymap::new_from_string(
            &xkb::Context::new(
                xkb::CONTEXT_NO_DEFAULT_INCLUDES | xkb::CONTEXT_NO_ENVIRONMENT_NAMES,
            ),
            KEYMAP.to_string(),
            xkb::KEYMAP_FORMAT_TEXT_V1,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .expect("test keymap compiles")
    }

    fn plan(text: &str) -> anyhow::Result<Vec<TypedKey>> {
        plan_keys(text, &keymap(), KeyboardState::default())
    }

    fn key(code: u32, modifiers: &[u32]) -> TypedKey {
        TypedKey {
            code,
            modifiers: modifiers.to_vec(),
        }
    }

    const LEFT_SHIFT: u32 = 42;
    const RIGHT_ALT: u32 = 100;

    #[test]
    fn levels_are_read_from_the_keymap_not_guessed() {
        assert_eq!(plan("A").unwrap(), [key(30, &[LEFT_SHIFT])]);
        assert_eq!(plan("T").unwrap(), [key(20, &[LEFT_SHIFT])]);
        assert_eq!(plan("¹").unwrap(), [key(2, &[RIGHT_ALT])]);
    }

    #[test]
    fn modifier_keys_go_in_the_order_that_types_the_character() {
        // Shift held first turns this AltGr key into Multi_key, so level 4 only
        // comes out with AltGr pressed first.
        assert_eq!(plan("¡").unwrap(), [key(2, &[RIGHT_ALT, LEFT_SHIFT])]);
    }

    #[test]
    fn fewest_modifiers_win_among_keys_of_the_same_rank() {
        // `b` is level 4 of one main key and level 1 of another.
        assert_eq!(plan("b").unwrap(), [key(4, &[])]);
    }

    #[test]
    fn a_character_only_on_a_keypad_level_is_not_its_keypad_symbol() {
        // Shift+KP1 gives KP_End here, as in `th`. This layout's real `1` is on
        // the main row, so the keypad key isn't used for it.
        assert_eq!(plan("1").unwrap(), [key(2, &[])]);
    }

    #[test]
    fn chords_hold_only_keys_that_just_set_a_typing_modifier() {
        // Left out: Alt, which apps read as a command, and Caps Lock, which sticks.
        assert_eq!(
            modifier_chords(&keymap(), 0),
            [
                vec![],
                vec![LEFT_SHIFT],
                vec![RIGHT_ALT],
                vec![LEFT_SHIFT, RIGHT_ALT]
            ]
        );
    }

    #[test]
    fn a_key_is_never_offered_behind_a_command_modifier() {
        let find = |c: char| {
            find_key(
                &keymap(),
                0,
                xkb::utf32_to_keysym(u32::from(c)),
                |_, mask| Some((mask.count_ones(), mask)),
            )
        };
        assert_eq!(find('÷'), None);
        assert_eq!(find('a'), Some((30, 0)));
    }

    #[test]
    fn a_character_only_behind_a_command_modifier_is_refused() {
        assert_eq!(
            plan("÷").unwrap_err().to_string(),
            "foreground_unavailable: the active keyboard layout has no key for \"÷\". Use \
             set_value on the element, or clipboard_write and then hotkey ctrl+v; nothing was typed"
        );
    }

    #[test]
    fn a_character_only_on_a_key_apps_drop_is_refused() {
        assert_eq!(
            plan("€").unwrap_err().to_string(),
            "foreground_unavailable: the active keyboard layout has no key for \"€\". Use \
             set_value on the element, or clipboard_write and then hotkey ctrl+v; nothing was typed"
        );
    }

    #[test]
    fn a_layout_the_keymap_lacks_refuses() {
        let keyboard = KeyboardState {
            group: 3,
            ..KeyboardState::default()
        };
        assert_eq!(
            plan_keys("a", &keymap(), keyboard)
                .unwrap_err()
                .to_string(),
            "foreground_unavailable: the active keyboard layout has no key for \"a\". Use set_value \
             on the element, or clipboard_write and then hotkey ctrl+v; nothing was typed"
        );
    }
}

//! Win32 agent-cursor overlay — transparent, click-through layered window.
//!
//! Matches the C# reference in CuaDriver.Win/Cursor/AgentCursorOverlay.cs:
//!
//! - Extended style: `WS_EX_TRANSPARENT | WS_EX_LAYERED | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW`
//! - Spans the virtual screen (all monitors).
//! - Render loop: dedicated STA thread, ~125 Hz (8ms timer via `SetTimer`).
//! - Pixel pipeline: `tiny-skia` → BGRA DIB → `UpdateLayeredWindow` per-pixel alpha.
//! - Z-ordering: every 80ms call `SetWindowPos` to stay just above the pinned target.
//! - Idle-hide: fade out over 180ms once `idle_hide_ms` has elapsed with no activity.
//!
//! ## Per-session cursors (2026-06 port from platform-macos #1779)
//!
//! Before this, the overlay was a process-wide singleton (one `RenderState`),
//! so concurrent MCP sessions clobbered each other last-writer-wins → one
//! shared cursor. It now keeps the shared keyed [`cursor_overlay::RenderMap`]:
//! each declared `session` owns its own cursor with its own visual state, and
//! the ~125 Hz tick composites them all into the single layered window in the
//! map's stable insertion (z) order. The lifecycle (lazy create, `session_end`
//! removal, resurrection tombstone, revival, default guard, sentinel seed) and
//! the frame-tick predicate live in `cursor_overlay` so every platform behaves
//! identically; this adapter adds per-key arrival waiters and Win32 plumbing.
//!
//! ## Cross-platform note (2026-05 dedup audit)
//!
//! Animation state + render pipeline live in `cursor_overlay::render_state`
//! (`RenderStateCore`, `tick_motion`, `apply_command_base`, `paint_cursor`).
//! What stays here is purely the Win32 window plumbing: message loop,
//! UpdateLayeredWindow paint, virtual-screen offset, z-order maintenance.

#![allow(non_snake_case, non_upper_case_globals)]

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use cursor_overlay::{
    CursorConfig, CursorKey, KeyedOverlayCommand, MotionConfig, MsgOutcome, OverlayCommand,
    OverlayMsg, RenderEntry, RenderStateCore, ScreenFrame, ZOrderEnforcer,
};

// ── Global channel ────────────────────────────────────────────────────────

static CMD_TX: OnceLock<std::sync::mpsc::SyncSender<OverlayMsg>> = OnceLock::new();
static CMD_RX_CELL: Mutex<Option<std::sync::mpsc::Receiver<OverlayMsg>>> = Mutex::new(None);
static RENDER: Mutex<Option<RenderMap>> = Mutex::new(None);

// ── Arrival-signal channels (one waiter slot per cursor key) ──────────────
//
// Each session's `animate_cursor_to` registers an arrival oneshot keyed by its
// own cursor key. A new animation only supersedes the SAME key's prior waiter,
// so concurrent sessions never cross-cancel each other's arrivals. Mirrors
// macOS so click handlers can `.await` until the cursor visually lands before
// dispatching the actual UIA / PostMessage action.
static ARRIVAL_TX: Mutex<Option<HashMap<CursorKey, tokio::sync::oneshot::Sender<()>>>> =
    Mutex::new(None);

fn arrival_register(key: CursorKey, tx: tokio::sync::oneshot::Sender<()>) {
    let mut guard = ARRIVAL_TX.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    // Cancel only the same key's previous waiter (superseded by new animation).
    if let Some(old_tx) = map.insert(key, tx) {
        let _ = old_tx.send(());
    }
}

fn arrival_fire(key: &CursorKey) {
    if let Ok(mut guard) = ARRIVAL_TX.lock() {
        if let Some(map) = guard.as_mut() {
            if let Some(tx) = map.remove(key) {
                let _ = tx.send(());
            }
        }
    }
}

/// Drop a removed session's waiter; the dropped sender releases its await.
fn arrival_cancel(key: &CursorKey) {
    if let Ok(mut guard) = ARRIVAL_TX.lock() {
        if let Some(map) = guard.as_mut() {
            map.remove(key);
        }
    }
}

// ── Keyed render collection ───────────────────────────────────────────────

/// Virtual-screen geometry and timing kept beside the shared keyed render map
/// (screen-global, written once in `run_overlay_thread`).
struct WinScreen {
    /// Virtual screen bounds (Win32 DIPs). `virt_x/y` are subtracted from each
    /// cursor's `core.pos` when rendering so the pixmap is laid out in
    /// window-local coordinates.
    virt_x: i32,
    virt_y: i32,
    virt_w: i32,
    virt_h: i32,
    /// Last WM_TIMER wall-clock stamp; used to compute real `dt` (Windows
    /// timer resolution defaults to 15ms so a hardcoded 8ms would run the
    /// animation at half speed).
    last_tick: Instant,
}

impl WinScreen {
    fn frame(&self) -> Option<ScreenFrame> {
        (self.virt_w > 0 && self.virt_h > 0).then(|| {
            ScreenFrame::new(
                f64::from(self.virt_x),
                f64::from(self.virt_y),
                f64::from(self.virt_w),
                f64::from(self.virt_h),
            )
        })
    }
}

type RenderMap = cursor_overlay::RenderMap<RenderState, WinScreen>;

/// Drain one message into the shared map, releasing a removed session's
/// arrival waiter. Returns the commanded key (also recorded as the map's
/// `last_active`).
fn apply_msg(map: &mut RenderMap, msg: OverlayMsg) -> Option<CursorKey> {
    let outcome = map.apply_msg(msg);
    if let MsgOutcome::Removed { key, .. } = &outcome {
        arrival_cancel(key);
    }
    outcome.applied_key().cloned()
}

pub fn init(cfg: CursorConfig) {
    static INITIALIZED: OnceLock<()> = OnceLock::new();
    INITIALIZED.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::sync_channel(4096);
        CMD_TX
            .set(tx)
            .expect("cursor overlay sender is initialized exactly once");
        *CMD_RX_CELL.lock().unwrap() = Some(rx);
        *ARRIVAL_TX.lock().unwrap() = Some(HashMap::new());
        *RENDER.lock().unwrap() = Some(RenderMap::new(
            cfg,
            WinScreen {
                virt_x: 0,
                virt_y: 0,
                virt_w: 1920,
                virt_h: 1080,
                last_tick: Instant::now(),
            },
        ));
    });
    cua_driver_core::cursor_events::install_cursor_event_sink(std::sync::Arc::new(
        |event: cua_driver_core::cursor_events::CursorEvent| {
            use cua_driver_core::cursor_events::{CursorEvent, CursorEventPhase};
            let (session, cmd) = match event {
                CursorEvent::SetSessionLabel { session, label } => {
                    (session, OverlayCommand::SetSessionLabel(label))
                }
                CursorEvent::Action {
                    session,
                    phase: CursorEventPhase::Begin,
                    semantics,
                } => (
                    session,
                    OverlayCommand::BeginAction {
                        action: semantics.action,
                        delivery: semantics.delivery,
                        target: semantics.target,
                    },
                ),
                CursorEvent::Action {
                    session,
                    phase: CursorEventPhase::End,
                    semantics,
                } => (session, OverlayCommand::EndAction(semantics.action)),
                CursorEvent::SelectTheme { session, selection } => (
                    session,
                    OverlayCommand::SetTheme {
                        theme_id: selection.theme_id,
                        reduced_motion: selection.reduced_motion,
                    },
                ),
            };
            send_command(session, cmd);
        },
    ));
}

/// Send a keyed command from any thread (MCP tool, etc.). Non-blocking; drops
/// if the channel is full (old commands are less important than new ones).
///
/// Empty key = anonymous (no session declared) → no cursor; the command is
/// dropped so a cursor-less run never paints. See `tools::resolve_cursor_key`.
pub fn send_command(key: CursorKey, cmd: OverlayCommand) {
    if key.is_empty() {
        return;
    }
    if let Some(tx) = CMD_TX.get() {
        let _ = tx.try_send(OverlayMsg::Cmd(KeyedOverlayCommand { key, cmd }));
        wake_overlay();
    }
}

/// Kick the overlay render timer back to the ACTIVE cadence immediately so a
/// command enqueued while the loop is parked in the slow IDLE heartbeat is
/// picked up within ~8ms instead of waiting out the full idle period (issue
/// #1808). `SetTimer` may be called cross-thread for a window owned by another
/// thread, so this is safe to invoke from the MCP tool threads. No-op until the
/// overlay window exists and a no-op when already ACTIVE.
#[cfg(target_os = "windows")]
fn wake_overlay() {
    use std::sync::atomic::Ordering::Relaxed;
    if TIMER_PERIOD_MS.load(Relaxed) == TIMER_MS_ACTIVE {
        return; // already ticking at frame cadence
    }
    let hwnd_isize = OVERLAY_HWND.load(Relaxed);
    if hwnd_isize == 0 {
        return; // overlay window not created yet
    }
    // Flip the cadence flag first so a racing WM_TIMER doesn't re-park us, then
    // arm the ACTIVE-period timer. The WM_TIMER handler re-confirms the cadence
    // from render state, so an over-eager wake just costs one cheap idle tick.
    TIMER_PERIOD_MS.store(TIMER_MS_ACTIVE, Relaxed);
    // Raise the 1 ms timer resolution alongside the ACTIVE arm — this is the
    // only wake path that bypasses the WM_TIMER re-arm (it pre-stores ACTIVE,
    // so the handler's cadence flip won't fire). `timeBeginPeriod` is
    // process-global and thread-safe; the atomic in the helper keeps the
    // begin/end calls balanced across this and the render thread.
    set_timer_resolution_raised(true);
    unsafe {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::SetTimer;
        let hwnd = HWND(hwnd_isize as *mut _);
        SetTimer(hwnd, TIMER_ID, TIMER_MS_ACTIVE, None);
    }
}

#[cfg(not(target_os = "windows"))]
fn wake_overlay() {}

/// Convenience for callsites not yet threaded with a session key: drives the
/// seeded `"default"` cursor (the anonymous / one-shot identity).
pub fn send_command_default(cmd: OverlayCommand) {
    send_command("default".to_owned(), cmd);
}

/// Remove a session's owned cursor from the render collection (fired from the
/// `session_end` hook). The `"default"` key is guarded against removal on the
/// render side, so this is a no-op for it; removing an absent key (anonymous
/// session that never created a cursor) is a harmless no-op.
pub fn remove_cursor(key: CursorKey) {
    if key.is_empty() {
        return;
    }
    if let Some(tx) = CMD_TX.get() {
        let _ = tx.try_send(OverlayMsg::Remove(key));
        wake_overlay();
    }
}

/// Clear the render-side tombstone after a successful explicit session
/// revival. Cursor recreation remains lazy until the next render command.
pub fn revive_cursor(key: CursorKey) {
    if key.is_empty() {
        return;
    }
    if let Some(tx) = CMD_TX.get() {
        let _ = tx.try_send(OverlayMsg::Revive(key));
        wake_overlay();
    }
}

/// Returns true if the cursor for `key` is currently enabled/visible. A session
/// with no own cursor yet falls back to the seeded `"default"` cursor.
pub fn is_enabled(key: &str) -> bool {
    RENDER
        .lock()
        .ok()
        .and_then(|g| {
            g.as_ref()
                .and_then(|m| m.cursor_or_default(key).map(|rs| rs.core.visible))
        })
        .unwrap_or(false)
}

/// Truthful render acknowledgement for lifecycle inspection. Unlike
/// [`is_enabled`], this checks the exact session key and never falls back to
/// the seeded default cursor.
pub fn is_visible_for_session(key: &str) -> bool {
    RENDER
        .lock()
        .ok()
        .and_then(|guard| {
            guard
                .as_ref()
                .and_then(|map| map.cursors.get(key))
                .map(|rs| {
                    rs.core.cfg.enabled
                        && rs.core.visible
                        && rs.core.idle_alpha >= 0.004
                        && rs.core.pos.0 >= -100.0
                })
        })
        .unwrap_or(false)
}

/// Snapshot the current motion config for `key`, falling back to the
/// `"default"` cursor's motion when that key has no own entry yet.
pub fn current_motion(key: &str) -> MotionConfig {
    RENDER
        .lock()
        .ok()
        .and_then(|g| {
            g.as_ref()
                .and_then(|m| m.cursor_or_default(key).map(|rs| rs.core.motion.clone()))
        })
        .unwrap_or_default()
}

pub fn current_theme_state(
    key: &str,
) -> Option<(
    String,
    String,
    String,
    Option<String>,
    cursor_overlay::CursorVisualState,
)> {
    let guard = RENDER.lock().ok()?;
    let map = guard.as_ref()?;
    let state = map.cursor_or_default(key)?;
    let (id, version, profile, fallback) = state.core.active_theme_metadata();
    Some((id, version, profile, fallback, state.core.visual.clone()))
}

/// Current screen position of the cursor for `key` (the off-screen sentinel
/// `(-200, -200)` if it has never been placed). A session with no own cursor
/// yet reports the sentinel so the click path treats it as first-placement.
pub fn current_position(key: &str) -> (f64, f64) {
    RENDER
        .lock()
        .ok()
        .and_then(|g| {
            g.as_ref()
                .and_then(|m| m.cursors.get(key))
                .map(|rs| rs.core.pos)
        })
        .unwrap_or((-200.0, -200.0))
}

/// Seed a brand-new (sentinel-positioned) cursor at an on-screen start point
/// offset up-left of `(target_x, target_y)` so the immediately-following
/// `MoveTo` glides INTO the target instead of silently snapping. No-op when the
/// cursor is already on-screen or its session already ended. Returns true if a
/// seed was applied. Mirrors `platform_macos::cursor::overlay::seed_start_*`.
fn seed_start_if_sentinel(key: &CursorKey, target_x: f64, target_y: f64) -> bool {
    let mut guard = RENDER.lock().unwrap();
    let Some(map) = guard.as_mut() else {
        return false;
    };
    let frame = map.platform.frame();
    map.seed_start_if_sentinel(key, target_x, target_y, frame)
}

/// Animate the overlay cursor for `key` to `(x, y)` and suspend until the
/// planned path completes (the spring-settle phase that follows is allowed to
/// keep running — we only wait for the visible glide to land).
///
/// Returns immediately (no animation, no wait) when:
/// - the key is empty (anonymous run → no cursor), or
/// - the cursor for `key` is disabled.
///
/// A brand-new cursor still at the off-screen sentinel is first seeded
/// on-screen via [`seed_start_if_sentinel`] so its FIRST action glides in.
/// Mirrors `platform_macos::cursor::overlay::animate_cursor_to`.
pub async fn animate_cursor_to(key: CursorKey, x: f64, y: f64) {
    if key.is_empty() {
        return;
    }
    // Seed a sentinel cursor on-screen so the MoveTo below glides instead of
    // being short-circuited.
    seed_start_if_sentinel(&key, x, y);

    let should_animate = {
        let guard = RENDER.lock().unwrap();
        match guard.as_ref().and_then(|m| m.cursors.get(&key)) {
            Some(rs) if rs.core.cfg.enabled && rs.core.pos.0 > -50.0 => true,
            _ => false,
        }
    };
    if !should_animate {
        return;
    }

    // Install the keyed oneshot sender BEFORE issuing MoveTo, so the render
    // thread's arrival-fire can never lose a race against an immediate
    // path-end (e.g. zero-length glide).
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    arrival_register(key.clone(), tx);

    send_command(
        key,
        OverlayCommand::MoveTo {
            x,
            y,
            // Arrive pointing upper-left (45°) — same convention as macOS /
            // Swift reference (`endAngleDegrees: 45`).
            end_heading_radians: std::f64::consts::FRAC_PI_4,
        },
    );

    let _ = rx.await;
}

/// Spin up the overlay on a dedicated thread (STA for Win32 message loop).
/// This is a non-blocking call — the overlay runs on its own thread.
pub fn run_on_thread() {
    let rx = match CMD_RX_CELL.lock().unwrap().take() {
        Some(r) => r,
        None => return, // init() not called; overlay disabled
    };

    let cfg = {
        let guard = RENDER.lock().unwrap();
        match &*guard {
            Some(m) => m.template.clone(),
            None => return,
        }
    };

    if !cfg.enabled {
        return;
    }

    std::thread::Builder::new()
        .name("cua-overlay-win".into())
        .spawn(move || {
            // Windows message loops must run on the same thread that created the window.
            run_overlay_thread(cfg, rx);
        })
        .expect("spawn overlay thread");
}

// ── Animation / render state ──────────────────────────────────────────────
//
// The platform-agnostic fields + tick + apply_command + render pipeline live
// in `cursor_overlay::render_state`. What stays here is just the per-cursor
// wrapper; the virtual-screen geometry + dt stamp live in `WinScreen`.

struct RenderState {
    core: RenderStateCore,
}

impl RenderEntry for RenderState {
    fn from_config(cfg: CursorConfig) -> Self {
        RenderState {
            core: RenderStateCore::new(cfg),
        }
    }

    fn core(&self) -> &RenderStateCore {
        &self.core
    }

    fn core_mut(&mut self) -> &mut RenderStateCore {
        &mut self.core
    }

    fn apply_command(&mut self, cmd: OverlayCommand) -> bool {
        // Windows uses the non-sentinel-snap behaviour for both MoveTo and
        // ClickPulse: every command updates `self.pos` unconditionally.
        // `ShowFocusRect` is not rendered on Windows — `apply_command_base`
        // returns `false` for it and we silently drop it here.
        self.core.apply_command_base(cmd, false, false)
    }

    // `tick` (arrival on path end) and `needs_frame_tick` use the shared
    // defaults. The shared predicate keeps a revealed cursor with resting
    // motion (the float bob) at the ACTIVE cadence, and parks a reduced-motion
    // cursor through its constant-alpha idle-hide countdown; that countdown is
    // advanced by [`RenderState::in_idle_countdown`] at the slow IDLE cadence.
}

impl RenderState {
    /// True while the cursor rests on-screen at full alpha with idle-hide
    /// pending: pixels are static (alpha pinned at 1.0) but `idle_secs` must
    /// keep accruing wall-clock time so the fade still starts on schedule.
    /// Ticked at the slow IDLE cadence; see the `real_dt` catch-up in the
    /// WM_TIMER handler, which compensates for the 0.05 s motion-dt clamp.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    fn in_idle_countdown(&self) -> bool {
        self.core.path.is_none()
            && self.core.spring.is_none()
            && self.core.click_t.is_none()
            && self.core.motion.idle_hide_ms > 0.0
            && self.core.visible
            && self.core.pos.0 >= -100.0
            && self.core.idle_alpha >= 1.0
    }
}

// ── Win32 message-loop thread ─────────────────────────────────────────────

#[cfg(target_os = "windows")]
fn run_overlay_thread(cfg: CursorConfig, rx: std::sync::mpsc::Receiver<OverlayMsg>) {
    use windows::core::PCWSTR;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::*;

    // NOTE: the 1 ms multimedia timer resolution (`timeBeginPeriod`) is NOT
    // raised unconditionally here any more — it is scoped to the ACTIVE
    // render cadence via `set_timer_resolution_raised` so an idle daemon no
    // longer holds the global resolution at 1 ms for its whole lifetime
    // (it raises system-wide timer-interrupt/wakeup rates and power draw).

    // Collect virtual screen bounds (all monitors).
    let virt_x = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
    let virt_y = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
    let virt_w = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
    let virt_h = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };

    // Update render map with virtual screen bounds.
    {
        let mut guard = RENDER.lock().unwrap();
        if let Some(map) = guard.as_mut() {
            map.platform = WinScreen {
                virt_x,
                virt_y,
                virt_w,
                virt_h,
                last_tick: Instant::now(),
            };
        }
    }

    // Register window class. Class + title use the `Cua.` namespace.
    let class_name_w: Vec<u16> = "Cua.AgentCursorOverlay\0".encode_utf16().collect();
    let title_w: Vec<u16> = format!("Cua.AgentCursorOverlay.{}\0", cfg.cursor_id)
        .encode_utf16()
        .collect();

    let hinstance = unsafe { GetModuleHandleW(PCWSTR::null()).unwrap_or_default() };

    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(wnd_proc),
        hInstance: hinstance.into(),
        lpszClassName: PCWSTR(class_name_w.as_ptr()),
        ..Default::default()
    };
    unsafe {
        RegisterClassExW(&wc);
    } // ignore error if already registered

    // WS_EX_TRANSPARENT | WS_EX_LAYERED | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW
    let ex_style = WS_EX_TRANSPARENT | WS_EX_LAYERED | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW;
    let style = WS_POPUP;

    let hwnd = unsafe {
        CreateWindowExW(
            ex_style,
            PCWSTR(class_name_w.as_ptr()),
            PCWSTR(title_w.as_ptr()),
            style,
            virt_x,
            virt_y,
            virt_w,
            virt_h,
            None,
            None,
            hinstance,
            None,
        )
    };

    if hwnd.is_err() {
        tracing::error!("Win32 overlay: CreateWindowExW failed");
        return;
    }
    let hwnd = hwnd.unwrap();

    // Show without activation (mirrors ShowWithoutActivation in C# ref).
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }

    // Arm the render timer in the ACTIVE cadence (~125 Hz) so the very first
    // frames (seed glide / startup) are smooth. The WM_TIMER handler drops it
    // to the slow IDLE cadence as soon as every cursor goes quiescent and
    // re-arms ACTIVE the instant a command arrives (issue #1808).
    unsafe {
        SetTimer(hwnd, TIMER_ID, TIMER_MS_ACTIVE, None);
    }
    TIMER_PERIOD_MS.store(TIMER_MS_ACTIVE, std::sync::atomic::Ordering::Relaxed);
    // Timer starts ACTIVE, so the 1 ms resolution is needed until the first
    // quiescent tick parks the loop (and lowers it again).
    set_timer_resolution_raised(true);

    // Store hwnd and rx globally for the wnd_proc callback.
    OVERLAY_HWND.store(hwnd.0 as isize, std::sync::atomic::Ordering::Relaxed);
    *CMD_RX_WIN.lock().unwrap() = Some(rx);
    LAST_ZTICK.store(0, std::sync::atomic::Ordering::Relaxed);
    let _ = Z_ORDER.set(WinZOrderEnforcer {
        hwnd_isize: hwnd.0 as isize,
    });

    // Standard Win32 message loop.
    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    // Message loop ended (WM_DESTROY): release the raised timer resolution
    // if the loop went down while still at the ACTIVE cadence.
    set_timer_resolution_raised(false);
}

#[cfg(not(target_os = "windows"))]
fn run_overlay_thread(_cfg: CursorConfig, _rx: std::sync::mpsc::Receiver<OverlayMsg>) {
    // No-op on non-Windows targets (cross-compile guard).
}

// ── Win32 globals (only used on Windows) ─────────────────────────────────

static OVERLAY_HWND: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
static CMD_RX_WIN: Mutex<Option<std::sync::mpsc::Receiver<OverlayMsg>>> = Mutex::new(None);
static LAST_ZTICK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static Z_ORDER: OnceLock<WinZOrderEnforcer> = OnceLock::new();

// ── Scoped multimedia timer resolution ────────────────────────────────────
//
// `timeBeginPeriod(1)` raises the GLOBAL Windows timer resolution to 1 ms —
// more timer interrupts and CPU wakeups machine-wide. The overlay only needs
// it while WM_TIMER runs at the 8 ms ACTIVE cadence, so it is raised on the
// IDLE→ACTIVE flip and released on ACTIVE→IDLE instead of being held for the
// daemon's whole lifetime (the old behaviour leaked it: `timeBeginPeriod` at
// thread start with no `timeEndPeriod` anywhere).
/// Tracks whether we currently hold a `timeBeginPeriod(1)` request. The lock
/// covers both the state transition and the WinMM call: an atomic swap alone
/// permits `timeEndPeriod` on one thread to overtake a delayed
/// `timeBeginPeriod` on another, leaving the process raised while the flag
/// says it is not.
static TIMER_RES_RAISED: Mutex<bool> = Mutex::new(false);

#[cfg(target_os = "windows")]
fn set_timer_resolution_raised(raise: bool) {
    use windows::Win32::Media::{timeBeginPeriod, timeEndPeriod};
    let mut raised = TIMER_RES_RAISED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if *raised == raise {
        return;
    }
    unsafe {
        if raise {
            let _ = timeBeginPeriod(1);
        } else {
            let _ = timeEndPeriod(1);
        }
    }
    *raised = raise;
}

#[cfg(not(target_os = "windows"))]
fn set_timer_resolution_raised(_raise: bool) {}

// ── Dirty-rect render surface ─────────────────────────────────────────────
//
// The composite path used to rebuild everything per frame: allocate + zero a
// full virtual-screen RGBA pixmap (~28 MB on a large desktop), create a full
// DIB section, swizzle EVERY pixel RGBA→BGRA, `UpdateLayeredWindow` the whole
// surface, then free it all — ~52 ms/frame measured, which saturated a core
// whenever the loop ran at frame cadence. Cursors only ever touch a tiny
// region, so the surface is now persistent and only the union of last frame's
// and this frame's cursor bounds is cleared, repainted, swizzled, and pushed
// via `UpdateLayeredWindowIndirect(prcDirty)`. The surface is freed whenever
// the loop parks at the IDLE cadence (the DWM keeps its own copy of the
// layered surface), so idle memory returns to baseline.

/// Half-open pixel rect `[x0, x1) × [y0, y1)` in overlay-window-local
/// coordinates.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq)]
struct DirtyRect {
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
impl DirtyRect {
    fn union(a: Option<DirtyRect>, b: Option<DirtyRect>) -> Option<DirtyRect> {
        match (a, b) {
            (Some(a), Some(b)) => Some(DirtyRect {
                x0: a.x0.min(b.x0),
                y0: a.y0.min(b.y0),
                x1: a.x1.max(b.x1),
                y1: a.y1.max(b.y1),
            }),
            (r, None) | (None, r) => r,
        }
    }

    /// Clamp to `[0, w) × [0, h)`; `None` if nothing remains.
    fn clamped(self, w: i32, h: i32) -> Option<DirtyRect> {
        let r = DirtyRect {
            x0: self.x0.max(0),
            y0: self.y0.max(0),
            x1: self.x1.min(w),
            y1: self.y1.min(h),
        };
        (r.x1 > r.x0 && r.y1 > r.y0).then_some(r)
    }
}

/// Conservative half-extent for the checked-in theme plus the session badge.
/// The badge is up to 188 px wide and clamps independently at display edges,
/// so a cursor at an edge can have badge pixels almost 188 px to one side.
/// Custom themes use a full-surface dirty rect because their bounded artifact
/// coordinates can legitimately extend beyond this default-theme envelope.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const CURSOR_PAD: i32 = cursor_overlay::session_badge::BADGE_MAX_WIDTH as i32 + 12;

#[cfg(target_os = "windows")]
struct WinSurface {
    pixmap: tiny_skia::Pixmap,
    /// Memory DC with the DIB selected, stored as isize (HDC is !Send).
    hdc_mem: isize,
    /// The DIB section bitmap handle.
    hbmp: isize,
    /// Pointer to the DIB's BGRA pixel bits (owned by the DIB section).
    bits: *mut u8,
    w: i32,
    h: i32,
    virt_x: i32,
    virt_y: i32,
}

#[cfg(target_os = "windows")]
impl WinSurface {
    unsafe fn create(virt_x: i32, virt_y: i32, w: i32, h: i32) -> Option<WinSurface> {
        use windows::Win32::Graphics::Gdi::*;

        let pixmap = tiny_skia::Pixmap::new(w.max(1) as u32, h.max(1) as u32)?;
        let hdc_screen = GetDC(None);
        let hdc_mem = CreateCompatibleDC(hdc_screen);
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // negative = top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits_ptr = std::ptr::null_mut::<std::ffi::c_void>();
        let hbmp = CreateDIBSection(hdc_mem, &bmi, DIB_RGB_COLORS, &mut bits_ptr, None, 0);
        ReleaseDC(None, hdc_screen);
        let Ok(hbmp) = hbmp else {
            let _ = DeleteDC(hdc_mem);
            return None;
        };
        if bits_ptr.is_null() {
            let _ = DeleteObject(hbmp);
            let _ = DeleteDC(hdc_mem);
            return None;
        }
        SelectObject(hdc_mem, hbmp);
        Some(WinSurface {
            pixmap,
            hdc_mem: hdc_mem.0 as isize,
            hbmp: hbmp.0 as isize,
            bits: bits_ptr as *mut u8,
            w,
            h,
            virt_x,
            virt_y,
        })
    }

    /// Zero the pixmap pixels inside `r` (rect must already be clamped).
    fn clear_rect(&mut self, r: DirtyRect) {
        let w = self.w as usize;
        let data = self.pixmap.data_mut();
        let row_len = (r.x1 - r.x0) as usize * 4;
        for y in r.y0..r.y1 {
            let start = (y as usize * w + r.x0 as usize) * 4;
            data[start..start + row_len].fill(0);
        }
    }

    /// Copy the pixels inside `r` from the pixmap (premultiplied RGBA) into
    /// the DIB bits (premultiplied BGRA). Only the dirty region is touched —
    /// this replaces the old full-surface per-frame swizzle.
    fn swizzle_rect(&mut self, r: DirtyRect) {
        let w = self.w as usize;
        let src = self.pixmap.data();
        // SAFETY: `bits` points at a live DIB section of exactly w*h*4 bytes;
        // the DIB outlives `self` (freed only in Drop) and only this (overlay)
        // thread touches it.
        let dst = unsafe { std::slice::from_raw_parts_mut(self.bits, w * self.h as usize * 4) };
        for y in r.y0..r.y1 {
            let row = (y as usize * w + r.x0 as usize) * 4;
            let row_end = (y as usize * w + r.x1 as usize) * 4;
            let (s, d) = (&src[row..row_end], &mut dst[row..row_end]);
            for i in (0..s.len()).step_by(4) {
                d[i] = s[i + 2]; // B
                d[i + 1] = s[i + 1]; // G
                d[i + 2] = s[i]; // R
                d[i + 3] = s[i + 3]; // A
            }
        }
    }
}

#[cfg(target_os = "windows")]
impl Drop for WinSurface {
    fn drop(&mut self) {
        use windows::Win32::Graphics::Gdi::{DeleteDC, DeleteObject, HBITMAP, HDC};
        unsafe {
            let _ = DeleteObject(HBITMAP(self.hbmp as *mut _));
            let _ = DeleteDC(HDC(self.hdc_mem as *mut _));
        }
    }
}

#[cfg(target_os = "windows")]
thread_local! {
    /// The persistent render surface. Overlay-thread only. `None` while the
    /// loop is parked at the IDLE cadence (freed to keep idle memory flat).
    static SURFACE: std::cell::RefCell<Option<WinSurface>> =
        const { std::cell::RefCell::new(None) };
    /// Window-local region painted by the previous rendered frame. Survives
    /// surface teardown so the first frame after a re-park correctly clears
    /// the stale cursor pixels the DWM is still displaying.
    static PREV_DIRTY: std::cell::Cell<Option<DirtyRect>> =
        const { std::cell::Cell::new(None) };
    /// The first `UpdateLayeredWindowIndirect` for each newly-created surface
    /// must push the full surface to establish the layered-window backing
    /// store. Surface recreation and idle teardown reset this flag.
    static FIRST_PRESENT: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

/// Paint this frame's cursors into the persistent surface and return the
/// window-local region that must be pushed to the screen (`None` = nothing
/// painted and nothing stale to erase — skip the present entirely).
#[cfg(target_os = "windows")]
fn composite_dirty(map: &RenderMap) -> Option<DirtyRect> {
    let screen = &map.platform;
    let (w, h) = (screen.virt_w.max(1), screen.virt_h.max(1));
    SURFACE.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.as_ref().map(|s| (s.virt_x, s.virt_y, s.w, s.h))
            != Some((screen.virt_x, screen.virt_y, w, h))
        {
            *slot = unsafe { WinSurface::create(screen.virt_x, screen.virt_y, w, h) };
            FIRST_PRESENT.with(|first| first.set(true));
        }
        let surf = slot.as_mut()?;

        // Union of every cursor that will produce pixels this frame
        // (mirrors paint_cursor's own visibility early-return).
        let mut current: Option<DirtyRect> = None;
        for rs in map.cursors.values() {
            if !rs.core.visible || rs.core.pos.0 < -100.0 || rs.core.idle_alpha < 0.004 {
                continue;
            }
            let cx = (rs.core.pos.0 - screen.virt_x as f64).round() as i32;
            let cy = (rs.core.pos.1 - screen.virt_y as f64).round() as i32;
            let custom_theme = rs
                .core
                .theme
                .as_deref()
                .is_some_and(|theme| theme.id != cursor_overlay::DEFAULT_THEME_ID);
            let r = if custom_theme {
                Some(DirtyRect {
                    x0: 0,
                    y0: 0,
                    x1: w,
                    y1: h,
                })
            } else {
                DirtyRect {
                    x0: cx - CURSOR_PAD,
                    y0: cy - CURSOR_PAD,
                    x1: cx + CURSOR_PAD,
                    y1: cy + CURSOR_PAD,
                }
                .clamped(w, h)
            };
            current = DirtyRect::union(current, r);
        }

        let prev = PREV_DIRTY.with(std::cell::Cell::get);
        for r in [prev, current].into_iter().flatten() {
            surf.clear_rect(r);
        }
        for rs in map.cursors.values() {
            cursor_overlay::paint_cursor(
                &mut surf.pixmap,
                &rs.core,
                screen.virt_x as f64,
                screen.virt_y as f64,
                None, // focus-rect is macOS-only
                1.0,
            );
        }
        PREV_DIRTY.with(|p| p.set(current));

        let upload = DirtyRect::union(prev, current)?;
        surf.swizzle_rect(upload);
        Some(upload)
    })
}

/// Push `dirty` from the persistent surface to the layered window via
/// `UpdateLayeredWindowIndirect`. The DWM copies the region into its own
/// backing store, so the surface itself may be freed afterwards.
#[cfg(target_os = "windows")]
unsafe fn present_surface(hwnd: windows::Win32::Foundation::HWND, dirty: DirtyRect) {
    use windows::Win32::Foundation::{COLORREF, POINT, RECT, SIZE};
    use windows::Win32::Graphics::Gdi::{GetDC, ReleaseDC, BLENDFUNCTION, HDC};
    use windows::Win32::UI::WindowsAndMessaging::{
        UpdateLayeredWindowIndirect, ULW_ALPHA, UPDATELAYEREDWINDOWINFO,
    };

    SURFACE.with(|cell| {
        let slot = cell.borrow();
        let Some(surf) = slot.as_ref() else { return };
        let dirty = if FIRST_PRESENT.with(std::cell::Cell::get) {
            DirtyRect {
                x0: 0,
                y0: 0,
                x1: surf.w,
                y1: surf.h,
            }
        } else {
            dirty
        };
        let hdc_screen = GetDC(None);
        let pt_dst = POINT {
            x: surf.virt_x,
            y: surf.virt_y,
        };
        let sz = SIZE {
            cx: surf.w,
            cy: surf.h,
        };
        let pt_src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: 0, // AC_SRC_OVER
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: 1, // AC_SRC_ALPHA
        };
        let rc_dirty = RECT {
            left: dirty.x0,
            top: dirty.y0,
            right: dirty.x1,
            bottom: dirty.y1,
        };
        let info = UPDATELAYEREDWINDOWINFO {
            cbSize: std::mem::size_of::<UPDATELAYEREDWINDOWINFO>() as u32,
            hdcDst: hdc_screen,
            pptDst: &pt_dst,
            psize: &sz,
            hdcSrc: HDC(surf.hdc_mem as *mut _),
            pptSrc: &pt_src,
            crKey: COLORREF(0),
            pblend: &blend,
            dwFlags: ULW_ALPHA,
            prcDirty: &rc_dirty,
        };
        if UpdateLayeredWindowIndirect(hwnd, &info).as_bool() {
            FIRST_PRESENT.with(|f| f.set(false));
        }
        ReleaseDC(None, hdc_screen);
    });
}

// ── Idle render gate (issue #1808) ────────────────────────────────────────
//
// The overlay window timer is re-armed between three cadences:
//   * ACTIVE  (`TIMER_MS_ACTIVE`, ~125 Hz) while any cursor is animating /
//     fading — this is what produces a smooth glide + click pulse.
//   * HOVER   (`TIMER_MS_HOVER`, 12.5 Hz) while a revealed cursor can show its
//     session badge again under the user's hardware pointer.
//   * IDLE    (`TIMER_MS_IDLE`, a slow heartbeat) when every cursor is
//     quiescent — the handler then only drains the command channel cheaply
//     and re-arms ACTIVE the instant a command arrives. No full-screen pixmap
//     allocation, no RGBA→BGRA copy, no UpdateLayeredWindow while idle.
//
// Before this gate the timer ran at ~125 Hz unconditionally and every tick
// allocated a virtual-screen pixmap, swizzled it pixel-by-pixel, and blitted
// it with UpdateLayeredWindow — burning 60–85% of a core with the cursor
// static (issue #1808). `TIMER_PERIOD_MS` is the cadence the timer is currently
// armed at; the WM_TIMER handler flips it based on `RenderMap::needs_frame_tick`.
const TIMER_ID: usize = 1;
const TIMER_MS_ACTIVE: u32 = 8; // ~125 Hz, matches the C# reference render rate
const TIMER_MS_HOVER: u32 = 80; // low-cost hardware-pointer hover sampling
const TIMER_MS_IDLE: u32 = 250; // slow heartbeat: drain channel, stay responsive
/// Current armed timer cadence in ms. Compared against the desired cadence each
/// WM_TIMER so we only call `SetTimer` when the desired cadence changes.
static TIMER_PERIOD_MS: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(TIMER_MS_ACTIVE);

// ── Window procedure ──────────────────────────────────────────────────────

#[cfg(target_os = "windows")]
unsafe extern "system" fn wnd_proc(
    hwnd: windows::Win32::Foundation::HWND,
    msg: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::Foundation::*;
    use windows::Win32::UI::WindowsAndMessaging::*;

    match msg {
        WM_TIMER => {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;

            // ── Optional overlay-FPS probe ───────────────────────────────────
            // Set CUA_DRIVER_RS_OVERLAY_FPS_FILE=<path> to append a measured
            // render-FPS line ~once/sec. Diagnostic only; when the env var is
            // unset this is a single OnceLock read + branch (no behaviour change).
            {
                use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
                static FPS_PATH: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
                static FPS_FRAMES: AtomicU64 = AtomicU64::new(0);
                static FPS_LAST: AtomicU64 = AtomicU64::new(0);
                if let Some(path) =
                    FPS_PATH.get_or_init(|| std::env::var("CUA_DRIVER_RS_OVERLAY_FPS_FILE").ok())
                {
                    let n = FPS_FRAMES.fetch_add(1, Relaxed) + 1;
                    let last = FPS_LAST.load(Relaxed);
                    if last == 0 {
                        FPS_LAST.store(now_ms, Relaxed);
                    } else if now_ms.wrapping_sub(last) >= 1000 {
                        let secs = (now_ms - last) as f64 / 1000.0;
                        let fps = n as f64 / secs.max(1e-3);
                        let cursors = RENDER
                            .lock()
                            .ok()
                            .and_then(|g| g.as_ref().map(|m| m.cursors.len()))
                            .unwrap_or(0);
                        if let Ok(mut f) = std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(path)
                        {
                            use std::io::Write;
                            let _ = writeln!(
                                f,
                                "overlay fps={fps:.1} avg_dt_ms={:.1} cursors={cursors}",
                                secs * 1000.0 / n as f64
                            );
                        }
                        FPS_FRAMES.store(0, Relaxed);
                        FPS_LAST.store(now_ms, Relaxed);
                    }
                }
            }

            // ── Drain commands, tick all cursors, maybe composite one pixmap ─
            // Measure real dt from last tick — Windows timer resolution defaults
            // to 15ms so the hardcoded 8ms ran the animation at half speed.
            //
            // Idle gate (issue #1808): the full-screen composite + RGBA→BGRA
            // swizzle + UpdateLayeredWindow only runs when a command arrived
            // this tick, when a previous tick left an animation in flight
            // (`was_active`), or when a cursor is still animating/fading after
            // this tick (`needs_tick`). When all three are false every cursor is
            // quiescent and the layered window already holds its resting frame,
            // so we skip the expensive work entirely and let the timer drop to
            // the slow IDLE cadence below.
            let was_active =
                TIMER_PERIOD_MS.load(std::sync::atomic::Ordering::Relaxed) == TIMER_MS_ACTIVE;
            let (upload, arrived, pinned_wid, needs_tick, needs_hover_poll) = {
                let mut guard = RENDER.lock().unwrap();
                if let Some(map) = guard.as_mut() {
                    // Drain the channel via get-or-create; the shared map
                    // tracks the last-touched key for z-order pinning.
                    let mut had_msg = false;
                    if let Ok(rx_guard) = CMD_RX_WIN.try_lock() {
                        if let Some(ref rx) = *rx_guard {
                            while let Ok(m) = rx.try_recv() {
                                had_msg = true;
                                apply_msg(map, m);
                            }
                        }
                    }

                    let now = Instant::now();
                    let real_dt = now.duration_since(map.platform.last_tick).as_secs_f64();
                    // Clamped dt for the motion physics: a large gap between
                    // ticks must not teleport an in-flight glide.
                    let dt = real_dt.clamp(0.0, 0.05);
                    map.platform.last_tick = now;

                    // Tick every cursor; record the ones that just arrived.
                    let mut arrived: Vec<CursorKey> = Vec::new();
                    for (k, rs) in map.cursors.iter_mut() {
                        if rs.tick(dt) {
                            arrived.push(k.clone());
                        }
                        // Idle-countdown wall-clock catch-up (render-gate fix):
                        // cursors parked in the constant-alpha countdown tick at
                        // the slow IDLE cadence, where the 0.05 s motion clamp
                        // would stretch the `idle_hide_ms` countdown ~5x. Credit
                        // the unclamped remainder so the fade starts on real
                        // time — but never advance past `fade_start`, so the
                        // 180 ms fade itself always plays out at frame cadence
                        // (a single 250 ms step would otherwise skip the fade
                        // and pop the cursor out).
                        if real_dt > dt && rs.in_idle_countdown() {
                            let fade_start = rs.core.motion.idle_hide_ms / 1000.0;
                            rs.core.idle_secs =
                                (rs.core.idle_secs + (real_dt - dt)).min(fade_start);
                        }
                    }
                    let mut pointer = POINT::default();
                    let pointer = GetCursorPos(&mut pointer)
                        .ok()
                        .map(|_| (f64::from(pointer.x), f64::from(pointer.y)));
                    let mut hover_changed = false;
                    for rs in map.cursors.values_mut() {
                        hover_changed |= rs.core.update_session_badge_hover(pointer);
                    }

                    // After ticking: does any cursor still need frame ticks?
                    let needs_tick = map.needs_frame_tick();
                    let needs_hover_poll = map
                        .cursors
                        .values()
                        .any(|rs| rs.core.session_badge_needs_hover_poll());

                    // Render only when something can have changed pixels this
                    // frame: a fresh command, a still-running animation, or the
                    // final settle frame as the previous animation winds down
                    // (`was_active && !needs_tick`). A fully-quiescent idle tick
                    // returns `None` here and does no compositing at all.
                    let should_render = had_msg || hover_changed || needs_tick || was_active;

                    if !should_render {
                        (None, arrived, None, needs_tick, needs_hover_poll)
                    } else {
                        // Decide where to pin the single overlay window in z.
                        //
                        // The overlay is ONE full-virtual-screen layered window,
                        // so it can occupy only one z-slot. It must sit ABOVE
                        // every window a live cursor is actuating, but NOT above
                        // whatever sits above those (the user's foreground). The
                        // right slot is therefore "just above the HIGHEST-z
                        // actuating window": the overlay is full-screen, so being
                        // above the topmost driven window puts it above all of
                        // them while still below anything stacked above them.
                        // Pinning above one fixed window (the old last-active
                        // behaviour) instead let any other driven window stacked
                        // above it occlude its cursors — the blink-out. NB: this
                        // is a RELATIVE z move (insert above a specific window),
                        // which works from this non-foreground thread; an
                        // absolute HWND_TOP can be refused by the foreground lock
                        // and sink the overlay behind everything.
                        let mut driven: Vec<u64> = Vec::new();
                        for rs in map.cursors.values() {
                            if !rs.core.visible || rs.core.idle_alpha < 0.004 {
                                continue;
                            }
                            if let Some(w) = rs.core.pinned_wid {
                                if !driven.contains(&w) {
                                    driven.push(w);
                                }
                            }
                        }
                        let pinned = unsafe { topmost_of(&driven) };

                        // Dirty-rect composite: paint only the union of last
                        // frame's and this frame's cursor bounds into the
                        // persistent surface (see `composite_dirty`). The old
                        // path re-allocated, fully repainted, and fully
                        // swizzled a virtual-screen pixmap every frame —
                        // ~52 ms/frame on a large desktop.
                        //
                        // TODO: thread `GetDpiForWindow` / per-monitor DPI
                        // awareness into the paint so cursors render crisp on
                        // HiDPI Windows displays (backing_scale stays 1.0 —
                        // preserves pre-retina-fix behaviour on Windows).
                        let upload = composite_dirty(map);

                        (upload, arrived, pinned, needs_tick, needs_hover_poll)
                    }
                } else {
                    (None, Vec::new(), None, false, false)
                }
            };

            if let Some(dirty) = upload {
                unsafe { present_surface(hwnd, dirty) };

                // Z-order maintenance every 80ms — delegate to the cross-platform
                // ZOrderEnforcer so the contract for "z+1 of the application under
                // test" is documented once in `cursor_overlay::z_order`. Only run
                // while we actually rendered: a quiescent overlay leaves its z-slot
                // untouched until the next command wakes the loop.
                let last = LAST_ZTICK.load(std::sync::atomic::Ordering::Relaxed);
                if now_ms.wrapping_sub(last) >= 80 {
                    LAST_ZTICK.store(now_ms, std::sync::atomic::Ordering::Relaxed);
                    if let Some(enforcer) = Z_ORDER.get() {
                        enforcer.reassert(pinned_wid);
                    }
                }
            }

            // Fire arrival oneshots for cursors whose path just ended — unblocks
            // each session's `animate_cursor_to(...).await` so the click action
            // only dispatches once that cursor has visually landed.
            for k in &arrived {
                arrival_fire(k);
            }

            // ── Re-arm the render timer at the cadence the current state needs ─
            // ACTIVE (~125 Hz) while animating/fading; IDLE (slow heartbeat) once
            // quiescent so a static cursor stops burning CPU (issue #1808). We
            // only call SetTimer on an actual cadence flip — re-arming with the
            // same period every tick would itself be needless work.
            let desired_ms = if needs_tick {
                TIMER_MS_ACTIVE
            } else if needs_hover_poll {
                TIMER_MS_HOVER
            } else {
                TIMER_MS_IDLE
            };
            // Hold the 1 ms global timer resolution only while at the ACTIVE
            // cadence (the helper no-ops unless the raised state changes).
            set_timer_resolution_raised(desired_ms == TIMER_MS_ACTIVE);
            if TIMER_PERIOD_MS.swap(desired_ms, std::sync::atomic::Ordering::Relaxed) != desired_ms
            {
                unsafe {
                    SetTimer(hwnd, TIMER_ID, desired_ms, None);
                }
                if desired_ms == TIMER_MS_IDLE {
                    // Parked: free the render surface (pixmap + DIB, tens of
                    // MB on a large desktop). The DWM keeps its own copy of
                    // the layered window's contents, and PREV_DIRTY survives
                    // so the next rendered frame clears any stale cursor
                    // pixels before presenting.
                    SURFACE.with(|cell| {
                        cell.borrow_mut().take();
                    });
                    FIRST_PRESENT.with(|first| first.set(true));
                }
            }

            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

// ── Z-order enforcer (Windows impl of cursor_overlay::ZOrderEnforcer) ────

/// Win32 implementation of [`cursor_overlay::ZOrderEnforcer`].
///
/// Stores the overlay HWND as an `isize` (HWND is `*mut c_void` and not
/// `Send`/`Sync`) and rehydrates it inside `reassert`. Driven by the
/// `WM_TIMER` branch in `wnd_proc` on the overlay STA thread.
struct WinZOrderEnforcer {
    hwnd_isize: isize,
}

/// Of the given window ids, return the one highest in the current z-order (the
/// first encountered walking top→bottom), or `None` if none are present. Used to
/// pick the single window the overlay should pin just above so it covers every
/// actuating window without rising above whatever sits above them.
#[cfg(target_os = "windows")]
unsafe fn topmost_of(ids: &[u64]) -> Option<u64> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{GetTopWindow, GetWindow, GW_HWNDNEXT};
    if ids.is_empty() {
        return None;
    }
    let mut h = GetTopWindow(None).unwrap_or(HWND(std::ptr::null_mut()));
    while !h.0.is_null() {
        if ids.contains(&(h.0 as u64)) {
            return Some(h.0 as u64);
        }
        h = GetWindow(h, GW_HWNDNEXT).unwrap_or(HWND(std::ptr::null_mut()));
    }
    ids.first().copied()
}

impl ZOrderEnforcer for WinZOrderEnforcer {
    fn reassert(&self, target: Option<u64>) {
        #[cfg(target_os = "windows")]
        unsafe {
            use windows::Win32::Foundation::HWND;
            use windows::Win32::UI::WindowsAndMessaging::*;

            let hwnd = HWND(self.hwnd_isize as *mut _);

            let pinned_target = target.and_then(|wid| {
                let h = HWND(wid as *mut _);
                if IsWindow(h).as_bool() {
                    Some(h)
                } else {
                    None
                }
            });

            // The overlay must sit JUST above the pinned target window so the
            // user's foreground app (a different non-topmost window — say their
            // terminal) renders on top of the overlay. Three Win32 pitfalls:
            //
            //   1. HWND_TOPMOST was the previous fallback. Once Windows promotes
            //      a window into the topmost band (sets WS_EX_TOPMOST), a later
            //      SetWindowPos with a normal target_hwnd does NOT drop it back
            //      out — the overlay stays above EVERYTHING non-topmost (incl.
            //      the user's foreground). That was the symptom in #1688-style
            //      reports.
            //   2. To drop out of the topmost band, we need an explicit
            //      SetWindowPos(hwnd, HWND_NOTOPMOST, …) call before the real
            //      z-order placement.
            //   3. `SetWindowPos(hwnd, target_hwnd, …)` does NOT mean "put hwnd
            //      above target_hwnd" — Win32 semantics are "insert hwnd
            //      *after* target_hwnd in z-order" (i.e. one slot BELOW
            //      target). To land overlay *above* target we have to insert
            //      it after target's previous sibling instead — the window
            //      currently just above target. `GetWindow(target, GW_HWNDPREV)`
            //      returns that (or null when target is already the topmost
            //      non-topmost window, in which case HWND_TOP raises overlay
            //      to the top and pushes target one slot down). This is the
            //      pitfall macOS / Linux dodge by virtue of their explicit
            //      `orderWindow:above:` / `StackMode::ABOVE` APIs.
            //
            match pinned_target {
                Some(target) => {
                    // Preserve the target-bound contract: normalize out of the
                    // topmost band, then insert exactly one slot above target.
                    let _ = SetWindowPos(
                        hwnd,
                        HWND_NOTOPMOST,
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
                    );
                    let prev = GetWindow(target, GW_HWNDPREV).unwrap_or(HWND(std::ptr::null_mut()));
                    let insert_after = if !prev.0.is_null() { prev } else { HWND_TOP };
                    let _ = SetWindowPos(
                        hwnd,
                        insert_after,
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE
                            | SWP_NOSIZE
                            | SWP_NOACTIVATE
                            | SWP_SHOWWINDOW
                            | SWP_NOOWNERZORDER,
                    );
                }
                None => {
                    // HWND_TOP is subject to the foreground lock and can leave
                    // the overlay in a stale ordinary-band slot. A temporary
                    // topmost transition is lock-free; immediately demoting it
                    // lands at the reliable front of the ordinary band. Both
                    // calls are non-activating, and the final state is never
                    // persistent topmost.
                    let flags = SWP_NOMOVE
                        | SWP_NOSIZE
                        | SWP_NOACTIVATE
                        | SWP_SHOWWINDOW
                        | SWP_NOOWNERZORDER;
                    let _ = SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, flags);
                    let _ = SetWindowPos(hwnd, HWND_NOTOPMOST, 0, 0, 0, 0, flags);
                }
            }

            let _ = target;
        }
    }
}

// ── Headless unit tests for the Windows adapter ───────────────────────────
//
// The keyed lifecycle (ownership, removal, default guard, tombstone, revival,
// stable z-order), the sentinel seed, and the shared frame-tick predicate are
// covered once in `cursor_overlay::render_map`. These tests cover what this
// adapter adds: the dirty-rect envelope, virtual-screen seeding, the IDLE
// cadence countdown, and arrival release on removal. On-screen rendering
// (UpdateLayeredWindow) needs a real display and is verified separately.

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_map() -> RenderMap {
        RenderMap::new(
            CursorConfig::default(),
            WinScreen {
                virt_x: 0,
                virt_y: 0,
                virt_w: 100,
                virt_h: 100,
                last_tick: Instant::now(),
            },
        )
    }

    fn move_msg(key: &str, x: f64, y: f64) -> OverlayMsg {
        OverlayMsg::Cmd(KeyedOverlayCommand {
            key: key.to_owned(),
            cmd: OverlayCommand::MoveTo {
                x,
                y,
                end_heading_radians: 0.0,
            },
        })
    }

    fn settle(map: &mut RenderMap) {
        for _ in 0..2000 {
            map.tick_all(0.016);
            if map.cursors.values().all(|rs| {
                rs.core.path.is_none() && rs.core.spring.is_none() && rs.core.click_t.is_none()
            }) {
                break;
            }
        }
    }

    #[test]
    fn default_dirty_envelope_covers_edge_clamped_session_badges() {
        assert!(
            CURSOR_PAD as f32 >= cursor_overlay::session_badge::BADGE_MAX_WIDTH + 8.0,
            "a cursor at a display edge can have the entire badge on one side"
        );
    }

    #[test]
    fn seed_clamps_into_a_virtual_screen_left_of_the_primary() {
        let mut map = empty_map();
        map.platform.virt_x = -1920;
        map.platform.virt_w = 3840;
        map.platform.virt_h = 1080;
        let frame = map.platform.frame();
        assert!(map.seed_start_if_sentinel("sessA", -1800.0, 500.0, frame));
        assert_eq!(map.cursors["sessA"].core.pos, (-1918.0, 360.0));

        map.platform.virt_w = 0;
        assert_eq!(map.platform.frame(), None);
    }

    #[test]
    fn removal_through_the_adapter_resolves_no_pin_key() {
        let mut map = empty_map();
        assert_eq!(
            apply_msg(&mut map, move_msg("sessA", 10.0, 10.0)).as_deref(),
            Some("sessA")
        );
        assert_eq!(
            apply_msg(&mut map, OverlayMsg::Remove("sessA".to_owned())),
            None
        );
        assert_eq!(map.last_active, None);
    }

    // Before the shared predicate, a landed Windows cursor dropped to the IDLE
    // cadence for the whole idle-hide delay and its resting bob froze.
    #[test]
    fn resting_cursor_keeps_the_active_cadence_for_its_bob() {
        let mut map = empty_map();
        map.seed_start_if_sentinel("sessA", 60.0, 60.0, map.platform.frame());
        apply_msg(&mut map, move_msg("sessA", 10.0, 10.0));
        settle(&mut map);
        let rs = &map.cursors["sessA"];
        assert!(rs.core.path.is_none() && rs.core.spring.is_none());
        assert_eq!(rs.core.idle_alpha, 1.0);
        assert!(rs.core.has_resting_motion());
        assert!(map.needs_frame_tick(), "the resting bob needs frames");
    }

    #[test]
    fn reduced_motion_countdown_parks_at_idle_cadence_but_fade_ticks() {
        let mut map = empty_map();
        map.seed_start_if_sentinel("sessA", 60.0, 60.0, map.platform.frame());
        map.cursors
            .get_mut("sessA")
            .unwrap()
            .core
            .visual
            .reduced_motion = cursor_overlay::ReducedMotion::On;
        apply_msg(&mut map, move_msg("sessA", 10.0, 10.0));
        for _ in 0..2000 {
            map.tick_all(0.016);
            if !map.needs_frame_tick() {
                break;
            }
        }

        // Landed cursor at constant alpha 1.0, idle-hide pending: pixels
        // cannot change, so it must NOT demand frame ticks (the render-gate
        // fix), but it must still be ticked for wall-clock accrual.
        let rs = &map.cursors["sessA"];
        assert!(rs.core.path.is_none() && rs.core.spring.is_none());
        assert_eq!(rs.core.idle_alpha, 1.0);
        assert!(!map.needs_frame_tick());
        assert!(rs.in_idle_countdown());

        // The fade animates pixels, so frame ticks return until alpha ~0.
        let rs = map.cursors.get_mut("sessA").unwrap();
        rs.core.idle_secs = rs.core.motion.idle_hide_ms / 1000.0;
        rs.tick(0.016);
        assert!(rs.core.idle_alpha < 1.0 && rs.core.idle_alpha >= 0.004);
        assert!(rs.needs_frame_tick());
        assert!(!rs.in_idle_countdown());

        for _ in 0..60 {
            map.tick_all(0.016);
        }
        assert!(
            !map.needs_frame_tick(),
            "fully faded cursor must be quiescent"
        );
    }
}

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
//! shared cursor. It now keeps a keyed [`RenderMap`] (`IndexMap<CursorKey,
//! RenderState>`): each declared `session` owns its own cursor with its own
//! palette, and the ~125 Hz tick composites them all into the single layered
//! window. `IndexMap` gives deterministic insertion-ordered iteration = stable
//! per-session z-order frame to frame. The lifecycle (lazy create, per-key
//! arrival isolation, `session_end` removal, resurrection tombstone) mirrors
//! `platform_macos::cursor::overlay` so the two platforms behave identically;
//! the shared `cursor_overlay::{CursorKey, KeyedOverlayCommand, OverlayMsg}`
//! types are the same ones macOS uses.
//!
//! ## Cross-platform note (2026-05 dedup audit)
//!
//! Animation state + render pipeline live in `cursor_overlay::render_state`
//! (`RenderStateCore`, `tick_motion`, `apply_command_base`, `paint_cursor`).
//! What stays here is purely the Win32 window plumbing: message loop,
//! UpdateLayeredWindow paint, virtual-screen offset, z-order maintenance.

#![allow(non_snake_case, non_upper_case_globals)]

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use cursor_overlay::{
    CursorConfig, CursorKey, KeyedOverlayCommand, MotionConfig, OverlayCommand, OverlayMsg,
    Palette, RenderStateCore, ZOrderEnforcer,
};
use indexmap::IndexMap;

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

// ── Keyed render collection ───────────────────────────────────────────────

/// The keyed, insertion-ordered collection of owned cursors that the render
/// loop composites every tick. Insertion order = stable z-order (later keys
/// paint on top). Virtual-screen geometry + the `WM_TIMER` dt stamp are
/// hoisted here (screen-global, written once in `run_overlay_thread`).
struct RenderMap {
    cursors: IndexMap<CursorKey, RenderState>,
    /// Virtual screen dimensions set after window creation (Win32 DIPs).
    /// `virt_x/y` are subtracted from each cursor's `core.pos` when rendering
    /// so the pixmap is laid out in window-local coordinates.
    virt_x: i32,
    virt_y: i32,
    virt_w: i32,
    virt_h: i32,
    /// Last WM_TIMER wall-clock stamp; used to compute real `dt` (Windows
    /// timer resolution defaults to 15ms so a hardcoded 8ms would run the
    /// animation at half speed).
    last_tick: Instant,
    /// Frozen launch-time config used as the template for lazily-created
    /// cursors (its palette is overridden per-key via `Palette::for_instance`).
    template: CursorConfig,
    /// Render-side tombstone of permanently-ended session cursor keys. A `Cmd`
    /// for a key in here is dropped WITHOUT get-or-create, so an in-flight
    /// click/move from another task that lands AFTER the owning session's
    /// `Remove` can never resurrect the just-removed cursor. "default" is never
    /// tombstoned (it backs the anonymous / one-shot path).
    ended: HashSet<CursorKey>,
    /// Cursor key whose target the overlay should currently sit above. A single
    /// layered window can occupy only one z-band, so the most-recently-touched
    /// cursor wins (mirrors macOS). `None` until the first PinAbove/Cmd.
    last_active: Option<CursorKey>,
}

/// Build the `RenderState` for a lazily-created cursor key: derive from the
/// launch template but give each non-default key its own palette so distinct
/// sessions get distinct colours automatically.
fn render_state_for_key(template: &CursorConfig, key: &str) -> RenderState {
    let mut rs = RenderState::new(template.clone());
    rs.core.palette = Palette::for_instance(key);
    rs
}

/// Apply one inbound [`OverlayMsg`] to the render map (drain step). Factored
/// out as a pure function so the per-session ownership + removal lifecycle is
/// unit-testable without any Win32 window.
///
/// Returns the resolved cursor key for a `Cmd` (so the caller can track the
/// last-active key for z-order pinning); `None` for a `Remove`.
fn apply_msg(map: &mut RenderMap, msg: OverlayMsg) -> Option<CursorKey> {
    match msg {
        OverlayMsg::Remove(key) => {
            // The "default" cursor backs the anonymous / one-shot path and must
            // survive every session_end + the daemon lifetime.
            if key != "default" {
                map.cursors.shift_remove(&key);
                if let Ok(mut guard) = ARRIVAL_TX.lock() {
                    if let Some(m) = guard.as_mut() {
                        m.remove(&key);
                    }
                }
                if map.last_active.as_deref() == Some(key.as_str()) {
                    map.last_active = None;
                }
                // Tombstone the key so a late in-flight Cmd from another task
                // cannot re-create the just-removed cursor.
                map.ended.insert(key);
            }
            None
        }
        OverlayMsg::Cmd(KeyedOverlayCommand { key, cmd }) => {
            // Drop a command for an already-ended session WITHOUT get-or-create
            // — this is the resurrection guard. Without it, a ClickPulse/MoveTo
            // landing after Remove would re-insert (and re-leak) the cursor.
            if map.ended.contains(&key) {
                return None;
            }
            let template = map.template.clone();
            let k = key.clone();
            let rs = map
                .cursors
                .entry(key)
                .or_insert_with(|| render_state_for_key(&template, &k));
            rs.apply_command(cmd);
            Some(k)
        }
    }
}

pub fn init(cfg: CursorConfig) {
    let (tx, rx) = std::sync::mpsc::sync_channel(4096);
    let _ = CMD_TX.set(tx);
    *CMD_RX_CELL.lock().unwrap() = Some(rx);
    *ARRIVAL_TX.lock().unwrap() = Some(HashMap::new());
    let mut cursors = IndexMap::new();
    cursors.insert("default".to_owned(), RenderState::new(cfg.clone()));
    *RENDER.lock().unwrap() = Some(RenderMap {
        cursors,
        virt_x: 0,
        virt_y: 0,
        virt_w: 1920,
        virt_h: 1080,
        last_tick: Instant::now(),
        template: cfg,
        ended: HashSet::new(),
        last_active: None,
    });
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

/// Returns true if the cursor for `key` is currently enabled/visible. A session
/// with no own cursor yet falls back to the seeded `"default"` cursor.
pub fn is_enabled(key: &str) -> bool {
    RENDER
        .lock()
        .ok()
        .and_then(|g| {
            g.as_ref().and_then(|m| {
                m.cursors
                    .get(key)
                    .or_else(|| m.cursors.get("default"))
                    .map(|rs| rs.core.visible)
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
            g.as_ref().and_then(|m| {
                m.cursors
                    .get(key)
                    .or_else(|| m.cursors.get("default"))
                    .map(|rs| rs.core.motion.clone())
            })
        })
        .unwrap_or_default()
}

/// Current screen position of the cursor for `key` (the off-screen sentinel
/// `(-200, -200)` if it has never been placed). A session with no own cursor
/// yet reports the sentinel so the click path treats it as first-placement.
pub fn current_position(key: &str) -> (f64, f64) {
    RENDER
        .lock()
        .ok()
        .and_then(|g| g.as_ref().and_then(|m| m.cursors.get(key)).map(|rs| rs.core.pos))
        .unwrap_or((-200.0, -200.0))
}

/// Seed a brand-new (sentinel-positioned) cursor at an on-screen start point
/// offset up-left of `(target_x, target_y)` so the immediately-following
/// `MoveTo` glides INTO the target instead of silently snapping. No-op when the
/// cursor is already on-screen or its session already ended. Returns true if a
/// seed was applied. Mirrors `platform_macos::cursor::overlay::seed_start_*`.
fn seed_start_if_sentinel(key: &CursorKey, target_x: f64, target_y: f64) -> bool {
    let mut guard = RENDER.lock().unwrap();
    let Some(map) = guard.as_mut() else { return false };
    seed_start_in_map(map, key, target_x, target_y)
}

/// Pure seed step operating on a borrowed [`RenderMap`] — factored out so the
/// get-or-create + clamp logic is unit-testable without the global `RENDER`
/// static or a Win32 window.
fn seed_start_in_map(map: &mut RenderMap, key: &CursorKey, target_x: f64, target_y: f64) -> bool {
    const SEED_OFFSET: f64 = 140.0;
    let (virt_x, virt_y) = (map.virt_x as f64, map.virt_y as f64);
    let (virt_w, virt_h) = (map.virt_w as f64, map.virt_h as f64);
    // Respect the resurrection guard: never seed (and thus re-create) a cursor
    // whose session already ended.
    if map.ended.contains(key) {
        return false;
    }
    let template = map.template.clone();
    let k = key.clone();
    let rs = map
        .cursors
        .entry(key.clone())
        .or_insert_with(|| render_state_for_key(&template, &k));
    if !(rs.core.cfg.enabled && rs.core.pos.0 < -50.0) {
        return false;
    }
    let mut sx = target_x - SEED_OFFSET;
    let mut sy = target_y - SEED_OFFSET;
    // Clamp into the virtual-screen frame so the seed never starts off-display.
    if virt_w > 0.0 && virt_h > 0.0 {
        sx = sx.clamp(virt_x + 2.0, virt_x + virt_w - 2.0);
        sy = sy.clamp(virt_y + 2.0, virt_y + virt_h - 2.0);
        // If clamping collapsed the seed onto the target (target in a corner),
        // nudge the other way so there is still a visible glide distance.
        if (sx - target_x).abs() < 8.0 && (sy - target_y).abs() < 8.0 {
            sx = (target_x + SEED_OFFSET).min(virt_x + virt_w - 2.0);
            sy = (target_y + SEED_OFFSET).min(virt_y + virt_h - 2.0);
        }
    }
    rs.core.pos = (sx, sy);
    true
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
// wrapper; the virtual-screen geometry + dt stamp moved up to `RenderMap`.

struct RenderState {
    core: RenderStateCore,
}

impl RenderState {
    fn new(cfg: CursorConfig) -> Self {
        RenderState {
            core: RenderStateCore::new(cfg),
        }
    }

    /// Advance the motion state by `dt`. Returns `true` the tick the planned
    /// path completes, so the WM_TIMER handler can fire the arrival oneshot
    /// that unblocks `animate_cursor_to`.
    fn tick(&mut self, dt: f64) -> bool {
        self.core.tick_motion(dt)
    }

    fn apply_command(&mut self, cmd: OverlayCommand) {
        // Windows uses the non-sentinel-snap behaviour for both MoveTo and
        // ClickPulse: every command updates `self.pos` unconditionally.
        // `ShowFocusRect` is not rendered on Windows — `apply_command_base`
        // returns `false` for it and we silently drop it here.
        let _ = self.core.apply_command_base(cmd, false, false);
    }

    /// True while the render loop must keep ticking at frame cadence because
    /// the next tick can still change pixels: an in-flight glide path, a
    /// spring-settle, a click pulse, or an idle-fade that has not yet fully
    /// faded the cursor out. A brand-new sentinel cursor (off-screen at
    /// `(-200, -200)`) and a cursor that has already faded to `idle_alpha ≈ 0`
    /// are both quiescent, so `mcp`/`serve` with no agent activity can let the
    /// timer go cheap instead of compositing a full virtual-screen pixmap at
    /// ~125 Hz. Mirrors `platform_macos::cursor::overlay`'s `needs_frame_tick`.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    fn needs_frame_tick(&self) -> bool {
        self.core.path.is_some()
            || self.core.spring.is_some()
            || self.core.click_t.is_some()
            || (self.core.motion.idle_hide_ms > 0.0
                && self.core.visible
                && self.core.pos.0 >= -100.0
                && self.core.idle_alpha >= 0.004)
    }
}

/// True if ANY owned cursor still needs frame ticks (animation / fade in
/// progress). When this is false the render loop is fully quiescent: the last
/// emitted frame already left the layered window in its resting / cleared
/// state, so the timer can drop to a slow idle cadence and skip the expensive
/// composite + `UpdateLayeredWindow` until the next command wakes it.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn render_map_needs_frame_tick(map: &RenderMap) -> bool {
    map.cursors.values().any(RenderState::needs_frame_tick)
}

// ── Win32 message-loop thread ─────────────────────────────────────────────

#[cfg(target_os = "windows")]
fn run_overlay_thread(cfg: CursorConfig, rx: std::sync::mpsc::Receiver<OverlayMsg>) {
    use windows::Win32::Media::timeBeginPeriod;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::*;
    use windows::core::PCWSTR;

    // Raise multimedia timer resolution to 1ms so SetTimer can deliver
    // WM_TIMER messages at ~8ms intervals (default is ~15ms).
    unsafe {
        let _ = timeBeginPeriod(1);
    }

    // Collect virtual screen bounds (all monitors).
    let virt_x = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
    let virt_y = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
    let virt_w = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
    let virt_h = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };

    // Update render map with virtual screen bounds.
    {
        let mut guard = RENDER.lock().unwrap();
        if let Some(map) = guard.as_mut() {
            map.virt_x = virt_x;
            map.virt_y = virt_y;
            map.virt_w = virt_w;
            map.virt_h = virt_h;
            map.last_tick = Instant::now();
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

// ── Idle render gate (issue #1808) ────────────────────────────────────────
//
// The overlay window timer is re-armed between two cadences:
//   * ACTIVE  (`TIMER_MS_ACTIVE`, ~125 Hz) while any cursor is animating /
//     fading — this is what produces a smooth glide + click pulse.
//   * IDLE    (`TIMER_MS_IDLE`, a slow heartbeat) when every cursor is
//     quiescent — the handler then only drains the command channel cheaply
//     and re-arms ACTIVE the instant a command arrives. No full-screen pixmap
//     allocation, no RGBA→BGRA copy, no UpdateLayeredWindow while idle.
//
// Before this gate the timer ran at ~125 Hz unconditionally and every tick
// allocated a virtual-screen pixmap, swizzled it pixel-by-pixel, and blitted
// it with UpdateLayeredWindow — burning 60–85% of a core with the cursor
// static (issue #1808). `TIMER_PERIOD_MS` is the cadence the timer is currently
// armed at; the WM_TIMER handler flips it based on `render_map_needs_frame_tick`.
const TIMER_ID: usize = 1;
const TIMER_MS_ACTIVE: u32 = 8; // ~125 Hz, matches the C# reference render rate
const TIMER_MS_IDLE: u32 = 250; // slow heartbeat: drain channel, stay responsive
/// Current armed timer cadence in ms. Compared against the desired cadence each
/// WM_TIMER so we only call `SetTimer` (re-arm) on an actual active↔idle flip.
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
                if let Some(path) = FPS_PATH.get_or_init(|| std::env::var("CUA_DRIVER_RS_OVERLAY_FPS_FILE").ok()) {
                    let n = FPS_FRAMES.fetch_add(1, Relaxed) + 1;
                    let last = FPS_LAST.load(Relaxed);
                    if last == 0 {
                        FPS_LAST.store(now_ms, Relaxed);
                    } else if now_ms.wrapping_sub(last) >= 1000 {
                        let secs = (now_ms - last) as f64 / 1000.0;
                        let fps = n as f64 / secs.max(1e-3);
                        let cursors = RENDER.lock().ok()
                            .and_then(|g| g.as_ref().map(|m| m.cursors.len())).unwrap_or(0);
                        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                            use std::io::Write;
                            let _ = writeln!(f, "overlay fps={fps:.1} avg_dt_ms={:.1} cursors={cursors}", secs * 1000.0 / n as f64);
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
            let (pixmap, arrived, pinned_wid, needs_tick) = {
                let mut guard = RENDER.lock().unwrap();
                if let Some(map) = guard.as_mut() {
                    // Drain the channel via get-or-create; track the last-touched
                    // key so the z-order pin follows the most-recent cursor.
                    let mut had_msg = false;
                    if let Ok(rx_guard) = CMD_RX_WIN.try_lock() {
                        if let Some(ref rx) = *rx_guard {
                            while let Ok(m) = rx.try_recv() {
                                had_msg = true;
                                if let Some(k) = apply_msg(map, m) {
                                    map.last_active = Some(k);
                                }
                            }
                        }
                    }

                    let now = Instant::now();
                    let dt = now
                        .duration_since(map.last_tick)
                        .as_secs_f64()
                        .clamp(0.0, 0.05);
                    map.last_tick = now;

                    // Tick every cursor; record the ones that just arrived.
                    let mut arrived: Vec<CursorKey> = Vec::new();
                    for (k, rs) in map.cursors.iter_mut() {
                        if rs.tick(dt) {
                            arrived.push(k.clone());
                        }
                    }

                    // After ticking: does any cursor still need frame ticks?
                    let needs_tick = render_map_needs_frame_tick(map);

                    // Render only when something can have changed pixels this
                    // frame: a fresh command, a still-running animation, or the
                    // final settle frame as the previous animation winds down
                    // (`was_active && !needs_tick`). A fully-quiescent idle tick
                    // returns `None` here and does no compositing at all.
                    let should_render = had_msg || needs_tick || was_active;

                    if !should_render {
                        (None, arrived, None, needs_tick)
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

                        // Composite every cursor into ONE virtual-screen pixmap.
                        // While ACTIVE this runs every ~8ms, so a steady
                        // per-frame blit keeps resting cursors flicker-free as
                        // others animate; once the loop goes idle the whole block
                        // is skipped (the `should_render` gate above).
                        let w = map.virt_w.max(1) as u32;
                        let h = map.virt_h.max(1) as u32;
                        let mut pm = tiny_skia::Pixmap::new(w.max(1), h.max(1))
                            .unwrap_or_else(|| tiny_skia::Pixmap::new(1, 1).unwrap());
                        for (_k, rs) in &map.cursors {
                            cursor_overlay::paint_cursor(
                                &mut pm,
                                &rs.core,
                                map.virt_x as f64,
                                map.virt_y as f64,
                                None, // focus-rect is macOS-only
                            );
                        }

                        (Some(pm), arrived, pinned, needs_tick)
                    }
                } else {
                    (None, Vec::new(), None, false)
                }
            };

            if let Some(pm) = pixmap {
                update_layered_window(hwnd, &pm);

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
            } else {
                TIMER_MS_IDLE
            };
            if TIMER_PERIOD_MS.swap(desired_ms, std::sync::atomic::Ordering::Relaxed) != desired_ms {
                unsafe {
                    SetTimer(hwnd, TIMER_ID, desired_ms, None);
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

// ── UpdateLayeredWindow helper ────────────────────────────────────────────

#[cfg(target_os = "windows")]
unsafe fn update_layered_window(
    hwnd: windows::Win32::Foundation::HWND,
    pixmap: &tiny_skia::Pixmap,
) {
    use windows::Win32::Foundation::*;
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::UI::WindowsAndMessaging::{UpdateLayeredWindow, ULW_ALPHA};

    let w = pixmap.width() as i32;
    let h = pixmap.height() as i32;
    if w <= 0 || h <= 0 {
        return;
    }

    let hdc_screen = GetDC(None);
    let hdc_mem = CreateCompatibleDC(hdc_screen);

    // Create a 32-bit top-down DIB section (BGRA).
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
    if hbmp.is_err() || bits_ptr.is_null() {
        let _ = DeleteDC(hdc_mem);
        ReleaseDC(None, hdc_screen);
        return;
    }
    let hbmp = hbmp.unwrap();
    SelectObject(hdc_mem, hbmp);

    // Copy pixels: tiny-skia produces premultiplied RGBA; Win32 expects premultiplied BGRA.
    let src = pixmap.data();
    let dst = std::slice::from_raw_parts_mut(bits_ptr as *mut u8, (w * h * 4) as usize);
    for i in 0..(w * h) as usize {
        let r = src[i * 4];
        let g = src[i * 4 + 1];
        let b = src[i * 4 + 2];
        let a = src[i * 4 + 3];
        // Swap R <-> B for BGRA.
        dst[i * 4] = b;
        dst[i * 4 + 1] = g;
        dst[i * 4 + 2] = r;
        dst[i * 4 + 3] = a;
    }

    // UpdateLayeredWindow.
    let virt_x;
    let virt_y;
    {
        let guard = RENDER.lock().unwrap();
        if let Some(map) = &*guard {
            virt_x = map.virt_x;
            virt_y = map.virt_y;
        } else {
            virt_x = 0;
            virt_y = 0;
        }
    }

    let pt_src = POINT { x: 0, y: 0 };
    let pt_dst = POINT { x: virt_x, y: virt_y };
    let sz = SIZE { cx: w, cy: h };
    let blend = BLENDFUNCTION {
        BlendOp: 0, // AC_SRC_OVER
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: 1, // AC_SRC_ALPHA
    };
    let _ = UpdateLayeredWindow(
        hwnd,
        hdc_screen,
        Some(&pt_dst),
        Some(&sz),
        hdc_mem,
        Some(&pt_src),
        COLORREF(0),
        Some(&blend),
        ULW_ALPHA,
    );

    let _ = DeleteObject(hbmp);
    let _ = DeleteDC(hdc_mem);
    ReleaseDC(None, hdc_screen);
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
    if ids.is_empty() { return None; }
    let mut h = GetTopWindow(None).unwrap_or(HWND(std::ptr::null_mut()));
    while !h.0.is_null() {
        if ids.contains(&(h.0 as u64)) { return Some(h.0 as u64); }
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
                if IsWindow(h).as_bool() { Some(h) } else { None }
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
            // Fallback when there's no live pin: HWND_TOP — top of non-topmost
            // band, NOT the topmost band. So a "no pin" overlay still respects
            // the user's foreground stack.
            let _ = SetWindowPos(
                hwnd,
                HWND_NOTOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
            );

            let insert_after = match pinned_target {
                Some(target) => {
                    let prev = GetWindow(target, GW_HWNDPREV).unwrap_or(HWND(std::ptr::null_mut()));
                    if !prev.0.is_null() { prev } else { HWND_TOP }
                }
                None => HWND_TOP,
            };
            let _ = SetWindowPos(
                hwnd,
                insert_after,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_NOOWNERZORDER,
            );

            let _ = target;
        }
    }
}

// ── Headless unit tests for the keyed render collection ───────────────────
//
// These prove the per-session ownership data model, the session_end removal
// lifecycle, the "default" guard, the resurrection tombstone, and the
// sentinel seed WITHOUT any Win32 window. The on-screen rendering
// (UpdateLayeredWindow) still needs a real display and is verified separately.

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_map() -> RenderMap {
        let mut cursors = IndexMap::new();
        cursors.insert("default".to_owned(), RenderState::new(CursorConfig::default()));
        RenderMap {
            cursors,
            virt_x: 0,
            virt_y: 0,
            virt_w: 100,
            virt_h: 100,
            last_tick: Instant::now(),
            template: CursorConfig::default(),
            ended: HashSet::new(),
            last_active: None,
        }
    }

    fn move_msg(key: &str, x: f64, y: f64) -> OverlayMsg {
        OverlayMsg::Cmd(KeyedOverlayCommand {
            key: key.to_owned(),
            cmd: OverlayCommand::MoveTo { x, y, end_heading_radians: 0.0 },
        })
    }

    #[test]
    fn two_sessions_produce_two_distinct_render_entries() {
        let mut map = empty_map();
        apply_msg(&mut map, move_msg("sessA", 10.0, 10.0));
        apply_msg(&mut map, move_msg("sessB", 42.0, 24.0));
        // default + sessA + sessB = 3 distinct owned cursors. The pre-port
        // regression: a single RenderState would clobber these to one cursor.
        assert_eq!(map.cursors.len(), 3);
        assert!(map.cursors.contains_key("sessA"));
        assert!(map.cursors.contains_key("sessB"));
        assert!(map.cursors.contains_key("default"));
    }

    #[test]
    fn session_end_removes_only_that_session() {
        let mut map = empty_map();
        apply_msg(&mut map, move_msg("sessA", 10.0, 10.0));
        apply_msg(&mut map, move_msg("sessB", 20.0, 20.0));
        assert_eq!(map.cursors.len(), 3);

        // session_end(A): A gone, B + default retained.
        apply_msg(&mut map, OverlayMsg::Remove("sessA".to_owned()));
        assert!(!map.cursors.contains_key("sessA"));
        assert!(map.cursors.contains_key("sessB"));
        assert!(map.cursors.contains_key("default"));
        assert_eq!(map.cursors.len(), 2);

        // Remove("default") is guarded — default survives.
        apply_msg(&mut map, OverlayMsg::Remove("default".to_owned()));
        assert!(map.cursors.contains_key("default"));

        // Remove of an absent key is a harmless no-op.
        let before = map.cursors.len();
        apply_msg(&mut map, OverlayMsg::Remove("never-existed".to_owned()));
        assert_eq!(map.cursors.len(), before);
    }

    #[test]
    fn lazily_created_cursors_get_distinct_palettes() {
        let mut map = empty_map();
        apply_msg(&mut map, move_msg("sessA", 10.0, 10.0));
        apply_msg(&mut map, move_msg("sessB", 20.0, 20.0));
        let a = &map.cursors["sessA"].core.palette;
        let b = &map.cursors["sessB"].core.palette;
        let def = &map.cursors["default"].core.palette;
        assert_ne!(a.name, def.name);
        assert_ne!(b.name, def.name);
    }

    #[test]
    fn insertion_order_is_stable_z_order() {
        let mut map = empty_map();
        apply_msg(&mut map, move_msg("first", 1.0, 1.0));
        apply_msg(&mut map, move_msg("second", 2.0, 2.0));
        // Re-touching "first" must NOT move it to the back (IndexMap keeps the
        // original slot), so z-order is stable frame to frame.
        apply_msg(&mut map, move_msg("first", 3.0, 3.0));
        let keys: Vec<&String> = map.cursors.keys().collect();
        assert_eq!(keys, vec!["default", "first", "second"]);
    }

    #[test]
    fn tombstone_blocks_resurrection_after_remove() {
        let mut map = empty_map();
        apply_msg(&mut map, move_msg("sessA", 10.0, 10.0));
        assert_eq!(map.cursors.len(), 2); // default + sessA

        apply_msg(&mut map, OverlayMsg::Remove("sessA".to_owned()));
        assert!(!map.cursors.contains_key("sessA"));
        assert_eq!(map.cursors.len(), 1);

        // A late in-flight Cmd for the ended session must be dropped WITHOUT
        // re-inserting (no get-or-create resurrection).
        let resolved = apply_msg(&mut map, move_msg("sessA", 99.0, 99.0));
        assert!(resolved.is_none(), "ended-session Cmd must be dropped, not resolved");
        assert!(!map.cursors.contains_key("sessA"), "tombstone must block resurrection");
        assert_eq!(map.cursors.len(), 1);
    }

    #[test]
    fn default_is_never_tombstoned() {
        let mut map = empty_map();
        apply_msg(&mut map, OverlayMsg::Remove("default".to_owned()));
        assert!(map.cursors.contains_key("default"));
        assert!(!map.ended.contains("default"));

        let resolved = apply_msg(&mut map, move_msg("default", 5.0, 5.0));
        assert_eq!(resolved.as_deref(), Some("default"));
        assert!(map.cursors.contains_key("default"));
    }

    #[test]
    fn seed_moves_sentinel_cursor_on_screen_for_first_action() {
        let mut map = empty_map(); // 100x100 frame at origin
        // No "sessA" cursor exists yet — the seed must get-or-create it.
        let seeded = seed_start_in_map(&mut map, &"sessA".to_owned(), 60.0, 60.0);
        assert!(seeded, "sentinel cursor must be seeded");
        let pos = map.cursors["sessA"].core.pos;
        assert!(pos.0 > -50.0 && pos.1 > -50.0, "seed must be on-screen, got {pos:?}");
        assert!(
            (pos.0 - 60.0).abs() > 4.0 || (pos.1 - 60.0).abs() > 4.0,
            "seed must differ from target to produce a visible glide, got {pos:?}"
        );
    }

    #[test]
    fn seed_is_noop_when_cursor_already_on_screen() {
        let mut map = empty_map();
        seed_start_in_map(&mut map, &"sessA".to_owned(), 60.0, 60.0);
        map.cursors.get_mut("sessA").unwrap().core.pos = (30.0, 30.0);
        let seeded_again = seed_start_in_map(&mut map, &"sessA".to_owned(), 80.0, 80.0);
        assert!(!seeded_again, "on-screen cursor must not be re-seeded");
        assert_eq!(map.cursors["sessA"].core.pos, (30.0, 30.0), "pos must be untouched");
    }

    #[test]
    fn seed_does_not_resurrect_ended_session() {
        let mut map = empty_map();
        map.ended.insert("sessA".to_owned());
        let seeded = seed_start_in_map(&mut map, &"sessA".to_owned(), 60.0, 60.0);
        assert!(!seeded, "ended session must not be seeded");
        assert!(!map.cursors.contains_key("sessA"), "ended session must not be resurrected");
    }

    #[test]
    fn sentinel_cursor_is_quiescent_no_frame_tick() {
        // A brand-new `mcp`/`serve` with no agent activity holds only the
        // "default" cursor at the off-screen sentinel (-200, -200). It must NOT
        // request frame ticks, so the render timer can drop to the slow idle
        // cadence instead of compositing a full-screen pixmap at ~125 Hz
        // (issue #1808 idle-CPU burn).
        let map = empty_map();
        assert!(
            !render_map_needs_frame_tick(&map),
            "an untouched sentinel-only overlay must be quiescent"
        );
    }

    #[test]
    fn animating_cursor_requests_frame_ticks() {
        // After a MoveTo, the cursor has an in-flight path → the loop must keep
        // ticking at frame cadence so the glide actually animates.
        let mut map = empty_map();
        apply_msg(&mut map, move_msg("sessA", 60.0, 60.0));
        assert!(
            render_map_needs_frame_tick(&map),
            "a cursor with an in-flight glide must request frame ticks"
        );
        assert!(
            map.cursors["sessA"].needs_frame_tick(),
            "the animating cursor itself must report needs_frame_tick"
        );
    }

    #[test]
    fn click_pulse_requests_frame_ticks_then_goes_quiescent() {
        let mut map = empty_map();
        // ClickPulse seeds click_t = Some(0.0) → active.
        apply_msg(
            &mut map,
            OverlayMsg::Cmd(KeyedOverlayCommand {
                key: "sessA".to_owned(),
                cmd: OverlayCommand::ClickPulse { x: 10.0, y: 10.0 },
            }),
        );
        assert!(
            render_map_needs_frame_tick(&map),
            "click pulse must keep ticking"
        );

        // Disable idle-hide so the only activity source is the click pulse, then
        // advance time past the pulse: the cursor must fall quiescent so the
        // loop can park.
        for rs in map.cursors.values_mut() {
            rs.core.motion.idle_hide_ms = 0.0;
        }
        for _ in 0..120 {
            for rs in map.cursors.values_mut() {
                rs.tick(0.016);
            }
        }
        assert!(
            !render_map_needs_frame_tick(&map),
            "after the click pulse finishes the overlay must go quiescent"
        );
    }

    #[test]
    fn remove_clears_last_active_for_that_key() {
        let mut map = empty_map();
        let k = apply_msg(&mut map, move_msg("sessA", 10.0, 10.0));
        map.last_active = k;
        assert_eq!(map.last_active.as_deref(), Some("sessA"));
        apply_msg(&mut map, OverlayMsg::Remove("sessA".to_owned()));
        assert_eq!(map.last_active, None, "removing the active cursor must clear last_active");
    }
}

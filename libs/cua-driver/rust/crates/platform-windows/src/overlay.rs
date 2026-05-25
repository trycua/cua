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
//! ## Cross-platform note (2026-05 dedup audit)
//!
//! Animation state + render pipeline live in `cursor_overlay::render_state`
//! (`RenderStateCore`, `tick_motion`, `apply_command_base`, `render_frame`).
//! What stays here is purely the Win32 window plumbing: message loop,
//! UpdateLayeredWindow paint, virtual-screen offset, z-order maintenance.

#![allow(non_snake_case, non_upper_case_globals)]

use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use cursor_overlay::{
    CursorConfig, MotionConfig, OverlayCommand, RenderStateCore,
};

// ── Global channel ────────────────────────────────────────────────────────

static CMD_TX: OnceLock<std::sync::mpsc::SyncSender<OverlayCommand>> = OnceLock::new();
static CMD_RX_CELL: Mutex<Option<std::sync::mpsc::Receiver<OverlayCommand>>> = Mutex::new(None);
static RENDER: Mutex<Option<RenderState>> = Mutex::new(None);

pub fn init(cfg: CursorConfig) {
    let (tx, rx) = std::sync::mpsc::sync_channel(4096);
    let _ = CMD_TX.set(tx);
    *CMD_RX_CELL.lock().unwrap() = Some(rx);
    *RENDER.lock().unwrap() = Some(RenderState::new(cfg));
}

pub fn send_command(cmd: OverlayCommand) {
    if let Some(tx) = CMD_TX.get() {
        let _ = tx.try_send(cmd);
    }
}

/// Returns the current glide duration in milliseconds (default 750).
/// Used by the click path to wait for the animation before firing ClickPulse.
pub fn glide_duration_ms() -> f64 {
    RENDER.lock().ok()
        .and_then(|g| g.as_ref().map(|rs| rs.core.motion.glide_duration_ms))
        .unwrap_or(750.0)
}

/// Returns true if the cursor overlay is currently enabled/visible.
pub fn is_enabled() -> bool {
    RENDER.lock().ok()
        .and_then(|g| g.as_ref().map(|rs| rs.core.visible))
        .unwrap_or(false)
}

/// Snapshot the current motion config (start_handle / end_handle / arc_size /
/// arc_flow / spring / glide_duration_ms / dwell_after_click_ms /
/// idle_hide_ms).  Mirrors macOS `current_motion()` so
/// `get_agent_cursor_state` can report the live values.
pub fn current_motion() -> MotionConfig {
    RENDER.lock().ok()
        .and_then(|g| g.as_ref().map(|rs| rs.core.motion.clone()))
        .unwrap_or_default()
}

/// Returns the current cursor position in screen coordinates.
pub fn current_position() -> (f64, f64) {
    RENDER.lock().ok()
        .and_then(|g| g.as_ref().map(|rs| rs.core.pos))
        .unwrap_or((-200.0, -200.0))
}

/// Returns true if the cursor is still at the off-screen initial position
/// (-200, -200), meaning it has never been positioned on screen yet.
pub fn is_at_initial_position() -> bool {
    RENDER.lock().ok()
        .and_then(|g| g.as_ref().map(|rs| rs.core.pos.0 < 0.0 && rs.core.pos.1 < 0.0))
        .unwrap_or(true)
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
            Some(rs) => rs.core.cfg.clone(),
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

// ── Animation state ───────────────────────────────────────────────────────
//
// The platform-agnostic fields + tick + apply_command + render pipeline live
// in `cursor_overlay::render_state` (2026-05 dedup audit). What stays here
// is the Windows-specific virtual-screen geometry + last_tick stamp for the
// WM_TIMER dt calculation.

struct RenderState {
    core: RenderStateCore,
    /// Virtual screen dimensions set after window creation (Win32 DIPs).
    /// `virt_x/y` are subtracted from `core.pos` when rendering so the
    /// pixmap is laid out in window-local coordinates.
    virt_x: i32,
    virt_y: i32,
    virt_w: i32,
    virt_h: i32,
    /// Last WM_TIMER wall-clock stamp; used to compute real `dt` (Windows
    /// timer resolution defaults to 15ms so a hardcoded 8ms would run the
    /// animation at half speed).
    last_tick: Instant,
}

impl RenderState {
    fn new(cfg: CursorConfig) -> Self {
        RenderState {
            core: RenderStateCore::new(cfg),
            last_tick: Instant::now(),
            virt_x: 0, virt_y: 0, virt_w: 1920, virt_h: 1080,
        }
    }

    fn tick(&mut self, dt: f64) {
        self.core.tick_motion(dt);
    }

    fn apply_command(&mut self, cmd: OverlayCommand) {
        // Windows uses the non-sentinel-snap behaviour for both MoveTo and
        // ClickPulse: every command updates `self.pos` unconditionally.
        // `ShowFocusRect` is not rendered on Windows — `apply_command_base`
        // returns `false` for it and we silently drop it here.
        let _ = self.core.apply_command_base(cmd, false, false);
    }
}

// ── Win32 message-loop thread ─────────────────────────────────────────────

#[cfg(target_os = "windows")]
fn run_overlay_thread(cfg: CursorConfig, rx: std::sync::mpsc::Receiver<OverlayCommand>) {
    use windows::Win32::UI::WindowsAndMessaging::*;
    use windows::Win32::Media::timeBeginPeriod;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::core::PCWSTR;

    // Raise multimedia timer resolution to 1ms so SetTimer can deliver
    // WM_TIMER messages at ~8ms intervals (default is ~15ms).
    // Mirrors `_timerResolutionRaised = timeBeginPeriod(1) == 0` in the
    // .NET reference (AgentCursorOverlay.cs).
    unsafe { let _ = timeBeginPeriod(1); }

    // Collect virtual screen bounds (all monitors).
    let virt_x = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
    let virt_y = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
    let virt_w = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
    let virt_h = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };

    // Update render state with virtual screen bounds.
    {
        let mut guard = RENDER.lock().unwrap();
        if let Some(rs) = guard.as_mut() {
            rs.virt_x = virt_x;
            rs.virt_y = virt_y;
            rs.virt_w = virt_w;
            rs.virt_h = virt_h;
        }
    }

    // Register window class. Class + title use the `Cua.` namespace
    // (PascalCase Windows-class convention, matches the `Programs\Cua\`
    // install-path namespace). Prior versions used `TropeCUA.` — a leaked
    // codename from an early C# reference impl. The string is observable
    // via Win32 EnumWindows; renaming gives users a recognisable namespace
    // (and the `trycua/` org branding) instead of "what's a TropeCUA?".
    let class_name_w: Vec<u16> = "Cua.AgentCursorOverlay\0".encode_utf16().collect();
    let title_w: Vec<u16> = format!("Cua.AgentCursorOverlay.{}\0", cfg.cursor_id)
        .encode_utf16().collect();

    let hinstance = unsafe { GetModuleHandleW(PCWSTR::null()).unwrap_or_default() };

    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(wnd_proc),
        hInstance: hinstance.into(),
        lpszClassName: PCWSTR(class_name_w.as_ptr()),
        ..Default::default()
    };
    unsafe { RegisterClassExW(&wc); } // ignore error if already registered

    // WS_EX_TRANSPARENT | WS_EX_LAYERED | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW
    let ex_style = WS_EX_TRANSPARENT | WS_EX_LAYERED | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW;
    let style    = WS_POPUP;

    let hwnd = unsafe {
        CreateWindowExW(
            ex_style,
            PCWSTR(class_name_w.as_ptr()),
            PCWSTR(title_w.as_ptr()),
            style,
            virt_x, virt_y, virt_w, virt_h,
            None, None,
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
    unsafe { let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE); }

    // Set up timer at 8ms (~125 Hz) matching the C# reference.
    unsafe { SetTimer(hwnd, 1, 8, None); }

    // Store hwnd and rx globally for the wnd_proc callback.
    OVERLAY_HWND.store(hwnd.0 as isize, std::sync::atomic::Ordering::Relaxed);
    *CMD_RX_WIN.lock().unwrap() = Some(rx);
    LAST_ZTICK.store(0, std::sync::atomic::Ordering::Relaxed);

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
fn run_overlay_thread(_cfg: CursorConfig, _rx: std::sync::mpsc::Receiver<OverlayCommand>) {
    // No-op on non-Windows targets (cross-compile guard).
}

// ── Win32 globals (only used on Windows) ─────────────────────────────────

static OVERLAY_HWND: std::sync::atomic::AtomicIsize =
    std::sync::atomic::AtomicIsize::new(0);
static CMD_RX_WIN: Mutex<Option<std::sync::mpsc::Receiver<OverlayCommand>>> = Mutex::new(None);
static LAST_ZTICK: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

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

            // Drain commands and tick animation.  Measure real dt from last
            // tick — Windows timer resolution defaults to 15ms so the
            // hardcoded 8ms was running the animation at half speed.
            let pixmap = {
                let mut guard = RENDER.lock().unwrap();
                if let Some(rs) = guard.as_mut() {
                    // Drain the channel.
                    if let Ok(rx_guard) = CMD_RX_WIN.try_lock() {
                        if let Some(ref rx) = *rx_guard {
                            while let Ok(cmd) = rx.try_recv() {
                                rs.apply_command(cmd);
                            }
                        }
                    }
                    let now = std::time::Instant::now();
                    let dt = now.duration_since(rs.last_tick).as_secs_f64().clamp(0.0, 0.05);
                    rs.last_tick = now;
                    rs.tick(dt);
                    Some(cursor_overlay::render_frame(
                        &rs.core,
                        rs.virt_w.max(1) as u32,
                        rs.virt_h.max(1) as u32,
                        rs.virt_x as f64,
                        rs.virt_y as f64,
                        None, // focus-rect is macOS-only
                    ))
                } else {
                    None
                }
            };

            if let Some(pm) = pixmap {
                update_layered_window(hwnd, &pm);
            }

            // Z-order maintenance every 80ms.
            let last = LAST_ZTICK.load(std::sync::atomic::Ordering::Relaxed);
            if now_ms.wrapping_sub(last) >= 80 {
                LAST_ZTICK.store(now_ms, std::sync::atomic::Ordering::Relaxed);
                reapply_z_order(hwnd);
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
    if w <= 0 || h <= 0 { return; }

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
    let hbmp = CreateDIBSection(
        hdc_mem,
        &bmi,
        DIB_RGB_COLORS,
        &mut bits_ptr,
        None,
        0,
    );
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
        dst[i * 4]     = b;
        dst[i * 4 + 1] = g;
        dst[i * 4 + 2] = r;
        dst[i * 4 + 3] = a;
    }

    // UpdateLayeredWindow.
    let virt_x;
    let virt_y;
    {
        let guard = RENDER.lock().unwrap();
        if let Some(rs) = &*guard {
            virt_x = rs.virt_x;
            virt_y = rs.virt_y;
        } else {
            virt_x = 0;
            virt_y = 0;
        }
    }

    let pt_src = POINT { x: 0, y: 0 };
    let pt_dst = POINT { x: virt_x, y: virt_y };
    let sz     = SIZE  { cx: w, cy: h };
    let blend  = BLENDFUNCTION {
        BlendOp:             0, // AC_SRC_OVER
        BlendFlags:          0,
        SourceConstantAlpha: 255,
        AlphaFormat:         1, // AC_SRC_ALPHA
    };
    let _ = UpdateLayeredWindow(hwnd, hdc_screen, Some(&pt_dst), Some(&sz),
                        hdc_mem, Some(&pt_src), COLORREF(0), Some(&blend), ULW_ALPHA);

    let _ = DeleteObject(hbmp);
    let _ = DeleteDC(hdc_mem);
    ReleaseDC(None, hdc_screen);
}

#[cfg(target_os = "windows")]
unsafe fn reapply_z_order(hwnd: windows::Win32::Foundation::HWND) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::*;

    // Read pinned_wid from render state (RENDER lock released before this is called).
    let pinned_wid = {
        let guard = RENDER.lock().unwrap();
        guard.as_ref().and_then(|rs| rs.core.pinned_wid)
    };

    let pinned_target = pinned_wid.and_then(|wid| {
        let h = HWND(wid as *mut _);
        if IsWindow(h).as_bool() { Some(h) } else { None }
    });

    // The overlay must sit JUST above the pinned target window so the
    // user's foreground app (a different non-topmost window — say their
    // terminal) renders on top of the overlay. Two pitfalls:
    //
    //   1. HWND_TOPMOST was the previous fallback. Once Windows promotes
    //      a window into the topmost band (sets WS_EX_TOPMOST), a later
    //      SetWindowPos with a normal target_hwnd does NOT drop it back
    //      out — the overlay stays above EVERYTHING non-topmost (incl.
    //      the user's foreground). That was the symptom in #1688-style
    //      reports: overlay sticks visible over a terminal even when
    //      the pinned window (Calculator) is behind it.
    //   2. To drop out of the topmost band, we need an explicit
    //      SetWindowPos(hwnd, HWND_NOTOPMOST, …) call. Then we can
    //      issue a second SetWindowPos with the real target_hwnd so
    //      the overlay lands ABOVE the pin but BELOW any window
    //      currently above the pin (the user's foreground).
    //
    // Fallback when there's no live pin: HWND_TOP — top of non-topmost
    // band, NOT the topmost band. So a "no pin" overlay still respects
    // the user's foreground stack.
    let _ = SetWindowPos(
        hwnd,
        HWND_NOTOPMOST,
        0, 0, 0, 0,
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
    );

    let insert_after = pinned_target.unwrap_or(HWND_TOP);
    let _ = SetWindowPos(
        hwnd,
        insert_after,
        0, 0, 0, 0,
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_NOOWNERZORDER,
    );
}

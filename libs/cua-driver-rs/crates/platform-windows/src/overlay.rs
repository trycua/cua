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

#![allow(non_snake_case, non_upper_case_globals)]

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use cursor_overlay::{
    CursorConfig, CursorShape, MotionConfig, OverlayCommand, Palette, PathPlanner, PathState,
    PlannedPath,
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
        .and_then(|g| g.as_ref().map(|rs| rs.motion.glide_duration_ms))
        .unwrap_or(750.0)
}

/// Returns true if the cursor overlay is currently enabled/visible.
pub fn is_enabled() -> bool {
    RENDER.lock().ok()
        .and_then(|g| g.as_ref().map(|rs| rs.visible))
        .unwrap_or(false)
}

/// Snapshot the current motion config (start_handle / end_handle / arc_size /
/// arc_flow / spring / glide_duration_ms / dwell_after_click_ms /
/// idle_hide_ms).  Mirrors macOS `current_motion()` so
/// `get_agent_cursor_state` can report the live values.
pub fn current_motion() -> MotionConfig {
    RENDER.lock().ok()
        .and_then(|g| g.as_ref().map(|rs| rs.motion.clone()))
        .unwrap_or_default()
}

/// Returns the current cursor position in screen coordinates.
pub fn current_position() -> (f64, f64) {
    RENDER.lock().ok()
        .and_then(|g| g.as_ref().map(|rs| rs.pos))
        .unwrap_or((-200.0, -200.0))
}

/// Returns true if the cursor is still at the off-screen initial position
/// (-200, -200), meaning it has never been positioned on screen yet.
pub fn is_at_initial_position() -> bool {
    RENDER.lock().ok()
        .and_then(|g| g.as_ref().map(|rs| rs.pos.0 < 0.0 && rs.pos.1 < 0.0))
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
            Some(rs) => rs.cfg.clone(),
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

// ── Animation state (same structure as macOS) ─────────────────────────────

struct RenderState {
    cfg:        CursorConfig,
    palette:    Palette,
    motion:     MotionConfig,
    pos:        (f64, f64),
    heading:    f64,
    path:       Option<PlannedPath>,
    dist:       f64,
    start_t:    Instant,
    spring:     Option<Spring>,
    spring_tgt: Option<(f64, f64, f64)>,
    click_t:    Option<f64>,
    shape:      Option<CursorShape>,
    visible:    bool,
    idle_secs:  f64,
    idle_alpha: f64,
    pinned_wid: Option<u64>,
    gradient_colors: Vec<[u8; 4]>,
    bloom_override: Option<[u8; 4]>,
    last_tick: Instant,
    // Virtual screen dimensions set after window creation.
    virt_x: i32,
    virt_y: i32,
    virt_w: i32,
    virt_h: i32,
}

#[derive(Clone, Copy)]
struct Spring { ox: f64, oy: f64, vx: f64, vy: f64 }

impl RenderState {
    fn new(cfg: CursorConfig) -> Self {
        let palette = cfg.palette();
        let motion  = cfg.motion.clone();
        let shape   = cfg.shape.clone();
        RenderState {
            cfg, palette, motion, shape,
            pos:        (-200.0, -200.0),
            heading:    std::f64::consts::FRAC_PI_4,
            path:       None,
            dist:       0.0,
            start_t:    Instant::now(),
            spring:     None,
            spring_tgt: None,
            click_t:    None,
            visible:    true,
            idle_secs:  0.0,
            idle_alpha: 1.0,
            pinned_wid: None,
            gradient_colors: vec![],
            bloom_override: None,
            last_tick: Instant::now(),
            virt_x: 0, virt_y: 0, virt_w: 1920, virt_h: 1080,
        }
    }

    fn tick(&mut self, dt: f64) {
        let spring_k  = self.motion.spring * 400.0;
        let spring_c  = self.motion.spring * 20.0;

        if let Some(ref p) = self.path {
            // Speed-based motion: 16*u²*(1-u)² peaks at exactly 1.0 at u=0.5,
            // matching Swift AgentCursorRenderer (peakSpeed=900, min=300/200).
            let path_frac = (self.dist / p.length.max(1.0)).clamp(0.0, 1.0);
            let profile   = 16.0 * path_frac * path_frac * (1.0 - path_frac) * (1.0 - path_frac);
            let floor     = if path_frac < 0.5 { self.motion.min_start_speed } else { self.motion.min_end_speed };
            let speed     = (floor + (self.motion.peak_speed - floor) * profile).max(floor);
            self.dist += speed * dt;

            let path_len = p.length.max(1.0);
            if self.dist >= path_len {
                let end = p.sample(path_len);
                let end_heading = p.end_visual_heading;
                let vh = end.heading;
                self.spring = Some(Spring {
                    ox: 0.0, oy: 0.0,
                    vx: speed * 0.5 * vh.cos(),
                    vy: speed * 0.5 * vh.sin(),
                });
                self.spring_tgt = Some((end.x, end.y, end_heading));
                self.pos = (end.x, end.y);
                self.heading = end_heading;
                self.path = None;
                self.dist = 0.0;
            } else {
                let s: PathState = p.sample(self.dist);
                self.pos = (s.x, s.y);
                let desired = s.heading + std::f64::consts::PI;
                let max_step = 14.0 * dt;
                self.heading = rotate_toward(self.heading, desired, max_step);
            }
        } else if let Some(mut s) = self.spring {
            if let Some((tx, ty, th)) = self.spring_tgt {
                let substeps = 4;
                let sdt = dt / substeps as f64;
                for _ in 0..substeps {
                    s.vx += (-spring_k * s.ox - spring_c * s.vx) * sdt;
                    s.vy += (-spring_k * s.oy - spring_c * s.vy) * sdt;
                    s.ox += s.vx * sdt;
                    s.oy += s.vy * sdt;
                }
                self.pos = (tx + s.ox, ty + s.oy);
                self.heading = th;
                if s.ox.hypot(s.oy) < 0.3 && s.vx.hypot(s.vy) < 2.0 {
                    self.pos = (tx, ty);
                    self.spring = None;
                } else {
                    self.spring = Some(s);
                }
            }
        }

        if let Some(t) = self.click_t {
            let next = t + dt * 4.0;
            self.click_t = if next >= 1.0 { None } else { Some(next) };
        }

        // Idle-hide with 180ms fade matching Windows reference.
        let idle_hide_ms = self.motion.idle_hide_ms;
        if idle_hide_ms > 0.0 {
            let moving = self.path.is_some() || self.spring.is_some() || self.click_t.is_some();
            if moving {
                self.idle_secs  = 0.0;
                self.idle_alpha = 1.0;
            } else {
                self.idle_secs += dt;
                let fade_start = idle_hide_ms / 1000.0;
                let fade_end   = fade_start + 0.18;
                if self.idle_secs > fade_end {
                    self.idle_alpha = 0.0;
                } else if self.idle_secs > fade_start {
                    let t = (self.idle_secs - fade_start) / 0.18;
                    self.idle_alpha = 1.0 - t.clamp(0.0, 1.0);
                }
            }
        } else {
            self.idle_alpha = 1.0;
        }
    }

    fn apply_command(&mut self, cmd: OverlayCommand) {
        match cmd {
            OverlayCommand::MoveTo { x, y, end_heading_radians } => {
                let (x0, y0) = self.pos;
                let th0 = self.heading + std::f64::consts::PI;
                let th1 = end_heading_radians + std::f64::consts::PI;
                const CLICK_OFFSET: f64 = 16.0;
                const TURN_RADIUS:  f64 = 80.0;
                let tx = x + end_heading_radians.cos() * CLICK_OFFSET;
                let ty = y + end_heading_radians.sin() * CLICK_OFFSET;
                let plan = PathPlanner::plan(
                    x0, y0, th0,
                    tx, ty, th1,
                    end_heading_radians,
                    TURN_RADIUS,
                );
                self.path = Some(plan);
                self.dist = 0.0;
                self.start_t = Instant::now();
                self.spring = None;
                self.spring_tgt = None;
                self.idle_secs = 0.0;
                self.idle_alpha = 1.0;
            }
            OverlayCommand::ClickPulse { x, y } => {
                self.pos = (x, y);
                self.click_t = Some(0.0);
                self.idle_secs = 0.0;
                self.idle_alpha = 1.0;
            }
            OverlayCommand::SetEnabled(v) => {
                self.visible = v;
            }
            OverlayCommand::SetMotion(m) => {
                self.motion = m;
            }
            OverlayCommand::SetPalette(p) => {
                self.palette = p;
            }
            OverlayCommand::PinAbove(wid) => {
                self.pinned_wid = Some(wid);
            }
            OverlayCommand::SetGradient { gradient_colors, bloom_color } => {
                self.gradient_colors = gradient_colors;
                self.bloom_override = bloom_color;
            }
            OverlayCommand::SetShape(shape) => {
                self.shape = shape;
            }
            OverlayCommand::ShowFocusRect(_) => {}
        }
    }
}

fn rotate_toward(current: f64, desired: f64, max_step: f64) -> f64 {
    let mut diff = desired - current;
    while diff >  std::f64::consts::PI { diff -= 2.0 * std::f64::consts::PI; }
    while diff < -std::f64::consts::PI { diff += 2.0 * std::f64::consts::PI; }
    current + diff.clamp(-max_step, max_step)
}

// ── tiny-skia render (shared logic) ──────────────────────────────────────

fn render_frame(rs: &RenderState) -> tiny_skia::Pixmap {
    let w = rs.virt_w.max(1) as u32;
    let h = rs.virt_h.max(1) as u32;
    let mut pm = tiny_skia::Pixmap::new(w, h)
        .unwrap_or_else(|| tiny_skia::Pixmap::new(1, 1).unwrap());

    if !rs.visible || rs.pos.0 < -100.0 || rs.idle_alpha < 0.004 {
        return pm;
    }

    let (px, py) = (
        rs.pos.0 - rs.virt_x as f64,
        rs.pos.1 - rs.virt_y as f64,
    );
    let heading     = rs.heading;
    let alpha_scale = rs.idle_alpha as f32;

    // Bloom — use runtime bloom_override if set.
    let bloom_r: f32 = 22.0;
    let (br, bg, bb) = if let Some([r, g, b, _]) = rs.bloom_override {
        (r, g, b)
    } else {
        let [r, g, b, _] = rs.palette.bloom_inner; (r, g, b)
    };
    let (or_, og, ob) = if let Some([r, g, b, _]) = rs.bloom_override {
        (r, g, b)
    } else {
        let [r, g, b, _] = rs.palette.bloom_outer; (r, g, b)
    };
    let bloom_inner = tiny_skia::Color::from_rgba8(br, bg, bb, (115.0 * alpha_scale) as u8);
    let bloom_outer = tiny_skia::Color::from_rgba8(or_, og, ob, (26.0 * alpha_scale) as u8);
    let bloom_zero  = tiny_skia::Color::from_rgba8(or_, og, ob, 0);

    let bloom_paint = {
        let mut p = tiny_skia::Paint::default();
        p.shader = tiny_skia::RadialGradient::new(
            tiny_skia::Point::from_xy(px as f32, py as f32),
            tiny_skia::Point::from_xy(px as f32, py as f32),
            bloom_r,
            vec![
                tiny_skia::GradientStop::new(0.0, bloom_inner),
                tiny_skia::GradientStop::new(0.5, bloom_outer),
                tiny_skia::GradientStop::new(1.0, bloom_zero),
            ],
            tiny_skia::SpreadMode::Pad,
            tiny_skia::Transform::identity(),
        ).unwrap_or(tiny_skia::Shader::SolidColor(bloom_inner));
        p.anti_alias = true;
        p
    };

    if let Some(r) = tiny_skia::Rect::from_xywh(
        (px - bloom_r as f64) as f32, (py - bloom_r as f64) as f32,
        bloom_r * 2.0, bloom_r * 2.0,
    ) {
        pm.fill_rect(r, &bloom_paint, tiny_skia::Transform::identity(), None);
    }

    // Click pulse.
    if let Some(t) = rs.click_t {
        let ring_r = (bloom_r + 20.0 * t as f32) * (1.0 - t as f32 * 0.5);
        let alpha   = ((1.0 - t) * 180.0 * alpha_scale as f64) as u8;
        let [cr, cg, cb, _] = rs.palette.cursor_mid;
        let ring_color = tiny_skia::Color::from_rgba8(cr, cg, cb, alpha);
        let mut ring_paint = tiny_skia::Paint::default();
        ring_paint.shader = tiny_skia::Shader::SolidColor(ring_color);
        ring_paint.anti_alias = true;
        let stroke = tiny_skia::Stroke { width: 2.0, ..Default::default() };
        let mut pb = tiny_skia::PathBuilder::new();
        pb.push_circle(px as f32, py as f32, ring_r);
        if let Some(path) = pb.finish() {
            pm.stroke_path(&path, &ring_paint, &stroke, tiny_skia::Transform::identity(), None);
        }
    }

    // Arrow / custom shape.
    if let Some(ref shape) = rs.shape {
        let sz = 32.0_f32;
        if let Some(pix) = tiny_skia::PixmapRef::from_bytes(&shape.pixels, shape.width, shape.height) {
            let transform = tiny_skia::Transform::from_rotate_at(
                heading.to_degrees() as f32 + 180.0,
                px as f32, py as f32,
            ).pre_translate(px as f32 - sz / 2.0, py as f32 - sz / 2.0);
            let mut paint = tiny_skia::PixmapPaint::default();
            paint.opacity = alpha_scale;
            pm.draw_pixmap(0, 0, pix, &paint, transform, None);
        }
    } else {
        let grad_override = if rs.gradient_colors.is_empty() { None } else { Some(&rs.gradient_colors) };
        draw_default_arrow(&mut pm, &rs.palette, grad_override, px as f32, py as f32, heading as f32, alpha_scale);
    }

    pm
}

fn draw_default_arrow(
    pm: &mut tiny_skia::Pixmap,
    palette: &Palette,
    gradient_override: Option<&Vec<[u8; 4]>>,
    px: f32, py: f32,
    heading: f32,
    alpha_scale: f32,
) {
    let verts: [(f32, f32); 4] = [(14.0, 0.0), (-8.0, -9.0), (-3.0, 0.0), (-8.0, 9.0)];
    let angle = heading + std::f64::consts::PI as f32;
    let (sa, ca) = (angle.sin(), angle.cos());
    let transform_pt = |(vx, vy): (f32, f32)| -> (f32, f32) {
        (px + ca * vx - sa * vy, py + sa * vx + ca * vy)
    };
    let pts: Vec<(f32, f32)> = verts.iter().map(|&v| transform_pt(v)).collect();
    let mut pb = tiny_skia::PathBuilder::new();
    pb.move_to(pts[0].0, pts[0].1);
    for p in &pts[1..] { pb.line_to(p.0, p.1); }
    pb.close();
    let arrow_path = match pb.finish() { Some(p) => p, None => return };

    let tip  = pts[0];
    let tail = ((pts[1].0 + pts[3].0) / 2.0, (pts[1].1 + pts[3].1) / 2.0);
    let (r0, g0, b0) = if let Some(g) = gradient_override.and_then(|g| g.first()) {
        (g[0], g[1], g[2])
    } else { let [r, g, b, _] = palette.cursor_start; (r, g, b) };
    let (r1, g1, b1) = if let Some(g) = gradient_override.and_then(|g| g.get(1).or_else(|| g.first())) {
        (g[0], g[1], g[2])
    } else { let [r, g, b, _] = palette.cursor_mid; (r, g, b) };
    let (r2, g2, b2) = if let Some(g) = gradient_override.and_then(|g| g.last()) {
        (g[0], g[1], g[2])
    } else { let [r, g, b, _] = palette.cursor_end; (r, g, b) };
    let a = (255.0 * alpha_scale) as u8;

    let fill_paint = {
        let mut p = tiny_skia::Paint::default();
        p.shader = tiny_skia::LinearGradient::new(
            tiny_skia::Point::from_xy(tip.0, tip.1),
            tiny_skia::Point::from_xy(tail.0, tail.1),
            vec![
                tiny_skia::GradientStop::new(0.00, tiny_skia::Color::from_rgba8(r0, g0, b0, a)),
                tiny_skia::GradientStop::new(0.53, tiny_skia::Color::from_rgba8(r1, g1, b1, a)),
                tiny_skia::GradientStop::new(1.00, tiny_skia::Color::from_rgba8(r2, g2, b2, a)),
            ],
            tiny_skia::SpreadMode::Pad,
            tiny_skia::Transform::identity(),
        ).unwrap_or(tiny_skia::Shader::SolidColor(tiny_skia::Color::from_rgba8(r1, g1, b1, a)));
        p.anti_alias = true;
        p
    };
    pm.fill_path(&arrow_path, &fill_paint, tiny_skia::FillRule::Winding,
                 tiny_skia::Transform::identity(), None);

    let mut stroke_paint = tiny_skia::Paint::default();
    stroke_paint.shader = tiny_skia::Shader::SolidColor(tiny_skia::Color::from_rgba8(255, 255, 255, a));
    stroke_paint.anti_alias = true;
    let stroke = tiny_skia::Stroke { width: 1.5, ..Default::default() };
    pm.stroke_path(&arrow_path, &stroke_paint, &stroke, tiny_skia::Transform::identity(), None);
}

// ── Win32 message-loop thread ─────────────────────────────────────────────

#[cfg(target_os = "windows")]
fn run_overlay_thread(cfg: CursorConfig, rx: std::sync::mpsc::Receiver<OverlayCommand>) {
    use windows::Win32::Foundation::*;
    use windows::Win32::UI::WindowsAndMessaging::*;
    use windows::Win32::Graphics::Gdi::*;
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

    // Register window class.
    let class_name_w: Vec<u16> = "TropeCUA.AgentCursorOverlay\0".encode_utf16().collect();
    let title_w: Vec<u16> = format!("TropeCUA.AgentCursorOverlay.{}\0", cfg.cursor_id)
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
    unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE); }

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
            TranslateMessage(&msg);
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
    use windows::Win32::Graphics::Gdi::*;

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
                    if let Ok(mut rx_guard) = CMD_RX_WIN.try_lock() {
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
                    Some(render_frame(rs))
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
    let mut bmi = BITMAPINFO {
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
        DeleteDC(hdc_mem);
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
    UpdateLayeredWindow(hwnd, hdc_screen, Some(&pt_dst), Some(&sz),
                        hdc_mem, Some(&pt_src), COLORREF(0), Some(&blend), ULW_ALPHA);

    DeleteObject(hbmp);
    DeleteDC(hdc_mem);
    ReleaseDC(None, hdc_screen);
}

#[cfg(target_os = "windows")]
unsafe fn reapply_z_order(hwnd: windows::Win32::Foundation::HWND) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::*;

    // Read pinned_wid from render state (RENDER lock released before this is called).
    let pinned_wid = {
        let guard = RENDER.lock().unwrap();
        guard.as_ref().and_then(|rs| rs.pinned_wid)
    };

    // If a target window is pinned, place the overlay just above it.
    // Falls back to HWND_TOP when not set or when the target is gone.
    let insert_after = if let Some(wid) = pinned_wid {
        let target = HWND(wid as *mut _);
        if IsWindow(target).as_bool() {
            target
        } else {
            HWND_TOPMOST
        }
    } else {
        HWND_TOPMOST
    };

    let _ = SetWindowPos(
        hwnd,
        insert_after,
        0, 0, 0, 0,
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_NOOWNERZORDER,
    );
}

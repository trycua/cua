//! The visible recording indicator — a glowing border drawn around the target
//! window — and its watchdog. This is the half of the "recording-LED" coupling
//! that *proves* recording is visible: its paint loop bumps the shared
//! [`IndicatorHeartbeat`] from inside an actually-presented frame, and its
//! watchdog marks the heartbeat compromised the instant the border stops being
//! visible / topmost / covering the target.
//!
//! Windows implementation: a click-through layered topmost tool window
//! (`WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW`)
//! that tracks the target window's rect and repaints a pulsing glow at ~30 Hz
//! via `UpdateLayeredWindow`. On non-Windows it is a no-op stub (capture is
//! `Unsupported` there anyway, so the gate never opens).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::gate::IndicatorHeartbeat;

/// A running recording indicator. Dropping it tears the border window down and
/// (because the heartbeat stops) latches capture dark.
pub struct Indicator {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Indicator {
    /// Start the indicator for `target_hwnd`, bumping `heartbeat` from each
    /// presented frame. `clock` must be the SAME monotonic-ms clock the capture
    /// gate uses, so freshness comparisons are apples-to-apples.
    pub fn start(
        target_hwnd: isize,
        heartbeat: Arc<IndicatorHeartbeat>,
        clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    ) -> anyhow::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let thread = platform::spawn(target_hwnd, heartbeat, clock, stop.clone())?;
        Ok(Self { stop, thread: Some(thread) })
    }
}

impl Drop for Indicator {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use super::*;
    use std::time::Instant;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::DefWindowProcW;

    /// Trivial window procedure — the border never handles messages itself
    /// (it is painted via UpdateLayeredWindow), so defer everything.
    unsafe extern "system" fn border_wnd_proc(
        hwnd: HWND,
        msg: u32,
        w: WPARAM,
        l: LPARAM,
    ) -> LRESULT {
        DefWindowProcW(hwnd, msg, w, l)
    }
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC,
        SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, HBITMAP,
        HDC, HGDIOBJ,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, GetWindowRect, IsIconic, IsWindow, IsWindowVisible,
        RegisterClassW, SetWindowPos, ShowWindow, UpdateLayeredWindow, HWND_TOPMOST, SWP_NOACTIVATE,
        SWP_NOSIZE, SW_SHOWNOACTIVATE, ULW_ALPHA, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
        WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
    };

    /// Outward glow radius in pixels. The glow starts exactly at the window's
    /// bounding box (no gap) and fades outward over this many pixels, so the
    /// window looks like it has a soft red blurred border + shadow.
    const GLOW: i32 = 18;
    const TARGET_FPS_MS: u64 = 33;

    pub fn spawn(
        target_hwnd: isize,
        heartbeat: Arc<IndicatorHeartbeat>,
        clock: Arc<dyn Fn() -> u64 + Send + Sync>,
        stop: Arc<AtomicBool>,
    ) -> anyhow::Result<std::thread::JoinHandle<()>> {
        let handle = std::thread::Builder::new()
            .name("recording-indicator".into())
            .spawn(move || {
                if let Err(e) = run(target_hwnd, &heartbeat, &clock, &stop) {
                    tracing::warn!("recording indicator stopped: {e}");
                }
                // On exit the heartbeat naturally goes stale; also latch dark.
                heartbeat.mark_compromised();
            })?;
        Ok(handle)
    }

    fn run(
        target_hwnd: isize,
        heartbeat: &Arc<IndicatorHeartbeat>,
        clock: &Arc<dyn Fn() -> u64 + Send + Sync>,
        stop: &Arc<AtomicBool>,
    ) -> anyhow::Result<()> {
        let class_name: Vec<u16> = "CuaRecordingBorder\0".encode_utf16().collect();
        let hinstance = unsafe {
            windows::Win32::System::LibraryLoader::GetModuleHandleW(PCWSTR::null())?
        };
        let wc = WNDCLASSW {
            lpfnWndProc: Some(border_wnd_proc),
            hInstance: hinstance.into(),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        // Ignore "class already registered" on a second start.
        unsafe {
            RegisterClassW(&wc);
        }

        let ex_style =
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST;
        let hwnd = unsafe {
            CreateWindowExW(
                ex_style,
                PCWSTR(class_name.as_ptr()),
                PCWSTR::null(),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                hinstance,
                None,
            )?
        };
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }

        let start = Instant::now();
        let mut last_size = (0i32, 0i32);
        // Reused DIB buffer; recreated when the target window resizes.
        let mut dib: Option<DibSurface> = None;

        let result = (|| -> anyhow::Result<()> {
            while !stop.load(Ordering::SeqCst) {
                let target = HWND(target_hwnd as *mut _);
                // Watchdog: target must still exist and be visible/non-minimized.
                let alive = unsafe {
                    IsWindow(target).as_bool()
                        && IsWindowVisible(target).as_bool()
                        && !IsIconic(target).as_bool()
                };
                if !alive {
                    heartbeat.mark_compromised();
                    std::thread::sleep(std::time::Duration::from_millis(TARGET_FPS_MS));
                    continue;
                }

                let mut rect = RECT::default();
                if unsafe { GetWindowRect(target, &mut rect) }.is_err() {
                    heartbeat.mark_compromised();
                    std::thread::sleep(std::time::Duration::from_millis(TARGET_FPS_MS));
                    continue;
                }

                let tx = rect.left - GLOW;
                let ty = rect.top - GLOW;
                let tw = (rect.right - rect.left) + 2 * GLOW;
                let th = (rect.bottom - rect.top) + 2 * GLOW;
                if tw <= 0 || th <= 0 {
                    std::thread::sleep(std::time::Duration::from_millis(TARGET_FPS_MS));
                    continue;
                }

                if (tw, th) != last_size {
                    dib = Some(DibSurface::new(tw, th)?);
                    last_size = (tw, th);
                }
                let surf = dib.as_mut().unwrap();

                let phase = start.elapsed().as_secs_f64();
                let pulse = 0.55 + 0.45 * (phase * 3.0).sin().abs();
                surf.paint_border(tw, th, GLOW, pulse);

                // Keep the border topmost and positioned over the target.
                unsafe {
                    let _ = SetWindowPos(
                        hwnd,
                        HWND_TOPMOST,
                        tx,
                        ty,
                        tw,
                        th,
                        SWP_NOACTIVATE,
                    );
                    let _ = SWP_NOSIZE;
                }

                let presented = surf.present(hwnd, tx, ty, tw, th);
                if presented {
                    // Heartbeat = proof a covering border frame was drawn. The
                    // covered rect is the TARGET window (not the inflated
                    // border) so the gate only opens for points in the window.
                    let now = clock();
                    heartbeat.present(
                        now,
                        [
                            rect.left,
                            rect.top,
                            rect.right - rect.left,
                            rect.bottom - rect.top,
                        ],
                    );
                } else {
                    heartbeat.mark_compromised();
                }

                std::thread::sleep(std::time::Duration::from_millis(TARGET_FPS_MS));
            }
            Ok(())
        })();

        unsafe {
            let _ = DestroyWindow(hwnd);
        }
        result
    }

    /// A 32-bit premultiplied-BGRA DIB section + memory DC for layered paint.
    struct DibSurface {
        screen_dc: HDC,
        dc: HDC,
        bmp: HBITMAP,
        old: HGDIOBJ,
        bits: *mut u8,
        #[allow(dead_code)]
        w: i32,
        #[allow(dead_code)]
        h: i32,
    }

    impl DibSurface {
        fn new(w: i32, h: i32) -> anyhow::Result<Self> {
            unsafe {
                let screen_dc = GetDC(None);
                let dc = CreateCompatibleDC(screen_dc);
                let bmi = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: w,
                        biHeight: -h, // top-down
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB.0,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
                let bmp = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, None, 0)?;
                if bits.is_null() {
                    let _ = DeleteDC(dc);
                    ReleaseDC(None, screen_dc);
                    anyhow::bail!("CreateDIBSection returned null bits");
                }
                let old = SelectObject(dc, bmp);
                Ok(Self { screen_dc, dc, bmp, old, bits: bits as *mut u8, w, h })
            }
        }

        /// Paint a red glow that emanates **outward** from the target window's
        /// bounding box. The buffer is the window inflated by `glow` on all
        /// sides; the inner rect `[glow, glow, w-glow, h-glow]` is the window
        /// itself and stays fully transparent (its content shows through, and
        /// WS_EX_TRANSPARENT keeps it click-through). For pixels outside the
        /// window, alpha is brightest right at the window edge (no gap) and
        /// falls off to 0 at `glow` px out — a soft blurred border + shadow.
        fn paint_border(&mut self, w: i32, h: i32, glow: i32, pulse: f64) {
            // Bright recording red (distinct from the cyan focus rect).
            let (cr, cg, cb) = (255.0f64, 45.0, 30.0);
            let buf = unsafe { std::slice::from_raw_parts_mut(self.bits, (w * h * 4) as usize) };
            let glow = glow.max(1);
            let glow_f = glow as f64;
            // Inner rect = the window bbox within the inflated buffer.
            let (il, it, ir, ib) = (glow, glow, w - 1 - glow, h - 1 - glow);
            for y in 0..h {
                for x in 0..w {
                    let i = ((y * w + x) * 4) as usize;
                    // Distance the pixel lies OUTSIDE the window rect (0 inside).
                    let dx = (il - x).max(x - ir).max(0) as f64;
                    let dy = (it - y).max(y - ib).max(0) as f64;
                    if dx == 0.0 && dy == 0.0 {
                        // inside the window: fully transparent
                        buf[i] = 0;
                        buf[i + 1] = 0;
                        buf[i + 2] = 0;
                        buf[i + 3] = 0;
                        continue;
                    }
                    let d = (dx * dx + dy * dy).sqrt();
                    if d > glow_f {
                        buf[i] = 0;
                        buf[i + 1] = 0;
                        buf[i + 2] = 0;
                        buf[i + 3] = 0;
                        continue;
                    }
                    // 1.0 at the window edge -> 0.0 at the outer edge of the glow.
                    let f = 1.0 - d / glow_f;
                    let mut a = f * f * 235.0 * pulse;
                    // Crisp bright line hugging the window edge (no gap).
                    if d <= 2.0 {
                        a = a.max(230.0 * pulse);
                    }
                    let a = a.clamp(0.0, 255.0);
                    let af = a / 255.0;
                    // premultiplied BGRA for ULW_ALPHA
                    buf[i] = (cb * af) as u8;
                    buf[i + 1] = (cg * af) as u8;
                    buf[i + 2] = (cr * af) as u8;
                    buf[i + 3] = a as u8;
                }
            }
        }

        /// Push the buffer to the layered window. Returns false on failure.
        fn present(&self, hwnd: HWND, x: i32, y: i32, w: i32, h: i32) -> bool {
            unsafe {
                let src = POINT { x: 0, y: 0 };
                let dst = POINT { x, y };
                let size = SIZE { cx: w, cy: h };
                let blend = BLENDFUNCTION {
                    BlendOp: 0,      // AC_SRC_OVER
                    BlendFlags: 0,
                    SourceConstantAlpha: 255,
                    AlphaFormat: 1, // AC_SRC_ALPHA
                };
                UpdateLayeredWindow(
                    hwnd,
                    self.screen_dc,
                    Some(&dst),
                    Some(&size),
                    self.dc,
                    Some(&src),
                    COLORREF(0),
                    Some(&blend),
                    ULW_ALPHA,
                )
                .is_ok()
            }
        }
    }

    impl Drop for DibSurface {
        fn drop(&mut self) {
            unsafe {
                SelectObject(self.dc, self.old);
                let _ = DeleteObject(self.bmp);
                let _ = DeleteDC(self.dc);
                ReleaseDC(None, self.screen_dc);
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod platform {
    use super::*;
    pub fn spawn(
        _target_hwnd: isize,
        _heartbeat: Arc<IndicatorHeartbeat>,
        _clock: Arc<dyn Fn() -> u64 + Send + Sync>,
        _stop: Arc<AtomicBool>,
    ) -> anyhow::Result<std::thread::JoinHandle<()>> {
        // No indicator on non-Windows yet; capture is Unsupported there so the
        // gate never opens regardless.
        anyhow::bail!("recording indicator not supported on this platform")
    }
}

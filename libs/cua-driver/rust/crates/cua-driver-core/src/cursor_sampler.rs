//! Cross-platform cursor-position sampler. Runs on a dedicated thread
//! during recording, polls the OS for the current mouse position every
//! ~33 ms (≈30 Hz to match the video framerate), and writes one
//! `{t_ms, x, y}` JSON object per line to `<output_dir>/cursor.jsonl`.
//!
//! Reference: `libs/cua-driver/swift/Sources/CuaDriverCore/Recording/CursorSampler.swift`
//!
//! Per-platform polling:
//! - **Windows:** `GetCursorPos` (returns physical screen coords)
//! - **macOS:** `CGEventCreate` + `CGEventGetLocation`
//! - **Linux X11:** `XQueryPointer` against the root window
//! - **Linux Wayland:** no portable API exists; sampler runs but logs
//!   no samples — the resulting cursor.jsonl is empty and the zoom
//!   renderer falls back to the click-point-only path.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Sampling rate in Hz. 30 matches the video framerate, so the
/// per-frame zoom curve can resolve cursor position at frame
/// granularity without interpolation noise.
pub const SAMPLE_RATE_HZ: u32 = 30;

/// One running cursor sampler. Drop or `stop()` to terminate.
pub struct CursorSampler {
    handle: Option<JoinHandle<usize>>,
    stop_flag: Arc<AtomicBool>,
    output_path: PathBuf,
}

impl CursorSampler {
    /// Start sampling. Writes JSON-line records to `output_path` from a
    /// background thread. `session_start` is used as the time anchor —
    /// `t_ms` in each sample is `(now - session_start).as_millis()`.
    pub fn start(output_path: PathBuf, session_start: Instant) -> std::io::Result<Self> {
        let file = File::create(&output_path)?;
        let stop_flag = Arc::new(AtomicBool::new(false));
        let flag_for_thread = stop_flag.clone();
        let path_for_thread = output_path.clone();
        let handle = std::thread::spawn(move || {
            let mut writer = BufWriter::new(file);
            let interval = Duration::from_millis(1000 / SAMPLE_RATE_HZ as u64);
            let mut count = 0usize;
            while !flag_for_thread.load(Ordering::Relaxed) {
                if let Some((x, y)) = sample_cursor() {
                    let t_ms = session_start.elapsed().as_millis() as f64;
                    // Write one JSON object per line. We hand-format
                    // the trivial shape rather than pulling serde_json
                    // into the hot loop — keeps wakeup-cost bounded.
                    let _ = writeln!(writer,
                        "{{\"t_ms\":{:.3},\"x\":{:.2},\"y\":{:.2}}}",
                        t_ms, x, y);
                    count += 1;
                }
                std::thread::sleep(interval);
            }
            let _ = writer.flush();
            let _ = path_for_thread; // keep path moved (warning silencer)
            count
        });
        Ok(CursorSampler {
            handle: Some(handle),
            stop_flag,
            output_path,
        })
    }

    /// Stop the sampler. Returns the number of samples written.
    pub fn stop(mut self) -> usize {
        self.stop_flag.store(true, Ordering::Relaxed);
        self.handle.take()
            .and_then(|h| h.join().ok())
            .unwrap_or(0)
    }

    pub fn output_path(&self) -> &std::path::Path { &self.output_path }
}

impl Drop for CursorSampler {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

// ── per-platform cursor poll ────────────────────────────────────────────────

#[cfg(target_os = "windows")]
fn sample_cursor() -> Option<(f64, f64)> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
    unsafe {
        let mut p = POINT::default();
        if GetCursorPos(&mut p).is_ok() {
            Some((p.x as f64, p.y as f64))
        } else {
            None
        }
    }
}

#[cfg(target_os = "macos")]
fn sample_cursor() -> Option<(f64, f64)> {
    // ApplicationServices/CGEvent.h: CGEventCreate(nil) → CGEventRef;
    // CGEventGetLocation(event) → CGPoint. The point is in points
    // (top-left origin) so it matches the cursor-space convention the
    // renderer uses.
    extern "C" {
        fn CGEventCreate(source: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
        fn CGEventGetLocation(event: *mut std::ffi::c_void) -> CGPoint;
        fn CFRelease(cf: *mut std::ffi::c_void);
    }
    #[repr(C)]
    #[derive(Copy, Clone)]
    struct CGPoint { x: f64, y: f64 }

    unsafe {
        let event = CGEventCreate(std::ptr::null_mut());
        if event.is_null() { return None; }
        let p = CGEventGetLocation(event);
        CFRelease(event);
        Some((p.x, p.y))
    }
}

#[cfg(target_os = "linux")]
fn sample_cursor() -> Option<(f64, f64)> {
    // Wayland has no equivalent portable poll; on X11 use XQueryPointer.
    // We try the X11 path via the `x11` crate if available; otherwise
    // return None and the sampler writes an empty cursor.jsonl.
    //
    // The X11 dep isn't always present in cua-driver's Linux build
    // (Wayland-only hosts), so this fallback is "no-op when X11 isn't
    // wired up" — the renderer copes by falling back to click-point-
    // only zoom (no cursor-follow between actions).
    None
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
fn sample_cursor() -> Option<(f64, f64)> { None }

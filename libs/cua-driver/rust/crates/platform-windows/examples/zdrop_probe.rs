//! Z-drop harness + Track A background-input probe.
//!
//! Measures the foreground "flash": when actuating input into a *background*
//! window, does the target window rise above the user's foreground window?
//!
//! The harness mirrors the `flash-repro` methodology referenced in
//! `uia/fg_bypass.rs`: capture the user's foreground window as `baseline`, then
//! poll the z-order at high frequency while an actuation runs on another thread.
//! A "z-drop" is any sample where `target` sits above `baseline` (or `baseline`
//! is no longer the foreground). 0 z-drops == no visible flash.
//!
//! Two actuators are compared against the SAME background target:
//!   - control: SendInput with a brief SetForegroundWindow swap (the existing
//!     flash path in `input/mouse.rs::send_click_synthesized`).
//!   - inject : Track A — pointer/touch injection (InitializeTouchInjection +
//!     InjectTouchInput), which RE showed has no foreground precondition
//!     (NtUserInjectMouseInput gates only on per-process injection-enable, not
//!     GetForegroundWindow — see docs/windows-background-input-re-plan.rs §4.4).
//!
//! Usage (run from an interactive desktop session, with a Chrome/WPF/etc window
//! open in the BACKGROUND and a different window focused in front):
//!   cargo run -p platform-windows --example zdrop_probe -- [mode] [pid]
//!   mode = both (default) | control | inject | list
//!
//! It prints the candidate windows, the baseline + target, a 2s countdown
//! (position your windows), then the z-drop numbers per actuator.
//!
//! WARNING: this performs a real left-click/tap at the target window's center.
//! Pick a target where a center click is harmless, or pass an explicit pid.

#[cfg(target_os = "windows")]
fn main() {
    probe::run();
}

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("zdrop_probe is Windows-only");
}

#[cfg(target_os = "windows")]
mod probe {
    use std::thread;
    use std::time::{Duration, Instant};

    use windows::Win32::Foundation::{BOOL, HANDLE, HWND, LPARAM, POINT, RECT, TRUE};
    use windows::Win32::UI::HiDpi::{
        SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    };
    use windows::Win32::UI::Input::Pointer::{
        InitializeTouchInjection, InjectTouchInput, POINTER_FLAG_DOWN, POINTER_FLAG_INCONTACT,
        POINTER_FLAG_INRANGE, POINTER_FLAG_UP, POINTER_INFO, POINTER_TOUCH_INFO,
        TOUCH_FEEDBACK_DEFAULT,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEINPUT, MOUSEEVENTF_LEFTDOWN,
        MOUSEEVENTF_LEFTUP,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClassNameW, GetCursorPos, GetForegroundWindow, GetWindow, GetWindowRect,
        GetWindowTextW, GetWindowThreadProcessId, GetTopWindow, IsWindowVisible, SetCursorPos,
        SetForegroundWindow, GW_HWNDNEXT, PT_TOUCH,
    };

    // ---- z-order helpers ----------------------------------------------------

    /// Walk the top-level z-order from the top; return true if `a` is above `b`.
    /// None if either isn't found in the walk.
    unsafe fn is_above(a: HWND, b: HWND) -> Option<bool> {
        let mut h = GetTopWindow(None).ok()?;
        loop {
            if h == a {
                return Some(true);
            }
            if h == b {
                return Some(false);
            }
            match GetWindow(h, GW_HWNDNEXT) {
                Ok(n) if !n.0.is_null() => h = n,
                _ => return None,
            }
        }
    }

    struct PollResult {
        samples: u64,
        target_above: u64,
        fg_not_baseline: u64,
    }

    /// Sample the z-order for `dur`, counting flashes of `target` over `baseline`.
    fn poll(baseline: usize, target: usize, dur: Duration) -> PollResult {
        let baseline = HWND(baseline as *mut _);
        let target = HWND(target as *mut _);
        let deadline = Instant::now() + dur;
        let mut r = PollResult { samples: 0, target_above: 0, fg_not_baseline: 0 };
        while Instant::now() < deadline {
            unsafe {
                r.samples += 1;
                if GetForegroundWindow() != baseline {
                    r.fg_not_baseline += 1;
                }
                if is_above(target, baseline) == Some(true) {
                    r.target_above += 1;
                }
            }
            thread::sleep(Duration::from_millis(4));
        }
        r
    }

    // ---- actuators ----------------------------------------------------------

    /// Control: the existing flash path — SetForegroundWindow swap + SendInput.
    fn actuate_sendinput_swap(target: usize, x: i32, y: i32) {
        let target = HWND(target as *mut _);
        unsafe {
            let prev_fg = GetForegroundWindow();
            let mut prev_cursor = POINT::default();
            let _ = GetCursorPos(&mut prev_cursor);
            let _ = SetForegroundWindow(target);
            thread::sleep(Duration::from_millis(8));
            let _ = SetCursorPos(x, y);
            let down = INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT { dwFlags: MOUSEEVENTF_LEFTDOWN, ..Default::default() },
                },
            };
            let up = INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT { dwFlags: MOUSEEVENTF_LEFTUP, ..Default::default() },
                },
            };
            let events = [down, up];
            SendInput(&events, std::mem::size_of::<INPUT>() as i32);
            thread::sleep(Duration::from_millis(40));
            let _ = SetCursorPos(prev_cursor.x, prev_cursor.y);
            if !prev_fg.0.is_null() && prev_fg != target {
                let _ = SetForegroundWindow(prev_fg);
            }
        }
    }

    /// Track A: coordinate-routed touch injection. No SetForegroundWindow.
    /// `hwndTarget` is left NULL so the system hit-tests by screen coordinate.
    fn actuate_touch_inject(x: i32, y: i32) -> Result<(), String> {
        unsafe {
            // Per-process injection enable (idempotent; ignore "already init").
            let _ = InitializeTouchInjection(1, TOUCH_FEEDBACK_DEFAULT);
            let mk = |flags| POINTER_TOUCH_INFO {
                pointerInfo: POINTER_INFO {
                    pointerType: PT_TOUCH,
                    pointerId: 0,
                    pointerFlags: flags,
                    sourceDevice: HANDLE::default(),
                    hwndTarget: HWND::default(),
                    ptPixelLocation: POINT { x, y },
                    ..Default::default()
                },
                touchFlags: 0,
                touchMask: 0,
                rcContact: RECT { left: x - 2, top: y - 2, right: x + 2, bottom: y + 2 },
                rcContactRaw: RECT { left: x - 2, top: y - 2, right: x + 2, bottom: y + 2 },
                orientation: 0,
                pressure: 512,
            };
            let down = mk(POINTER_FLAG_DOWN | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT);
            InjectTouchInput(&[down]).map_err(|e| format!("InjectTouchInput(down): {e}"))?;
            thread::sleep(Duration::from_millis(30));
            let up = mk(POINTER_FLAG_UP);
            InjectTouchInput(&[up]).map_err(|e| format!("InjectTouchInput(up): {e}"))?;
            Ok(())
        }
    }

    // ---- window discovery ----------------------------------------------------

    unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let v = &mut *(lparam.0 as *mut Vec<HWND>);
        if IsWindowVisible(hwnd).as_bool() {
            v.push(hwnd);
        }
        TRUE
    }

    fn class_of(h: HWND) -> String {
        let mut buf = [0u16; 128];
        let n = unsafe { GetClassNameW(h, &mut buf) };
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }
    fn title_of(h: HWND) -> String {
        let mut buf = [0u16; 256];
        let n = unsafe { GetWindowTextW(h, &mut buf) };
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }
    fn pid_of(h: HWND) -> u32 {
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
        pid
    }

    fn center(h: HWND) -> Option<(i32, i32)> {
        let mut r = RECT::default();
        unsafe { GetWindowRect(h, &mut r).ok()? };
        if r.right <= r.left || r.bottom <= r.top {
            return None;
        }
        Some(((r.left + r.right) / 2, (r.top + r.bottom) / 2))
    }

    fn run_trial(name: &str, baseline: HWND, target: HWND, x: i32, y: i32, actuate: impl FnOnce()) {
        let b = baseline.0 as usize;
        let t = target.0 as usize;
        let poller = thread::spawn(move || poll(b, t, Duration::from_millis(1200)));
        thread::sleep(Duration::from_millis(120)); // let poller establish "baseline on top"
        let _ = (x, y);
        actuate();
        let r = poller.join().unwrap();
        let pct = |n: u64| if r.samples == 0 { 0.0 } else { 100.0 * n as f64 / r.samples as f64 };
        println!(
            "  [{name:8}] samples={:4}  target_above_baseline={:4} ({:5.1}%)  fg!=baseline={:4} ({:5.1}%)",
            r.samples, r.target_above, pct(r.target_above), r.fg_not_baseline, pct(r.fg_not_baseline)
        );
    }

    pub fn run() {
        unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
        let args: Vec<String> = std::env::args().skip(1).collect();
        let mode = args.get(0).map(|s| s.as_str()).unwrap_or("both");
        let forced_pid: Option<u32> = args.get(1).and_then(|s| s.parse().ok());

        let mut wins: Vec<HWND> = Vec::new();
        unsafe {
            let _ = EnumWindows(Some(collect), LPARAM(&mut wins as *mut _ as isize));
        }
        let fg = unsafe { GetForegroundWindow() };

        // Candidate = visible, titled, not the current foreground, not shell.
        let preferred = ["chrome", "msedge", "firefox", "notepad", "wpf", "soffice"];
        let candidates: Vec<HWND> = wins
            .iter()
            .copied()
            .filter(|&h| h != fg && !title_of(h).is_empty())
            .filter(|&h| {
                let c = class_of(h).to_lowercase();
                !c.contains("progman") && !c.contains("workerw") && !c.contains("shell_traywnd")
            })
            .collect();

        if mode == "list" || candidates.is_empty() {
            println!("Foreground (baseline): pid={} class={:?} title={:?}", pid_of(fg), class_of(fg), title_of(fg));
            println!("Candidate background windows:");
            for h in &candidates {
                println!("  pid={:6} class={:24} title={:?}", pid_of(*h), class_of(*h), title_of(*h));
            }
            if candidates.is_empty() {
                eprintln!("\nNo background candidate windows. Open one and focus a different window.");
            }
            if mode == "list" {
                return;
            }
        }

        let target = match forced_pid {
            Some(pid) => candidates.iter().copied().find(|&h| pid_of(h) == pid),
            None => candidates
                .iter()
                .copied()
                .find(|&h| {
                    let t = title_of(h).to_lowercase();
                    let c = class_of(h).to_lowercase();
                    preferred.iter().any(|p| t.contains(p) || c.contains(p))
                })
                .or_else(|| candidates.first().copied()),
        };
        let Some(target) = target else {
            eprintln!("No target window selected.");
            return;
        };

        let Some((x, y)) = center(target) else {
            eprintln!("Target has no usable rect.");
            return;
        };

        println!("baseline (user fg): pid={} title={:?}", pid_of(fg), title_of(fg));
        println!(
            "target (background): pid={} class={:?} title={:?}  click@({x},{y})",
            pid_of(target), class_of(target), title_of(target)
        );
        println!("Position your windows now — actuating in 2s. (target should stay BEHIND baseline)");
        thread::sleep(Duration::from_secs(2));

        if mode == "both" || mode == "control" {
            run_trial("control", fg, target, x, y, || actuate_sendinput_swap(target.0 as usize, x, y));
            thread::sleep(Duration::from_millis(400));
        }
        if mode == "both" || mode == "inject" {
            // re-read baseline: control may have left fg elsewhere; refocus check
            let base2 = unsafe { GetForegroundWindow() };
            run_trial("inject", base2, target, x, y, || {
                if let Err(e) = actuate_touch_inject(x, y) {
                    eprintln!("  touch-inject error: {e}");
                }
            });
        }
        println!("\nInterpretation: control should show a high target_above%/fg!=baseline% (the flash).");
        println!("Track A (inject) showing ~0% target_above == background input with no visible raise.");
    }
}

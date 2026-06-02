//! Multi-cursor background computer-use demo orchestrator.
//!
//! Launches a "legacy form" app in five UI frameworks, arranged as a 2x2 grid
//! of background windows around one foreground "master" in the middle. When the
//! human clicks SUBMIT or types into the master, the master emits the action on
//! stdout; this orchestrator replays it onto all four background corners
//! simultaneously, each via its OWN cua-driver session (= its own uniquely
//! coloured agent cursor), entirely in the background — no window is raised and
//! the user's cursor never moves.
//!
//! Proves cua-driver drives every framework with OR without an accessibility
//! tree (the GDI corner has none; cua-driver's default dispatch falls back to
//! pixel/injection there, UIA-Invoke on the rest) — all concurrently.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Sender};
use std::thread;
use std::time::{Duration, Instant};

use windows::core::PWSTR;
use windows::Win32::Foundation::{BOOL, HANDLE, HWND, LPARAM, POINT, RECT, TRUE};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, SetInformationJobObject,
    JobObjectExtendedLimitInformation, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClientRect, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
    SetForegroundWindow, SetWindowPos, HWND_TOP, SWP_NOACTIVATE, SWP_SHOWWINDOW,
    SWP_NOZORDER,
};

// ── kill-on-exit job: everything we spawn dies when the orchestrator exits ────
static JOB: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
fn job() -> HANDLE {
    let raw = *JOB.get_or_init(|| unsafe {
        let h = CreateJobObjectW(None, windows::core::PCWSTR::null()).expect("CreateJobObjectW");
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let _ = SetInformationJobObject(h, JobObjectExtendedLimitInformation,
            &info as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32);
        h.0 as usize
    });
    HANDLE(raw as *mut core::ffi::c_void)
}
fn assign_to_job(child: &Child) {
    use std::os::windows::io::AsRawHandle;
    unsafe {
        let h = HANDLE(child.as_raw_handle() as *mut core::ffi::c_void);
        let _ = AssignProcessToJobObject(job(), h);
    }
}

const CLIENT_W: f64 = 480.0;
const CLIENT_H: f64 = 300.0;
// Outer window size (client + non-client frame for a fixed caption window).
const WIN_W: i32 = 496;
const WIN_H: i32 = 338;

struct Corner {
    title: &'static str,   // unique substring to find the window
    session: &'static str, // palette name -> cursor color
    hwnd: HWND,
    pid: u32,
}

fn main() {
    let demo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
    let repo_root = demo_root.parent().unwrap().parent().unwrap().to_path_buf();

    let cua_driver = std::env::var("CUA_DRIVER_EXE").map(PathBuf::from).unwrap_or_else(|_| {
        repo_root.join("libs/cua-driver/rust/target/debug/cua-driver.exe")
    });
    let legacy_app = std::env::var("LEGACY_APP_EXE").map(PathBuf::from).unwrap_or_else(|_| {
        demo_root.join("target/debug/legacy-app.exe")
    });
    if !cua_driver.exists() { eprintln!("cua-driver.exe not found at {cua_driver:?}; set CUA_DRIVER_EXE"); std::process::exit(1); }
    if !legacy_app.exists() { eprintln!("legacy-app.exe not found at {legacy_app:?}; build it first"); std::process::exit(1); }

    // 1. Start the persistent daemon (hosts the cursor overlay + sessions).
    eprintln!("[orch] starting cua-driver daemon…");
    let mut daemon = Command::new(&cua_driver).arg("serve")
        .stdout(Stdio::null()).stderr(Stdio::null())
        .spawn().expect("spawn cua-driver serve");
    assign_to_job(&daemon);
    thread::sleep(Duration::from_millis(1500));

    // 2. Launch the four corner apps (each a different framework) + the master.
    let mut kids: Vec<Child> = Vec::new();
    let launch = |kids: &mut Vec<Child>, cmd: &mut Command| {
        match cmd.stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
            Ok(c) => { assign_to_job(&c); kids.push(c); }
            Err(e) => eprintln!("[orch] launch failed: {e}"),
        }
    };

    // GDI corner (Rust, NO a11y tree).
    launch(&mut kids, Command::new(&legacy_app).args(["gdi", "Win32 GDI (no a11y)"]));

    // WinForms (.NET classic controls).
    let winforms = std::env::var("WINFORMS_EXE").map(PathBuf::from).unwrap_or_else(|_| {
        demo_root.join("dotnet/winforms/bin/Debug/net10.0-windows/winforms-legacy.exe")
    });
    if winforms.exists() { launch(&mut kids, &mut Command::new(&winforms)); }
    else { eprintln!("[orch] (skipping) winforms exe missing: {winforms:?}"); }

    // WPF (.NET XAML/UIA).
    let wpf = std::env::var("WPF_EXE").map(PathBuf::from).unwrap_or_else(|_| {
        demo_root.join("dotnet/wpf/bin/Debug/net10.0-windows/wpf-legacy.exe")
    });
    if wpf.exists() { launch(&mut kids, &mut Command::new(&wpf)); }
    else { eprintln!("[orch] (skipping) wpf exe missing: {wpf:?}"); }

    // Electron (Chromium) via the locally-installed electron binary.
    let electron_dir = std::env::var("ELECTRON_DIR").map(PathBuf::from).unwrap_or_else(|_| demo_root.join("electron"));
    let electron_bin = electron_dir.join("node_modules/.bin/electron.cmd");
    if electron_bin.exists() {
        let mut c = Command::new(&electron_bin);
        c.arg(".").current_dir(&electron_dir);
        launch(&mut kids, &mut c);
    } else { eprintln!("[orch] (skipping) electron not installed at {electron_bin:?} (run npm install)"); }

    // Master (foreground, instrumented) — stdout piped so we can read events.
    let mut master = Command::new(&legacy_app).args(["master", "Master (Win32 controls)"])
        .stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn master");
    assign_to_job(&master);
    let master_out = master.stdout.take().unwrap();

    // 3. Find + place windows. Corners get colors; master goes center foreground.
    thread::sleep(Duration::from_millis(2500)); // app windows + electron warmup
    let mut corners = vec![
        Corner { title: "Win32 GDI", session: "crimson",   hwnd: HWND::default(), pid: 0 },
        Corner { title: "WinForms",  session: "amber",     hwnd: HWND::default(), pid: 0 },
        Corner { title: "WPF",       session: "aqua",      hwnd: HWND::default(), pid: 0 },
        Corner { title: "Electron",  session: "mint_lime", hwnd: HWND::default(), pid: 0 },
    ];
    for c in corners.iter_mut() {
        if let Some((h, pid)) = find_window_by_title(c.title) { c.hwnd = h; c.pid = pid; }
        else { eprintln!("[orch] (skipping) no window found for '{}'", c.title); }
    }
    corners.retain(|c| !c.hwnd.0.is_null());
    let master_hwnd = find_window_by_title("Master (Win32").map(|(h, _)| h);

    let (sw, sh) = screen_size();
    // 2x2 corners + center.
    let m = ((sw - WIN_W) / 2, (sh - WIN_H) / 2);
    let pad_x = (sw / 12).max(20);
    let pad_y = (sh / 12).max(20);
    let positions = [
        (pad_x, pad_y),                       // TL
        (sw - WIN_W - pad_x, pad_y),           // TR
        (pad_x, sh - WIN_H - pad_y),           // BL
        (sw - WIN_W - pad_x, sh - WIN_H - pad_y), // BR
    ];
    for (i, c) in corners.iter().enumerate() {
        let (x, y) = positions[i % 4];
        place(c.hwnd, x, y, false);
    }
    if let Some(mh) = master_hwnd {
        place(mh, m.0, m.1, true);
        unsafe { let _ = SetForegroundWindow(mh); }
    }

    // 4. Pre-arm a coloured cursor per session (lazy-create + enable).
    for c in &corners {
        let _ = run_call(&cua_driver, "set_agent_cursor_enabled",
            &format!(r#"{{"enabled":true,"session":"{}"}}"#, c.session));
    }

    // 5. Spawn one driver thread per corner; fan out master events to all.
    let mut senders: Vec<Sender<Action>> = Vec::new();
    let mut handles = Vec::new();
    for c in &corners {
        let (tx, rx) = channel::<Action>();
        senders.push(tx);
        let cua = cua_driver.clone();
        let (pid, hwnd_addr, session) = (c.pid, c.hwnd.0 as isize, c.session.to_string());
        handles.push(thread::spawn(move || {
            let hwnd = HWND(hwnd_addr as *mut _);
            for act in rx {
                match act {
                    Action::Click { rx, ry } => {
                        if let Some((x, y)) = client_rel_to_local_px(hwnd, rx, ry) {
                            let ok = run_call(&cua, "click", &format!(
                                r#"{{"pid":{pid},"window_id":{hwnd_addr},"x":{x},"y":{y},"session":"{session}"}}"#));
                            eprintln!("[drive {session}] click ({x},{y}) -> {}", if ok { "ok" } else { "FAIL" });
                        }
                    }
                    Action::Type { text } => {
                        // Focus the field first (so the chars land), then type.
                        if let Some((x, y)) = client_rel_to_local_px(hwnd, 0.5, 0.28) {
                            let _ = run_call(&cua, "click", &format!(
                                r#"{{"pid":{pid},"window_id":{hwnd_addr},"x":{x},"y":{y},"session":"{session}"}}"#));
                        }
                        let esc = json_escape(&text);
                        let ok = run_call(&cua, "type_text", &format!(
                            r#"{{"pid":{pid},"window_id":{hwnd_addr},"text":"{esc}","session":"{session}"}}"#));
                        eprintln!("[drive {session}] type {text:?} -> {}", if ok { "ok" } else { "FAIL" });
                    }
                }
            }
        }));
    }

    eprintln!("[orch] ready — click SUBMIT or type+SUBMIT in the center window; \
               watch {} coloured cursors drive the corners in the background.", corners.len());

    // Optional self-playing mode for unattended demo/verification: emit a
    // TYPE then CLICK every few seconds, fanning out to all corners.
    if std::env::args().any(|a| a == "--auto") {
        let s2 = senders.clone();
        thread::spawn(move || {
            for i in 1..=3 {
                thread::sleep(Duration::from_secs(3));
                eprintln!("[orch] AUTO {i}: TYPE then CLICK");
                for tx in &s2 { let _ = tx.send(Action::Type { text: format!("auto {i}") }); }
                thread::sleep(Duration::from_millis(1800));
                for tx in &s2 { let _ = tx.send(Action::Click { rx: 0.5, ry: 0.577 }); }
            }
        });
    }

    // 6. Read master events; fan out to all corner threads concurrently.
    let reader = BufReader::new(master_out);
    for line in reader.lines().map_while(Result::ok) {
        let parts: Vec<&str> = line.trim().split('\t').collect();
        let action = match parts.as_slice() {
            ["CLICK", rx, ry] => rx.parse::<f64>().ok().zip(ry.parse::<f64>().ok())
                .map(|(rx, ry)| Action::Click { rx, ry }),
            ["TYPE", text] => Some(Action::Type { text: (*text).to_string() }),
            _ => None,
        };
        if let Some(a) = action {
            eprintln!("[orch] user action: {a:?} -> driving {} corners", senders.len());
            for tx in &senders { let _ = tx.send(a.clone()); }
        }
    }

    // Master exited -> tear everything down.
    drop(senders);
    for h in handles { let _ = h.join(); }
    let _ = master.kill();
    for mut k in kids { let _ = k.kill(); }
    let _ = daemon.kill();
}

#[derive(Clone, Debug)]
enum Action {
    Click { rx: f64, ry: f64 },
    Type { text: String },
}

fn json_escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => {}
            '\t' => o.push_str("\\t"),
            c => o.push(c),
        }
    }
    o
}

/// Run `cua-driver call <tool> <json>` (proxies to the running daemon).
fn run_call(cua: &PathBuf, tool: &str, json: &str) -> bool {
    Command::new(cua)
        .arg("call").arg(tool).arg(json)
        .stdout(Stdio::null()).stderr(Stdio::null())
        .status().map(|s| s.success()).unwrap_or(false)
}

fn screen_size() -> (i32, i32) {
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
    unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) }
}

fn place(hwnd: HWND, x: i32, y: i32, activate: bool) {
    if hwnd.0.is_null() { return; }
    let flags = if activate { SWP_SHOWWINDOW | SWP_NOZORDER } else { SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_NOZORDER };
    unsafe { let _ = SetWindowPos(hwnd, HWND_TOP, x, y, WIN_W, WIN_H, flags); }
}

/// Convert a client-relative point (0..1) to the click tool's window-local
/// screenshot-pixel space: ClientToScreen, then subtract the DWM extended
/// frame top-left + the 1px capture inset (mirrors `bitmap_to_screen`).
fn client_rel_to_local_px(hwnd: HWND, rx: f64, ry: f64) -> Option<(i32, i32)> {
    unsafe {
        let mut cr = RECT::default();
        GetClientRect(hwnd, &mut cr).ok()?;
        let cw = (cr.right - cr.left) as f64;
        let ch = (cr.bottom - cr.top) as f64;
        let mut pt = POINT { x: (rx * cw) as i32, y: (ry * ch) as i32 };
        let _ = ClientToScreen(hwnd, &mut pt);
        let mut dwm = RECT::default();
        if DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut dwm as *mut _ as *mut core::ffi::c_void,
            std::mem::size_of::<RECT>() as u32).is_err()
        {
            return Some((pt.x, pt.y));
        }
        Some((pt.x - dwm.left - 1, pt.y - dwm.top - 1))
    }
}

// ── window discovery by title substring ───────────────────────────────────────

struct Finder { needle: String, hwnd: HWND, pid: u32 }

unsafe extern "system" fn enum_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let f = &mut *(lparam.0 as *mut Finder);
    if !IsWindowVisible(hwnd).as_bool() { return TRUE; }
    let mut buf = [0u16; 256];
    let n = GetWindowTextW(hwnd, &mut buf);
    if n > 0 {
        let title = String::from_utf16_lossy(&buf[..n as usize]);
        if title.contains(&f.needle) {
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            f.hwnd = hwnd;
            f.pid = pid;
            return BOOL(0); // stop
        }
    }
    let _ = PWSTR::null();
    TRUE
}

fn find_window_by_title(needle: &str) -> Option<(HWND, u32)> {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let mut f = Finder { needle: needle.to_string(), hwnd: HWND::default(), pid: 0 };
        unsafe { let _ = EnumWindows(Some(enum_cb), LPARAM(&mut f as *mut _ as isize)); }
        if !f.hwnd.0.is_null() { return Some((f.hwnd, f.pid)); }
        if Instant::now() > deadline { return None; }
        thread::sleep(Duration::from_millis(300));
    }
}

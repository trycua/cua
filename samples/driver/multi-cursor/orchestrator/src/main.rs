//! Multi-cursor background computer-use demo orchestrator.
//!
//! Launches the "National Records System" terminal in five UI frameworks: a 2x2
//! grid of background windows (each = half the work area) around one foreground
//! "master" in the middle that overlaps all four. When the human submits a
//! record in the master, the master emits the action on stdout; this
//! orchestrator replays it onto all four background corners concurrently, each
//! via its OWN cua-driver session (= its own uniquely-coloured agent cursor),
//! in the background — no window raised, the user's cursor never moved.
//!
//! Driving is element-based where an accessibility tree exists (set_value for
//! the SUBJECT-NAME field + UIA-Invoke for SUBMIT — no foreground steal, no
//! SendInput) and pixel-based for the GDI corner that has no a11y tree. Each
//! a11y corner is VERIFIED by reading the records grid back: if the typed
//! record didn't land, it's logged as FAIL.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Sender};
use std::thread;
use std::time::{Duration, Instant};

use windows::core::PWSTR;
use windows::Win32::Foundation::{BOOL, HANDLE, HWND, LPARAM, POINT, RECT, TRUE};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, SetInformationJobObject,
    JobObjectExtendedLimitInformation, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClientRect, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
    SetForegroundWindow, SetWindowPos, HWND_TOP, SWP_NOACTIVATE, SWP_NOZORDER, SWP_SHOWWINDOW,
};

const FIELD_FRAC: (f64, f64) = (0.28, 0.145); // Account-Name field (GDI pixel fallback)

// ── maze (line tool) ───────────────────────────────────────────────────────────
// A spiral maze as straight segments in [0,1] of the drawing REGION. cua-driver
// draws each as one press-drag-release (no curves = "line tool only"). The same
// segments rasterized are the reference the screenshot is diffed against.
const MAZE: [(f64, f64, f64, f64); 7] = [
    (0.10, 0.12, 0.90, 0.12),
    (0.90, 0.12, 0.90, 0.88),
    (0.90, 0.88, 0.26, 0.88),
    (0.26, 0.88, 0.26, 0.40),
    (0.26, 0.40, 0.66, 0.40),
    (0.66, 0.40, 0.66, 0.64),
    (0.66, 0.64, 0.46, 0.64),
];
// Client-fraction rect of the window that sits INSIDE every framework's drawing
// pad (all four put the doodle top-right). cua-driver draws here and the
// verifier crops the same rect — so the rasterized reference aligns regardless
// of where each framework actually lays its canvas out. Tuned via screenshots.
// Kept inside the SHORTEST drawing pad: WPF/Electron host theirs in a fixed
// ~150px row, so the usable band ends well above the spreadsheet below it.
// Overshooting here drops the lower maze onto the grid (and selects rows).
const REGION: [f64; 4] = [0.470, 0.180, 0.900, 0.295]; // l, t, r, b (client fractions)
// A drawn maze must score at least this (F1 of ink overlap vs the reference,
// dilated) to count as COMMITTED; below it = FAIL.
const MAZE_PASS: f64 = 0.45;

// ── kill-on-exit job ──────────────────────────────────────────────────────────
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
    unsafe { let _ = AssignProcessToJobObject(job(), HANDLE(child.as_raw_handle() as *mut core::ffi::c_void)); }
}

struct Corner {
    title: &'static str,    // unique substring to find the window
    session: &'static str,  // palette name -> cursor color
    hwnd: HWND,
    pid: u32,
    field_idx: Option<u64>, // UIA element for the SUBJECT-NAME field (a11y corners)
    submit_idx: Option<u64>,// UIA element for the SUBMIT button
}

fn main() {
    let demo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
    let repo_root = demo_root.parent().unwrap().parent().unwrap().to_path_buf();

    let cua = std::env::var("CUA_DRIVER_EXE").map(PathBuf::from)
        .unwrap_or_else(|_| repo_root.join("libs/cua-driver/rust/target/debug/cua-driver.exe"));
    let legacy_app = std::env::var("LEGACY_APP_EXE").map(PathBuf::from)
        .unwrap_or_else(|_| demo_root.join("target/debug/legacy-app.exe"));
    if !cua.exists() { eprintln!("cua-driver.exe not found at {cua:?}"); std::process::exit(1); }
    if !legacy_app.exists() { eprintln!("legacy-app.exe not found at {legacy_app:?}"); std::process::exit(1); }

    eprintln!("[orch] starting cua-driver daemon…");
    let mut daemon = Command::new(&cua).arg("serve").stdout(Stdio::null()).stderr(Stdio::null())
        .spawn().expect("spawn cua-driver serve");
    assign_to_job(&daemon);
    thread::sleep(Duration::from_millis(1500));

    // Launch corners + master.
    let mut kids: Vec<Child> = Vec::new();
    let mut launch = |kids: &mut Vec<Child>, cmd: &mut Command| {
        if let Ok(c) = cmd.stdout(Stdio::null()).stderr(Stdio::null()).spawn() { assign_to_job(&c); kids.push(c); }
    };
    launch(&mut kids, Command::new(&legacy_app).args(["gdi", "Win32 GDI (no a11y)"]));
    let winforms = demo_root.join("dotnet/winforms/bin/Debug/net10.0-windows/winforms-legacy.exe");
    if winforms.exists() { launch(&mut kids, &mut Command::new(&winforms)); } else { eprintln!("[orch] (skip) winforms missing"); }
    let wpf = demo_root.join("dotnet/wpf/bin/Debug/net10.0-windows/wpf-legacy.exe");
    if wpf.exists() { launch(&mut kids, &mut Command::new(&wpf)); } else { eprintln!("[orch] (skip) wpf missing"); }
    let electron_bin = demo_root.join("electron/node_modules/.bin/electron.cmd");
    if electron_bin.exists() {
        let mut c = Command::new(&electron_bin); c.arg(".").current_dir(demo_root.join("electron"));
        launch(&mut kids, &mut c);
    } else { eprintln!("[orch] (skip) electron not installed"); }

    let mut master = Command::new(&legacy_app).args(["master", "Master (Win32 controls)"])
        .stdout(Stdio::piped()).stderr(Stdio::null()).spawn().expect("spawn master");
    assign_to_job(&master);
    let master_out = master.stdout.take().unwrap();

    thread::sleep(Duration::from_secs(3)); // window + electron warmup

    let mut corners = vec![
        Corner { title: "Win32 GDI", session: "crimson",   hwnd: HWND::default(), pid: 0, field_idx: None, submit_idx: None },
        Corner { title: "WinForms",  session: "amber",     hwnd: HWND::default(), pid: 0, field_idx: None, submit_idx: None },
        Corner { title: "WPF",       session: "aqua",      hwnd: HWND::default(), pid: 0, field_idx: None, submit_idx: None },
        Corner { title: "Electron",  session: "mint_lime", hwnd: HWND::default(), pid: 0, field_idx: None, submit_idx: None },
    ];
    for c in corners.iter_mut() {
        if let Some((h, pid)) = find_window_by_title(c.title) { c.hwnd = h; c.pid = pid; }
        else { eprintln!("[orch] (skip) no window for '{}'", c.title); }
    }
    corners.retain(|c| !c.hwnd.0.is_null());
    let master_hwnd = find_window_by_title("Master (Win32").map(|(h, _)| h);

    // Layout: each window half the work area; corners tile quadrants, master centered on top.
    let wa = work_area();
    let ww = (wa.right - wa.left) / 2;
    let wh = (wa.bottom - wa.top) / 2;
    let positions = [(wa.left, wa.top), (wa.left + ww, wa.top), (wa.left, wa.top + wh), (wa.left + ww, wa.top + wh)];
    for (i, c) in corners.iter().enumerate() { let (x, y) = positions[i % 4]; place(c.hwnd, x, y, ww, wh, false); }
    if let Some(mh) = master_hwnd {
        // Master is smaller than a quadrant so it overlaps the central seam of
        // all four corners without hiding their forms (which sit at the
        // quadrant centers, outside this box). Centered on the work area.
        let (mw, mh2) = (ww / 2, wh / 2);
        let (mx, my) = (wa.left + ww - mw / 2, wa.top + wh - mh2 / 2);
        place(mh, mx, my, mw, mh2, true);
        unsafe { let _ = SetForegroundWindow(mh); }
    }
    thread::sleep(Duration::from_millis(800)); // let resized layouts settle

    // Discover UIA elements per corner (field + SUBMIT). GDI has none -> pixel.
    for c in corners.iter_mut() {
        let tree = run_call_out(&cua, "get_window_state",
            &format!(r#"{{"pid":{},"window_id":{},"capture_mode":"ax","session":"{}"}}"#, c.pid, c.hwnd.0 as isize, c.session));
        c.field_idx = find_idx(&tree, |line| line.contains("] Edit"));
        c.submit_idx = find_idx(&tree, |line| line.contains("] Button") && line.to_uppercase().contains("ADD RECORD"));
        eprintln!("[orch] {:<9} session={:<9} field={:?} submit={:?} {}",
            c.title, c.session, c.field_idx, c.submit_idx,
            if c.field_idx.is_some() { "(a11y: set_value+invoke)" } else { "(no a11y: pixel)" });
        // Pre-arm a coloured cursor for this session.
        let _ = run_call(&cua, "set_agent_cursor_enabled", &format!(r#"{{"enabled":true,"session":"{}"}}"#, c.session));
    }

    // One driver thread per corner.
    let mut senders: Vec<Sender<Action>> = Vec::new();
    let mut handles = Vec::new();
    for c in &corners {
        let (tx, rx) = channel::<Action>();
        senders.push(tx);
        let cua = cua.clone();
        let pid = c.pid;
        let hwnd_addr = c.hwnd.0 as isize;
        let session = c.session.to_string();
        let title = c.title.to_string();
        handles.push(thread::spawn(move || {
            let hwnd = HWND(hwnd_addr as *mut _);
            let ax = format!(r#"{{"pid":{pid},"window_id":{hwnd_addr},"capture_mode":"ax","session":"{session}"}}"#);
            let mut seen_records: i64 = -1;
            for act in rx {
                // Re-discover element indices on a FRESH snapshot each action —
                // a11y vs pixel is decided per action (Chromium's tree appears
                // late and re-numbers as rows are added; GDI never has a tree).
                let tree = run_call_out(&cua, "get_window_state", &ax);
                let field = find_idx(&tree, |l| l.contains("] Edit"));
                let submit = find_idx(&tree, |l| l.contains("] Button") && l.to_uppercase().contains("ADD RECORD"));
                match act {
                    Action::Type { text } => {
                        let ok = match field {
                            // a11y: plain type_text with element_index — cua-driver
                            // auto-routes to UIA ValuePattern.SetValue on its own.
                            Some(idx) => run_call(&cua, "type_text", &format!(
                                r#"{{"pid":{pid},"window_id":{hwnd_addr},"element_index":{idx},"text":"{}","session":"{session}"}}"#, json_escape(&text))),
                            // no a11y (GDI): focus the field by pixel, then WM_CHAR.
                            None => {
                                if let Some((x, y)) = client_rel_to_local_px(hwnd, FIELD_FRAC.0, FIELD_FRAC.1) {
                                    let _ = run_call(&cua, "click", &format!(
                                        r#"{{"pid":{pid},"window_id":{hwnd_addr},"x":{x},"y":{y},"session":"{session}"}}"#));
                                }
                                run_call(&cua, "type_text", &format!(
                                    r#"{{"pid":{pid},"window_id":{hwnd_addr},"text":"{}","session":"{session}"}}"#, json_escape(&text)))
                            }
                        };
                        eprintln!("[drive {session}] type {text:?} -> {}", if ok { "ok" } else { "FAIL" });
                    }
                    Action::Click { rx, ry } => {
                        let ok = match submit {
                            Some(idx) => run_call(&cua, "click", &format!(
                                r#"{{"pid":{pid},"window_id":{hwnd_addr},"element_index":{idx},"session":"{session}"}}"#)),
                            None => {
                                if let Some((x, y)) = client_rel_to_local_px(hwnd, rx, ry) {
                                    run_call(&cua, "click", &format!(
                                        r#"{{"pid":{pid},"window_id":{hwnd_addr},"x":{x},"y":{y},"session":"{session}"}}"#))
                                } else { false }
                            }
                        };
                        eprintln!("[drive {session}] click Add Record -> {}", if ok { "ok" } else { "FAIL" });

                        // Verify a record was actually COMMITTED: the "Records: N"
                        // counter must increment. (Checking for the typed text
                        // alone is fooled by it sitting in the field.) Only the
                        // a11y corners expose the counter; GDI's is drawn pixels,
                        // so it's verified visually (its grid grows on screen).
                        thread::sleep(Duration::from_millis(300));
                        let t2 = run_call_out(&cua, "get_window_state", &ax);
                        if let Some(n) = records_count(&t2) {
                            let landed = n as i64 > seen_records;
                            seen_records = n as i64;
                            eprintln!("[verify {session}] {title} Records={n} -> {}",
                                if landed { "COMMITTED ✓" } else { "FAIL (no new record)" });
                        }
                    }
                    Action::DrawMaze => {
                        // Pure coordinate drawing — NO element targeting, NO app
                        // cooperation. Each maze segment is one straight
                        // press-drag-release into the common REGION; cua-driver
                        // PostMessages the drag where it can and pen-injects it
                        // where the canvas (Chromium/WPF) drops posted mouse.
                        let map = |mx: f64, my: f64| {
                            let cfx = REGION[0] + mx * (REGION[2] - REGION[0]);
                            let cfy = REGION[1] + my * (REGION[3] - REGION[1]);
                            client_rel_to_local_px(hwnd, cfx, cfy)
                        };
                        let mut drawn = 0;
                        for (x0, y0, x1, y1) in MAZE.iter() {
                            if let (Some((fx, fy)), Some((tx, ty))) = (map(*x0, *y0), map(*x1, *y1)) {
                                if run_call(&cua, "drag", &format!(
                                    r#"{{"pid":{pid},"window_id":{hwnd_addr},"from_x":{fx},"from_y":{fy},"to_x":{tx},"to_y":{ty},"dispatch":"background","session":"{session}"}}"#)) {
                                    drawn += 1;
                                }
                            }
                            thread::sleep(Duration::from_millis(120));
                        }
                        eprintln!("[draw {session}] maze: {drawn}/{} segments", MAZE.len());

                        // Read back by SCREENSHOT (WGC captures the occluded
                        // window) and diff the rendered ink against the
                        // rasterized reference maze — normalized into the same
                        // REGION grid, pass only under threshold.
                        thread::sleep(Duration::from_millis(400));
                        let shot = std::env::temp_dir().join(format!("maze_{session}.png"));
                        let _ = std::fs::remove_file(&shot);
                        run_call_shot(&cua, &format!(
                            r#"{{"pid":{pid},"window_id":{hwnd_addr},"capture_mode":"vision","max_image_dimension":4096,"session":"{session}"}}"#), &shot);
                        match score_maze(hwnd, &shot) {
                            Some(score) => eprintln!("[verify {session}] {title} maze score={score:.2} -> {}",
                                if score >= MAZE_PASS { "DREW MAZE ✓" } else { "FAIL (lines don't match)" }),
                            None => eprintln!("[verify {session}] {title} maze -> FAIL (no screenshot/region)"),
                        }
                    }
                }
            }
        }));
    }

    eprintln!("[orch] ready — submit a record in the center MASTER; watch {} coloured cursors \
               drive the corner terminals in the background.", corners.len());

    if std::env::args().any(|a| a == "--auto") {
        let s2 = senders.clone();
        thread::spawn(move || {
            for i in 1..=3 {
                thread::sleep(Duration::from_secs(3));
                eprintln!("[orch] AUTO {i}: TYPE then CLICK");
                for tx in &s2 { let _ = tx.send(Action::Type { text: format!("SUBJECT-{i:03}") }); }
                thread::sleep(Duration::from_millis(1800));
                // SAVE/"Add Record" button center (matches the GDI/master layout)
                // for the pixel-driven GDI corner; a11y corners ignore rx/ry.
                for tx in &s2 { let _ = tx.send(Action::Click { rx: 0.22, ry: 0.495 }); }
            }
            // Finally: draw the same maze into every corner's drawing pad by
            // pure coordinate drags, then screenshot-diff each against the
            // reference. Proves canvas actuation with no a11y, no app help.
            thread::sleep(Duration::from_secs(2));
            eprintln!("[orch] AUTO 4: DRAW MAZE (line tool) + screenshot diff");
            for tx in &s2 { let _ = tx.send(Action::DrawMaze); }
        });
    }

    let reader = BufReader::new(master_out);
    for line in reader.lines().map_while(Result::ok) {
        let parts: Vec<&str> = line.trim().split('\t').collect();
        let action = match parts.as_slice() {
            ["CLICK", rx, ry] => rx.parse::<f64>().ok().zip(ry.parse::<f64>().ok()).map(|(rx, ry)| Action::Click { rx, ry }),
            ["TYPE", text] => Some(Action::Type { text: (*text).to_string() }),
            _ => None,
        };
        if let Some(a) = action {
            eprintln!("[orch] user action: {a:?} -> driving {} corners", senders.len());
            for tx in &senders { let _ = tx.send(a.clone()); }
        }
    }

    drop(senders);
    for h in handles { let _ = h.join(); }
    let _ = master.kill();
    for mut k in kids { let _ = k.kill(); }
    let _ = daemon.kill();
}

#[derive(Clone, Debug)]
enum Action { Click { rx: f64, ry: f64 }, Type { text: String }, DrawMaze }

fn json_escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch { '"' => o.push_str("\\\""), '\\' => o.push_str("\\\\"), '\n' => o.push_str("\\n"),
                   '\r' => {}, '\t' => o.push_str("\\t"), c => o.push(c) }
    }
    o
}

/// `cua-driver call <tool> <json>` (proxies to the daemon). True on success.
fn run_call(cua: &PathBuf, tool: &str, json: &str) -> bool {
    Command::new(cua).arg("call").arg(tool).arg(json)
        .stdout(Stdio::null()).stderr(Stdio::null())
        .status().map(|s| s.success()).unwrap_or(false)
}
/// Same, capturing stdout (for get_window_state readback).
fn run_call_out(cua: &PathBuf, tool: &str, json: &str) -> String {
    Command::new(cua).arg("call").arg(tool).arg(json)
        .stderr(Stdio::null()).output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
}
/// get_window_state writing the screenshot PNG to `out` (WGC; captures even a
/// fully occluded window). Used to read back what cua-driver drew.
fn run_call_shot(cua: &PathBuf, json: &str, out: &std::path::Path) -> bool {
    Command::new(cua).arg("call").arg("get_window_state").arg(json)
        .arg("--screenshot-out-file").arg(out)
        .stdout(Stdio::null()).stderr(Stdio::null())
        .status().map(|s| s.success()).unwrap_or(false)
}

/// DWM extended-frame size of `hwnd` — the pixel size of the WGC bitmap before
/// get_window_state downscales it to fit `max_image_dimension`.
fn window_frame_size(hwnd: HWND) -> Option<(i32, i32)> {
    unsafe {
        let mut d = RECT::default();
        if DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut d as *mut _ as *mut core::ffi::c_void, std::mem::size_of::<RECT>() as u32).is_err() {
            return None;
        }
        Some((d.right - d.left, d.bottom - d.top))
    }
}

/// Shape-only F1 (0..1) of the maze cua-driver drew vs the reference maze.
///
/// Reads the ink from the window screenshot (cropped to the drawing REGION),
/// then normalizes BOTH the drawn ink and the reference into a unit grid by
/// their own bounding boxes — so the score reflects the *shape* of the lines,
/// not where on the canvas they landed or at what scale (the user only wants
/// the shape penalized). The screenshot is downscaled to fit
/// `max_image_dimension`, so REGION (full-res bitmap px) is scaled by the
/// PNG/frame ratio before cropping.
fn score_maze(hwnd: HWND, png: &std::path::Path) -> Option<f64> {
    let img = image::open(png).ok()?.to_luma8();
    let (iw, ih) = (img.width() as i32, img.height() as i32);
    let (fw, fh) = window_frame_size(hwnd)?;
    if fw <= 0 || fh <= 0 { return None; }
    let (fx, fy) = (iw as f64 / fw as f64, ih as f64 / fh as f64);
    let (ax, ay) = client_rel_to_local_px(hwnd, REGION[0], REGION[1])?;
    let (bx, by) = client_rel_to_local_px(hwnd, REGION[2], REGION[3])?;
    let cx0 = (((ax.min(bx)) as f64 * fx).floor() as i32).max(0);
    let cy0 = (((ay.min(by)) as f64 * fy).floor() as i32).max(0);
    let cx1 = (((ax.max(bx)) as f64 * fx).ceil() as i32).min(iw - 1);
    let cy1 = (((ay.max(by)) as f64 * fy).ceil() as i32).min(ih - 1);
    if cx1 - cx0 < 12 || cy1 - cy0 < 12 { return None; }

    // Drawn ink + its bounding box.
    let mut ink: Vec<(i32, i32)> = Vec::new();
    let (mut nx, mut ny, mut xx, mut xy) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for y in cy0..=cy1 {
        for x in cx0..=cx1 {
            if img.get_pixel(x as u32, y as u32).0[0] < 150 {
                ink.push((x, y));
                if x < nx { nx = x } if x > xx { xx = x }
                if y < ny { ny = y } if y > xy { xy = y }
            }
        }
    }
    const N: usize = 96;
    if ink.len() < 20 || xx <= nx || xy <= ny { return None; }
    let (bw, bh) = ((xx - nx) as f64, (xy - ny) as f64);
    let mut a = vec![false; N * N];
    for (x, y) in ink {
        let gx = (((x - nx) as f64 / bw) * (N as f64 - 1.0)).round() as usize;
        let gy = (((y - ny) as f64 / bh) * (N as f64 - 1.0)).round() as usize;
        a[gy.min(N - 1) * N + gx.min(N - 1)] = true;
    }

    // Reference maze, normalized to ITS bounding box the same way.
    let (mut rnx, mut rny, mut rxx, mut rxy) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for (x0, y0, x1, y1) in MAZE.iter() {
        for (px, py) in [(*x0, *y0), (*x1, *y1)] {
            if px < rnx { rnx = px } if px > rxx { rxx = px }
            if py < rny { rny = py } if py > rxy { rxy = py }
        }
    }
    let (rbw, rbh) = ((rxx - rnx).max(1e-6), (rxy - rny).max(1e-6));
    let mut b = vec![false; N * N];
    for (x0, y0, x1, y1) in MAZE.iter() {
        raster_line(&mut b, N, (x0 - rnx) / rbw, (y0 - rny) / rbh, (x1 - rnx) / rbw, (y1 - rny) / rbh);
    }
    Some(mask_f1(&a, &b, N, 2))
}

/// Bresenham rasterize a normalized [0,1] segment into the N×N reference mask.
fn raster_line(mask: &mut [bool], n: usize, x0: f64, y0: f64, x1: f64, y1: f64) {
    let s = n as f64 - 1.0;
    let (px1, py1) = ((x1 * s) as i32, (y1 * s) as i32);
    let (mut x, mut y) = ((x0 * s) as i32, (y0 * s) as i32);
    let (dx, dy) = ((px1 - x).abs(), -(py1 - y).abs());
    let (sx, sy) = (if x < px1 { 1 } else { -1 }, if y < py1 { 1 } else { -1 });
    let mut err = dx + dy;
    loop {
        if x >= 0 && y >= 0 && (x as usize) < n && (y as usize) < n { mask[y as usize * n + x as usize] = true; }
        if x == px1 && y == py1 { break; }
        let e2 = 2 * err;
        if e2 >= dy { err += dy; x += sx; }
        if e2 <= dx { err += dx; y += sy; }
    }
}

/// Chebyshev dilation by radius `r`.
fn dilate(m: &[bool], n: usize, r: i32) -> Vec<bool> {
    let mut o = vec![false; n * n];
    for y in 0..n as i32 {
        for x in 0..n as i32 {
            if !m[y as usize * n + x as usize] { continue; }
            for dy in -r..=r {
                for dx in -r..=r {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx >= 0 && ny >= 0 && (nx as usize) < n && (ny as usize) < n { o[ny as usize * n + nx as usize] = true; }
                }
            }
        }
    }
    o
}

/// F1 of two ink masks, each dilated by `r` so near-misses count as matches.
fn mask_f1(a: &[bool], b: &[bool], n: usize, r: i32) -> f64 {
    let (da, db) = (dilate(a, n, r), dilate(b, n, r));
    let (mut ca, mut cb, mut ma, mut mb) = (0usize, 0usize, 0usize, 0usize);
    for i in 0..n * n {
        if a[i] { ca += 1; if db[i] { ma += 1; } }
        if b[i] { cb += 1; if da[i] { mb += 1; } }
    }
    if ca == 0 || cb == 0 { return 0.0; }
    let precision = ma as f64 / ca as f64; // drawn ink that is near the reference
    let recall = mb as f64 / cb as f64;     // reference covered by drawn ink
    if precision + recall == 0.0 { 0.0 } else { 2.0 * precision * recall / (precision + recall) }
}

/// First `[N]` element index on a tree line matching `pred`.
fn find_idx(tree: &str, pred: impl Fn(&str) -> bool) -> Option<u64> {
    // The tree is one JSON line with literal "\n" separators between rows.
    for line in tree.split("\\n") {
        if !pred(line) { continue; }
        let st = line.find('[')? + 1;
        let en = line[st..].find(']')? + st;
        if let Ok(n) = line[st..en].trim().parse() { return Some(n); }
    }
    None
}

/// Parse the "Records: N" counter from a window-state tree (status bar).
fn records_count(tree: &str) -> Option<u32> {
    let pos = tree.find("Records:")?;
    let rest = &tree[pos + "Records:".len()..];
    let digits: String = rest.trim_start().chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

fn work_area() -> RECT {
    use windows::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETWORKAREA, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS};
    let mut wa = RECT::default();
    unsafe { let _ = SystemParametersInfoW(SPI_GETWORKAREA, 0, Some(&mut wa as *mut _ as *mut core::ffi::c_void), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)); }
    if wa.right <= wa.left || wa.bottom <= wa.top {
        use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
        unsafe { wa = RECT { left: 0, top: 0, right: GetSystemMetrics(SM_CXSCREEN), bottom: GetSystemMetrics(SM_CYSCREEN) }; }
    }
    wa
}

fn place(hwnd: HWND, x: i32, y: i32, w: i32, h: i32, activate: bool) {
    if hwnd.0.is_null() { return; }
    let flags = if activate { SWP_SHOWWINDOW } else { SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_NOZORDER };
    unsafe { let _ = SetWindowPos(hwnd, HWND_TOP, x, y, w, h, flags); }
}

/// Client-relative (0..1) -> the click tool's window-local screenshot-pixel
/// space (ClientToScreen minus the DWM extended-frame top-left + 1px inset).
fn client_rel_to_local_px(hwnd: HWND, rx: f64, ry: f64) -> Option<(i32, i32)> {
    unsafe {
        let mut cr = RECT::default();
        GetClientRect(hwnd, &mut cr).ok()?;
        let mut pt = POINT { x: (rx * (cr.right - cr.left) as f64) as i32, y: (ry * (cr.bottom - cr.top) as f64) as i32 };
        let _ = ClientToScreen(hwnd, &mut pt);
        let mut dwm = RECT::default();
        if DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut dwm as *mut _ as *mut core::ffi::c_void, std::mem::size_of::<RECT>() as u32).is_err() {
            return Some((pt.x, pt.y));
        }
        Some((pt.x - dwm.left - 1, pt.y - dwm.top - 1))
    }
}

// ── window discovery by title substring (case-insensitive) ────────────────────
struct Finder { needle: String, hwnd: HWND, pid: u32 }
unsafe extern "system" fn enum_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let f = &mut *(lparam.0 as *mut Finder);
    if !IsWindowVisible(hwnd).as_bool() { return TRUE; }
    let mut buf = [0u16; 256];
    let n = GetWindowTextW(hwnd, &mut buf);
    if n > 0 {
        let title = String::from_utf16_lossy(&buf[..n as usize]);
        if title.to_lowercase().contains(&f.needle.to_lowercase()) {
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            f.hwnd = hwnd; f.pid = pid;
            return BOOL(0);
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

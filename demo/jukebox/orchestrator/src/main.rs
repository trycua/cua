//! CUA JUKEBOX orchestrator — coordinated multi-cursor "computer-use" music.
//!
//! Reads a MIDI file (or a built-in demo song), turns each track into its own
//! instrument window (a miniwob-style minigame), and gives each one its OWN
//! cua-driver session = its OWN uniquely-coloured agent cursor. While the song
//! plays, every note steers that track's cursor onto its widget and clicks it
//! in the background — the click is what makes the sound. One cursor per part,
//! one colour per agent, all driven off a single clock: the dumbest possible
//! orchestra, performed entirely by background computer-use.
//!
//! Timing is locked with the cursor's `glide_duration_ms` (set per session via
//! `set_agent_cursor_motion`): every glide takes a known, fixed time regardless
//! of how far the cursor must travel across its pitch strip, so the orchestrator
//! can pre-roll each click by exactly that lead and land the actuation on the
//! beat. (That field is honoured identically on macOS and Windows — it lives in
//! the shared render core.)

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

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

// Widget geometry — MUST match jukebox-app's WX0/WX1/WY0/WY1/KEYS_SPAN.
const WX0: f64 = 0.06;
const WX1: f64 = 0.94;
const WY0: f64 = 0.34;
const WY1: f64 = 0.92;
const KEYS_SPAN: i32 = 24;
const MAX_TRACKS: usize = 9;

// One cua-driver palette per agent. Keying a session to a palette NAME makes
// that session's overlay cursor render in that palette automatically
// (`Palette::for_instance(session)`), exactly like the sibling multi-cursor
// demo. The hex is that palette's mid colour, reused for the instrument window
// accent AND the transport legend, so the cursor, its window, and the legend
// all read as the same colour. (`cursor_color` on `set_agent_cursor_motion`
// only updates registry state, not the Windows overlay paint — so we rely on
// the session-name palette instead of fighting it.)
const SESSIONS: [(&str, &str); 9] = [
    ("crimson",     "#e85262"),
    ("amber",       "#f4b242"),
    ("aqua",        "#4ccce0"),
    ("mint_lime",   "#60daae"),
    ("orchid",      "#dd71ec"),
    ("soft_purple", "#b284ff"),
    ("rose_gold",   "#f784aa"),
    ("chartreuse",  "#b8dc36"),
    ("cobalt",      "#507eec"),
];

#[derive(Clone, Copy, PartialEq)]
enum Kind { Pad, Keys, Drums }

#[derive(Clone)]
struct Note {
    t: f64,
    pitch: u8,
    /// Carried from the source for fidelity; the instrument picks its own hit
    /// velocity (a background click can't carry one), so it's unused here.
    #[allow(dead_code)]
    vel: u8,
}

struct Track {
    name: String,
    notes: Vec<Note>,
    kind: Kind,
    wave: &'static str,
    root: i32,
    /// Number of semitones the pitch strip spans (fit to the track's range so
    /// wide melodies aren't clamped to the top key). Drums ignore it.
    span: i32,
    color: &'static str,
    session: String,
    title: String,
    hwnd: HWND,
    pid: u32,
    /// True when this track is percussion (most notes on MIDI channel 10, or a
    /// drum-kit name) → rendered as a 3-zone kick/snare/hat pad.
    is_drum: bool,
}

struct Song { tracks: Vec<Track>, bpm: u32, dur_sec: f64 }

/// Send-able snapshot of one cursor's worth of work (a "voice" — a single pool
/// member of a track). HWND is carried as an isize because the raw handle
/// pointer isn't `Send`. Several voices can share one track's window/colour.
struct Voice {
    pid: u32,
    hwnd_addr: isize,
    session: String,
    kind: Kind,
    root: i32,
    span: i32,
    notes: Arc<Vec<Note>>,
}

/// Max same-colour cursors a single track may grow to (a chord wider than this
/// reuses cursor 0 and accepts a little overlap rather than spawning forever).
const MAX_POOL: usize = 6;

/// Assign each (time-sorted) note to a pool cursor: try the lowest-indexed
/// cursor that's free (its previous glide finished ≥ now), else grow the pool,
/// else (at the cap) reuse cursor 0. `busy` is the glide window a cursor is
/// "occupied" for after it fires. Returns the per-note cursor index and the
/// resulting pool size. This is the deterministic form of "try the first, go to
/// the next if occupied during the 100ms, spawn one if none free".
fn assign_pool(notes: &[Note], busy: f64, cap: usize) -> (Vec<usize>, usize) {
    let mut free_at: Vec<f64> = Vec::new();
    let mut assign = Vec::with_capacity(notes.len());
    for n in notes {
        let chosen = free_at.iter().position(|&f| f <= n.t + 1e-6);
        let i = match chosen {
            Some(i) => i,
            None if free_at.len() < cap => { free_at.push(0.0); free_at.len() - 1 }
            None => 0,
        };
        free_at[i] = n.t + busy;
        assign.push(i);
    }
    (assign, free_at.len().max(1))
}

/// Blend a `#rrggbb` toward white by `t` (0=unchanged, 1=white) for a gradient
/// tip stop, so a forced-colour cursor still reads as a lit arrow, not a flat blob.
fn lighten(hex: &str, t: f64) -> String {
    let v = u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap_or(0xffffff);
    let f = |sh: u32| { let c = ((v >> sh) & 0xff) as f64; (c + (255.0 - c) * t) as u32 };
    format!("#{:02x}{:02x}{:02x}", f(16), f(8), f(0))
}

// ── kill-on-exit job (whole tree dies with the orchestrator) ───────────────────
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

fn main() {
    let demo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
    let repo_root = demo_root.parent().unwrap().parent().unwrap().to_path_buf();

    let cua = std::env::var("CUA_DRIVER_EXE").map(PathBuf::from)
        .unwrap_or_else(|_| repo_root.join("libs/cua-driver/rust/target/debug/cua-driver.exe"));
    let app = std::env::var("JUKEBOX_APP_EXE").map(PathBuf::from)
        .unwrap_or_else(|_| demo_root.join("target/debug/jukebox-app.exe"));
    if !cua.exists() { eprintln!("cua-driver.exe not found at {cua:?}"); std::process::exit(1); }
    if !app.exists() { eprintln!("jukebox-app.exe not found at {app:?} — run `cargo build` first"); std::process::exit(1); }

    // Raise the system timer resolution to 1ms so the per-note `thread::sleep`
    // that schedules each click is accurate to ~1ms instead of Windows' default
    // ~15ms — the dominant residual timing jitter once the per-note process
    // spawn was removed. Process-global; the OS restores it on exit.
    unsafe { let _ = windows::Win32::Media::timeBeginPeriod(1); }

    let args: Vec<String> = std::env::args().collect();
    let auto = args.iter().any(|a| a == "--auto");
    let midi_path = args.iter().skip(1).find(|a| a.to_lowercase().ends_with(".mid") || a.to_lowercase().ends_with(".midi"));
    let glide_ms: u64 = std::env::var("JUKEBOX_GLIDE_MS").ok().and_then(|s| s.parse().ok()).unwrap_or(200);
    // The dominant actuation latency IS the glide, so default the pre-roll to it
    // (the per-voice EMA refines from there); overridable with JUKEBOX_LEAD_MS.
    let lead = Duration::from_millis(std::env::var("JUKEBOX_LEAD_MS").ok().and_then(|s| s.parse().ok()).unwrap_or(glide_ms));

    let mut song = match midi_path {
        Some(p) => match load_midi(p) { Ok(s) => s, Err(e) => { eprintln!("[orch] MIDI parse failed ({e}); using demo song"); demo_song() } },
        None => demo_song(),
    };
    assign_roles(&mut song);
    if song.tracks.is_empty() { eprintln!("[orch] no tracks with notes"); std::process::exit(1); }
    eprintln!("[orch] {} parts @ {} bpm, {:.1}s:", song.tracks.len(), song.bpm, song.dur_sec);
    for t in &song.tracks {
        let kind = match t.kind { Kind::Drums => "drums", Kind::Pad => "pad", Kind::Keys => "keys" };
        let lo = t.notes.iter().map(|n| n.pitch as i32).min().unwrap_or(0);
        let hi = t.notes.iter().map(|n| n.pitch as i32).max().unwrap_or(0);
        let range = hi - lo + 1;
        let fit = if t.kind == Kind::Keys && range > t.span {
            format!(" RANGE {range}st > strip {} → {} note(s) clamped", t.span,
                t.notes.iter().filter(|n| (n.pitch as i32 - t.root) >= t.span).count())
        } else { String::new() };
        eprintln!("  · {:<22} {:<6} {:<8} {:>4} notes  range {}-{} ({}st), strip {}st{}",
            t.name, kind, t.wave, t.notes.len(), lo, hi, range, t.span, fit);
    }

    // Reap any leftover daemon from a previous run FIRST. Its job object only
    // fires KILL_ON_JOB_CLOSE when the *owning* orchestrator exits cleanly, so a
    // hard-killed run can leave a `cua-driver serve` alive — and a stale daemon
    // keeps its own full-screen overlay, whose agent0..N cursors stack on top of
    // this run's at the same (deterministic) coordinates. That's what makes it
    // look like more than one cursor per window. Start from a clean slate.
    for img in ["cua-driver.exe", "jukebox-app.exe"] {
        let _ = Command::new("taskkill").args(["/F", "/IM", img])
            .stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
    // Give the killed daemon time to release its named pipe before we start a
    // fresh one — too short a wait races the pipe handoff and can wedge the new
    // daemon's first connections (observed as a one-off ~10s-late performance).
    thread::sleep(Duration::from_millis(800));

    // Start the cua-driver daemon.
    eprintln!("[orch] starting cua-driver daemon…");
    let mut daemon = Command::new(&cua).arg("serve").stdout(Stdio::null()).stderr(Stdio::null())
        .spawn().expect("spawn cua-driver serve");
    assign_to_job(&daemon);
    thread::sleep(Duration::from_millis(1500));

    // Launch the controller (foreground) + one instrument window per track.
    let mut controller = Command::new(&app)
        .args(["controller", "--title", "CUA JUKEBOX — Transport",
               "--color", "#3bd0ff",
               "--bpm", &song.bpm.to_string(),
               "--dur-ms", &((song.dur_sec * 1000.0) as u64).to_string(),
               "--tracks", &song.tracks.iter().map(|t| format!("{}|{}", t.name, t.color)).collect::<Vec<_>>().join(",")])
        .stdout(Stdio::piped()).stderr(Stdio::null()).spawn().expect("spawn controller");
    assign_to_job(&controller);
    let controller_out = controller.stdout.take().unwrap();

    let mut kids: Vec<Child> = Vec::new();
    for t in &song.tracks {
        let mut c = Command::new(&app);
        c.args(["instrument",
                "--title", &t.title,
                "--kind", match t.kind { Kind::Keys => "keys", Kind::Drums => "drums", Kind::Pad => "pad" },
                "--wave", t.wave,
                "--color", t.color,
                "--label", &t.name,
                "--root", &t.root.to_string(),
                "--span", &t.span.to_string()]);
        if let Ok(ch) = c.stdout(Stdio::null()).stderr(Stdio::null()).spawn() { assign_to_job(&ch); kids.push(ch); }
    }

    thread::sleep(Duration::from_millis(1400)); // window warmup

    // Discover HWNDs/pids.
    for t in song.tracks.iter_mut() {
        if let Some((h, pid)) = find_window_by_title(&t.title) { t.hwnd = h; t.pid = pid; }
        else { eprintln!("[orch] (warn) no window for '{}'", t.title); }
    }
    song.tracks.retain(|t| !t.hwnd.0.is_null());
    let controller_hwnd = find_window_by_title("CUA JUKEBOX — Transport").map(|(h, _)| h);

    // Layout: a 600×80 transport bar above a tight grid of fixed 200×160 tiles,
    // packed GAP px apart and centered in the work area.
    const TILE_W: i32 = 200;
    const TILE_H: i32 = 160;
    const CTRL_W: i32 = 600;
    const CTRL_H: i32 = 80;
    const GAP: i32 = 6;
    let wa = work_area();
    let n = song.tracks.len() as i32;
    let cols = (n as f64).sqrt().ceil().max(1.0) as i32;
    let rows = (n + cols - 1) / cols.max(1);
    let grid_w = cols * TILE_W + (cols - 1) * GAP;
    let grid_h = rows * TILE_H + (rows - 1) * GAP;
    let block_w = grid_w.max(CTRL_W);
    let block_h = CTRL_H + GAP + grid_h;
    let ox = wa.left + (((wa.right - wa.left) - block_w) / 2).max(0);
    let oy = wa.top + (((wa.bottom - wa.top) - block_h) / 2).max(0);
    if let Some(ch) = controller_hwnd {
        place(ch, ox + (block_w - CTRL_W) / 2, oy, CTRL_W, CTRL_H, true);
    }
    let gy = oy + CTRL_H + GAP;
    let gx = ox + (block_w - grid_w) / 2;
    for (i, t) in song.tracks.iter().enumerate() {
        let (col, row) = (i as i32 % cols, i as i32 / cols);
        place(t.hwnd, gx + col * (TILE_W + GAP), gy + row * (TILE_H + GAP), TILE_W, TILE_H, false);
    }
    if let Some(ch) = controller_hwnd { unsafe { let _ = SetForegroundWindow(ch); } }
    thread::sleep(Duration::from_millis(700));

    // Build per-track cursor POOLS and arm each member. A track usually needs
    // one cursor, but when notes land within the glide window (e.g. a chord) the
    // first cursor is still mid-glide, so the next free pool cursor takes the
    // note — growing the pool. Every member of a track is forced to the SAME
    // colour (via set_agent_cursor_style's gradient — cursor_color on
    // set_agent_cursor_motion is registry-only on the Windows overlay and would
    // not repaint), so a chord fans out into several identically-coloured
    // cursors on that one window.
    // A cursor is "occupied" for the whole click — the glide PLUS the dispatch
    // overhead (IPC + click-post + arrival frame), not just the glide. Sizing
    // the pool against the real click duration means any track whose notes are
    // closer together than that spawns enough cursors to keep up, instead of one
    // cursor falling progressively behind.
    let busy_sec = (glide_ms as f64 + 130.0) / 1000.0;
    let mut voices: Vec<Voice> = Vec::new();
    for t in &song.tracks {
        let (assign, pool) = assign_pool(&t.notes, busy_sec, MAX_POOL);
        let tip = lighten(t.color, 0.5);
        for k in 0..pool {
            let session = format!("{}_{}", t.session, k);
            let _ = run_call(&cua, "set_agent_cursor_enabled",
                &format!(r#"{{"enabled":true,"session":"{session}"}}"#));
            // Fixed 100ms glide so every actuation lands on the beat regardless
            // of travel distance (and so "occupied for the glide window" has a
            // known length the pool sizes against).
            let _ = run_call(&cua, "set_agent_cursor_motion", &format!(
                r#"{{"session":"{session}","cursor_label":"{}","cursor_size":15,"glide_duration_ms":{glide_ms},"spring":1.0,"arc_size":0.08,"dwell_after_click_ms":0,"idle_hide_ms":0}}"#,
                t.name));
            let _ = run_call(&cua, "set_agent_cursor_style", &format!(
                r#"{{"session":"{session}","gradient_colors":["{tip}","{}"],"bloom_color":"{}"}}"#,
                t.color, t.color));
            let notes_k: Vec<Note> = t.notes.iter().zip(assign.iter())
                .filter(|(_, &a)| a == k).map(|(n, _)| n.clone()).collect();
            voices.push(Voice {
                pid: t.pid,
                hwnd_addr: t.hwnd.0 as isize,
                session,
                kind: t.kind,
                root: t.root,
                span: t.span,
                notes: Arc::new(notes_k),
            });
        }
        if pool > 1 {
            eprintln!("[orch] {:<10} pool={} same-colour cursors (notes closer than {:.0}ms)", t.name, pool, busy_sec * 1000.0);
        }
    }
    let plans: Arc<Vec<Voice>> = Arc::new(voices);

    let cua = Arc::new(cua);
    let playing = Arc::new(AtomicBool::new(false));
    let generation = Arc::new(AtomicU64::new(0));
    let dur_sec = song.dur_sec;
    let base_lead_ms = lead.as_secs_f64() * 1000.0;
    // `timing` collects the signed per-note error (actual−scheduled) for the
    // end-of-song report; cleared each performance. The adaptive lead itself is
    // PER-VOICE (a thread-local EMA below), so one congested track — e.g. the
    // Pad pool firing a triad — can't skew the lead of the tight single-cursor
    // tracks.
    let timing = Arc::new(std::sync::Mutex::new(Vec::<f64>::new()));
    // Pause/resume: `offset` is the song-second to (re)start from; `seg` records
    // the wall-clock start + offset of the running segment so PAUSE can compute
    // where we are. STOP resets offset to 0; PAUSE saves the current position.
    let offset = Arc::new(std::sync::Mutex::new(0.0_f64));
    let seg = Arc::new(std::sync::Mutex::new((Instant::now(), 0.0_f64)));

    let start_perf = {
        let plans = plans.clone();
        let cua = cua.clone();
        let playing = playing.clone();
        let generation = generation.clone();
        let timing = timing.clone();
        let seg = seg.clone();
        move |off: f64| {
            playing.store(true, Ordering::SeqCst);
            let g = generation.fetch_add(1, Ordering::SeqCst) + 1;
            let start = Instant::now();
            *seg.lock().unwrap() = (start, off);
            timing.lock().unwrap().clear();
            eprintln!("[orch] ▶ performing from {off:.1}s — {} cursors actuating in the background", plans.len());
            for ti in 0..plans.len() {
                let plans = plans.clone();
                let cua = cua.clone();
                let playing = playing.clone();
                let generation = generation.clone();
                let timing = timing.clone();
                thread::spawn(move || {
                    let p = &plans[ti];
                    let hwnd = HWND(p.hwnd_addr as *mut core::ffi::c_void);
                    // Thread-local adaptive lead (ms) for THIS voice only,
                    // warm-started at the ~constant dispatch overhead on top of
                    // the glide (IPC + click-post + the arrival frame) so even
                    // the first notes land near the beat; the EMA refines it.
                    let mut corr = 110.0_f64;
                    // One persistent daemon connection for this whole track —
                    // pipelines every click with no per-note process spawn (that
                    // spawn was the entire timing-jitter source). The daemon runs
                    // a handler task per connection, so the 6 tracks actuate
                    // concurrently; within a track, clicks are lock-step (the
                    // response marks the actuation moment we measure against).
                    let pipe = std::env::var("CUA_DRIVER_PIPE")
                        .unwrap_or_else(|_| r"\\.\pipe\cua-driver".into());
                    let mut conn = DaemonConn::open(&pipe);
                    for (idx, note) in p.notes.iter().enumerate() {
                        if !playing.load(Ordering::SeqCst) || generation.load(Ordering::SeqCst) != g { return; }
                        if note.t < off { continue; } // already played before the resume point
                        // Fire early by the adaptive lead so the actuation lands
                        // on the note's scheduled time despite the glide latency.
                        // `start` represents song-time `off`, so schedule at the
                        // note's time minus that offset.
                        let eff_lead = (base_lead_ms + corr).max(0.0);
                        let due = start + Duration::from_secs_f64(note.t - off);
                        let fire = due.checked_sub(Duration::from_secs_f64(eff_lead / 1000.0)).unwrap_or(due);
                        let now = Instant::now();
                        if fire > now { thread::sleep(fire - now); }
                        if !playing.load(Ordering::SeqCst) || generation.load(Ordering::SeqCst) != g { return; }
                        let (xf, yf) = target(p, idx, note.pitch);
                        if let Some((x, y)) = client_rel_to_local_px(hwnd, xf, yf) {
                            let args = format!(
                                r#"{{"pid":{},"window_id":{},"x":{},"y":{},"session":"{}"}}"#,
                                p.pid, p.hwnd_addr, x, y, p.session);
                            let ok = match conn.as_mut() { Some(c) => c.call("click", &args), None => false };
                            if !ok {
                                // Reconnect once, else fall back to a one-shot call.
                                conn = DaemonConn::open(&pipe);
                                match conn.as_mut() {
                                    Some(c) => { let _ = c.call("click", &args); }
                                    None => { let _ = run_call(&cua, "click", &args); }
                                }
                            }
                            // Response received ⇒ the click landed (actuation moment).
                            let err_ms = (start.elapsed().as_secs_f64() - note.t) * 1000.0;
                            timing.lock().unwrap().push(err_ms);
                            // EMA drives the mean error to zero: eff_lead = base +
                            // corr and err = latency − eff_lead, so corr += α·err
                            // converges corr → latency − base. Clamp tight so a
                            // transient stall can't integrate the lead away. α is
                            // fairly high so short voices (a few-note pool member)
                            // converge within the song.
                            corr = (corr + 0.30 * err_ms).clamp(-base_lead_ms, 600.0);
                        }
                    }
                });
            }
            // End-of-song timing report (one per performance/generation).
            let timing = timing.clone();
            let generation = generation.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_secs_f64((dur_sec - off).max(0.0) + 0.8));
                if generation.load(Ordering::SeqCst) != g { return; }
                report_timing(&timing.lock().unwrap(), base_lead_ms);
            });
        }
    };

    if auto {
        thread::sleep(Duration::from_millis(800));
        start_perf(0.0);
    } else {
        eprintln!("[orch] ready — click ▶ PLAY in the Transport window to start the performance.");
    }

    // React to the transport's PLAY / PAUSE / STOP (the human's one action that
    // drives the whole coordinated fleet — like the master window in the sibling
    // demo). PLAY resumes from the held offset; PAUSE freezes the position; STOP
    // resets to the top.
    use std::io::{BufRead, BufReader};
    let reader = BufReader::new(controller_out);
    for line in reader.lines().map_while(Result::ok) {
        // Drag-drop a .mid onto the transport → restart the whole demo with that
        // track. Re-exec a fresh orchestrator (detached, outside our job object,
        // so it survives our exit); it reaps our daemon/windows on startup and
        // comes up READY on the dropped file (no --auto — the user presses ▶).
        if let Some(path) = line.trim_end().strip_prefix("LOAD\t") {
            eprintln!("[orch] ↻ restarting with {path}");
            if let Ok(exe) = std::env::current_exe() {
                let _ = Command::new(exe).arg(path)
                    .stdout(Stdio::null()).stderr(Stdio::null()).spawn();
            }
            break; // fall through to teardown; the new orchestrator takes over
        }
        match line.trim() {
            // Idempotent: ignore PLAY while already performing (resume still works
            // — PAUSE clears `playing` first), so a stray PLAY can't double-start.
            "PLAY" if !playing.load(Ordering::SeqCst) => { let off = *offset.lock().unwrap(); start_perf(off); }
            "PAUSE" => {
                playing.store(false, Ordering::SeqCst);
                let (rs, off) = *seg.lock().unwrap();
                let pos = off + rs.elapsed().as_secs_f64();
                *offset.lock().unwrap() = pos;
                eprintln!("[orch] ⏸ paused at {pos:.1}s");
            }
            "STOP" => {
                playing.store(false, Ordering::SeqCst);
                *offset.lock().unwrap() = 0.0;
                eprintln!("[orch] ■ stopped");
            }
            _ => {}
        }
    }

    // Controller closed → tear everything down (job object kills the tree too).
    playing.store(false, Ordering::SeqCst);
    let _ = controller.kill();
    for mut k in kids { let _ = k.kill(); }
    let _ = daemon.kill();
}

// ── song construction ──────────────────────────────────────────────────────────

/// True if a track name reads like a full drum kit (vs a single drum part like
/// "Kick", which stays a single pad).
fn name_is_kit(name: &str) -> bool {
    let n = name.to_lowercase();
    ["drum kit", "drumkit", "drums", "percussion", "drum set"].iter().any(|s| n.contains(s))
}

/// Map a MIDI channel-10 drum note to a 3-zone pad: 0=kick, 1=snare/clap/tom,
/// 2=hat/cymbal/perc. (General MIDI percussion key map.)
fn drum_zone(pitch: u8) -> i32 {
    match pitch {
        35 | 36 => 0,                                              // bass/kick drums
        37 | 38 | 39 | 40 | 41 | 43 | 45 | 47 | 48 | 50 => 1,      // snare, clap, rim, toms
        _ => 2,                                                    // hats, cymbals, perc
    }
}

/// Infer a melodic track's minigame + synth voice from its (lower-cased) name.
/// Wave keywords (square/saw/triangle/8-bit/chip/synth/scifi/…) are matched
/// explicitly so chiptune/GM names map to a fitting voice instead of plain sine.
fn infer(name: &str) -> (Kind, &'static str) {
    let n = name.to_lowercase();
    let has = |k: &[&str]| k.iter().any(|s| n.contains(s));
    // Single drum parts (not full kits) → one-shot pad.
    if has(&["kick", "bass drum", "bd"]) { (Kind::Pad, "kick") }
    else if has(&["snare", "clap", "sd"]) { (Kind::Pad, "snare") }
    else if has(&["hi-hat", "hihat", "hat", "cymbal", "ride", "shaker", "tom "]) { (Kind::Pad, "hat") }
    // Bass before the wave words so "Bass Guitar" is a bass, not a saw lead.
    else if has(&["bass"]) { (Kind::Keys, "saw") }
    // Explicit waveform names FIRST (so "8-Bit Triangle" is a triangle, not
    // caught by the chip→square fallback below).
    else if has(&["sawtooth", "saw"]) { (Kind::Keys, "saw") }
    else if has(&["triangle"]) { (Kind::Keys, "triangle") }
    else if has(&["square", "pulse"]) { (Kind::Keys, "square") }
    // Generic chiptune → square.
    else if has(&["8-bit", "8bit", "chip", "nes"]) { (Kind::Keys, "square") }
    // Timbre families.
    else if has(&["lead", "synth", "scifi", "sci-fi", "guitar", "trumpet", "sax", "brass", "pluck"]) { (Kind::Keys, "square") }
    else if has(&["pad", "string", "choir", "organ", "ambient", "smooth", "warm"]) { (Kind::Keys, "triangle") }
    else if has(&["arp", "bell", "key", "piano", "mallet", "celesta", "harp"]) { (Kind::Keys, "triangle") }
    else { (Kind::Keys, "sine") }
}

fn assign_roles(song: &mut Song) {
    song.tracks.truncate(MAX_TRACKS);
    for (i, t) in song.tracks.iter_mut().enumerate() {
        let (kind, wave) = if t.is_drum { (Kind::Drums, "kick") } else { infer(&t.name) };
        t.kind = kind;
        t.wave = wave;
        let (sess, hex) = SESSIONS[i % SESSIONS.len()];
        t.color = hex;
        t.session = sess.to_string();
        t.title = format!("JUKEBOX {i:02} — {}", t.name);
        // Fit the pitch strip to the track's actual range so wide melodies are
        // played accurately instead of being clamped to the top key: root = the
        // lowest note, span = the full range (min 12 semitones for a usable
        // strip, capped at 48 = 4 octaves so the keys don't get microscopic).
        let lo = t.notes.iter().map(|n| n.pitch as i32).min().unwrap_or(48);
        let hi = t.notes.iter().map(|n| n.pitch as i32).max().unwrap_or(72);
        t.root = lo;
        t.span = (hi - lo + 1).clamp(12, 48);
    }
}

fn mk_track(name: &str, notes: Vec<Note>) -> Track {
    Track { name: name.into(), notes, kind: Kind::Keys, wave: "sine", root: 48, span: KEYS_SPAN,
        color: "#888", session: String::new(), title: String::new(), hwnd: HWND::default(), pid: 0,
        is_drum: false }
}

/// Generated 8-bar, 6-part loop at 120 bpm — a 1:1 feel-port of the HTML demo
/// song so the grid moves immediately even with no .mid file.
fn demo_song() -> Song {
    let q = 0.5_f64; // quarter-note seconds at 120 bpm
    let bars = 8;
    let roots = [36, 36, 43, 41];
    let sc = [0, 2, 4, 7, 9, 12];
    let (mut bass, mut kick, mut hat, mut pad, mut arp, mut lead) =
        (vec![], vec![], vec![], vec![], vec![], vec![]);
    for b in 0..bars {
        let o = b as f64 * q * 4.0;
        let r = roots[b % 4] as u8;
        bass.push(Note { t: o, pitch: r, vel: 100 });
        bass.push(Note { t: o + q, pitch: r, vel: 80 });
        bass.push(Note { t: o + 2.0 * q, pitch: r + 7, vel: 95 });
        bass.push(Note { t: o + 3.0 * q, pitch: r, vel: 80 });
        for k in 0..4 { kick.push(Note { t: o + k as f64 * q, pitch: 36, vel: 110 }); }
        for h in 0..8 { hat.push(Note { t: o + h as f64 * q / 2.0, pitch: 42, vel: if h % 2 == 0 { 80 } else { 55 } }); }
        // Triads struck simultaneously — three notes at the same instant means
        // one cursor can't cover them within the 100ms glide, so the Pad track
        // grows a 3-cursor same-colour pool that fans out across the strip.
        for iv in [12, 16, 19] { pad.push(Note { t: o, pitch: r + iv, vel: 52 }); }
        for iv in [12, 15, 19] { pad.push(Note { t: o + 2.0 * q, pitch: r + iv, vel: 50 }); }
        for a in 0..8 { arp.push(Note { t: o + a as f64 * q / 2.0, pitch: r + 24 + sc[(a + b) % 6] as u8, vel: 70 }); }
        if b % 2 == 1 { for l in 0..4 { lead.push(Note { t: o + l as f64 * q, pitch: r + 24 + sc[(l * 2) % 6] as u8, vel: 88 }); } }
    }
    Song {
        tracks: vec![
            mk_track("Bass", bass), mk_track("Kick", kick), mk_track("Hat", hat),
            mk_track("Pad", pad), mk_track("Arp", arp), mk_track("Lead", lead),
        ],
        bpm: 120,
        dur_sec: bars as f64 * q * 4.0,
    }
}

/// Parse a Standard MIDI File into per-track note lists. Single tempo (first
/// tempo event wins) — good enough for the demo; documented in the README.
fn load_midi(path: &str) -> Result<Song, String> {
    use midly::{MetaMessage, MidiMessage, Smf, Timing, TrackEventKind};
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let smf = Smf::parse(&data).map_err(|e| e.to_string())?;
    let ppq = match smf.header.timing { Timing::Metrical(t) => t.as_int() as f64, _ => 480.0 };
    // First tempo found across all tracks (us per quarter-note).
    let mut tempo = 500_000.0_f64;
    'outer: for tr in &smf.tracks {
        for ev in tr {
            if let TrackEventKind::Meta(MetaMessage::Tempo(t)) = ev.kind { tempo = t.as_int() as f64; break 'outer; }
        }
    }
    let spt = (tempo / 1e6) / ppq; // seconds per tick
    let mut tracks = Vec::new();
    let mut dur = 0.0_f64;
    for tr in &smf.tracks {
        let mut tick = 0u64;
        let mut name = String::new();
        let mut on: std::collections::HashMap<u8, (f64, u8)> = std::collections::HashMap::new();
        let mut notes = Vec::new();
        let mut drum_notes = 0usize; // notes seen on MIDI channel 10 (index 9)
        for ev in tr {
            tick += ev.delta.as_int() as u64;
            let now = tick as f64 * spt;
            match ev.kind {
                TrackEventKind::Meta(MetaMessage::TrackName(bytes)) =>
                    if name.is_empty() { name = String::from_utf8_lossy(bytes).trim().to_string(); },
                TrackEventKind::Midi { channel, message: MidiMessage::NoteOn { key, vel } } => {
                    if vel.as_int() > 0 {
                        on.insert(key.as_int(), (now, vel.as_int()));
                        if channel.as_int() == 9 { drum_notes += 1; }
                    }
                    else if let Some((t0, v)) = on.remove(&key.as_int()) { notes.push(Note { t: t0, pitch: key.as_int(), vel: v }); dur = dur.max(now); }
                }
                TrackEventKind::Midi { message: MidiMessage::NoteOff { key, .. }, .. } => {
                    if let Some((t0, v)) = on.remove(&key.as_int()) { notes.push(Note { t: t0, pitch: key.as_int(), vel: v }); dur = dur.max(now); }
                }
                _ => {}
            }
        }
        if !notes.is_empty() {
            notes.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap());
            let idx = tracks.len();
            let nm = if name.is_empty() { format!("Track {}", idx + 1) } else { name.clone() };
            // Percussion if most notes were on channel 10, or the name is a kit.
            let is_drum = drum_notes * 2 > notes.len() || name_is_kit(&nm);
            let mut t = mk_track(&nm, notes);
            t.is_drum = is_drum;
            tracks.push(t);
        }
    }
    if tracks.is_empty() { return Err("no note tracks".into()); }
    let bpm = (60.0 / (tempo / 1e6)).round() as u32;
    Ok(Song { tracks, bpm, dur_sec: dur + 0.5 })
}

// ── cua-driver call + Win32 helpers ─────────────────────────────────────────────

/// Print the per-note timing diff: how far each actuation landed from its
/// scheduled beat (actual − scheduled, ms). `mean`→0 means the adaptive lead
/// has cancelled the systematic latency; `sd`/`p90` are the residual jitter
/// (dominated by per-note `cua-driver call` subprocess spawn).
fn report_timing(errs: &[f64], base_lead_ms: f64) {
    if errs.is_empty() { eprintln!("[timing] no notes measured"); return; }
    let n = errs.len();
    let mut s = errs.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mean = errs.iter().sum::<f64>() / n as f64;
    let sd = (errs.iter().map(|e| (e - mean).powi(2)).sum::<f64>() / n as f64).sqrt();
    let absmean = errs.iter().map(|e| e.abs()).sum::<f64>() / n as f64;
    let pct = |q: f64| s[((n as f64 * q) as usize).min(n - 1)];
    eprintln!(
        "[timing] n={n}  mean={mean:+.1}ms  |mean|={absmean:.1}ms  sd={sd:.1}ms  \
         median={:+.1}ms  p10={:+.1}  p90={:+.1}  min={:+.1}  max={:+.1}",
        pct(0.5), pct(0.1), pct(0.9), s[0], s[n - 1],
    );
    eprintln!(
        "[timing] base lead {:.0}ms, refined per-voice; + ⇒ late, − ⇒ early.",
        base_lead_ms,
    );
}

fn run_call(cua: &PathBuf, tool: &str, json: &str) -> bool {
    Command::new(cua).arg("call").arg(tool).arg(json)
        .stdout(Stdio::null()).stderr(Stdio::null())
        .status().map(|s| s.success()).unwrap_or(false)
}

/// A persistent connection to the `cua-driver serve` daemon over its
/// line-delimited-JSON named pipe (`\\.\pipe\cua-driver`). The daemon handles
/// many requests per connection, so holding ONE open per track lets us pipeline
/// every note's click without paying a `cua-driver call` process spawn per note
/// — that spawn (tens of ms, high variance) was the entire timing-jitter
/// source. One connection per track also means tracks dispatch concurrently
/// server-side (a handler task per connection).
struct DaemonConn {
    writer: std::fs::File,
    reader: std::io::BufReader<std::fs::File>,
}

impl DaemonConn {
    fn open(pipe: &str) -> Option<DaemonConn> {
        use std::time::Instant as I;
        let deadline = I::now() + Duration::from_secs(3);
        loop {
            match std::fs::OpenOptions::new().read(true).write(true).open(pipe) {
                Ok(f) => {
                    let reader = std::io::BufReader::new(f.try_clone().ok()?);
                    return Some(DaemonConn { writer: f, reader });
                }
                Err(_) if I::now() < deadline => thread::sleep(Duration::from_millis(40)),
                Err(_) => return None,
            }
        }
    }

    /// Send one `call` and block for its response line. `args` is a JSON object
    /// literal. Returns whether the tool reported ok. The daemon processes one
    /// request per connection at a time and replies in order, so this lockstep
    /// is safe and the response marks the actuation moment.
    fn call(&mut self, tool: &str, args: &str) -> bool {
        use std::io::{BufRead, Write};
        let line = format!(r#"{{"method":"call","name":"{tool}","args":{args}}}"#) + "\n";
        if self.writer.write_all(line.as_bytes()).is_err() { return false; }
        if self.writer.flush().is_err() { return false; }
        let mut resp = String::new();
        match self.reader.read_line(&mut resp) {
            Ok(0) | Err(_) => false,
            Ok(_) => resp.contains("\"ok\":true"),
        }
    }
}

/// Window-local (x,y) fraction the cursor should click for a note. Melodic
/// strips map pitch → key cell (fixed y). Drum pads have no pitch, so we hop the
/// click around the pad each hit (deterministic per note index) — otherwise the
/// cursor would click dead-centre every time and look static; the hop makes the
/// 100ms glide visible. Must match jukebox-app's WX0/WX1/WY0/WY1/KEYS_SPAN.
fn target(p: &Voice, idx: usize, pitch: u8) -> (f64, f64) {
    match p.kind {
        Kind::Keys => {
            let span = p.span.max(1);
            let semi = (pitch as i32 - p.root).clamp(0, span - 1);
            (WX0 + (semi as f64 + 0.5) / span as f64 * (WX1 - WX0), (WY0 + WY1) / 2.0)
        }
        Kind::Drums => {
            // Click the kick / snare / hat zone this drum note belongs to.
            let z = drum_zone(pitch);
            (WX0 + (z as f64 + 0.5) / 3.0 * (WX1 - WX0), (WY0 + WY1) / 2.0)
        }
        Kind::Pad => {
            let h = (idx as u32).wrapping_mul(2_654_435_761);
            let xf = 0.30 + (h % 1000) as f64 / 1000.0 * 0.40;
            let yf = WY0 + 0.10 + ((h / 1000) % 1000) as f64 / 1000.0 * (WY1 - WY0 - 0.20);
            (xf, yf)
        }
    }
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

/// Client-relative (0..1) → the click tool's window-local screenshot-pixel space
/// (ClientToScreen minus the DWM extended-frame top-left + 1px inset). Same math
/// as the multi-cursor demo.
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

// ── window discovery by title substring (case-insensitive) ──────────────────────
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

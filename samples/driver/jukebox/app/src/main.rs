//! "CUA JUKEBOX" — one window per part of a song, each a tiny miniwob-style
//! minigame that an agent cursor actuates in time with the music. This binary
//! is BOTH window kinds the demo launches:
//!
//!   controller  — the transport: a big PLAY/STOP button (a real Win32 control
//!                 the human clicks), a track list, and a playhead. Emits
//!                 `PLAY` / `STOP` on stdout for the orchestrator to react to.
//!
//!   instrument  — one minigame + its own synth voice. When something clicks
//!                 the widget (the orchestrator drives these via cua-driver in
//!                 the background), the window plays its note and flashes. Two
//!                 widget kinds:
//!                   pad  — a single drum pad; any click = a fixed hit.
//!                   keys — a pitch strip; the click's X selects the semitone,
//!                          so the orchestrator "plays a melody" by choosing
//!                          where on the strip each note lands.
//!
//! Every instrument owns its own `rodio` output stream, so notes from the
//! separate instrument processes mix at the OS audio mixer — real polyphony
//! across the whole coordinated fleet.

#![windows_subsystem = "windows"]

use std::cell::RefCell;
use std::f32::consts::PI;
use std::io::Write;
use std::time::Instant;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW, CreateSolidBrush,
    DeleteDC, DeleteObject, DrawTextW, EndPaint, FillRect, FrameRect, InvalidateRect, SelectObject,
    SetBkMode, SetTextColor, DT_CENTER, DT_LEFT, DT_SINGLELINE, DT_VCENTER, HBRUSH, HFONT,
    PAINTSTRUCT, SRCCOPY, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{DragAcceptFiles, DragFinish, DragQueryFileW, HDROP};
use windows::Win32::UI::WindowsAndMessaging::*;

// ── widget geometry (client fractions) — MUST match orchestrator's targets ────
const WX0: f64 = 0.06; // widget left
const WX1: f64 = 0.94; // widget right
const WY0: f64 = 0.34; // widget top
const WY1: f64 = 0.92; // widget bottom
const KEYS_SPAN: i32 = 24; // semitones across a `keys` strip (2 octaves)

// ── synth ─────────────────────────────────────────────────────────────────────
#[derive(Clone, Copy, PartialEq)]
enum Wave { Sine, Square, Saw, Triangle, Kick, Snare, Hat }

impl Wave {
    fn parse(s: &str) -> Wave {
        match s {
            "sine" => Wave::Sine,
            "square" => Wave::Square,
            "saw" => Wave::Saw,
            "triangle" => Wave::Triangle,
            "kick" => Wave::Kick,
            "snare" => Wave::Snare,
            "hat" => Wave::Hat,
            _ => Wave::Sine,
        }
    }
}

fn midi_to_freq(m: f32) -> f32 { 440.0 * 2f32.powf((m - 69.0) / 12.0) }

/// A short enveloped oscillator. One per note; rodio mixes overlapping ones.
struct Tone {
    sr: u32,
    idx: u32,
    total: u32,
    phase: f32,
    wave: Wave,
    amp: f32,
    f0: f32,      // start frequency
    f1: f32,      // end frequency (0 = no pitch sweep)
    rng: u32,     // xorshift noise state
    prev_noise: f32,
}

impl Tone {
    fn note(wave: Wave, freq: f32, vel: f32) -> Tone {
        let sr = 44_100u32;
        let v = (vel / 127.0).clamp(0.05, 1.0);
        let (dur, amp, f0, f1) = match wave {
            Wave::Kick => (0.20, 0.55 * v, 155.0, 48.0),
            Wave::Snare => (0.18, 0.34 * v, 180.0, 0.0),
            Wave::Hat => (0.05, 0.24 * v, 9000.0, 0.0),
            _ => (0.34, 0.17 * v, freq, 0.0),
        };
        Tone {
            sr,
            idx: 0,
            total: (dur * sr as f32) as u32,
            phase: 0.0,
            wave,
            amp,
            f0,
            f1,
            rng: 0x9E37_79B9 ^ (freq as u32).wrapping_mul(2654435761),
            prev_noise: 0.0,
        }
    }
    fn noise(&mut self) -> f32 {
        // xorshift32
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

impl Iterator for Tone {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.idx >= self.total { return None; }
        let dur = self.total as f32 / self.sr as f32;
        let t = self.idx as f32 / self.sr as f32;
        let p = self.idx as f32 / self.total as f32;
        let atk = 0.004;
        let env = if t < atk { t / atk } else { (1.0 - (t - atk) / (dur - atk)).max(0.0).powf(1.6) };
        let freq = if self.f1 > 0.0 { self.f0 * (self.f1 / self.f0).powf(p) } else { self.f0 };
        self.phase += 2.0 * PI * freq / self.sr as f32;
        if self.phase > 2.0 * PI { self.phase -= 2.0 * PI; }
        let osc = match self.wave {
            Wave::Sine | Wave::Kick => self.phase.sin(),
            Wave::Square => if self.phase.sin() >= 0.0 { 1.0 } else { -1.0 },
            Wave::Saw => self.phase / PI - 1.0,
            Wave::Triangle => (2.0 / PI) * self.phase.sin().asin(),
            Wave::Snare => { let n = self.noise(); 0.7 * n + 0.3 * self.phase.sin() }
            Wave::Hat => { let n = self.noise(); let hp = n - self.prev_noise; self.prev_noise = n; hp }
        };
        self.idx += 1;
        Some(osc * env * self.amp)
    }
}

impl rodio::Source for Tone {
    fn current_frame_len(&self) -> Option<usize> { None }
    fn channels(&self) -> u16 { 1 }
    fn sample_rate(&self) -> u32 { self.sr }
    fn total_duration(&self) -> Option<std::time::Duration> { None }
}

// ── state ──────────────────────────────────────────────────────────────────────
#[derive(Clone, Copy, PartialEq)]
enum Mode { Controller, Instrument }

#[derive(Clone, Copy, PartialEq)]
enum Kind { Pad, Keys, Drums }

/// One note actuation's fading highlight. `key` is the strip cell it lit (-1 for
/// a drum pad); `inten` fades 1→0 and `age` grows so a pad press expands an
/// outward ring. Each pulse fades on its own clock, so overlapping presses stay
/// individually visible.
#[derive(Clone, Copy)]
struct Pulse { key: i32, inten: f32, age: f32 }

struct Audio {
    _stream: rodio::OutputStream,
    handle: rodio::OutputStreamHandle,
}

struct State {
    mode: Mode,
    cw: i32,
    ch: i32,
    accent: COLORREF,
    accent_rgb: (u8, u8, u8),
    label: String,
    f_title: HFONT,
    f_label: HFONT,
    f_big: HFONT,

    // instrument
    kind: Kind,
    wave: Wave,
    root: f32,
    /// Semitones the pitch strip spans (fit to the track's range by the
    /// orchestrator) — keys mapping + cell count use this, not the constant.
    span: i32,
    /// Melodic strips: recent note actuations, each fading on its own clock so
    /// a chord fanned across the pool lights several keys at once, individually.
    pulses: Vec<Pulse>,
    /// Drum pads: a single brightness that constantly fades and gets an impulse
    /// on each hit — so the faster you trigger it, the brighter it glows.
    pad_glow: f32,
    /// Drum kits: one such brightness per kick/snare/hat zone.
    zone_glow: [f32; 3],
    hits: u32,
    audio: Option<Audio>,

    // controller
    bpm: u32,
    dur_ms: u64,
    tracks: Vec<(String, COLORREF)>,
    hbtn: HWND,
    playing: bool,
    play_start: Option<Instant>,
    /// Song time (ms) already elapsed in finished segments — frozen while paused
    /// so the playhead (and the orchestrator's resume offset) hold position.
    accum_ms: u64,
}

thread_local! { static STATE: RefCell<Option<State>> = const { RefCell::new(None) }; }

const ID_PLAY: isize = 2001;
// Single transport button is an icon: ▶ when stopped/paused, ⏸ while playing.
const ICON_PLAY: PCWSTR = w!("▶");
const ICON_PAUSE: PCWSTR = w!("⏸");

fn emit(line: &str) { let _ = writeln!(std::io::stdout(), "{line}"); let _ = std::io::stdout().flush(); }
fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() }
fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1).cloned())
}
fn parse_color(s: &str) -> (COLORREF, (u8, u8, u8)) {
    let h = s.trim_start_matches('#');
    let v = u32::from_str_radix(h, 16).unwrap_or(0x00FFFF);
    let (r, g, b) = (((v >> 16) & 255) as u8, ((v >> 8) & 255) as u8, (v & 255) as u8);
    (COLORREF(((b as u32) << 16) | ((g as u32) << 8) | r as u32), (r, g, b))
}
fn rgb(r: u8, g: u8, b: u8) -> COLORREF { COLORREF(((b as u32) << 16) | ((g as u32) << 8) | r as u32) }
fn mix(a: (u8, u8, u8), b: (u8, u8, u8), t: f32) -> COLORREF {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    rgb(f(a.0, b.0), f(a.1, b.1), f(a.2, b.2))
}
fn fr(cw: i32, ch: i32, l: f64, t: f64, r: f64, b: f64) -> RECT {
    RECT { left: (cw as f64 * l) as i32, top: (ch as f64 * t) as i32,
           right: (cw as f64 * r) as i32, bottom: (ch as f64 * b) as i32 }
}
unsafe fn mk_font(h: i32, w: i32, face: PCWSTR) -> HFONT { CreateFontW(h, 0, 0, 0, w, 0, 0, 0, 0, 0, 0, 0, 0, face) }

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mode = if args.get(1).map(|s| s.as_str()) == Some("controller") { Mode::Controller } else { Mode::Instrument };
    let title = arg(&args, "--title").unwrap_or_else(|| "JUKEBOX".into());
    let (accent, accent_rgb) = parse_color(&arg(&args, "--color").unwrap_or_else(|| "#3bd0ff".into()));
    let label = arg(&args, "--label").unwrap_or_else(|| "PART".into());
    let kind = match arg(&args, "--kind").as_deref() {
        Some("keys") => Kind::Keys,
        Some("drums") => Kind::Drums,
        _ => Kind::Pad,
    };
    let wave = Wave::parse(&arg(&args, "--wave").unwrap_or_else(|| "sine".into()));
    let root: f32 = arg(&args, "--root").and_then(|s| s.parse().ok()).unwrap_or(48.0);
    let span: i32 = arg(&args, "--span").and_then(|s| s.parse().ok()).unwrap_or(KEYS_SPAN).clamp(1, 96);
    let bpm: u32 = arg(&args, "--bpm").and_then(|s| s.parse().ok()).unwrap_or(120);
    let dur_ms: u64 = arg(&args, "--dur-ms").and_then(|s| s.parse().ok()).unwrap_or(16000);
    let tracks: Vec<(String, COLORREF)> = arg(&args, "--tracks").map(|s| s.split(',')
        .filter_map(|t| { let mut it = t.splitn(2, '|'); Some((it.next()?.to_string(), parse_color(it.next().unwrap_or("#888")).0)) })
        .collect()).unwrap_or_default();

    let audio = if mode == Mode::Instrument {
        rodio::OutputStream::try_default().ok().map(|(s, h)| Audio { _stream: s, handle: h })
    } else { None };

    unsafe {
        let hmod = GetModuleHandleW(None).unwrap();
        let class = w!("CuaJukeboxWindow");
        let bg = CreateSolidBrush(rgb(0x0b, 0x0c, 0x12));
        let wc = WNDCLASSW { lpfnWndProc: Some(wnd_proc), hInstance: hmod.into(), lpszClassName: class,
            hbrBackground: bg, hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(), ..Default::default() };
        RegisterClassW(&wc);

        STATE.with(|s| *s.borrow_mut() = Some(State {
            mode, cw: 0, ch: 0, accent, accent_rgb, label,
            f_title: HFONT::default(), f_label: HFONT::default(), f_big: HFONT::default(),
            kind, wave, root, span, pulses: Vec::new(), pad_glow: 0.0, zone_glow: [0.0; 3], hits: 0, audio,
            bpm, dur_ms, tracks, hbtn: HWND::default(),
            playing: false, play_start: None, accum_ms: 0,
        }));

        let tw = wide(&title);
        // Fixed sizes the orchestrator also lays out against: a thin 600×80
        // transport bar, and tight 200×160 instrument tiles. Borderless
        // (WS_POPUP) so the tiles pack together with no caption/frame — the
        // whole client is the visualizer. (Esc on the focused transport quits.)
        let (dw, dh) = if mode == Mode::Controller { (600, 80) } else { (200, 160) };
        let hwnd = CreateWindowExW(WINDOW_EX_STYLE(0), class, PCWSTR(tw.as_ptr()), WS_POPUP | WS_VISIBLE,
            CW_USEDEFAULT, CW_USEDEFAULT, dw, dh, None, None, HINSTANCE(hmod.0), None).expect("CreateWindowExW");
        let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
        SetTimer(hwnd, 1, 40, None); // ~25fps: glow fade / playhead (region-clipped)

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() { let _ = TranslateMessage(&msg); DispatchMessageW(&msg); }
    }
}

unsafe fn relayout(hwnd: HWND, cw: i32, ch: i32) {
    STATE.with(|s| {
        let mut b = s.borrow_mut(); let Some(st) = b.as_mut() else { return };
        st.cw = cw; st.ch = ch;
        for f in [st.f_title, st.f_label, st.f_big] { if !f.is_invalid() { let _ = DeleteObject(f); } }
        let seg = wide("Segoe UI"); let mono = wide("Consolas");
        st.f_title = mk_font(-(ch / 16).clamp(14, 30), 800, PCWSTR(seg.as_ptr()));
        st.f_label = mk_font(-(ch / 26).clamp(11, 18), 500, PCWSTR(mono.as_ptr()));
        st.f_big   = mk_font(-(ch / 9).clamp(20, 64), 800, PCWSTR(seg.as_ptr()));
        if st.mode == Mode::Controller && !st.hbtn.0.is_null() {
            // Thin transport bar: one small square icon button at the left edge,
            // leaving room for the title. Big font so the ▶ / ⏸ glyph reads.
            let r = fr(cw, ch, 0.012, 0.16, 0.085, 0.84);
            let _ = MoveWindow(st.hbtn, r.left, r.top, r.right - r.left, r.bottom - r.top, true);
            SendMessageW(st.hbtn, WM_SETFONT, WPARAM(st.f_big.0 as usize), LPARAM(1));
        }
    });
    let _ = InvalidateRect(hwnd, None, true);
}

extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_CREATE => {
                if STATE.with(|s| s.borrow().as_ref().map(|st| st.mode)) == Some(Mode::Controller) {
                    let hinst = HINSTANCE(GetModuleHandleW(None).unwrap().0);
                    let hbtn = CreateWindowExW(
                        WINDOW_EX_STYLE(0), w!("BUTTON"), ICON_PLAY,
                        WS_CHILD | WS_VISIBLE | WINDOW_STYLE((BS_PUSHBUTTON | BS_CENTER | BS_VCENTER) as u32),
                        0, 0, 10, 10, hwnd, HMENU(ID_PLAY as *mut core::ffi::c_void), hinst, None).unwrap_or_default();
                    STATE.with(|s| if let Some(st) = s.borrow_mut().as_mut() { st.hbtn = hbtn; });
                    DragAcceptFiles(hwnd, true); // drop a .mid on the transport to load it
                }
                LRESULT(0)
            }
            WM_DROPFILES => {
                let hdrop = HDROP(wp.0 as *mut core::ffi::c_void);
                let mut buf = [0u16; 1024];
                let n = DragQueryFileW(hdrop, 0, Some(&mut buf));
                if n > 0 {
                    let path = String::from_utf16_lossy(&buf[..n as usize]);
                    emit(&format!("LOAD\t{path}")); // orchestrator restarts with this track
                }
                DragFinish(hdrop);
                LRESULT(0)
            }
            WM_SIZE => { let (cw, ch) = ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32); if cw > 0 && ch > 0 { relayout(hwnd, cw, ch); } LRESULT(0) }
            WM_COMMAND => {
                let (id, code) = ((wp.0 & 0xFFFF) as isize, ((wp.0 >> 16) & 0xFFFF) as u32);
                if code == BN_CLICKED && id == ID_PLAY { on_play_toggle(hwnd); }
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                if STATE.with(|s| s.borrow().as_ref().map(|st| st.mode)) == Some(Mode::Instrument) {
                    let x = (lp.0 & 0xFFFF) as i16 as i32;
                    actuate(hwnd, x);
                }
                LRESULT(0)
            }
            WM_TIMER => { on_tick(hwnd); LRESULT(0) }
            // Esc quits (borderless windows have no close button); closing the
            // transport EOFs its stdout, which tears the whole demo down.
            WM_KEYDOWN if wp.0 == 0x1B => { PostQuitMessage(0); LRESULT(0) }
            // We fully repaint every frame via a double-buffered WM_PAINT, so
            // suppress the default background erase — that erase-then-paint is
            // the other half of GDI flicker.
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => { paint(hwnd); LRESULT(0) }
            WM_DESTROY => { PostQuitMessage(0); LRESULT(0) }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

/// Instrument actuation: the click's X selects what to play (pitch on a strip,
/// or which kick/snare/hat zone on a kit), then play the voice + flash. Fires
/// whether the click came from the human or — the point — from cua-driver.
unsafe fn actuate(hwnd: HWND, x: i32) {
    STATE.with(|s| {
        let mut b = s.borrow_mut(); let Some(st) = b.as_mut() else { return };
        let cw = st.cw.max(1) as f64;
        let frac = ((x as f64 / cw) - WX0) / (WX1 - WX0);
        let (key, freq, wave, vel) = match st.kind {
            Kind::Keys => {
                let key = (frac * st.span as f64).floor().clamp(0.0, (st.span - 1) as f64) as i32;
                // Each press is its OWN pulse, fading independently — a chord
                // lights several keys at once, each on its own clock.
                st.pulses.push(Pulse { key, inten: 1.0, age: 0.0 });
                if st.pulses.len() > 32 { st.pulses.remove(0); }
                (key, midi_to_freq(st.root + key as f32), st.wave, 104.0)
            }
            Kind::Drums => {
                // X selects the kick/snare/hat zone; light that zone's brightness.
                let z = (frac * 3.0).floor().clamp(0.0, 2.0) as usize;
                st.zone_glow[z] = (st.zone_glow[z] + 0.5).min(1.0);
                let w = [Wave::Kick, Wave::Snare, Wave::Hat][z];
                (z as i32, 0.0, w, 112.0)
            }
            Kind::Pad => {
                // Impulse on a constantly-fading brightness: rapid hits
                // accumulate toward full, sparse hits fade between them.
                st.pad_glow = (st.pad_glow + 0.5).min(1.0);
                (-1, midi_to_freq(st.root), st.wave, 112.0)
            }
        };
        st.hits += 1;
        if let Some(a) = &st.audio { let _ = a.handle.play_raw(Tone::note(wave, freq, vel)); }
        // Heartbeat for the orchestrator / verification (stdout is null in
        // normal runs, so this is a no-op there).
        emit(&format!("HIT {} key={} {:.0}Hz", st.hits, key, freq));
    });
    let _ = InvalidateRect(hwnd, None, false);
}

/// The single transport button is a play/pause toggle: playing → pause (freeze
/// at the current position so the next press resumes from here); paused/stopped
/// → play/resume. The icon flips ▶ ⇄ ⏸.
unsafe fn on_play_toggle(hwnd: HWND) {
    STATE.with(|s| {
        let mut b = s.borrow_mut(); let Some(st) = b.as_mut() else { return };
        if st.playing {
            // → pause: freeze position, show the ▶ (play/resume) icon.
            if let Some(t0) = st.play_start { st.accum_ms += t0.elapsed().as_millis() as u64; }
            st.playing = false; st.play_start = None;
            emit("PAUSE"); let _ = SetWindowTextW(st.hbtn, ICON_PLAY);
        } else {
            // → play/resume: show the ⏸ (pause) icon.
            st.playing = true; st.play_start = Some(Instant::now());
            emit("PLAY"); let _ = SetWindowTextW(st.hbtn, ICON_PAUSE);
        }
    });
    let _ = InvalidateRect(hwnd, None, true);
}

/// Current song position in ms (accumulated finished segments + the running one).
fn position_ms(st: &State) -> u64 {
    st.accum_ms + st.play_start.map(|t| t.elapsed().as_millis() as u64).unwrap_or(0)
}

unsafe fn on_tick(hwnd: HWND) {
    // Returns the region to repaint this tick (None = nothing changed). Only the
    // changing band is invalidated, so an idle header/legend never re-composites
    // — that frees DWM/GPU bandwidth for the agent-cursor overlay (which is the
    // expensive full-virtual-screen layered window the cursors live on).
    let region: Option<RECT> = STATE.with(|s| {
        let mut b = s.borrow_mut(); let st = b.as_mut()?;
        let (cw, ch) = (st.cw.max(1), st.ch.max(1));
        match st.mode {
            Mode::Instrument => {
                let had = !st.pulses.is_empty() || st.pad_glow > 0.01
                    || st.zone_glow.iter().any(|&z| z > 0.01);
                for p in st.pulses.iter_mut() { p.age += 0.040; p.inten *= 0.88; }
                st.pulses.retain(|p| p.inten > 0.03);
                st.pad_glow *= 0.88;
                if st.pad_glow < 0.01 { st.pad_glow = 0.0; }
                for z in st.zone_glow.iter_mut() { *z *= 0.88; if *z < 0.01 { *z = 0.0; } }
                had.then(|| fr(cw, ch, 0.0, WY0 - 0.03, 1.0, 1.0)) // widget band only
            }
            Mode::Controller => {
                if !st.playing { return None; }
                if position_ms(st) > st.dur_ms + 250 {
                    // Song ended → reset to the top. Emit STOP so the orchestrator
                    // clears its playing flag + resets its resume offset to 0.
                    st.playing = false; st.play_start = None; st.accum_ms = 0;
                    emit("STOP"); let _ = SetWindowTextW(st.hbtn, ICON_PLAY);
                    return Some(fr(cw, ch, 0.0, 0.0, 1.0, 1.0)); // full repaint once
                }
                Some(fr(cw, ch, 0.0, 0.91, 1.0, 1.0)) // playhead bar only
            }
        }
    });
    if let Some(r) = region { let _ = InvalidateRect(hwnd, Some(&r), false); }
}

unsafe fn text(hdc: windows::Win32::Graphics::Gdi::HDC, r: RECT, s: &str, fmt: windows::Win32::Graphics::Gdi::DRAW_TEXT_FORMAT) {
    let mut t = wide(s); let mut rr = r;
    DrawTextW(hdc, &mut t, &mut rr, fmt | DT_SINGLELINE);
}

unsafe fn paint(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    STATE.with(|s| {
        let b = s.borrow(); let Some(st) = b.as_ref() else { return };
        let (cw, ch) = (st.cw.max(1), st.ch.max(1));
        // Double-buffer: build the whole frame in an off-screen DC, then blit it
        // to the window in one BitBlt. Painting straight to the window DC (with a
        // full-client background fill every frame, ~30fps from the glow/playhead
        // timer) is what caused the flicker — never the windows being recreated.
        let mem = CreateCompatibleDC(hdc);
        let bmp = CreateCompatibleBitmap(hdc, cw, ch);
        let old = SelectObject(mem, bmp);
        SetBkMode(mem, TRANSPARENT);

        let bg = CreateSolidBrush(rgb(0x0b, 0x0c, 0x12));
        let panel = CreateSolidBrush(rgb(0x16, 0x18, 0x22));
        let line = CreateSolidBrush(rgb(0x2a, 0x2d, 0x3e));
        let dim = rgb(0x6a, 0x6f, 0x85);
        let ink = rgb(0xe8, 0xea, 0xf2);
        let full = RECT { left: 0, top: 0, right: cw, bottom: ch };
        FillRect(mem, &full, bg);

        match st.mode {
            Mode::Controller => paint_controller(mem, st, cw, ch, &panel, &line, ink, dim),
            Mode::Instrument => paint_instrument(mem, st, cw, ch, &panel, &line, ink, dim),
        }
        for o in [bg, panel, line] { let _ = DeleteObject(o); }

        let _ = BitBlt(hdc, 0, 0, cw, ch, mem, 0, 0, SRCCOPY);
        SelectObject(mem, old);
        let _ = DeleteObject(bmp);
        let _ = DeleteDC(mem);
    });
    let _ = EndPaint(hwnd, &ps);
}

unsafe fn paint_controller(hdc: windows::Win32::Graphics::Gdi::HDC, st: &State, cw: i32, ch: i32, _panel: &HBRUSH, line: &HBRUSH, _ink: COLORREF, dim: COLORREF) {
    // Thin transport bar. A small icon play/pause button sits at the far left
    // (~9%); lay the rest out horizontally: title, bpm, track swatches, playhead.
    SelectObject(hdc, st.f_title); SetTextColor(hdc, rgb(0xff, 0xff, 0xff));
    text(hdc, fr(cw, ch, 0.11, 0.06, 0.55, 0.58), "CUA JUKEBOX", DT_LEFT | DT_VCENTER);
    SelectObject(hdc, st.f_label); SetTextColor(hdc, dim);
    text(hdc, fr(cw, ch, 0.115, 0.52, 0.55, 0.95),
        &format!("{} parts · {} bpm", st.tracks.len(), st.bpm), DT_LEFT | DT_VCENTER);

    // Row of track colour swatches (matches each instrument tile's colour).
    let n = st.tracks.len().max(1);
    let (x0, x1) = (0.56_f64, 0.985_f64);
    let sw_w = (x1 - x0) / n as f64;
    for (i, (_name, col)) in st.tracks.iter().enumerate() {
        let sx = x0 + i as f64 * sw_w;
        let sw = fr(cw, ch, sx, 0.18, sx + sw_w * 0.7, 0.66);
        let cb = CreateSolidBrush(*col); FillRect(hdc, &sw, cb); let _ = DeleteObject(cb);
    }

    // Playhead along the very bottom.
    let bar = fr(cw, ch, 0.0, 0.92, 1.0, 1.0);
    FillRect(hdc, &bar, *line);
    let frac = (position_ms(st) as f64 / st.dur_ms.max(1) as f64).clamp(0.0, 1.0);
    if frac > 0.0 {
        let mut fb = bar; fb.right = bar.left + ((bar.right - bar.left) as f64 * frac) as i32;
        // Dim while paused, bright while playing.
        let c = if st.playing { st.accent } else { mix((0x2a, 0x2d, 0x3e), st.accent_rgb, 0.45) };
        let cb = CreateSolidBrush(c); FillRect(hdc, &fb, cb); let _ = DeleteObject(cb);
    }
}

unsafe fn paint_instrument(hdc: windows::Win32::Graphics::Gdi::HDC, st: &State, cw: i32, ch: i32, _panel: &HBRUSH, line: &HBRUSH, ink: COLORREF, dim: COLORREF) {
    let black = (0x0b, 0x0c, 0x12);
    // header: swatch + label + hits
    let sw = fr(cw, ch, 0.06, 0.09, 0.10, 0.17);
    let cb = CreateSolidBrush(st.accent); FillRect(hdc, &sw, cb); let _ = DeleteObject(cb);
    SelectObject(hdc, st.f_title); SetTextColor(hdc, ink);
    text(hdc, fr(cw, ch, 0.13, 0.07, 0.78, 0.20), &st.label, DT_LEFT | DT_VCENTER);
    SelectObject(hdc, st.f_label); SetTextColor(hdc, dim);
    text(hdc, fr(cw, ch, 0.60, 0.07, 0.95, 0.20), &format!("{}♪", st.hits), DT_LEFT | DT_VCENTER);

    let frame = |r: &RECT, c: COLORREF| { let br = CreateSolidBrush(c); FrameRect(hdc, r, HBRUSH(br.0)); let _ = DeleteObject(br); };
    let panel_bg = (0x20, 0x23, 0x30);

    match st.kind {
        Kind::Pad => {
            // Brightness = the constantly-fading, impulse-driven pad_glow: fast
            // hits pile up to a bright pad, sparse hits let it dim between them.
            let g = st.pad_glow.clamp(0.0, 1.0);
            let pad = fr(cw, ch, WX0 + 0.10, WY0, WX1 - 0.10, WY1);
            let body = mix(panel_bg, st.accent_rgb, g * 0.9);
            let bb = CreateSolidBrush(body); FillRect(hdc, &pad, bb); let _ = DeleteObject(bb);
            frame(&pad, st.accent);
            SelectObject(hdc, st.f_big);
            SetTextColor(hdc, if g > 0.4 { rgb(black.0, black.1, black.2) } else { st.accent });
            let cap = match st.wave { Wave::Kick => "KICK", Wave::Snare => "SNARE", Wave::Hat => "HAT", _ => "PAD" };
            text(hdc, pad, cap, DT_CENTER | DT_VCENTER);
        }
        Kind::Keys => {
            let strip = fr(cw, ch, WX0, WY0, WX1, WY1);
            FillRect(hdc, &strip, *line);
            let span = st.span.max(1);
            let w = (strip.right - strip.left) as f64 / span as f64;
            for k in 0..span {
                let kx = strip.left + (k as f64 * w) as i32;
                let cell = RECT { left: kx + 1, top: strip.top + 1, right: kx + w as i32 - 1, bottom: strip.bottom - 1 };
                // Each cell glows by the brightest pulse on THAT key — so a chord
                // lights several cells at once, each fading independently.
                let inten = st.pulses.iter().filter(|p| p.key == k).map(|p| p.inten).fold(0.0_f32, f32::max);
                let c = if inten > 0.02 { mix(panel_bg, st.accent_rgb, inten) }
                        else if k % 12 == 0 { rgb(0x20, 0x23, 0x30) } else { rgb(0x18, 0x1a, 0x24) };
                let bb = CreateSolidBrush(c); FillRect(hdc, &cell, bb); let _ = DeleteObject(bb);
            }
            frame(&strip, st.accent);
            SelectObject(hdc, st.f_label); SetTextColor(hdc, dim);
            text(hdc, fr(cw, ch, WX0, WY1 + 0.005, WX1, 0.99), "pitch ◄ low · high ►", DT_CENTER | DT_VCENTER);
        }
        Kind::Drums => {
            // Three pads — KICK · SNARE · HAT — each its own impulse/fade glow.
            let caps = ["KICK", "SNARE", "HAT"];
            SelectObject(hdc, st.f_label);
            for z in 0..3usize {
                let zx0 = WX0 + z as f64 * (WX1 - WX0) / 3.0;
                let zx1 = WX0 + (z as f64 + 1.0) * (WX1 - WX0) / 3.0;
                let pad = fr(cw, ch, zx0 + 0.01, WY0, zx1 - 0.01, WY1);
                let g = st.zone_glow[z].clamp(0.0, 1.0);
                let body = mix(panel_bg, st.accent_rgb, g * 0.9);
                let bb = CreateSolidBrush(body); FillRect(hdc, &pad, bb); let _ = DeleteObject(bb);
                frame(&pad, st.accent);
                SetTextColor(hdc, if g > 0.4 { rgb(black.0, black.1, black.2) } else { st.accent });
                text(hdc, pad, caps[z], DT_CENTER | DT_VCENTER);
            }
        }
    }
}

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! X11 input-echo and color fixture for Linux conformance tests.
//!
//! A plain core-X11 window (no toolkit), so synthetic XSendEvent input and
//! XTest input both arrive exactly as sent. It:
//!
//! - paints a solid color (`--color RRGGBB`) or a 2x2 grid of known colors
//!   (`--grid`: red, green / blue, white) that fills the window at any size;
//! - appends one JSON line per event to `--log` (or `CUA_ENV_TEST_LOG`):
//!   `ready` (with its window id and pid), `key` (keysym, char, send_event),
//!   `button` (button, window-local x/y, send_event), `focus_in`/`focus_out`,
//!   `configure` (new size);
//! - optionally renames itself after `--rename-after-ms N` to `<title> (renamed)`;
//! - sets `_NET_WM_PID`, `WM_CLASS` (`cua-x11-pad`) and requests its
//!   `--x/--y` position.
//!
//! It runs only on Linux, inside the test container.

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("cua-spacesd-x11-pad only runs on Linux");
    std::process::exit(1);
}

#[cfg(target_os = "linux")]
fn main() {
    if let Err(error) = linux::run() {
        eprintln!("cua-spacesd-x11-pad: {error}");
        std::process::exit(1);
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::io::Write as _;
    use std::time::{Duration, Instant};

    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::*;
    use x11rb::protocol::Event;
    use x11rb::wrapper::ConnectionExt as _;

    struct Options {
        title: String,
        log: Option<String>,
        x: i16,
        y: i16,
        width: u16,
        height: u16,
        grid: bool,
        color: u32,
        rename_after: Option<Duration>,
        /// Play a continuous sine at this frequency (through `pacat`).
        tone_hz: Option<f64>,
        /// Paint white for 200 ms at CLOCK_MONOTONIC `flash_start_ms + k *
        /// flash_every_ms` (the A/V sync test beeps on the same schedule).
        flash_start_ms: Option<u64>,
        flash_every_ms: u64,
        /// Also beep (through the sound server) on the flash schedule.
        beep: bool,
    }

    fn options() -> Result<Options, String> {
        let mut options = Options {
            title: "cua x11 pad".into(),
            log: std::env::var("CUA_ENV_TEST_LOG").ok(),
            x: 0,
            y: 0,
            width: 320,
            height: 240,
            grid: false,
            color: 0x808080,
            rename_after: None,
            tone_hz: None,
            flash_start_ms: None,
            flash_every_ms: 1_000,
            beep: false,
        };
        let mut args = std::env::args().skip(1);
        while let Some(flag) = args.next() {
            let mut value = || args.next().ok_or_else(|| format!("{flag} needs a value"));
            match flag.as_str() {
                "--title" => options.title = value()?,
                "--log" => options.log = Some(value()?),
                "--x" => options.x = value()?.parse().map_err(|_| "bad --x")?,
                "--y" => options.y = value()?.parse().map_err(|_| "bad --y")?,
                "--width" => options.width = value()?.parse().map_err(|_| "bad --width")?,
                "--height" => options.height = value()?.parse().map_err(|_| "bad --height")?,
                "--grid" => options.grid = true,
                "--color" => {
                    options.color = u32::from_str_radix(value()?.trim_start_matches('#'), 16)
                        .map_err(|_| "bad --color")?
                }
                "--rename-after-ms" => {
                    options.rename_after = Some(Duration::from_millis(
                        value()?.parse().map_err(|_| "bad --rename-after-ms")?,
                    ))
                }
                "--tone-hz" => {
                    options.tone_hz = Some(value()?.parse().map_err(|_| "bad --tone-hz")?)
                }
                "--flash-start-monotonic-ms" => {
                    options.flash_start_ms = Some(
                        value()?
                            .parse()
                            .map_err(|_| "bad --flash-start-monotonic-ms")?,
                    )
                }
                "--beep" => options.beep = true,
                // Relative form: the fixture's own CLOCK_MONOTONIC (a sandbox
                // such as gVisor has its own monotonic clock).
                "--flash-start-in-ms" => {
                    let delay: u64 = value()?.parse().map_err(|_| "bad --flash-start-in-ms")?;
                    options.flash_start_ms = Some(monotonic_ms() + delay);
                }
                "--flash-every-ms" => {
                    options.flash_every_ms = value()?.parse().map_err(|_| "bad --flash-every-ms")?
                }
                other => return Err(format!("unknown flag {other}")),
            }
        }
        Ok(options)
    }

    /// Grid colors: top-left, top-right, bottom-left, bottom-right.
    pub const GRID: [u32; 4] = [0xff0000, 0x00ff00, 0x0000ff, 0xffffff];

    struct Logger {
        file: Option<std::fs::File>,
        started: Instant,
    }

    impl Logger {
        fn line(&mut self, mut value: serde_json::Value) {
            if let Some(object) = value.as_object_mut() {
                object.insert(
                    "t_ms".into(),
                    serde_json::json!(self.started.elapsed().as_millis() as u64),
                );
            }
            if let Some(file) = self.file.as_mut() {
                let _ = writeln!(file, "{value}");
                let _ = file.flush();
            }
        }
    }

    fn atom(conn: &impl Connection, name: &str) -> Result<Atom, Box<dyn std::error::Error>> {
        Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom)
    }

    fn set_title(
        conn: &impl Connection,
        window: Window,
        title: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let utf8 = atom(conn, "UTF8_STRING")?;
        let net_wm_name = atom(conn, "_NET_WM_NAME")?;
        conn.change_property8(
            PropMode::REPLACE,
            window,
            net_wm_name,
            utf8,
            title.as_bytes(),
        )?;
        conn.change_property8(
            PropMode::REPLACE,
            window,
            AtomEnum::WM_NAME,
            AtomEnum::STRING,
            title.as_bytes(),
        )?;
        conn.flush()?;
        Ok(())
    }

    fn paint(
        conn: &impl Connection,
        window: Window,
        gc: Gcontext,
        width: u16,
        height: u16,
        options: &Options,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let fill = |color: u32,
                    x: i16,
                    y: i16,
                    w: u16,
                    h: u16|
         -> Result<(), Box<dyn std::error::Error>> {
            conn.change_gc(gc, &ChangeGCAux::new().foreground(color))?;
            conn.poly_fill_rectangle(
                window,
                gc,
                &[Rectangle {
                    x,
                    y,
                    width: w,
                    height: h,
                }],
            )?;
            Ok(())
        };
        if options.grid {
            let (half_w, half_h) = (width / 2, height / 2);
            fill(GRID[0], 0, 0, half_w, half_h)?;
            fill(GRID[1], half_w as i16, 0, width - half_w, half_h)?;
            fill(GRID[2], 0, half_h as i16, half_w, height - half_h)?;
            fill(
                GRID[3],
                half_w as i16,
                half_h as i16,
                width - half_w,
                height - half_h,
            )?;
        } else {
            fill(options.color, 0, 0, width, height)?;
        }
        conn.flush()?;
        Ok(())
    }

    const RATE: u32 = 48_000;
    const CHUNK_FRAMES: usize = 480; // 10 ms

    /// A low-latency PulseAudio playback stream (libpulse-simple, loaded at
    /// run time): 20 ms target buffer, 10 ms pre-buffer.
    struct Player {
        _library: libloading::Library,
        write: unsafe extern "C" fn(
            *mut std::ffi::c_void,
            *const std::ffi::c_void,
            usize,
            *mut i32,
        ) -> i32,
        handle: *mut std::ffi::c_void,
    }

    // SAFETY: the handle is used from one thread only.
    unsafe impl Send for Player {}

    #[repr(C)]
    struct SampleSpec {
        format: i32,
        rate: u32,
        channels: u8,
    }

    #[repr(C)]
    struct BufferAttr {
        maxlength: u32,
        tlength: u32,
        prebuf: u32,
        minreq: u32,
        fragsize: u32,
    }

    /// Bytes buffered ahead by the player (its playback latency).
    const PLAYER_BUFFER_MS: u64 = 20;

    impl Player {
        fn open() -> Option<Self> {
            type NewFn = unsafe extern "C" fn(
                *const std::ffi::c_char,
                *const std::ffi::c_char,
                i32,
                *const std::ffi::c_char,
                *const std::ffi::c_char,
                *const SampleSpec,
                *const std::ffi::c_void,
                *const BufferAttr,
                *mut i32,
            ) -> *mut std::ffi::c_void;
            // SAFETY: well-known libpulse-simple symbols with their C signatures.
            unsafe {
                let library = libloading::Library::new("libpulse-simple.so.0").ok()?;
                let new: NewFn = *library.get(b"pa_simple_new\0").ok()?;
                let write = *library.get(b"pa_simple_write\0").ok()?;
                let spec = SampleSpec {
                    format: 3,
                    rate: RATE,
                    channels: 2,
                };
                let chunk = (CHUNK_FRAMES * 4) as u32;
                let attr = BufferAttr {
                    maxlength: u32::MAX,
                    tlength: chunk * 2,
                    prebuf: chunk,
                    minreq: u32::MAX,
                    fragsize: u32::MAX,
                };
                let mut error = 0;
                let handle = new(
                    std::ptr::null(),
                    c"cua-fixture".as_ptr(),
                    1,
                    std::ptr::null(),
                    c"cua-fixture".as_ptr(),
                    &spec,
                    std::ptr::null(),
                    &attr,
                    &mut error,
                );
                if handle.is_null() {
                    return None;
                }
                Some(Self {
                    _library: library,
                    write,
                    handle,
                })
            }
        }

        fn write(&mut self, samples: &[i16]) -> bool {
            let mut error = 0;
            // SAFETY: `samples` outlives the call; the handle is open.
            unsafe {
                (self.write)(
                    self.handle,
                    samples.as_ptr().cast(),
                    samples.len() * 2,
                    &mut error,
                ) >= 0
            }
        }
    }

    /// Real-time audio: a continuous tone and/or 100 ms 1 kHz beeps that
    /// become audible at CLOCK_MONOTONIC `beep_start_ms + k * every_ms`
    /// (written ahead by the player's buffer, as a media player does).
    fn start_audio(tone_hz: Option<f64>, beeps: Option<(u64, u64)>) {
        std::thread::spawn(move || {
            let Some(mut player) = Player::open() else {
                return;
            };
            let started = Instant::now();
            let start_ms = monotonic_ms();
            let mut phase = 0.0f64;
            let mut beep_phase = 0.0f64;
            for chunk_index in 0u64.. {
                let due = started + Duration::from_millis(chunk_index * 10);
                let now = Instant::now();
                if due > now {
                    std::thread::sleep(due - now);
                }
                // This chunk becomes audible PLAYER_BUFFER_MS after it is written.
                let audible_ms = start_ms + chunk_index * 10 + PLAYER_BUFFER_MS;
                let beeping = beeps.is_some_and(|(at, every)| {
                    audible_ms >= at && (audible_ms - at) % every.max(250) < 100
                });
                let mut buffer = Vec::with_capacity(CHUNK_FRAMES * 2);
                for _ in 0..CHUNK_FRAMES {
                    let value = if beeping {
                        beep_phase += 1_000.0 / f64::from(RATE);
                        (beep_phase * std::f64::consts::TAU).sin() * 12_000.0
                    } else if let Some(hz) = tone_hz {
                        phase += hz / f64::from(RATE);
                        (phase * std::f64::consts::TAU).sin() * 12_000.0
                    } else {
                        0.0
                    };
                    buffer.push(value as i16);
                    buffer.push(value as i16);
                }
                if !player.write(&buffer) {
                    return;
                }
            }
        });
    }

    /// CLOCK_MONOTONIC in milliseconds (shared with every process on the
    /// machine, unlike `Instant`'s opaque origin).
    pub fn monotonic_ms() -> u64 {
        let mut now = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: valid pointer to a timespec.
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) };
        now.tv_sec as u64 * 1_000 + now.tv_nsec as u64 / 1_000_000
    }

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let options = options()?;
        let mut logger = Logger {
            file: options
                .log
                .as_ref()
                .map(|path| {
                    std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)
                })
                .transpose()?,
            started: Instant::now(),
        };
        let (conn, screen_num) = x11rb::connect(None)?;
        let screen = &conn.setup().roots[screen_num];
        let window = conn.generate_id()?;
        conn.create_window(
            screen.root_depth,
            window,
            screen.root,
            options.x,
            options.y,
            options.width,
            options.height,
            0,
            WindowClass::INPUT_OUTPUT,
            screen.root_visual,
            &CreateWindowAux::new()
                .background_pixel(screen.black_pixel)
                .event_mask(
                    EventMask::EXPOSURE
                        | EventMask::KEY_PRESS
                        | EventMask::KEY_RELEASE
                        | EventMask::BUTTON_PRESS
                        | EventMask::BUTTON_RELEASE
                        | EventMask::FOCUS_CHANGE
                        | EventMask::STRUCTURE_NOTIFY,
                ),
        )?;
        set_title(&conn, window, &options.title)?;
        let pid = atom(&conn, "_NET_WM_PID")?;
        conn.change_property32(
            PropMode::REPLACE,
            window,
            pid,
            AtomEnum::CARDINAL,
            &[std::process::id()],
        )?;
        conn.change_property8(
            PropMode::REPLACE,
            window,
            AtomEnum::WM_CLASS,
            AtomEnum::STRING,
            b"cua-x11-pad\0CuaX11Pad\0",
        )?;
        // WM_NORMAL_HINTS: user-specified position and size (flags 1|2).
        conn.change_property32(
            PropMode::REPLACE,
            window,
            AtomEnum::WM_NORMAL_HINTS,
            AtomEnum::WM_SIZE_HINTS,
            &[
                1 | 2,
                options.x as u32,
                options.y as u32,
                u32::from(options.width),
                u32::from(options.height),
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
            ],
        )?;
        let gc = conn.generate_id()?;
        conn.create_gc(gc, window, &CreateGCAux::new())?;
        conn.map_window(window)?;
        conn.flush()?;
        let setup = conn.setup();
        let min_keycode = setup.min_keycode;
        let mapping = conn
            .get_keyboard_mapping(min_keycode, setup.max_keycode - min_keycode + 1)?
            .reply()?;
        let per = usize::from(mapping.keysyms_per_keycode).max(1);
        let (mut width, mut height) = (options.width, options.height);
        let mut renamed = false;
        let mut ready = false;
        if options.tone_hz.is_some() || options.beep {
            start_audio(
                options.tone_hz,
                options
                    .beep
                    .then(|| (options.flash_start_ms.unwrap_or(0), options.flash_every_ms)),
            );
        }
        let mut next_flash = options.flash_start_ms;
        let mut flash_until: Option<Instant> = None;
        loop {
            if let Some(at) = next_flash {
                if monotonic_ms() >= at {
                    next_flash = Some(at + options.flash_every_ms.max(250));
                    conn.change_gc(gc, &ChangeGCAux::new().foreground(0xffffff))?;
                    conn.poly_fill_rectangle(
                        window,
                        gc,
                        &[Rectangle {
                            x: 0,
                            y: 0,
                            width,
                            height,
                        }],
                    )?;
                    conn.flush()?;
                    flash_until = Some(Instant::now() + Duration::from_millis(200));
                    logger.line(serde_json::json!({"event": "flash", "monotonic_ms": monotonic_ms(), "scheduled_ms": at}));
                }
            }
            if flash_until.is_some_and(|until| Instant::now() >= until) {
                flash_until = None;
                paint(&conn, window, gc, width, height, &options)?;
            }
            if let (Some(after), false) = (options.rename_after, renamed) {
                if logger.started.elapsed() >= after {
                    renamed = true;
                    let title = format!("{} (renamed)", options.title);
                    set_title(&conn, window, &title)?;
                    logger.line(serde_json::json!({"event": "renamed", "title": title}));
                }
            }
            let Some(event) = conn.poll_for_event()? else {
                std::thread::sleep(Duration::from_millis(5));
                continue;
            };
            match event {
                Event::Expose(expose) if expose.count == 0 => {
                    paint(&conn, window, gc, width, height, &options)?;
                    if !ready {
                        ready = true;
                        logger.line(serde_json::json!({"event": "ready", "window": window, "pid": std::process::id(), "title": options.title}));
                    }
                }
                Event::ConfigureNotify(configure) if configure.window == window => {
                    if (configure.width, configure.height) != (width, height) {
                        width = configure.width;
                        height = configure.height;
                        logger.line(serde_json::json!({"event": "configure", "width": width, "height": height}));
                        paint(&conn, window, gc, width, height, &options)?;
                    }
                }
                Event::KeyPress(key) => {
                    let index = usize::from(key.detail.saturating_sub(min_keycode)) * per;
                    let shifted = u16::from(key.state) & u16::from(KeyButMask::SHIFT) != 0;
                    let keysym = mapping
                        .keysyms
                        .get(index + usize::from(shifted))
                        .copied()
                        .filter(|sym| *sym != 0)
                        .or_else(|| mapping.keysyms.get(index).copied())
                        .unwrap_or(0);
                    let character = char::from_u32(keysym).filter(|c| (' '..='~').contains(c));
                    logger.line(serde_json::json!({
                        "event": "key",
                        "keysym": keysym,
                        "char": character.map(String::from),
                        "state": u16::from(key.state),
                        "send_event": key.response_type & 0x80 != 0,
                    }));
                }
                Event::ButtonPress(button) => {
                    logger.line(serde_json::json!({
                        "event": "button",
                        "button": button.detail,
                        "x": button.event_x,
                        "y": button.event_y,
                        "send_event": button.response_type & 0x80 != 0,
                    }));
                }
                Event::FocusIn(_) => logger.line(serde_json::json!({"event": "focus_in"})),
                Event::FocusOut(_) => logger.line(serde_json::json!({"event": "focus_out"})),
                Event::DestroyNotify(_) => return Ok(()),
                Event::Error(error) => logger
                    .line(serde_json::json!({"event": "x_error", "error": format!("{error:?}")})),
                _ => {}
            }
        }
    }
}

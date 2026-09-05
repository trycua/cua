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
//! - **Linux X11:** no poll is wired; the sampler writes no samples
//! - **Linux Wayland:** no portable API exists. Hyprland is queried
//!   through a bounded JSON `cursorpos` request on the instance
//!   socket. Other compositors write no samples — the zoom renderer
//!   falls back to the click-point-only path.

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

const SAMPLE_INTERVAL: Duration = Duration::from_millis(1000 / SAMPLE_RATE_HZ as u64);
const STOP_POLL: Duration = Duration::from_millis(5);

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
        Self::start_with_sample(output_path, session_start, sample_cursor)
    }

    fn start_with_sample(
        output_path: PathBuf,
        session_start: Instant,
        mut sample: impl FnMut(&AtomicBool) -> Option<(f64, f64)> + Send + 'static,
    ) -> std::io::Result<Self> {
        let file = File::create(&output_path)?;
        let stop_flag = Arc::new(AtomicBool::new(false));
        let flag_for_thread = stop_flag.clone();
        let path_for_thread = output_path.clone();
        let handle = std::thread::spawn(move || {
            let mut writer = BufWriter::new(file);
            let mut count = 0usize;
            while !flag_for_thread.load(Ordering::Relaxed) {
                if let Some((x, y)) = sample(&flag_for_thread) {
                    if x.is_finite() && y.is_finite() {
                        let t_ms = session_start.elapsed().as_millis() as f64;
                        // Write one JSON object per line. We hand-format
                        // the trivial shape rather than pulling serde_json
                        // into the hot loop — keeps wakeup-cost bounded.
                        let _ = writeln!(
                            writer,
                            "{{\"t_ms\":{:.3},\"x\":{:.2},\"y\":{:.2}}}",
                            t_ms, x, y
                        );
                        count += 1;
                    }
                }
                sleep_interruptibly(&flag_for_thread, SAMPLE_INTERVAL);
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
        self.handle.take().and_then(|h| h.join().ok()).unwrap_or(0)
    }

    pub fn output_path(&self) -> &std::path::Path {
        &self.output_path
    }
}

impl Drop for CursorSampler {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn sleep_interruptibly(stop: &AtomicBool, total: Duration) {
    let deadline = Instant::now() + total;
    while !stop.load(Ordering::Relaxed) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return;
        }
        std::thread::sleep(remaining.min(STOP_POLL));
    }
}

// ── per-platform cursor poll ────────────────────────────────────────────────

#[cfg(target_os = "windows")]
fn sample_cursor(_stop: &AtomicBool) -> Option<(f64, f64)> {
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
fn sample_cursor(_stop: &AtomicBool) -> Option<(f64, f64)> {
    // ApplicationServices/CGEvent.h: CGEventCreate(nil) → CGEventRef;
    // CGEventGetLocation(event) → CGPoint. The point is in points
    // (top-left origin) so it matches the cursor-space convention the
    // renderer uses.
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn CGEventCreate(source: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
        fn CGEventGetLocation(event: *mut std::ffi::c_void) -> CGPoint;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(cf: *mut std::ffi::c_void);
    }
    #[repr(C)]
    #[derive(Copy, Clone)]
    struct CGPoint {
        x: f64,
        y: f64,
    }

    unsafe {
        let event = CGEventCreate(std::ptr::null_mut());
        if event.is_null() {
            return None;
        }
        let p = CGEventGetLocation(event);
        CFRelease(event);
        Some((p.x, p.y))
    }
}

#[cfg(target_os = "linux")]
fn sample_cursor(stop: &AtomicBool) -> Option<(f64, f64)> {
    // Stock Wayland has no portable pointer query. Hyprland exposes one
    // via JSON `cursorpos` on the instance socket (global layout
    // coordinates). Other compositors keep returning None so
    // cursor.jsonl stays empty. See trycua/cua#2194.
    sample_hyprland_cursor(stop)
}

#[cfg(target_os = "linux")]
const HYPRLAND_CURSORPOS_REQUEST: &[u8] = b"j/cursorpos";
#[cfg(target_os = "linux")]
const HYPRLAND_QUERY_DEADLINE: Duration = Duration::from_millis(100);
#[cfg(target_os = "linux")]
const HYPRLAND_REPLY_LIMIT: usize = 128;

#[cfg(target_os = "linux")]
fn sample_hyprland_cursor(stop: &AtomicBool) -> Option<(f64, f64)> {
    let path = hyprland_socket_path(|name| std::env::var_os(name))?;
    query_hyprland_cursor(&path, stop)
}

#[cfg(target_os = "linux")]
fn hyprland_socket_path(env: impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<PathBuf> {
    // Explicit session metadata wins over a signature inherited from a
    // parent Hyprland session (e.g. nested GNOME, Sway, or X11).
    if let Some(kind) = env("XDG_SESSION_TYPE").filter(|v| !v.is_empty()) {
        if !kind.to_str()?.eq_ignore_ascii_case("wayland") {
            return None;
        }
    }
    let current = env("XDG_CURRENT_DESKTOP").filter(|v| !v.is_empty());
    let session = env("XDG_SESSION_DESKTOP").filter(|v| !v.is_empty());
    if current.is_some() || session.is_some() {
        let mentions_hyprland = |value: Option<&std::ffi::OsString>| {
            value.and_then(|v| v.to_str()).is_some_and(|desktop| {
                desktop
                    .split(':')
                    .any(|part| part.eq_ignore_ascii_case("Hyprland"))
            })
        };
        if !mentions_hyprland(current.as_ref()) && !mentions_hyprland(session.as_ref()) {
            return None;
        }
    }
    let signature = env("HYPRLAND_INSTANCE_SIGNATURE")?;
    let signature = signature.to_str()?;
    if signature.is_empty()
        || signature == "."
        || signature == ".."
        || signature.contains(['/', '\0'])
    {
        return None;
    }
    let runtime = env("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("/run/user/{}", unsafe { libc::getuid() })));
    if !runtime.is_absolute() {
        return None;
    }
    // Hyprland v0.45.0 hyprctl/main.cpp:getRuntimeDir/request:
    // $XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/.socket.sock,
    // or /run/user/<uid>/hypr/... when XDG_RUNTIME_DIR is unset.
    Some(runtime.join("hypr").join(signature).join(".socket.sock"))
}

#[cfg(target_os = "linux")]
fn query_hyprland_cursor(path: &std::path::Path, stop: &AtomicBool) -> Option<(f64, f64)> {
    use std::io::{ErrorKind, Read};
    use std::net::Shutdown;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;

    if stop.load(Ordering::Relaxed) {
        return None;
    }

    let deadline = Instant::now() + HYPRLAND_QUERY_DEADLINE;
    let bytes = path.as_os_str().as_bytes();
    // SAFETY: sockaddr_un is a plain C struct; zero initializes the pathname.
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if bytes.len() >= address.sun_path.len() || bytes.contains(&0) {
        return None;
    }
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (dst, src) in address.sun_path.iter_mut().zip(bytes) {
        *dst = *src as libc::c_char;
    }
    // Set O_NONBLOCK at creation, not after connect: a full Unix socket
    // accept queue can otherwise block recording shutdown indefinitely.
    // SAFETY: valid domain/type, and the returned fd is owned exactly once.
    let fd = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            0,
        )
    };
    if fd < 0 {
        return None;
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    // SAFETY: address is initialized and its size matches the supplied length.
    let connected = unsafe {
        libc::connect(
            fd.as_raw_fd(),
            &address as *const _ as *const libc::sockaddr,
            std::mem::size_of_val(&address) as libc::socklen_t,
        )
    };
    // A busy/missing compositor is a skipped sample (including EAGAIN or
    // EINPROGRESS), never a retry loop or a blocking connect. Hyprland
    // evaluates this socket synchronously; an unclosed connection freezes
    // the compositor until its five-second timeout, so we always drop.
    if connected != 0 {
        return None;
    }
    let mut stream = std::os::unix::net::UnixStream::from(fd);
    write_all_deadline(&mut stream, HYPRLAND_CURSORPOS_REQUEST, deadline, stop)?;
    stream.shutdown(Shutdown::Write).ok()?;

    let mut reply = Vec::with_capacity(HYPRLAND_REPLY_LIMIT);
    let mut chunk = [0; HYPRLAND_REPLY_LIMIT];
    loop {
        if stop.load(Ordering::Relaxed) || Instant::now() >= deadline {
            return None;
        }
        match stream.read(&mut chunk) {
            Ok(0) => return parse_hypr_cursorpos_json(&reply),
            Ok(n) if reply.len() + n <= HYPRLAND_REPLY_LIMIT => {
                reply.extend_from_slice(&chunk[..n])
            }
            Ok(_) => return None,
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(_) => return None,
        }
    }
}

#[cfg(target_os = "linux")]
fn write_all_deadline(
    stream: &mut impl Write,
    mut data: &[u8],
    deadline: Instant,
    stop: &AtomicBool,
) -> Option<()> {
    use std::io::ErrorKind;
    while !data.is_empty() {
        if stop.load(Ordering::Relaxed) || Instant::now() >= deadline {
            return None;
        }
        match stream.write(data) {
            Ok(0) => return None,
            Ok(n) => data = &data[n..],
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(_) => return None,
        }
    }
    Some(())
}

#[cfg(target_os = "linux")]
fn parse_hypr_cursorpos_json(bytes: &[u8]) -> Option<(f64, f64)> {
    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let object = value.as_object()?;
    let x = object.get("x")?.as_f64()?;
    let y = object.get("y")?.as_f64()?;
    if x.is_finite() && y.is_finite() {
        Some((x, y))
    } else {
        None
    }
}

#[cfg(all(test, target_os = "linux"))]
mod linux_cursor_tests {
    use super::*;
    use std::io::Read;
    use std::os::unix::net::UnixListener;
    use std::sync::atomic::AtomicUsize;

    fn server(
        reply: &'static [u8],
        delay: Duration,
    ) -> (tempfile::TempDir, PathBuf, JoinHandle<()>) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ipc.sock");
        let listener = UnixListener::bind(&path).unwrap();
        listener.set_nonblocking(true).unwrap();
        let handle = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(1);
            loop {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_read_timeout(Some(Duration::from_secs(1)))
                            .unwrap();
                        let mut request = Vec::new();
                        stream.read_to_end(&mut request).unwrap();
                        assert_eq!(request, HYPRLAND_CURSORPOS_REQUEST);
                        std::thread::sleep(delay);
                        let _ = stream.write_all(reply);
                        break;
                    }
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    _ => break,
                }
            }
        });
        (dir, path, handle)
    }

    fn environment_path(entries: &[(&str, &str)]) -> Option<PathBuf> {
        hyprland_socket_path(|name| {
            entries
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).into())
        })
    }

    #[test]
    fn discovers_only_the_current_hyprland_session() {
        let base = [
            ("HYPRLAND_INSTANCE_SIGNATURE", "instance"),
            ("XDG_RUNTIME_DIR", "/run/user/123"),
        ];
        assert_eq!(
            environment_path(&base),
            Some(PathBuf::from("/run/user/123/hypr/instance/.socket.sock"))
        );
        for (key, value) in [
            ("XDG_SESSION_TYPE", "x11"),
            ("XDG_CURRENT_DESKTOP", "GNOME"),
            ("XDG_SESSION_DESKTOP", "sway"),
        ] {
            let mut values = base.to_vec();
            values.push((key, value));
            assert_eq!(
                environment_path(&values),
                None,
                "inherited signature in {key}={value}"
            );
        }
        assert_eq!(environment_path(&[]), None);
        assert_eq!(
            environment_path(&[("HYPRLAND_INSTANCE_SIGNATURE", "")]),
            None
        );
        for signature in ["../escape", "/absolute", ".", "..", "bad\0signature"] {
            assert_eq!(
                environment_path(&[("HYPRLAND_INSTANCE_SIGNATURE", signature)]),
                None
            );
        }
        let mut values = base.to_vec();
        values.extend([
            ("XDG_SESSION_TYPE", "wayland"),
            ("XDG_CURRENT_DESKTOP", "Hyprland:uwsm"),
            ("XDG_SESSION_DESKTOP", "uwsm"),
        ]);
        assert_eq!(environment_path(&values), environment_path(&base));
    }

    #[test]
    fn full_socket_backlog_does_not_block_connect() {
        use std::os::fd::AsRawFd;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("full.sock");
        let listener = UnixListener::bind(&path).unwrap();
        // Linux permits backlog + 1 queued connections; fill that single slot.
        assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), 0) }, 0);
        let queued = std::os::unix::net::UnixStream::connect(&path).unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(500));
            drop(listener);
            drop(queued);
        });
        let start = Instant::now();
        assert_eq!(query_hyprland_cursor(&path, &AtomicBool::new(false)), None);
        let elapsed = start.elapsed();
        release.join().unwrap();
        assert!(
            elapsed < Duration::from_millis(300),
            "connect took {elapsed:?}"
        );
    }

    #[test]
    fn oversized_reply_is_rejected() {
        let mut reply = vec![b' '; 1024];
        reply.extend_from_slice(br#"{"x":1,"y":2}"#);
        let reply = Box::leak(reply.into_boxed_slice());
        let (_dir, path, handle) = server(reply, Duration::ZERO);
        let result = query_hyprland_cursor(&path, &AtomicBool::new(false));
        handle.join().unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn stalled_query_has_total_deadline() {
        let (_dir, path, handle) = server(br#"{"x":1,"y":2}"#, Duration::from_millis(500));
        let start = Instant::now();
        let result = query_hyprland_cursor(&path, &AtomicBool::new(false));
        let elapsed = start.elapsed();
        handle.join().unwrap();
        assert_eq!(result, None, "accepted reply after deadline");
        assert!(
            elapsed < Duration::from_millis(300),
            "query took {elapsed:?}"
        );
    }

    #[test]
    fn queries_private_hyprland_socket() {
        let (_dir, path, handle) = server(br#"{"x":-598,"y":317}"#, Duration::ZERO);
        let result = query_hyprland_cursor(&path, &AtomicBool::new(false));
        handle.join().unwrap();
        assert_eq!(result, Some((-598.0, 317.0)));
    }

    #[test]
    fn stop_flag_aborts_a_blocked_read() {
        let (_dir, path, handle) = server(br#"{"x":1,"y":2}"#, Duration::from_millis(500));
        let stop = Arc::new(AtomicBool::new(false));
        let path = path.clone();
        let stop_for_thread = stop.clone();
        let query = std::thread::spawn(move || query_hyprland_cursor(&path, &stop_for_thread));
        std::thread::sleep(Duration::from_millis(20));
        let start = Instant::now();
        stop.store(true, Ordering::Relaxed);
        let result = query.join().unwrap();
        let elapsed = start.elapsed();
        handle.join().unwrap();
        assert_eq!(result, None);
        assert!(
            elapsed < Duration::from_millis(100),
            "stop waited {elapsed:?}"
        );
    }

    #[test]
    fn stop_returns_while_a_sample_is_blocked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cursor.jsonl");
        let entered = Arc::new(AtomicBool::new(false));
        let entered_for_sample = entered.clone();
        let sampler = CursorSampler::start_with_sample(path, Instant::now(), move |stop| {
            entered_for_sample.store(true, Ordering::Relaxed);
            let started = Instant::now();
            while !stop.load(Ordering::Relaxed) && started.elapsed() < Duration::from_secs(5) {
                std::thread::sleep(Duration::from_millis(5));
            }
            None
        })
        .unwrap();
        let wait_for_enter = Instant::now();
        while !entered.load(Ordering::Relaxed) {
            assert!(
                wait_for_enter.elapsed() < Duration::from_secs(1),
                "sampler thread did not enter the blocked sample"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        let start = Instant::now();
        let count = sampler.stop();
        assert_eq!(count, 0);
        assert!(
            start.elapsed() < Duration::from_millis(200),
            "stop took {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn records_finite_json_and_skips_nonfinite_samples() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cursor.jsonl");
        let remaining = Arc::new(AtomicUsize::new(3));
        let remaining_for_sample = remaining.clone();
        let sampler = CursorSampler::start_with_sample(path.clone(), Instant::now(), move |_| {
            match remaining_for_sample.fetch_sub(1, Ordering::Relaxed) {
                3 => Some((1.5, 2.25)),
                2 => Some((f64::NAN, 1.0)),
                _ => None,
            }
        })
        .unwrap();
        std::thread::sleep(Duration::from_millis(120));
        let count = sampler.stop();
        let text = std::fs::read_to_string(&path).unwrap();
        let rows: Vec<serde_json::Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).expect("valid JSON record"))
            .collect();
        assert_eq!(count, 1);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["x"], 1.5);
        assert_eq!(rows[0]["y"], 2.25);
        assert!(rows[0]["t_ms"].as_f64().unwrap().is_finite());
    }

    #[test]
    fn rejects_nonfinite_and_malformed_cursor_positions() {
        for text in [
            br#"{"x":null,"y":0}"# as &[u8],
            br#"{"x":"1","y":2}"#,
            br#"{"x":1}"#,
            br#"[1,2]"#,
            b"NaN, 0",
            b"1, 2",
            b"",
            br#"{"x":1e999,"y":2}"#,
        ] {
            assert_eq!(
                parse_hypr_cursorpos_json(text),
                None,
                "accepted {}",
                String::from_utf8_lossy(text)
            );
        }
        assert_eq!(
            parse_hypr_cursorpos_json(br#"{"x":-598.25,"y":-317}"#),
            Some((-598.25, -317.0))
        );
        assert_eq!(
            parse_hypr_cursorpos_json(br#"{"x":4367,"y":-964,"extra":true}"#),
            Some((4367.0, -964.0))
        );
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
fn sample_cursor(_stop: &AtomicBool) -> Option<(f64, f64)> {
    None
}

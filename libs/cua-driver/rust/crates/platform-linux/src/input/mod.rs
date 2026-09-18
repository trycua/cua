//! Background input injection for Linux via X11 XSendEvent.
//!
//! XSendEvent sends synthetic events directly to a window without changing
//! input focus — the Linux equivalent of PostMessage on Windows, and the
//! mechanism behind the cross-platform "no focus steal" contract.
//!
//! Note: a few apps check the `send_event` flag and ignore synthetic events.
//! Terminal emulators are the notable case (xterm's `allowSendEvents` is off by
//! default); those are handled out of band by writing to the pty master — see
//! `crate::tty`. We deliberately do NOT fall back to the XTest extension for
//! them, because XTest delivers to the *focused* window and would break the
//! no-focus-steal contract.

/// Shared `delivery_mode` contract (background|foreground) — mirrors macOS
/// `tools::DeliveryMode` and Windows `input::delivery`.
pub mod delivery;
pub mod focus_guard;
pub mod foreground;
mod mpx_keyboard;
mod mpx_owner;

pub use focus_guard::{FocusGuardReport, FocusSnapshot, SameAppWindow};
pub use foreground::{with_x11_foreground_opts, FocusAfter, ForegroundOptions, ForegroundReport};
pub use mpx_keyboard::{
    real_keyboard_input_available, send_virtual_keyboard_key, send_virtual_keyboard_text,
    KeyboardDeliveryReport, MPX_UINPUT_PATH,
};

use anyhow::{anyhow, bail, Context, Result};
use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, EventType, InputEvent, Key, RelativeAxisType};
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::fs;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::sleep;
use std::time::Duration;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::rust_connection::RustConnection;

const CLICK_DELAY_MS: u64 = 35;
const DOUBLE_CLICK_DELAY_MS: u64 = 50;
const KEY_DELAY_MS: u64 = 10;

#[derive(Clone, Debug)]
pub struct VirtualPointerDrag {
    pub target_window: u64,
    pub button: u8,
    /// Screen-coordinate waypoints. The pointer presses once at `path[0]`,
    /// glides through every waypoint (arc-length interpolated), and releases
    /// once at the last point — a single continuous held drag, so a curved
    /// path (e.g. a sampled y = f(x)) draws as one smooth stroke rather than
    /// a chain of press/release dabs. Must contain >= 2 points.
    pub path: Vec<(i32, i32)>,
    pub duration_ms: u64,
    pub steps: usize,
}

/// Cumulative segment lengths along `path` and its total length.
fn path_cumulative(path: &[(i32, i32)]) -> (Vec<f64>, f64) {
    let mut cum = Vec::with_capacity(path.len());
    let mut total = 0.0;
    cum.push(0.0);
    for w in path.windows(2) {
        let dx = (w[1].0 - w[0].0) as f64;
        let dy = (w[1].1 - w[0].1) as f64;
        total += (dx * dx + dy * dy).sqrt();
        cum.push(total);
    }
    (cum, total)
}

/// Sample `y = f(x)` over `[x_from, x_to]` into `samples` window-local
/// waypoints. Evaluated with meval (sin/cos/^/etc.); non-finite outputs
/// (ln of a negative, 1/0, …) are dropped. Errors on a bad expression or
/// fewer than 2 finite points.
pub fn sample_function(
    expr: &str,
    x_from: f64,
    x_to: f64,
    samples: u64,
) -> Result<Vec<(f64, f64)>> {
    let parsed: meval::Expr = expr
        .parse()
        .map_err(|e| anyhow!("invalid fn '{expr}': {e}"))?;
    let f = parsed
        .bind("x")
        .map_err(|e| anyhow!("fn must be in terms of x: {e}"))?;
    let n = samples.max(2);
    let mut pts = Vec::with_capacity(n as usize);
    for i in 0..n {
        let x = x_from + (x_to - x_from) * (i as f64) / ((n - 1) as f64);
        let y = f(x);
        if x.is_finite() && y.is_finite() {
            pts.push((x, y));
        }
    }
    if pts.len() < 2 {
        bail!("fn produced fewer than 2 finite points over the domain");
    }
    Ok(pts)
}

/// Point at arc-length fraction `t` (0..1) along `path`.
fn point_on_path(path: &[(i32, i32)], cum: &[f64], total: f64, t: f64) -> (i32, i32) {
    if path.len() == 1 || total <= 0.0 {
        return *path.last().unwrap();
    }
    let d = t.clamp(0.0, 1.0) * total;
    let mut i =
        match cum.binary_search_by(|v| v.partial_cmp(&d).unwrap_or(std::cmp::Ordering::Less)) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        };
    if i >= path.len() - 1 {
        i = path.len() - 2;
    }
    let seg = cum[i + 1] - cum[i];
    let f = if seg > 0.0 { (d - cum[i]) / seg } else { 0.0 };
    let x = path[i].0 as f64 + (path[i + 1].0 - path[i].0) as f64 * f;
    let y = path[i].1 as f64 + (path[i + 1].1 - path[i].1) as f64 * f;
    (x.round() as i32, y.round() as i32)
}

/// One session's XI2 master pair (XIAddMaster always creates a pointer AND a
/// keyboard) plus the uinput slaves attached to it. The keyboard slave is
/// created lazily by `mpx_keyboard::ensure_master_keyboard`, so a pointer-only
/// click never pays for a second device hotplug.
#[derive(Clone, Copy, Debug)]
struct MasterPointerIds {
    pointer_id: i32,
    keyboard_id: i32,
    _slave_pointer_id: i32,
    slave_keyboard_id: Option<i32>,
}

static MPX_POINTERS: OnceLock<Mutex<HashMap<String, MasterPointerIds>>> = OnceLock::new();
static UINPUT_POINTERS: OnceLock<Mutex<HashMap<String, Arc<Mutex<VirtualDevice>>>>> =
    OnceLock::new();
static XLIB_THREADS_READY: OnceLock<Result<(), String>> = OnceLock::new();
/// Serialises every MPX operation against the idle reaper (and each other),
/// so a retained master pair is never torn down while a call is using it.
static MPX_OP_LOCK: Mutex<()> = Mutex::new(());
static MPX_LAST_USE: OnceLock<Mutex<HashMap<String, std::time::Instant>>> = OnceLock::new();
static MPX_IDLE_REAPER: std::sync::Once = std::sync::Once::new();
/// A session's retained master pair is removed after this much inactivity;
/// `end_session` and the startup reaper cover the explicit and crash cases.
const MPX_IDLE_TTL: Duration = Duration::from_secs(180);
const MPX_IDLE_REAPER_PERIOD: Duration = Duration::from_secs(30);
static MPX_NAME_COUNTER: AtomicU64 = AtomicU64::new(1);
// evdev 0.12.2 asserts `name.len() + 1 < UINPUT_MAX_NAME_SIZE` while building
// a device. Linux defines UINPUT_MAX_NAME_SIZE as 80, leaving 78 usable bytes.
const EVDEV_UINPUT_NAME_MAX_BYTES: usize = 78;
const UINPUT_POINTER_SUFFIX: &str = " uinput pointer";
pub const UINPUT_UNAVAILABLE_CODE: &str = "uinput_unavailable";
/// Result `path` for pointer actions delivered as real button events from the
/// session's MPX virtual master pointer.
pub const MPX_POINTER_PATH: &str = "mpx_pointer";

#[derive(Debug, thiserror::Error)]
#[error("Linux uinput device unavailable: {reason}")]
struct UinputUnavailable {
    reason: String,
}

fn mpx_pointers() -> &'static Mutex<HashMap<String, MasterPointerIds>> {
    MPX_POINTERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn mpx_last_use() -> &'static Mutex<HashMap<String, std::time::Instant>> {
    MPX_LAST_USE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Take the MPX operation lock for one call on `cursor_id`, stamp its last
/// use, and make sure the idle reaper is running. Hold the guard for the
/// whole operation.
fn mpx_op_guard(cursor_id: &str) -> std::sync::MutexGuard<'static, ()> {
    let guard = MPX_OP_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    mpx_last_use()
        .lock()
        .unwrap()
        .insert(cursor_id.to_owned(), std::time::Instant::now());
    MPX_IDLE_REAPER.call_once(|| {
        std::thread::Builder::new()
            .name("cua-mpx-idle-reaper".into())
            .spawn(|| loop {
                sleep(MPX_IDLE_REAPER_PERIOD);
                let _op = MPX_OP_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                let stale = {
                    let last_use = mpx_last_use().lock().unwrap();
                    stale_cursor_ids(&last_use, std::time::Instant::now(), MPX_IDLE_TTL)
                };
                for cursor_id in stale {
                    tracing::info!(cursor_id, "removing idle MPX master pair");
                    forget_master_pointer(&cursor_id);
                }
            })
            .ok();
    });
    guard
}

/// Sessions whose last MPX use is older than `ttl`.
fn stale_cursor_ids(
    last_use: &HashMap<String, std::time::Instant>,
    now: std::time::Instant,
    ttl: Duration,
) -> Vec<String> {
    last_use
        .iter()
        .filter(|(_, at)| now.saturating_duration_since(**at) >= ttl)
        .map(|(id, _)| id.clone())
        .collect()
}

fn uinput_pointers() -> &'static Mutex<HashMap<String, Arc<Mutex<VirtualDevice>>>> {
    UINPUT_POINTERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn master_pointer_name(cursor_id: &str) -> String {
    let nonce = MPX_NAME_COUNTER.fetch_add(1, Ordering::Relaxed);
    if let Ok(owner) = mpx_owner::Owner::current() {
        return owner.master_name(nonce);
    }
    // Unknown procfs identity cannot safely participate in automatic recovery.
    // Preserve ordinary operation, with the legacy name and cleanup behavior.
    let prefix = "CUA ";
    let suffix = format!(" mp-{}-{nonce}", std::process::id());
    let max_cursor_bytes = EVDEV_UINPUT_NAME_MAX_BYTES
        .saturating_sub(prefix.len() + suffix.len() + UINPUT_POINTER_SUFFIX.len());
    let cursor_id = sanitize_device_name(cursor_id);
    format!(
        "{prefix}{}{suffix}",
        truncate_utf8(&cursor_id, max_cursor_bytes)
    )
}

fn slave_pointer_name(master_name: &str) -> String {
    format!("{master_name}{UINPUT_POINTER_SUFFIX}")
}

fn truncate_utf8(value: &str, max_bytes: usize) -> &str {
    let mut end = value.len().min(max_bytes);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn sanitize_device_name(value: &str) -> String {
    value
        .chars()
        .map(|ch| if ch.is_control() { '_' } else { ch })
        .collect()
}

fn normalize_uinput_device_name(name: &str) -> String {
    let sanitized = sanitize_device_name(name);
    truncate_utf8(&sanitized, EVDEV_UINPUT_NAME_MAX_BYTES).to_owned()
}

fn panic_payload_message(payload: &(dyn std::any::Any + Send)) -> &str {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("unknown panic")
}

pub(crate) fn uinput_unavailable(reason: impl Into<String>) -> anyhow::Error {
    UinputUnavailable {
        reason: reason.into(),
    }
    .into()
}

fn guarded_uinput_creation<T>(name: &str, create: impl FnOnce(&str) -> Result<T>) -> Result<T> {
    let name = normalize_uinput_device_name(name);
    match catch_unwind(AssertUnwindSafe(|| create(&name))) {
        Ok(Ok(device)) => Ok(device),
        Ok(Err(error)) => Err(uinput_unavailable(error.to_string())),
        Err(payload) => Err(uinput_unavailable(format!(
            "device creation panicked: {}",
            panic_payload_message(payload.as_ref())
        ))),
    }
}

pub fn is_uinput_unavailable(error: &anyhow::Error) -> bool {
    error.downcast_ref::<UinputUnavailable>().is_some()
}

fn master_pointer_device_name(master_name: &str) -> String {
    format!("{master_name} pointer")
}

fn master_keyboard_device_name(master_name: &str) -> String {
    format!("{master_name} keyboard")
}

fn open_display() -> Result<*mut x11::xlib::Display> {
    match XLIB_THREADS_READY.get_or_init(|| {
        let rc = unsafe { x11::xlib::XInitThreads() };
        if rc == 0 {
            Err("XInitThreads failed".to_owned())
        } else {
            Ok(())
        }
    }) {
        Ok(()) => {}
        Err(err) => bail!("{err}"),
    }
    let display = unsafe { x11::xlib::XOpenDisplay(ptr::null()) };
    if display.is_null() {
        bail!("XOpenDisplay returned null");
    }
    Ok(display)
}

fn xi2_query_devices(display: *mut x11::xlib::Display) -> Result<Vec<(i32, i32, String)>> {
    let mut count = 0;
    let ptr =
        unsafe { x11::xinput2::XIQueryDevice(display, x11::xinput2::XIAllDevices, &mut count) };
    if ptr.is_null() {
        bail!("XIQueryDevice returned null");
    }
    let mut out = Vec::new();
    for i in 0..count {
        let info = unsafe { *ptr.add(i as usize) };
        let name = if info.name.is_null() {
            String::new()
        } else {
            unsafe { CStr::from_ptr(info.name) }
                .to_string_lossy()
                .into_owned()
        };
        out.push((info.deviceid, info._use, name));
    }
    unsafe { x11::xinput2::XIFreeDeviceInfo(ptr) };
    Ok(out)
}

fn x_server_vendor(display: *mut x11::xlib::Display) -> String {
    let ptr = unsafe { x11::xlib::XServerVendor(display) };
    if ptr.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned()
}

fn supports_parallel_pointer_injection(display: *mut x11::xlib::Display) -> Result<()> {
    let vendor = x_server_vendor(display);
    if vendor.to_ascii_lowercase().contains("tigervnc") {
        bail!(
            "parallel_mouse_drag is not supported on this X server ('{vendor}'). \
             Xtigervnc exposes only its built-in VNC/XTEST devices, so Linux uinput/libinput \
             pointers cannot become real X input devices here."
        );
    }
    if is_xtigervnc_process_running() {
        let display_name = std::env::var("DISPLAY").unwrap_or_else(|_| "<unknown>".to_owned());
        bail!(
            "parallel_mouse_drag is not supported on display {display_name} because the active X server is Xtigervnc. \
             Xtigervnc exposes only its built-in VNC/XTEST devices, so Linux uinput/libinput pointers \
             cannot become real X input devices in this environment."
        );
    }
    Ok(())
}

fn is_xtigervnc_process_running() -> bool {
    let display = std::env::var("DISPLAY").unwrap_or_default();
    // Extract display number from DISPLAY (e.g., ":0" -> "0", "host:1.0" -> "1")
    let display_num = display
        .rsplit(':')
        .next()
        .unwrap_or("")
        .split('.')
        .next()
        .unwrap_or("")
        .trim();
    if display_num.is_empty() {
        return false;
    }
    // The X server writes its PID to the standard lock file /tmp/.X{N}-lock
    let lock_path = format!("/tmp/.X{display_num}-lock");
    let Ok(contents) = fs::read_to_string(&lock_path) else {
        return false;
    };
    let pid = contents.trim();
    if pid.is_empty() || !pid.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    // Check the executable path of the X server process directly
    if let Ok(exe) = fs::read_link(format!("/proc/{pid}/exe")) {
        return exe.file_name().and_then(|n| n.to_str()) == Some("Xtigervnc");
    }
    // Fallback: check the process name via comm (limited to 15 chars, but "Xtigervnc" fits)
    if let Ok(comm) = fs::read_to_string(format!("/proc/{pid}/comm")) {
        return comm.trim() == "Xtigervnc";
    }
    false
}

pub fn check_parallel_pointer_support() -> Result<()> {
    let display = open_display()?;
    let result = supports_parallel_pointer_injection(display);
    unsafe { x11::xlib::XCloseDisplay(display) };
    result
}

/// The file name of the X server binary backing `DISPLAY`, read from the PID in
/// the server's standard `/tmp/.X{N}-lock` file. Used to recognise servers that
/// can't expose uinput/libinput pointers as real X input slaves.
fn x_server_exe_name() -> Option<String> {
    let display = std::env::var("DISPLAY").unwrap_or_default();
    let display_num = display
        .rsplit(':')
        .next()
        .unwrap_or("")
        .split('.')
        .next()
        .unwrap_or("")
        .trim();
    if display_num.is_empty() {
        return None;
    }
    let lock_path = format!("/tmp/.X{display_num}-lock");
    let contents = fs::read_to_string(&lock_path).ok()?;
    let pid = contents.trim();
    if pid.is_empty() || !pid.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if let Ok(exe) = fs::read_link(format!("/proc/{pid}/exe")) {
        if let Some(name) = exe.file_name().and_then(|n| n.to_str()) {
            return Some(name.to_owned());
        }
    }
    fs::read_to_string(format!("/proc/{pid}/comm"))
        .ok()
        .map(|s| s.trim().to_owned())
}

/// True when `DISPLAY` is served by a headless Xvfb. Xvfb has no udev/libinput
/// hotplug, so a uinput device never becomes an X input slave — the whole MPX
/// real-input path (master pointer + uinput slave + shield grab) can never
/// work, and `ensure_master_pointer` would otherwise burn the 5 s slave-bind
/// timeout per attempt before failing.
fn is_xvfb_process_running() -> bool {
    x_server_exe_name().as_deref() == Some("Xvfb")
}

/// Cheap up-front probe (no device creation, no slave-bind wait) for whether the
/// no-focus-steal MPX real-input pointer path can work on this X server. Lets the
/// click/scroll tools decide whether to attempt the MPX path or go straight to
/// the legacy XSendEvent fallback, without paying the multi-second uinput
/// slave-bind timeout on servers (Xvfb, Xtigervnc) where it can never succeed.
///
/// NOTE: this only rules out the servers known to lack uinput→X-slave hotplug.
/// A `true` result means "worth attempting"; the per-action call still fails
/// gracefully (and the caller falls back) if the slave never binds.
fn real_pointer_capabilities_available(
    server_supported: bool,
    xvfb: bool,
    uinput_accessible: bool,
    unsafe_hotplug_session: bool,
) -> bool {
    server_supported && !xvfb && uinput_accessible && !unsafe_hotplug_session
}

fn nonempty(value: Option<&str>) -> bool {
    value.is_some_and(|value| !value.trim().is_empty())
}

fn desktop_value_is_kde(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        value
            .split([':', ';', ','])
            .map(str::trim)
            .any(|token| token.eq_ignore_ascii_case("kde") || token.eq_ignore_ascii_case("plasma"))
    })
}

/// KDE Plasma 6 / Qt 6.11 applications on X11 can crash session-wide when an
/// ephemeral uinput pointer is hotplugged into Xorg. Foreground input does not
/// need that device: the click, drag, scroll, and keyboard tools already use
/// XTEST after activating the target window. Disable only the MPX/uinput
/// capability here so callers retain their existing foreground escalation and
/// XSendEvent fallback behavior.
fn kde_x11_uinput_hotplug_is_unsafe(
    session_type: Option<&str>,
    current_desktop: Option<&str>,
    session_desktop: Option<&str>,
    desktop_session: Option<&str>,
    kde_full_session: Option<&str>,
    display: Option<&str>,
    wayland_display: Option<&str>,
) -> bool {
    let explicit_x11 = session_type.is_some_and(|value| value.eq_ignore_ascii_case("x11"));
    let explicit_wayland = session_type.is_some_and(|value| value.eq_ignore_ascii_case("wayland"));
    if explicit_wayland || (!explicit_x11 && nonempty(wayland_display)) {
        return false;
    }

    let x11 = explicit_x11 || nonempty(display);
    let kde = desktop_value_is_kde(current_desktop)
        || desktop_value_is_kde(session_desktop)
        || desktop_value_is_kde(desktop_session)
        || kde_full_session.is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes"
            )
        });

    x11 && kde
}

fn kde_x11_uinput_hotplug_is_unsafe_from_env() -> bool {
    let session_type = std::env::var("XDG_SESSION_TYPE").ok();
    let current_desktop = std::env::var("XDG_CURRENT_DESKTOP").ok();
    let session_desktop = std::env::var("XDG_SESSION_DESKTOP").ok();
    let desktop_session = std::env::var("DESKTOP_SESSION").ok();
    let kde_full_session = std::env::var("KDE_FULL_SESSION").ok();
    let display = std::env::var("DISPLAY").ok();
    let wayland_display = std::env::var("WAYLAND_DISPLAY").ok();

    kde_x11_uinput_hotplug_is_unsafe(
        session_type.as_deref(),
        current_desktop.as_deref(),
        session_desktop.as_deref(),
        desktop_session.as_deref(),
        kde_full_session.as_deref(),
        display.as_deref(),
        wayland_display.as_deref(),
    )
}

pub(crate) fn uinput_accessible() -> bool {
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/uinput")
        .is_ok()
}

pub fn real_pointer_input_available() -> bool {
    // Do not even probe /dev/uinput on an affected KDE/X11 session. Creating
    // the device is itself the dangerous operation; a later fallback is too
    // late once Xorg has announced the hotplug to Qt clients.
    if kde_x11_uinput_hotplug_is_unsafe_from_env() {
        return false;
    }

    // `ensure_master_pointer` creates an XI2 master before attaching the
    // uinput slave. If this process cannot open /dev/uinput, attempting that
    // path on every click/scroll would create and abandon an XInput master
    // pair until Xorg terminates the client with BadAlloc. Skip MPX entirely
    // when the required device is inaccessible.
    if !uinput_accessible() {
        return false;
    }
    let Ok(display) = open_display() else {
        return false;
    };
    let supported = real_pointer_capabilities_available(
        supports_parallel_pointer_injection(display).is_ok(),
        is_xvfb_process_running(),
        true,
        false,
    );
    unsafe { x11::xlib::XCloseDisplay(display) };
    supported
}

fn ensure_master_pointer(cursor_id: &str) -> Result<MasterPointerIds> {
    ensure_master_pointer_for_session(cursor_id, kde_x11_uinput_hotplug_is_unsafe_from_env())
}

fn ensure_master_pointer_for_session(
    cursor_id: &str,
    unsafe_hotplug_session: bool,
) -> Result<MasterPointerIds> {
    if unsafe_hotplug_session {
        return Err(uinput_unavailable(
            "disabled on KDE Plasma X11; retry with delivery_mode='foreground'",
        ));
    }

    if let Some(ids) = mpx_pointers().lock().unwrap().get(cursor_id).copied() {
        return Ok(ids);
    }

    let display = open_display()?;
    let mut major = 2;
    let mut minor = 3;
    let rc = unsafe { x11::xinput2::XIQueryVersion(display, &mut major, &mut minor) };
    if rc != 0 {
        unsafe { x11::xlib::XCloseDisplay(display) };
        bail!("XIQueryVersion failed with status {rc}");
    }

    let base = master_pointer_name(cursor_id);
    let device_name = slave_pointer_name(&base);
    // Acquire the non-X resource before mutating the XInput hierarchy. The
    // inexpensive availability probe above handles the normal permission
    // denial; this ordering also prevents a race or late open failure from
    // leaking a newly created master pair.
    let uinput_device = match create_uinput_pointer(&device_name) {
        Ok(device) => device,
        Err(error) => {
            unsafe { x11::xlib::XCloseDisplay(display) };
            return Err(error);
        }
    };
    let mut change = x11::xinput2::XIAnyHierarchyChangeInfo::default();
    let name = CString::new(base.clone())?;
    unsafe {
        let add = change.add();
        (*add)._type = x11::xinput2::XIAddMaster;
        (*add).name = name.as_ptr() as *mut _;
        // Core events stay on so core-only apps (xterm, Tk, …) receive the
        // drags too. Note this is not what makes the WM focus the dragged
        // window — XI2-aware WMs grab buttons for XIAllMasterDevices — see
        // the active-window save/restore in send_parallel_virtual_pointer_drags.
        (*add).send_core = 1;
        (*add).enable = 1;
    }
    let rc = unsafe { x11::xinput2::XIChangeHierarchy(display, &mut change, 1) };
    unsafe {
        x11::xlib::XSync(display, 0);
    }
    if rc != 0 {
        unsafe { x11::xlib::XCloseDisplay(display) };
        bail!("XIChangeHierarchy(XIAddMaster) failed with status {rc}");
    }

    let devices = xi2_query_devices(display)?;
    let mut pointer_id = None;
    let mut keyboard_id = None;
    let pointer_name = master_pointer_device_name(&base);
    let keyboard_name = master_keyboard_device_name(&base);
    for (device_id, use_, device_name) in devices {
        if use_ == x11::xinput2::XIMasterPointer && device_name == pointer_name {
            pointer_id = Some(device_id);
        } else if use_ == x11::xinput2::XIMasterKeyboard && device_name == keyboard_name {
            keyboard_id = Some(device_id);
        }
    }

    let pointer_id = pointer_id
        .ok_or_else(|| anyhow!("failed to locate created master pointer for '{cursor_id}'"))?;
    let keyboard_id = keyboard_id
        .ok_or_else(|| anyhow!("failed to locate created master keyboard for '{cursor_id}'"))?;

    let slave_pointer_id = match wait_for_slave_id(
        display,
        &device_name,
        x11::xinput2::XISlavePointer,
        Duration::from_secs(5),
    ) {
        Ok(id) => id,
        Err(error) => {
            // Never leave the fresh master pair behind when the slave never
            // hotplugs (no udev/libinput on this server): a stray master with
            // nothing attached is exactly the leak the startup reaper exists for.
            let _ = remove_master_pointer(display, pointer_id);
            unsafe { x11::xlib::XCloseDisplay(display) };
            return Err(error);
        }
    };
    attach_slave_to_master(display, slave_pointer_id, pointer_id)?;
    set_flat_pointer_accel(display, slave_pointer_id);
    unsafe { x11::xlib::XCloseDisplay(display) };

    let ids = MasterPointerIds {
        pointer_id,
        keyboard_id,
        _slave_pointer_id: slave_pointer_id,
        slave_keyboard_id: None,
    };
    mpx_pointers()
        .lock()
        .unwrap()
        .insert(cursor_id.to_owned(), ids);
    uinput_pointers()
        .lock()
        .unwrap()
        .insert(cursor_id.to_owned(), Arc::new(Mutex::new(uinput_device)));
    Ok(ids)
}

pub fn forget_master_pointer(cursor_id: &str) {
    mpx_last_use().lock().unwrap().remove(cursor_id);
    // Drop the uinput slaves first: closing the fds unplugs them, so the master
    // removal below never has to hand a live slave back to the user's core
    // devices (XIAttachToMaster only re-homes slaves that still exist).
    uinput_pointers().lock().unwrap().remove(cursor_id);
    mpx_keyboard::forget_uinput_keyboard(cursor_id);
    let Some(ids) = mpx_pointers().lock().unwrap().remove(cursor_id) else {
        return;
    };

    let Ok(display) = open_display() else {
        return;
    };
    let _ = remove_master_pointer(display, ids.pointer_id);
    unsafe { x11::xlib::XCloseDisplay(display) };
}

fn remove_master_pointer(display: *mut x11::xlib::Display, pointer_id: i32) -> Result<()> {
    let devices = xi2_query_devices(display)?;

    let mut virtual_core_pointer = None;
    let mut virtual_core_keyboard = None;
    for (device_id, use_, device_name) in devices {
        if device_name == "Virtual core pointer" && use_ == x11::xinput2::XIMasterPointer {
            virtual_core_pointer = Some(device_id);
        } else if device_name == "Virtual core keyboard" && use_ == x11::xinput2::XIMasterKeyboard {
            virtual_core_keyboard = Some(device_id);
        }
    }

    let (Some(return_pointer), Some(return_keyboard)) =
        (virtual_core_pointer, virtual_core_keyboard)
    else {
        bail!("cannot remove MPX master without its virtual core return devices");
    };

    let mut change = x11::xinput2::XIAnyHierarchyChangeInfo::default();
    unsafe {
        let remove = change.remove();
        (*remove)._type = x11::xinput2::XIRemoveMaster;
        (*remove).deviceid = pointer_id;
        (*remove).return_mode = x11::xinput2::XIAttachToMaster;
        (*remove).return_pointer = return_pointer;
        (*remove).return_keyboard = return_keyboard;
        let rc = x11::xinput2::XIChangeHierarchy(display, &mut change, 1);
        x11::xlib::XSync(display, 0);
        if rc != 0 {
            bail!("XIChangeHierarchy(XIRemoveMaster) failed with status {rc}");
        }
    }
    Ok(())
}

/// Recover only versioned masters whose local owner is provably gone. A PID
/// alone cannot identify an owner on a shared/remote X server or across restarts.
pub(crate) fn reap_orphaned_master_pointers() {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        return;
    }
    let Ok(owner) = mpx_owner::Owner::current() else {
        return;
    };
    let Ok(display) = open_display() else {
        return;
    };
    // Prevent an ID from being removed/reused by another X client between our
    // enumeration and removal. No network or arbitrary filesystem reads occur
    // under this grab: owner checks inspect local procfs and kill(pid, 0).
    unsafe {
        x11::xlib::XGrabServer(display);
    }
    // A panic while the server is grabbed would freeze every other X client
    // (the whole desktop) until this process dies. Catch it so the ungrab
    // below always runs, and report it like any other incomplete recovery.
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<()> {
        for (id, use_, name) in xi2_query_devices(display)? {
            if use_ != x11::xinput2::XIMasterPointer {
                continue;
            }
            let Some(candidate) = mpx_owner::Owner::from_pointer_name(&name) else {
                continue;
            };
            if candidate.stale_in(&owner) {
                remove_master_pointer(display, id)?;
                tracing::info!(device_id = id, "removed orphaned Cua MPX master pair");
            }
        }
        Ok(())
    }));
    unsafe {
        x11::xlib::XUngrabServer(display);
        x11::xlib::XSync(display, 0);
        x11::xlib::XCloseDisplay(display);
    }
    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::warn!("MPX orphan recovery incomplete: {error}"),
        Err(payload) => tracing::warn!(
            "MPX orphan recovery panicked (server grab released): {}",
            panic_payload_message(payload.as_ref())
        ),
    }
}

fn create_uinput_pointer(name: &str) -> Result<VirtualDevice> {
    guarded_uinput_creation(name, |name| {
        let mut keys = AttributeSet::<Key>::new();
        keys.insert(Key::BTN_LEFT);
        keys.insert(Key::BTN_RIGHT);
        keys.insert(Key::BTN_MIDDLE);

        let mut rel_axes = AttributeSet::<RelativeAxisType>::new();
        rel_axes.insert(RelativeAxisType::REL_X);
        rel_axes.insert(RelativeAxisType::REL_Y);
        // REL_WHEEL (vertical) and REL_HWHEEL (horizontal) so the same uinput
        // slave can also drive scroll: libinput turns these into the XI2
        // smooth-scroll events GTK consumes, where synthetic Button4-7
        // XSendEvents are dropped.
        rel_axes.insert(RelativeAxisType::REL_WHEEL);
        rel_axes.insert(RelativeAxisType::REL_HWHEEL);

        Ok(evdev::uinput::VirtualDeviceBuilder::new()?
            .name(name)
            .with_keys(&keys)?
            .with_relative_axes(&rel_axes)?
            .build()?)
    })
}

/// Poll `XIQueryDevice` until the X server has hot-added the uinput device
/// named `device_name` as a slave of kind `use_` (`XISlavePointer` /
/// `XISlaveKeyboard`). udev + xf86-input-libinput do the hotplug on a real
/// Xorg; Xvfb/Xtigervnc never will, which is what the timeout covers.
fn wait_for_slave_id(
    display: *mut x11::xlib::Display,
    device_name: &str,
    use_: i32,
    timeout: Duration,
) -> Result<i32> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        for (device_id, seen_use, seen_name) in xi2_query_devices(display)? {
            if seen_use == use_ && seen_name == device_name {
                return Ok(device_id);
            }
        }
        if std::time::Instant::now() >= deadline {
            bail!("timed out waiting for X input slave device '{device_name}'");
        }
        sleep(Duration::from_millis(50));
    }
}

fn attach_slave_to_master(
    display: *mut x11::xlib::Display,
    slave_pointer_id: i32,
    master_pointer_id: i32,
) -> Result<()> {
    let mut change = x11::xinput2::XIAnyHierarchyChangeInfo::default();
    unsafe {
        let attach = change.attach();
        (*attach)._type = x11::xinput2::XIAttachSlave;
        (*attach).deviceid = slave_pointer_id;
        (*attach).new_master = master_pointer_id;
    }
    let rc = unsafe { x11::xinput2::XIChangeHierarchy(display, &mut change, 1) };
    unsafe { x11::xlib::XSync(display, 0) };
    if rc != 0 {
        bail!("XIChangeHierarchy(XIAttachSlave) failed with status {rc}");
    }
    Ok(())
}

fn set_flat_pointer_accel(display: *mut x11::xlib::Display, slave_pointer_id: i32) {
    // Pin libinput's accel profile to flat so relative deltas map 1:1 onto
    // cursor movement — the default adaptive profile rescales small deltas
    // and makes drag endpoints drift off-target by a few pixels.
    // Best-effort: the property only exists under xf86-input-libinput.
    unsafe {
        let prop = x11::xlib::XInternAtom(
            display,
            c"libinput Accel Profile Enabled".as_ptr(),
            x11::xlib::True,
        );
        if prop == 0 {
            return;
        }
        let mut type_ret: x11::xlib::Atom = 0;
        let mut format_ret: std::os::raw::c_int = 0;
        let mut num_items: std::os::raw::c_ulong = 0;
        let mut bytes_after: std::os::raw::c_ulong = 0;
        let mut data: *mut std::os::raw::c_uchar = std::ptr::null_mut();
        let rc = x11::xinput2::XIGetProperty(
            display,
            slave_pointer_id,
            prop,
            0,
            16,
            x11::xlib::False,
            x11::xlib::AnyPropertyType as x11::xlib::Atom,
            &mut type_ret,
            &mut format_ret,
            &mut num_items,
            &mut bytes_after,
            &mut data,
        );
        if rc != x11::xlib::Success as i32 || data.is_null() {
            return;
        }
        // Profile order is (adaptive, flat[, custom]); enable flat only.
        if format_ret == 8 && (2..=8).contains(&num_items) {
            let mut values = vec![0u8; num_items as usize];
            values[1] = 1;
            x11::xinput2::XIChangeProperty(
                display,
                slave_pointer_id,
                prop,
                type_ret,
                8,
                x11::xlib::PropModeReplace,
                values.as_mut_ptr(),
                num_items as std::os::raw::c_int,
            );
            x11::xlib::XSync(display, 0);
        }
        x11::xlib::XFree(data as *mut _);
    }
}

fn warp_master_pointer(
    display: *mut x11::xlib::Display,
    ids: MasterPointerIds,
    x: i32,
    y: i32,
) -> Result<()> {
    let root = unsafe { x11::xlib::XDefaultRootWindow(display) };
    let rc = unsafe {
        x11::xinput2::XIWarpPointer(
            display,
            ids.pointer_id,
            0,
            root,
            0.0,
            0.0,
            0,
            0,
            x as f64,
            y as f64,
        )
    };
    // XSync (not XFlush): the button press that follows is emitted through
    // uinput on a separate kernel pipeline, and races ahead of a merely
    // queued warp request. Once XSync returns the server has executed the
    // warp, so the press lands at the warped position.
    unsafe { x11::xlib::XSync(display, 0) };
    if rc != 0 {
        bail!("XIWarpPointer failed with status {rc}");
    }
    Ok(())
}

/// True when `window` is override-redirect — a menu, tooltip, or other popup
/// the WM does not manage. Such a window takes its own active pointer grab, so
/// the shield-grab-and-replay dance cannot deliver into it; a plain warp+press
/// on the virtual master is what reaches it (and there is no WM focus to steal).
fn is_override_redirect(display: *mut x11::xlib::Display, window: x11::xlib::Window) -> bool {
    let mut attrs: x11::xlib::XWindowAttributes = unsafe { std::mem::zeroed() };
    let previous_handler = unsafe { x11::xlib::XSetErrorHandler(Some(ignore_x_error)) };
    let rc = unsafe { x11::xlib::XGetWindowAttributes(display, window, &mut attrs) };
    unsafe { x11::xlib::XSetErrorHandler(previous_handler) };
    rc != 0 && attrs.override_redirect != 0
}

/// Direct child of the root window under screen point `(x, y)`: the WM frame
/// of a managed toplevel, or an override-redirect popup (menu, tooltip).
fn root_child_under_point(
    display: *mut x11::xlib::Display,
    x: i32,
    y: i32,
) -> Option<x11::xlib::Window> {
    let root = unsafe { x11::xlib::XDefaultRootWindow(display) };
    let mut child: x11::xlib::Window = 0;
    let mut dx = 0;
    let mut dy = 0;
    let rc = unsafe {
        x11::xlib::XTranslateCoordinates(display, root, root, x, y, &mut dx, &mut dy, &mut child)
    };
    (rc != 0 && child != 0).then_some(child)
}

/// The root child (WM frame or the window itself) that contains `window`.
fn root_child_of(
    display: *mut x11::xlib::Display,
    window: x11::xlib::Window,
) -> Option<x11::xlib::Window> {
    let root = unsafe { x11::xlib::XDefaultRootWindow(display) };
    let previous_handler = unsafe { x11::xlib::XSetErrorHandler(Some(ignore_x_error)) };
    let mut current = window;
    let mut result = None;
    for _ in 0..64 {
        let mut root_ret: x11::xlib::Window = 0;
        let mut parent: x11::xlib::Window = 0;
        let mut children: *mut x11::xlib::Window = ptr::null_mut();
        let mut count: std::os::raw::c_uint = 0;
        let rc = unsafe {
            x11::xlib::XQueryTree(
                display,
                current,
                &mut root_ret,
                &mut parent,
                &mut children,
                &mut count,
            )
        };
        if !children.is_null() {
            unsafe { x11::xlib::XFree(children as *mut _) };
        }
        if rc == 0 || parent == 0 {
            break;
        }
        if parent == root {
            result = Some(current);
            break;
        }
        current = parent;
    }
    unsafe {
        x11::xlib::XSync(display, 0);
        x11::xlib::XSetErrorHandler(previous_handler);
    }
    result
}

pub(super) fn xi_mask_len() -> usize {
    (x11::xinput2::XI_LASTEVENT as usize >> 3) + 1
}

/// Look up the XInputExtension major opcode so we can recognise its
/// GenericEvent cookies on the display connection.
fn xinput_opcode(display: *mut x11::xlib::Display) -> Option<std::os::raw::c_int> {
    let name = match CString::new("XInputExtension") {
        Ok(n) => n,
        Err(_) => return None,
    };
    let mut opcode = 0;
    let mut event = 0;
    let mut error = 0;
    let present = unsafe {
        x11::xlib::XQueryExtension(display, name.as_ptr(), &mut opcode, &mut event, &mut error)
    };
    if present != 0 {
        Some(opcode)
    } else {
        None
    }
}

/// Modifier evdev codes the virtual keyboard may hold mid-chord.
const MODIFIER_EVDEV_CODES: [u16; 8] = [42, 54, 29, 97, 56, 100, 125, 126];

/// Buttons (1..=3) currently held on the master pointer `pointer_id`, plus the
/// paired master keyboard's modifier state, via `XIQueryPointer`.
fn virtual_master_input_state(
    display: *mut x11::xlib::Display,
    pointer_id: i32,
) -> (Vec<u8>, x11::xinput2::XIModifierState) {
    let root = unsafe { x11::xlib::XDefaultRootWindow(display) };
    let mut root_ret = 0;
    let mut child_ret = 0;
    let (mut rx, mut ry, mut wx, mut wy) = (0f64, 0f64, 0f64, 0f64);
    let mut buttons = x11::xinput2::XIButtonState::default();
    let mut mods = x11::xinput2::XIModifierState::default();
    let mut group = x11::xinput2::XIModifierState::default();
    let prev = unsafe { x11::xlib::XSetErrorHandler(Some(ignore_x_error)) };
    let rc = unsafe {
        x11::xinput2::XIQueryPointer(
            display,
            pointer_id,
            root,
            &mut root_ret,
            &mut child_ret,
            &mut rx,
            &mut ry,
            &mut wx,
            &mut wy,
            &mut buttons,
            &mut mods,
            &mut group,
        )
    };
    unsafe { x11::xlib::XSetErrorHandler(prev) };
    let mut held = Vec::new();
    if rc != 0 && !buttons.mask.is_null() && buttons.mask_len > 0 {
        let mask = unsafe { std::slice::from_raw_parts(buttons.mask, buttons.mask_len as usize) };
        for button in 1u8..=3 {
            let byte = (button / 8) as usize;
            if mask.get(byte).is_some_and(|b| b & (1 << (button % 8)) != 0) {
                held.push(button);
            }
        }
        unsafe { x11::xlib::XFree(buttons.mask as *mut _) };
    }
    (held, mods)
}

/// Release anything the session's virtual master still holds from an earlier
/// aborted action: pointer buttons (a shield replay that timed out returned
/// before the release) and keyboard modifiers (a chord interrupted mid-way).
///
/// A held button is not cosmetic: every key event from the paired virtual
/// keyboard then carries `Button1Mask` (bit 8) in its core `state`, which
/// at-spi2 forwards verbatim and Orca reads as its `ORCA_MODIFIER_MASK`
/// (`1 << 8`) — a plain space becomes "Orca+space" and opens the Screen
/// Reader Preferences. Returns what was released, for the tool's report.
fn release_stuck_virtual_input(
    cursor_id: &str,
    display: *mut x11::xlib::Display,
    ids: MasterPointerIds,
    keyboard: &Mutex<VirtualDevice>,
) -> Vec<String> {
    let mut released = Vec::new();
    let (held, mods) = virtual_master_input_state(display, ids.pointer_id);
    if !held.is_empty() {
        if let Some(pointer) = uinput_pointers().lock().unwrap().get(cursor_id).cloned() {
            let mut pointer = pointer.lock().unwrap();
            for &button in &held {
                if emit_button(&mut pointer, button, false).is_ok() {
                    released.push(format!("button{button}"));
                }
            }
        }
    }
    if mods.base != 0 {
        let mut keyboard = keyboard.lock().unwrap();
        let mut any = false;
        for code in MODIFIER_EVDEV_CODES {
            any |= keyboard
                .emit(&[InputEvent::new(EventType::KEY, code, 0)])
                .is_ok();
        }
        if any {
            released.push(format!("modifiers(base=0x{:x})", mods.base));
        }
    }
    if mods.locked != 0 || mods.latched != 0 {
        // A locked/latched modifier on the virtual master (CapsLock leaked
        // from an interrupted chord) would shift every later character.
        unsafe {
            let prev = x11::xlib::XSetErrorHandler(Some(ignore_x_error));
            x11::xlib::XkbLockModifiers(display, ids.keyboard_id as u32, 0xff, 0);
            x11::xlib::XkbLatchModifiers(display, ids.keyboard_id as u32, 0xff, 0);
            x11::xlib::XSetErrorHandler(prev);
        }
        released.push(format!(
            "locked/latched modifiers (0x{:x}/0x{:x})",
            mods.locked, mods.latched
        ));
    }
    if !released.is_empty() {
        unsafe { x11::xlib::XSync(display, 0) };
        tracing::warn!(cursor_id, ?released, "released stuck virtual input state");
        sleep(Duration::from_millis(20));
    }
    released
}

/// Release a device frozen by a synchronous grab; harmless when not frozen.
fn thaw_device(display: *mut x11::xlib::Display, device_id: i32) {
    unsafe {
        let prev = x11::xlib::XSetErrorHandler(Some(ignore_x_error));
        x11::xinput2::XIAllowEvents(
            display,
            device_id,
            x11::xinput2::XIAsyncDevice,
            x11::xlib::CurrentTime,
        );
        x11::xlib::XSync(display, 0);
        x11::xlib::XSetErrorHandler(prev);
    }
}

fn ewmh_active_window(display: *mut x11::xlib::Display) -> Option<x11::xlib::Window> {
    unsafe {
        let atom = x11::xlib::XInternAtom(display, c"_NET_ACTIVE_WINDOW".as_ptr(), x11::xlib::True);
        if atom == 0 {
            return None;
        }
        let root = x11::xlib::XDefaultRootWindow(display);
        let mut type_ret: x11::xlib::Atom = 0;
        let mut format_ret: std::os::raw::c_int = 0;
        let mut nitems: std::os::raw::c_ulong = 0;
        let mut bytes_after: std::os::raw::c_ulong = 0;
        let mut data: *mut std::os::raw::c_uchar = std::ptr::null_mut();
        let rc = x11::xlib::XGetWindowProperty(
            display,
            root,
            atom,
            0,
            1,
            x11::xlib::False,
            x11::xlib::XA_WINDOW,
            &mut type_ret,
            &mut format_ret,
            &mut nitems,
            &mut bytes_after,
            &mut data,
        );
        if rc != x11::xlib::Success as i32 || data.is_null() {
            return None;
        }
        let window = if nitems >= 1 && format_ret == 32 {
            Some(*(data as *const std::os::raw::c_ulong) as x11::xlib::Window)
        } else {
            None
        };
        x11::xlib::XFree(data as *mut _);
        window.filter(|w| *w != 0)
    }
}

/// Current X server time via the standard PropertyNotify round-trip.
/// EWMH activation requests stamped CurrentTime(0) lose to the WM's
/// focus-stealing prevention whenever any newer input exists.
fn x_server_time(display: *mut x11::xlib::Display) -> x11::xlib::Time {
    unsafe {
        let root = x11::xlib::XDefaultRootWindow(display);
        let win = x11::xlib::XCreateSimpleWindow(display, root, -1, -1, 1, 1, 0, 0, 0);
        x11::xlib::XSelectInput(display, win, x11::xlib::PropertyChangeMask);
        let atom = x11::xlib::XInternAtom(display, c"CUA_TIME_PROBE".as_ptr(), x11::xlib::False);
        x11::xlib::XChangeProperty(
            display,
            win,
            atom,
            x11::xlib::XA_STRING,
            8,
            x11::xlib::PropModeReplace,
            [0u8].as_ptr(),
            0,
        );
        x11::xlib::XSync(display, 0);
        let mut time: x11::xlib::Time = x11::xlib::CurrentTime;
        let mut ev: x11::xlib::XEvent = std::mem::zeroed();
        while x11::xlib::XCheckWindowEvent(display, win, x11::xlib::PropertyChangeMask, &mut ev)
            != 0
        {
            if ev.get_type() == x11::xlib::PropertyNotify {
                time = ev.property.time;
            }
        }
        x11::xlib::XDestroyWindow(display, win);
        x11::xlib::XFlush(display);
        time
    }
}

fn ewmh_activate_window(
    display: *mut x11::xlib::Display,
    window: x11::xlib::Window,
    current_active: x11::xlib::Window,
) {
    unsafe {
        let atom = x11::xlib::XInternAtom(display, c"_NET_ACTIVE_WINDOW".as_ptr(), x11::xlib::True);
        if atom == 0 {
            return;
        }
        let root = x11::xlib::XDefaultRootWindow(display);
        let mut ev: x11::xlib::XClientMessageEvent = std::mem::zeroed();
        ev.type_ = x11::xlib::ClientMessage;
        ev.window = window;
        ev.message_type = atom;
        ev.format = 32;
        ev.data.set_long(0, 2); // source indication: pager/tool
        ev.data
            .set_long(1, x_server_time(display) as std::os::raw::c_long);
        ev.data.set_long(2, current_active as std::os::raw::c_long);
        x11::xlib::XSendEvent(
            display,
            root,
            x11::xlib::False,
            x11::xlib::SubstructureRedirectMask | x11::xlib::SubstructureNotifyMask,
            &mut ev as *mut _ as *mut x11::xlib::XEvent,
        );
        x11::xlib::XSync(display, 0);
    }
}

/// Foreground rung for X11 (`delivery_mode:"foreground"`): activate `xid`
/// (EWMH `_NET_ACTIVE_WINDOW` + core input focus), confirm the transition
/// against both the WM's active window and the X input-focus tree, then run
/// `body` (which injects the input while the window holds focus). The target
/// is left active afterwards; see [`foreground`] for the rationale and the
/// deadline/watchdog behaviour. `settle_ms` is a minimum confirmation budget.
pub fn with_x11_foreground<T>(
    xid: u64,
    settle_ms: u64,
    body: impl FnOnce() -> Result<T>,
) -> Result<T> {
    with_x11_foreground_opts(xid, ForegroundOptions::from_settle_hint(settle_ms), body)
        .map(|(value, _report)| value)
}

/// Activate `xid` and LEAVE it active (no restore) — the persistent foreground
/// swap behind `bring_to_front`. Returns the window that was active before, so
/// the caller can report/inspect it. Best-effort; returns `None` prior on a
/// headless display.
pub fn x11_activate_window_persistent(xid: u64) -> Result<Option<u64>> {
    let display = unsafe { x11::xlib::XOpenDisplay(ptr::null()) };
    if display.is_null() {
        bail!(
            "cannot activate window: no X display (DISPLAY={:?})",
            std::env::var("DISPLAY").ok()
        );
    }
    let prior = ewmh_active_window(display).map(|w| w as u64);
    ewmh_activate_window(
        display,
        xid as x11::xlib::Window,
        prior.unwrap_or(0) as x11::xlib::Window,
    );
    unsafe {
        // Some WMs honor `_NET_ACTIVE_WINDOW` as raise-only. Persistent
        // activation must establish input focus too, just like the bounded
        // foreground rung above, or `bring_to_front` can report success while
        // keyboard focus remains on the previous window.
        let previous_handler = x11::xlib::XSetErrorHandler(Some(ignore_x_error));
        x11::xlib::XSetInputFocus(
            display,
            xid as x11::xlib::Window,
            x11::xlib::RevertToParent,
            x11::xlib::CurrentTime,
        );
        x11::xlib::XSync(display, 0);
        x11::xlib::XSetErrorHandler(previous_handler);
        x11::xlib::XCloseDisplay(display);
    }
    Ok(prior)
}

fn button_code(button: u8) -> Result<Key> {
    match button {
        1 => Ok(Key::BTN_LEFT),
        2 => Ok(Key::BTN_MIDDLE),
        3 => Ok(Key::BTN_RIGHT),
        _ => bail!("unsupported button {button} for uinput pointer"),
    }
}

fn emit_button(device: &mut VirtualDevice, button: u8, press: bool) -> Result<()> {
    let code = button_code(button)?;
    device.emit(&[InputEvent::new(
        EventType::KEY,
        code.0,
        if press { 1 } else { 0 },
    )])?;
    Ok(())
}

/// Emit a release for `button` regardless of state; the kernel drops a
/// release for a button that is not held, so this is safe to call on every
/// exit path of a press train or gesture.
fn release_button_best_effort(device: &Arc<Mutex<VirtualDevice>>, button: u8) {
    if let Ok(mut device) = device.lock() {
        if let Err(error) = emit_button(&mut device, button, false) {
            tracing::warn!("virtual pointer button {button} release failed: {error:#}");
        }
    }
}

fn emit_relative_motion(device: &mut VirtualDevice, dx: i32, dy: i32) -> Result<()> {
    let mut events = Vec::with_capacity(2);
    if dx != 0 {
        events.push(InputEvent::new(
            EventType::RELATIVE,
            RelativeAxisType::REL_X.0,
            dx,
        ));
    }
    if dy != 0 {
        events.push(InputEvent::new(
            EventType::RELATIVE,
            RelativeAxisType::REL_Y.0,
            dy,
        ));
    }
    if events.is_empty() {
        return Ok(());
    }
    device.emit(&events)?;
    Ok(())
}

/// Emit one wheel detent on the uinput slave. `horizontal` selects REL_HWHEEL
/// (positive = right) over REL_WHEEL (positive = up); `value` is the signed
/// detent count. libinput translates these into the XI2 scroll events GTK reads.
fn emit_scroll(device: &mut VirtualDevice, horizontal: bool, value: i32) -> Result<()> {
    let axis = if horizontal {
        RelativeAxisType::REL_HWHEEL
    } else {
        RelativeAxisType::REL_WHEEL
    };
    device.emit(&[InputEvent::new(EventType::RELATIVE, axis.0, value)])?;
    Ok(())
}

pub fn send_parallel_virtual_pointer_drags(drags: &[(String, VirtualPointerDrag)]) -> Result<()> {
    let _op = mpx_op_guard(drags.first().map(|(id, _)| id.as_str()).unwrap_or("default"));
    {
        let mut last_use = mpx_last_use().lock().unwrap();
        for (cursor_id, _) in drags {
            last_use.insert(cursor_id.clone(), std::time::Instant::now());
        }
    }
    let display = open_display()?;
    supports_parallel_pointer_injection(display)?;

    struct ActiveDrag {
        cursor_id: String,
        ids: MasterPointerIds,
        device: Arc<Mutex<VirtualDevice>>,
        drag: VirtualPointerDrag,
        cum: Vec<f64>,
        total: f64,
        steps: usize,
        step_delay: Duration,
        current_step: usize,
        next_at: std::time::Instant,
        last_x: i32,
        last_y: i32,
    }

    let start_at = std::time::Instant::now() + Duration::from_millis(120);
    let mut active = Vec::with_capacity(drags.len());

    // A click-to-focus WM that grabs buttons for every master device would
    // activate the target on the press. Remember the focus state and hand it
    // back afterwards so parallel drags don't steal it (mutter grabs for the
    // Virtual Core Pointer only, so there the restore is a no-op).
    let saved_focus = save_focus_state(display);

    let result = (|| -> Result<()> {
        for (cursor_id, drag) in drags {
            let ids = ensure_master_pointer(cursor_id)?;
            let device = uinput_pointers()
                .lock()
                .unwrap()
                .get(cursor_id)
                .cloned()
                .ok_or_else(|| anyhow!("missing uinput pointer for '{cursor_id}'"))?;
            let (cum, total) = path_cumulative(&drag.path);
            let start = *drag.path.first().unwrap_or(&(0, 0));
            active.push(ActiveDrag {
                cursor_id: cursor_id.clone(),
                ids,
                device,
                drag: drag.clone(),
                cum,
                total,
                steps: drag.steps.max(1),
                step_delay: if drag.steps.max(1) > 1 {
                    Duration::from_millis(drag.duration_ms / drag.steps.max(1) as u64)
                } else {
                    Duration::from_millis(drag.duration_ms)
                },
                current_step: 0,
                next_at: start_at,
                last_x: start.0,
                last_y: start.1,
            });
        }

        let now = std::time::Instant::now();
        if start_at > now {
            std::thread::sleep(start_at - now);
        }

        // Press each drag straight from its virtual master (see
        // `send_virtual_pointer_click` for why there is no XI2 shield grab:
        // on this server the grab-and-replay swallowed the press). The press
        // point must not be covered by another toplevel.
        for item in &active {
            let start = *item.drag.path.first().unwrap_or(&(0, 0));
            if let PointCover::Occluded(occluded) = occluding_window(
                display,
                item.drag.target_window as x11::xlib::Window,
                start.0,
                start.1,
            )? {
                return Err(occluded.into());
            }
            thaw_device(display, item.ids.pointer_id);
            warp_master_pointer(display, item.ids, start.0, start.1)?;
            {
                let mut device = item.device.lock().unwrap();
                emit_button(&mut device, item.drag.button, true)?;
            }
        }
        sleep(Duration::from_millis(40));

        while active.iter().any(|item| item.current_step < item.steps) {
            let now = std::time::Instant::now();
            let mut advanced = false;
            let mut next_deadline = None;

            for item in &mut active {
                if item.current_step >= item.steps {
                    continue;
                }
                if now >= item.next_at {
                    item.current_step += 1;
                    let t = item.current_step as f64 / item.steps as f64;
                    let (ix, iy) = point_on_path(&item.drag.path, &item.cum, item.total, t);
                    let dx = ix - item.last_x;
                    let dy = iy - item.last_y;
                    if dx != 0 || dy != 0 {
                        let mut device = item.device.lock().unwrap();
                        emit_relative_motion(&mut device, dx, dy)?;
                        // Keep the agent cursor overlay tracking the drag so
                        // the gesture is visible, not just its endpoints.
                        crate::overlay::send_command_for(
                            item.cursor_id.clone(),
                            cursor_overlay::OverlayCommand::SnapTo {
                                x: ix as f64,
                                y: iy as f64,
                                heading_radians: Some((dy as f64).atan2(dx as f64)),
                            },
                        );
                    }
                    item.last_x = ix;
                    item.last_y = iy;
                    item.next_at = now + item.step_delay;
                    advanced = true;
                }
                if item.current_step < item.steps {
                    next_deadline = Some(match next_deadline {
                        Some(deadline) => std::cmp::min(deadline, item.next_at),
                        None => item.next_at,
                    });
                }
            }

            if !advanced {
                if let Some(deadline) = next_deadline {
                    let now = std::time::Instant::now();
                    if deadline > now {
                        std::thread::sleep(deadline - now);
                    }
                }
            }
        }

        for item in &active {
            let mut device = item.device.lock().unwrap();
            emit_button(&mut device, item.drag.button, false)?;
        }
        Ok(())
    })();
    // Whatever happened above, no virtual master may keep a button held
    // (a stuck Button1 poisons the core modifier state for later chords).
    for (cursor_id, drag) in drags {
        if let Some(device) = uinput_pointers().lock().unwrap().get(cursor_id).cloned() {
            release_button_best_effort(&device, drag.button);
        }
    }
    // The per-session master pair is retained for reuse (torn down on
    // end_session / idle / startup reap). Creating and destroying an XI2 master
    // plus hot-plugging a uinput slave on every call churns the XInput
    // hierarchy hard enough to crash fragile toolkits (LibreOffice VCL); one
    // long-lived pair per session avoids that and is cheaper. The focus is
    // still saved and restored around each gesture.
    let _ = drags;
    restore_focus_state(display, &saved_focus);
    unsafe {
        x11::xlib::XCloseDisplay(display);
    }
    result
}

/// A discrete no-focus-steal pointer click driven through the same real-input
/// pipeline as [`send_parallel_virtual_pointer_drags`] — MPX master pointer +
/// uinput slave — reduced to a press/release (or a short press/release train
/// for `count` > 1) at one screen point.
///
/// This is what lands **right / middle / double** clicks (and any left click
/// the AT-SPI path can't actuate) on XInput2 toolkits: GTK3/4, VCL, Qt and
/// Chromium silently drop synthetic `XSendEvent` pointer events, so those
/// clicks are otherwise no-ops. Coordinates are screen-absolute;
/// `target_window` is the X11 toplevel the caller means to hit. The press is
/// delivered straight from the virtual master: no XI2 shield grab. On GNOME
/// (mutter) the click-to-focus passive grab is installed for the Virtual Core
/// Pointer only, so a second master's press never activates the window; the
/// grab-and-replay "shield" that was meant to hide the press from the WM
/// instead swallowed it on this server (the replayed press never reached the
/// application) and left the device frozen. Focus is still saved and restored
/// around the click for WMs that do grab every master device. `button` is an
/// X button number (1=left, 2=middle, 3=right); `count` >= 1 (2 = double-click).
#[derive(Clone, Debug)]
pub struct VirtualPointerClick {
    pub target_window: u64,
    pub x: i32,
    pub y: i32,
    pub button: u8,
    pub count: usize,
}

/// The screen point the caller aimed at is covered by another toplevel, so a
/// real pointer press there would land on the covering window, not the
/// target. Refused before any input is sent.
#[derive(Debug, Clone)]
pub struct TargetOccluded {
    pub target_window: u64,
    pub covering_window: u64,
    pub covering_title: String,
    pub covering_pid: Option<u32>,
    pub x: i32,
    pub y: i32,
}

impl std::fmt::Display for TargetOccluded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "screen point ({}, {}) over window {} is covered by window {}{}{}; a real pointer \
             press there would land on the covering window",
            self.x,
            self.y,
            self.target_window,
            self.covering_window,
            if self.covering_title.is_empty() {
                String::new()
            } else {
                format!(" \"{}\"", self.covering_title)
            },
            self.covering_pid
                .map(|pid| format!(" (pid {pid})"))
                .unwrap_or_default(),
        )
    }
}
impl std::error::Error for TargetOccluded {}

/// A mapped override-redirect toplevel: a popup menu, popover, combo list
/// or tooltip. The WM does not manage it, so it is absent from
/// `list_windows`; the pointer tools name it so a caller can walk it with
/// `get_window_state(pid, window_id=<window>)`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PopupWindow {
    pub window: u64,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub title: String,
    pub pid: Option<u32>,
}

impl PopupWindow {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "window_id": self.window,
            "bounds": { "x": self.x, "y": self.y, "width": self.width, "height": self.height },
            "title": self.title,
            "pid": self.pid,
        })
    }

    /// `popup window 12345678 "Edit" (220x340 at 410,220)`.
    pub fn describe(&self) -> String {
        format!(
            "popup window {}{} ({}x{} at {},{})",
            self.window,
            if self.title.is_empty() {
                String::new()
            } else {
                format!(" \"{}\"", self.title)
            },
            self.width,
            self.height,
            self.x,
            self.y
        )
    }
}

/// The caller's window-local point does not lie inside the window: a
/// coordinate-frame mistake (screen pixels passed as window pixels, or the
/// wrong window_id), refused before any input is sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PointOutsideWindow {
    pub target_window: u64,
    pub screen_x: i32,
    pub screen_y: i32,
    pub window_x: i32,
    pub window_y: i32,
    /// Screen-space `(x, y, width, height)` of the window.
    pub bounds: (i32, i32, u32, u32),
}

impl std::fmt::Display for PointOutsideWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (bx, by, bw, bh) = self.bounds;
        write!(
            f,
            "window-local point ({}, {}) of window {} resolves to screen ({}, {}), outside \
             the window's bounds (x={bx}, y={by}, {bw}x{bh}); no input was sent",
            self.window_x, self.window_y, self.target_window, self.screen_x, self.screen_y
        )
    }
}
impl std::error::Error for PointOutsideWindow {}

/// Screen-space `(x, y, width, height)` of `window`, or `None` when it is
/// gone.
fn window_screen_bounds(
    display: *mut x11::xlib::Display,
    window: x11::xlib::Window,
) -> Option<(i32, i32, u32, u32)> {
    let previous_handler = unsafe { x11::xlib::XSetErrorHandler(Some(ignore_x_error)) };
    let mut root: x11::xlib::Window = 0;
    let (mut gx, mut gy) = (0i32, 0i32);
    let (mut width, mut height, mut border, mut depth) = (0u32, 0u32, 0u32, 0u32);
    let rc = unsafe {
        x11::xlib::XGetGeometry(
            display, window, &mut root, &mut gx, &mut gy, &mut width, &mut height, &mut border,
            &mut depth,
        )
    };
    let mut result = None;
    if rc != 0 && root != 0 {
        let (mut dx, mut dy) = (0i32, 0i32);
        let mut child: x11::xlib::Window = 0;
        let ok = unsafe {
            x11::xlib::XTranslateCoordinates(display, window, root, 0, 0, &mut dx, &mut dy, &mut child)
        };
        if ok != 0 {
            result = Some((dx, dy, width, height));
        }
    }
    unsafe {
        x11::xlib::XSync(display, 0);
        x11::xlib::XSetErrorHandler(previous_handler);
    }
    result
}

fn bounds_contain(bounds: (i32, i32, u32, u32), x: i32, y: i32) -> bool {
    let (bx, by, bw, bh) = bounds;
    x >= bx && y >= by && x < bx.saturating_add(bw as i32) && y < by.saturating_add(bh as i32)
}

/// Describe a mapped override-redirect root child as a [`PopupWindow`].
fn popup_info(display: *mut x11::xlib::Display, child: x11::xlib::Window) -> Option<PopupWindow> {
    let mut attrs: x11::xlib::XWindowAttributes = unsafe { std::mem::zeroed() };
    let previous_handler = unsafe { x11::xlib::XSetErrorHandler(Some(ignore_x_error)) };
    let rc = unsafe { x11::xlib::XGetWindowAttributes(display, child, &mut attrs) };
    unsafe {
        x11::xlib::XSync(display, 0);
        x11::xlib::XSetErrorHandler(previous_handler);
    }
    if rc == 0 || attrs.override_redirect == 0 || attrs.map_state != x11::xlib::IsViewable {
        return None;
    }
    if attrs.width <= 1 || attrs.height <= 1 {
        return None;
    }
    let pid = crate::x11::window_pid(child as u64).or_else(|| {
        window_children(display, child)
            .into_iter()
            .find_map(|kid| crate::x11::window_pid(kid as u64))
    });
    Some(PopupWindow {
        window: child as u64,
        x: attrs.x,
        y: attrs.y,
        width: attrs.width as u32,
        height: attrs.height as u32,
        title: window_title_for_report(display, child),
        pid,
    })
}

/// Every mapped override-redirect child of the root, bottom to top.
fn mapped_popups(display: *mut x11::xlib::Display) -> Vec<PopupWindow> {
    let root = unsafe { x11::xlib::XDefaultRootWindow(display) };
    window_children(display, root)
        .into_iter()
        .filter_map(|child| popup_info(display, child))
        .collect()
}

/// The mapped override-redirect popup (menu, popover, combo list) under the
/// screen point, if any.
pub fn popup_under_screen_point(x: i32, y: i32) -> Option<PopupWindow> {
    let display = open_display().ok()?;
    let popup = root_child_under_point(display, x, y).and_then(|child| popup_info(display, child));
    unsafe {
        x11::xlib::XCloseDisplay(display);
    }
    popup
}

/// `xid` described as a popup when it is a mapped override-redirect window.
pub fn popup_window_info(xid: u64) -> Option<PopupWindow> {
    let display = open_display().ok()?;
    let popup = popup_info(display, xid as x11::xlib::Window);
    unsafe {
        x11::xlib::XCloseDisplay(display);
    }
    popup
}

/// Mapped popups on the screen right now (bottom to top), for a caller that
/// wants "the menu that is open" without a window id.
pub fn mapped_popup_windows() -> Vec<PopupWindow> {
    let Ok(display) = open_display() else {
        return Vec::new();
    };
    let popups = mapped_popups(display);
    unsafe {
        x11::xlib::XCloseDisplay(display);
    }
    popups
}

/// `_NET_WM_WINDOW_TYPE` says the popup is a tooltip / notification / DND
/// icon: mapped and override-redirect, but nobody's grab.
fn popup_is_passive(display: *mut x11::xlib::Display, window: x11::xlib::Window) -> bool {
    let type_atom = intern_atom(display, "_NET_WM_WINDOW_TYPE");
    let passive: Vec<x11::xlib::Atom> = [
        "_NET_WM_WINDOW_TYPE_TOOLTIP",
        "_NET_WM_WINDOW_TYPE_NOTIFICATION",
        "_NET_WM_WINDOW_TYPE_DND",
    ]
    .iter()
    .map(|name| intern_atom(display, name))
    .collect();
    let mut actual_type: x11::xlib::Atom = 0;
    let mut actual_format: std::os::raw::c_int = 0;
    let mut nitems: std::os::raw::c_ulong = 0;
    let mut bytes_after: std::os::raw::c_ulong = 0;
    let mut prop: *mut std::os::raw::c_uchar = ptr::null_mut();
    let previous_handler = unsafe { x11::xlib::XSetErrorHandler(Some(ignore_x_error)) };
    let rc = unsafe {
        x11::xlib::XGetWindowProperty(
            display,
            window,
            type_atom,
            0,
            8,
            0,
            x11::xlib::XA_ATOM,
            &mut actual_type,
            &mut actual_format,
            &mut nitems,
            &mut bytes_after,
            &mut prop,
        )
    };
    unsafe {
        x11::xlib::XSync(display, 0);
        x11::xlib::XSetErrorHandler(previous_handler);
    }
    let mut result = false;
    if rc == 0 && !prop.is_null() {
        if actual_format == 32 && nitems > 0 {
            let atoms = unsafe {
                std::slice::from_raw_parts(prop as *const std::os::raw::c_ulong, nitems as usize)
            };
            result = atoms
                .iter()
                .any(|atom| passive.contains(&(*atom as x11::xlib::Atom)));
        }
        unsafe { x11::xlib::XFree(prop as *mut _) };
    }
    result
}

fn intern_atom(display: *mut x11::xlib::Display, name: &str) -> x11::xlib::Atom {
    let cname = CString::new(name).unwrap_or_default();
    unsafe { x11::xlib::XInternAtom(display, cname.as_ptr(), 0) }
}

/// The topmost mapped override-redirect popup that `pid` owns (a Qt combo
/// list or completer, a GTK/VCL menu) — the toolkit behind it holds an
/// active keyboard grab that makes the X server drop core key events from
/// any *other* master keyboard aimed at that client (`IsInterferingGrab`),
/// so the virtual master keyboard route is silently lost while it is up.
/// Tooltips / notifications are not grabs and do not count.
pub fn popup_of_pid(pid: u32) -> Option<PopupWindow> {
    let display = open_display().ok()?;
    let popup = mapped_popups(display)
        .into_iter()
        .rev()
        .filter(|popup| popup.pid == Some(pid))
        .find(|popup| !popup_is_passive(display, popup.window as x11::xlib::Window));
    unsafe {
        x11::xlib::XCloseDisplay(display);
    }
    popup
}

/// `_NET_WM_PID` of the window holding the core keyboard focus, walking up
/// its X parents (the focus often sits on a child of the client toplevel).
pub fn core_focus_owner_pid() -> Option<u32> {
    let display = open_display().ok()?;
    let mut focus: x11::xlib::Window = 0;
    let mut revert: std::os::raw::c_int = 0;
    unsafe { x11::xlib::XGetInputFocus(display, &mut focus, &mut revert) };
    let root = unsafe { x11::xlib::XDefaultRootWindow(display) };
    let mut owner = None;
    let mut current = focus;
    for _ in 0..8 {
        if current == 0 || current == root || current == x11::xlib::PointerRoot as x11::xlib::Window {
            break;
        }
        if let Some(pid) = crate::x11::window_pid(current as u64) {
            owner = Some(pid);
            break;
        }
        let mut parent: x11::xlib::Window = 0;
        let mut qroot: x11::xlib::Window = 0;
        let mut children: *mut x11::xlib::Window = ptr::null_mut();
        let mut n: std::os::raw::c_uint = 0;
        let previous_handler = unsafe { x11::xlib::XSetErrorHandler(Some(ignore_x_error)) };
        let rc = unsafe {
            x11::xlib::XQueryTree(display, current, &mut qroot, &mut parent, &mut children, &mut n)
        };
        unsafe {
            x11::xlib::XSync(display, 0);
            x11::xlib::XSetErrorHandler(previous_handler);
        }
        if !children.is_null() {
            unsafe { x11::xlib::XFree(children as *mut _) };
        }
        if rc == 0 || parent == current {
            break;
        }
        current = parent;
    }
    unsafe {
        x11::xlib::XCloseDisplay(display);
    }
    owner
}

/// Delivery path name for keys sent on the core keyboard while the target's
/// own popup holds the keyboard grab.
pub const XTEST_CORE_GRAB_PATH: &str = "xtest_core_grab";

/// A popup of the target pid holds the keyboard grab and the core focus is
/// not the target's, so no route reaches the target without stealing input
/// from the focused application; refused before any key was sent.
#[derive(Debug, Clone)]
pub struct PopupKeyboardGrab {
    pub pid: u32,
    pub popup: PopupWindow,
    pub focus_owner: Option<u32>,
}

impl std::fmt::Display for PopupKeyboardGrab {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "pid {} has {} open, which holds a keyboard grab that drops keys from the virtual \
             keyboard, and the core focus belongs to {}; no key was sent",
            self.pid,
            self.popup.describe(),
            match self.focus_owner {
                Some(pid) => format!("pid {pid}"),
                None => "no window".to_owned(),
            }
        )
    }
}
impl std::error::Error for PopupKeyboardGrab {}

/// Keys for a target whose own popup (`popup`) holds the keyboard grab: the
/// X server routes the *core* keyboard to the grab holder, which is the
/// target itself, so XTest on the core keyboard reaches it without any
/// focus change — verified afterwards on the core focus and the active
/// window. Refused (`PopupKeyboardGrab`) when the core focus belongs to
/// another pid: then a missing grab would send the keys elsewhere.
pub fn send_keys_under_popup_grab(
    pid: u32,
    popup: PopupWindow,
    send: impl FnOnce() -> Result<()>,
) -> Result<KeyboardDeliveryReport> {
    let focus_owner = core_focus_owner_pid();
    if focus_owner != Some(pid) {
        return Err(PopupKeyboardGrab {
            pid,
            popup,
            focus_owner,
        }
        .into());
    }
    let display = open_display()?;
    let saved = save_focus_state(display);
    let sent = send();
    unsafe { x11::xlib::XSync(display, 0) };
    let unchanged = focus_state_unchanged(display, &saved);
    unsafe {
        x11::xlib::XCloseDisplay(display);
    }
    sent?;
    Ok(KeyboardDeliveryReport {
        virtual_focus_held: true,
        core_focus_unchanged: unchanged,
        delivery_confirmed: true,
        key_events: 0,
        skipped_characters: Vec::new(),
        focus_guard: None,
        released_stuck: Vec::new(),
        path: XTEST_CORE_GRAB_PATH,
        grab_popup: Some(popup),
    })
}

/// Cheap post-checks a real-pointer action can make without touching the
/// application: whether the WM's active window / core focus stayed put, and
/// whether the screen region around the action point changed.
#[derive(Clone, Debug, Default)]
pub struct PointerEffect {
    /// `_NET_ACTIVE_WINDOW` and the core input focus were the same after the
    /// action as before it (sampled before the safety-net restore).
    pub focus_unchanged: bool,
    /// Percentage of pixels (0..100) that changed in a bounded region around
    /// the action point, comparing right before the press with ~250 ms after
    /// the release; `None` when the capture failed.
    pub region_diff_pct: Option<f64>,
    /// Real screen point the pointer acted at.
    pub x: i32,
    pub y: i32,
    /// Mapped override-redirect toplevels (menus, popovers, combo lists,
    /// tooltips) that appeared between the press and the post-check. A
    /// context menu opening is a window change the contract accepts as
    /// evidence of a landed click.
    pub popups_appeared: usize,
    /// Background focus-guard outcome when the tool ran the action under
    /// [`focus_guard`]: whether the application moved the desktop focus after
    /// the press (a menu grab, a dialog) and whether it was restored.
    pub focus_guard: Option<FocusGuardReport>,
    /// The point was under another window of the target's own pid (its
    /// dialog over its main window) and the press went to that window.
    pub retargeted_to: Option<SameAppCover>,
    /// The popups behind `popups_appeared`, so the tool can name them.
    pub popups: Vec<PopupWindow>,
    /// The window-local point the caller asked for, when it had one.
    pub window_point: Option<(i32, i32)>,
    /// A toplevel of ANOTHER pid under the action point (a drop target, the
    /// window a drag ended on): what it did during the action.
    pub foreign_window: Option<ForeignWindowEffect>,
}

/// What a toplevel of another pid under the action point did during the
/// action: its title before/after and the windows its pid mapped meanwhile.
/// The focus guard only watches the target pid, so a file dropped onto VLC
/// (auto-played, title changed) would otherwise be invisible.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ForeignWindowEffect {
    pub window: u64,
    pub pid: u32,
    pub title_before: String,
    pub title_after: String,
    pub new_windows: Vec<(u64, String)>,
}

impl ForeignWindowEffect {
    pub fn changed(&self) -> bool {
        self.title_before != self.title_after || !self.new_windows.is_empty()
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "window_id": self.window,
            "pid": self.pid,
            "title_before": self.title_before,
            "title_after": self.title_after,
            "new_windows": self.new_windows.iter().map(|(id, title)| serde_json::json!({"window_id": id, "title": title})).collect::<Vec<_>>(),
        })
    }

    /// One sentence for the tool text.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if self.title_before != self.title_after {
            parts.push(format!(
                "its title changed from \"{}\" to \"{}\"",
                self.title_before, self.title_after
            ));
        }
        if !self.new_windows.is_empty() {
            parts.push(format!(
                "it opened {}",
                self.new_windows
                    .iter()
                    .map(|(id, title)| format!("window {id} \"{title}\""))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        format!(
            "The point was over window {} of another application (pid {}); {}.",
            self.window,
            self.pid,
            if parts.is_empty() {
                "it did not visibly react (title unchanged, no new window)".to_owned()
            } else {
                parts.join(" and ")
            }
        )
    }
}

/// The topmost managed toplevel under the screen point that belongs to a
/// pid other than `target_pid`, with that pid's toplevels at this moment.
fn foreign_toplevel_under(target_pid: Option<u32>, x: i32, y: i32) -> Option<(crate::x11::WindowInfo, Vec<u64>)> {
    let windows = crate::x11::list_windows(None);
    let hit = windows
        .iter()
        .rev()
        .find(|w| {
            w.is_on_screen
                && w.width > 0
                && w.height > 0
                && x >= w.x
                && y >= w.y
                && x < w.x + w.width as i32
                && y < w.y + w.height as i32
        })?
        .clone();
    let pid = hit.pid?;
    if target_pid == Some(pid) {
        return None;
    }
    let owned = windows
        .iter()
        .filter(|w| w.pid == Some(pid))
        .map(|w| w.xid)
        .collect();
    Some((hit, owned))
}

/// Re-read the foreign toplevel after the action.
fn foreign_window_effect(before: Option<(crate::x11::WindowInfo, Vec<u64>)>) -> Option<ForeignWindowEffect> {
    let (hit, owned) = before?;
    let pid = hit.pid?;
    let title_after = crate::x11::window_info(hit.xid)
        .map(|w| w.title)
        .unwrap_or_default();
    let new_windows = crate::x11::list_windows(Some(pid))
        .into_iter()
        .filter(|w| !owned.contains(&w.xid) && w.is_on_screen)
        .map(|w| (w.xid, w.title))
        .collect();
    Some(ForeignWindowEffect {
        window: hit.xid,
        pid,
        title_before: hit.title,
        title_after,
        new_windows,
    })
}

impl PointerEffect {
    /// A region that changed more than this has plainly reacted to the click
    /// (a menu, a caret, a selection, a pressed button). Below it the click
    /// may still have landed without a visible reaction.
    pub const LANDED_THRESHOLD_PCT: f64 = 0.4;

    pub fn landed(&self) -> bool {
        self.popups_appeared > 0
            || self
                .region_diff_pct
                .is_some_and(|pct| pct >= Self::LANDED_THRESHOLD_PCT)
    }
}

/// Mapped override-redirect children of the root window: popup menus and
/// popovers, but also tooltips and the agent-cursor overlay, so only the
/// *new* ones across an action are meaningful (see [`mapped_popups`]).
fn new_popups(before: &[PopupWindow], after: Vec<PopupWindow>) -> Vec<PopupWindow> {
    after
        .into_iter()
        .filter(|popup| !before.iter().any(|old| old.window == popup.window))
        .collect()
}

/// Debug overrides for the multi-click cadence (milliseconds), read once.
fn click_cadence() -> (u64, u64) {
    fn env_ms(name: &str, default: u64) -> u64 {
        std::env::var(name)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }
    (
        env_ms("CUA_MPX_PRESS_MS", 12),
        env_ms("CUA_MPX_CLICK_GAP_MS", CLICK_DELAY_MS),
    )
}

/// Half-size of the square screen region compared around the action point.
const EFFECT_REGION_HALF: i32 = 120;
/// Half-size of the central square masked out of the comparison: the agent
/// cursor overlay pulses there and would count as a change.
const EFFECT_MASK_HALF: i32 = 32;
/// Wait for the application to react before comparing the region.
const EFFECT_SETTLE: Duration = Duration::from_millis(250);

/// Raw ZPixmap bytes of a root-window region (clamped to the screen).
fn root_region_pixels(cx: i32, cy: i32) -> Option<(Vec<u8>, usize, usize)> {
    let (conn, screen_num) = RustConnection::connect(None).ok()?;
    let screen = &conn.setup().roots[screen_num];
    let sw = i32::from(screen.width_in_pixels);
    let sh = i32::from(screen.height_in_pixels);
    let x0 = (cx - EFFECT_REGION_HALF).clamp(0, sw.max(1) - 1);
    let y0 = (cy - EFFECT_REGION_HALF).clamp(0, sh.max(1) - 1);
    let x1 = (cx + EFFECT_REGION_HALF).clamp(x0 + 1, sw);
    let y1 = (cy + EFFECT_REGION_HALF).clamp(y0 + 1, sh);
    let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
    let reply = conn
        .get_image(
            ImageFormat::Z_PIXMAP,
            screen.root,
            x0 as i16,
            y0 as i16,
            w as u16,
            h as u16,
            u32::MAX,
        )
        .ok()?
        .reply()
        .ok()?;
    if reply.depth < 24 || reply.data.len() < w * h * 4 {
        return None;
    }
    Some((reply.data, w, h))
}

/// Percentage of pixels outside the central mask whose 8-bit channels differ
/// by more than a small tolerance between two captures of the same region.
pub(crate) fn region_diff_pct(
    before: &(Vec<u8>, usize, usize),
    after: &(Vec<u8>, usize, usize),
) -> Option<f64> {
    let (a, w, h) = before;
    let (b, w2, h2) = after;
    if w != w2 || h != h2 || a.len() != b.len() || *w == 0 || *h == 0 {
        return None;
    }
    let (cx, cy) = (*w as i32 / 2, *h as i32 / 2);
    let mut changed = 0usize;
    let mut counted = 0usize;
    for y in 0..*h {
        for x in 0..*w {
            if (x as i32 - cx).abs() <= EFFECT_MASK_HALF
                && (y as i32 - cy).abs() <= EFFECT_MASK_HALF
            {
                continue;
            }
            counted += 1;
            let i = (y * w + x) * 4;
            let differs = (0..3).any(|c| {
                let pa = i32::from(a[i + c]);
                let pb = i32::from(b[i + c]);
                (pa - pb).abs() > 24
            });
            if differs {
                changed += 1;
            }
        }
    }
    (counted > 0).then(|| 100.0 * changed as f64 / counted as f64)
}

/// Best-effort `_NET_WM_NAME` / `WM_NAME` of a window (or of a child, for a
/// WM frame), for naming a covering window in a refusal.
fn window_title_for_report(display: *mut x11::xlib::Display, window: x11::xlib::Window) -> String {
    fn name_of(display: *mut x11::xlib::Display, window: x11::xlib::Window) -> Option<String> {
        unsafe {
            let net_name =
                x11::xlib::XInternAtom(display, c"_NET_WM_NAME".as_ptr(), x11::xlib::True);
            for prop in [net_name, x11::xlib::XA_WM_NAME] {
                if prop == 0 {
                    continue;
                }
                let mut type_ret: x11::xlib::Atom = 0;
                let mut format_ret = 0;
                let mut nitems: std::os::raw::c_ulong = 0;
                let mut bytes_after: std::os::raw::c_ulong = 0;
                let mut data: *mut std::os::raw::c_uchar = std::ptr::null_mut();
                let rc = x11::xlib::XGetWindowProperty(
                    display,
                    window,
                    prop,
                    0,
                    256,
                    x11::xlib::False,
                    x11::xlib::AnyPropertyType as x11::xlib::Atom,
                    &mut type_ret,
                    &mut format_ret,
                    &mut nitems,
                    &mut bytes_after,
                    &mut data,
                );
                if rc == x11::xlib::Success as i32 && !data.is_null() {
                    let text = if format_ret == 8 && nitems > 0 {
                        Some(
                            String::from_utf8_lossy(std::slice::from_raw_parts(
                                data,
                                nitems as usize,
                            ))
                            .into_owned(),
                        )
                    } else {
                        None
                    };
                    x11::xlib::XFree(data as *mut _);
                    if let Some(text) = text.filter(|t| !t.trim().is_empty()) {
                        return Some(text);
                    }
                }
            }
        }
        None
    }
    let previous_handler = unsafe { x11::xlib::XSetErrorHandler(Some(ignore_x_error)) };
    let mut title = name_of(display, window);
    if title.is_none() {
        title = window_children(display, window)
            .into_iter()
            .rev()
            .find_map(|kid| name_of(display, kid));
    }
    unsafe {
        x11::xlib::XSync(display, 0);
        x11::xlib::XSetErrorHandler(previous_handler);
    }
    title.unwrap_or_default()
}

/// Direct children of `window` (empty on error). Caller installs the error
/// handler.
fn window_children(
    display: *mut x11::xlib::Display,
    window: x11::xlib::Window,
) -> Vec<x11::xlib::Window> {
    let mut root_ret: x11::xlib::Window = 0;
    let mut parent: x11::xlib::Window = 0;
    let mut children: *mut x11::xlib::Window = ptr::null_mut();
    let mut count: std::os::raw::c_uint = 0;
    let rc = unsafe {
        x11::xlib::XQueryTree(
            display,
            window,
            &mut root_ret,
            &mut parent,
            &mut children,
            &mut count,
        )
    };
    if rc == 0 || children.is_null() {
        return Vec::new();
    }
    let kids = unsafe { std::slice::from_raw_parts(children, count as usize) }.to_vec();
    unsafe { x11::xlib::XFree(children as *mut _) };
    kids
}

/// The toplevel (root child) the virtual pointer would press on at `(x, y)`
/// when the caller means `window`. `Ok(None)` when the point is over the
/// target itself (its WM frame) or over an override-redirect popup (a menu or
/// combo list the click is meant to reach, or the agent-cursor overlay);
/// `Ok(Some(_))` names the window that covers the point instead.
fn occluding_window(
    display: *mut x11::xlib::Display,
    window: x11::xlib::Window,
    x: i32,
    y: i32,
) -> Result<PointCover> {
    if let Some(bounds) = window_screen_bounds(display, window) {
        if !bounds_contain(bounds, x, y) {
            return Ok(PointCover::Outside(PointOutsideWindow {
                target_window: window as u64,
                screen_x: x,
                screen_y: y,
                window_x: x - bounds.0,
                window_y: y - bounds.1,
                bounds,
            }));
        }
    }
    let under = root_child_under_point(display, x, y);
    let frame = root_child_of(display, window);
    match (under, frame) {
        (Some(under), Some(frame)) if under != frame && !is_override_redirect(display, under) => {
            let previous_handler = unsafe { x11::xlib::XSetErrorHandler(Some(ignore_x_error)) };
            let covering_client = window_children(display, under)
                .into_iter()
                .find(|kid| crate::x11::window_pid(*kid as u64).is_some());
            let covering_pid = crate::x11::window_pid(under as u64)
                .or_else(|| covering_client.and_then(|kid| crate::x11::window_pid(kid as u64)));
            unsafe {
                x11::xlib::XSync(display, 0);
                x11::xlib::XSetErrorHandler(previous_handler);
            }
            let target_pid = crate::x11::window_pid(window as u64);
            let title = window_title_for_report(display, under);
            // The app's own window (a dialog, a file chooser) sits over the
            // point: the press goes where the caller can see it, on that
            // window, and the effect reports the retarget.
            if target_pid.is_some() && covering_pid == target_pid {
                return Ok(PointCover::SameApp(SameAppCover {
                    window: covering_client.map(u64::from).unwrap_or(under as u64),
                    title,
                }));
            }
            Ok(PointCover::Occluded(TargetOccluded {
                target_window: window as u64,
                covering_window: under as u64,
                covering_title: title,
                covering_pid,
                x,
                y,
            }))
        }
        (Some(_), Some(_)) | (None, _) => Ok(PointCover::Clear),
        (Some(_), None) => bail!("target window {window} is not mapped on this screen"),
    }
}

/// What sits over the screen point a background pointer action aims at.
#[derive(Debug, Clone)]
enum PointCover {
    /// The target (or a popup meant to receive the press).
    Clear,
    /// A window of the target's own pid; the press is retargeted to it.
    SameApp(SameAppCover),
    /// Another application's window; refused.
    Occluded(TargetOccluded),
    /// The point is not inside the target window at all; refused.
    Outside(PointOutsideWindow),
}

/// The target application's own window that received a press aimed at a
/// point of another of its windows (its dialog over its main window).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SameAppCover {
    /// Client window id (the `list_windows` id, not the WM frame).
    pub window: u64,
    pub title: String,
}

/// True when neither the EWMH active window nor the core focus moved since
/// `saved` was taken.
fn focus_state_unchanged(display: *mut x11::xlib::Display, saved: &SavedFocus) -> bool {
    unsafe { x11::xlib::XSync(display, 0) };
    let mut core_focus: x11::xlib::Window = 0;
    let mut revert_to: std::os::raw::c_int = 0;
    unsafe {
        x11::xlib::XGetInputFocus(display, &mut core_focus, &mut revert_to);
    }
    ewmh_active_window(display) == saved.ewmh_active && core_focus == saved.core_focus
}

fn pointer_effect(
    display: *mut x11::xlib::Display,
    saved: &SavedFocus,
    before: Option<(Vec<u8>, usize, usize)>,
    popups_before: &[PopupWindow],
    foreign_before: Option<(crate::x11::WindowInfo, Vec<u64>)>,
    x: i32,
    y: i32,
) -> PointerEffect {
    unsafe { x11::xlib::XSync(display, 0) };
    sleep(EFFECT_SETTLE);
    let after = root_region_pixels(x, y);
    let popups = new_popups(popups_before, mapped_popups(display));
    let foreign_window = foreign_window_effect(foreign_before);
    PointerEffect {
        focus_unchanged: focus_state_unchanged(display, saved),
        region_diff_pct: match (before, after) {
            (Some(b), Some(a)) => region_diff_pct(&b, &a),
            _ => None,
        },
        x,
        y,
        popups_appeared: popups.len(),
        focus_guard: None,
        retargeted_to: None,
        popups,
        window_point: None,
        foreign_window,
    }
}

/// Land a discrete click via the MPX real-input pipeline (see
/// [`VirtualPointerClick`]). Refuses with [`TargetOccluded`] when another
/// toplevel covers the point. Returns the post-checks the tool reports.
pub fn send_virtual_pointer_click(
    cursor_id: &str,
    click: &VirtualPointerClick,
) -> Result<PointerEffect> {
    let _op = mpx_op_guard(cursor_id);
    mpx_last_use()
        .lock()
        .unwrap()
        .insert(cursor_id.to_owned(), std::time::Instant::now());
    let display = open_display()?;
    supports_parallel_pointer_injection(display)?;
    let saved_focus = save_focus_state(display);

    let result = (|| -> Result<PointerEffect> {
        let window = click.target_window as x11::xlib::Window;
        let retargeted_to = match occluding_window(display, window, click.x, click.y)? {
            PointCover::Clear => None,
            PointCover::SameApp(cover) => Some(cover),
            PointCover::Occluded(occluded) => return Err(occluded.into()),
            PointCover::Outside(outside) => return Err(outside.into()),
        };
        let ids = ensure_master_pointer(cursor_id)?;
        let device = uinput_pointers()
            .lock()
            .unwrap()
            .get(cursor_id)
            .cloned()
            .ok_or_else(|| anyhow!("missing uinput pointer for '{cursor_id}'"))?;
        let count = click.count.max(1);
        // A device left frozen by an earlier synchronous grab (a WM that never
        // replayed) would queue this press forever.
        thaw_device(display, ids.pointer_id);
        warp_master_pointer(display, ids, click.x, click.y)?;
        let popups_before = mapped_popups(display);
        let foreign_before =
            foreign_toplevel_under(crate::x11::window_pid(click.target_window), click.x, click.y);
        let before = root_region_pixels(click.x, click.y);
        let (press_ms, gap_ms) = click_cadence();
        let train = (|| -> Result<()> {
            for i in 0..count {
                {
                    let mut device = device.lock().unwrap();
                    emit_button(&mut device, click.button, true)?;
                    // A real press and release are separate evdev frames; give
                    // the toolkit a press it can see before the release lands.
                    sleep(Duration::from_millis(press_ms));
                    emit_button(&mut device, click.button, false)?;
                }
                // Multi-click cadence: keep press→press well under the toolkit
                // double-click threshold (GTK default 250 ms) so count=2 lands
                // as a real double-click, not two singles.
                if count > 1 && i + 1 < count {
                    sleep(Duration::from_millis(gap_ms));
                }
            }
            Ok(())
        })();
        // Never leave the button held: a stuck Button1 on the virtual master
        // sets bit 8 of the core modifier state (Orca's modifier), so every
        // later virtual-keyboard chord would open Orca's preferences.
        release_button_best_effort(&device, click.button);
        train?;
        let mut effect =
            pointer_effect(display, &saved_focus, before, &popups_before, foreign_before, click.x, click.y);
        effect.retargeted_to = retargeted_to;
        Ok(effect)
    })();

    restore_focus_state(display, &saved_focus);
    unsafe {
        x11::xlib::XCloseDisplay(display);
    }
    result
}

/// One held drag on the session's virtual master pointer: press at
/// `path[0]`, glide through the waypoints, release at the end. Same delivery
/// rules as [`send_virtual_pointer_click`] (no shield grab, occlusion refusal
/// at the press point, focus saved and restored).
pub fn send_virtual_pointer_drag(
    cursor_id: &str,
    drag: &VirtualPointerDrag,
) -> Result<PointerEffect> {
    let _op = mpx_op_guard(cursor_id);
    mpx_last_use()
        .lock()
        .unwrap()
        .insert(cursor_id.to_owned(), std::time::Instant::now());
    let display = open_display()?;
    supports_parallel_pointer_injection(display)?;
    let saved_focus = save_focus_state(display);

    let result = (|| -> Result<PointerEffect> {
        if drag.path.len() < 2 {
            bail!("drag path needs at least 2 points");
        }
        let window = drag.target_window as x11::xlib::Window;
        let start = drag.path[0];
        let end = drag.path[drag.path.len() - 1];
        let retargeted_to = match occluding_window(display, window, start.0, start.1)? {
            PointCover::Clear => None,
            PointCover::SameApp(cover) => Some(cover),
            PointCover::Occluded(occluded) => return Err(occluded.into()),
            PointCover::Outside(outside) => return Err(outside.into()),
        };
        let ids = ensure_master_pointer(cursor_id)?;
        let device = uinput_pointers()
            .lock()
            .unwrap()
            .get(cursor_id)
            .cloned()
            .ok_or_else(|| anyhow!("missing uinput pointer for '{cursor_id}'"))?;
        let (cum, total) = path_cumulative(&drag.path);
        let steps = drag.steps.max(1);
        let step_delay = Duration::from_millis(drag.duration_ms / steps as u64);
        thaw_device(display, ids.pointer_id);
        warp_master_pointer(display, ids, start.0, start.1)?;
        let popups_before = mapped_popups(display);
        let foreign_before =
            foreign_toplevel_under(crate::x11::window_pid(drag.target_window), end.0, end.1);
        let before = root_region_pixels(end.0, end.1);
        let gesture = (|| -> Result<()> {
            {
                let mut device = device.lock().unwrap();
                emit_button(&mut device, drag.button, true)?;
            }
            // Let the toolkit register the press (and arm its drag threshold)
            // before the first motion.
            sleep(Duration::from_millis(40));
            let (mut last_x, mut last_y) = start;
            for step in 1..=steps {
                let t = step as f64 / steps as f64;
                let (ix, iy) = point_on_path(&drag.path, &cum, total, t);
                let (dx, dy) = (ix - last_x, iy - last_y);
                if dx != 0 || dy != 0 {
                    let mut device = device.lock().unwrap();
                    emit_relative_motion(&mut device, dx, dy)?;
                    crate::overlay::send_command_for(
                        cursor_id.to_owned(),
                        cursor_overlay::OverlayCommand::SnapTo {
                            x: ix as f64,
                            y: iy as f64,
                            heading_radians: Some((dy as f64).atan2(dx as f64)),
                        },
                    );
                }
                last_x = ix;
                last_y = iy;
                sleep(step_delay);
            }
            // Relative motion accumulates libinput rounding; pin the release
            // to the exact end point before letting go.
            warp_master_pointer(display, ids, end.0, end.1)?;
            sleep(Duration::from_millis(20));
            let mut device = device.lock().unwrap();
            emit_button(&mut device, drag.button, false)?;
            Ok(())
        })();
        // Release on every exit path (see `send_virtual_pointer_click`).
        release_button_best_effort(&device, drag.button);
        gesture?;
        let mut effect = pointer_effect(display, &saved_focus, before, &popups_before, foreign_before, end.0, end.1);
        effect.retargeted_to = retargeted_to;
        Ok(effect)
    })();

    restore_focus_state(display, &saved_focus);
    unsafe {
        x11::xlib::XCloseDisplay(display);
    }
    result
}

/// A discrete no-focus-steal scroll driven through the MPX master pointer +
/// uinput slave. Unlike a click it needs no shield grab: WMs don't focus on
/// wheel input, and libinput turns the emitted REL_WHEEL/REL_HWHEEL detents into
/// the XI2 smooth-scroll events GTK consumes — where synthetic Button4-7
/// `XSendEvent`s are dropped. `x`,`y` are the screen point to scroll over (the
/// scroll lands on whatever window owns that pixel under our master pointer);
/// `ticks` is a signed detent count (+up / +right per evdev convention).
#[derive(Clone, Debug)]
pub struct VirtualPointerScroll {
    pub target_window: u64,
    pub x: i32,
    pub y: i32,
    pub horizontal: bool,
    pub ticks: i32,
}

/// Land a discrete scroll via the MPX real-input pipeline (see
/// [`VirtualPointerScroll`]). Warps the dedicated master pointer over the target
/// point, then emits `|ticks|` wheel detents on the uinput slave. The master is
/// torn down and focus restored on exit, matching the click/drag paths.
pub fn send_virtual_pointer_scroll(cursor_id: &str, scroll: &VirtualPointerScroll) -> Result<()> {
    let _op = mpx_op_guard(cursor_id);
    let display = open_display()?;
    supports_parallel_pointer_injection(display)?;
    let saved_focus = save_focus_state(display);

    let result = (|| -> Result<()> {
        let ids = ensure_master_pointer(cursor_id)?;
        let device = uinput_pointers()
            .lock()
            .unwrap()
            .get(cursor_id)
            .cloned()
            .ok_or_else(|| anyhow!("missing uinput pointer for '{cursor_id}'"))?;

        warp_master_pointer(display, ids, scroll.x, scroll.y)?;
        let detents = scroll.ticks.unsigned_abs() as usize;
        if detents == 0 {
            return Ok(());
        }
        let unit = if scroll.ticks >= 0 { 1 } else { -1 };
        for _ in 0..detents {
            {
                let mut device = device.lock().unwrap();
                emit_scroll(&mut device, scroll.horizontal, unit)?;
            }
            sleep(Duration::from_millis(CLICK_DELAY_MS));
        }
        Ok(())
    })();

    let _ = cursor_id; // master pair retained for reuse (see the drag path).
    restore_focus_state(display, &saved_focus);
    unsafe {
        x11::xlib::XCloseDisplay(display);
    }
    result
}

/// Pre-drag focus snapshot: the EWMH active window when a conforming WM is
/// running, plus the core input focus as a WM-agnostic fallback.
struct SavedFocus {
    ewmh_active: Option<x11::xlib::Window>,
    core_focus: x11::xlib::Window,
    core_revert_to: std::os::raw::c_int,
}

fn save_focus_state(display: *mut x11::xlib::Display) -> SavedFocus {
    let mut core_focus: x11::xlib::Window = 0;
    let mut core_revert_to: std::os::raw::c_int = 0;
    unsafe {
        x11::xlib::XGetInputFocus(display, &mut core_focus, &mut core_revert_to);
    }
    SavedFocus {
        ewmh_active: ewmh_active_window(display),
        core_focus,
        core_revert_to,
    }
}

unsafe extern "C" fn ignore_x_error(
    _display: *mut x11::xlib::Display,
    _event: *mut x11::xlib::XErrorEvent,
) -> std::os::raw::c_int {
    0
}

fn restore_focus_state(display: *mut x11::xlib::Display, saved: &SavedFocus) {
    // Let the release/focus events from the drag settle before reading the
    // post-drag state, so we don't race the WM's own focus update.
    unsafe { x11::xlib::XSync(display, 0) };

    if let Some(prev) = saved.ewmh_active {
        // Fast path: on WMs whose click-to-focus grab ignores the virtual
        // master (mutter grabs for the Virtual Core Pointer only) nothing
        // moved, and a 300 ms settle per click would be pure latency.
        if ewmh_active_window(display) == Some(prev) {
            let mut stable = 0;
            for _ in 0..4 {
                sleep(Duration::from_millis(25));
                if ewmh_active_window(display) == Some(prev) {
                    stable += 1;
                } else {
                    stable = 0;
                    break;
                }
            }
            if stable >= 3 {
                return;
            }
        }
        // EWMH path: ask the WM to re-activate, so its active-window
        // bookkeeping (decorations, stacking) stays consistent. The WM
        // processes its own click-to-focus for the drag asynchronously and
        // can re-activate the dragged window even after one re-activation of
        // ours has landed — so don't stop at first success: require the
        // active window to hold stable for consecutive checks, re-sending on
        // every regression, within a bounded budget.
        sleep(Duration::from_millis(300));
        let mut stable = 0;
        for attempt in 0..15 {
            let now = ewmh_active_window(display);
            if now == Some(prev) {
                stable += 1;
                if stable >= 3 {
                    return;
                }
            } else {
                stable = 0;
                // MPX clicks can leave a core-protocol WM believing the
                // dragged window is focused while the core focus never moved
                // there: its XSetInputFocus for our activation is then a
                // no-op, no FocusIn arrives, and its bookkeeping never
                // updates. Bounce the core focus onto the window the WM
                // believes active so the activation produces a real focus
                // transition the WM can observe.
                if attempt >= 2 {
                    if let Some(now_win) = now {
                        unsafe {
                            let prev_handler = x11::xlib::XSetErrorHandler(Some(ignore_x_error));
                            x11::xlib::XSetInputFocus(
                                display,
                                now_win,
                                x11::xlib::RevertToParent,
                                x11::xlib::CurrentTime,
                            );
                            x11::xlib::XSync(display, 0);
                            x11::xlib::XSetErrorHandler(prev_handler);
                        }
                        sleep(Duration::from_millis(100));
                    }
                }
                ewmh_activate_window(display, prev, now.unwrap_or(0));
            }
            sleep(Duration::from_millis(200));
        }
        if stable == 0 {
            tracing::warn!("focus restore: WM did not re-activate 0x{prev:x}");
        }
        return;
    }

    // No EWMH WM (bare X / minimal WM): restore the core input focus
    // directly. The saved window may have been destroyed meanwhile, and
    // Xlib's default error handler exits the process on BadWindow, so the
    // restore runs under a scoped ignore-errors handler.
    if saved.core_focus == 0 {
        return;
    }
    unsafe {
        let mut now_focus: x11::xlib::Window = 0;
        let mut now_revert: std::os::raw::c_int = 0;
        x11::xlib::XGetInputFocus(display, &mut now_focus, &mut now_revert);
        if now_focus == saved.core_focus {
            return;
        }
        let prev_handler = x11::xlib::XSetErrorHandler(Some(ignore_x_error));
        x11::xlib::XSetInputFocus(
            display,
            saved.core_focus,
            saved.core_revert_to,
            x11::xlib::CurrentTime,
        );
        x11::xlib::XSync(display, 0);
        x11::xlib::XSetErrorHandler(prev_handler);
    }
}

#[derive(Clone, Copy, Debug)]
struct EventTarget {
    window: Window,
    local_x: i16,
    local_y: i16,
    root_x: i16,
    root_y: i16,
}

fn point_in_rect(x: i32, y: i32, geom: &GetGeometryReply) -> bool {
    x >= geom.x as i32
        && y >= geom.y as i32
        && x < geom.x as i32 + geom.width as i32
        && y < geom.y as i32 + geom.height as i32
}

fn deepest_child_at_point(
    conn: &RustConnection,
    window: Window,
    local_x: i32,
    local_y: i32,
) -> Result<(Window, i32, i32)> {
    let tree = conn.query_tree(window)?.reply()?;
    for child in tree.children.iter().rev() {
        let Ok(geom) = conn.get_geometry(*child)?.reply() else {
            continue;
        };
        if !point_in_rect(local_x, local_y, &geom) {
            continue;
        }
        let child_x = local_x - geom.x as i32;
        let child_y = local_y - geom.y as i32;
        return deepest_child_at_point(conn, *child, child_x, child_y);
    }
    Ok((window, local_x, local_y))
}

fn resolve_event_target(conn: &RustConnection, xid: u64, x: i32, y: i32) -> Result<EventTarget> {
    let top = xid as Window;
    let root = conn.setup().roots[0].root;
    let root_pos = conn.translate_coordinates(top, root, 0, 0)?.reply()?;
    let (window, local_x, local_y) = deepest_child_at_point(conn, top, x, y)?;
    Ok(EventTarget {
        window,
        local_x: local_x as i16,
        local_y: local_y as i16,
        root_x: (root_pos.dst_x as i32 + x) as i16,
        root_y: (root_pos.dst_y as i32 + y) as i16,
    })
}

fn button_state_mask(button: u8) -> KeyButMask {
    match button {
        1 => KeyButMask::BUTTON1,
        2 => KeyButMask::BUTTON2,
        3 => KeyButMask::BUTTON3,
        4 => KeyButMask::BUTTON4,
        5 => KeyButMask::BUTTON5,
        _ => KeyButMask::from(0u16),
    }
}

/// Open an X11 connection for background input injection, failing *loudly* and
/// actionably when input cannot be delivered — rather than letting a pure
/// Wayland session fall through to an X11 path that silently no-ops yet reports
/// success (#1921). On a pure Wayland session with the native backend off, this
/// returns a clear error naming the fix; otherwise it connects and, on any
/// connect failure, surfaces DISPLAY so the cause is diagnosable.
fn connect_x11_for_input() -> Result<(RustConnection, usize)> {
    if let Some(reason) = crate::wayland::wayland_input_unavailable_reason() {
        bail!("{reason}");
    }
    RustConnection::connect(None).map_err(|e| {
        anyhow!(
            "cannot inject input: X11 connection failed (DISPLAY={:?}): {e}",
            std::env::var("DISPLAY").ok()
        )
    })
}

/// Send a synthetic FocusIn event to a window without changing the actual X11 input focus.
/// This can trigger toolkit-level focus handlers (e.g., Qt5's AT-SPI bridge) without
/// moving the window manager's active window. Use with send_focus_out to restore state.
pub fn send_focus_in(xid: u64) -> Result<()> {
    let (conn, _) = connect_x11_for_input()?;
    let window = xid as u32;

    let focus_in = FocusInEvent {
        response_type: FOCUS_IN_EVENT,
        detail: NotifyDetail::NONLINEAR,
        sequence: 0,
        event: window,
        mode: NotifyMode::NORMAL,
    };

    conn.send_event(false, window, EventMask::FOCUS_CHANGE, &focus_in)?;
    conn.flush()?;
    Ok(())
}

/// Send a synthetic FocusOut event to restore focus state after send_focus_in.
pub fn send_focus_out(xid: u64) -> Result<()> {
    let (conn, _) = connect_x11_for_input()?;
    let window = xid as u32;

    let focus_out = FocusOutEvent {
        response_type: FOCUS_OUT_EVENT,
        detail: NotifyDetail::NONLINEAR,
        sequence: 0,
        event: window,
        mode: NotifyMode::NORMAL,
    };

    conn.send_event(false, window, EventMask::FOCUS_CHANGE, &focus_out)?;
    conn.flush()?;
    Ok(())
}

/// Send a button click (down + up) to a window at window-local coordinates.
pub fn send_click(xid: u64, x: i32, y: i32, count: usize, button: u8) -> Result<()> {
    send_click_with_modifiers(xid, x, y, count, button, &[])
}

/// Send a target-addressed X11 click whose event-state mask carries the named
/// modifiers. Unlike a plain AT-SPI action, this preserves multi-selection
/// semantics without changing the X input focus.
pub fn send_click_with_modifiers(
    xid: u64,
    x: i32,
    y: i32,
    count: usize,
    button: u8,
    modifiers: &[&str],
) -> Result<()> {
    let (conn, _) = connect_x11_for_input()?;
    let root = conn.setup().roots[0].root;
    let modifier_state = modifiers_to_state(modifiers);

    for _ in 0..count {
        let target = resolve_event_target(&conn, xid, x, y)?;
        let press = ButtonPressEvent {
            response_type: BUTTON_PRESS_EVENT,
            detail: button,
            sequence: 0,
            time: x11rb::CURRENT_TIME,
            root,
            event: target.window,
            child: x11rb::NONE,
            root_x: target.root_x,
            root_y: target.root_y,
            event_x: target.local_x,
            event_y: target.local_y,
            state: modifier_state,
            same_screen: true,
        };

        let release = ButtonReleaseEvent {
            response_type: BUTTON_RELEASE_EVENT,
            detail: button,
            sequence: 0,
            time: x11rb::CURRENT_TIME,
            root,
            event: target.window,
            child: x11rb::NONE,
            root_x: target.root_x,
            root_y: target.root_y,
            event_x: target.local_x,
            event_y: target.local_y,
            state: KeyButMask::from(
                u16::from(modifier_state) | u16::from(button_state_mask(button)),
            ),
            same_screen: true,
        };

        conn.send_event(false, target.window, EventMask::BUTTON_PRESS, &press)?;
        sleep(Duration::from_millis(CLICK_DELAY_MS));
        conn.send_event(false, target.window, EventMask::BUTTON_RELEASE, &release)?;
        conn.flush()?;

        if count > 1 {
            sleep(Duration::from_millis(80));
        }
    }
    Ok(())
}

/// Send a press-drag-release gesture via XSendEvent (ButtonPress + MotionNotify steps + ButtonRelease).
///
/// `xid` — target window XID. `from_x/y`, `to_x/y` — window-local coords.
/// `duration_ms` — total budget. `steps` — interpolated MotionNotify events.
/// `button` — X11 button number (1=left, 2=middle, 3=right).
pub fn send_drag(
    xid: u64,
    from_x: i32,
    from_y: i32,
    to_x: i32,
    to_y: i32,
    duration_ms: u64,
    steps: usize,
    button: u8,
) -> Result<()> {
    let (conn, _) = connect_x11_for_input()?;
    let root = conn.setup().roots[0].root;
    let steps = steps.max(1);
    let step_delay_ms = if steps > 1 {
        duration_ms / steps as u64
    } else {
        duration_ms
    };
    let press_target = resolve_event_target(&conn, xid, from_x, from_y)?;

    // ButtonPress at start.
    let press = ButtonPressEvent {
        response_type: BUTTON_PRESS_EVENT,
        detail: button,
        sequence: 0,
        time: x11rb::CURRENT_TIME,
        root,
        event: press_target.window,
        child: x11rb::NONE,
        root_x: press_target.root_x,
        root_y: press_target.root_y,
        event_x: press_target.local_x,
        event_y: press_target.local_y,
        state: KeyButMask::from(0u16),
        same_screen: true,
    };
    conn.send_event(false, press_target.window, EventMask::BUTTON_PRESS, &press)?;
    conn.flush()?;
    sleep(Duration::from_millis(CLICK_DELAY_MS));

    // Interpolated MotionNotify steps.
    for i in 1..=steps {
        let t = i as f64 / steps as f64;
        let ix = from_x + ((to_x - from_x) as f64 * t).round() as i32;
        let iy = from_y + ((to_y - from_y) as f64 * t).round() as i32;
        let target = resolve_event_target(&conn, xid, ix, iy)?;
        let motion = MotionNotifyEvent {
            response_type: MOTION_NOTIFY_EVENT,
            detail: Motion::NORMAL,
            sequence: 0,
            time: x11rb::CURRENT_TIME,
            root,
            event: target.window,
            child: x11rb::NONE,
            root_x: target.root_x,
            root_y: target.root_y,
            event_x: target.local_x,
            event_y: target.local_y,
            state: button_state_mask(button),
            same_screen: true,
        };
        conn.send_event(false, target.window, EventMask::POINTER_MOTION, &motion)?;
        conn.flush()?;
        if step_delay_ms > 0 {
            sleep(Duration::from_millis(step_delay_ms));
        }
    }

    // ButtonRelease at end.
    let release_target = resolve_event_target(&conn, xid, to_x, to_y)?;
    let release = ButtonReleaseEvent {
        response_type: BUTTON_RELEASE_EVENT,
        detail: button,
        sequence: 0,
        time: x11rb::CURRENT_TIME,
        root,
        event: release_target.window,
        child: x11rb::NONE,
        root_x: release_target.root_x,
        root_y: release_target.root_y,
        event_x: release_target.local_x,
        event_y: release_target.local_y,
        state: button_state_mask(button),
        same_screen: true,
    };
    conn.send_event(
        false,
        release_target.window,
        EventMask::BUTTON_RELEASE,
        &release,
    )?;
    conn.flush()?;
    Ok(())
}

pub fn send_button_down(xid: u64, x: i32, y: i32, button: u8) -> Result<()> {
    let (conn, _) = connect_x11_for_input()?;
    let root = conn.setup().roots[0].root;
    let target = resolve_event_target(&conn, xid, x, y)?;
    let press = ButtonPressEvent {
        response_type: BUTTON_PRESS_EVENT,
        detail: button,
        sequence: 0,
        time: x11rb::CURRENT_TIME,
        root,
        event: target.window,
        child: x11rb::NONE,
        root_x: target.root_x,
        root_y: target.root_y,
        event_x: target.local_x,
        event_y: target.local_y,
        state: KeyButMask::from(0u16),
        same_screen: true,
    };
    conn.send_event(false, target.window, EventMask::BUTTON_PRESS, &press)?;
    conn.flush()?;
    Ok(())
}

pub fn send_motion(xid: u64, x: i32, y: i32, button: Option<u8>) -> Result<()> {
    let (conn, _) = connect_x11_for_input()?;
    let root = conn.setup().roots[0].root;
    let target = resolve_event_target(&conn, xid, x, y)?;
    let motion = MotionNotifyEvent {
        response_type: MOTION_NOTIFY_EVENT,
        detail: Motion::NORMAL,
        sequence: 0,
        time: x11rb::CURRENT_TIME,
        root,
        event: target.window,
        child: x11rb::NONE,
        root_x: target.root_x,
        root_y: target.root_y,
        event_x: target.local_x,
        event_y: target.local_y,
        state: button
            .map(button_state_mask)
            .unwrap_or_else(|| KeyButMask::from(0u16)),
        same_screen: true,
    };
    conn.send_event(false, target.window, EventMask::POINTER_MOTION, &motion)?;
    conn.flush()?;
    Ok(())
}

pub fn send_button_up(xid: u64, x: i32, y: i32, button: u8) -> Result<()> {
    let (conn, _) = connect_x11_for_input()?;
    let root = conn.setup().roots[0].root;
    let target = resolve_event_target(&conn, xid, x, y)?;
    let release = ButtonReleaseEvent {
        response_type: BUTTON_RELEASE_EVENT,
        detail: button,
        sequence: 0,
        time: x11rb::CURRENT_TIME,
        root,
        event: target.window,
        child: x11rb::NONE,
        root_x: target.root_x,
        root_y: target.root_y,
        event_x: target.local_x,
        event_y: target.local_y,
        state: button_state_mask(button),
        same_screen: true,
    };
    conn.send_event(false, target.window, EventMask::BUTTON_RELEASE, &release)?;
    conn.flush()?;
    Ok(())
}

/// Type a string by sending KeyPress/KeyRelease events for each character.
pub fn send_type_text(xid: u64, text: &str) -> Result<()> {
    send_type_text_with_delay(xid, text, 0)
}

/// Type a string with an additional `inter_char_ms` delay between each character.
pub fn send_type_text_with_delay(xid: u64, text: &str, inter_char_ms: u64) -> Result<()> {
    let (conn, _) = connect_x11_for_input()?;
    let window = xid as u32;
    let root = conn.setup().roots[0].root;
    let mapping = conn.get_keyboard_mapping(8, 248)?.reply()?;

    for ch in text.chars() {
        // Resolve the keycode and whether Shift must be held — without it,
        // uppercase and shifted symbols would otherwise type their unshifted
        // form (e.g. "A" arriving as "a").
        let Some((keycode, needs_shift)) = char_to_keycode_shift(&mapping, ch as u32) else {
            continue;
        };
        let state = if needs_shift {
            KeyButMask::SHIFT
        } else {
            KeyButMask::from(0u16)
        };

        let press = KeyPressEvent {
            response_type: KEY_PRESS_EVENT,
            detail: keycode,
            sequence: 0,
            time: x11rb::CURRENT_TIME,
            root,
            event: window,
            child: x11rb::NONE,
            root_x: 0,
            root_y: 0,
            event_x: 0,
            event_y: 0,
            state,
            same_screen: true,
        };
        let release = KeyReleaseEvent {
            response_type: KEY_RELEASE_EVENT,
            detail: keycode,
            sequence: 0,
            time: x11rb::CURRENT_TIME,
            root,
            event: window,
            child: x11rb::NONE,
            root_x: 0,
            root_y: 0,
            event_x: 0,
            event_y: 0,
            state,
            same_screen: true,
        };

        conn.send_event(false, window, EventMask::KEY_PRESS, &press)?;
        // Start the hold interval after sending the press, not while it is buffered.
        conn.flush()?;
        sleep(Duration::from_millis(KEY_DELAY_MS));
        conn.send_event(false, window, EventMask::KEY_RELEASE, &release)?;
        conn.flush()?;
        if inter_char_ms > 0 {
            sleep(Duration::from_millis(inter_char_ms));
        }
    }
    // Deliver the final release before this short-lived connection closes.
    conn.get_input_focus()?.reply()?;
    Ok(())
}

/// Type `text` into whatever window currently holds X keyboard focus, using the
/// XTest extension. Unlike [`send_type_text`] (synthetic XSendEvent, which GTK/Qt
/// silently drop for key input), XTest injects *real* input events, so they
/// actually reach the focused widget — a spreadsheet cell, a terminal, a canvas —
/// that exposes no AT-SPI EditableText interface to fill. There is no window
/// argument because XTest always delivers to the focused window; the caller
/// focuses the target by clicking it first. `\n`/`\t` map to Return/Tab.
pub fn send_type_text_xtest(text: &str) -> Result<()> {
    use x11rb::protocol::xtest::ConnectionExt as _;
    let (conn, _) = connect_x11_for_input()?;
    let mapping = conn.get_keyboard_mapping(8, 248)?.reply()?;
    // Shift keycode (modifier index 0) for shifted characters.
    let modmap = conn.get_modifier_mapping()?.reply()?;
    let kpm = modmap.keycodes_per_modifier() as usize;
    let shift_kc = modmap
        .keycodes
        .get(..kpm)
        .and_then(|s| s.iter().copied().find(|&k| k != 0))
        .unwrap_or(50);
    for ch in text.chars() {
        let cp = match ch {
            '\n' => 0xff0d, // XK_Return
            '\t' => 0xff09, // XK_Tab
            c => c as u32,
        };
        let Some((keycode, needs_shift)) = char_to_keycode_shift(&mapping, cp) else {
            continue;
        };
        if needs_shift {
            conn.xtest_fake_input(KEY_PRESS_EVENT, shift_kc, 0, x11rb::NONE, 0, 0, 0)?;
        }
        conn.xtest_fake_input(KEY_PRESS_EVENT, keycode, 0, x11rb::NONE, 0, 0, 0)?;
        conn.xtest_fake_input(KEY_RELEASE_EVENT, keycode, 0, x11rb::NONE, 0, 0, 0)?;
        if needs_shift {
            conn.xtest_fake_input(KEY_RELEASE_EVENT, shift_kc, 0, x11rb::NONE, 0, 0, 0)?;
        }
        conn.flush()?;
        sleep(Duration::from_millis(KEY_DELAY_MS));
    }
    // Round-trip so the server delivers the final character's key events before
    // this short-lived connection drops (see send_key_xtest — keyboard XTEST
    // events queued on a connection that closes immediately can be lost).
    let _ = conn.get_input_focus()?.reply();
    Ok(())
}

/// Press a named key (with optional modifiers) into whatever window holds X
/// keyboard focus, via the XTest extension. This is the REAL-input analogue of
/// [`send_key`]: XTest events are indistinguishable from physical input, so
/// GTK/Qt/Chromium/Firefox accept them — whereas the synthetic `XSendEvent`
/// path in [`send_key`] is silently dropped by those toolkits (they check the
/// `send_event` flag). Used by the `foreground` delivery rung, which activates
/// the target first, so XTest-to-focus lands on the intended widget.
///
/// Modifiers (e.g. `["ctrl"]`, `["ctrl","shift"]`) are pressed before and
/// released after the key. Modifier names resolve to their keysyms
/// (Control_L/Shift_L/Alt_L/Super_L), then to keycodes that are in the server's
/// modifier map, so the modifier mask actually engages. Sparse/headless keymaps
/// that lack a keysym borrow a spare keycode (xdotool-style) via
/// [`keycode_for_keysym`]; the returned guards restore the map on drop.
pub fn send_key_xtest(key: &str, modifiers: &[&str]) -> Result<()> {
    use x11rb::protocol::xtest::ConnectionExt as _;
    let (conn, _) = connect_x11_for_input()?;
    let mapping = conn.get_keyboard_mapping(8, 248)?.reply()?;

    // Resolve modifier keycodes first. Keep any spare-keycode remap guards alive
    // until after the events are delivered (drop at end of fn).
    let mut guards = Vec::new();
    let mut mod_keycodes = Vec::new();
    for m in modifiers {
        let ks = key_name_to_keysym(m)?;
        let (kc, guard) = keycode_for_keysym(&conn, &mapping, ks, m)?;
        if let Some(g) = guard {
            guards.push(g);
        }
        mod_keycodes.push(kc);
    }

    // Resolve the main key, SHIFT-AWARE. A keysym at the shifted level (slot 1)
    // — e.g. '*', '+', '(' on a US layout, including the spelled-out names
    // asterisk/plus/parenleft — must be typed with Shift held; a bare keycode
    // press emits the slot-0 glyph instead (e.g. '*' -> '8', '+' -> '='). Prefer
    // the slot-0/slot-1 lookup (`char_to_keycode_shift`); fall back to the
    // spare-keycode remap (which binds the keysym across all levels, so no shift)
    // only when the keysym is absent from the map.
    let keysym = key_name_to_keysym(key)?;
    let (keycode, needs_shift) = match char_to_keycode_shift(&mapping, keysym) {
        Some(found) => found,
        None => {
            let (kc, guard) = keycode_for_keysym(&conn, &mapping, keysym, key)?;
            if let Some(g) = guard {
                guards.push(g);
            }
            (kc, false)
        }
    };

    // Hold Shift around the key when it lives at the shifted level and the caller
    // didn't already pass Shift as a modifier. Resolve the Shift keycode from the
    // server's modifier map so the mask actually engages.
    let shift_requested = modifiers.iter().any(|m| m.eq_ignore_ascii_case("shift"));
    let auto_shift_kc = if needs_shift && !shift_requested {
        let modmap = conn.get_modifier_mapping()?.reply()?;
        let kpm = modmap.keycodes_per_modifier() as usize;
        modmap
            .keycodes
            .get(..kpm)
            .and_then(|s| s.iter().copied().find(|&k| k != 0))
    } else {
        None
    };

    // Press modifiers (+ auto-Shift), tap the key, release in reverse order.
    for &kc in &mod_keycodes {
        conn.xtest_fake_input(KEY_PRESS_EVENT, kc, 0, x11rb::NONE, 0, 0, 0)?;
    }
    if let Some(sk) = auto_shift_kc {
        conn.xtest_fake_input(KEY_PRESS_EVENT, sk, 0, x11rb::NONE, 0, 0, 0)?;
    }
    conn.xtest_fake_input(KEY_PRESS_EVENT, keycode, 0, x11rb::NONE, 0, 0, 0)?;
    // Flush modifiers and key-down before measuring the delivered hold interval.
    conn.flush()?;
    sleep(Duration::from_millis(KEY_DELAY_MS));
    conn.xtest_fake_input(KEY_RELEASE_EVENT, keycode, 0, x11rb::NONE, 0, 0, 0)?;
    if let Some(sk) = auto_shift_kc {
        conn.xtest_fake_input(KEY_RELEASE_EVENT, sk, 0, x11rb::NONE, 0, 0, 0)?;
    }
    for &kc in mod_keycodes.iter().rev() {
        conn.xtest_fake_input(KEY_RELEASE_EVENT, kc, 0, x11rb::NONE, 0, 0, 0)?;
    }
    conn.flush()?;

    // Round-trip so the server actually PROCESSES (delivers) the injected XTEST
    // key events before this function returns and drops its short-lived
    // connection. `flush()` only writes the requests to the socket; without a
    // following reply-bearing request the connection can close before the server
    // routes the KeyPress/KeyRelease to the focused window, and the events are
    // dropped. Observed under Xtigervnc: identical raw `xtest_fake_input` calls
    // deliver when the connection stays alive but vanish from this short-lived
    // one — pointer events (send_click_xtest_desktop) survive, keyboard events do
    // not. Unconditional (previously only ran for spare-keycode remaps).
    let _ = conn.get_input_focus()?.reply();
    if !guards.is_empty() {
        // Spare-keycode remap: give the target a beat to translate the events
        // under the temporary mapping before the guards restore it on drop.
        sleep(Duration::from_millis(KEY_DELAY_MS));
    }
    drop(guards);
    Ok(())
}

/// Screen-absolute click via the XTest extension — the desktop-target
/// foreground click. It warps the real pointer to `(x, y)` and injects a true
/// button press/release there, so the event lands on whatever window owns that
/// screen pixel (the Linux peer of the Windows `WindowFromPoint` + macOS
/// global-HID `CGEvent` desktop click).
///
/// XTest delivering to the focused / under-pointer window is precisely why the
/// *background* paths above use `XSendEvent` instead (see the module header) —
/// but it is exactly what desktop scope wants: the agent has located the target
/// by vision on the whole screen and issues a real screen-absolute pointer
/// click. `button` is an X button number (1=left, 2=middle, 3=right).
pub fn send_click_xtest_desktop(x: i32, y: i32, button: u8, count: usize) -> Result<()> {
    send_click_xtest_desktop_with_modifiers(x, y, button, count, &[])
}

/// Real XTest click with physical modifier down/up transitions around the
/// pointer gesture. Used only after the caller selected foreground delivery.
pub fn send_click_xtest_desktop_with_modifiers(
    x: i32,
    y: i32,
    button: u8,
    count: usize,
    modifiers: &[&str],
) -> Result<()> {
    use x11rb::protocol::xtest::ConnectionExt as _;
    let (conn, screen_num) = connect_x11_for_input()?;
    let root = conn.setup().roots[screen_num].root;
    let mapping = conn.get_keyboard_mapping(8, 248)?.reply()?;
    let mut guards = Vec::new();
    let mut modifier_keycodes = Vec::new();
    for modifier in modifiers {
        let keysym = key_name_to_keysym(modifier)?;
        let (keycode, guard) = keycode_for_keysym(&conn, &mapping, keysym, modifier)?;
        if let Some(guard) = guard {
            guards.push(guard);
        }
        modifier_keycodes.push(keycode);
    }
    let mut pressed = Vec::new();
    let gesture_result = (|| -> Result<()> {
        for &keycode in &modifier_keycodes {
            conn.xtest_fake_input(KEY_PRESS_EVENT, keycode, 0, x11rb::NONE, 0, 0, 0)?;
            pressed.push(keycode);
        }
        // Absolute pointer warp (MotionNotify, detail=0 => absolute) so the
        // button events that follow are delivered at (x, y).
        conn.xtest_fake_input(MOTION_NOTIFY_EVENT, 0, 0, root, x as i16, y as i16, 0)?;
        let count = count.max(1);
        for click_index in 0..count {
            conn.xtest_fake_input(BUTTON_PRESS_EVENT, button, 0, root, x as i16, y as i16, 0)?;
            conn.xtest_fake_input(BUTTON_RELEASE_EVENT, button, 0, root, x as i16, y as i16, 0)?;
            if click_index + 1 < count {
                // Chromium needs the first pair to reach the server before the
                // second pair. A zero-gap batch produces two click events but
                // no DOM dblclick event under Xvfb/Openbox.
                conn.flush()?;
                sleep(Duration::from_millis(DOUBLE_CLICK_DELAY_MS));
            }
        }
        Ok(())
    })();

    // Always attempt to release every modifier that was successfully queued,
    // including when a later pointer request fails. A failed gesture must not
    // leave the desktop with a logically stuck Ctrl/Shift/Alt/Super key.
    let mut release_result: Result<()> = Ok(());
    for &keycode in pressed.iter().rev() {
        if let Err(error) =
            conn.xtest_fake_input(KEY_RELEASE_EVENT, keycode, 0, x11rb::NONE, 0, 0, 0)
        {
            if release_result.is_ok() {
                release_result = Err(error.into());
            }
        }
    }
    conn.flush()?;
    // Round-trip so the server processes the warp+button events before this
    // short-lived connection drops. Pointer events happened to survive the close
    // under Xtigervnc where keyboard events did not (see send_key_xtest), but make
    // it explicit so the desktop click is reliable across X servers too.
    let _ = conn.get_input_focus()?.reply();
    drop(guards);
    gesture_result?;
    release_result?;
    Ok(())
}

/// Move the real X11 pointer to a screen-absolute desktop coordinate.
pub fn send_move_xtest_desktop(x: i32, y: i32) -> Result<()> {
    use x11rb::protocol::xtest::ConnectionExt as _;
    let (conn, screen_num) = connect_x11_for_input()?;
    let root = conn.setup().roots[screen_num].root;
    conn.xtest_fake_input(MOTION_NOTIFY_EVENT, 0, 0, root, x as i16, y as i16, 0)?;
    conn.flush()?;
    let _ = conn.get_input_focus()?.reply();
    Ok(())
}

/// Scroll the window under a screen-absolute point via real XTest wheel-button
/// events. X11 buttons 4/5 are vertical up/down and 6/7 horizontal left/right.
pub fn send_scroll_xtest_desktop(x: i32, y: i32, direction: &str, amount: usize) -> Result<()> {
    use x11rb::protocol::xtest::ConnectionExt as _;
    let button = match direction {
        "up" => 4,
        "down" => 5,
        "left" => 6,
        "right" => 7,
        other => anyhow::bail!("unknown desktop scroll direction: {other}"),
    };
    let (conn, screen_num) = connect_x11_for_input()?;
    let root = conn.setup().roots[screen_num].root;
    conn.xtest_fake_input(MOTION_NOTIFY_EVENT, 0, 0, root, x as i16, y as i16, 0)?;
    for _ in 0..amount.max(1) {
        conn.xtest_fake_input(BUTTON_PRESS_EVENT, button, 0, root, x as i16, y as i16, 0)?;
        conn.xtest_fake_input(BUTTON_RELEASE_EVENT, button, 0, root, x as i16, y as i16, 0)?;
    }
    conn.flush()?;
    let _ = conn.get_input_focus()?.reply();
    Ok(())
}

/// Screen-absolute drag via XTest. The caller activates the target first; XTest
/// then supplies one real press, interpolated pointer motion, and one release.
/// This is the foreground counterpart to the window-addressed XSendEvent drag.
pub fn send_drag_xtest_desktop(
    from_x: i32,
    from_y: i32,
    to_x: i32,
    to_y: i32,
    button: u8,
    duration_ms: u64,
    steps: usize,
) -> Result<()> {
    use x11rb::protocol::xtest::ConnectionExt as _;
    let (conn, screen_num) = connect_x11_for_input()?;
    let root = conn.setup().roots[screen_num].root;
    let steps = steps.max(1);
    let delay = duration_ms / steps as u64;

    conn.xtest_fake_input(
        MOTION_NOTIFY_EVENT,
        0,
        0,
        root,
        from_x as i16,
        from_y as i16,
        0,
    )?;
    conn.xtest_fake_input(
        BUTTON_PRESS_EVENT,
        button,
        0,
        root,
        from_x as i16,
        from_y as i16,
        0,
    )?;
    conn.flush()?;
    for step in 1..=steps {
        let t = step as f64 / steps as f64;
        let x = from_x as f64 + (to_x - from_x) as f64 * t;
        let y = from_y as f64 + (to_y - from_y) as f64 * t;
        conn.xtest_fake_input(
            MOTION_NOTIFY_EVENT,
            0,
            0,
            root,
            x.round() as i16,
            y.round() as i16,
            0,
        )?;
        conn.flush()?;
        if delay > 0 {
            sleep(Duration::from_millis(delay));
        }
    }
    conn.xtest_fake_input(
        BUTTON_RELEASE_EVENT,
        button,
        0,
        root,
        to_x as i16,
        to_y as i16,
        0,
    )?;
    conn.flush()?;
    let _ = conn.get_input_focus()?.reply();
    Ok(())
}

/// Send a named key press to a window.
pub fn send_key(xid: u64, key: &str, modifiers: &[&str]) -> Result<()> {
    send_key_to_target(xid, None, key, modifiers)
}

/// Send a named key to the deepest child at window-local coordinates without
/// activating the window. Coordinate keyboard actions use this so embedded
/// Chromium renderers receive the event on their input surface rather than on
/// the native top-level wrapper.
pub fn send_key_at(xid: u64, x: i32, y: i32, key: &str, modifiers: &[&str]) -> Result<()> {
    send_key_to_target(xid, Some((x, y)), key, modifiers)
}

fn send_key_to_target(
    xid: u64,
    point: Option<(i32, i32)>,
    key: &str,
    modifiers: &[&str],
) -> Result<()> {
    let (conn, _) = connect_x11_for_input()?;
    let target = point
        .map(|(x, y)| resolve_event_target(&conn, xid, x, y))
        .transpose()?;
    let window = target.map(|target| target.window).unwrap_or(xid as u32);
    let root = conn.setup().roots[0].root;

    // Resolve the named key to a keysym, then to a keycode. On sparse/headless
    // keymaps (e.g. a minimal Xwayland :0) the keysym may have no keycode at all
    // — historically this failed with "Keysym 0x.. not in keyboard map".
    // `keycode_for_keysym` instead borrows a spare keycode and hands back a guard
    // that restores the original mapping once the event has been delivered.
    let keysym = key_name_to_keysym(key)?;
    let mapping = conn.get_keyboard_mapping(8, 248)?.reply()?;
    let (keycode, remap_guard) = keycode_for_keysym(&conn, &mapping, keysym, key)?;

    // XSendEvent's state mask describes modifiers for one key event, but does
    // not update Chromium's internal modifier state by itself. Emit the
    // modifier transitions as part of the background chord so web handlers see
    // the same ordered sequence as physical input without activating the
    // target window.
    let mut remap_guards = Vec::new();
    let mut modifier_keycodes = Vec::new();
    for modifier in modifiers {
        let modifier_keysym = key_name_to_keysym(modifier)?;
        let (modifier_keycode, guard) =
            keycode_for_keysym(&conn, &mapping, modifier_keysym, modifier)?;
        if let Some(guard) = guard {
            remap_guards.push(guard);
        }
        modifier_keycodes.push((modifier_keycode, modifiers_to_state(&[*modifier])));
    }

    let send_key_event = |response_type, detail, state, event_mask| {
        let event = KeyPressEvent {
            response_type,
            detail,
            sequence: 0,
            time: x11rb::CURRENT_TIME,
            root,
            event: window,
            child: x11rb::NONE,
            root_x: target.map(|target| target.root_x).unwrap_or(0),
            root_y: target.map(|target| target.root_y).unwrap_or(0),
            event_x: target.map(|target| target.local_x).unwrap_or(0),
            event_y: target.map(|target| target.local_y).unwrap_or(0),
            state,
            same_screen: true,
        };
        conn.send_event(false, window, event_mask, &event)
    };

    let mut state_bits = 0u16;
    for &(modifier_keycode, modifier_mask) in &modifier_keycodes {
        send_key_event(
            KEY_PRESS_EVENT,
            modifier_keycode,
            KeyButMask::from(state_bits),
            EventMask::KEY_PRESS,
        )?;
        state_bits |= u16::from(modifier_mask);
    }
    let state = KeyButMask::from(state_bits);
    send_key_event(KEY_PRESS_EVENT, keycode, state, EventMask::KEY_PRESS)?;
    // Otherwise X11 receives both transitions together after the sleep.
    conn.flush()?;
    sleep(Duration::from_millis(KEY_DELAY_MS));
    send_key_event(KEY_RELEASE_EVENT, keycode, state, EventMask::KEY_RELEASE)?;
    for &(modifier_keycode, modifier_mask) in modifier_keycodes.iter().rev() {
        send_key_event(
            KEY_RELEASE_EVENT,
            modifier_keycode,
            KeyButMask::from(state_bits),
            EventMask::KEY_RELEASE,
        )?;
        state_bits &= !u16::from(modifier_mask);
    }
    // Deliver releases before closing the connection, even without a key remap.
    conn.get_input_focus()?.reply()?;

    // If we borrowed a spare keycode for this keysym, give the target client a
    // moment to translate the synthetic event under the temporary mapping before
    // we restore it. The keycode->keysym lookup is client-side, so restoring too
    // eagerly would race delivery. A server round-trip (which only returns once
    // our queued requests have been processed) plus a short settle keeps that
    // race closed; the guard then reinstates the original keysyms on drop.
    if remap_guard.is_some() || !remap_guards.is_empty() {
        sleep(Duration::from_millis(KEY_DELAY_MS));
    }
    drop(remap_guard);
    Ok(())
}

/// Find the keycode that emits `keysym`, plus whether Shift must be held (the
/// keysym sits in the shifted column of the keyboard map). Prefers the
/// unshifted column when a keysym appears in both. Keysym for ASCII / Latin-1
/// is just the codepoint.
fn char_to_keycode_shift(mapping: &GetKeyboardMappingReply, keysym: u32) -> Option<(u8, bool)> {
    let per = mapping.keysyms_per_keycode as usize;
    if per == 0 {
        return None;
    }
    for (i, syms) in mapping.keysyms.chunks(per).enumerate() {
        if syms.first() == Some(&keysym) {
            return Some(((8 + i) as u8, false));
        }
        if per > 1 && syms.get(1) == Some(&keysym) {
            return Some(((8 + i) as u8, true));
        }
    }
    None
}

/// Map a human key name (e.g. "Return", "F5", "a") to its X11 keysym. Pure name
/// resolution — no server interaction — split out from keycode lookup so the
/// keysym can be remapped onto a spare keycode when the keymap lacks it.
fn key_name_to_keysym(key: &str) -> Result<u32> {
    // Common X11 keysym names.
    let keysym: u32 = match key.to_lowercase().as_str() {
        "return" | "enter" => 0xFF0D,
        "tab" => 0xFF09,
        "escape" | "esc" => 0xFF1B,
        "space" | " " => 0x0020,
        "backspace" => 0xFF08,
        "delete" | "del" => 0xFFFF,
        "insert" | "ins" => 0xFF63,
        "home" => 0xFF50,
        "end" => 0xFF57,
        "pageup" | "pgup" => 0xFF55,
        "pagedown" | "pgdn" => 0xFF56,
        "up" => 0xFF52,
        "down" => 0xFF54,
        "left" => 0xFF51,
        "right" => 0xFF53,
        "f1" => 0xFFBE,
        "f2" => 0xFFBF,
        "f3" => 0xFFC0,
        "f4" => 0xFFC1,
        "f5" => 0xFFC2,
        "f6" => 0xFFC3,
        "f7" => 0xFFC4,
        "f8" => 0xFFC5,
        "f9" => 0xFFC6,
        "f10" => 0xFFC7,
        "f11" => 0xFFC8,
        "f12" => 0xFFC9,
        "shift" => 0xFFE1,
        "ctrl" | "control" => 0xFFE3,
        "alt" => 0xFFE9,
        "super" | "meta" | "win" => 0xFFEB,
        "capslock" => 0xFFE5,
        "numlock" => 0xFF7F,
        // Common X keysym names for punctuation. The single-char branch below
        // already resolves the literal glyph ("+", "=", "*", "/"), but callers
        // that speak the X keysym-name vocabulary may pass the spelled-out name.
        // For the ASCII range the keysym value equals the codepoint.
        "plus" => 0x2B,
        "minus" | "dash" => 0x2D,
        "equal" | "equals" => 0x3D,
        "asterisk" | "star" => 0x2A,
        "slash" => 0x2F,
        "backslash" => 0x5C,
        "period" | "dot" => 0x2E,
        "comma" => 0x2C,
        "semicolon" => 0x3B,
        "colon" => 0x3A,
        "underscore" => 0x5F,
        "parenleft" => 0x28,
        "parenright" => 0x29,
        s if s.len() == 1 => s.chars().next().unwrap() as u32,
        _ => anyhow::bail!("Unknown key: {key}"),
    };
    Ok(keysym)
}

/// A keycode we have *temporarily* rebound to host a keysym that is absent from
/// the current X keyboard map (sparse/headless keymaps such as a minimal
/// Xwayland). On drop it reinstates the keycode's original keysyms so the
/// server's mapping is left exactly as we found it. Modelled on xdotool's
/// remap-a-spare-keycode trick (`_xdo_charcodemap` / `XChangeKeyboardMapping`).
struct RemappedKeycode<'a> {
    conn: &'a RustConnection,
    keycode: u8,
    keysyms_per_keycode: u8,
    original_keysyms: Vec<u32>,
}

impl Drop for RemappedKeycode<'_> {
    fn drop(&mut self) {
        // Best-effort restore: re-install the original keysyms for this keycode
        // and flush. Errors are swallowed deliberately — Drop must not panic in
        // the daemon, and the worst case of a failed restore is a single spare
        // keycode left mapped (it was unused to begin with), never a crash.
        let _ = self.conn.change_keyboard_mapping(
            1,
            self.keycode,
            self.keysyms_per_keycode,
            &self.original_keysyms,
        );
        let _ = self.conn.flush();
    }
}

/// Temporarily bind `keysym` onto a spare (fully unused) keycode so it can be
/// injected even when no existing keycode emits it. Returns a guard that
/// restores the original mapping on drop. Errors only if the keymap has no free
/// keycode left to borrow.
fn remap_spare_keycode<'a>(
    conn: &'a RustConnection,
    mapping: &GetKeyboardMappingReply,
    keysym: u32,
) -> Result<RemappedKeycode<'a>> {
    let per = mapping.keysyms_per_keycode as usize;
    if per == 0 {
        bail!("empty keyboard mapping; cannot remap keysym 0x{keysym:X}");
    }

    // Find a keycode whose every keysym slot is NoSymbol (0) — i.e. completely
    // unused — so borrowing it cannot clobber a real key. Scan high-to-low:
    // high keycodes are far likelier to be free than the low, populated ones.
    let spare = mapping
        .keysyms
        .chunks(per)
        .enumerate()
        .rev()
        .find(|(_, syms)| syms.iter().all(|&s| s == 0))
        .map(|(i, _)| (8 + i) as u8)
        .ok_or_else(|| anyhow!("no spare keycode available to remap keysym 0x{keysym:X}"))?;

    // Snapshot the original keysyms (all NoSymbol, but capture them so restore is
    // exact regardless), then bind the requested keysym across every column of
    // the borrowed keycode so it resolves irrespective of modifier state/group.
    let idx = (spare as usize - 8) * per;
    let original_keysyms = mapping.keysyms[idx..idx + per].to_vec();
    let new_keysyms = vec![keysym; per];
    conn.change_keyboard_mapping(1, spare, per as u8, &new_keysyms)?;
    // Round-trip so the server has installed the new mapping before we emit the
    // key event against it.
    let _ = conn.get_input_focus()?.reply();

    Ok(RemappedKeycode {
        conn,
        keycode: spare,
        keysyms_per_keycode: per as u8,
        original_keysyms,
    })
}

/// Resolve `keysym` to a keycode usable in a synthetic key event. First scans
/// the existing keyboard mapping; if no keycode emits the keysym (common on
/// sparse headless keymaps like a minimal Xwayland) it borrows a spare keycode
/// and remaps it, returning a guard that restores the original mapping on drop.
/// The guard is `None` when the keysym was already present (no cleanup needed).
fn keycode_for_keysym<'a>(
    conn: &'a RustConnection,
    mapping: &GetKeyboardMappingReply,
    keysym: u32,
    key: &str,
) -> Result<(u8, Option<RemappedKeycode<'a>>)> {
    let per = mapping.keysyms_per_keycode as usize;
    if per > 0 {
        for (i, syms) in mapping.keysyms.chunks(per).enumerate() {
            if syms.iter().any(|&s| s == keysym) {
                return Ok(((8 + i) as u8, None));
            }
        }
    }

    // Not in the map — fall back to remapping a spare keycode (xdotool-style).
    let guard = remap_spare_keycode(conn, mapping, keysym).with_context(|| {
        format!("Keysym 0x{keysym:X} not in keyboard map for key '{key}' and no spare keycode could be remapped")
    })?;
    let keycode = guard.keycode;
    Ok((keycode, Some(guard)))
}

fn modifiers_to_state(modifiers: &[&str]) -> KeyButMask {
    let mut state = 0u16;
    for m in modifiers {
        match m.to_lowercase().as_str() {
            "shift" => state |= u16::from(KeyButMask::SHIFT),
            "ctrl" | "control" => state |= u16::from(KeyButMask::CONTROL),
            "alt" | "mod1" => state |= u16::from(KeyButMask::MOD1),
            "super" | "mod4" | "win" | "meta" => state |= u16::from(KeyButMask::MOD4),
            _ => {}
        }
    }
    KeyButMask::from(state)
}

/// Inject text into a Tk window via Tk's `send` command — the Tk-specific
/// override for focus-free writes (Tk has no AT-SPI bridge). Requires the target
/// app to have registered itself with a known name via `tk appname <name>`.
/// Returns Ok(true) if text was sent, Ok(false) if the target isn't reachable
/// (not a Tk app or `wish` unavailable), Err on a send failure.
pub fn inject_tk_send(text: &str) -> Result<bool> {
    use std::io::Write;

    // Escape the text for safe Tcl interpolation (braces for literal strings).
    // Tcl's `send` command: `send <target-app-name> <tcl-command>`.
    // We target "cua-tk-target" (the name the test app registers with) and
    // insert at the entry widget's current cursor position.
    let tcl_text = text
        .replace("\\", "\\\\")
        .replace("{", "\\{")
        .replace("}", "\\}");

    // Tk's `send` is synchronous: it blocks the sender until the *target's* Tcl
    // event loop services the request and replies. If the target is wedged, or
    // the X server refuses `send` (SECURITY ext / xauth mismatch), it can block
    // forever. Guard against that two ways:
    //   1. A Tcl-level `after` timer that force-exits wish if the send hasn't
    //      completed in time. We issue the write with `send -async` so the local
    //      event loop stays live to fire the timer, then `vwait` on a flag.
    //   2. A Rust-level wall-clock kill below, so even a totally wedged wish
    //      (e.g. blocked before reaching the event loop) can't hang the driver.
    let tcl_script = format!(
        r#"set ::done 0
set ::rc 0
after 5000 {{ set ::rc 2; set ::done 1 }}
if {{[catch {{send -async cua-tk-target {{.entry insert insert {{{}}}}}}} err]}} {{
    puts stderr "tk send failed: $err"
    exit 1
}}
after 500 {{ set ::done 1 }}
vwait ::done
if {{$::rc == 2}} {{
    puts stderr "tk send timed out"
    exit 1
}}
exit 0"#,
        tcl_text
    );

    // Try to spawn wish (Tk's shell). If it's not available, this isn't a
    // Tk-based environment and we should fall back to XSendEvent.
    let mut child = match std::process::Command::new("wish")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return Ok(false),
    };

    if let Some(mut stdin) = child.stdin.take() {
        // Ignore write errors: if wish already exited we observe it via wait().
        let _ = stdin.write_all(tcl_script.as_bytes());
        // stdin drops here → EOF, so wish runs the script to completion.
    }

    // Wall-clock backstop: poll for exit and hard-kill if wish overruns the
    // deadline. Guarantees the driver task can never hang on a blocked Tk send,
    // regardless of whether the Tcl-level timer fired.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        match child.try_wait()? {
            Some(status) => {
                let mut stderr = String::new();
                if let Some(mut err) = child.stderr.take() {
                    use std::io::Read;
                    let _ = err.read_to_string(&mut stderr);
                }
                if status.success() {
                    return Ok(true);
                }
                // Target not registered or send timed out → not a usable Tk
                // target; let the caller fall back to XSendEvent.
                if stderr.contains("application named")
                    || stderr.contains("no registered")
                    || stderr.contains("timed out")
                {
                    return Ok(false);
                }
                anyhow::bail!("wish send failed: {}", stderr);
            }
            None => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Ok(false);
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    }
}

#[cfg(test)]
mod path_tests {
    use super::{
        create_uinput_pointer, ensure_master_pointer_for_session, guarded_uinput_creation,
        is_uinput_unavailable, kde_x11_uinput_hotplug_is_unsafe, master_pointer_name,
        modifiers_to_state, normalize_uinput_device_name, path_cumulative, point_on_path,
        real_pointer_capabilities_available, sample_function, slave_pointer_name,
        EVDEV_UINPUT_NAME_MAX_BYTES, UINPUT_POINTER_SUFFIX,
    };
    use x11rb::protocol::xproto::KeyButMask;

    #[test]
    fn click_modifier_state_combines_canonical_names_and_aliases() {
        let state = modifiers_to_state(&["ctrl", "shift", "meta"]);
        assert!(state.contains(KeyButMask::CONTROL));
        assert!(state.contains(KeyButMask::SHIFT));
        assert!(state.contains(KeyButMask::MOD4));
        assert!(!state.contains(KeyButMask::MOD1));

        assert_eq!(
            modifiers_to_state(&["control", "alt"]),
            KeyButMask::from(u16::from(KeyButMask::CONTROL) | u16::from(KeyButMask::MOD1))
        );
    }

    #[test]
    fn idle_reaper_selects_only_sessions_past_the_ttl() {
        use super::{stale_cursor_ids, MPX_IDLE_TTL};
        let now = std::time::Instant::now();
        let mut last_use = std::collections::HashMap::new();
        last_use.insert("fresh".to_owned(), now);
        last_use.insert("old".to_owned(), now - std::time::Duration::from_secs(400));
        last_use.insert("edge".to_owned(), now - MPX_IDLE_TTL);
        let mut stale = stale_cursor_ids(&last_use, now, MPX_IDLE_TTL);
        stale.sort();
        assert_eq!(stale, vec!["edge".to_owned(), "old".to_owned()]);
        assert!(stale_cursor_ids(&std::collections::HashMap::new(), now, MPX_IDLE_TTL).is_empty());
    }

    #[test]
    fn slave_pointer_name_fits_evdev_uinput_limit() {
        for cursor_id in ["m".repeat(200), "cursor-鼠".repeat(50)] {
            let name = slave_pointer_name(&master_pointer_name(&cursor_id));
            assert!(
                name.len() <= 78,
                "evdev 0.12 requires uinput names to be at most 78 bytes, got {}",
                name.len()
            );
        }

        let cursor_id = "same-long-cursor".repeat(20);
        let first = slave_pointer_name(&master_pointer_name(&cursor_id));
        let second = slave_pointer_name(&master_pointer_name(&cursor_id));
        assert_ne!(first, second, "truncation must retain the unique nonce");
        assert!(first.ends_with(UINPUT_POINTER_SUFFIX));
        assert!(second.ends_with(UINPUT_POINTER_SUFFIX));
    }

    #[test]
    fn uinput_name_normalization_covers_byte_boundaries_and_multibyte_text() {
        let exact = "a".repeat(EVDEV_UINPUT_NAME_MAX_BYTES);
        assert_eq!(normalize_uinput_device_name(&exact), exact);

        let overlong_ascii = "a".repeat(EVDEV_UINPUT_NAME_MAX_BYTES + 1);
        assert_eq!(
            normalize_uinput_device_name(&overlong_ascii),
            "a".repeat(EVDEV_UINPUT_NAME_MAX_BYTES)
        );

        let exact_multibyte = format!("{}鼠", "a".repeat(75));
        assert_eq!(exact_multibyte.len(), EVDEV_UINPUT_NAME_MAX_BYTES);
        assert_eq!(
            normalize_uinput_device_name(&exact_multibyte),
            exact_multibyte
        );

        let split_multibyte = format!("{}鼠", "a".repeat(77));
        let normalized = normalize_uinput_device_name(&split_multibyte);
        assert_eq!(normalized, "a".repeat(77));
        assert!(normalized.is_char_boundary(normalized.len()));

        assert_eq!(
            normalize_uinput_device_name("CUA\0pointer\n"),
            "CUA_pointer_"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn uinput_creation_panic_is_contained_and_daemon_worker_remains_usable() {
        let failed = tokio::task::spawn_blocking(|| {
            guarded_uinput_creation::<()>("panic", |_| panic!("synthetic evdev panic"))
        })
        .await
        .expect("the blocking worker must not unwind");
        let error = failed.expect_err("the panic must become an error");
        assert!(is_uinput_unavailable(&error));
        assert!(error.to_string().contains("device creation panicked"));

        let subsequent = tokio::task::spawn_blocking(|| {
            guarded_uinput_creation("subsequent", |name| Ok(name.to_owned()))
        })
        .await
        .expect("the runtime must remain usable after the contained panic")
        .expect("a subsequent device operation must succeed");
        assert_eq!(subsequent, "subsequent");
    }

    #[test]
    fn uinput_creation_error_is_stably_typed() {
        let error =
            guarded_uinput_creation::<()>("failure", |_| anyhow::bail!("permission denied"))
                .expect_err("the injected builder error must be returned");
        assert!(is_uinput_unavailable(&error));
        assert_eq!(
            error.to_string(),
            "Linux uinput device unavailable: permission denied"
        );
    }

    #[test]
    #[ignore = "requires a writable /dev/uinput device"]
    fn real_uinput_accepts_normalized_overlong_multibyte_name() {
        let overlong_name = format!("CUA {}{UINPUT_POINTER_SUFFIX}", "鼠".repeat(100));
        let device = create_uinput_pointer(&overlong_name)
            .expect("normalized device name should create a real uinput pointer");
        drop(device);
    }

    #[test]
    fn real_pointer_capabilities_require_uinput_access() {
        assert!(real_pointer_capabilities_available(
            true, false, true, false
        ));
        assert!(!real_pointer_capabilities_available(
            true, false, false, false
        ));
        assert!(!real_pointer_capabilities_available(
            false, false, true, false
        ));
        assert!(!real_pointer_capabilities_available(
            true, true, true, false
        ));
        assert!(!real_pointer_capabilities_available(
            true, false, true, true
        ));
    }

    #[test]
    fn kde_x11_sessions_disable_uinput_pointer_hotplug() {
        assert!(kde_x11_uinput_hotplug_is_unsafe(
            Some("x11"),
            Some("KDE"),
            None,
            None,
            None,
            Some(":0"),
            None,
        ));
        assert!(kde_x11_uinput_hotplug_is_unsafe(
            Some("x11"),
            Some("KDE"),
            None,
            None,
            None,
            Some(":0"),
            Some("wayland-0"),
        ));
        assert!(kde_x11_uinput_hotplug_is_unsafe(
            None,
            None,
            Some("plasma"),
            None,
            Some("true"),
            Some(":1"),
            None,
        ));

        assert!(!kde_x11_uinput_hotplug_is_unsafe(
            Some("wayland"),
            Some("KDE"),
            None,
            None,
            Some("true"),
            Some(":0"),
            Some("wayland-0"),
        ));
        assert!(!kde_x11_uinput_hotplug_is_unsafe(
            Some("x11"),
            Some("GNOME"),
            None,
            None,
            None,
            Some(":0"),
            None,
        ));

        let error = ensure_master_pointer_for_session("regression-test", true)
            .expect_err("the creation choke point must refuse before opening X11 or uinput");
        assert!(is_uinput_unavailable(&error));
        assert!(error.to_string().contains("delivery_mode='foreground'"));
    }

    #[test]
    fn sample_linear_function() {
        let pts = sample_function("x", 0.0, 10.0, 11).unwrap();
        assert_eq!(pts.len(), 11);
        assert_eq!(pts.first().unwrap(), &(0.0, 0.0));
        assert_eq!(pts.last().unwrap(), &(10.0, 10.0));
        assert!((pts[5].0 - 5.0).abs() < 1e-9 && (pts[5].1 - 5.0).abs() < 1e-9);
    }

    #[test]
    fn sample_affine_and_trig() {
        let pts = sample_function("2*x+1", 0.0, 4.0, 5).unwrap();
        for (x, y) in pts {
            assert!((y - (2.0 * x + 1.0)).abs() < 1e-9);
        }
        // sin(x) parses and yields finite, bounded values.
        let s = sample_function("100+50*sin(x)", 0.0, 6.28, 40).unwrap();
        assert!(s.iter().all(|(_, y)| (49.9..=150.1).contains(y)));
    }

    #[test]
    fn invalid_expression_errors() {
        assert!(sample_function("x +", 0.0, 1.0, 4).is_err());
        assert!(sample_function("3*z", 0.0, 1.0, 4).is_err()); // unknown var
    }

    #[test]
    fn non_finite_points_are_dropped() {
        // ln(x) is -inf/NaN for x<=0; the finite tail must still sample.
        let pts = sample_function("ln(x)", -2.0, 5.0, 50).unwrap();
        assert!(pts.iter().all(|(_, y)| y.is_finite()));
        assert!(pts.len() >= 2);
    }

    #[test]
    fn cumulative_lengths_and_total() {
        // 3-4-5 triangle then a zero-length repeat.
        let path = [(0, 0), (3, 4), (3, 4)];
        let (cum, total) = path_cumulative(&path);
        assert_eq!(cum.len(), 3);
        assert!((cum[0] - 0.0).abs() < 1e-9);
        assert!((cum[1] - 5.0).abs() < 1e-9);
        assert!((cum[2] - 5.0).abs() < 1e-9);
        assert!((total - 5.0).abs() < 1e-9);
    }

    #[test]
    fn straight_segment_interpolates_by_fraction() {
        let path = [(0, 0), (10, 0)];
        let (cum, total) = path_cumulative(&path);
        assert_eq!(point_on_path(&path, &cum, total, 0.0), (0, 0));
        assert_eq!(point_on_path(&path, &cum, total, 0.5), (5, 0));
        assert_eq!(point_on_path(&path, &cum, total, 1.0), (10, 0));
    }

    #[test]
    fn multi_segment_follows_arc_length() {
        // L-shape: (0,0)->(10,0)->(10,10), total length 20.
        let path = [(0, 0), (10, 0), (10, 10)];
        let (cum, total) = path_cumulative(&path);
        assert!((total - 20.0).abs() < 1e-9);
        // Halfway by arc length lands exactly on the corner.
        assert_eq!(point_on_path(&path, &cum, total, 0.5), (10, 0));
        // 3/4 of the way is 5px down the second segment.
        assert_eq!(point_on_path(&path, &cum, total, 0.75), (10, 5));
    }

    #[test]
    fn fraction_is_clamped_and_endpoints_exact() {
        let path = [(2, 2), (8, 2), (8, 8)];
        let (cum, total) = path_cumulative(&path);
        // t past the ends clamps to the terminal points (no overshoot).
        assert_eq!(point_on_path(&path, &cum, total, -0.5), (2, 2));
        assert_eq!(point_on_path(&path, &cum, total, 2.0), (8, 8));
    }

    #[test]
    fn degenerate_path_returns_last_point() {
        let path = [(5, 5), (5, 5)];
        let (cum, total) = path_cumulative(&path);
        assert!((total - 0.0).abs() < 1e-9);
        assert_eq!(point_on_path(&path, &cum, total, 0.3), (5, 5));
    }
}

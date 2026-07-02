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

use anyhow::{anyhow, bail, Context, Result};
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::fs;
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::sleep;
use std::time::Duration;
use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, EventType, InputEvent, Key, RelativeAxisType};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::rust_connection::RustConnection;

const CLICK_DELAY_MS: u64 = 35;
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
pub fn sample_function(expr: &str, x_from: f64, x_to: f64, samples: u64) -> Result<Vec<(f64, f64)>> {
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
    let mut i = match cum.binary_search_by(|v| v.partial_cmp(&d).unwrap_or(std::cmp::Ordering::Less)) {
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

#[derive(Clone, Copy, Debug)]
struct MasterPointerIds {
    pointer_id: i32,
    keyboard_id: i32,
    slave_pointer_id: i32,
}

static MPX_POINTERS: OnceLock<Mutex<HashMap<String, MasterPointerIds>>> = OnceLock::new();
static UINPUT_POINTERS: OnceLock<Mutex<HashMap<String, Arc<Mutex<VirtualDevice>>>>> = OnceLock::new();
static XLIB_THREADS_READY: OnceLock<Result<(), String>> = OnceLock::new();
static MPX_NAME_COUNTER: AtomicU64 = AtomicU64::new(1);

fn mpx_pointers() -> &'static Mutex<HashMap<String, MasterPointerIds>> {
    MPX_POINTERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn uinput_pointers() -> &'static Mutex<HashMap<String, Arc<Mutex<VirtualDevice>>>> {
    UINPUT_POINTERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn master_pointer_name(cursor_id: &str) -> String {
    let nonce = MPX_NAME_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("CUA {cursor_id} mp-{}-{nonce}", std::process::id())
}

fn slave_pointer_name(master_name: &str) -> String {
    format!("{master_name} uinput pointer")
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

fn xi2_query_devices(
    display: *mut x11::xlib::Display,
) -> Result<Vec<(i32, i32, String)>> {
    let mut count = 0;
    let ptr = unsafe { x11::xinput2::XIQueryDevice(display, x11::xinput2::XIAllDevices, &mut count) };
    if ptr.is_null() {
        bail!("XIQueryDevice returned null");
    }
    let mut out = Vec::new();
    for i in 0..count {
        let info = unsafe { *ptr.add(i as usize) };
        let name = if info.name.is_null() {
            String::new()
        } else {
            unsafe { CStr::from_ptr(info.name) }.to_string_lossy().into_owned()
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
    unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned()
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
pub fn real_pointer_input_available() -> bool {
    let Ok(display) = open_display() else {
        return false;
    };
    let supported = supports_parallel_pointer_injection(display).is_ok() && !is_xvfb_process_running();
    unsafe { x11::xlib::XCloseDisplay(display) };
    supported
}

fn ensure_master_pointer(cursor_id: &str) -> Result<MasterPointerIds> {
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

    let pointer_id = pointer_id.ok_or_else(|| anyhow!("failed to locate created master pointer for '{cursor_id}'"))?;
    let keyboard_id = keyboard_id.ok_or_else(|| anyhow!("failed to locate created master keyboard for '{cursor_id}'"))?;

    let device_name = slave_pointer_name(&base);
    let uinput_device = create_uinput_pointer(&device_name)?;
    let slave_pointer_id = wait_for_slave_pointer_id(display, &device_name)?;
    attach_slave_to_master(display, slave_pointer_id, pointer_id)?;
    set_flat_pointer_accel(display, slave_pointer_id);
    unsafe { x11::xlib::XCloseDisplay(display) };

    let ids = MasterPointerIds { pointer_id, keyboard_id, slave_pointer_id };
    mpx_pointers().lock().unwrap().insert(cursor_id.to_owned(), ids);
    uinput_pointers()
        .lock()
        .unwrap()
        .insert(cursor_id.to_owned(), Arc::new(Mutex::new(uinput_device)));
    Ok(ids)
}

pub fn forget_master_pointer(cursor_id: &str) {
    uinput_pointers().lock().unwrap().remove(cursor_id);
    let Some(ids) = mpx_pointers().lock().unwrap().remove(cursor_id) else {
        return;
    };

    let Ok(display) = open_display() else {
        return;
    };

    let Ok(devices) = xi2_query_devices(display) else {
        unsafe { x11::xlib::XCloseDisplay(display) };
        return;
    };

    let mut virtual_core_pointer = None;
    let mut virtual_core_keyboard = None;
    for (device_id, use_, device_name) in devices {
        if device_name == "Virtual core pointer" && use_ == x11::xinput2::XIMasterPointer {
            virtual_core_pointer = Some(device_id);
        } else if device_name == "Virtual core keyboard" && use_ == x11::xinput2::XIMasterKeyboard {
            virtual_core_keyboard = Some(device_id);
        }
    }

    let (Some(return_pointer), Some(return_keyboard)) = (virtual_core_pointer, virtual_core_keyboard) else {
        unsafe { x11::xlib::XCloseDisplay(display) };
        return;
    };

    let mut change = x11::xinput2::XIAnyHierarchyChangeInfo::default();
    unsafe {
        let remove = change.remove();
        (*remove)._type = x11::xinput2::XIRemoveMaster;
        (*remove).deviceid = ids.pointer_id;
        (*remove).return_mode = x11::xinput2::XIAttachToMaster;
        (*remove).return_pointer = return_pointer;
        (*remove).return_keyboard = return_keyboard;
        let _ = x11::xinput2::XIChangeHierarchy(display, &mut change, 1);
        x11::xlib::XSync(display, 0);
        x11::xlib::XCloseDisplay(display);
    }
}

fn create_uinput_pointer(name: &str) -> Result<VirtualDevice> {
    let mut keys = AttributeSet::<Key>::new();
    keys.insert(Key::BTN_LEFT);
    keys.insert(Key::BTN_RIGHT);
    keys.insert(Key::BTN_MIDDLE);

    let mut rel_axes = AttributeSet::<RelativeAxisType>::new();
    rel_axes.insert(RelativeAxisType::REL_X);
    rel_axes.insert(RelativeAxisType::REL_Y);
    // REL_WHEEL (vertical) and REL_HWHEEL (horizontal) so the same uinput slave
    // can also drive scroll: libinput turns these into the XI2 smooth-scroll
    // events GTK consumes, where synthetic Button4-7 XSendEvents are dropped.
    rel_axes.insert(RelativeAxisType::REL_WHEEL);
    rel_axes.insert(RelativeAxisType::REL_HWHEEL);

    Ok(
        evdev::uinput::VirtualDeviceBuilder::new()?
            .name(name)
            .with_keys(&keys)?
            .with_relative_axes(&rel_axes)?
            .build()?,
    )
}

fn wait_for_slave_pointer_id(display: *mut x11::xlib::Display, device_name: &str) -> Result<i32> {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        for (device_id, use_, seen_name) in xi2_query_devices(display)? {
            if use_ == x11::xinput2::XISlavePointer && seen_name == device_name {
                return Ok(device_id);
            }
        }
        if std::time::Instant::now() >= deadline {
            bail!("timed out waiting for X input slave pointer '{device_name}'");
        }
        sleep(Duration::from_millis(50));
    }
}

fn attach_slave_to_master(display: *mut x11::xlib::Display, slave_pointer_id: i32, master_pointer_id: i32) -> Result<()> {
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

fn warp_master_pointer(display: *mut x11::xlib::Display, ids: MasterPointerIds, x: i32, y: i32) -> Result<()> {
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

/// XIAnyModifier (1u32 << 31). The x11 crate doesn't export it.
const XI_ANY_MODIFIER: std::os::raw::c_int = 0x8000_0000u32 as std::os::raw::c_int;

fn xi_mask_len() -> usize {
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

/// Install a device-specific XI2 synchronous passive button grab on `window`
/// for `device_id`. This shields the drag: the grab is newer than (and thus
/// checked before) the window manager's click-to-focus grab on the same
/// window, and being device-specific it does not conflict with the WM's
/// core/all-master grabs. The matching press freezes the device and is
/// delivered to us; replaying it (XIReplayDevice) re-checks grabs only
/// *below* this window and then delivers the event normally to the app, so
/// the WM never sees the press and never steals focus.
fn install_shield_grab(
    display: *mut x11::xlib::Display,
    device_id: i32,
    window: x11::xlib::Window,
    button: u8,
) -> Result<()> {
    let mut mask_bits = vec![0u8; xi_mask_len()];
    x11::xinput2::XISetMask(&mut mask_bits, x11::xinput2::XI_ButtonPress);
    let mut evmask = x11::xinput2::XIEventMask {
        deviceid: device_id,
        mask_len: mask_bits.len() as std::os::raw::c_int,
        mask: mask_bits.as_mut_ptr(),
    };
    let mut mods = x11::xinput2::XIGrabModifiers {
        modifiers: XI_ANY_MODIFIER,
        status: 0,
    };
    let rc = unsafe {
        x11::xinput2::XIGrabButton(
            display,
            device_id,
            button as std::os::raw::c_int,
            window,
            0, // cursor: None
            x11::xinput2::XIGrabModeSync,  // freeze the pointer on press
            x11::xinput2::XIGrabModeAsync, // leave the paired keyboard alone
            x11::xlib::False,              // owner_events: deliver to us
            &mut evmask,
            1,
            &mut mods,
        )
    };
    unsafe { x11::xlib::XSync(display, 0) };
    if rc != 0 {
        bail!("XIGrabButton(shield) failed with status {rc}");
    }
    Ok(())
}

fn remove_shield_grab(display: *mut x11::xlib::Display, device_id: i32, window: x11::xlib::Window, button: u8) {
    let mut mods = x11::xinput2::XIGrabModifiers {
        modifiers: XI_ANY_MODIFIER,
        status: 0,
    };
    unsafe {
        let prev = x11::xlib::XSetErrorHandler(Some(ignore_x_error));
        x11::xinput2::XIUngrabButton(display, device_id, button as std::os::raw::c_int, window, 1, &mut mods);
        x11::xlib::XSync(display, 0);
        x11::xlib::XSetErrorHandler(prev);
    }
}

/// Drain the frozen shield presses for `pending_devices` and replay each so
/// it continues to the application. Returns the set of device ids we failed
/// to see within the timeout (their drags still proceed; the focus-restore
/// safety net covers any leak).
fn replay_shielded_presses(
    display: *mut x11::xlib::Display,
    xi_opcode: std::os::raw::c_int,
    pending_devices: &mut std::collections::HashSet<i32>,
    timeout: Duration,
) {
    let deadline = std::time::Instant::now() + timeout;
    while !pending_devices.is_empty() && std::time::Instant::now() < deadline {
        // Only block on XNextEvent when something is queued, so a missing
        // press can't hang us past the deadline.
        if unsafe { x11::xlib::XPending(display) } == 0 {
            sleep(Duration::from_millis(2));
            continue;
        }
        let mut ev: x11::xlib::XEvent = unsafe { std::mem::zeroed() };
        unsafe { x11::xlib::XNextEvent(display, &mut ev) };
        if unsafe { ev.type_ } != x11::xlib::GenericEvent {
            continue;
        }
        let mut cookie = unsafe { ev.generic_event_cookie };
        if cookie.extension != xi_opcode || cookie.evtype != x11::xinput2::XI_ButtonPress {
            continue;
        }
        if unsafe { x11::xlib::XGetEventData(display, &mut cookie) } == 0 {
            continue;
        }
        let de = cookie.data as *const x11::xinput2::XIDeviceEvent;
        if !de.is_null() {
            let device_id = unsafe { (*de).deviceid };
            let time = unsafe { (*de).time };
            if pending_devices.remove(&device_id) {
                unsafe {
                    x11::xinput2::XIAllowEvents(display, device_id, x11::xinput2::XIReplayDevice, time);
                    x11::xlib::XSync(display, 0);
                }
            }
        }
        unsafe { x11::xlib::XFreeEventData(display, &mut cookie) };
    }
}

fn ewmh_active_window(display: *mut x11::xlib::Display) -> Option<x11::xlib::Window> {
    unsafe {
        let atom = x11::xlib::XInternAtom(
            display,
            c"_NET_ACTIVE_WINDOW".as_ptr(),
            x11::xlib::True,
        );
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
        while x11::xlib::XCheckWindowEvent(
            display,
            win,
            x11::xlib::PropertyChangeMask,
            &mut ev,
        ) != 0
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
        let atom = x11::xlib::XInternAtom(
            display,
            c"_NET_ACTIVE_WINDOW".as_ptr(),
            x11::xlib::True,
        );
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
        ev.data.set_long(1, x_server_time(display) as std::os::raw::c_long);
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

/// Foreground rung for X11 (`delivery_mode:"foreground"`): briefly activate
/// `xid` via EWMH `_NET_ACTIVE_WINDOW`, run `body` (which injects the input
/// while the window holds focus), then restore the prior active window. The
/// Linux analogue of macOS `with_foreground_assist` / the Windows foreground
/// swap, reusing the existing [`ewmh_active_window`] / [`ewmh_activate_window`]
/// primitives (proper `x_server_time` stamping beats the WM's focus-stealing
/// prevention).
///
/// Best-effort: if no X display can be opened the body still runs (without
/// activation) so a headless/Wayland path degrades rather than hard-fails.
/// `settle_ms` is the pause after activation before the first injected event —
/// the WM needs a moment to complete the focus swap (mirrors the macOS settle).
pub fn with_x11_foreground<T>(
    xid: u64,
    settle_ms: u64,
    body: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let display = unsafe { x11::xlib::XOpenDisplay(ptr::null()) };
    if display.is_null() {
        return body();
    }
    let prior = ewmh_active_window(display);
    ewmh_activate_window(display, xid as x11::xlib::Window, prior.unwrap_or(0));
    unsafe { x11::xlib::XSync(display, 0); }
    if settle_ms > 0 {
        std::thread::sleep(std::time::Duration::from_millis(settle_ms));
    }
    // EWMH `_NET_ACTIVE_WINDOW` is honored as *raise-only* by WMs with
    // focus-stealing prevention (e.g. KWin): the window reaches the top of the
    // stack — enough for a coordinate click, which lands by stacking — but the X
    // *input focus* never transfers, so XTest key events (which the server routes
    // to the focused window) go to whatever was focused before. Set the input
    // focus explicitly too, exactly as `xdotool windowactivate` does, so the
    // foreground key/hotkey rung actually reaches the target. Guarded against
    // BadMatch on a not-yet-viewable window — Xlib's default handler would exit
    // the process — reusing the scoped `ignore_x_error` pattern used elsewhere.
    unsafe {
        let prev_handler = x11::xlib::XSetErrorHandler(Some(ignore_x_error));
        x11::xlib::XSetInputFocus(
            display,
            xid as x11::xlib::Window,
            x11::xlib::RevertToParent,
            x11::xlib::CurrentTime,
        );
        x11::xlib::XSync(display, 0);
        x11::xlib::XSetErrorHandler(prev_handler);
    }
    let result = body();
    // Restore the prior active window (brief swap, like macOS/Windows).
    if let Some(p) = prior {
        ewmh_activate_window(display, p, xid as x11::xlib::Window);
        unsafe { x11::xlib::XSync(display, 0); }
    }
    unsafe { x11::xlib::XCloseDisplay(display); }
    result
}

/// Activate `xid` and LEAVE it active (no restore) — the persistent foreground
/// swap behind `bring_to_front`. Returns the window that was active before, so
/// the caller can report/inspect it. Best-effort; returns `None` prior on a
/// headless display.
pub fn x11_activate_window_persistent(xid: u64) -> Result<Option<u64>> {
    let display = unsafe { x11::xlib::XOpenDisplay(ptr::null()) };
    if display.is_null() {
        bail!("cannot activate window: no X display (DISPLAY={:?})", std::env::var("DISPLAY").ok());
    }
    let prior = ewmh_active_window(display).map(|w| w as u64);
    ewmh_activate_window(display, xid as x11::xlib::Window, prior.unwrap_or(0) as x11::xlib::Window);
    unsafe { x11::xlib::XSync(display, 0); x11::xlib::XCloseDisplay(display); }
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
    device.emit(&[InputEvent::new(EventType::KEY, code.0, if press { 1 } else { 0 })])?;
    Ok(())
}

fn emit_relative_motion(device: &mut VirtualDevice, dx: i32, dy: i32) -> Result<()> {
    let mut events = Vec::with_capacity(2);
    if dx != 0 {
        events.push(InputEvent::new(EventType::RELATIVE, RelativeAxisType::REL_X.0, dx));
    }
    if dy != 0 {
        events.push(InputEvent::new(EventType::RELATIVE, RelativeAxisType::REL_Y.0, dy));
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

pub fn send_parallel_virtual_pointer_drags(
    drags: &[(String, VirtualPointerDrag)],
) -> Result<()> {
    let display = open_display()?;
    supports_parallel_pointer_injection(display)?;
    let xi_opcode = xinput_opcode(display);

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

    // Click-to-focus WMs grab buttons for XIAllMasterDevices, so the drag's
    // press activates the target window exactly like a user click would.
    // Remember the focus state and hand it back afterwards so parallel
    // drags don't steal it.
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

        // Shield each drag from the WM's click-to-focus grab, then press.
        // Per item: install a device-specific sync grab on the target window,
        // warp, press, and immediately replay the frozen press so it reaches
        // the app while the WM stays blind to it. We replay each press before
        // emitting the next so only ONE device is ever frozen at a time — the
        // X server drops replayed presses when several devices are frozen on
        // the same window and replayed together. The few-ms stagger this adds
        // to the presses is invisible; the concurrency that matters is motion.
        // Shielding is mandatory: if install/replay fails, abort instead of
        // continuing with a drag that could steal focus and rely on restore.
        let mut shielded = std::collections::HashSet::new();
        for item in &active {
            let opcode = xi_opcode.ok_or_else(|| {
                anyhow!("parallel_mouse_drag requires XInput/XI2 shield grabs for no-focus-steal operation")
            })?;
            install_shield_grab(
                display,
                item.ids.pointer_id,
                item.drag.target_window as x11::xlib::Window,
                item.drag.button,
            )
            .with_context(|| format!("shield grab failed for '{}'", item.cursor_id))?;
            shielded.insert(item.ids.pointer_id);
            let start = *item.drag.path.first().unwrap_or(&(0, 0));
            warp_master_pointer(display, item.ids, start.0, start.1)?;
            {
                let mut device = item.device.lock().unwrap();
                emit_button(&mut device, item.drag.button, true)?;
            }
            let mut pending = std::collections::HashSet::from([item.ids.pointer_id]);
            replay_shielded_presses(display, opcode, &mut pending, Duration::from_millis(1000));
            if !pending.is_empty() {
                return Err(anyhow!(
                    "shield replay timed out before XI_ButtonPress arrived for '{}'",
                    item.cursor_id
                ));
            }
        }

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

        // Remove the shields now that the drag is done. The button is only
        // grabbed for ButtonPress, so the shield is dormant during motion and
        // release; this just stops it matching the next gesture's press.
        for item in &active {
            if shielded.contains(&item.ids.pointer_id) {
                remove_shield_grab(
                    display,
                    item.ids.pointer_id,
                    item.drag.target_window as x11::xlib::Window,
                    item.drag.button,
                );
            }
        }
        Ok(())
    })();
    // Remove the per-session masters before handing focus back: non-MPX-aware
    // WMs (xfwm4, openbox) desync their focus bookkeeping while foreign
    // master keyboards linger, and the next call recreates masters cheaply.
    for (cursor_id, _) in drags {
        forget_master_pointer(cursor_id);
    }
    restore_focus_state(display, &saved_focus);
    unsafe {
        x11::xlib::XCloseDisplay(display);
    }
    result
}

/// A discrete no-focus-steal pointer click driven through the same real-input
/// pipeline as [`send_parallel_virtual_pointer_drags`] — MPX master pointer +
/// uinput slave + XI2 shield grab — reduced to a press/release (or a short
/// press/release train for `count` > 1) at one screen point.
///
/// This is what lands **right / middle / double** clicks (and any left click
/// the AT-SPI path can't actuate) on XInput2 toolkits: GTK3/4 silently drop
/// synthetic `XSendEvent` pointer events and never see XTEST core events, so
/// those clicks are otherwise no-ops. Coordinates are screen-absolute;
/// `target_window` is the X11 window the shield grab is installed on so the WM
/// never sees the press and never steals focus. `button` is an X button number
/// (1=left, 2=middle, 3=right); `count` >= 1 (2 = double-click).
#[derive(Clone, Debug)]
pub struct VirtualPointerClick {
    pub target_window: u64,
    pub x: i32,
    pub y: i32,
    pub button: u8,
    pub count: usize,
}

/// Land a discrete click via the MPX real-input pipeline (see
/// [`VirtualPointerClick`]). Mirrors the per-item press/replay logic of
/// `send_parallel_virtual_pointer_drags`: install a device-specific synchronous
/// XI2 shield grab on the target window, warp the master pointer, then for each
/// press freeze→replay it so the application receives a real button event while
/// the WM stays blind to it. The master is torn down and focus restored on exit
/// (matching the drag) to keep non-MPX WMs' focus bookkeeping consistent.
pub fn send_virtual_pointer_click(cursor_id: &str, click: &VirtualPointerClick) -> Result<()> {
    let display = open_display()?;
    supports_parallel_pointer_injection(display)?;
    let xi_opcode = xinput_opcode(display);
    let saved_focus = save_focus_state(display);

    let result = (|| -> Result<()> {
        let ids = ensure_master_pointer(cursor_id)?;
        let device = uinput_pointers()
            .lock()
            .unwrap()
            .get(cursor_id)
            .cloned()
            .ok_or_else(|| anyhow!("missing uinput pointer for '{cursor_id}'"))?;
        let opcode = xi_opcode.ok_or_else(|| {
            anyhow!("no-focus-steal click requires XInput/XI2 shield grabs")
        })?;

        let window = click.target_window as x11::xlib::Window;
        install_shield_grab(display, ids.pointer_id, window, click.button)
            .with_context(|| format!("shield grab failed for '{cursor_id}'"))?;
        // Run the press train under a guard so the shield is always removed,
        // even on an early error mid-train.
        let click_result = (|| -> Result<()> {
            warp_master_pointer(display, ids, click.x, click.y)?;
            let count = click.count.max(1);
            for i in 0..count {
                {
                    let mut device = device.lock().unwrap();
                    emit_button(&mut device, click.button, true)?;
                }
                // The shield grab freezes the device on every press; drain and
                // replay this one so it reaches the app (and re-arms for the
                // next press in a multi-click train).
                let mut pending = std::collections::HashSet::from([ids.pointer_id]);
                replay_shielded_presses(display, opcode, &mut pending, Duration::from_millis(1000));
                if !pending.is_empty() {
                    return Err(anyhow!(
                        "shield replay timed out before XI_ButtonPress arrived for '{cursor_id}'"
                    ));
                }
                {
                    let mut device = device.lock().unwrap();
                    emit_button(&mut device, click.button, false)?;
                }
                // Multi-click cadence: keep press→press well under the toolkit
                // double-click threshold (GTK default 250 ms) so count=2 lands
                // as a real double-click, not two singles.
                if count > 1 && i + 1 < count {
                    sleep(Duration::from_millis(CLICK_DELAY_MS));
                }
            }
            Ok(())
        })();
        remove_shield_grab(display, ids.pointer_id, window, click.button);
        click_result
    })();

    forget_master_pointer(cursor_id);
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

    forget_master_pointer(cursor_id);
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
                            let prev_handler =
                                x11::xlib::XSetErrorHandler(Some(ignore_x_error));
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
    let (conn, _) = connect_x11_for_input()?;
    let root = conn.setup().roots[0].root;

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
            state: KeyButMask::from(0u16),
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
            state: button_state_mask(button),
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
    let step_delay_ms = if steps > 1 { duration_ms / steps as u64 } else { duration_ms };
    let press_target = resolve_event_target(&conn, xid, from_x, from_y)?;

    // ButtonPress at start.
    let press = ButtonPressEvent {
        response_type: BUTTON_PRESS_EVENT,
        detail: button,
        sequence: 0,
        time: x11rb::CURRENT_TIME,
        root, event: press_target.window, child: x11rb::NONE,
        root_x: press_target.root_x, root_y: press_target.root_y,
        event_x: press_target.local_x, event_y: press_target.local_y,
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
            root, event: target.window, child: x11rb::NONE,
            root_x: target.root_x, root_y: target.root_y,
            event_x: target.local_x, event_y: target.local_y,
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
        root, event: release_target.window, child: x11rb::NONE,
        root_x: release_target.root_x, root_y: release_target.root_y,
        event_x: release_target.local_x, event_y: release_target.local_y,
        state: button_state_mask(button),
        same_screen: true,
    };
    conn.send_event(false, release_target.window, EventMask::BUTTON_RELEASE, &release)?;
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
        state: button.map(button_state_mask).unwrap_or_else(|| KeyButMask::from(0u16)),
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
        let state = if needs_shift { KeyButMask::SHIFT } else { KeyButMask::from(0u16) };

        let press = KeyPressEvent {
            response_type: KEY_PRESS_EVENT,
            detail: keycode,
            sequence: 0,
            time: x11rb::CURRENT_TIME,
            root, event: window, child: x11rb::NONE,
            root_x: 0, root_y: 0, event_x: 0, event_y: 0,
            state,
            same_screen: true,
        };
        let release = KeyReleaseEvent {
            response_type: KEY_RELEASE_EVENT,
            detail: keycode,
            sequence: 0,
            time: x11rb::CURRENT_TIME,
            root, event: window, child: x11rb::NONE,
            root_x: 0, root_y: 0, event_x: 0, event_y: 0,
            state,
            same_screen: true,
        };

        conn.send_event(false, window, EventMask::KEY_PRESS, &press)?;
        sleep(Duration::from_millis(KEY_DELAY_MS));
        conn.send_event(false, window, EventMask::KEY_RELEASE, &release)?;
        conn.flush()?;
        if inter_char_ms > 0 {
            sleep(Duration::from_millis(inter_char_ms));
        }
    }
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

/// Screen-absolute click via the XTest extension — the `capture_scope="desktop"`
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
    use x11rb::protocol::xtest::ConnectionExt as _;
    let (conn, screen_num) = connect_x11_for_input()?;
    let root = conn.setup().roots[screen_num].root;
    // Absolute pointer warp (MotionNotify, detail=0 => absolute) so the button
    // events that follow are delivered at (x, y).
    conn.xtest_fake_input(MOTION_NOTIFY_EVENT, 0, 0, root, x as i16, y as i16, 0)?;
    for _ in 0..count.max(1) {
        conn.xtest_fake_input(BUTTON_PRESS_EVENT, button, 0, root, x as i16, y as i16, 0)?;
        conn.xtest_fake_input(BUTTON_RELEASE_EVENT, button, 0, root, x as i16, y as i16, 0)?;
    }
    conn.flush()?;
    // Round-trip so the server processes the warp+button events before this
    // short-lived connection drops. Pointer events happened to survive the close
    // under Xtigervnc where keyboard events did not (see send_key_xtest), but make
    // it explicit so the desktop click is reliable across X servers too.
    let _ = conn.get_input_focus()?.reply();
    Ok(())
}

/// Send a named key press to a window.
pub fn send_key(xid: u64, key: &str, modifiers: &[&str]) -> Result<()> {
    let (conn, _) = connect_x11_for_input()?;
    let window = xid as u32;
    let root = conn.setup().roots[0].root;

    // Resolve the named key to a keysym, then to a keycode. On sparse/headless
    // keymaps (e.g. a minimal Xwayland :0) the keysym may have no keycode at all
    // — historically this failed with "Keysym 0x.. not in keyboard map".
    // `keycode_for_keysym` instead borrows a spare keycode and hands back a guard
    // that restores the original mapping once the event has been delivered.
    let keysym = key_name_to_keysym(key)?;
    let mapping = conn.get_keyboard_mapping(8, 248)?.reply()?;
    let (keycode, remap_guard) = keycode_for_keysym(&conn, &mapping, keysym, key)?;
    let state = modifiers_to_state(modifiers);

    let press = KeyPressEvent {
        response_type: KEY_PRESS_EVENT,
        detail: keycode,
        sequence: 0,
        time: x11rb::CURRENT_TIME,
        root,
        event: window,
        child: x11rb::NONE,
        root_x: 0, root_y: 0,
        event_x: 0, event_y: 0,
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
        root_x: 0, root_y: 0,
        event_x: 0, event_y: 0,
        state,
        same_screen: true,
    };

    conn.send_event(false, window, EventMask::KEY_PRESS, &press)?;
    sleep(Duration::from_millis(KEY_DELAY_MS));
    conn.send_event(false, window, EventMask::KEY_RELEASE, &release)?;
    conn.flush()?;

    // If we borrowed a spare keycode for this keysym, give the target client a
    // moment to translate the synthetic event under the temporary mapping before
    // we restore it. The keycode->keysym lookup is client-side, so restoring too
    // eagerly would race delivery. A server round-trip (which only returns once
    // our queued requests have been processed) plus a short settle keeps that
    // race closed; the guard then reinstates the original keysyms on drop.
    if remap_guard.is_some() {
        let _ = conn.get_input_focus()?.reply();
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
        "f1" => 0xFFBE, "f2" => 0xFFBF, "f3" => 0xFFC0, "f4" => 0xFFC1,
        "f5" => 0xFFC2, "f6" => 0xFFC3, "f7" => 0xFFC4, "f8" => 0xFFC5,
        "f9" => 0xFFC6, "f10" => 0xFFC7, "f11" => 0xFFC8, "f12" => 0xFFC9,
        "shift" => 0xFFE1, "ctrl" | "control" => 0xFFE3, "alt" => 0xFFE9, "super" | "meta" | "win" => 0xFFEB,
        "capslock" => 0xFFE5, "numlock" => 0xFF7F,
        // Common X keysym names for punctuation. The single-char branch below
        // already resolves the literal glyph ("+", "=", "*", "/"), but callers
        // that speak the X keysym-name vocabulary may pass the spelled-out name.
        // For the ASCII range the keysym value equals the codepoint.
        "plus" => 0x2B, "minus" | "dash" => 0x2D, "equal" | "equals" => 0x3D,
        "asterisk" | "star" => 0x2A, "slash" => 0x2F, "backslash" => 0x5C,
        "period" | "dot" => 0x2E, "comma" => 0x2C, "semicolon" => 0x3B,
        "colon" => 0x3A, "underscore" => 0x5F, "parenleft" => 0x28, "parenright" => 0x29,
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
    let tcl_text = text.replace("\\", "\\\\").replace("{", "\\{").replace("}", "\\}");

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
        .spawn() {
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
    use super::{path_cumulative, point_on_path, sample_function};

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

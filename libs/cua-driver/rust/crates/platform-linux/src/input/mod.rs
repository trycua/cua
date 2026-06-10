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

use anyhow::{anyhow, bail, Result};
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

#[derive(Clone, Copy, Debug)]
pub struct VirtualPointerDrag {
    pub target_window: u64,
    pub button: u8,
    pub from_x: i32,
    pub from_y: i32,
    pub to_x: i32,
    pub to_y: i32,
    pub duration_ms: u64,
    pub steps: usize,
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
    let display_name = std::env::var("DISPLAY").ok();
    let Ok(proc_entries) = fs::read_dir("/proc") else {
        return false;
    };
    for entry in proc_entries.flatten() {
        let file_name = entry.file_name();
        let Some(pid) = file_name.to_str() else {
            continue;
        };
        if !pid.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(cmdline) = fs::read(entry.path().join("cmdline")) else {
            continue;
        };
        if cmdline.is_empty() {
            continue;
        }
        let cmd = String::from_utf8_lossy(&cmdline).replace('\0', " ");
        if !cmd.contains("Xtigervnc") {
            continue;
        }
        if let Some(display_name) = &display_name {
            if cmd.contains(display_name) {
                return true;
            }
        } else {
            return true;
        }
    }
    false
}

pub fn check_parallel_pointer_support() -> Result<()> {
    let display = open_display()?;
    let result = supports_parallel_pointer_injection(display);
    unsafe { x11::xlib::XCloseDisplay(display) };
    result
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
    rel_axes.insert(RelativeAxisType::REL_WHEEL);

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
        ev.data.set_long(1, 0);
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

pub fn send_parallel_virtual_pointer_drags(
    drags: &[(String, VirtualPointerDrag)],
) -> Result<()> {
    let display = open_display()?;
    supports_parallel_pointer_injection(display)?;

    struct ActiveDrag {
        cursor_id: String,
        ids: MasterPointerIds,
        device: Arc<Mutex<VirtualDevice>>,
        drag: VirtualPointerDrag,
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
            active.push(ActiveDrag {
                cursor_id: cursor_id.clone(),
                ids,
                device,
                drag: *drag,
                steps: drag.steps.max(1),
                step_delay: if drag.steps.max(1) > 1 {
                    Duration::from_millis(drag.duration_ms / drag.steps.max(1) as u64)
                } else {
                    Duration::from_millis(drag.duration_ms)
                },
                current_step: 0,
                next_at: start_at,
                last_x: drag.from_x,
                last_y: drag.from_y,
            });
        }

        let now = std::time::Instant::now();
        if start_at > now {
            std::thread::sleep(start_at - now);
        }

        for item in &active {
            warp_master_pointer(display, item.ids, item.drag.from_x, item.drag.from_y)?;
            let mut device = item.device.lock().unwrap();
            emit_button(&mut device, item.drag.button, true)?;
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
                    let ix = item.drag.from_x
                        + ((item.drag.to_x - item.drag.from_x) as f64 * t).round() as i32;
                    let iy = item.drag.from_y
                        + ((item.drag.to_y - item.drag.from_y) as f64 * t).round() as i32;
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

fn restore_focus_state(display: *mut x11::xlib::Display, saved: &SavedFocus) {
    // Let the release/focus events from the drag settle before reading the
    // post-drag state, so we don't race the WM's own focus update.
    unsafe { x11::xlib::XSync(display, 0) };

    if let Some(prev) = saved.ewmh_active {
        // EWMH path: ask the WM to re-activate, so its active-window
        // bookkeeping (decorations, stacking) stays consistent. The WM
        // applies its own click-to-focus while it works through the drag's
        // release (sync-grab replay), which can land after a one-shot
        // request — give it a moment to settle, then verify and retry.
        sleep(Duration::from_millis(250));
        for attempt in 0..6 {
            let now = ewmh_active_window(display);
            if now == Some(prev) {
                return;
            }
            tracing::debug!("focus restore attempt {attempt}: prev=0x{prev:x} now={now:x?}");
            ewmh_activate_window(display, prev, now.unwrap_or(0));
            sleep(Duration::from_millis(150));
        }
        tracing::warn!("focus restore: WM did not re-activate 0x{prev:x}");
        return;
    }

    // No EWMH WM (bare X / minimal WM): restore the core input focus
    // directly. The saved window may have been destroyed meanwhile, and
    // Xlib's default error handler exits the process on BadWindow, so the
    // restore runs under a scoped ignore-errors handler.
    if saved.core_focus == 0 {
        return;
    }
    unsafe extern "C" fn ignore_x_error(
        _display: *mut x11::xlib::Display,
        _event: *mut x11::xlib::XErrorEvent,
    ) -> std::os::raw::c_int {
        0
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

/// Send a synthetic FocusIn event to a window without changing the actual X11 input focus.
/// This can trigger toolkit-level focus handlers (e.g., Qt5's AT-SPI bridge) without
/// moving the window manager's active window. Use with send_focus_out to restore state.
pub fn send_focus_in(xid: u64) -> Result<()> {
    let (conn, _) = RustConnection::connect(None)?;
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
    let (conn, _) = RustConnection::connect(None)?;
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
    let (conn, _) = RustConnection::connect(None)?;
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
    let (conn, _) = RustConnection::connect(None)?;
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
    let (conn, _) = RustConnection::connect(None)?;
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
    let (conn, _) = RustConnection::connect(None)?;
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
    let (conn, _) = RustConnection::connect(None)?;
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
    let (conn, _) = RustConnection::connect(None)?;
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

/// Send a named key press to a window.
pub fn send_key(xid: u64, key: &str, modifiers: &[&str]) -> Result<()> {
    let (conn, _) = RustConnection::connect(None)?;
    let window = xid as u32;
    let root = conn.setup().roots[0].root;

    let keycode = key_name_to_keycode(&conn, key)?;
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

fn key_name_to_keycode(conn: &RustConnection, key: &str) -> Result<u8> {
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
        s if s.len() == 1 => s.chars().next().unwrap() as u32,
        _ => anyhow::bail!("Unknown key: {key}"),
    };

    let km = conn.get_keyboard_mapping(8, 248)?.reply()?;
    let kpc = km.keysyms_per_keycode as usize;
    for (i, syms) in km.keysyms.chunks(kpc).enumerate() {
        if syms.iter().any(|&s| s == keysym) {
            return Ok((8 + i) as u8);
        }
    }
    anyhow::bail!("Keysym 0x{keysym:X} not in keyboard map for key '{key}'")
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

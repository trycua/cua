//! Focus-free keyboard delivery on X11 via XInput2 MPX: a second master
//! keyboard with its own focus, fed by a uinput slave.
//!
//! X11 has exactly one *core* keyboard focus, which is why XTest (real input,
//! but always to the core focus) cannot type into a background window, and
//! why the toolkits that ignore `send_event` (GTK, LibreOffice VCL, Qt,
//! Chromium) drop the XSendEvent alternative. MPX lifts the single-focus
//! limit: every master keyboard carries its own focus window
//! (`XISetFocus`), and key events from a slave attached to that master are
//! delivered — as real, non-synthetic XI2 *and* core events — to that master's
//! focus, while the user's core keyboard, its focus and the pointer stay
//! untouched. XTEST slaves cannot be re-attached to another master, so the
//! slave is a uinput keyboard hot-added through udev/libinput.
//!
//! Lifecycle mirrors the MPX pointer click: the master pair is created (or
//! reused) for the session, the keyboard slave is attached, the target window
//! is focused for that master only, keys are emitted, delivery is confirmed
//! through the server's raw key events, and the pair is torn down again so a
//! non-MPX-aware WM never sees a lingering foreign master.

use super::*;
use x11rb::protocol::xproto::{ConnectionExt as _, GetKeyboardMappingReply};

const UINPUT_KEYBOARD_SUFFIX: &str = " uinput keyboard";
/// Structured `path` reported for a delivery through the virtual master keyboard.
pub const MPX_UINPUT_PATH: &str = "mpx_uinput";
/// evdev keycodes are X keycodes minus this offset (X reserves 0..=7).
const X_KEYCODE_OFFSET: u16 = 8;
/// Highest evdev key code the uinput keyboard advertises (KEY_* range below
/// the BTN_* block at 0x100, so libinput classifies it as a keyboard only).
const MAX_EVDEV_KEY: u16 = 0xff;
/// Unicode keysyms are the codepoint with this bit set (X11 keysym encoding).
const UNICODE_KEYSYM_BASE: u32 = 0x0100_0000;
/// How long the server gets to report the last emitted key through a raw
/// event before the pair is torn down.
const DELIVERY_CONFIRM_TIMEOUT: Duration = Duration::from_millis(1500);

static UINPUT_KEYBOARDS: OnceLock<Mutex<HashMap<String, Arc<Mutex<VirtualDevice>>>>> =
    OnceLock::new();

fn uinput_keyboards() -> &'static Mutex<HashMap<String, Arc<Mutex<VirtualDevice>>>> {
    UINPUT_KEYBOARDS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) fn forget_uinput_keyboard(cursor_id: &str) {
    uinput_keyboards().lock().unwrap().remove(cursor_id);
}

fn slave_keyboard_name(master_name: &str) -> String {
    format!("{master_name}{UINPUT_KEYBOARD_SUFFIX}")
}

/// The keyboard route needs exactly what the MPX pointer route needs: a real
/// Xorg with udev/libinput hotplug and a writable `/dev/uinput`.
pub fn real_keyboard_input_available() -> bool {
    real_pointer_input_available()
}

/// One evdev key transition for the uinput keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct KeyStep {
    pub evdev_code: u16,
    pub press: bool,
}

impl KeyStep {
    fn x_keycode(self) -> u8 {
        (self.evdev_code + X_KEYCODE_OFFSET) as u8
    }
}

/// X keycode → evdev code. X keycodes below 8 have no evdev counterpart.
pub(super) fn evdev_code_for_x_keycode(keycode: u8) -> Option<u16> {
    let keycode = u16::from(keycode);
    (keycode >= X_KEYCODE_OFFSET && keycode - X_KEYCODE_OFFSET <= MAX_EVDEV_KEY)
        .then(|| keycode - X_KEYCODE_OFFSET)
}

/// Keysym for a typed character: Latin-1 keysyms are the codepoint, everything
/// above is the Unicode keysym; `\n` and `\t` become Return and Tab.
pub(super) fn keysym_for_char(ch: char) -> u32 {
    match ch {
        '\n' | '\r' => 0xff0d,
        '\t' => 0xff09,
        c if (c as u32) < 0x100 => c as u32,
        c => UNICODE_KEYSYM_BASE | c as u32,
    }
}

/// Press/release steps for one key at `keycode`, wrapped in Shift when the
/// keysym lives in the shifted column and Shift is not already held.
fn tap_steps(keycode: u8, needs_shift: bool, shift: Option<u16>, steps: &mut Vec<KeyStep>) {
    let Some(code) = evdev_code_for_x_keycode(keycode) else {
        return;
    };
    let shift = if needs_shift { shift } else { None };
    if let Some(shift) = shift {
        steps.push(KeyStep {
            evdev_code: shift,
            press: true,
        });
    }
    steps.push(KeyStep {
        evdev_code: code,
        press: true,
    });
    steps.push(KeyStep {
        evdev_code: code,
        press: false,
    });
    if let Some(shift) = shift {
        steps.push(KeyStep {
            evdev_code: shift,
            press: false,
        });
    }
}

/// Plan the evdev transitions that type `text` under `mapping`. Characters
/// with no keycode in the map are returned in the second tuple element (the
/// caller may remap a spare keycode for them or report them as skipped).
pub(super) fn plan_text(
    mapping: &GetKeyboardMappingReply,
    shift_x_keycode: Option<u8>,
    text: &str,
) -> (Vec<KeyStep>, Vec<char>) {
    let shift = shift_x_keycode.and_then(evdev_code_for_x_keycode);
    let mut steps = Vec::with_capacity(text.len() * 2);
    let mut missing = Vec::new();
    for ch in text.chars() {
        match char_to_keycode_shift(mapping, keysym_for_char(ch)) {
            Some((keycode, needs_shift)) => tap_steps(keycode, needs_shift, shift, &mut steps),
            None => missing.push(ch),
        }
    }
    (steps, missing)
}

/// Plan a chord: modifiers pressed in order, the key tapped (with Shift when
/// its keysym is shifted and Shift was not requested), modifiers released in
/// reverse order — the same ordering physical input produces.
pub(super) fn plan_chord(
    modifier_x_keycodes: &[u8],
    key_x_keycode: u8,
    key_needs_shift: bool,
    shift_x_keycode: Option<u8>,
) -> Vec<KeyStep> {
    let modifiers: Vec<u16> = modifier_x_keycodes
        .iter()
        .filter_map(|&keycode| evdev_code_for_x_keycode(keycode))
        .collect();
    let shift = shift_x_keycode.and_then(evdev_code_for_x_keycode);
    let shift_requested = shift.is_some_and(|shift| modifiers.contains(&shift));
    let mut steps = Vec::with_capacity(modifiers.len() * 2 + 4);
    for &code in &modifiers {
        steps.push(KeyStep {
            evdev_code: code,
            press: true,
        });
    }
    tap_steps(
        key_x_keycode,
        key_needs_shift && !shift_requested,
        shift,
        &mut steps,
    );
    for &code in modifiers.iter().rev() {
        steps.push(KeyStep {
            evdev_code: code,
            press: false,
        });
    }
    steps
}

/// What the virtual-keyboard route could observe about its own delivery.
/// Nothing here reads the application's state back (that stays the caller's
/// screenshot/AT-SPI job); it only says the events went where they were aimed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyboardDeliveryReport {
    /// The target window still owned the virtual master keyboard's focus
    /// after the last key event.
    pub virtual_focus_held: bool,
    /// `XGetInputFocus` (the user's core focus) was identical before and after.
    pub core_focus_unchanged: bool,
    /// The server reported the last emitted key transition as a raw event.
    pub delivery_confirmed: bool,
    /// Key transitions emitted through the uinput slave.
    pub key_events: usize,
    /// Characters that have no keycode in the current keymap and could not be
    /// hosted on a spare keycode either.
    pub skipped_characters: Vec<char>,
}

impl KeyboardDeliveryReport {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "path": MPX_UINPUT_PATH,
            "virtual_focus_held": self.virtual_focus_held,
            "core_focus_unchanged": self.core_focus_unchanged,
            "delivery_confirmed": self.delivery_confirmed,
            "key_events": self.key_events,
            "skipped_characters": self.skipped_characters.iter().collect::<String>(),
        })
    }
}

fn create_uinput_keyboard(name: &str) -> Result<VirtualDevice> {
    guarded_uinput_creation(name, |name| {
        let mut keys = AttributeSet::<Key>::new();
        // Every KEY_* code the X keycode range can address, so any keycode the
        // server's keymap resolves to can be emitted, including the modifiers.
        for code in 1..=MAX_EVDEV_KEY {
            keys.insert(Key::new(code));
        }
        Ok(evdev::uinput::VirtualDeviceBuilder::new()?
            .name(name)
            .with_keys(&keys)?
            .build()?)
    })
}

/// Make sure the session's master pair carries an attached uinput keyboard
/// slave; creates the pair (pointer + keyboard master, uinput pointer) through
/// `ensure_master_pointer` first when needed.
fn ensure_master_keyboard(cursor_id: &str) -> Result<(MasterPointerIds, Arc<Mutex<VirtualDevice>>)> {
    let ids = ensure_master_pointer(cursor_id)?;
    if let (Some(_), Some(device)) = (
        ids.slave_keyboard_id,
        uinput_keyboards().lock().unwrap().get(cursor_id).cloned(),
    ) {
        return Ok((ids, device));
    }

    let display = open_display()?;
    let result = (|| -> Result<(MasterPointerIds, Arc<Mutex<VirtualDevice>>)> {
        // The master keyboard's device name is "<base> keyboard"; recover the
        // base to derive the slave's name so the reaper can pair them by eye.
        let base = xi2_query_devices(display)?
            .into_iter()
            .find(|(id, _, _)| *id == ids.keyboard_id)
            .and_then(|(_, _, name)| name.strip_suffix(" keyboard").map(str::to_owned))
            .ok_or_else(|| anyhow!("master keyboard {} vanished", ids.keyboard_id))?;
        let device_name = slave_keyboard_name(&base);
        let device = create_uinput_keyboard(&device_name)?;
        let slave_keyboard_id = wait_for_slave_id(
            display,
            &device_name,
            x11::xinput2::XISlaveKeyboard,
            Duration::from_secs(5),
        )?;
        attach_slave_to_master(display, slave_keyboard_id, ids.keyboard_id)?;
        let ids = MasterPointerIds {
            slave_keyboard_id: Some(slave_keyboard_id),
            ..ids
        };
        mpx_pointers()
            .lock()
            .unwrap()
            .insert(cursor_id.to_owned(), ids);
        let device = Arc::new(Mutex::new(device));
        uinput_keyboards()
            .lock()
            .unwrap()
            .insert(cursor_id.to_owned(), device.clone());
        Ok((ids, device))
    })();
    unsafe { x11::xlib::XCloseDisplay(display) };
    result
}

/// Shift keycode from the server's modifier map (modifier index 0).
fn shift_keycode(conn: &RustConnection) -> Option<u8> {
    let modmap = conn.get_modifier_mapping().ok()?.reply().ok()?;
    let per = modmap.keycodes_per_modifier() as usize;
    modmap
        .keycodes
        .get(..per)
        .and_then(|shift| shift.iter().copied().find(|&keycode| keycode != 0))
}

fn xi_get_focus(display: *mut x11::xlib::Display, device_id: i32) -> Option<x11::xlib::Window> {
    let mut focus: x11::xlib::Window = 0;
    let rc = unsafe { x11::xinput2::XIGetFocus(display, device_id, &mut focus) };
    (rc == 0).then_some(focus)
}

fn core_focus(display: *mut x11::xlib::Display) -> x11::xlib::Window {
    let mut focus: x11::xlib::Window = 0;
    let mut revert_to = 0;
    unsafe { x11::xlib::XGetInputFocus(display, &mut focus, &mut revert_to) };
    focus
}

/// Select raw key events on the root window for `device_id` so the server's
/// processing of our own key transitions can be observed on `display`.
fn select_raw_key_events(display: *mut x11::xlib::Display, device_id: i32) -> Result<()> {
    let root = unsafe { x11::xlib::XDefaultRootWindow(display) };
    let mut mask_bits = vec![0u8; xi_mask_len()];
    x11::xinput2::XISetMask(&mut mask_bits, x11::xinput2::XI_RawKeyPress);
    x11::xinput2::XISetMask(&mut mask_bits, x11::xinput2::XI_RawKeyRelease);
    let mut evmask = x11::xinput2::XIEventMask {
        deviceid: device_id,
        mask_len: mask_bits.len() as std::os::raw::c_int,
        mask: mask_bits.as_mut_ptr(),
    };
    let rc = unsafe { x11::xinput2::XISelectEvents(display, root, &mut evmask, 1) };
    unsafe { x11::xlib::XSync(display, 0) };
    if rc != 0 {
        bail!("XISelectEvents(raw keys) failed with status {rc}");
    }
    Ok(())
}

/// Block (bounded) until the server reports `last` as a raw key event from
/// one of `device_ids`. Returns false on timeout.
fn wait_for_raw_key(
    display: *mut x11::xlib::Display,
    xi_opcode: std::os::raw::c_int,
    device_ids: &[i32],
    last: KeyStep,
    timeout: Duration,
) -> bool {
    let want_type = if last.press {
        x11::xinput2::XI_RawKeyPress
    } else {
        x11::xinput2::XI_RawKeyRelease
    };
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
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
        if cookie.extension != xi_opcode
            || !matches!(
                cookie.evtype,
                x11::xinput2::XI_RawKeyPress | x11::xinput2::XI_RawKeyRelease
            )
        {
            continue;
        }
        if unsafe { x11::xlib::XGetEventData(display, &mut cookie) } == 0 {
            continue;
        }
        let raw = cookie.data as *const x11::xinput2::XIRawEvent;
        let matched = !raw.is_null() && {
            let raw = unsafe { &*raw };
            cookie.evtype == want_type
                && raw.detail == i32::from(last.x_keycode())
                && (device_ids.contains(&raw.deviceid) || device_ids.contains(&raw.sourceid))
        };
        unsafe { x11::xlib::XFreeEventData(display, &mut cookie) };
        if matched {
            return true;
        }
    }
    false
}

/// Everything the planners need from the server's keymap, resolved once per
/// call on a short-lived x11rb connection (the same helpers the XSendEvent and
/// XTest routes use, so keycode choice is identical across routes).
struct Keymap {
    conn: RustConnection,
    mapping: GetKeyboardMappingReply,
    shift: Option<u8>,
}

impl Keymap {
    fn load() -> Result<Self> {
        let (conn, _) = connect_x11_for_input()?;
        let mapping = conn.get_keyboard_mapping(8, 248)?.reply()?;
        let shift = shift_keycode(&conn);
        Ok(Self {
            conn,
            mapping,
            shift,
        })
    }
}

/// Deliver `steps` to `target_window` through the session's virtual master
/// keyboard. `remap_guards` keeps any spare-keycode remaps alive until the
/// server has processed the events.
fn deliver(
    cursor_id: &str,
    target_window: u64,
    steps: &[KeyStep],
    skipped_characters: Vec<char>,
    remap_guards: Vec<RemappedKeycode<'_>>,
    keymap_conn: &RustConnection,
) -> Result<KeyboardDeliveryReport> {
    let display = open_display()?;
    let result = (|| -> Result<KeyboardDeliveryReport> {
        supports_parallel_pointer_injection(display)?;
        let mut major = 2;
        let mut minor = 3;
        let rc = unsafe { x11::xinput2::XIQueryVersion(display, &mut major, &mut minor) };
        if rc != 0 {
            bail!("XIQueryVersion failed with status {rc}");
        }
        let xi_opcode = xinput_opcode(display)
            .ok_or_else(|| anyhow!("virtual keyboard delivery requires XInput2"))?;
        let (ids, device) = ensure_master_keyboard(cursor_id)?;
        let slave_keyboard_id = ids
            .slave_keyboard_id
            .ok_or_else(|| anyhow!("uinput keyboard for '{cursor_id}' is not attached"))?;

        let window = target_window as x11::xlib::Window;
        let core_before = core_focus(display);
        select_raw_key_events(display, slave_keyboard_id)?;
        // A BadWindow/BadMatch from XISetFocus (target unmapped meanwhile) must
        // surface as an error, not take the daemon down through Xlib's default
        // handler.
        let previous_handler = unsafe { x11::xlib::XSetErrorHandler(Some(ignore_x_error)) };
        let rc = unsafe {
            x11::xinput2::XISetFocus(display, ids.keyboard_id, window, x11::xlib::CurrentTime)
        };
        unsafe { x11::xlib::XSync(display, 0) };
        unsafe { x11::xlib::XSetErrorHandler(previous_handler) };
        if rc != 0 {
            bail!("XISetFocus(virtual master keyboard) failed with status {rc}");
        }
        if xi_get_focus(display, ids.keyboard_id) != Some(window) {
            bail!(
                "virtual master keyboard did not take focus on window 0x{target_window:x} \
                 (is it mapped?)"
            );
        }

        let mut last = None;
        for step in steps {
            {
                let mut device = device.lock().unwrap();
                device.emit(&[InputEvent::new(
                    EventType::KEY,
                    step.evdev_code,
                    if step.press { 1 } else { 0 },
                )])?;
            }
            last = Some(*step);
            sleep(Duration::from_millis(KEY_DELAY_MS));
        }
        let delivery_confirmed = match last {
            Some(last) => wait_for_raw_key(
                display,
                xi_opcode,
                &[slave_keyboard_id, ids.keyboard_id],
                last,
                DELIVERY_CONFIRM_TIMEOUT,
            ),
            None => true,
        };
        // The raw event proves the server consumed the transition; give the
        // toolkit a beat to translate it under any temporary remap before the
        // guards restore the keymap and the slave disappears.
        sleep(Duration::from_millis(30));
        let _ = keymap_conn.get_input_focus()?.reply();
        let virtual_focus_held = xi_get_focus(display, ids.keyboard_id) == Some(window);
        let core_focus_unchanged = core_focus(display) == core_before;
        if !delivery_confirmed {
            tracing::warn!(
                cursor_id,
                "virtual keyboard: server did not report the last key event within {:?}",
                DELIVERY_CONFIRM_TIMEOUT
            );
        }
        Ok(KeyboardDeliveryReport {
            virtual_focus_held,
            core_focus_unchanged,
            delivery_confirmed,
            key_events: steps.len(),
            skipped_characters,
        })
    })();
    drop(remap_guards);
    // Tear the pair down per call, exactly like the pointer click: a lingering
    // foreign master keyboard desyncs non-MPX-aware WMs' focus bookkeeping,
    // and the next call recreates it in well under a second.
    forget_master_pointer(cursor_id);
    unsafe { x11::xlib::XCloseDisplay(display) };
    result
}

/// Type `text` into `target_window` without touching the core focus or the
/// user's pointer. See the module docs for the mechanism.
pub fn send_virtual_keyboard_text(
    cursor_id: &str,
    target_window: u64,
    text: &str,
) -> Result<KeyboardDeliveryReport> {
    let keymap = Keymap::load()?;
    let (mut steps, missing) = plan_text(&keymap.mapping, keymap.shift, text);
    // Characters absent from the keymap (a sparse layout, or a non-Latin glyph)
    // are hosted on spare keycodes, xdotool-style; the guards restore the map
    // after delivery. They are typed after the in-map text — this fallback is
    // for the odd stray glyph, not for ordering-sensitive prose.
    let mut guards = Vec::new();
    let mut skipped = Vec::new();
    let shift = keymap.shift.and_then(evdev_code_for_x_keycode);
    for ch in missing {
        match remap_spare_keycode(&keymap.conn, &keymap.mapping, keysym_for_char(ch)) {
            Ok(guard) => {
                tap_steps(guard.keycode, false, shift, &mut steps);
                guards.push(guard);
            }
            Err(_) => skipped.push(ch),
        }
    }
    deliver(cursor_id, target_window, &steps, skipped, guards, &keymap.conn)
}

/// Press `key` with `modifiers` held, into `target_window`, without touching
/// the core focus or the user's pointer.
pub fn send_virtual_keyboard_key(
    cursor_id: &str,
    target_window: u64,
    key: &str,
    modifiers: &[&str],
) -> Result<KeyboardDeliveryReport> {
    let keymap = Keymap::load()?;
    let mut guards = Vec::new();
    let mut modifier_keycodes = Vec::with_capacity(modifiers.len());
    for modifier in modifiers {
        let keysym = key_name_to_keysym(modifier)?;
        let (keycode, guard) =
            keycode_for_keysym(&keymap.conn, &keymap.mapping, keysym, modifier)?;
        guards.extend(guard);
        modifier_keycodes.push(keycode);
    }
    let keysym = key_name_to_keysym(key)?;
    let (keycode, needs_shift) = match char_to_keycode_shift(&keymap.mapping, keysym) {
        Some(found) => found,
        None => {
            let (keycode, guard) =
                keycode_for_keysym(&keymap.conn, &keymap.mapping, keysym, key)?;
            guards.extend(guard);
            (keycode, false)
        }
    };
    let steps = plan_chord(&modifier_keycodes, keycode, needs_shift, keymap.shift);
    deliver(cursor_id, target_window, &steps, Vec::new(), guards, &keymap.conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny US-like keymap: keycode 38 = a/A, 10 = 1/!, 36 = Return,
    /// 50 = Shift_L, 37 = Control_L, 64 = Alt_L, 250 = spare (NoSymbol).
    fn mapping() -> GetKeyboardMappingReply {
        let per = 2usize;
        let mut keysyms = vec![0u32; (248 - 8 + 1) * per];
        let mut set = |keycode: usize, syms: [u32; 2]| {
            let idx = (keycode - 8) * per;
            keysyms[idx..idx + per].copy_from_slice(&syms);
        };
        set(38, [0x61, 0x41]);
        set(10, [0x31, 0x21]);
        set(36, [0xff0d, 0xff0d]);
        set(50, [0xffe1, 0xffe1]);
        set(37, [0xffe3, 0xffe3]);
        set(64, [0xffe9, 0xffe9]);
        GetKeyboardMappingReply {
            keysyms_per_keycode: per as u8,
            sequence: 0,
            keysyms,
        }
    }

    fn step(evdev_code: u16, press: bool) -> KeyStep {
        KeyStep { evdev_code, press }
    }

    #[test]
    fn x_keycodes_map_to_evdev_by_subtracting_eight() {
        assert_eq!(evdev_code_for_x_keycode(38), Some(30)); // KEY_A
        assert_eq!(evdev_code_for_x_keycode(50), Some(42)); // KEY_LEFTSHIFT
        assert_eq!(evdev_code_for_x_keycode(37), Some(29)); // KEY_LEFTCTRL
        assert_eq!(evdev_code_for_x_keycode(8), Some(0));
        assert_eq!(evdev_code_for_x_keycode(7), None);
        assert_eq!(evdev_code_for_x_keycode(0), None);
        assert_eq!(step(30, true).x_keycode(), 38);
    }

    #[test]
    fn characters_resolve_to_latin1_unicode_or_named_keysyms() {
        assert_eq!(keysym_for_char('a'), 0x61);
        assert_eq!(keysym_for_char('é'), 0xe9);
        assert_eq!(keysym_for_char('€'), 0x0100_0000 | 0x20ac);
        assert_eq!(keysym_for_char('\n'), 0xff0d);
        assert_eq!(keysym_for_char('\t'), 0xff09);
    }

    #[test]
    fn text_plan_taps_keys_and_wraps_shifted_glyphs_in_shift() {
        let (steps, missing) = plan_text(&mapping(), Some(50), "a!\n");
        assert!(missing.is_empty());
        assert_eq!(
            steps,
            vec![
                step(30, true),
                step(30, false),
                step(42, true),
                step(2, true),
                step(2, false),
                step(42, false),
                step(28, true),
                step(28, false),
            ]
        );
    }

    #[test]
    fn text_plan_reports_characters_the_keymap_lacks() {
        let (steps, missing) = plan_text(&mapping(), Some(50), "aé€");
        assert_eq!(steps.len(), 2);
        assert_eq!(missing, vec!['é', '€']);
    }

    #[test]
    fn text_plan_without_a_shift_keycode_types_the_unshifted_level() {
        // No Shift in the modifier map: the keycode is still tapped (the
        // server types the base glyph) rather than the character being lost.
        let (steps, _) = plan_text(&mapping(), None, "A");
        assert_eq!(steps, vec![step(30, true), step(30, false)]);
    }

    #[test]
    fn chord_plan_nests_modifiers_around_the_key_in_physical_order() {
        // ctrl+alt+a
        assert_eq!(
            plan_chord(&[37, 64], 38, false, Some(50)),
            vec![
                step(29, true),
                step(56, true),
                step(30, true),
                step(30, false),
                step(56, false),
                step(29, false),
            ]
        );
    }

    #[test]
    fn chord_plan_adds_shift_only_when_the_key_is_shifted_and_shift_not_held() {
        // ctrl+A where 'A' is the shifted level of keycode 38: auto-Shift.
        assert_eq!(
            plan_chord(&[37], 38, true, Some(50)),
            vec![
                step(29, true),
                step(42, true),
                step(30, true),
                step(30, false),
                step(42, false),
                step(29, false),
            ]
        );
        // ctrl+shift+A: Shift already requested, never doubled.
        assert_eq!(
            plan_chord(&[37, 50], 38, true, Some(50)),
            vec![
                step(29, true),
                step(42, true),
                step(30, true),
                step(30, false),
                step(42, false),
                step(29, false),
            ]
        );
    }

    #[test]
    fn report_json_names_the_route() {
        let report = KeyboardDeliveryReport {
            virtual_focus_held: true,
            core_focus_unchanged: true,
            delivery_confirmed: false,
            key_events: 4,
            skipped_characters: vec!['€'],
        };
        let json = report.to_json();
        assert_eq!(json["path"], MPX_UINPUT_PATH);
        assert_eq!(json["key_events"], 4);
        assert_eq!(json["delivery_confirmed"], false);
        assert_eq!(json["skipped_characters"], "€");
    }

    #[test]
    fn slave_keyboard_name_fits_the_uinput_limit_for_versioned_masters() {
        let owner = mpx_owner::Owner::current().expect("procfs identity");
        let name = slave_keyboard_name(&owner.master_name(u64::MAX));
        assert!(name.len() <= EVDEV_UINPUT_NAME_MAX_BYTES, "{name}");
        assert!(name.ends_with(UINPUT_KEYBOARD_SUFFIX));
    }
}

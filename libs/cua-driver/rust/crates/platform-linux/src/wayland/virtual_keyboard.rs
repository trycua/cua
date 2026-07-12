//! Stable physical-key injection for wlroots virtual-keyboard compositors.

use std::ffi::CString;
use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::os::fd::{AsFd, FromRawFd};

use wayland_client::protocol::{wl_registry, wl_seat::WlSeat};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};

use self::protocol::zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1;
use self::protocol::zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1;

mod protocol {
    use wayland_client;
    use wayland_client::protocol::*;

    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("protocol/virtual-keyboard-unstable-v1.xml");
    }
    use self::__interfaces::*;
    wayland_scanner::generate_client_code!("protocol/virtual-keyboard-unstable-v1.xml");
}

const XKB_KEYMAP: &str = r#"xkb_keymap {
 xkb_keycodes { include "evdev+aliases(qwerty)" };
 xkb_types { include "complete" };
 xkb_compatibility { include "complete" };
 xkb_symbols { include "pc+us+inet(evdev)" };
 xkb_geometry { include "pc(pc105)" };
};
"#;

const KEY_RELEASED: u32 = 0;
const KEY_PRESSED: u32 = 1;

#[derive(Default)]
struct State {
    seat: Option<WlSeat>,
    manager: Option<ZwpVirtualKeyboardManagerV1>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            if interface == WlSeat::interface().name {
                state.seat = Some(registry.bind(name, version.min(7), qh, ()));
            } else if interface == ZwpVirtualKeyboardManagerV1::interface().name {
                state.manager = Some(registry.bind(name, version.min(1), qh, ()));
            }
        }
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(
        _: &mut Self,
        _: &WlSeat,
        _: <WlSeat as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpVirtualKeyboardManagerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwpVirtualKeyboardManagerV1,
        _: <ZwpVirtualKeyboardManagerV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpVirtualKeyboardV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwpVirtualKeyboardV1,
        _: <ZwpVirtualKeyboardV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

pub(super) fn hotkey(modifiers: &[String], key: &str) -> anyhow::Result<()> {
    let transitions = hotkey_transitions(modifiers, key)?;
    let conn = Connection::connect_to_env()?;
    let mut queue = conn.new_event_queue::<State>();
    let qh = queue.handle();
    conn.display().get_registry(&qh, ());
    let mut state = State::default();
    queue.roundtrip(&mut state)?;
    queue.roundtrip(&mut state)?;

    let seat = state
        .seat
        .clone()
        .ok_or_else(|| anyhow::anyhow!("compositor exposes no wl_seat"))?;
    let manager = state.manager.clone().ok_or_else(|| {
        anyhow::anyhow!("compositor exposes no zwp_virtual_keyboard_manager_v1")
    })?;
    let keyboard = manager.create_virtual_keyboard(&seat, &qh, ());
    let keymap = keymap_file()?;
    keyboard.keymap(1, keymap.as_fd(), XKB_KEYMAP.len() as u32 + 1);
    queue.roundtrip(&mut state)?;

    // A harmless Shift tap absorbs the first-event loss seen on fresh
    // headless wlroots seats without changing the requested chord.
    send_key(&keyboard, &mut queue, &mut state, 42, KEY_PRESSED)?;
    send_key(&keyboard, &mut queue, &mut state, 42, KEY_RELEASED)?;
    for transition in transitions {
        send_key(
            &keyboard,
            &mut queue,
            &mut state,
            transition.keycode,
            if transition.pressed {
                KEY_PRESSED
            } else {
                KEY_RELEASED
            },
        )?;
        keyboard.modifiers(transition.modifier_mask, 0, 0, 0);
        queue.roundtrip(&mut state)?;
    }
    keyboard.destroy();
    queue.roundtrip(&mut state)?;
    Ok(())
}

fn send_key(
    keyboard: &ZwpVirtualKeyboardV1,
    queue: &mut wayland_client::EventQueue<State>,
    state: &mut State,
    keycode: u32,
    key_state: u32,
) -> anyhow::Result<()> {
    keyboard.key(super::event_time_ms(), keycode, key_state);
    queue.roundtrip(state)?;
    std::thread::sleep(std::time::Duration::from_millis(4));
    Ok(())
}

fn keymap_file() -> anyhow::Result<File> {
    let name = CString::new("cua-driver-keymap")?;
    let fd = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut file = unsafe { File::from_raw_fd(fd) };
    file.write_all(XKB_KEYMAP.as_bytes())?;
    file.write_all(&[0])?;
    file.seek(SeekFrom::Start(0))?;
    Ok(file)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KeyTransition {
    keycode: u32,
    pressed: bool,
    modifier_mask: u32,
}

fn hotkey_transitions(modifiers: &[String], key: &str) -> anyhow::Result<Vec<KeyTransition>> {
    let mut modifier_mask = 0;
    let mut modifier_keys = Vec::with_capacity(modifiers.len());
    for modifier in modifiers {
        let (keycode, mask) = match modifier.as_str() {
            "ctrl" => (29, 4),
            "shift" => (42, 1),
            "alt" => (56, 8),
            "logo" => (125, 64),
            other => anyhow::bail!("unsupported Wayland modifier '{other}'"),
        };
        modifier_keys.push((keycode, mask));
    }
    let keycode = super::key_to_evdev(key)
        .ok_or_else(|| anyhow::anyhow!("no evdev keycode mapping for key '{key}'"))?;
    let mut transitions = Vec::with_capacity(modifier_keys.len() * 2 + 2);
    for (modifier_key, mask) in &modifier_keys {
        modifier_mask |= mask;
        transitions.push(KeyTransition {
            keycode: *modifier_key,
            pressed: true,
            modifier_mask,
        });
    }
    transitions.push(KeyTransition {
        keycode,
        pressed: true,
        modifier_mask,
    });
    transitions.push(KeyTransition {
        keycode,
        pressed: false,
        modifier_mask,
    });
    for (modifier_key, mask) in modifier_keys.iter().rev() {
        modifier_mask &= !mask;
        transitions.push(KeyTransition {
            keycode: *modifier_key,
            pressed: false,
            modifier_mask,
        });
    }
    Ok(transitions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chord_uses_physical_digit7_and_balanced_modifiers() {
        assert_eq!(
            hotkey_transitions(&["ctrl".into(), "shift".into()], "7").unwrap(),
            [
                KeyTransition { keycode: 29, pressed: true, modifier_mask: 4 },
                KeyTransition { keycode: 42, pressed: true, modifier_mask: 5 },
                KeyTransition { keycode: 8, pressed: true, modifier_mask: 5 },
                KeyTransition { keycode: 8, pressed: false, modifier_mask: 5 },
                KeyTransition { keycode: 42, pressed: false, modifier_mask: 4 },
                KeyTransition { keycode: 29, pressed: false, modifier_mask: 0 },
            ]
        );
    }
}

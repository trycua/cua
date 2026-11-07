// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Caps Lock, Num Lock and the modifier keys as KWin knows them, through
//! `org_kde_kwin_keystate`. KWin before 6.8 tells a libei client nothing about the
//! keyboard's state, and an EIS key goes through that state. Version 5 (KWin 6.3)
//! is the first to say whether a modifier key is held. KWin 6.3 to 6.5 offers this global only to
//! a program whose desktop entry lists it in `X-KDE-Wayland-Interfaces`; KWin 6.6
//! offers it to any client.

use std::time::{Duration, Instant};

use wayland_client::protocol::{wl_callback::WlCallback, wl_registry};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};

use self::protocol::org_kde_kwin_keystate::{Event, Key, OrgKdeKwinKeystate, State as KeyState};
use super::Answered;

mod protocol {
    use wayland_client;

    pub mod __interfaces {
        wayland_scanner::generate_interfaces!("protocol/keystate.xml");
    }
    use self::__interfaces::*;
    wayland_scanner::generate_client_code!("protocol/keystate.xml");
}

/// The key states that change what a typing key produces.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct LockKeys {
    pub caps: bool,
    pub num: bool,
    /// A modifier key, Caps Lock or Num Lock included, is held or latched.
    pub modifier_held: bool,
}

#[derive(Default)]
struct State {
    keystate: Option<OrgKdeKwinKeystate>,
    locks: LockKeys,
}

/// The key states, or `None` when this compositor doesn't offer them to this
/// program, offers a version that doesn't report modifier keys, or doesn't answer
/// in time.
pub(super) fn lock_keys() -> Option<LockKeys> {
    const ANSWER_WITHIN: Duration = Duration::from_millis(250);
    let deadline = Instant::now() + ANSWER_WITHIN;
    let connection = Connection::connect_to_env().ok()?;
    let mut queue = connection.new_event_queue();
    connection.display().get_registry(&queue.handle(), ());
    let mut state = State::default();
    super::bounded_roundtrip(&connection, &mut queue, &mut state, deadline).ok()?;
    let keystate = state.keystate.clone()?;
    if keystate.version() < MODIFIERS_SINCE {
        return None;
    }
    keystate.fetchStates();
    super::bounded_roundtrip(&connection, &mut queue, &mut state, deadline).ok()?;
    Some(state.locks)
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
            if interface == OrgKdeKwinKeystate::interface().name {
                state.keystate = Some(registry.bind(name, version.min(MODIFIERS_SINCE), qh, ()));
            }
        }
    }
}

impl Dispatch<OrgKdeKwinKeystate, ()> for State {
    fn event(
        state: &mut Self,
        _: &OrgKdeKwinKeystate,
        event: Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Event::StateChanged {
            key,
            state: key_state,
        } = event;
        state.locks.apply(key, key_state);
    }
}

impl LockKeys {
    /// Take in one `stateChanged`. A lock key is on only when locked. Any other
    /// state but unlocked, on any key, changes what a key press types.
    fn apply(&mut self, key: u32, state: u32) {
        let locked = state == u32::from(KeyState::Locked);
        let lock_key = if key == u32::from(Key::Capslock) {
            self.caps = locked;
            true
        } else if key == u32::from(Key::Numlock) {
            self.num = locked;
            true
        } else {
            key == u32::from(Key::Scrolllock)
        };
        let idle = state == u32::from(KeyState::Unlocked) || (lock_key && locked);
        self.modifier_held |= !idle;
    }
}

/// The version that reports Shift, Ctrl, Alt, Meta and AltGr, and held keys.
const MODIFIERS_SINCE: u32 = 5;

wayland_client::delegate_dispatch!(State: [WlCallback: Answered] => Answered);

#[cfg(test)]
mod tests {
    use super::*;

    /// One `fetchStates` answer as KWin 6.3 sends it, in its order.
    fn fetched(states: [(Key, KeyState); 8]) -> LockKeys {
        let mut locks = LockKeys::default();
        for (key, state) in states {
            locks.apply(key.into(), state.into());
        }
        locks
    }

    fn answer(caps: KeyState, shift: KeyState) -> [(Key, KeyState); 8] {
        use KeyState::Unlocked;
        [
            (Key::Scrolllock, Unlocked),
            (Key::Alt, Unlocked),
            (Key::Shift, shift),
            (Key::Control, Unlocked),
            (Key::Meta, Unlocked),
            (Key::Altgr, Unlocked),
            (Key::Capslock, caps),
            (Key::Numlock, KeyState::Locked),
        ]
    }

    #[test]
    fn locked_caps_and_num_are_on_and_nothing_is_held() {
        assert_eq!(
            fetched(answer(KeyState::Locked, KeyState::Unlocked)),
            LockKeys {
                caps: true,
                num: true,
                modifier_held: false,
            }
        );
    }

    #[test]
    fn a_pressed_latched_or_locked_modifier_counts_as_held() {
        for shift in [KeyState::Pressed, KeyState::Latched, KeyState::Locked] {
            let locks = fetched(answer(KeyState::Unlocked, shift));
            assert!(locks.modifier_held, "{shift:?}");
            assert!(!locks.caps);
        }
        let pressed_caps = fetched(answer(KeyState::Pressed, KeyState::Unlocked));
        assert!(pressed_caps.modifier_held);
        assert!(!pressed_caps.caps);
    }
}

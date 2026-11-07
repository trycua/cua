// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Caps Lock and Num Lock as KWin knows them, through `org_kde_kwin_keystate`.
//! KWin before 6.8 tells a libei client nothing about the keyboard's state, and
//! an EIS key goes through that state. This global is privileged: KWin offers it
//! only to a program whose desktop entry lists it in `X-KDE-Wayland-Interfaces`.

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

/// The lock keys that change what a typing key produces.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct LockKeys {
    pub caps: bool,
    pub num: bool,
}

#[derive(Default)]
struct State {
    keystate: Option<OrgKdeKwinKeystate>,
    locks: LockKeys,
}

/// The lock keys that are on, or `None` when this compositor doesn't offer them
/// to this program, or doesn't answer in time.
pub(super) fn lock_keys() -> Option<LockKeys> {
    const ANSWER_WITHIN: Duration = Duration::from_millis(250);
    let deadline = Instant::now() + ANSWER_WITHIN;
    let connection = Connection::connect_to_env().ok()?;
    let mut queue = connection.new_event_queue();
    connection.display().get_registry(&queue.handle(), ());
    let mut state = State::default();
    super::bounded_roundtrip(&connection, &mut queue, &mut state, deadline).ok()?;
    let keystate = state.keystate.clone()?;
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
            name, interface, ..
        } = event
        {
            if interface == OrgKdeKwinKeystate::interface().name {
                state.keystate = Some(registry.bind(name, 1, qh, ()));
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
        let Event::StateChanged { key, state: lock } = event;
        let on = u32::from(KeyState::Locked) == lock;
        if key == u32::from(Key::Capslock) {
            state.locks.caps = on;
        } else if key == u32::from(Key::Numlock) {
            state.locks.num = on;
        }
    }
}

wayland_client::delegate_dispatch!(State: [WlCallback: Answered] => Answered);

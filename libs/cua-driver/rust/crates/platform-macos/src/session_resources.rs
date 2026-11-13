// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

use std::collections::HashMap;
use std::sync::{Mutex, Once};

use crate::virtual_display;

static OWNED: Mutex<Option<HashMap<String, Vec<u32>>>> = Mutex::new(None);
static INSTALL: Once = Once::new();

fn install() {
    INSTALL.call_once(|| {
        cua_driver_core::session::register_session_end_hook(release);
    });
}

pub fn own_display(session: &str, display_id: u32) {
    install();
    let mut guard = OWNED.lock().unwrap_or_else(|e| e.into_inner());
    let displays = guard
        .get_or_insert_with(HashMap::new)
        .entry(session.to_owned())
        .or_default();
    if !displays.contains(&display_id) {
        displays.push(display_id);
    }
}

pub fn forget_display(display_id: u32) {
    let mut guard = OWNED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(map) = guard.as_mut() {
        for displays in map.values_mut() {
            displays.retain(|id| *id != display_id);
        }
    }
}

fn release(session: &str) {
    let displays = {
        let mut guard = OWNED.lock().unwrap_or_else(|e| e.into_inner());
        guard.as_mut().and_then(|map| map.remove(session))
    };
    for id in displays.unwrap_or_default() {
        let _ = virtual_display::remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forgetting_a_display_drops_it_from_every_session() {
        own_display("session-a", 9001);
        own_display("session-b", 9001);
        forget_display(9001);
        let guard = OWNED.lock().unwrap();
        let map = guard.as_ref().unwrap();
        assert!(map["session-a"].is_empty());
        assert!(map["session-b"].is_empty());
    }

    #[test]
    fn releasing_an_unknown_session_does_nothing() {
        release("session-never-seen");
    }
}

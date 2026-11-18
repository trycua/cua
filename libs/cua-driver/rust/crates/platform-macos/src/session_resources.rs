// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

use std::collections::HashMap;
use std::sync::{Mutex, Once};

use crate::virtual_display::{self, DisplayInfo};

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

/// Run `create` on the blocking pool and record the display for `session` in
/// the same task, so a caller cancelled mid-create still leaves the display
/// owned and released when the session ends.
pub fn create_owned(
    session: String,
    create: impl FnOnce() -> Result<DisplayInfo, String> + Send + 'static,
) -> tokio::task::JoinHandle<Result<DisplayInfo, String>> {
    tokio::task::spawn_blocking(move || {
        let info = create()?;
        own_display(&session, info.display_id);
        Ok(info)
    })
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

    fn owned_by(session: &str) -> Vec<u32> {
        let guard = OWNED.lock().unwrap();
        guard
            .as_ref()
            .and_then(|map| map.get(session).cloned())
            .unwrap_or_default()
    }

    #[tokio::test]
    async fn a_display_created_after_its_caller_is_cancelled_is_still_owned() {
        let (release_create, wait_for_release) = std::sync::mpsc::channel::<()>();
        let (started, wait_for_start) = std::sync::mpsc::channel::<()>();
        let task = create_owned("session-cancelled".to_owned(), move || {
            started.send(()).unwrap();
            wait_for_release.recv().unwrap();
            Ok(DisplayInfo {
                display_id: 9002,
                x: 0.0,
                y: 0.0,
                width: 1280.0,
                height: 720.0,
            })
        });
        wait_for_start.recv().unwrap();
        drop(task);
        release_create.send(()).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while owned_by("session-cancelled").is_empty() && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(owned_by("session-cancelled"), vec![9002]);
    }

    #[test]
    fn releasing_an_unknown_session_does_nothing() {
        release("session-never-seen");
    }
}

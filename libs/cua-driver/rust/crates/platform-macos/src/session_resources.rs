// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, Once};

use crate::virtual_display::{self, DisplayInfo};

#[derive(Default)]
struct Owned {
    displays: HashMap<String, Vec<u32>>,
    creating: HashMap<String, Creating>,
}

/// Creates still running for one session, and how many times that session has
/// ended since the first of them started. An end bumps `ends` even when the
/// same id is revived before a create finishes, so the create can tell its
/// session episode is gone.
#[derive(Default)]
struct Creating {
    in_flight: usize,
    ends: u64,
}

static OWNED: Mutex<Option<Owned>> = Mutex::new(None);
static INSTALL: Once = Once::new();

fn install() {
    INSTALL.call_once(|| {
        cua_driver_core::session::register_session_end_hook(release);
    });
}

fn owned() -> MutexGuard<'static, Option<Owned>> {
    OWNED.lock().unwrap_or_else(|e| e.into_inner())
}

/// Run `create` on the blocking pool and record the display for `session` in
/// the same task, so a caller cancelled mid-create still leaves the display
/// owned and released when the session ends. If the session ended while the
/// display was being created, its cleanup has already run, so the display is
/// removed here instead.
pub fn create_owned(
    session: String,
    create: impl FnOnce() -> Result<DisplayInfo, String> + Send + 'static,
) -> tokio::task::JoinHandle<Result<DisplayInfo, String>> {
    install();
    let episode = {
        let mut guard = owned();
        let creating = guard
            .get_or_insert_with(Owned::default)
            .creating
            .entry(session.clone())
            .or_default();
        creating.in_flight += 1;
        creating.ends
    };
    tokio::task::spawn_blocking(move || {
        let created = create();
        let ended = {
            let mut guard = owned();
            let state = guard.get_or_insert_with(Owned::default);
            let ended = match state.creating.get_mut(&session) {
                Some(creating) => {
                    let ended = creating.ends != episode;
                    creating.in_flight -= 1;
                    if creating.in_flight == 0 {
                        state.creating.remove(&session);
                    }
                    ended
                }
                None => false,
            };
            if let (Ok(info), false) = (&created, ended) {
                let displays = state.displays.entry(session.clone()).or_default();
                if !displays.contains(&info.display_id) {
                    displays.push(info.display_id);
                }
            }
            ended
        };
        let info = created?;
        if ended {
            let _ = virtual_display::remove(info.display_id);
            return Err(format!(
                "session {session} ended while display {} was being created; it was removed",
                info.display_id
            ));
        }
        Ok(info)
    })
}

pub fn forget_display(display_id: u32) {
    if let Some(state) = owned().as_mut() {
        for displays in state.displays.values_mut() {
            displays.retain(|id| *id != display_id);
        }
    }
}

fn release(session: &str) {
    let displays = {
        let mut guard = owned();
        let Some(state) = guard.as_mut() else {
            return;
        };
        if let Some(creating) = state.creating.get_mut(session) {
            creating.ends += 1;
        }
        state.displays.remove(session)
    };
    for id in displays.unwrap_or_default() {
        let _ = virtual_display::remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display(display_id: u32) -> DisplayInfo {
        DisplayInfo {
            display_id,
            x: 0.0,
            y: 0.0,
            width: 1280.0,
            height: 720.0,
        }
    }

    fn owned_by(session: &str) -> Vec<u32> {
        owned()
            .as_ref()
            .and_then(|state| state.displays.get(session).cloned())
            .unwrap_or_default()
    }

    /// Start a create for `session` that finishes with `display_id` once the
    /// returned sender fires.
    fn blocked_create(
        session: &str,
        display_id: u32,
    ) -> (
        std::sync::mpsc::Sender<()>,
        tokio::task::JoinHandle<Result<DisplayInfo, String>>,
    ) {
        let (release_create, wait_for_release) = std::sync::mpsc::channel::<()>();
        let task = create_owned(session.to_owned(), move || {
            wait_for_release.recv().unwrap();
            Ok(display(display_id))
        });
        (release_create, task)
    }

    #[tokio::test]
    async fn forgetting_a_display_drops_it_from_every_session() {
        for session in ["session-a", "session-b"] {
            let (release_create, task) = blocked_create(session, 9001);
            release_create.send(()).unwrap();
            task.await.unwrap().unwrap();
        }
        forget_display(9001);
        assert!(owned_by("session-a").is_empty());
        assert!(owned_by("session-b").is_empty());
    }

    #[tokio::test]
    async fn a_display_created_after_its_caller_is_cancelled_is_still_owned() {
        let (release_create, task) = blocked_create("session-cancelled", 9002);
        drop(task);
        release_create.send(()).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while owned_by("session-cancelled").is_empty() && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(owned_by("session-cancelled"), vec![9002]);
    }

    #[tokio::test]
    async fn a_display_created_after_its_session_ended_is_not_left_owned() {
        let (release_create, task) = blocked_create("session-ended-mid-create", 9003);
        assert!(cua_driver_core::session::fire_session_end(
            "session-ended-mid-create"
        ));
        release_create.send(()).unwrap();
        let err = task.await.unwrap().unwrap_err();
        assert!(err.contains("ended while display 9003"), "{err}");
        assert!(owned_by("session-ended-mid-create").is_empty());
    }

    #[tokio::test]
    async fn a_display_created_across_an_end_and_revive_is_not_left_owned() {
        let session = "session-revived-mid-create";
        let (release_create, task) = blocked_create(session, 9004);
        assert!(cua_driver_core::session::fire_session_end(session));
        assert!(cua_driver_core::session::revive_session(session));
        release_create.send(()).unwrap();
        let err = task.await.unwrap().unwrap_err();
        assert!(err.contains("ended while display 9004"), "{err}");
        assert!(owned_by(session).is_empty());
    }

    #[test]
    fn releasing_an_unknown_session_does_nothing() {
        release("session-never-seen");
    }
}

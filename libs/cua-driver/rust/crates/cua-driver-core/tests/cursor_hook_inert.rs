//! The cursor hook must cost nothing for the users who never register one.
//!
//! That is the overwhelming majority of cua-driver consumers: the daemon, the
//! CLI, every SDK caller that does not embed a remote-desktop host. This file
//! is a *separate test binary* on purpose: `set_cursor_hook_fn` writes a
//! process-wide `OnceLock`, so "no hook is registered" is only observable in a
//! process where nothing ever registers one. Merging these assertions into a
//! binary that also exercises registration would silently stop testing the
//! unregistered case the moment test order changed.

use cua_driver_core::cursor_hook::{cursor_hook_enabled, push_cursor_event, CursorHookEvent};

#[test]
fn hook_is_inert_and_silent_when_nothing_is_registered() {
    assert!(
        !cursor_hook_enabled(),
        "no hook was registered in this process, so the hot-path fast check must report disabled"
    );

    // Pushing must be a no-op rather than a panic, an unwrap of an empty
    // OnceLock, or a deadlock. The macOS cursor write path calls this on every
    // commanded move, so a fault here would take down every consumer that never
    // asked for the feature.
    for i in 0..1000 {
        push_cursor_event(CursorHookEvent {
            cursor_id: format!("cursor-{i}"),
            x: i as f64,
            y: -(i as f64),
            pressed: i % 2 == 0,
        });
    }

    assert!(
        !cursor_hook_enabled(),
        "pushing events must not implicitly install a hook"
    );
}

/// Concurrent pushes with no hook installed must stay a no-op and must not
/// contend on anything. `OnceLock::get` on an unset cell is a relaxed load, so
/// this also documents that the unregistered path takes no lock.
#[test]
fn concurrent_pushes_without_a_hook_are_safe() {
    let threads: Vec<_> = (0..8)
        .map(|t| {
            std::thread::spawn(move || {
                for i in 0..500 {
                    push_cursor_event(CursorHookEvent {
                        cursor_id: format!("t{t}"),
                        x: i as f64,
                        y: i as f64,
                        pressed: false,
                    });
                }
            })
        })
        .collect();
    for t in threads {
        t.join().expect("no-op pushes must not panic");
    }
    assert!(!cursor_hook_enabled());
}

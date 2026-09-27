//! The cursor hook must publish its platform limitation explicitly.
//!
//! `AGENTS.md`: "When a platform cannot support the same contract, return or
//! publish that limitation explicitly instead of substituting misleading
//! behavior."
//!
//! Only the macOS adapter emits cursor events; the Windows and Linux adapters
//! have no cursor write path wired to the hook. But `set_cursor_hook_fn`
//! succeeds everywhere, so an embedder that trusted `cursor_hook_enabled()`
//! would conclude on Windows that tracking was live and then wait forever for a
//! first event. The viewer would show a pointer frozen at the origin — a
//! confidently wrong picture, indistinguishable from a hung stream.
//!
//! `cursor_hook_supported()` is the explicit publication of that limitation.
//! Its own binary, because both flags it reads are process-global one-shots and
//! this file must observe the *undeclared* state.

use cua_driver_core::cursor_hook::{
    cursor_hook_enabled, cursor_hook_supported, declare_cursor_hook_emitter, push_cursor_event,
    set_cursor_hook_fn, CursorHookEvent,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

#[test]
fn support_is_about_the_platform_not_about_registration() {
    // A bare core build — which is what the Windows and Linux adapters link
    // against for this module — declares no emitter.
    assert!(
        !cursor_hook_supported(),
        "no adapter declared an emitter, so this build must report cursor \
         tracking as unsupported"
    );

    // Registering a hook must NOT make the platform look supported. This is the
    // whole point: the two questions are independent, and conflating them is
    // what makes a silent Windows host look like a working one.
    let seen = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&seen);
    set_cursor_hook_fn(move |_| {
        counter.fetch_add(1, Ordering::SeqCst);
    });

    assert!(
        cursor_hook_enabled(),
        "an embedder did register, so the hook is enabled"
    );
    assert!(
        !cursor_hook_supported(),
        "registering an observer must not be mistaken for the platform being \
         able to produce events; an embedder must still be able to tell the \
         user that this host cannot track cursors"
    );

    // ...and once an adapter declares itself, support flips — while the two
    // remain separately observable.
    declare_cursor_hook_emitter();
    assert!(cursor_hook_supported());
    assert!(cursor_hook_enabled());

    // Declaring an emitter must not synthesise events by itself.
    assert_eq!(
        seen.load(Ordering::SeqCst),
        0,
        "declaring support must not fabricate cursor events"
    );
    push_cursor_event(CursorHookEvent {
        cursor_id: "s".into(),
        x: 0.0,
        y: 0.0,
        pressed: false,
    });
    assert_eq!(seen.load(Ordering::SeqCst), 1);

    // Idempotent: repeated declaration is harmless.
    declare_cursor_hook_emitter();
    assert!(cursor_hook_supported());
}

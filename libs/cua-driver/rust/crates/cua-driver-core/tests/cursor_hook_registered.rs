//! Behaviour of the cursor hook once an embedder has registered one.
//!
//! Separate binary from `cursor_hook_inert.rs` because `set_cursor_hook_fn`
//! backs onto a process-wide `OnceLock`: registration is irreversible, so the
//! registered and unregistered contracts cannot be asserted in one process.
//!
//! The single `#[test]` below is deliberate for the same reason. Rust runs the
//! tests in a binary on parallel threads; splitting these assertions into
//! several tests would race over the one-shot registration and the shared sink.

use cua_driver_core::cursor_hook::{
    cursor_hook_enabled, push_cursor_event, set_cursor_hook_fn, CursorHookEvent,
};
use std::sync::{Arc, Mutex};

#[test]
fn registered_hook_receives_events_verbatim_and_keeps_sources_distinct() {
    let sink: Arc<Mutex<Vec<CursorHookEvent>>> = Arc::new(Mutex::new(Vec::new()));

    assert!(
        !cursor_hook_enabled(),
        "precondition: nothing registered yet in this process"
    );

    let recorder = Arc::clone(&sink);
    assert!(
        set_cursor_hook_fn(move |ev| recorder.lock().unwrap().push(ev)),
        "the first registration in a process must report that it took effect"
    );

    assert!(
        cursor_hook_enabled(),
        "after registration the hot-path check must report enabled"
    );

    // ── Fields survive the trip unmodified ────────────────────────────────
    // A host renders the pointer from exactly these numbers; a hook that
    // rounded, swapped or dropped a field would misplace every cursor.
    push_cursor_event(CursorHookEvent {
        cursor_id: "session-alpha".into(),
        x: 12.5,
        y: -40.25,
        pressed: false,
    });
    push_cursor_event(CursorHookEvent {
        cursor_id: "session-alpha".into(),
        x: 12.5,
        y: -40.25,
        pressed: true,
    });

    {
        let got = sink.lock().unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].cursor_id, "session-alpha");
        assert_eq!(got[0].x, 12.5);
        assert_eq!(got[0].y, -40.25);
        assert!(!got[0].pressed, "a move must arrive as pressed=false");
        assert!(got[1].pressed, "a press edge must arrive as pressed=true");
    }
    sink.lock().unwrap().clear();

    // ── Identity: N participants must not collapse into one cursor ────────
    // The hook carries `cursor_id` precisely so a multi-participant host can
    // draw one pointer per agent. A consumer that stamped a constant id (or an
    // emitter that dropped the caller's id) would render every agent on top of
    // every other. This asserts the driver hands out the caller's own id.
    for id in ["agent-a", "agent-b", "agent-c"] {
        push_cursor_event(CursorHookEvent {
            cursor_id: id.into(),
            x: 1.0,
            y: 2.0,
            pressed: false,
        });
    }
    {
        let got = sink.lock().unwrap();
        let ids: Vec<&str> = got.iter().map(|e| e.cursor_id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["agent-a", "agent-b", "agent-c"],
            "each source must keep its own cursor id; collapsing them merges every \
             participant's pointer into one"
        );
    }
    sink.lock().unwrap().clear();

    // ── Re-registration is ignored, not honoured ──────────────────────────
    // Documented as "call once at startup". If a second registration silently
    // replaced the first, a library initialising late would steal the host's
    // cursor stream and the host would go blind with no error.
    let hijack = Arc::new(Mutex::new(0usize));
    let hijack_probe = Arc::clone(&hijack);
    assert!(
        !set_cursor_hook_fn(move |_| {
            *hijack_probe.lock().unwrap() += 1;
        }),
        "a second registration must REPORT that it did not take effect. \
         Silently returning as though it had is how a host ends up rendering \
         no pointers with no way to discover why it owns no stream"
    );
    push_cursor_event(CursorHookEvent {
        cursor_id: "after-second-register".into(),
        x: 0.0,
        y: 0.0,
        pressed: false,
    });
    assert_eq!(
        *hijack.lock().unwrap(),
        0,
        "a second set_cursor_hook_fn must be ignored"
    );
    assert_eq!(
        sink.lock().unwrap().len(),
        1,
        "the originally registered hook must keep receiving events"
    );
    sink.lock().unwrap().clear();

    // ── Concurrency: every event from every thread is delivered exactly once ──
    let threads: Vec<_> = (0..8)
        .map(|t| {
            std::thread::spawn(move || {
                for i in 0..100 {
                    push_cursor_event(CursorHookEvent {
                        cursor_id: format!("thread-{t}"),
                        x: i as f64,
                        y: t as f64,
                        pressed: false,
                    });
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    let got = sink.lock().unwrap();
    assert_eq!(got.len(), 800, "no event may be dropped or duplicated");
    for t in 0..8 {
        let n = got
            .iter()
            .filter(|e| e.cursor_id == format!("thread-{t}"))
            .count();
        assert_eq!(n, 100, "thread {t}: all of its events must survive intact");
    }
}

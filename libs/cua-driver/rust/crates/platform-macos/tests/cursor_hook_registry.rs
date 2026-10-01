//! The cursor hook must obey the registry's write-boundary resurrection guard.
//!
//! `CursorRegistry` refuses to create or touch a cursor whose id is empty or
//! whose session has already ended — the "resurrection guard" repeated at every
//! write boundary in `cursor/state.rs`. The cursor hook is a new write
//! boundary: it publishes cursor activity to an embedder that draws pointers on
//! a remote viewer. If the hook escaped the guard, a click arriving after
//! `session_end` would make the embedder re-draw a cursor the driver has
//! already cleared, and the viewer would show a ghost pointer that nothing can
//! remove.
//!
//! Before the fix this was live: `update_position` applied the guard, but the
//! press edge was pushed straight to `push_cursor_event` from four call sites
//! in `tools/click.rs`, bypassing it. The session_end and empty-id assertions
//! below each fail against that version and pass against this one.
//!
//! One test binary, one `#[test]`: `set_cursor_hook_fn` is a process-wide
//! one-shot, so the registration and the assertions must share a process and
//! must not race.

use cua_driver_core::cursor_hook::{set_cursor_hook_fn, CursorHookEvent};
use platform_macos::cursor::state::CursorRegistry;
use std::sync::{Arc, Mutex};

#[test]
fn registry_cursor_hook_honours_the_resurrection_guard() {
    let sink: Arc<Mutex<Vec<CursorHookEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&sink);
    set_cursor_hook_fn(move |ev| recorder.lock().unwrap().push(ev));

    let take = || -> Vec<CursorHookEvent> { std::mem::take(&mut *sink.lock().unwrap()) };

    let registry = CursorRegistry::new();

    // ── A live cursor reports both its move and its press ─────────────────
    registry.update_position("session-live", 10.0, 20.0);
    registry.note_press("session-live", 10.0, 20.0);
    let got = take();
    assert_eq!(got.len(), 2, "a live cursor must report move and press");
    assert_eq!(got[0].cursor_id, "session-live");
    assert_eq!((got[0].x, got[0].y), (10.0, 20.0));
    assert!(!got[0].pressed);
    assert!(got[1].pressed, "note_press must mark the event as a press");
    assert_eq!(
        (got[1].x, got[1].y),
        (10.0, 20.0),
        "a press must carry the point it happened at"
    );

    // ── Distinct sessions stay distinct ───────────────────────────────────
    // A multi-participant host draws one pointer per session id. Were the
    // registry to normalise or share ids, every participant would land on the
    // same pointer.
    registry.update_position("session-a", 1.0, 1.0);
    registry.update_position("session-b", 2.0, 2.0);
    registry.note_press("session-b", 2.0, 2.0);
    let got = take();
    let ids: Vec<&str> = got.iter().map(|e| e.cursor_id.as_str()).collect();
    assert_eq!(ids, vec!["session-a", "session-b", "session-b"]);
    assert_eq!(
        got[0].x, 1.0,
        "session-a must keep its own position, not session-b's"
    );

    // ── The empty-id sentinel means "no cursor", so it must emit nothing ───
    // `cursor_key` is an empty string when there is no cursor to speak of.
    // `update_position` already treated it that way; the press path must agree,
    // otherwise the same value means "anonymous default cursor" on one path and
    // "no cursor at all" on the other.
    registry.update_position("", 5.0, 5.0);
    registry.note_press("", 5.0, 5.0);
    assert!(
        take().is_empty(),
        "the empty cursor id is the no-cursor sentinel: neither a move nor a \
         press may reach the hook"
    );

    // ── A press after session_end must be suppressed (the regression) ──────
    registry.update_position("session-ending", 7.0, 7.0);
    assert_eq!(take().len(), 1, "precondition: the session is live");

    cua_driver_core::session::end_session("session-ending");

    registry.update_position("session-ending", 8.0, 8.0);
    registry.note_press("session-ending", 8.0, 8.0);
    assert!(
        take().is_empty(),
        "after session_end the registry has cleared this cursor; neither an \
         in-flight move nor an in-flight press may reach the embedder, or the \
         viewer resurrects a pointer the driver considers gone"
    );

    // ── Suppression is scoped to the ended session only ────────────────────
    // A guard that over-fired would silence every other participant's cursor.
    registry.update_position("session-live", 9.0, 9.0);
    registry.note_press("session-live", 9.0, 9.0);
    assert_eq!(
        take().len(),
        2,
        "ending one session must not silence any other cursor"
    );
}

/// Structural lock on the fix above.
///
/// The guard is only worth anything if it cannot be walked around. The original
/// bug was not a wrong guard, it was four tool call sites that never reached
/// one — and the natural way to wire up the next tool is to copy an existing
/// `push_cursor_event(...)` block, which is exactly how the bypass would come
/// back. Keeping emission to a single private choke point inside
/// `CursorRegistry` means any new tool must go through `update_position` or
/// `note_press` and is guarded automatically.
///
/// This asserts the invariant on the source itself because there is no runtime
/// signal for "a tool pushed an event directly": such a call simply works, and
/// silently skips the guard.
#[test]
fn cursor_events_are_emitted_from_exactly_one_place_in_this_adapter() {
    fn rs_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("readable source dir") {
            let path = entry.expect("readable entry").path();
            if path.is_dir() {
                rs_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rs_files(&src, &mut files);
    assert!(!files.is_empty(), "expected to find adapter sources");

    let mut offenders = Vec::new();
    for file in &files {
        let text = std::fs::read_to_string(file).expect("readable source file");
        for (i, line) in text.lines().enumerate() {
            // Skip prose: the call is named in doc comments explaining the rule.
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") {
                continue;
            }
            if line.contains("push_cursor_event") {
                let _ = i;
                offenders.push(
                    file.strip_prefix(&src)
                        .unwrap_or(file)
                        .display()
                        .to_string(),
                );
            }
        }
    }

    assert_eq!(
        offenders,
        vec!["cursor/state.rs".to_string()],
        "cursor events must be pushed only from CursorRegistry::emit_cursor_event. \
         A direct push_cursor_event call elsewhere bypasses the empty-id and \
         session_end resurrection guards, which is the exact regression \
         registry_cursor_hook_honours_the_resurrection_guard exists to prevent. \
         Call update_position or note_press instead. \
         (If emit_cursor_event legitimately moved, update the expected location.)"
    );
}

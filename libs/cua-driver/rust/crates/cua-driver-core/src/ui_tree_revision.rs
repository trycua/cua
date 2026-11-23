// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Per-window UI-tree revision (#3586).
//!
//! Every `get_window_state` tree read records its rendered rows here. A
//! window's `ui_tree_revision` stays the same while reads of the same view
//! (query, max_elements, max_depth) return the same rows, and moves to a new,
//! larger value when they change. A `since` read that answers `no_change`
//! therefore keeps the revision, and a `diff` moves it.
//!
//! An action may carry [`UI_TREE_REVISION_ARG`], the revision its target was
//! computed from. Dispatch refuses it with `stale_revision` before delivery
//! when a later read of that window saw the tree change, or when this driver
//! has no revision for the window. The check covers changes a read has
//! observed: the ledger does not watch the window between reads.
//!
//! Revisions come from one process-wide counter that starts at the process
//! start time in milliseconds, so a revision from an earlier driver process
//! does not match one this process hands out.

use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use serde_json::{json, Value};

use crate::protocol::ToolResult;
use crate::window_state_view::ViewKey;

/// The per-call action argument and the `get_window_state` output field.
pub const UI_TREE_REVISION_ARG: &str = "ui_tree_revision";

/// Actions that accept [`UI_TREE_REVISION_ARG`].
pub const UI_TREE_REVISION_TOOLS: &[&str] = &[
    "click",
    "double_click",
    "right_click",
    "drag",
    "scroll",
    "type_text",
    "press_key",
    "hotkey",
    "set_value",
];

/// Views remembered per window; the oldest is dropped past this.
const VIEWS_PER_WINDOW: usize = 4;
/// Snapshot ids remembered per window, to find the window of an element token.
const SNAPSHOTS_PER_WINDOW: usize = 8;
/// Windows remembered; the least recently read is dropped past this.
const WINDOWS: usize = 64;

struct WindowEntry {
    revision: u64,
    /// Row hash of the last read of each view, most recent last.
    views: VecDeque<(ViewKey, u64)>,
    snapshots: VecDeque<String>,
    last_read: u64,
}

#[derive(Default)]
struct Ledger {
    windows: HashMap<(i64, u64), WindowEntry>,
    reads: u64,
}

fn ledger() -> &'static Mutex<Ledger> {
    static LEDGER: OnceLock<Mutex<Ledger>> = OnceLock::new();
    LEDGER.get_or_init(|| Mutex::new(Ledger::default()))
}

fn next_revision() -> u64 {
    static NEXT: OnceLock<AtomicU64> = OnceLock::new();
    NEXT.get_or_init(|| {
        let start_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(1);
        AtomicU64::new(start_ms.max(1))
    })
    .fetch_add(1, Ordering::Relaxed)
}

fn rows_hash(markdown: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    markdown.hash(&mut hasher);
    hasher.finish()
}

/// Record a tree read of `(pid, window_id)` and return the window's revision.
///
/// The first read of a window, and a read whose rows differ from the last
/// read of the same view, take a new revision. A view read for the first
/// time keeps the window's revision: there is no earlier read of it to
/// compare with.
pub fn record(
    pid: i64,
    window_id: u64,
    view: &ViewKey,
    snapshot_id: Option<&str>,
    markdown: &str,
) -> u64 {
    let hash = rows_hash(markdown);
    let mut guard = ledger().lock().unwrap_or_else(|e| e.into_inner());
    guard.reads += 1;
    let read = guard.reads;
    let entry = guard
        .windows
        .entry((pid, window_id))
        .or_insert_with(|| WindowEntry {
            revision: next_revision(),
            views: VecDeque::new(),
            snapshots: VecDeque::new(),
            last_read: read,
        });
    entry.last_read = read;
    match entry.views.iter().position(|(key, _)| key == view) {
        Some(pos) => {
            let (_, previous) = entry.views.remove(pos).expect("position is in range");
            if previous != hash {
                entry.revision = next_revision();
            }
        }
        None => {
            if entry.views.len() >= VIEWS_PER_WINDOW {
                entry.views.pop_front();
            }
        }
    }
    entry.views.push_back((view.clone(), hash));
    if let Some(sid) = snapshot_id {
        if !entry.snapshots.iter().any(|s| s == sid) {
            if entry.snapshots.len() >= SNAPSHOTS_PER_WINDOW {
                entry.snapshots.pop_front();
            }
            entry.snapshots.push_back(sid.to_owned());
        }
    }
    let revision = entry.revision;
    if guard.windows.len() > WINDOWS {
        if let Some(oldest) = guard
            .windows
            .iter()
            .min_by_key(|(_, e)| e.last_read)
            .map(|(key, _)| *key)
        {
            guard.windows.remove(&oldest);
        }
    }
    revision
}

/// The window's current revision, when this driver has read its tree.
pub fn current(pid: i64, window_id: u64) -> Option<u64> {
    let guard = ledger().lock().unwrap_or_else(|e| e.into_inner());
    guard.windows.get(&(pid, window_id)).map(|e| e.revision)
}

/// The window whose read produced `snapshot_id`, while it is remembered.
fn window_for_snapshot(snapshot_id: &str) -> Option<(i64, u64)> {
    let guard = ledger().lock().unwrap_or_else(|e| e.into_inner());
    guard
        .windows
        .iter()
        .find(|(_, e)| e.snapshots.iter().any(|s| s == snapshot_id))
        .map(|(key, _)| *key)
}

/// Whether `tool` accepts [`UI_TREE_REVISION_ARG`].
pub fn accepts(tool: &str) -> bool {
    UI_TREE_REVISION_TOOLS.contains(&tool)
}

/// The JSON schema of [`UI_TREE_REVISION_ARG`].
pub fn schema() -> Value {
    json!({
        "type": "integer",
        "minimum": 0,
        "description": "The `ui_tree_revision` of the get_window_state read this action was \
                        planned from. The action is refused with `stale_revision`, before any \
                        input is delivered, when a later read of the same window saw the tree \
                        change or the driver has no revision for the window. It covers changes \
                        a read has observed: the driver does not watch the window between reads."
    })
}

/// Check an action's [`UI_TREE_REVISION_ARG`] against the target window's
/// current revision. `Ok` when the argument is absent or current; `Err`
/// carries the refusal. The window comes from `pid` + `window_id`, or from
/// the read that produced `element_token`.
pub fn check(tool: &str, args: &Value) -> Result<(), ToolResult> {
    let Some(value) = args.get(UI_TREE_REVISION_ARG) else {
        return Ok(());
    };
    let Some(expected) = value.as_u64() else {
        let detail = format!("ui_tree_revision must be a non-negative integer, got {value}");
        return Err(
            ToolResult::error(format!("{tool}: {detail}")).with_structured(json!({
                "code": "invalid_arguments",
                "tool": tool,
                "detail": detail,
            })),
        );
    };
    let window = match (
        args.get("pid").and_then(Value::as_i64),
        args.get("window_id").and_then(Value::as_u64),
    ) {
        (Some(pid), Some(window_id)) => Some((pid, window_id)),
        _ => args
            .get("element_token")
            .and_then(Value::as_str)
            .and_then(crate::element_token::parse_token)
            .and_then(|(snapshot_id, _)| {
                window_for_snapshot(&crate::element_token::format_snapshot_id(snapshot_id))
            }),
    };
    let now = window.and_then(|(pid, window_id)| current(pid, window_id));
    if now == Some(expected) {
        return Ok(());
    }
    let (reason, message) = match (window, now) {
        (None, _) => (
            "unknown_window",
            format!(
                "{tool} refused: ui_tree_revision {expected} names no window. Pass pid and \
                 window_id, or an element_token from a get_window_state read this driver \
                 still remembers."
            ),
        ),
        (Some(_), None) => (
            "no_revision",
            format!(
                "{tool} refused: this driver has no ui_tree_revision for the window. Read it \
                 with get_window_state and plan the action from that read."
            ),
        ),
        (Some(_), Some(current)) => (
            "changed",
            format!(
                "{tool} refused: the window's tree changed since ui_tree_revision {expected} \
                 (now {current}). Read it again with get_window_state (since: <snapshot_id>) \
                 and plan the action from that read."
            ),
        ),
    };
    Err(ToolResult::error(message.clone()).with_structured(json!({
        "status": "refused",
        "refusal": {
            "code": "stale_revision",
            "message": message,
            "reason": reason,
            "expected": expected,
            "current": now,
        }
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(max_elements: usize) -> ViewKey {
        ViewKey {
            query: None,
            max_elements,
            max_depth: None,
        }
    }

    // Each test uses its own pid: the ledger is process-wide.

    #[test]
    fn same_rows_keep_the_revision_and_changed_rows_move_it() {
        let first = record(9101, 1, &view(250), Some("s1"), "- [1] Button \"OK\"");
        let again = record(9101, 1, &view(250), Some("s2"), "- [1] Button \"OK\"");
        assert_eq!(first, again);
        let changed = record(9101, 1, &view(250), Some("s3"), "- [1] Button \"Done\"");
        assert!(changed > first, "{changed} > {first}");
        assert_eq!(current(9101, 1), Some(changed));
    }

    #[test]
    fn a_new_view_keeps_the_revision_and_its_own_change_moves_it() {
        let first = record(9102, 1, &view(250), None, "- [1] Button \"OK\"");
        let other = record(9102, 1, &view(10), None, "- [1] Button \"OK\" (short)");
        assert_eq!(first, other);
        // Alternating views with unchanged rows never moves it.
        assert_eq!(
            record(9102, 1, &view(250), None, "- [1] Button \"OK\""),
            first
        );
        assert_eq!(
            record(9102, 1, &view(10), None, "- [1] Button \"OK\" (short)"),
            first
        );
        assert!(record(9102, 1, &view(10), None, "- [1] Button \"Done\"") > first);
    }

    #[test]
    fn windows_have_their_own_revisions() {
        let a = record(9103, 1, &view(250), None, "- [1] Button \"OK\"");
        let b = record(9103, 2, &view(250), None, "- [1] Button \"OK\"");
        assert_ne!(a, b);
        assert_eq!(current(9103, 1), Some(a));
        assert_eq!(current(9103, 3), None);
    }

    fn refusal(result: ToolResult) -> Value {
        assert_eq!(result.is_error, Some(true));
        result.structured_content.unwrap()["refusal"].clone()
    }

    #[test]
    fn a_current_revision_passes_and_a_moved_one_is_refused() {
        let rev = record(
            9104,
            7,
            &view(250),
            Some("s0009104a"),
            "- [1] Button \"OK\"",
        );
        assert!(check(
            "click",
            &json!({"pid": 9104, "window_id": 7, "ui_tree_revision": rev})
        )
        .is_ok());
        assert!(check(
            "click",
            &json!({"element_token": "s0009104a:1", "ui_tree_revision": rev})
        )
        .is_ok());
        assert!(check("click", &json!({"pid": 9104, "window_id": 7})).is_ok());

        let now = record(
            9104,
            7,
            &view(250),
            Some("s0009104b"),
            "- [1] Button \"Done\"",
        );
        for args in [
            json!({"pid": 9104, "window_id": 7, "ui_tree_revision": rev}),
            json!({"element_token": "s0009104a:1", "ui_tree_revision": rev}),
        ] {
            let r = refusal(check("click", &args).unwrap_err());
            assert_eq!(r["code"], "stale_revision");
            assert_eq!(r["reason"], "changed");
            assert_eq!(r["expected"], rev);
            assert_eq!(r["current"], now);
        }
    }

    #[test]
    fn an_unread_or_unnamed_window_is_refused() {
        let r = refusal(
            check(
                "click",
                &json!({"pid": 9105, "window_id": 1, "ui_tree_revision": 5}),
            )
            .unwrap_err(),
        );
        assert_eq!(r["code"], "stale_revision");
        assert_eq!(r["reason"], "no_revision");
        assert_eq!(r["current"], Value::Null);

        let r = refusal(
            check(
                "press_key",
                &json!({"element_token": "s000910f:1", "ui_tree_revision": 5}),
            )
            .unwrap_err(),
        );
        assert_eq!(r["code"], "stale_revision");
        assert_eq!(r["reason"], "unknown_window");
    }

    #[test]
    fn a_malformed_revision_is_invalid_arguments() {
        for value in [json!(-1), json!("3"), json!(1.5), json!(null)] {
            let result = check(
                "click",
                &json!({"pid": 1, "window_id": 1, "ui_tree_revision": value}),
            )
            .unwrap_err();
            assert_eq!(
                result.structured_content.unwrap()["code"],
                "invalid_arguments",
                "{value}"
            );
        }
    }
}

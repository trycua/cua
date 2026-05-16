//! Window-change detector — Rust port of Swift's
//! `WindowChangeDetector` (`libs/cua-driver/Sources/CuaDriverServer/Tools/WindowChangeDetector.swift`).
//!
//! ## What this does
//!
//! Action tools (click, type_text, hotkey, …) on a backgrounded app can
//! trigger window/foreground side-effects: a "Sign In" button opens a
//! modal sheet, a Safari link spawns a new tab, an autocomplete dropdown
//! pops a helper window. The Rust port mirrors Swift's
//! snapshot → action → detect cycle so tool results can:
//!
//! 1. Surface the side-effect to the agent (one-line suffix on the
//!    tool result, matching Swift verbatim).
//! 2. Arm a **wildcard** focus-steal suppression entry that covers the
//!    full snapshot→detect window. Wildcards (`target_pid = None`)
//!    catch any activation other than the prior frontmost — so even an
//!    app we didn't know about (Safari activating because a UTM Gallery
//!    link routed to it) is suppressed before the first compositor
//!    frame.
//!
//! ## Usage
//!
//! ```ignore
//! // Callers capture frontmost BEFORE the snapshot so the wildcard
//! // suppressor and the snapshot's recorded frontmost agree on the
//! // pid to restore to — avoids a race where another app activates
//! // between the caller's `frontmost_pid()` and the detector's own.
//! let prior_front = apps::frontmost_pid();
//! let snapshot = WindowChangeDetector::snapshot(prior_front);
//! // … perform action …
//! let changes = snapshot.detect();
//! // changes.result_suffix() — append to ToolResult text.
//! ```
//!
//! Dropping the `Snapshot` ends the suppression lease (RAII). `detect()`
//! also drops the lease before returning — the lease's `Drop` is
//! idempotent so explicit-detect + later-drop is safe.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use crate::apps;
use crate::focus_steal::{self, SuppressionLease};
use crate::windows::{self, WindowInfo};

/// One window that appeared between `snapshot()` and `detect()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowEvent {
    pub window_id: u32,
    pub pid: i32,
    pub app_name: String,
    pub title: String,
}

/// Categorical diff entry. We mirror Swift which only emits
/// `WindowEvent` rows for *new* windows — closed/changed never appear
/// in the result suffix — but keep them as enum variants for future
/// extensibility and so unit tests can pin down the diff semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowChange {
    Opened(WindowEvent),
    Closed { window_id: u32 },
}

/// State captured immediately before the action fires.
///
/// Holds:
/// - `window_ids` — the set of visible layer-0 window IDs at snapshot
///   time. `detect()` diffs against this.
/// - `front_pid` — the OS frontmost pid at snapshot time. `detect()`
///   reports whether a *different* pid became frontmost. The wildcard
///   suppressor in `focus_steal` will normally restore the original
///   front before `detect()`'s poll loop observes the change, so this
///   field is best-effort.
/// - `_lease` — the wildcard suppression lease. Dropping the snapshot
///   ends suppression. Held inside `Option` so `detect()` can take it
///   and drop early.
pub struct Snapshot {
    window_ids: HashSet<u32>,
    front_pid: Option<i32>,
    _lease: Option<SuppressionLease>,
}

/// Result of `detect()` — what changed during the action window.
#[derive(Debug, Clone)]
pub struct Changes {
    pub new_windows: Vec<WindowEvent>,
    pub foreground_changed: bool,
}

impl Changes {
    pub fn no_change() -> Self {
        Self {
            new_windows: Vec::new(),
            foreground_changed: false,
        }
    }

    /// True when we found evidence that the action triggered a cross-app
    /// side-effect that required (or would have required) a foreground
    /// restore. Matches Swift's `Changes.needsRestore`.
    pub fn needs_restore(&self) -> bool {
        self.foreground_changed || !self.new_windows.is_empty()
    }

    /// One-liner summary to append to a tool result, or empty string
    /// when nothing interesting happened.
    ///
    /// Format mirrors Swift `WindowChangeDetector.Changes.resultSuffix`
    /// **verbatim** so MCP callers that key off the suffix wording
    /// don't need a per-binary special case.
    pub fn result_suffix(&self) -> String {
        if !self.needs_restore() {
            return String::new();
        }

        if !self.new_windows.is_empty() {
            // Group by app name (stable order), join titles per app.
            let mut by_app: std::collections::BTreeMap<&str, Vec<&str>> =
                std::collections::BTreeMap::new();
            for w in &self.new_windows {
                by_app.entry(&w.app_name).or_default().push(&w.title);
            }
            let summaries: Vec<String> = by_app
                .into_iter()
                .map(|(app, titles)| {
                    let titles: Vec<String> = titles
                        .into_iter()
                        .filter(|t| !t.is_empty())
                        .map(|t| format!("\"{t}\""))
                        .collect();
                    if titles.is_empty() {
                        app.to_string()
                    } else {
                        format!("{app} ({})", titles.join(", "))
                    }
                })
                .collect();
            format!(
                "\n\n🪟 Action opened new window(s): {}.",
                summaries.join("; ")
            )
        } else {
            "\n\n🔀 Action caused a different app to become frontmost.".to_string()
        }
    }
}

/// Default poll deadline — new windows triggered by a click typically
/// appear within ~200ms on macOS; 1.0s gives the wildcard suppressor
/// time to fire and settle.
const DEFAULT_TIMEOUT: Duration = Duration::from_millis(1000);

/// Default inter-poll interval. Matches Swift's 50ms.
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Public API. Mirrors Swift `enum WindowChangeDetector` — no state of
/// its own; all state lives inside the returned `Snapshot`.
pub struct WindowChangeDetector;

impl WindowChangeDetector {
    /// Capture the current window set + frontmost pid and arm the
    /// wildcard focus-steal suppressor. Call immediately before
    /// dispatching the action.
    ///
    /// `prior_front` is the frontmost pid the **caller** already
    /// observed — typically captured one line earlier via
    /// `apps::frontmost_pid()` for the surrounding `focus_guard`
    /// lease. We use the caller's value (not a fresh re-read) so the
    /// wildcard suppressor's `restore_to` matches what the focus-guard
    /// lease saw; a race where another app became frontmost between
    /// the caller's read and this method would otherwise leave the
    /// two leases targeting different pids.
    ///
    /// Returns `Snapshot`. Drop ends suppression (via the held
    /// `SuppressionLease`); call `Snapshot::detect()` to consume the
    /// snapshot and get a `Changes` summary.
    ///
    /// Safe to call from any thread — `CGWindowListCopyWindowInfo` is
    /// documented as thread-safe.
    pub fn snapshot(prior_front: Option<i32>) -> Snapshot {
        let window_ids: HashSet<u32> = windows::visible_windows()
            .into_iter()
            .filter(|w| w.layer == 0)
            .map(|w| w.window_id)
            .collect();

        // Arm wildcard suppression — covers snapshot → detect window.
        // restore_to = caller-captured frontmost; target = wildcard
        // (any other pid). If there's no frontmost (rare — screensaver,
        // login window), we skip the lease; foreground-change tracking
        // still runs.
        let lease = prior_front.map(|restore_to| {
            focus_steal::begin_suppression(
                None, // wildcard
                restore_to,
                "WindowChangeDetector.snapshot",
            )
        });

        Snapshot {
            window_ids,
            front_pid: prior_front,
            _lease: lease,
        }
    }
}

impl Snapshot {
    /// Frontmost pid at snapshot time, if any.
    pub fn front_pid(&self) -> Option<i32> {
        self.front_pid
    }

    /// Poll for up to `DEFAULT_TIMEOUT` for new windows or a
    /// foreground-app change. Returns as soon as a change is detected
    /// or the timeout elapses.
    ///
    /// Consumes the snapshot — the wildcard suppression lease is
    /// dropped when this returns (covers the full action + detection
    /// window).
    pub fn detect(self) -> Changes {
        self.detect_with(DEFAULT_TIMEOUT, DEFAULT_POLL_INTERVAL)
    }

    /// Async wrapper around `detect()` — runs the synchronous poll
    /// loop on a `spawn_blocking` thread so it doesn't stall the
    /// tokio runtime. Most action-tool call sites should prefer this
    /// over the blocking `detect()`.
    pub async fn detect_async(self) -> Changes {
        // Move the Snapshot (and its embedded lease) onto the blocking
        // thread; the lease's Drop runs there when detect_with returns.
        tokio::task::spawn_blocking(move || self.detect())
            .await
            .unwrap_or_else(|_| Changes::no_change())
    }

    /// Same as `detect()` but with configurable timing — exposed for
    /// tests / callers that want a tighter or looser poll window.
    pub fn detect_with(self, timeout: Duration, poll_interval: Duration) -> Changes {
        let deadline = Instant::now() + timeout;
        loop {
            std::thread::sleep(poll_interval);

            let current: Vec<WindowInfo> = windows::visible_windows()
                .into_iter()
                .filter(|w| w.layer == 0)
                .collect();
            let current_ids: HashSet<u32> = current.iter().map(|w| w.window_id).collect();

            let new_windows: Vec<WindowEvent> = current
                .iter()
                .filter(|w| !self.window_ids.contains(&w.window_id))
                .map(|w| WindowEvent {
                    window_id: w.window_id,
                    pid: w.pid,
                    app_name: w.app_name.clone(),
                    title: w.title.clone(),
                })
                .collect();
            // Diff the other direction too — keeps unit tests honest
            // even though Swift's result_suffix only uses opened windows.
            let _closed: Vec<u32> = self
                .window_ids
                .iter()
                .copied()
                .filter(|id| !current_ids.contains(id))
                .collect();

            let current_front = apps::frontmost_pid();
            let foreground_changed = match (self.front_pid, current_front) {
                (Some(orig), Some(cur)) => orig != cur,
                _ => false,
            };

            if !new_windows.is_empty() || foreground_changed {
                return Changes {
                    new_windows,
                    foreground_changed,
                };
            }
            if Instant::now() >= deadline {
                return Changes::no_change();
            }
        }
    }

    // ── Internal helpers — also used by unit tests via the `pub(super)`
    // path so the diff logic can be exercised without driving the live
    // window enumerator. ────────────────────────────────────────────

    /// Pure-function diff: given the snapshot's window-id set + a
    /// list of currently-visible windows, return the (opened, closed)
    /// classification.
    pub(crate) fn diff(
        snapshot_ids: &HashSet<u32>,
        current: &[WindowInfo],
    ) -> (Vec<WindowEvent>, Vec<u32>) {
        let current_ids: HashSet<u32> = current.iter().map(|w| w.window_id).collect();
        let opened: Vec<WindowEvent> = current
            .iter()
            .filter(|w| !snapshot_ids.contains(&w.window_id))
            .map(|w| WindowEvent {
                window_id: w.window_id,
                pid: w.pid,
                app_name: w.app_name.clone(),
                title: w.title.clone(),
            })
            .collect();
        let closed: Vec<u32> = snapshot_ids
            .iter()
            .copied()
            .filter(|id| !current_ids.contains(id))
            .collect();
        (opened, closed)
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::windows::WindowBounds;

    fn win(id: u32, pid: i32, app: &str, title: &str) -> WindowInfo {
        WindowInfo {
            window_id: id,
            pid,
            app_name: app.into(),
            title: title.into(),
            bounds: WindowBounds {
                x: 0.,
                y: 0.,
                width: 100.,
                height: 100.,
            },
            layer: 0,
            z_index: 0,
            is_on_screen: true,
            on_current_space: None,
            space_ids: None,
        }
    }

    #[test]
    fn diff_finds_opened_window() {
        let snap: HashSet<u32> = [1, 2].into_iter().collect();
        let cur = vec![
            win(1, 100, "Safari", "Home"),
            win(2, 100, "Safari", "Tab2"),
            win(3, 101, "Mail", "Inbox"),
        ];
        let (opened, closed) = Snapshot::diff(&snap, &cur);
        assert_eq!(opened.len(), 1);
        assert_eq!(opened[0].window_id, 3);
        assert_eq!(opened[0].app_name, "Mail");
        assert_eq!(opened[0].title, "Inbox");
        assert!(closed.is_empty());
    }

    #[test]
    fn diff_finds_closed_window() {
        let snap: HashSet<u32> = [1, 2, 3].into_iter().collect();
        let cur = vec![win(1, 100, "Safari", "Home")];
        let (opened, closed) = Snapshot::diff(&snap, &cur);
        assert!(opened.is_empty());
        assert_eq!(closed.len(), 2);
        let closed_set: HashSet<u32> = closed.into_iter().collect();
        assert!(closed_set.contains(&2));
        assert!(closed_set.contains(&3));
    }

    #[test]
    fn diff_no_change() {
        let snap: HashSet<u32> = [1, 2].into_iter().collect();
        let cur = vec![win(1, 100, "Safari", "A"), win(2, 100, "Safari", "B")];
        let (opened, closed) = Snapshot::diff(&snap, &cur);
        assert!(opened.is_empty());
        assert!(closed.is_empty());
    }

    #[test]
    fn changes_result_suffix_no_change_is_empty() {
        let c = Changes::no_change();
        assert_eq!(c.result_suffix(), "");
        assert!(!c.needs_restore());
    }

    #[test]
    fn changes_result_suffix_single_new_window_with_title() {
        let c = Changes {
            new_windows: vec![WindowEvent {
                window_id: 99,
                pid: 100,
                app_name: "Chrome".into(),
                title: "New Tab".into(),
            }],
            foreground_changed: false,
        };
        assert!(c.needs_restore());
        assert_eq!(
            c.result_suffix(),
            "\n\n🪟 Action opened new window(s): Chrome (\"New Tab\")."
        );
    }

    #[test]
    fn changes_result_suffix_groups_windows_by_app() {
        let c = Changes {
            new_windows: vec![
                WindowEvent {
                    window_id: 1,
                    pid: 100,
                    app_name: "Chrome".into(),
                    title: "Tab A".into(),
                },
                WindowEvent {
                    window_id: 2,
                    pid: 100,
                    app_name: "Chrome".into(),
                    title: "Tab B".into(),
                },
                WindowEvent {
                    window_id: 3,
                    pid: 101,
                    app_name: "Mail".into(),
                    title: "".into(),
                },
            ],
            foreground_changed: true,
        };
        let suffix = c.result_suffix();
        // BTreeMap sort order is alphabetical by app name → Chrome before Mail.
        assert_eq!(
            suffix,
            "\n\n🪟 Action opened new window(s): Chrome (\"Tab A\", \"Tab B\"); Mail."
        );
    }

    #[test]
    fn changes_result_suffix_foreground_change_only() {
        let c = Changes {
            new_windows: vec![],
            foreground_changed: true,
        };
        assert!(c.needs_restore());
        assert_eq!(
            c.result_suffix(),
            "\n\n🔀 Action caused a different app to become frontmost."
        );
    }

    #[test]
    fn changes_result_suffix_empty_title_is_dropped() {
        let c = Changes {
            new_windows: vec![WindowEvent {
                window_id: 1,
                pid: 100,
                app_name: "Finder".into(),
                title: "".into(),
            }],
            foreground_changed: false,
        };
        // No title → just the app name, no parentheses.
        assert_eq!(c.result_suffix(), "\n\n🪟 Action opened new window(s): Finder.");
    }

    /// Regression: `snapshot(prior_front)` must store the caller's
    /// captured front pid verbatim (rather than re-reading it inside
    /// the function and racing with concurrent activations).
    #[test]
    fn snapshot_stores_caller_prior_front() {
        // Use an obviously bogus pid so we'd notice if the impl silently
        // fell back to the live frontmost on this test runner.
        let bogus_prior = Some(424242_i32);
        let snap = WindowChangeDetector::snapshot(bogus_prior);
        assert_eq!(snap.front_pid(), bogus_prior);

        // None must round-trip too — and must skip the lease without
        // panicking (no frontmost to restore to).
        let snap_none = WindowChangeDetector::snapshot(None);
        assert_eq!(snap_none.front_pid(), None);
    }
}

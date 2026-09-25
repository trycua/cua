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

use cua_driver_core::window_observation::WindowObservationBounds;

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
    /// Whether the post-action window poll ran. `false` when the host bound
    /// skipped it or the poll task was lost: an empty `new_windows` then means
    /// nothing was watched, not that nothing opened.
    pub polled: bool,
}

impl Changes {
    pub fn no_change() -> Self {
        Self {
            new_windows: Vec::new(),
            foreground_changed: false,
            polled: true,
        }
    }

    pub fn not_polled() -> Self {
        Self {
            polled: false,
            ..Self::no_change()
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

/// Resolve the post-action observation bounds from raw host values against
/// the macOS defaults. Pure; `cua_driver_core::window_observation` owns the
/// parsing and clamping rules shared with the Linux adapter.
fn observation_bounds_from(
    timeout_raw: Option<&str>,
    poll_raw: Option<&str>,
) -> WindowObservationBounds {
    WindowObservationBounds::from_raw(
        timeout_raw,
        poll_raw,
        DEFAULT_TIMEOUT,
        DEFAULT_POLL_INTERVAL,
    )
}

/// Post-action observation bounds chosen by the embedding host through
/// `CUA_DRIVER_WINDOW_CHANGE_TIMEOUT_MS` / `CUA_DRIVER_WINDOW_CHANGE_POLL_MS`
/// on the daemon environment; `DEFAULT_TIMEOUT` / `DEFAULT_POLL_INTERVAL`
/// when unset or unparsable.
pub(crate) fn host_observation_bounds() -> WindowObservationBounds {
    WindowObservationBounds::from_env(DEFAULT_TIMEOUT, DEFAULT_POLL_INTERVAL)
}

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
        Self::capture(prior_front, true, None)
    }

    /// Capture the same before-state without arming reactive focus suppression.
    /// Foreground delivery owns its temporary activation and restoration, so a
    /// wildcard lease would race the target while the action is settling.
    pub fn snapshot_without_suppression(prior_front: Option<i32>) -> Snapshot {
        Self::capture(prior_front, false, None)
    }

    /// Capture the before-state and suppress cross-app activations while
    /// allowing one intentional target activation.
    ///
    /// The raw background pixel-click path needs this middle ground:
    /// focus-without-raise makes `allowed_pid` AppKit-active so its event queue
    /// accepts the click, but a link or hand-off that activates a different app
    /// must still restore the user's original foreground.
    pub fn snapshot_allowing_activation(prior_front: Option<i32>, allowed_pid: i32) -> Snapshot {
        Self::capture(prior_front, true, Some(allowed_pid))
    }

    fn capture(
        prior_front: Option<i32>,
        suppress_focus: bool,
        allowed_pid: Option<i32>,
    ) -> Snapshot {
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
        let lease = prior_front
            .filter(|_| suppress_focus)
            .map(|restore_to| match allowed_pid {
                Some(pid) => focus_steal::begin_suppression_allowing(
                    pid,
                    restore_to,
                    "WindowChangeDetector.snapshot_allowing_activation",
                ),
                None => focus_steal::begin_suppression(
                    None, // wildcard
                    restore_to,
                    "WindowChangeDetector.snapshot",
                ),
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
    /// An embedding host that already observes the target continuously
    /// can bound this window through `CUA_DRIVER_WINDOW_CHANGE_TIMEOUT_MS` /
    /// `CUA_DRIVER_WINDOW_CHANGE_POLL_MS` on the daemon's environment.
    /// Unset or unparsable values keep the defaults, so public callers see
    /// no behavior change.
    ///
    /// Consumes the snapshot — the wildcard suppression lease is
    /// dropped when this returns (covers the full action + detection
    /// window). A shorter timeout therefore also shortens the wildcard
    /// focus-steal protection, and a zero timeout releases it as soon as
    /// the action returns and reports no change.
    pub fn detect(self) -> Changes {
        self.detect_bounded(host_observation_bounds())
    }

    /// `detect()` with explicit bounds. A zero timeout drops the snapshot
    /// (and its wildcard suppression lease) immediately without reading
    /// the window list again.
    pub(crate) fn detect_bounded(self, bounds: WindowObservationBounds) -> Changes {
        if bounds.skips_observation() {
            drop(self);
            return Changes::not_polled();
        }
        self.detect_with(bounds.timeout, bounds.poll)
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
            .unwrap_or_else(|_| Changes::not_polled())
    }

    /// Same as `detect()` but with configurable timing.
    fn detect_with(self, timeout: Duration, poll_interval: Duration) -> Changes {
        let deadline = Instant::now() + timeout;
        loop {
            std::thread::sleep(poll_interval);

            let current: Vec<WindowInfo> = windows::visible_windows()
                .into_iter()
                .filter(|w| w.layer == 0)
                .collect();

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
            let current_front = apps::frontmost_pid();
            let foreground_changed = match (self.front_pid, current_front) {
                (Some(orig), Some(cur)) => orig != cur,
                _ => false,
            };

            if !new_windows.is_empty() || foreground_changed {
                return Changes {
                    new_windows,
                    foreground_changed,
                    polled: true,
                };
            }
            if Instant::now() >= deadline {
                return Changes::no_change();
            }
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_result_suffix_no_change_is_empty() {
        let c = Changes::no_change();
        assert_eq!(c.result_suffix(), "");
        assert!(!c.needs_restore());
    }

    #[test]
    fn changes_result_suffix_single_new_window_with_title() {
        let c = Changes {
            polled: true,
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
            polled: true,
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
            polled: true,
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
            polled: true,
            new_windows: vec![WindowEvent {
                window_id: 1,
                pid: 100,
                app_name: "Finder".into(),
                title: "".into(),
            }],
            foreground_changed: false,
        };
        // No title → just the app name, no parentheses.
        assert_eq!(
            c.result_suffix(),
            "\n\n🪟 Action opened new window(s): Finder."
        );
    }

    #[test]
    fn host_bounds_unset_keep_macos_defaults() {
        let b = observation_bounds_from(None, None);
        assert_eq!(b.timeout, DEFAULT_TIMEOUT);
        assert_eq!(b.poll, DEFAULT_POLL_INTERVAL);
        assert!(!b.skips_observation());
    }

    #[test]
    fn host_bounds_valid_values_are_honored() {
        let b = observation_bounds_from(Some("200"), Some("20"));
        assert_eq!(b.timeout, Duration::from_millis(200));
        assert_eq!(b.poll, Duration::from_millis(20));
    }

    #[test]
    fn host_bounds_zero_timeout_skips_observation() {
        let b = observation_bounds_from(Some("0"), None);
        assert!(b.skips_observation());
        assert!(!b.poll.is_zero());
    }

    #[test]
    fn host_bounds_invalid_values_keep_macos_defaults() {
        for raw in ["", "abc", "-5", "2.5", "99999999999999999999"] {
            let b = observation_bounds_from(Some(raw), Some(raw));
            assert_eq!(b.timeout, DEFAULT_TIMEOUT, "timeout for {raw:?}");
            assert_eq!(b.poll, DEFAULT_POLL_INTERVAL, "poll for {raw:?}");
        }
    }

    #[test]
    fn host_bounds_too_large_values_are_clamped() {
        use cua_driver_core::window_observation::{
            MAX_WINDOW_CHANGE_POLL, MAX_WINDOW_CHANGE_TIMEOUT,
        };
        let b = observation_bounds_from(Some("3600000"), Some("60000"));
        assert_eq!(b.timeout, MAX_WINDOW_CHANGE_TIMEOUT);
        assert_eq!(b.poll, MAX_WINDOW_CHANGE_POLL);
    }

    /// A zero timeout returns immediately with no change instead of
    /// sleeping a poll interval. Reads the window list once (in
    /// `snapshot`) and sends no input.
    #[test]
    fn zero_timeout_detect_returns_without_polling() {
        let snap = WindowChangeDetector::snapshot(None);
        let started = Instant::now();
        let changes = snap.detect_bounded(observation_bounds_from(Some("0"), None));
        assert!(started.elapsed() < DEFAULT_POLL_INTERVAL);
        assert!(!changes.polled);
        assert!(!changes.needs_restore());
        assert_eq!(changes.result_suffix(), "");
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

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

/// Returns true when a window belongs to this cua-driver process, including
/// transient UI such as the agent cursor overlay. Those windows are internal
/// implementation details rather than action-triggered application windows.
fn is_daemon_window(window: &WindowInfo) -> bool {
    window.pid == std::process::id() as i32
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
#[cfg(test)]
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
        let window_ids: HashSet<u32> = host_windows().into_iter().map(|w| w.window_id).collect();
        Self::capture_from(window_ids, prior_front, suppress_focus, allowed_pid)
    }

    /// `capture` over an already-read window set.
    fn capture_from(
        window_ids: HashSet<u32>,
        prior_front: Option<i32>,
        suppress_focus: bool,
        allowed_pid: Option<i32>,
    ) -> Snapshot {
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
        self.detect_bounded_with(bounds, observe_host)
    }

    /// `detect_bounded` over an explicit observer of the current layer-0
    /// windows and frontmost pid.
    fn detect_bounded_with(
        self,
        bounds: WindowObservationBounds,
        observe: impl FnMut() -> (Vec<WindowInfo>, Option<i32>),
    ) -> Changes {
        if bounds.skips_observation() {
            drop(self);
            return Changes::not_polled();
        }
        self.detect_with(bounds.timeout, bounds.poll, observe)
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
    fn detect_with(
        self,
        timeout: Duration,
        poll_interval: Duration,
        mut observe: impl FnMut() -> (Vec<WindowInfo>, Option<i32>),
    ) -> Changes {
        let deadline = Instant::now() + timeout;
        loop {
            std::thread::sleep(poll_interval);

            let (current, current_front) = observe();
            // Keep the live detector and the pure regression tests on the same
            // diff path so daemon-window filtering cannot drift between them.
            let (new_windows, _closed) = Self::diff(&self.window_ids, &current);

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

    // ── Internal helpers — also used by unit tests so the diff logic can be
    // exercised without driving the live window enumerator. ──────────────

    /// Pure-function diff: given the snapshot's window-id set + a
    /// list of currently-visible windows, return the (opened, closed)
    /// classification. Opened windows owned by this daemon are excluded so
    /// transient UI such as the cursor overlay is not reported as an action
    /// side effect.
    pub(crate) fn diff(
        snapshot_ids: &HashSet<u32>,
        current: &[WindowInfo],
    ) -> (Vec<WindowEvent>, Vec<u32>) {
        let current_ids: HashSet<u32> = current.iter().map(|w| w.window_id).collect();
        let opened: Vec<WindowEvent> = current
            .iter()
            .filter(|w| !snapshot_ids.contains(&w.window_id))
            .filter(|w| !is_daemon_window(w))
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

/// The host's visible layer-0 windows.
fn host_windows() -> Vec<WindowInfo> {
    windows::visible_windows()
        .into_iter()
        .filter(|w| w.layer == 0)
        .collect()
}

/// One live observation for the poll loop: layer-0 windows and the frontmost pid.
fn observe_host() -> (Vec<WindowInfo>, Option<i32>) {
    (host_windows(), apps::frontmost_pid())
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::windows::WindowBounds;

    fn win(window_id: u32, pid: i32, app_name: &str, title: &str) -> WindowInfo {
        WindowInfo {
            window_id,
            pid,
            app_name: app_name.to_owned(),
            title: title.to_owned(),
            bounds: WindowBounds {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 100.0,
            },
            layer: 0,
            z_index: 0,
            is_on_screen: true,
            current_space_id: None,
            on_current_space: None,
            space_ids: None,
        }
    }

    /// Regression for trycua/cua#1592 Bug 2. This exercises the same `diff`
    /// path used by `detect_with`, rather than separately testing a predicate
    /// that production could accidentally stop applying.
    #[test]
    fn diff_excludes_new_windows_owned_by_the_daemon() {
        let snap: HashSet<u32> = [1].into_iter().collect();
        let daemon_pid = std::process::id() as i32;
        let cur = vec![
            win(1, daemon_pid + 1, "Safari", "Home"),
            win(2, daemon_pid, "Cua Driver", ""),
            win(3, daemon_pid + 2, "Mail", "Inbox"),
        ];

        let (opened, closed) = Snapshot::diff(&snap, &cur);

        assert_eq!(opened.len(), 1);
        assert_eq!(opened[0].window_id, 3);
        assert_eq!(opened[0].app_name, "Mail");
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
    /// sleeping a poll interval, and never observes the host again.
    #[test]
    fn zero_timeout_detect_returns_without_polling() {
        let snap = WindowChangeDetector::capture_from(HashSet::new(), None, false, None);
        let started = Instant::now();
        let changes = snap.detect_bounded_with(observation_bounds_from(Some("0"), None), || {
            panic!("a zero timeout must not observe the host")
        });
        assert!(started.elapsed() < DEFAULT_POLL_INTERVAL);
        assert!(!changes.polled);
        assert!(!changes.needs_restore());
        assert_eq!(changes.result_suffix(), "");
    }

    /// The other half of the pair above: a poll that ran is `polled` whether
    /// or not anything opened before its deadline, so a skipped poll never
    /// reads as a quiet one.
    #[test]
    fn a_poll_that_ran_is_polled_even_when_it_times_out() {
        let snap = WindowChangeDetector::capture_from(HashSet::from([1]), Some(7), false, None);
        let mut observations = 0;
        let changes =
            snap.detect_bounded_with(observation_bounds_from(Some("30"), Some("10")), || {
                observations += 1;
                (vec![win(1, 7, "App", "Main")], Some(7))
            });
        assert!(changes.polled);
        assert!(observations >= 1);
        assert!(changes.new_windows.is_empty());
        assert!(!changes.foreground_changed);
    }

    /// A poll reports a newly opened window and a foreground change as soon
    /// as it observes them.
    #[test]
    fn a_poll_reports_new_windows_and_foreground_changes() {
        let snap = WindowChangeDetector::capture_from(HashSet::from([1]), Some(7), false, None);
        let changes =
            snap.detect_bounded_with(observation_bounds_from(Some("1000"), Some("1")), || {
                (
                    vec![win(1, 7, "App", "Main"), win(2, 8, "Other", "Popup")],
                    Some(8),
                )
            });
        assert!(changes.polled);
        assert!(changes.foreground_changed);
        assert_eq!(
            changes
                .new_windows
                .iter()
                .map(|w| w.window_id)
                .collect::<Vec<_>>(),
            vec![2]
        );
    }

    /// Regression: the snapshot must store the caller's captured front pid
    /// verbatim (rather than re-reading it inside the function and racing
    /// with concurrent activations).
    #[test]
    fn snapshot_stores_caller_prior_front() {
        // Use an obviously bogus pid so we'd notice if the impl silently
        // fell back to a live frontmost read.
        let bogus_prior = Some(424242_i32);
        let snap = WindowChangeDetector::capture_from(HashSet::new(), bogus_prior, false, None);
        assert_eq!(snap.front_pid(), bogus_prior);

        let snap_none = WindowChangeDetector::capture_from(HashSet::new(), None, true, None);
        assert_eq!(snap_none.front_pid(), None);
        assert!(
            snap_none._lease.is_none(),
            "no frontmost pid means no lease"
        );
    }
}

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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
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
    /// A change the previous action's detached tail saw after that action
    /// had returned; reported on this action's result.
    earlier: Option<Late>,
}

/// Result of `detect()` — what changed during the action window.
#[derive(Debug, Clone)]
pub struct Changes {
    pub new_windows: Vec<WindowEvent>,
    pub foreground_changed: bool,
    /// Windows the previous action opened after its result had returned.
    pub earlier_new_windows: Vec<WindowEvent>,
    /// The previous action made another app frontmost after it returned.
    pub earlier_foreground_changed: bool,
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
            earlier_new_windows: Vec::new(),
            earlier_foreground_changed: false,
            polled: true,
        }
    }

    fn with_earlier(mut self, earlier: Option<Late>) -> Self {
        if let Some(late) = earlier {
            self.earlier_new_windows = late.new_windows;
            self.earlier_foreground_changed = late.foreground_changed;
        }
        self
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
        let mut suffix = String::new();
        if !self.earlier_new_windows.is_empty() {
            suffix.push_str(&format!(
                "\n\n🪟 After the previous action returned, it opened new window(s): {}.",
                window_summaries(&self.earlier_new_windows)
            ));
        } else if self.earlier_foreground_changed {
            suffix.push_str(
                "\n\n🔀 After the previous action returned, a different app became frontmost.",
            );
        }
        if !self.needs_restore() {
            return suffix;
        }

        if !self.new_windows.is_empty() {
            suffix.push_str(&format!(
                "\n\n🪟 Action opened new window(s): {}.",
                window_summaries(&self.new_windows)
            ));
        } else {
            suffix.push_str("\n\n🔀 Action caused a different app to become frontmost.");
        }
        suffix
    }
}

/// `App ("Title", …); Other` for a result suffix, grouped by app name.
fn window_summaries(windows: &[WindowEvent]) -> String {
    let mut by_app: std::collections::BTreeMap<&str, Vec<&str>> = std::collections::BTreeMap::new();
    for w in windows {
        by_app.entry(&w.app_name).or_default().push(&w.title);
    }
    by_app
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
        .collect::<Vec<_>>()
        .join("; ")
}

/// Returns true when a window belongs to this cua-driver process, including
/// transient UI such as the agent cursor overlay. Those windows are internal
/// implementation details rather than action-triggered application windows.
fn is_daemon_window(window: &WindowInfo) -> bool {
    window.pid == std::process::id() as i32
}

/// Default observation deadline. New windows triggered by a click
/// typically appear within ~200ms on macOS; 1.0s gives the wildcard
/// suppressor time to fire and settle.
const DEFAULT_TIMEOUT: Duration = Duration::from_millis(1000);

/// Default inter-poll interval.
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(25);

/// How long the action result itself waits for a window change. Menus,
/// popovers and sheets map well inside this. When nothing has changed by
/// then the result returns, and the rest of the observation deadline runs
/// detached (see [`Tail`]): the focus-steal lease stays armed and a late
/// window is reported on the next action's result. Before this, every
/// action that opened nothing paid the whole deadline.
const REPORT_WINDOW: Duration = Duration::from_millis(300);

/// A late change older than this is not reported on a later result.
const LATE_REPORT_MAX_AGE: Duration = Duration::from_secs(10);

/// The detached remainder of the last quiet observation: its focus-steal
/// lease, held until the deadline, until a change shows, or until the driver
/// starts its next action or intentional activation ([`end_tail`]).
struct Tail {
    id: u64,
    _lease: Option<SuppressionLease>,
}

static TAIL: Mutex<Option<Tail>> = Mutex::new(None);
static NEXT_TAIL_ID: AtomicU64 = AtomicU64::new(1);

/// A change a detached tail saw after its action had returned.
#[derive(Debug, Clone)]
struct Late {
    at: Instant,
    new_windows: Vec<WindowEvent>,
    foreground_changed: bool,
}

static LATE: Mutex<Option<Late>> = Mutex::new(None);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// End the detached observation of the previous action, releasing its
/// focus-steal lease now. Called before the driver's next action and before
/// any intentional activation, so a lingering wildcard lease never reverts
/// an activation the driver itself asked for.
pub fn end_tail() {
    let tail = lock(&TAIL).take();
    drop(tail);
}

/// Take a late change recorded by a detached tail, if it is recent.
fn take_late() -> Option<Late> {
    lock(&LATE)
        .take()
        .filter(|late| late.at.elapsed() <= LATE_REPORT_MAX_AGE)
}

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
        // A new action supersedes the previous action's detached observation.
        end_tail();
        let window_ids: HashSet<u32> = host_windows().into_iter().map(|w| w.window_id).collect();
        let mut snapshot = Self::capture_from(window_ids, prior_front, suppress_focus, allowed_pid);
        snapshot.earlier = take_late();
        snapshot
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
            earlier: None,
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
    /// The result waits at most [`REPORT_WINDOW`] when nothing changes; the
    /// rest of the deadline runs detached with the wildcard suppression lease
    /// still held, until the driver's next action or intentional activation
    /// ends it ([`end_tail`]). A window that shows during that remainder is
    /// reported on the next action's result. A shorter timeout therefore also
    /// shortens the wildcard focus-steal protection, and a zero timeout
    /// releases it as soon as the action returns and reports no change.
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
        mut self,
        bounds: WindowObservationBounds,
        observe: impl FnMut() -> (Vec<WindowInfo>, Option<i32>) + Send + 'static,
    ) -> Changes {
        let earlier = self.earlier.take();
        if bounds.skips_observation() {
            drop(self);
            return Changes::not_polled().with_earlier(earlier);
        }
        let started = Instant::now();
        let report = REPORT_WINDOW.min(bounds.timeout);
        let mut observe = observe;
        let changes = match self.poll_until(started + report, bounds.poll, &mut observe) {
            Ok(changes) => changes,
            Err(quiet) => {
                quiet.continue_detached(started + bounds.timeout, bounds.poll, observe);
                Changes::no_change()
            }
        };
        changes.with_earlier(earlier)
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

    /// One observation against the snapshot: the change it shows, if any.
    fn observe_once(
        &self,
        observe: &mut impl FnMut() -> (Vec<WindowInfo>, Option<i32>),
    ) -> Option<Changes> {
        let (current, current_front) = observe();
        // Keep the live detector and the pure regression tests on the same
        // diff path so daemon-window filtering cannot drift between them.
        let (new_windows, _closed) = Self::diff(&self.window_ids, &current);
        let foreground_changed = match (self.front_pid, current_front) {
            (Some(orig), Some(cur)) => orig != cur,
            _ => false,
        };
        (!new_windows.is_empty() || foreground_changed).then(|| Changes {
            new_windows,
            foreground_changed,
            ..Changes::no_change()
        })
    }

    /// Poll until a change shows (`Ok`, the lease is dropped) or `deadline`
    /// passes with none (`Err`, the snapshot and its lease handed back).
    fn poll_until(
        self,
        deadline: Instant,
        poll_interval: Duration,
        observe: &mut impl FnMut() -> (Vec<WindowInfo>, Option<i32>),
    ) -> Result<Changes, Self> {
        loop {
            std::thread::sleep(poll_interval);
            if let Some(changes) = self.observe_once(observe) {
                return Ok(changes);
            }
            if Instant::now() >= deadline {
                return Err(self);
            }
        }
    }

    /// Keep observing on a background thread until `deadline`, holding the
    /// focus-steal lease, after the action's result has returned. A change
    /// seen here is recorded for the next action's result. [`end_tail`] (the
    /// next action, or an intentional activation) ends it early.
    fn continue_detached(
        mut self,
        deadline: Instant,
        poll_interval: Duration,
        mut observe: impl FnMut() -> (Vec<WindowInfo>, Option<i32>) + Send + 'static,
    ) {
        if Instant::now() >= deadline {
            return;
        }
        let id = NEXT_TAIL_ID.fetch_add(1, Ordering::Relaxed);
        *lock(&TAIL) = Some(Tail {
            id,
            _lease: self._lease.take(),
        });
        let still_mine = move || lock(&TAIL).as_ref().is_some_and(|tail| tail.id == id);
        let spawned = std::thread::Builder::new()
            .name("cua-window-watch".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(poll_interval);
                    if !still_mine() {
                        return;
                    }
                    if let Some(changes) = self.observe_once(&mut observe) {
                        if !still_mine() {
                            return;
                        }
                        *lock(&LATE) = Some(Late {
                            at: Instant::now(),
                            new_windows: changes.new_windows,
                            foreground_changed: changes.foreground_changed,
                        });
                        // Keep the lease until the deadline: the change may be
                        // an activation it is still reverting.
                        break;
                    }
                    if Instant::now() >= deadline {
                        break;
                    }
                }
                let remaining = deadline.saturating_duration_since(Instant::now());
                if !remaining.is_zero() {
                    std::thread::sleep(remaining);
                }
                let mut tail = lock(&TAIL);
                if tail.as_ref().is_some_and(|tail| tail.id == id) {
                    tail.take();
                }
            });
        if spawned.is_err() {
            // No thread: release the lease now rather than holding it forever.
            end_tail();
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
            ..Changes::no_change()
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
            ..Changes::no_change()
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
            ..Changes::no_change()
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
            ..Changes::no_change()
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
        let observations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = observations.clone();
        let changes =
            snap.detect_bounded_with(observation_bounds_from(Some("30"), Some("10")), move || {
                counter.fetch_add(1, Ordering::SeqCst);
                (vec![win(1, 7, "App", "Main")], Some(7))
            });
        assert!(changes.polled);
        assert!(observations.load(Ordering::SeqCst) >= 1);
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

    /// The tail state is process-wide; tests that touch it run one at a time.
    static TAIL_TEST_LOCK: Mutex<()> = Mutex::new(());

    /// A quiet action returns after the report window, not the whole
    /// deadline, and the remainder keeps running detached.
    #[test]
    fn a_quiet_poll_returns_after_the_report_window_and_detaches_the_rest() {
        let _serial = lock(&TAIL_TEST_LOCK);
        end_tail();
        let snap = WindowChangeDetector::capture_from(HashSet::from([1]), Some(7), false, None);
        let started = Instant::now();
        let changes = snap
            .detect_bounded_with(observation_bounds_from(Some("3000"), Some("10")), || {
                (vec![win(1, 7, "App", "Main")], Some(7))
            });
        let elapsed = started.elapsed();
        assert!(changes.polled);
        assert!(!changes.needs_restore());
        assert!(elapsed >= REPORT_WINDOW, "returned after {elapsed:?}");
        assert!(
            elapsed < Duration::from_millis(1500),
            "returned after {elapsed:?}"
        );
        assert!(lock(&TAIL).is_some(), "the remainder runs detached");
        end_tail();
        assert!(lock(&TAIL).is_none());
    }

    /// A deadline inside the report window behaves as before: no tail.
    #[test]
    fn a_short_deadline_leaves_no_detached_tail() {
        let _serial = lock(&TAIL_TEST_LOCK);
        end_tail();
        let snap = WindowChangeDetector::capture_from(HashSet::from([1]), Some(7), false, None);
        let changes = snap
            .detect_bounded_with(observation_bounds_from(Some("40"), Some("10")), || {
                (vec![win(1, 7, "App", "Main")], Some(7))
            });
        assert!(!changes.needs_restore());
        assert!(lock(&TAIL).is_none());
    }

    /// A window that opens after the result returned is reported on the next
    /// action's result, then not again.
    #[test]
    fn a_late_window_is_reported_on_the_next_result() {
        let _serial = lock(&TAIL_TEST_LOCK);
        end_tail();
        lock(&LATE).take();
        let opened_at = Instant::now() + REPORT_WINDOW + Duration::from_millis(60);
        let snap = WindowChangeDetector::capture_from(HashSet::from([1]), Some(7), false, None);
        let first = snap.detect_bounded_with(
            observation_bounds_from(Some("2000"), Some("10")),
            move || {
                if Instant::now() >= opened_at {
                    (
                        vec![win(1, 7, "App", "Main"), win(2, 7, "App", "Save")],
                        Some(7),
                    )
                } else {
                    (vec![win(1, 7, "App", "Main")], Some(7))
                }
            },
        );
        assert!(!first.needs_restore());
        let wait_until = Instant::now() + Duration::from_secs(2);
        while lock(&LATE).is_none() && Instant::now() < wait_until {
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut next =
            WindowChangeDetector::capture_from(HashSet::from([1, 2]), Some(7), false, None);
        end_tail();
        next.earlier = take_late();
        let second = next.detect_bounded_with(observation_bounds_from(Some("0"), None), || {
            panic!("a zero timeout must not observe the host")
        });
        assert_eq!(
            second.result_suffix(),
            "\n\n🪟 After the previous action returned, it opened new window(s): App (\"Save\")."
        );
        assert!(take_late().is_none(), "reported once");
    }

    /// The next action ends the previous tail before it acts, so a lingering
    /// wildcard lease cannot revert an activation the driver asked for.
    #[test]
    fn end_tail_stops_the_detached_observation() {
        let _serial = lock(&TAIL_TEST_LOCK);
        end_tail();
        lock(&LATE).take();
        let snap = WindowChangeDetector::capture_from(HashSet::from([1]), Some(7), false, None);
        let observations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = observations.clone();
        let _ = snap.detect_bounded_with(
            observation_bounds_from(Some("3000"), Some("10")),
            move || {
                counter.fetch_add(1, Ordering::SeqCst);
                (vec![win(1, 7, "App", "Main")], Some(7))
            },
        );
        end_tail();
        std::thread::sleep(Duration::from_millis(40));
        let after_end = observations.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(80));
        assert_eq!(observations.load(Ordering::SeqCst), after_end);
        assert!(lock(&LATE).is_none());
    }
}

// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Process-lifetime Chromium/Electron accessibility enablement.
//!
//! Chromium-family apps (Arc, VS Code, Electron shells) ship their web-content
//! AX tree OFF and only build it once an assistive client asks for it. The
//! walker flips `AXManualAccessibility` (falling back to
//! `AXEnhancedUserInterface` only when the modern attribute is unsupported and
//! the app is Chromium or Electron —
//! see [`super::bindings::enable_chromium_accessibility`]) and waits for the
//! asynchronously-built tree to appear before it is read.
//!
//! It also brings a hidden or covered Chromium window's tree up to date before
//! a read ([`flush_pending_web_updates`]).
//!
//! The "already enabled" cache is keyed by the observed process lifetime
//! (pid + kernel start time), not by the numeric pid alone: pids are recycled,
//! and a relaunched Electron app must not inherit a stale "enabled" decision
//! that would skip enablement and return an empty web-content tree.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use core_foundation::base::{CFRelease, CFTypeRef};

use super::bindings::{
    copy_children, copy_element_attr, copy_string_attr, element_at_screen_position,
    element_screen_rect, enable_chromium_accessibility, AXUIElementRef, AccessibilityOptIn,
};

const CHROMIUM_SETTLE_MAX_SECONDS: f64 = 0.5;
const CHROMIUM_SETTLE_POLL_SECONDS: f64 = 0.05;
const MATERIALIZE_TIMEOUT_SECONDS: f64 = 4.0;
const MATERIALIZE_POLL_SECONDS: f64 = 0.1;
const MATERIALIZE_MAX_ATTEMPTS: u32 = 3;
const MATERIALIZE_RETRY_BACKOFF_SECONDS: f64 = 30.0;
const RUN_LOOP_PUMPS_PER_POLL: u32 = 4;
const WEB_AREA_ROLE: &str = "AXWebArea";
const MENU_BAR_ROLE: &str = "AXMenuBar";
const WEB_AREA_MAX_DEPTH: u32 = 10;
const WEB_AREA_PROBE_NODES: u32 = 400;
const WEB_AREA_SETTLE_SAMPLE_CAP: u32 = 128;

type ProcessStartStamp = (u64, u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WebContent {
    Present { nodes: u32 },
    Absent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wait {
    Complete,
    /// Not Chromium or Electron: nothing was enabled and there is no web
    /// content to wait for or flush.
    NotNeeded,
    TimedOut {
        attempts: u32,
        attempted_at: Instant,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProcessEnablement {
    stamp: ProcessStartStamp,
    wait: Wait,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Attempt {
    Skip,
    Run { prior_timeouts: u32 },
}

/// Chromium or Electron: a Chromium-family name or bundle id, or an app bundle
/// that carries `Electron Framework.framework`.
fn is_chromium_family(pid: i32) -> bool {
    crate::browser::platform::is_chromium(
        &crate::apps::get_app_name_for_pid(pid).unwrap_or_default(),
        &crate::apps::bundle_id_for_pid(pid).unwrap_or_default(),
    ) || crate::apps::bundles_electron(pid)
}

/// Kernel start time of a process: `(pbi_start_tvsec, pbi_start_tvusec)`.
/// `None` when the process is gone or proc info is unreadable.
fn process_start_stamp(pid: i32) -> Option<ProcessStartStamp> {
    // SAFETY: proc_pidinfo writes at most `size` bytes into `info` and returns
    // the number of bytes filled (<= size) or <= 0 on failure.
    unsafe {
        let mut info: libc::proc_bsdinfo = std::mem::zeroed();
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let filled = libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            &mut info as *mut _ as *mut libc::c_void,
            size,
        );
        if filled != size {
            return None;
        }
        Some((info.pbi_start_tvsec, info.pbi_start_tvusec))
    }
}

static ENABLEMENT_STATE: LazyLock<Mutex<HashMap<i32, ProcessEnablement>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn next_attempt(
    cached: Option<&ProcessEnablement>,
    observed: Option<ProcessStartStamp>,
    now: Instant,
) -> Attempt {
    let fresh = Attempt::Run { prior_timeouts: 0 };
    let (Some(cached), Some(observed)) = (cached, observed) else {
        return fresh;
    };
    if cached.stamp != observed {
        return fresh;
    }
    match cached.wait {
        Wait::Complete | Wait::NotNeeded => Attempt::Skip,
        Wait::TimedOut {
            attempts,
            attempted_at,
        } => {
            let backoff_elapsed = now.saturating_duration_since(attempted_at)
                >= Duration::from_secs_f64(MATERIALIZE_RETRY_BACKOFF_SECONDS);
            if attempts < MATERIALIZE_MAX_ATTEMPTS && backoff_elapsed {
                Attempt::Run {
                    prior_timeouts: attempts,
                }
            } else {
                Attempt::Skip
            }
        }
    }
}

fn wait_outcome(
    opt_in: AccessibilityOptIn,
    prior_timeouts: u32,
    attempted_at: Instant,
    await_tree: impl FnOnce() -> bool,
) -> Option<Wait> {
    match opt_in {
        AccessibilityOptIn::NotAccepted => None,
        AccessibilityOptIn::NotNeeded => Some(Wait::NotNeeded),
        // Chrome builds its web tree asynchronously after either opt-in, so
        // both wait for it. Returning at once for `AXEnhancedUserInterface`
        // let the first read return browser chrome only (CUA-1766).
        AccessibilityOptIn::EnhancedUserInterface | AccessibilityOptIn::ManualAccessibility => {
            Some(if await_tree() {
                Wait::Complete
            } else {
                Wait::TimedOut {
                    attempts: prior_timeouts + 1,
                    attempted_at,
                }
            })
        }
    }
}

unsafe fn count_web_area_descendants(element: AXUIElementRef, depth: u32, cap: &mut u32) -> u32 {
    if depth == 0 || *cap == 0 {
        return 0;
    }
    let children = copy_children(element);
    let mut total = 0u32;
    for child in children {
        if *cap > 0 {
            *cap -= 1;
            total += 1;
            total += count_web_area_descendants(child, depth - 1, cap);
        }
        CFRelease(child as CFTypeRef);
    }
    total
}

unsafe fn find_populated_web_area(element: AXUIElementRef, depth: u32, visits: &mut u32) -> u32 {
    if depth == 0 {
        return 0;
    }
    let mut populated_nodes = 0u32;
    for child in copy_children(element) {
        if populated_nodes == 0 && *visits > 0 {
            *visits -= 1;
            let role = copy_string_attr(child, "AXRole");
            populated_nodes = match role.as_deref() {
                // Menu items are numerous and never hold web content.
                Some(MENU_BAR_ROLE) => 0,
                // An AXWebArea with no children is a stub: Chrome has not
                // built the page tree yet, so it does not count as content.
                Some(WEB_AREA_ROLE) => {
                    let mut sample_cap = WEB_AREA_SETTLE_SAMPLE_CAP;
                    count_web_area_descendants(child, 4, &mut sample_cap)
                }
                _ => find_populated_web_area(child, depth - 1, visits),
            };
        }
        CFRelease(child as CFTypeRef);
    }
    populated_nodes
}

unsafe fn probe_web_content(app_element: AXUIElementRef) -> WebContent {
    let mut visits = WEB_AREA_PROBE_NODES;
    let nodes = find_populated_web_area(app_element, WEB_AREA_MAX_DEPTH, &mut visits);
    if nodes > 0 {
        WebContent::Present { nodes }
    } else {
        WebContent::Absent
    }
}

fn pump_for(seconds: f64) {
    pump_bounded(seconds, |remaining| {
        crate::permissions::panel::pump_run_loop_briefly(remaining)
    });
}

fn pump_bounded(seconds: f64, mut pump: impl FnMut(f64)) {
    let deadline = Instant::now() + Duration::from_secs_f64(seconds.max(0.0));
    for _ in 0..RUN_LOOP_PUMPS_PER_POLL {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return;
        }
        pump(remaining.as_secs_f64());
    }
    let remaining = deadline.saturating_duration_since(Instant::now());
    if !remaining.is_zero() {
        std::thread::sleep(remaining);
    }
}

fn await_web_content(
    mut probe: impl FnMut() -> WebContent,
    mut reassert: impl FnMut(),
    mut settle: impl FnMut(f64),
) -> bool {
    let steps = (MATERIALIZE_TIMEOUT_SECONDS / MATERIALIZE_POLL_SECONDS).round() as u32;
    let reassert_step = steps / 2;
    let settle_slices = (CHROMIUM_SETTLE_MAX_SECONDS / CHROMIUM_SETTLE_POLL_SECONDS).round() as u32;
    for step in 0..=steps {
        match probe() {
            WebContent::Present { nodes: initial } => {
                reassert();
                // Poll in bounded 50 ms slices up to 500 ms rather than paying
                // a blind 500 ms sleep when the AXWebArea subtree is already
                // populated and stable.
                let mut prev = initial;
                for slice in 0..settle_slices {
                    settle(CHROMIUM_SETTLE_POLL_SECONDS);
                    if slice + 1 == settle_slices {
                        break;
                    }
                    if let WebContent::Present { nodes: current } = probe() {
                        if current > 0 && current == prev {
                            break;
                        }
                        prev = current;
                    }
                }
                return true;
            }
            WebContent::Absent => {}
        }
        if step == reassert_step {
            reassert();
        }
        if step < steps {
            settle(MATERIALIZE_POLL_SECONDS);
        }
    }
    false
}

/// # Safety
///
/// `app_element` must be a valid application `AXUIElementRef` for `pid`.
pub unsafe fn ensure_chromium_ax_enabled(pid: i32, app_element: AXUIElementRef) {
    let stamp = process_start_stamp(pid);
    let cached = ENABLEMENT_STATE
        .lock()
        .ok()
        .and_then(|state| state.get(&pid).copied());
    let attempted_at = Instant::now();
    let prior_timeouts = match next_attempt(cached.as_ref(), stamp, attempted_at) {
        Attempt::Skip => return,
        Attempt::Run { prior_timeouts } => prior_timeouts,
    };
    let chromium_family = || is_chromium_family(pid);
    let outcome = wait_outcome(
        enable_chromium_accessibility(app_element, chromium_family),
        prior_timeouts,
        attempted_at,
        || {
            await_web_content(
                || probe_web_content(app_element),
                || {
                    enable_chromium_accessibility(app_element, chromium_family);
                },
                pump_for,
            )
        },
    );
    if let (Some(stamp), Some(wait)) = (stamp, outcome) {
        if let Ok(mut state) = ENABLEMENT_STATE.lock() {
            state.insert(pid, ProcessEnablement { stamp, wait });
        }
    }
}

/// Whether the cached opt-in belongs to the process lifetime observed now.
fn opted_in(cached: Option<&ProcessEnablement>, observed: Option<ProcessStartStamp>) -> bool {
    matches!(
        (cached, observed),
        (Some(cached), Some(observed))
            if cached.stamp == observed && cached.wait != Wait::NotNeeded
    )
}

/// Whether this process lifetime accepted the Chromium accessibility opt-in,
/// i.e. its windows hold Chromium web content.
fn chromium_web_process(pid: i32) -> bool {
    let cached = ENABLEMENT_STATE
        .lock()
        .ok()
        .and_then(|state| state.get(&pid).copied());
    opted_in(cached.as_ref(), process_start_stamp(pid))
}

/// The point to hit-test for a web area: the middle of the part of it inside
/// its window, or of the whole area when the window frame is unknown or the
/// two do not overlap.
fn hit_test_point(web_area: [f64; 4], window: Option<[f64; 4]>) -> (f64, f64) {
    let [x, y, w, h] = web_area;
    if let Some([wx, wy, ww, wh]) = window {
        let left = x.max(wx);
        let top = y.max(wy);
        let right = (x + w).min(wx + ww);
        let bottom = (y + h).min(wy + wh);
        if right - left >= 1.0 && bottom - top >= 1.0 {
            return ((left + right) / 2.0, (top + bottom) / 2.0);
        }
    }
    (x + w / 2.0, y + h / 2.0)
}

/// First `AXWebArea` under `element`, retained, within a bounded walk.
unsafe fn find_web_area(
    element: AXUIElementRef,
    depth: u32,
    visits: &mut u32,
) -> Option<AXUIElementRef> {
    if depth == 0 {
        return None;
    }
    let mut found = None;
    for child in copy_children(element) {
        if found.is_none() && *visits > 0 {
            *visits -= 1;
            match copy_string_attr(child, "AXRole").as_deref() {
                Some(MENU_BAR_ROLE) => {}
                Some(WEB_AREA_ROLE) => {
                    found = Some(child);
                    continue;
                }
                _ => found = find_web_area(child, depth - 1, visits),
            }
        }
        CFRelease(child as CFTypeRef);
    }
    found
}

/// Bring the browser-side accessibility tree of `window`'s web content up to
/// date before it is read. Returns whether a flush was sent.
///
/// Chromium sends a page's accessibility changes to the browser process on a
/// rendering pass, and a hidden or fully covered window gets none. After an
/// action changes the page (a press, a value write), every accessibility read
/// of that window keeps returning the tree from before the action, so a
/// working press looks like it did nothing and the next press appears to
/// apply the previous one. Any accessibility request the page receives makes
/// it send its pending changes first; a hit-test is the request with no effect
/// on the page, so one at the web area makes the next read current. A no-op
/// for processes that never accepted the Chromium opt-in (native apps).
///
/// # Safety
///
/// `window` must be a valid `AXUIElementRef` of a window of `pid`.
pub unsafe fn flush_pending_web_updates(pid: i32, window: AXUIElementRef) -> bool {
    if !chromium_web_process(pid) {
        return false;
    }
    let mut visits = WEB_AREA_PROBE_NODES;
    let Some(web_area) = find_web_area(window, WEB_AREA_MAX_DEPTH, &mut visits) else {
        return false;
    };
    let area = element_screen_rect(web_area);
    CFRelease(web_area as CFTypeRef);
    let Some(area) = area else {
        return false;
    };
    let (x, y) = hit_test_point(area, element_screen_rect(window));
    match element_at_screen_position(pid, x, y) {
        Some(hit) => {
            CFRelease(hit as CFTypeRef);
            true
        }
        None => false,
    }
}

/// [`flush_pending_web_updates`] for the window holding `element`.
///
/// # Safety
///
/// `element` must be a valid `AXUIElementRef` of `pid`.
pub unsafe fn flush_pending_web_updates_for_element(pid: i32, element: AXUIElementRef) -> bool {
    if !chromium_web_process(pid) {
        return false;
    }
    let Some(window) = copy_element_attr(element, "AXWindow") else {
        return false;
    };
    let flushed = flush_pending_web_updates(pid, window);
    CFRelease(window as CFTypeRef);
    flushed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    fn drive(script: Vec<WebContent>) -> (bool, Vec<String>) {
        let log = RefCell::new(Vec::new());
        let mut remaining = script.into_iter();
        let materialized = await_web_content(
            || {
                let next = remaining.next().unwrap_or(WebContent::Absent);
                log.borrow_mut().push(format!("probe:{next:?}"));
                next
            },
            || log.borrow_mut().push("reassert".to_string()),
            |seconds| log.borrow_mut().push(format!("settle:{seconds}")),
        );
        (materialized, log.into_inner())
    }

    #[test]
    fn a_busy_run_loop_bounds_pumps_and_still_waits_the_whole_interval() {
        let pumps = Cell::new(0u32);
        let started = Instant::now();
        pump_bounded(0.2, |_| pumps.set(pumps.get() + 1));
        assert!(
            pumps.get() <= RUN_LOOP_PUMPS_PER_POLL,
            "a poll interval must not re-enter the run loop unboundedly: {} pumps",
            pumps.get()
        );
        assert!(
            started.elapsed() >= Duration::from_secs_f64(0.2),
            "a poll interval must not return early: {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn start_stamp_reads_the_current_process_and_rejects_dead_pids() {
        let own = process_start_stamp(std::process::id() as i32);
        assert!(own.is_some(), "own process start time must be readable");
        // Pid 0 is the kernel idle task; proc info for it is not readable from
        // user space, so lifetime keying must fail closed (None).
        assert_eq!(process_start_stamp(0), None);
    }

    #[test]
    fn distinct_lifetimes_do_not_alias() {
        // Two different processes must not produce identical (pid, stamp)
        // cache keys. Use launchd (pid 1) vs our own process.
        let own_pid = std::process::id() as i32;
        let own = process_start_stamp(own_pid);
        let launchd = process_start_stamp(1);
        if let (Some(own), Some(launchd)) = (own, launchd) {
            assert_ne!(
                (own_pid, own),
                (1, launchd),
                "cache keys must differ across processes"
            );
        }
    }

    #[test]
    fn only_the_opted_in_process_lifetime_gets_web_flushes() {
        let now = Instant::now();
        let launch = (100, 10);
        for wait in [
            Wait::Complete,
            Wait::TimedOut {
                attempts: 1,
                attempted_at: now,
            },
        ] {
            let cached = ProcessEnablement {
                stamp: launch,
                wait,
            };
            assert!(opted_in(Some(&cached), Some(launch)));
            assert!(
                !opted_in(Some(&cached), Some((101, 20))),
                "a relaunched process at a recycled pid is unknown"
            );
            assert!(
                !opted_in(Some(&cached), None),
                "unreadable lifetime fails closed"
            );
        }
        assert!(
            !opted_in(None, Some(launch)),
            "a process that never accepted the opt-in (native app) gets no flush"
        );
        let native = ProcessEnablement {
            stamp: launch,
            wait: Wait::NotNeeded,
        };
        assert!(
            !opted_in(Some(&native), Some(launch)),
            "a native app that needed no opt-in gets no flush"
        );
        assert_eq!(
            next_attempt(Some(&native), Some(launch), now + Duration::from_secs(3600)),
            Attempt::Skip,
            "a native app is classified once per process lifetime"
        );
    }

    #[test]
    fn the_flush_hit_test_lands_inside_the_visible_part_of_the_web_area() {
        // Web area fully inside the window: its middle.
        assert_eq!(
            hit_test_point(
                [100.0, 100.0, 200.0, 100.0],
                Some([0.0, 0.0, 1000.0, 800.0])
            ),
            (200.0, 150.0)
        );
        // Web area running past the window's right edge: middle of the overlap.
        assert_eq!(
            hit_test_point(
                [40.0, 60.0, 1120.0, 600.0],
                Some([40.0, 30.0, 960.0, 640.0])
            ),
            (520.0, 360.0)
        );
        // No window frame, or no overlap: the web area's own middle.
        assert_eq!(hit_test_point([0.0, 0.0, 10.0, 20.0], None), (5.0, 10.0));
        assert_eq!(
            hit_test_point([0.0, 0.0, 10.0, 20.0], Some([50.0, 50.0, 10.0, 10.0])),
            (5.0, 10.0)
        );
    }

    #[test]
    fn cache_hit_requires_the_same_readable_process_lifetime() {
        let now = Instant::now();
        let first_launch = (100, 10);
        let relaunched = (101, 20);
        let cached = ProcessEnablement {
            stamp: first_launch,
            wait: Wait::Complete,
        };

        assert_eq!(
            next_attempt(Some(&cached), Some(first_launch), now),
            Attempt::Skip
        );
        assert_eq!(
            next_attempt(Some(&cached), Some(relaunched), now),
            Attempt::Run { prior_timeouts: 0 },
            "a relaunched process must not inherit the prior enablement cache entry"
        );
        assert_eq!(
            next_attempt(Some(&cached), None, now),
            Attempt::Run { prior_timeouts: 0 }
        );
        assert_eq!(
            next_attempt(None, Some(first_launch), now),
            Attempt::Run { prior_timeouts: 0 }
        );
    }

    #[test]
    fn both_chromium_opt_ins_wait_for_web_content_and_others_do_not() {
        let now = Instant::now();
        for opt_in in [
            AccessibilityOptIn::EnhancedUserInterface,
            AccessibilityOptIn::ManualAccessibility,
        ] {
            let polls = Cell::new(0u32);
            let ready = wait_outcome(opt_in, 0, now, || {
                polls.set(polls.get() + 1);
                true
            });
            assert_eq!(ready, Some(Wait::Complete));
            assert_eq!(polls.get(), 1, "{opt_in:?} must wait for the web tree");
            assert_eq!(
                wait_outcome(opt_in, 1, now, || false),
                Some(Wait::TimedOut {
                    attempts: 2,
                    attempted_at: now
                }),
                "{opt_in:?}: a tree that never appears is a timeout, not Complete"
            );
        }
        let polls = Cell::new(0u32);
        assert_eq!(
            wait_outcome(AccessibilityOptIn::NotNeeded, 0, now, || {
                polls.set(polls.get() + 1);
                false
            }),
            Some(Wait::NotNeeded),
            "a native app needs no enablement"
        );
        assert_eq!(
            wait_outcome(AccessibilityOptIn::NotAccepted, 0, now, || {
                polls.set(polls.get() + 1);
                false
            }),
            None
        );
        assert_eq!(polls.get(), 0, "native apps must not pay the wait");
    }

    #[test]
    fn a_materialized_tree_is_cached_for_the_process_lifetime() {
        let now = Instant::now();
        let stamp = (100, 10);

        let wait = wait_outcome(AccessibilityOptIn::ManualAccessibility, 0, now, || true);
        assert_eq!(wait, Some(Wait::Complete));

        let cached = ProcessEnablement {
            stamp,
            wait: wait.unwrap(),
        };
        assert_eq!(
            next_attempt(Some(&cached), Some(stamp), now + Duration::from_secs(3600)),
            Attempt::Skip,
            "a materialized process must be enabled once per lifetime"
        );
    }

    #[test]
    fn a_timed_out_tree_is_retried_after_a_backoff_and_only_a_bounded_number_of_times() {
        let now = Instant::now();
        let stamp = (100, 10);
        let backoff = Duration::from_secs_f64(MATERIALIZE_RETRY_BACKOFF_SECONDS);

        let wait = wait_outcome(AccessibilityOptIn::ManualAccessibility, 0, now, || false);
        assert_eq!(
            wait,
            Some(Wait::TimedOut {
                attempts: 1,
                attempted_at: now
            })
        );

        let timed_out = ProcessEnablement {
            stamp,
            wait: wait.unwrap(),
        };
        assert_eq!(
            next_attempt(Some(&timed_out), Some(stamp), now),
            Attempt::Skip,
            "a timed-out process must not re-pay the wait on the next walk"
        );
        assert_eq!(
            next_attempt(Some(&timed_out), Some(stamp), now + backoff),
            Attempt::Run { prior_timeouts: 1 },
            "a timed-out process must be retried once the backoff elapsed"
        );

        let exhausted = ProcessEnablement {
            stamp,
            wait: Wait::TimedOut {
                attempts: MATERIALIZE_MAX_ATTEMPTS,
                attempted_at: now,
            },
        };
        assert_eq!(
            next_attempt(Some(&exhausted), Some(stamp), now + backoff * 1000),
            Attempt::Skip,
            "the wait must be paid a bounded number of times per process lifetime"
        );
    }

    #[test]
    fn an_already_materialized_tree_is_re_asserted_without_polling() {
        let (materialized, log) = drive(vec![
            WebContent::Present { nodes: 42 },
            WebContent::Present { nodes: 42 },
        ]);
        assert!(materialized);
        assert_eq!(
            log,
            vec![
                "probe:Present { nodes: 42 }".to_string(),
                "reassert".to_string(),
                format!("settle:{CHROMIUM_SETTLE_POLL_SECONDS}"),
                "probe:Present { nodes: 42 }".to_string(),
            ],
            "an already-stable AXWebArea exits after one 50ms check instead of sleeping 500ms"
        );
    }

    #[test]
    fn a_slow_tree_is_polled_then_re_asserted() {
        let (materialized, log) = drive(vec![
            WebContent::Absent,
            WebContent::Absent,
            WebContent::Present { nodes: 5 },
            WebContent::Present { nodes: 28 },
            WebContent::Present { nodes: 28 },
        ]);
        assert!(materialized);
        assert_eq!(
            log,
            vec![
                "probe:Absent".to_string(),
                format!("settle:{MATERIALIZE_POLL_SECONDS}"),
                "probe:Absent".to_string(),
                format!("settle:{MATERIALIZE_POLL_SECONDS}"),
                "probe:Present { nodes: 5 }".to_string(),
                "reassert".to_string(),
                format!("settle:{CHROMIUM_SETTLE_POLL_SECONDS}"),
                "probe:Present { nodes: 28 }".to_string(),
                format!("settle:{CHROMIUM_SETTLE_POLL_SECONDS}"),
                "probe:Present { nodes: 28 }".to_string(),
            ],
            "materialization and stabilization must be detected by polling, not a blind 500ms sleep"
        );
    }

    #[test]
    fn a_continuously_growing_tree_is_bounded_by_max_settle_budget() {
        let mut script = Vec::new();
        for n in 1..=20 {
            script.push(WebContent::Present { nodes: n });
        }
        let (materialized, log) = drive(script);
        assert!(materialized);
        let settles = log
            .iter()
            .filter(|e| *e == &format!("settle:{CHROMIUM_SETTLE_POLL_SECONDS}"))
            .count();
        assert_eq!(
            (settles as f64 * CHROMIUM_SETTLE_POLL_SECONDS * 1000.0).round(),
            (CHROMIUM_SETTLE_MAX_SECONDS * 1000.0).round()
        );
    }

    #[test]
    fn nothing_appearing_re_asserts_once_midway_and_gives_up_on_budget() {
        let (materialized, log) = drive(vec![]);
        assert!(!materialized, "a timeout must not claim enablement");

        let probes = log.iter().filter(|e| e.starts_with("probe")).count();
        let polls = log
            .iter()
            .filter(|e| *e == &format!("settle:{MATERIALIZE_POLL_SECONDS}"))
            .count();
        let reasserts = log.iter().filter(|e| *e == "reassert").count();
        assert_eq!(reasserts, 1, "exactly one mid-budget re-assertion");
        assert_eq!(probes, polls + 1, "every wait is followed by a probe");
        assert_eq!(
            polls as f64 * MATERIALIZE_POLL_SECONDS,
            MATERIALIZE_TIMEOUT_SECONDS,
            "the wait must be bounded by the materialization budget"
        );
        assert!(
            !log.iter()
                .any(|e| e == &format!("settle:{CHROMIUM_SETTLE_POLL_SECONDS}")),
            "no post-materialization settle when nothing materialized"
        );
    }
}

//! Process-lifetime Chromium/Electron accessibility enablement.
//!
//! Chromium-family apps (Arc, VS Code, Electron shells) ship their web-content
//! AX tree OFF and only build it once an assistive client asks for it. The
//! walker flips `AXManualAccessibility` (falling back to
//! `AXEnhancedUserInterface` only when the modern attribute is unsupported —
//! see [`super::bindings::enable_chromium_accessibility`]) and then waits for
//! the asynchronously-built tree to actually appear before it is read.
//!
//! The wait is a poll for the expected tree shape, not a fixed sleep: measured
//! on cold VS Code / Cursor / Obsidian launches, the Chromium tree takes
//! 0.6-2.4 s to materialize, so a fixed half-second settle walks a still-empty
//! app and reports a four-row title-bar window as if that were the whole UI.
//! A second assertion after materialization is what attaches the editor buffer
//! (`AXTextArea` with the document) in the VS Code family.
//!
//! The "already enabled" cache is keyed by the observed process lifetime
//! (pid + kernel start time), not by the numeric pid alone: pids are recycled,
//! and a relaunched Electron app must not inherit a stale "enabled" decision
//! that would skip enablement and return an empty web-content tree. It is
//! populated only when materialization was observed, so a timed-out app is
//! retried by the next walk instead of being treated as enabled forever.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use core_foundation::base::{CFRelease, CFTypeRef};

use super::bindings::{
    copy_children, copy_string_attr, enable_chromium_accessibility, AXUIElementRef,
};

/// How long to let the re-asserted app attach its editor buffer before the
/// tree is read. Paid at most once per process lifetime.
const CHROMIUM_SETTLE_SECONDS: f64 = 0.5;

/// Upper bound on the wait for the Chromium tree to materialize. Worst cold
/// launch measured was 2.34 s (VS Code); 4 s leaves headroom on a loaded
/// machine while staying inside the walk deadline.
const MATERIALIZE_TIMEOUT_SECONDS: f64 = 4.0;

/// Poll interval while waiting for the Chromium tree.
const MATERIALIZE_POLL_SECONDS: f64 = 0.1;

/// The role that only exists once the Chromium tree has been built.
const WEB_AREA_ROLE: &str = "AXWebArea";

/// How deep below the application element to look for the web area. Measured
/// depth is 6 (Obsidian) to 8 (VS Code / Cursor) once materialized.
const WEB_AREA_MAX_DEPTH: u32 = 10;

/// How many elements a single probe may visit. An un-materialized Chromium app
/// exposes 12-15 nodes in total, so this only bounds the probe on the walk
/// where the tree has just appeared.
const WEB_AREA_PROBE_NODES: u32 = 400;

type ProcessStartStamp = (u64, u64);

/// What one probe saw of the app's web-content tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WebContent {
    /// An `AXWebArea` descendant exists — the Chromium tree is materialized.
    Present,
    /// Not there yet; keep waiting.
    Absent,
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

/// Process lifetimes for which enablement has already run and the tree has
/// been observed. Values are the process start stamp observed at enablement
/// time.
static ENABLED_PROCESSES: LazyLock<Mutex<HashMap<i32, Option<ProcessStartStamp>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn cached_lifetime_is_current(
    cached: Option<&Option<ProcessStartStamp>>,
    observed: Option<ProcessStartStamp>,
) -> bool {
    cached.is_some_and(|cached| cached.is_some() && *cached == observed)
}

/// Look for an `AXWebArea` below `element`, bounded in depth and in visited
/// nodes.
///
/// # Safety
///
/// `element` must be a valid, live `AXUIElementRef`.
unsafe fn has_web_area(element: AXUIElementRef, depth: u32, visits: &mut u32) -> bool {
    if depth == 0 {
        return false;
    }
    let mut found = false;
    // `copy_children` hands over a +1 reference for every child, so each one is
    // released here even after a match short-circuits the search.
    for child in copy_children(element) {
        if !found && *visits > 0 {
            *visits -= 1;
            found = copy_string_attr(child, "AXRole").as_deref() == Some(WEB_AREA_ROLE)
                || has_web_area(child, depth - 1, visits);
        }
        CFRelease(child as CFTypeRef);
    }
    found
}

/// One probe of `app_element` for a materialized Chromium tree.
///
/// # Safety
///
/// `app_element` must be a valid application `AXUIElementRef`.
unsafe fn probe_web_content(app_element: AXUIElementRef) -> WebContent {
    let mut visits = WEB_AREA_PROBE_NODES;
    if has_web_area(app_element, WEB_AREA_MAX_DEPTH, &mut visits) {
        WebContent::Present
    } else {
        WebContent::Absent
    }
}

/// Let the app work for `seconds` of wall clock.
///
/// `pump_run_loop_briefly` returns as soon as the run loop has handled one
/// input source, so on its own it is an upper bound, not a delay — a busy app
/// would collapse the whole poll budget into a few milliseconds. Pumping until
/// the interval has actually passed keeps step count and elapsed time in sync.
fn pump_for(seconds: f64) {
    let deadline = Instant::now() + Duration::from_secs_f64(seconds.max(0.0));
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return;
        }
        crate::permissions::panel::pump_run_loop_briefly(remaining.as_secs_f64());
    }
}

/// Wait for the Chromium tree, re-asserting enablement, and report whether the
/// tree was observed. Split from the AX calls so the state machine is testable.
///
/// The first assertion has already been accepted when this runs. Two things
/// still have to happen: the tree has to appear (poll, do not sleep), and — for
/// the VS Code family — enablement has to be asserted a *second* time before
/// the editor buffer is attached to the tree. A re-assertion halfway through
/// the budget covers the case where the first assertion landed before the
/// renderer was up and was dropped.
fn await_web_content(
    mut probe: impl FnMut() -> WebContent,
    mut reassert: impl FnMut(),
    mut settle: impl FnMut(f64),
) -> bool {
    let steps = (MATERIALIZE_TIMEOUT_SECONDS / MATERIALIZE_POLL_SECONDS).round() as u32;
    let reassert_step = steps / 2;
    for step in 0..=steps {
        match probe() {
            WebContent::Present => {
                reassert();
                settle(CHROMIUM_SETTLE_SECONDS);
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

/// Flip Chromium/Electron accessibility on for `pid`'s application element and
/// wait for the web-content tree to materialize — once per observed process
/// lifetime. Native Cocoa apps reject the attribute and pay no wait.
///
/// A timeout is not fatal: the caller walks whatever the app exposes, and the
/// process stays un-cached so the next walk tries again.
///
/// # Safety
///
/// `app_element` must be a valid application `AXUIElementRef` for `pid`.
pub unsafe fn ensure_chromium_ax_enabled(pid: i32, app_element: AXUIElementRef) {
    let stamp = process_start_stamp(pid);
    let already_enabled = ENABLED_PROCESSES
        .lock()
        .map(|cache| cached_lifetime_is_current(cache.get(&pid), stamp))
        .unwrap_or(false);
    if already_enabled {
        return;
    }
    if !enable_chromium_accessibility(app_element) {
        return;
    }
    let materialized = await_web_content(
        || probe_web_content(app_element),
        || {
            enable_chromium_accessibility(app_element);
        },
        pump_for,
    );
    if materialized {
        if let Ok(mut cache) = ENABLED_PROCESSES.lock() {
            cache.insert(pid, stamp);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Records what the state machine did, with a probe script standing in for
    /// the AX calls.
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
    fn a_poll_interval_waits_for_the_whole_interval() {
        // The run-loop pump returns as soon as one input source is handled, so
        // the poll interval has to be enforced against the clock: otherwise a
        // busy app burns the entire materialization budget in milliseconds.
        let started = Instant::now();
        pump_for(0.2);
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
    fn cache_hit_requires_the_same_readable_process_lifetime() {
        let first_launch = Some((100, 10));
        let relaunched = Some((101, 20));

        assert!(cached_lifetime_is_current(
            Some(&first_launch),
            first_launch
        ));
        assert!(
            !cached_lifetime_is_current(Some(&first_launch), relaunched),
            "a relaunched process must not inherit the prior enablement cache entry"
        );
        assert!(!cached_lifetime_is_current(Some(&first_launch), None));
        assert!(!cached_lifetime_is_current(Some(&None), first_launch));
        assert!(!cached_lifetime_is_current(None, first_launch));
    }

    #[test]
    fn an_already_materialized_tree_is_re_asserted_without_polling() {
        // Warm app (or one that built its tree during the first assertion):
        // one probe, the editor-exposing re-assertion, one settle. No poll.
        let (materialized, log) = drive(vec![WebContent::Present]);
        assert!(materialized);
        assert_eq!(
            log,
            vec![
                "probe:Present".to_string(),
                "reassert".to_string(),
                format!("settle:{CHROMIUM_SETTLE_SECONDS}"),
            ]
        );
    }

    #[test]
    fn a_slow_tree_is_polled_then_re_asserted() {
        let (materialized, log) = drive(vec![
            WebContent::Absent,
            WebContent::Absent,
            WebContent::Present,
        ]);
        assert!(materialized);
        assert_eq!(
            log,
            vec![
                "probe:Absent".to_string(),
                format!("settle:{MATERIALIZE_POLL_SECONDS}"),
                "probe:Absent".to_string(),
                format!("settle:{MATERIALIZE_POLL_SECONDS}"),
                "probe:Present".to_string(),
                "reassert".to_string(),
                format!("settle:{CHROMIUM_SETTLE_SECONDS}"),
            ],
            "materialization must be detected by polling, not by a fixed sleep"
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
                .any(|e| e == &format!("settle:{CHROMIUM_SETTLE_SECONDS}")),
            "no post-materialization settle when nothing materialized"
        );
    }
}

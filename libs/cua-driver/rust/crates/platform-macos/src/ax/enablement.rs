//! Process-lifetime Chromium/Electron accessibility enablement.
//!
//! Chromium-family apps (Arc, VS Code, Electron shells) ship their web-content
//! AX tree OFF and only build it once an assistive client asks for it. The
//! walker flips `AXManualAccessibility` (falling back to
//! `AXEnhancedUserInterface` only when the modern attribute is unsupported —
//! see [`super::bindings::enable_chromium_accessibility`]) and waits for the
//! asynchronously-built tree to appear before it is read.
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
    copy_children, copy_string_attr, enable_chromium_accessibility, AXUIElementRef,
    AccessibilityOptIn,
};

const CHROMIUM_SETTLE_SECONDS: f64 = 0.5;
const MATERIALIZE_TIMEOUT_SECONDS: f64 = 4.0;
const MATERIALIZE_POLL_SECONDS: f64 = 0.1;
const MATERIALIZE_MAX_ATTEMPTS: u32 = 3;
const MATERIALIZE_RETRY_BACKOFF_SECONDS: f64 = 30.0;
const RUN_LOOP_PUMPS_PER_POLL: u32 = 4;
const WEB_AREA_ROLE: &str = "AXWebArea";
const WEB_AREA_MAX_DEPTH: u32 = 10;
const WEB_AREA_PROBE_NODES: u32 = 400;

type ProcessStartStamp = (u64, u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WebContent {
    Present,
    Absent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wait {
    Complete,
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
        Wait::Complete => Attempt::Skip,
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
        AccessibilityOptIn::EnhancedUserInterface => Some(Wait::Complete),
        AccessibilityOptIn::ManualAccessibility => Some(if await_tree() {
            Wait::Complete
        } else {
            Wait::TimedOut {
                attempts: prior_timeouts + 1,
                attempted_at,
            }
        }),
    }
}

unsafe fn has_web_area(element: AXUIElementRef, depth: u32, visits: &mut u32) -> bool {
    if depth == 0 {
        return false;
    }
    let mut found = false;
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

unsafe fn probe_web_content(app_element: AXUIElementRef) -> WebContent {
    let mut visits = WEB_AREA_PROBE_NODES;
    if has_web_area(app_element, WEB_AREA_MAX_DEPTH, &mut visits) {
        WebContent::Present
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


unsafe fn diag_raw_tree(element: AXUIElementRef, depth: usize, out: &mut Vec<String>) {
    if out.len() > 250 || depth > 9 {
        return;
    }
    let role = super::bindings::copy_string_attr(element, "AXRole");
    let title = super::bindings::copy_string_attr(element, "AXTitle")
        .or_else(|| super::bindings::copy_string_attr(element, "AXDescription"));
    let children = super::bindings::copy_children(element);
    if role.as_deref() != Some("AXMenuBar") {
        out.push(format!("{}{:?} {:?} children={}", "  ".repeat(depth), role, title.map(|t| t.chars().take(50).collect::<String>()), children.len()));
        for child in &children {
            diag_raw_tree(*child, depth + 1, out);
        }
    }
    for child in children {
        core_foundation::base::CFRelease(child as core_foundation::base::CFTypeRef);
    }
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
    let decision = next_attempt(cached.as_ref(), stamp, attempted_at);
    let diag = |line: String| {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("/tmp/cua-diag-4125.log") {
            let _ = writeln!(f, "[diag-4126] pid={pid} {line}");
        }
    };
    diag(format!(
        "ensure cached={cached:?} decision_skip={} probe_before={:?} manual_attr={:?} eui_attr={:?}",
        matches!(decision, Attempt::Skip),
        probe_web_content(app_element),
        super::bindings::copy_bool_attr(app_element, "AXManualAccessibility"),
        super::bindings::copy_bool_attr(app_element, "AXEnhancedUserInterface"),
    ));
    let prior_timeouts = match decision {
        Attempt::Skip => return,
        Attempt::Run { prior_timeouts } => prior_timeouts,
    };
    let opt_in = enable_chromium_accessibility(app_element);
    diag(format!(
        "enable opt_in={opt_in:?} manual_set_err={} eui_set_err={}",
        super::bindings::set_bool_attr_true(app_element, "AXManualAccessibility"),
        super::bindings::set_bool_attr_true(app_element, "AXEnhancedUserInterface"),
    ));
    let outcome = wait_outcome(
        opt_in,
        prior_timeouts,
        attempted_at,
        || {
            await_web_content(
                || probe_web_content(app_element),
                || {
                    enable_chromium_accessibility(app_element);
                },
                pump_for,
            )
        },
    );
    diag(format!(
        "after outcome={outcome:?} probe_after={:?} elapsed={:?}",
        probe_web_content(app_element),
        attempted_at.elapsed()
    ));
    let mut raw = Vec::new();
    diag_raw_tree(app_element, 0, &mut raw);
    diag(format!("raw tree:\n{}", raw.join("\n")));
    if let (Some(stamp), Some(wait)) = (stamp, outcome) {
        if let Ok(mut state) = ENABLEMENT_STATE.lock() {
            state.insert(pid, ProcessEnablement { stamp, wait });
        }
    }
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
    fn only_the_manual_accessibility_path_polls_for_web_content() {
        let now = Instant::now();
        let polls = Cell::new(0u32);
        let probe = || {
            polls.set(polls.get() + 1);
            false
        };

        assert_eq!(
            wait_outcome(AccessibilityOptIn::EnhancedUserInterface, 0, now, probe),
            Some(Wait::Complete),
            "an app that only accepts AXEnhancedUserInterface has no web area to wait for"
        );
        assert_eq!(
            wait_outcome(AccessibilityOptIn::NotAccepted, 0, now, probe),
            None
        );
        assert_eq!(
            polls.get(),
            0,
            "only the Chromium opt-in may pay the materialization wait"
        );
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

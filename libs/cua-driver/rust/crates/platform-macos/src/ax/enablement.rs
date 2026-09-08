//! Process-lifetime Chromium/Electron accessibility enablement.
//!
//! Chromium-family apps (Arc, VS Code, Electron shells) ship their web-content
//! AX tree OFF and only build it once an assistive client asks for it. The
//! walker flips `AXManualAccessibility` (falling back to
//! `AXEnhancedUserInterface` only when the modern attribute is unsupported —
//! see [`super::bindings::enable_chromium_accessibility`]) and pays a one-time
//! settle so the asynchronously-built tree exists before it is read.
//!
//! The "already enabled" cache is keyed by the observed process lifetime
//! (pid + kernel start time), not by the numeric pid alone: pids are recycled,
//! and a relaunched Electron app must not inherit a stale "enabled" decision
//! that would skip enablement and return an empty web-content tree.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use super::bindings::{enable_chromium_accessibility, AXUIElementRef};

/// How long to let a freshly-enabled Chromium/Electron app build its
/// web-content AX tree before we read it. Paid at most once per process
/// lifetime.
const CHROMIUM_SETTLE_SECONDS: f64 = 0.5;

type ProcessStartStamp = (u64, u64);

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

/// Process lifetimes for which enablement has already run and the settle has
/// been paid. Values are the process start stamp observed at enablement time.
fn enabled_processes() -> &'static Mutex<HashMap<i32, Option<ProcessStartStamp>>> {
    static ENABLED: OnceLock<Mutex<HashMap<i32, Option<ProcessStartStamp>>>> = OnceLock::new();
    ENABLED.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cached_lifetime_is_current(
    cached: Option<&Option<ProcessStartStamp>>,
    observed: Option<ProcessStartStamp>,
) -> bool {
    cached.is_some_and(|cached| cached.is_some() && *cached == observed)
}

/// Flip Chromium/Electron accessibility on for `pid`'s application element and
/// wait for the web-content tree to settle — once per observed process
/// lifetime. Native Cocoa apps reject the attribute and pay no settle cost.
///
/// # Safety
///
/// `app_element` must be a valid application `AXUIElementRef` for `pid`.
pub unsafe fn ensure_chromium_ax_enabled(pid: i32, app_element: AXUIElementRef) {
    let stamp = process_start_stamp(pid);
    let already_enabled = enabled_processes()
        .lock()
        .map(|cache| cached_lifetime_is_current(cache.get(&pid), stamp))
        .unwrap_or(false);
    if already_enabled {
        return;
    }
    if enable_chromium_accessibility(app_element) {
        crate::permissions::panel::pump_run_loop_briefly(CHROMIUM_SETTLE_SECONDS);
        if let Ok(mut cache) = enabled_processes().lock() {
            cache.insert(pid, stamp);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}

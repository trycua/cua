// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Host-chosen pacing for synthesized mouse input and the WebKit focus wait
//! (macOS only). Same contract as [`crate::key_pacing`]: read once from the
//! daemon environment, never from a tool argument; unset, empty or unparsable
//! values keep the adapter default.
//!
//! - [`MOUSE_PRIMER_ENV`] (`CUA_DRIVER_MOUSE_PRIMER_MS`): settle after the
//!   `mouseMoved` primer. Default 12.
//! - [`CLICK_GAP_ENV`] (`CUA_DRIVER_CLICK_GAP_MS`): gap between mouse down
//!   and mouse up. Default 28. An NSButton's tracking loop can miss a tighter
//!   gap, so only lower it for targets you have checked.
//! - [`MULTI_CLICK_GAP_ENV`] (`CUA_DRIVER_MULTI_CLICK_GAP_MS`): gap between
//!   the clicks of a double or triple click. Default 80.
//! - [`WEBKIT_SETTLE_ENV`] (`CUA_DRIVER_WEBKIT_SETTLE_MS`): the hard cap on
//!   how long an AX press on a web text field waits for DOM focus. The wait
//!   itself is event driven and normally ends much sooner. `0` skips it.
//!   Default 800.

use std::time::Duration;

pub const MOUSE_PRIMER_ENV: &str = "CUA_DRIVER_MOUSE_PRIMER_MS";
pub const CLICK_GAP_ENV: &str = "CUA_DRIVER_CLICK_GAP_MS";
pub const MULTI_CLICK_GAP_ENV: &str = "CUA_DRIVER_MULTI_CLICK_GAP_MS";
pub const WEBKIT_SETTLE_ENV: &str = "CUA_DRIVER_WEBKIT_SETTLE_MS";

pub const DEFAULT_MOUSE_PRIMER: Duration = Duration::from_millis(12);
pub const DEFAULT_CLICK_GAP: Duration = Duration::from_millis(28);
pub const DEFAULT_MULTI_CLICK_GAP: Duration = Duration::from_millis(80);
pub const DEFAULT_WEBKIT_SETTLE_CAP: Duration = Duration::from_millis(800);

/// Smallest mouse gap; smaller values are raised, as for key gaps.
pub const MIN_MOUSE_GAP: Duration = Duration::from_millis(2);
/// Largest mouse gap and settle cap; larger values are clamped.
pub const MAX_MOUSE_GAP: Duration = Duration::from_millis(200);
pub const MAX_WEBKIT_SETTLE_CAP: Duration = Duration::from_millis(5000);

fn parse_ms(raw: Option<&str>) -> Option<Duration> {
    raw.and_then(|r| r.trim().parse::<u64>().ok())
        .map(Duration::from_millis)
}

/// Resolve a mouse gap from a raw environment value. Pure.
pub fn mouse_gap_from_raw(raw: Option<&str>, default_gap: Duration) -> Duration {
    parse_ms(raw)
        .unwrap_or(default_gap)
        .clamp(MIN_MOUSE_GAP, MAX_MOUSE_GAP)
}

/// Resolve the WebKit settle cap from a raw environment value. Pure.
pub fn webkit_settle_cap_from_raw(raw: Option<&str>) -> Duration {
    parse_ms(raw)
        .unwrap_or(DEFAULT_WEBKIT_SETTLE_CAP)
        .min(MAX_WEBKIT_SETTLE_CAP)
}

fn mouse_gap_from_env(name: &str, default_gap: Duration) -> Duration {
    mouse_gap_from_raw(std::env::var(name).ok().as_deref(), default_gap)
}

macro_rules! once {
    ($f:ident, $e:expr) => {
        pub fn $f() -> Duration {
            static V: std::sync::OnceLock<Duration> = std::sync::OnceLock::new();
            *V.get_or_init(|| $e)
        }
    };
}

once!(
    mouse_primer_settle,
    mouse_gap_from_env(MOUSE_PRIMER_ENV, DEFAULT_MOUSE_PRIMER)
);
once!(
    click_gap,
    mouse_gap_from_env(CLICK_GAP_ENV, DEFAULT_CLICK_GAP)
);
once!(
    multi_click_gap,
    mouse_gap_from_env(MULTI_CLICK_GAP_ENV, DEFAULT_MULTI_CLICK_GAP)
);
once!(
    webkit_settle_cap,
    webkit_settle_cap_from_raw(std::env::var(WEBKIT_SETTLE_ENV).ok().as_deref())
);

/// What a bounded poll saw on its last read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Settle {
    /// The signal held for the stability window; the wait ended early.
    Settled(Duration),
    /// The cap ran out first; the caller proceeds as the fixed wait did.
    TimedOut,
}

/// Wait until `ready()` has been true on two reads `stability` apart, polling
/// every `poll`, for at most `cap` in total. `sleep` is injected so tests run
/// on a fake clock. `now` returns elapsed time since the wait began.
pub fn wait_until_stable(
    cap: Duration,
    poll: Duration,
    stability: Duration,
    mut ready: impl FnMut() -> bool,
    mut now: impl FnMut() -> Duration,
    mut sleep: impl FnMut(Duration),
) -> Settle {
    loop {
        if now() >= cap {
            return Settle::TimedOut;
        }
        if ready() {
            sleep(stability.min(cap.saturating_sub(now())));
            if ready() {
                return Settle::Settled(now());
            }
        }
        if now() >= cap {
            return Settle::TimedOut;
        }
        sleep(poll.min(cap.saturating_sub(now())));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn mouse_gaps_default_parse_and_clamp() {
        assert_eq!(mouse_gap_from_raw(None, ms(28)), ms(28));
        assert_eq!(mouse_gap_from_raw(Some(" 8\n"), ms(28)), ms(8));
        assert_eq!(mouse_gap_from_raw(Some("0"), ms(28)), MIN_MOUSE_GAP);
        assert_eq!(mouse_gap_from_raw(Some("9999"), ms(28)), MAX_MOUSE_GAP);
        for raw in ["", "abc", "-1", "1.5", "8ms"] {
            assert_eq!(mouse_gap_from_raw(Some(raw), ms(28)), ms(28), "{raw:?}");
        }
    }

    #[test]
    fn webkit_cap_defaults_allows_zero_and_clamps() {
        assert_eq!(webkit_settle_cap_from_raw(None), ms(800));
        assert_eq!(webkit_settle_cap_from_raw(Some("0")), ms(0));
        assert_eq!(webkit_settle_cap_from_raw(Some("250")), ms(250));
        assert_eq!(
            webkit_settle_cap_from_raw(Some("99999")),
            MAX_WEBKIT_SETTLE_CAP
        );
        assert_eq!(webkit_settle_cap_from_raw(Some("x")), ms(800));
    }

    /// Fake clock: `sleep` advances it, `ready` flips at `ready_at`.
    fn run(cap: u64, ready_at: u64, flaps: bool) -> Settle {
        let t = Cell::new(0u64);
        let reads = Cell::new(0u32);
        wait_until_stable(
            ms(cap),
            ms(5),
            ms(10),
            || {
                reads.set(reads.get() + 1);
                // `flaps`: ready exactly once, then drops, never stable.
                if flaps {
                    reads.get() == 1
                } else {
                    t.get() >= ready_at
                }
            },
            || ms(t.get()),
            |d| t.set(t.get() + d.as_millis() as u64),
        )
    }

    #[test]
    fn ends_early_once_the_signal_is_stable() {
        match run(800, 30, false) {
            Settle::Settled(d) => assert!(d <= ms(60), "{d:?}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn already_ready_costs_only_the_stability_window() {
        assert_eq!(run(800, 0, false), Settle::Settled(ms(10)));
    }

    #[test]
    fn times_out_at_the_cap_when_the_signal_never_comes() {
        assert_eq!(run(800, u64::MAX, false), Settle::TimedOut);
    }

    #[test]
    fn a_flapping_signal_is_not_stable() {
        assert_eq!(run(100, 0, true), Settle::TimedOut);
    }

    #[test]
    fn zero_cap_never_waits() {
        assert_eq!(run(0, 0, false), Settle::TimedOut);
    }

    /// Micro-benchmark on the real clock against a simulated page whose DOM
    /// focus lands `focus_after` into the wait. Run with
    /// `cargo test -p cua-driver-core --lib bench_webkit_settle -- --ignored --nocapture`.
    #[test]
    #[ignore = "timing benchmark; prints numbers"]
    fn bench_webkit_settle() {
        const RUNS: usize = 20;
        println!("fixed wait before: 800 ms for every web text field click");
        for focus_after in [10u64, 30, 100, 300] {
            let mut took: Vec<f64> = (0..RUNS)
                .map(|_| {
                    let start = std::time::Instant::now();
                    let focused = move || start.elapsed() >= ms(focus_after);
                    wait_until_stable(
                        ms(800),
                        ms(4),
                        ms(8),
                        focused,
                        || start.elapsed(),
                        std::thread::sleep,
                    );
                    start.elapsed().as_secs_f64() * 1e3
                })
                .collect();
            took.sort_by(|a, b| a.partial_cmp(b).unwrap());
            println!(
                "focus lands at {focus_after:>3} ms: event-driven wait median {:.1} ms (max {:.1})",
                took[RUNS / 2],
                took[RUNS - 1]
            );
        }
        let start = std::time::Instant::now();
        wait_until_stable(
            ms(800),
            ms(4),
            ms(8),
            || false,
            || start.elapsed(),
            std::thread::sleep,
        );
        println!(
            "focus never lands: capped at {:.0} ms",
            start.elapsed().as_secs_f64() * 1e3
        );
    }
}

// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Damage-driven settle: end a post-action wait at the first changed frame.
//!
//! Some actions sleep a fixed time so the UI can repaint before the driver
//! reads it back. A platform frame watcher (macOS: a ScreenCaptureKit stream,
//! which delivers a frame only when the display content changed) feeds each
//! frame's [`frame_digest`] and dirty rectangles into a [`DamageSignal`]. The
//! caller arms the signal for the region it acts on before the action and
//! [`settle`]s after it: the wait ends at the first frame that differs from
//! the one before it and was redrawn inside that region, and the old fixed
//! duration stays as the cap.
//!
//! A changed frame is **not** application readiness. Any repaint inside the
//! region satisfies it, a caret blink included. It only turns a fixed sleep
//! into "wake at the first repaint, cap at the old duration"; a caller that
//! needs semantic readiness still polls AX.
//!
//! Without a watcher (no Screen Recording grant, no direct-capture consent,
//! a stream that failed or has not started yet, or a platform with no
//! watcher) the wait is the fixed sleep, unchanged. Only macOS has a watcher;
//! Windows and Linux keep their fixed sleeps.

use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// A watcher with no armed caller for this long stops its stream; the next
/// arm starts it again.
pub const IDLE_STOP: Duration = Duration::from_secs(30);

/// FNV-1a over the first `row_bytes` of each of `rows` rows laid out `stride`
/// bytes apart, so row padding, which a repaint does not write, is left out.
/// A frame shorter than its layout hashes only the rows it holds.
pub fn frame_digest(bytes: &[u8], row_bytes: usize, stride: usize, rows: usize) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let stride = stride.max(row_bytes).max(1);
    bytes
        .chunks(stride)
        .take(rows)
        .flat_map(|row| &row[..row_bytes.min(row.len())])
        .fold(OFFSET, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(PRIME)
        })
}

/// A rectangle in the watcher's frame coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Region {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Region {
    fn intersects(&self, other: &Region) -> bool {
        self.x < other.x + other.width
            && other.x < self.x + self.width
            && self.y < other.y + other.height
            && other.y < self.y + self.height
    }
}

/// How a settle ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Damage {
    /// A changed frame was redrawn inside the armed region.
    Changed,
    /// No such frame arrived within the cap.
    TimedOut,
    /// No watcher, or it failed during the wait; the rest of the cap was slept.
    Unavailable,
}

struct Arm {
    id: u64,
    region: Region,
    changed: bool,
}

struct State {
    /// Digest of the last frame delivered; `None` before the first one.
    digest: Option<u64>,
    failed: bool,
    arms: Vec<Arm>,
    next_id: u64,
    last_use: Instant,
}

/// Frames from one display's watcher, and the callers waiting on them.
pub struct DamageSignal {
    state: Mutex<State>,
    changed: Condvar,
}

impl Default for DamageSignal {
    fn default() -> Self {
        Self::new()
    }
}

impl DamageSignal {
    /// A signal for a watcher that has delivered no frame yet. It counts as
    /// used now, so it is not idle before its first arm.
    pub fn new() -> Self {
        Self {
            state: Mutex::new(State {
                digest: None,
                failed: false,
                arms: Vec::new(),
                next_id: 0,
                last_use: Instant::now(),
            }),
            changed: Condvar::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Record a delivered frame with the rectangles redrawn since the last
    /// one; `None` when the platform did not say, which counts the whole
    /// frame. The first frame only sets what is on screen: an arm taken
    /// before it cannot tell whether it shows the action's effect, so it waits
    /// for a later change or the cap.
    pub fn frame(&self, digest: u64, dirty: Option<&[Region]>) {
        let mut state = self.lock();
        if state.failed
            || state
                .digest
                .replace(digest)
                .is_none_or(|last| last == digest)
        {
            return;
        }
        let mut woke = false;
        for arm in &mut state.arms {
            if dirty.is_none_or(|rects| rects.iter().any(|rect| rect.intersects(&arm.region))) {
                arm.changed = true;
                woke = true;
            }
        }
        if woke {
            self.changed.notify_all();
        }
    }

    /// The watcher stopped delivering frames. Waiters wake and sleep out the
    /// rest of their cap; later arms get `None`.
    pub fn fail(&self) {
        self.lock().failed = true;
        self.changed.notify_all();
    }

    /// Watch `region` from the current frame on, before an action. `None`
    /// once the watcher failed.
    pub fn arm(self: &Arc<Self>, region: Region) -> Option<Armed> {
        let mut state = self.lock();
        if state.failed {
            return None;
        }
        let id = state.next_id;
        state.next_id += 1;
        state.arms.push(Arm {
            id,
            region,
            changed: false,
        });
        state.last_use = Instant::now();
        Some(Armed {
            signal: Arc::clone(self),
            id,
        })
    }

    /// Whether the watcher should stop at `now`: it failed, or nobody holds an
    /// arm and nobody armed it for [`IDLE_STOP`].
    pub fn should_stop(&self, now: Instant) -> bool {
        let state = self.lock();
        state.failed
            || (state.arms.is_empty() && now.saturating_duration_since(state.last_use) >= IDLE_STOP)
    }
}

/// A region watched by [`DamageSignal::arm`] since before an action.
pub struct Armed {
    signal: Arc<DamageSignal>,
    id: u64,
}

impl Armed {
    /// Block until a changed frame is redrawn inside the region, the watcher
    /// fails, or `cap` passes.
    pub fn wait_for_change(&self, cap: Duration) -> Damage {
        let deadline = Instant::now() + cap;
        let mut state = self.signal.lock();
        loop {
            if state
                .arms
                .iter()
                .any(|arm| arm.id == self.id && arm.changed)
            {
                return Damage::Changed;
            }
            if state.failed {
                return Damage::Unavailable;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Damage::TimedOut;
            }
            state = self
                .signal
                .changed
                .wait_timeout(state, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}

impl Drop for Armed {
    fn drop(&mut self) {
        let mut state = self.signal.lock();
        state.arms.retain(|arm| arm.id != self.id);
        state.last_use = Instant::now();
    }
}

/// Wait after an action for at most `cap`: until the first changed frame in
/// the armed region, otherwise (no arm, or the watcher fails) for the rest of
/// the cap, as the fixed sleep did.
pub fn settle(armed: Option<Armed>, cap: Duration) -> Damage {
    let start = Instant::now();
    let outcome = armed.map_or(Damage::Unavailable, |armed| armed.wait_for_change(cap));
    if outcome == Damage::Unavailable {
        std::thread::sleep(cap.saturating_sub(start.elapsed()));
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    const CAP: Duration = Duration::from_millis(400);
    const SOON: Duration = Duration::from_millis(30);

    const TARGET: Region = Region {
        x: 10.0,
        y: 10.0,
        width: 20.0,
        height: 20.0,
    };
    const ELSEWHERE: Region = Region {
        x: 100.0,
        y: 100.0,
        width: 5.0,
        height: 5.0,
    };

    fn watching(first: u64) -> Arc<DamageSignal> {
        let signal = Arc::new(DamageSignal::new());
        signal.frame(first, None);
        signal
    }

    fn deliver_after(signal: &Arc<DamageSignal>, delay: Duration, digest: u64, dirty: Region) {
        let signal = Arc::clone(signal);
        thread::spawn(move || {
            thread::sleep(delay);
            signal.frame(digest, Some(&[dirty]));
        });
    }

    #[test]
    fn digest_skips_row_padding_and_sees_content() {
        let padded = [1, 2, 9, 9, 3, 4, 7, 7];
        let other_padding = [1, 2, 0, 0, 3, 4, 0, 0];
        let changed = [1, 2, 9, 9, 3, 5, 7, 7];
        assert_eq!(
            frame_digest(&padded, 2, 4, 2),
            frame_digest(&other_padding, 2, 4, 2)
        );
        assert_ne!(
            frame_digest(&padded, 2, 4, 2),
            frame_digest(&changed, 2, 4, 2)
        );
        assert_eq!(
            frame_digest(&padded, 2, 4, 1),
            frame_digest(&[1, 2], 2, 2, 1)
        );
    }

    #[test]
    fn a_change_in_the_region_ends_the_wait_before_the_cap() {
        let signal = watching(1);
        let armed = signal.arm(TARGET).unwrap();
        deliver_after(&signal, SOON, 2, TARGET);
        let start = Instant::now();
        assert_eq!(settle(Some(armed), CAP), Damage::Changed);
        assert!(start.elapsed() < CAP, "{:?}", start.elapsed());
    }

    #[test]
    fn a_change_elsewhere_rides_out_the_cap() {
        let signal = watching(1);
        let armed = signal.arm(TARGET).unwrap();
        deliver_after(&signal, SOON, 2, ELSEWHERE);
        let start = Instant::now();
        assert_eq!(settle(Some(armed), CAP), Damage::TimedOut);
        assert!(start.elapsed() >= CAP);
    }

    #[test]
    fn an_identical_redraw_rides_out_the_cap() {
        let signal = watching(1);
        let armed = signal.arm(TARGET).unwrap();
        deliver_after(&signal, SOON, 1, TARGET);
        assert_eq!(settle(Some(armed), CAP), Damage::TimedOut);
    }

    #[test]
    fn unknown_dirty_rects_count_the_whole_frame() {
        let signal = watching(1);
        let armed = signal.arm(TARGET).unwrap();
        signal.frame(2, None);
        assert_eq!(armed.wait_for_change(Duration::ZERO), Damage::Changed);
    }

    #[test]
    fn a_change_before_the_wait_is_not_lost() {
        let signal = watching(1);
        let armed = signal.arm(TARGET).unwrap();
        signal.frame(2, Some(&[TARGET]));
        signal.frame(1, Some(&[ELSEWHERE]));
        assert_eq!(armed.wait_for_change(Duration::ZERO), Damage::Changed);
    }

    #[test]
    fn the_first_frame_after_arming_is_not_a_change() {
        let signal = Arc::new(DamageSignal::new());
        let armed = signal.arm(TARGET).unwrap();
        signal.frame(1, None);
        assert_eq!(armed.wait_for_change(SOON), Damage::TimedOut);
        signal.frame(2, None);
        assert_eq!(armed.wait_for_change(SOON), Damage::Changed);
    }

    #[test]
    fn a_failed_watcher_falls_back_to_the_full_cap() {
        let signal = watching(1);
        let armed = signal.arm(TARGET).unwrap();
        let failing = Arc::clone(&signal);
        thread::spawn(move || {
            thread::sleep(SOON);
            failing.fail();
        });
        let start = Instant::now();
        assert_eq!(settle(Some(armed), CAP), Damage::Unavailable);
        assert!(start.elapsed() >= CAP);
        assert!(signal.arm(TARGET).is_none());
        assert!(signal.should_stop(Instant::now()));
    }

    #[test]
    fn no_watcher_sleeps_the_full_cap() {
        let start = Instant::now();
        assert_eq!(settle(None, CAP), Damage::Unavailable);
        assert!(start.elapsed() >= CAP);
    }

    #[test]
    fn stops_only_when_idle_and_unarmed() {
        let signal = watching(1);
        assert!(!signal.should_stop(Instant::now()));
        let armed = signal.arm(TARGET).unwrap();
        assert!(!signal.should_stop(Instant::now() + IDLE_STOP * 2));
        drop(armed);
        assert!(!signal.should_stop(Instant::now()));
        assert!(signal.should_stop(Instant::now() + IDLE_STOP));
    }
}

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Delay-based encoder rate control for one media session.
//!
//! The controller looks at the worst attached viewer every evaluation window
//! (500 ms): how long its oldest queued frame has waited, and how much its ack
//! round trip exceeds the lowest round trip seen recently. Congestion is
//! acted on only after it persists, bitrate is reduced before frame rate
//! (a bitrate-targeted encoder does not send fewer bytes at fewer frames),
//! and recovery is slow and additive. A very large delay halves both at once.

use std::time::Duration;

/// Evaluation window.
pub const RATE_WINDOW: Duration = Duration::from_millis(500);
/// Excess delay above which a window counts as congested.
pub const CONGESTED_DELAY_MS: f64 = 150.0;
/// Excess delay above which the controller cuts hard immediately.
pub const SEVERE_DELAY_MS: f64 = 1000.0;
/// Consecutive congested windows before a normal cut.
pub const BAD_WINDOWS_BEFORE_CUT: u32 = 3;
/// Consecutive healthy windows before a recovery step.
pub const GOOD_WINDOWS_BEFORE_STEP: u32 = 6;
pub const MIN_BITRATE_KBPS: u32 = 250;
pub const MIN_FPS: u16 = 5;

/// One window's congestion signal (the worst viewer).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CongestionSample {
    /// Age of the oldest frame still queued for a viewer, in ms.
    pub queue_delay_ms: f64,
    /// Ack round trip above the viewer's recent minimum, in ms.
    pub ack_excess_ms: f64,
    /// Dependent frames discarded during the window.
    pub dropped_frames: u64,
    /// True when the target produced frames during the window. Recovery only
    /// probes upward while something is actually being sent.
    pub active: bool,
}

impl CongestionSample {
    fn excess_ms(&self) -> f64 {
        self.queue_delay_ms.max(self.ack_excess_ms)
    }
}

/// What the controller wants the encoder to do after a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateDecision {
    pub bitrate_kbps: u32,
    pub max_fps: u16,
}

#[derive(Debug, Clone)]
pub struct RateController {
    target_bitrate_kbps: u32,
    target_fps: u16,
    bitrate_kbps: u32,
    fps: u16,
    bad_windows: u32,
    good_windows: u32,
}

impl RateController {
    pub fn new(target_bitrate_kbps: u32, target_fps: u16) -> Self {
        let target_bitrate_kbps = target_bitrate_kbps.max(MIN_BITRATE_KBPS);
        let target_fps = target_fps.max(1);
        Self {
            target_bitrate_kbps,
            target_fps,
            bitrate_kbps: target_bitrate_kbps,
            fps: target_fps,
            bad_windows: 0,
            good_windows: 0,
        }
    }

    pub fn current(&self) -> RateDecision {
        RateDecision {
            bitrate_kbps: self.bitrate_kbps,
            max_fps: self.fps,
        }
    }

    /// Change the ceiling (a client `SetPreferences`). The current values
    /// jump to the new ceiling; the controller cuts again if needed.
    pub fn set_targets(&mut self, bitrate_kbps: u32, fps: u16) {
        *self = Self::new(bitrate_kbps, fps);
    }

    /// Feed one window. Returns a decision only when it changed.
    pub fn on_window(&mut self, sample: CongestionSample) -> Option<RateDecision> {
        let before = self.current();
        let excess = sample.excess_ms();
        if excess >= SEVERE_DELAY_MS {
            self.bitrate_kbps = (self.bitrate_kbps / 2).max(MIN_BITRATE_KBPS);
            self.fps = (self.fps / 2).max(MIN_FPS.min(self.target_fps));
            self.bad_windows = 0;
            self.good_windows = 0;
        } else if excess >= CONGESTED_DELAY_MS || sample.dropped_frames > 0 {
            self.good_windows = 0;
            self.bad_windows += 1;
            if self.bad_windows >= BAD_WINDOWS_BEFORE_CUT {
                self.bad_windows = 0;
                if self.bitrate_kbps > MIN_BITRATE_KBPS {
                    // At most a 20% cut per step.
                    self.bitrate_kbps = (self.bitrate_kbps * 4 / 5).max(MIN_BITRATE_KBPS);
                } else {
                    self.fps = (self.fps * 4 / 5).max(MIN_FPS.min(self.target_fps));
                }
            }
        } else {
            self.bad_windows = 0;
            if sample.active {
                self.good_windows += 1;
            }
            if self.good_windows >= GOOD_WINDOWS_BEFORE_STEP {
                self.good_windows = 0;
                // Restore frame rate first (it was cut last), then bitrate.
                if self.fps < self.target_fps {
                    self.fps = (self.fps + (self.target_fps / 10).max(1)).min(self.target_fps);
                } else if self.bitrate_kbps < self.target_bitrate_kbps {
                    let step = (self.bitrate_kbps / 10).max(50);
                    self.bitrate_kbps = (self.bitrate_kbps + step).min(self.target_bitrate_kbps);
                }
            }
        }
        let after = self.current();
        (after != before).then_some(after)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn congested() -> CongestionSample {
        CongestionSample {
            queue_delay_ms: 300.0,
            active: true,
            ..CongestionSample::default()
        }
    }

    fn healthy() -> CongestionSample {
        CongestionSample {
            queue_delay_ms: 5.0,
            active: true,
            ..CongestionSample::default()
        }
    }

    #[test]
    fn congestion_must_persist_and_cuts_bitrate_before_fps() {
        let mut controller = RateController::new(4000, 30);
        assert_eq!(controller.on_window(congested()), None);
        assert_eq!(controller.on_window(congested()), None);
        let decision = controller.on_window(congested()).unwrap();
        assert_eq!(decision.bitrate_kbps, 3200);
        assert_eq!(decision.max_fps, 30);
    }

    #[test]
    fn fps_is_cut_only_at_the_bitrate_floor() {
        let mut controller = RateController::new(MIN_BITRATE_KBPS, 30);
        for _ in 0..3 {
            controller.on_window(congested());
        }
        assert_eq!(controller.current().bitrate_kbps, MIN_BITRATE_KBPS);
        assert_eq!(controller.current().max_fps, 24);
    }

    #[test]
    fn severe_delay_halves_both_immediately() {
        let mut controller = RateController::new(4000, 30);
        let decision = controller
            .on_window(CongestionSample {
                queue_delay_ms: 1500.0,
                ..CongestionSample::default()
            })
            .unwrap();
        assert_eq!(
            decision,
            RateDecision {
                bitrate_kbps: 2000,
                max_fps: 15
            }
        );
    }

    #[test]
    fn recovery_is_slow_restores_fps_first_and_never_exceeds_target() {
        let mut controller = RateController::new(4000, 30);
        controller.on_window(CongestionSample {
            queue_delay_ms: 2000.0,
            ..CongestionSample::default()
        });
        for _ in 0..5 {
            assert_eq!(controller.on_window(healthy()), None);
        }
        let first = controller.on_window(healthy()).unwrap();
        assert_eq!(first.max_fps, 18);
        assert_eq!(first.bitrate_kbps, 2000);
        for _ in 0..600 {
            controller.on_window(healthy());
        }
        assert_eq!(
            controller.current(),
            RateDecision {
                bitrate_kbps: 4000,
                max_fps: 30
            }
        );
    }

    #[test]
    fn idle_windows_do_not_probe_upward() {
        let mut controller = RateController::new(4000, 30);
        controller.on_window(CongestionSample {
            queue_delay_ms: 2000.0,
            ..CongestionSample::default()
        });
        for _ in 0..50 {
            assert_eq!(
                controller.on_window(CongestionSample::default()),
                None,
                "an idle screen gives no evidence the path can carry more"
            );
        }
    }
}

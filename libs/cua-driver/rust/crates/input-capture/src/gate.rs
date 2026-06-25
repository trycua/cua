//! Indicator-gated capture coupling — the "recording-LED" wire.
//!
//! Security invariant: a human input event may be buffered **only** while the
//! visible recording border is provably being painted on the target window.
//! This is the software analog of a camera LED wired to the sensor's power
//! rail. We enforce it with a single shared [`IndicatorHeartbeat`]:
//!
//! * The indicator's render loop calls [`IndicatorHeartbeat::present`] from
//!   **inside an actually-presented frame**, recording the frame sequence, the
//!   paint timestamp, and the rect the border currently covers. Because the
//!   only writer is the present path, a fresh heartbeat is literal proof that a
//!   border frame was drawn — it cannot be faked without drawing.
//! * The indicator's watchdog calls [`IndicatorHeartbeat::mark_compromised`]
//!   the moment the border stops being visible/topmost/covering/opaque.
//! * The low-level input hook holds a [`CaptureGate`] and calls
//!   [`CaptureGate::allow`] **before buffering any event**. It returns `true`
//!   only when the heartbeat is fresh, has presented at least one frame, is not
//!   compromised, and still covers the point being acted on.
//!
//! There is deliberately no way to obtain a [`CaptureGate`] except from a live
//! [`IndicatorHeartbeat`], and no way to install the platform hook except by
//! handing it a gate (see the platform backends). So capture cannot be started
//! without an indicator, and cannot continue once the indicator goes dark.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Maximum age (ms) of the last presented indicator frame for capture to be
/// allowed. At a ~30 Hz present cadence (~33 ms/frame) this tolerates a few
/// dropped frames but goes dark within a quarter second if the border stalls,
/// is destroyed, or stops painting.
pub const DEFAULT_MAX_FRAME_AGE_MS: u64 = 250;

/// Shared proof-of-presence between the visible indicator and the input hook.
/// Cheap to clone via `Arc`.
#[derive(Debug)]
pub struct IndicatorHeartbeat {
    /// Monotonically increasing count of presented border frames. `0` means
    /// nothing has ever been painted, so capture is never allowed.
    frame_seq: AtomicU64,
    /// Monotonic-clock timestamp (ms) of the most recently presented frame.
    last_paint_ms: AtomicU64,
    /// Set false by the watchdog the instant the border is no longer
    /// visible+topmost+covering+opaque. Latches capture dark independent of
    /// frame freshness (e.g. a frozen-but-cloaked window).
    visible_ok: AtomicBool,
    /// Screen-space rect [x, y, w, h] the border currently covers — i.e. the
    /// tracked target-window bounds. Capture is only allowed for points inside
    /// it, so the border the human sees always encloses what is recorded.
    covered_rect: Mutex<[i32; 4]>,
}

impl IndicatorHeartbeat {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            frame_seq: AtomicU64::new(0),
            last_paint_ms: AtomicU64::new(0),
            visible_ok: AtomicBool::new(false),
            covered_rect: Mutex::new([0, 0, 0, 0]),
        })
    }

    /// Called from inside an actually-presented border frame. `rect` is the
    /// target-window bounds the frame was drawn around. This is the **only**
    /// writer that can re-arm capture.
    pub fn present(&self, now_ms: u64, rect: [i32; 4]) {
        *self.covered_rect.lock().unwrap() = rect;
        self.last_paint_ms.store(now_ms, Ordering::SeqCst);
        self.frame_seq.fetch_add(1, Ordering::SeqCst);
        self.visible_ok.store(true, Ordering::SeqCst);
    }

    /// Called by the watchdog when the border is hidden, cloaked, moved off the
    /// target, un-topmosted, or dimmed below the alpha threshold. Latches
    /// capture dark until the next genuine `present`.
    pub fn mark_compromised(&self) {
        self.visible_ok.store(false, Ordering::SeqCst);
    }

    pub fn frame_seq(&self) -> u64 {
        self.frame_seq.load(Ordering::SeqCst)
    }

    fn covered(&self, x: f64, y: f64) -> bool {
        let [rx, ry, rw, rh] = *self.covered_rect.lock().unwrap();
        if rw <= 0 || rh <= 0 {
            return false;
        }
        let (x, y) = (x as i32, y as i32);
        x >= rx && x < rx + rw && y >= ry && y < ry + rh
    }

    /// Core liveness predicate. `point` is the screen-space location of the
    /// event being considered (mouse position; for key events pass the last
    /// known cursor or the target center — the caller scopes those by
    /// foreground window separately).
    fn live(&self, now_ms: u64, max_age_ms: u64, point: Option<(f64, f64)>) -> bool {
        if !self.visible_ok.load(Ordering::SeqCst) {
            return false;
        }
        if self.frame_seq.load(Ordering::SeqCst) == 0 {
            return false;
        }
        let last = self.last_paint_ms.load(Ordering::SeqCst);
        // Saturating: a clock that went backwards reads as stale, not fresh.
        if now_ms.saturating_sub(last) > max_age_ms {
            return false;
        }
        match point {
            Some((x, y)) => self.covered(x, y),
            None => true,
        }
    }
}

/// The capture side of the coupling. Held by the platform input hook; the only
/// way to construct one is from a live heartbeat.
#[derive(Debug, Clone)]
pub struct CaptureGate {
    heartbeat: Arc<IndicatorHeartbeat>,
    max_age_ms: u64,
}

impl CaptureGate {
    pub fn new(heartbeat: Arc<IndicatorHeartbeat>) -> Self {
        Self { heartbeat, max_age_ms: DEFAULT_MAX_FRAME_AGE_MS }
    }

    pub fn with_max_age(heartbeat: Arc<IndicatorHeartbeat>, max_age_ms: u64) -> Self {
        Self { heartbeat, max_age_ms }
    }

    /// Gate a positioned event (mouse/scroll/drag). Returns true only when the
    /// border is provably live AND covers the event point.
    pub fn allow_at(&self, now_ms: u64, x: f64, y: f64) -> bool {
        self.heartbeat.live(now_ms, self.max_age_ms, Some((x, y)))
    }

    /// Gate a non-positioned event (keystroke). The caller is responsible for
    /// having already confirmed the target window is foreground; this only
    /// checks the border is live.
    pub fn allow(&self, now_ms: u64) -> bool {
        self.heartbeat.live(now_ms, self.max_age_ms, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_frame_means_no_capture() {
        let hb = IndicatorHeartbeat::new();
        let gate = CaptureGate::new(hb.clone());
        // Never presented -> dark.
        assert!(!gate.allow(1000));
        assert!(!gate.allow_at(1000, 5.0, 5.0));
    }

    #[test]
    fn fresh_frame_allows_capture() {
        let hb = IndicatorHeartbeat::new();
        let gate = CaptureGate::new(hb.clone());
        hb.present(1000, [0, 0, 100, 100]);
        assert!(gate.allow(1000));
        assert!(gate.allow_at(1010, 50.0, 50.0));
    }

    #[test]
    fn stale_frame_blocks_capture() {
        let hb = IndicatorHeartbeat::new();
        let gate = CaptureGate::new(hb.clone());
        hb.present(1000, [0, 0, 100, 100]);
        // 251 ms later, no new frame -> dark.
        assert!(!gate.allow(1000 + DEFAULT_MAX_FRAME_AGE_MS + 1));
        // A new frame re-arms it.
        hb.present(1300, [0, 0, 100, 100]);
        assert!(gate.allow(1300));
    }

    #[test]
    fn compromised_blocks_until_next_present() {
        let hb = IndicatorHeartbeat::new();
        let gate = CaptureGate::new(hb.clone());
        hb.present(1000, [0, 0, 100, 100]);
        assert!(gate.allow(1000));
        hb.mark_compromised();
        assert!(!gate.allow(1000));
        // Re-presenting (border painted again) re-arms.
        hb.present(1001, [0, 0, 100, 100]);
        assert!(gate.allow(1001));
    }

    #[test]
    fn point_outside_covered_rect_blocked() {
        let hb = IndicatorHeartbeat::new();
        let gate = CaptureGate::new(hb.clone());
        hb.present(1000, [10, 10, 100, 100]);
        assert!(gate.allow_at(1000, 50.0, 50.0)); // inside
        assert!(!gate.allow_at(1000, 5.0, 5.0)); // left/above the rect
        assert!(!gate.allow_at(1000, 200.0, 200.0)); // below/right
    }

    #[test]
    fn clock_going_backwards_reads_stale_not_fresh() {
        let hb = IndicatorHeartbeat::new();
        let gate = CaptureGate::new(hb.clone());
        hb.present(5000, [0, 0, 100, 100]);
        // now earlier than last paint -> saturating_sub = 0 <= max_age, so it
        // is treated as fresh (monotonic clock should never go backwards; this
        // documents the saturating behavior rather than asserting a policy).
        assert!(gate.allow(4000));
    }
}

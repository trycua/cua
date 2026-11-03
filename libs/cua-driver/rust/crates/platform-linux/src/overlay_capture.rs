//! Linux adapter of `cursor_overlay::capture_exclusion`.
//!
//! X11 has no per-window capture exclusion: a root `GetImage` returns whatever
//! the server is showing, our override-redirect overlay included. The adapter
//! therefore hides the overlay around the grab:
//!
//! 1. the capture thread raises a hold and wakes the overlay owner thread;
//! 2. the owner thread empties the overlay's bounding shape, round-trips the
//!    server so the change is in effect, answers with what it knows about the
//!    pixels it last painted, and paints nothing until the hold is released;
//! 3. the capture thread lets the uncovered windows repaint, grabs the root,
//!    and releases the hold; the owner thread repaints the current frame.
//!
//! Under a compositing manager the uncovered pixels come back from the
//! windows' own buffers on the next composite. Without one the windows below
//! must answer Expose, and a slow client can leave our last frame in the
//! framebuffer. Those pixels are still exactly what the overlay uploaded, so
//! they are found by comparison and replaced with the desktop the overlay
//! recorded under them ([`scrub_residual_overlay`]) — the same save-under rule
//! the overlay's own compositing uses.
//!
//! Wayland overlays (layer-shell surface, GNOME Shell helper) are drawn by the
//! compositor into the same output the screencopy / portal capture reads, and
//! the Driver has no way to leave them out. That limitation is reported as
//! `not_excluded` instead of being papered over.

use std::sync::{Condvar, Mutex};
use std::time::Duration;

use cursor_overlay::capture_exclusion::{ExclusionStart, OverlayCaptureExcluder};

/// Method names reported in `agent_overlay_capture.method`.
pub const X11_METHOD: &str = "x11_unshaped_overlay";
pub const X11_COMPOSITED_METHOD: &str = "x11_unshaped_overlay_under_compositor";
pub const X11_SAVE_UNDER_REPAIR: &str = "x11_save_under";

/// How long the capture thread waits for the overlay owner to hide.
const HOLD_ACK_TIMEOUT: Duration = Duration::from_millis(500);
/// Time given to the compositor, or to the windows answering Expose, before
/// the root is read. Whatever is still ours afterwards is scrubbed.
const HIDE_SETTLE: Duration = Duration::from_millis(50);

/// One rect the overlay painted: the desktop it recorded under the rect and
/// the pixels it uploaded, both row-major BGRX (`width * height * 4`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResidualPatch {
    pub x: i32,
    pub y: i32,
    pub width: usize,
    pub height: usize,
    pub under: Vec<u8>,
    pub uploaded: Vec<u8>,
}

/// What the overlay owner reported after hiding.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HiddenOverlay {
    /// Whether any overlay pixel was on screen before the hide.
    pub was_shown: bool,
    /// Whether a compositing manager blends the overlay (no residual check is
    /// possible or needed: the compositor repaints from window buffers).
    pub composited: bool,
    /// Painted rects that may still hold our pixels, newest first.
    pub residual: Vec<ResidualPatch>,
}

/// The owner thread's answer to a hold request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HoldAnswer {
    Hidden(HiddenOverlay),
    Failed(String),
}

#[derive(Default)]
struct HoldState {
    generation: u64,
    requested: bool,
    answer: Option<(u64, HoldAnswer)>,
}

/// Handshake between a capture thread and the overlay owner thread.
pub struct CaptureHold {
    state: Mutex<HoldState>,
    changed: Condvar,
}

impl CaptureHold {
    pub const fn new() -> Self {
        Self {
            state: Mutex::new(HoldState {
                generation: 0,
                requested: false,
                answer: None,
            }),
            changed: Condvar::new(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HoldState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Capture side: ask the owner to hide. Returns the request generation.
    pub fn request(&self) -> u64 {
        let mut state = self.lock();
        state.generation = state.generation.wrapping_add(1);
        state.requested = true;
        state.answer = None;
        state.generation
    }

    /// Capture side: wait for the owner's answer to `generation`.
    pub fn wait_answer(&self, generation: u64, timeout: Duration) -> Option<HoldAnswer> {
        let state = self.lock();
        let (mut state, _) = self
            .changed
            .wait_timeout_while(
                state,
                timeout,
                |state| !matches!(&state.answer, Some((answered, _)) if *answered == generation),
            )
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match state.answer.take() {
            Some((answered, answer)) if answered == generation => Some(answer),
            other => {
                state.answer = other;
                None
            }
        }
    }

    /// Capture side: let the overlay paint again.
    pub fn release(&self) {
        let mut state = self.lock();
        state.requested = false;
        state.answer = None;
        self.changed.notify_all();
    }

    /// Owner side: the request still waiting for an answer, if any.
    pub fn unanswered_request(&self) -> Option<u64> {
        let state = self.lock();
        (state.requested && state.answer.is_none()).then_some(state.generation)
    }

    /// Owner side: whether painting is still held.
    pub fn is_requested(&self) -> bool {
        self.lock().requested
    }

    /// Owner side: answer `generation`. A stale answer (the capture already
    /// gave up and released, or asked again) is dropped.
    pub fn answer(&self, generation: u64, answer: HoldAnswer) {
        let mut state = self.lock();
        if state.requested && state.generation == generation {
            state.answer = Some((generation, answer));
            self.changed.notify_all();
        }
    }
}

impl Default for CaptureHold {
    fn default() -> Self {
        Self::new()
    }
}

/// The process-wide hold shared by desktop captures and the X11 overlay owner.
pub static CAPTURE_HOLD: CaptureHold = CaptureHold::new();

/// Replace overlay pixels still present in `rgba` (row-major RGBA of the whole
/// root, `width * height * 4`) with the desktop recorded under them.
///
/// A patch only speaks for pixels it actually painted (`uploaded != under`).
/// For those, the newest patch decides: if the captured pixel still equals
/// what that patch uploaded, the window below never repainted and the pixel is
/// replaced with `under`; otherwise the window repainted and the capture is
/// the truth. Pixels a patch did not paint fall through to older patches.
///
/// Returns how many pixels were replaced.
pub fn scrub_residual_overlay(
    rgba: &mut [u8],
    width: usize,
    height: usize,
    patches: &[ResidualPatch],
) -> usize {
    if rgba.len() != width.saturating_mul(height).saturating_mul(4) {
        return 0;
    }
    let mut decided = vec![false; width * height];
    let mut replaced = 0;
    for patch in patches {
        let expected = patch.width.saturating_mul(patch.height).saturating_mul(4);
        if patch.under.len() != expected || patch.uploaded.len() != expected {
            continue;
        }
        for row in 0..patch.height {
            let Some(y) = offset(patch.y, row, height) else {
                continue;
            };
            for column in 0..patch.width {
                let Some(x) = offset(patch.x, column, width) else {
                    continue;
                };
                let source = (row * patch.width + column) * 4;
                let uploaded = &patch.uploaded[source..source + 3];
                let under = &patch.under[source..source + 3];
                if uploaded == under {
                    continue;
                }
                let index = y * width + x;
                if decided[index] {
                    continue;
                }
                decided[index] = true;
                let target = index * 4;
                let captured = &rgba[target..target + 3];
                // BGRX patch vs RGBA capture.
                if captured[0] == uploaded[2]
                    && captured[1] == uploaded[1]
                    && captured[2] == uploaded[0]
                {
                    rgba[target] = under[2];
                    rgba[target + 1] = under[1];
                    rgba[target + 2] = under[0];
                    replaced += 1;
                }
            }
        }
    }
    replaced
}

fn offset(origin: i32, delta: usize, limit: usize) -> Option<usize> {
    let value = i64::from(origin) + i64::try_from(delta).ok()?;
    usize::try_from(value).ok().filter(|value| *value < limit)
}

/// Excluder used by the Linux `get_desktop_state`.
pub struct OverlayExcluder;

impl OverlayCaptureExcluder for OverlayExcluder {
    type Hidden = HiddenOverlay;

    fn exclude(&self) -> ExclusionStart<HiddenOverlay> {
        if let Some(reason) = crate::overlay::wayland_capture_limitation() {
            return ExclusionStart::Unsupported { reason };
        }
        if !crate::overlay::x11_overlay_live() {
            return ExclusionStart::NotPresent;
        }
        let generation = CAPTURE_HOLD.request();
        crate::overlay::wake_x11_overlay();
        match CAPTURE_HOLD.wait_answer(generation, HOLD_ACK_TIMEOUT) {
            Some(HoldAnswer::Hidden(hidden)) => {
                if hidden.was_shown {
                    std::thread::sleep(HIDE_SETTLE);
                }
                ExclusionStart::Excluded {
                    method: if hidden.composited {
                        X11_COMPOSITED_METHOD
                    } else {
                        X11_METHOD
                    },
                    hidden,
                }
            }
            Some(HoldAnswer::Failed(error)) => {
                release_hold();
                ExclusionStart::Unsupported {
                    reason: format!("the X11 agent cursor overlay could not be hidden: {error}"),
                }
            }
            None => {
                release_hold();
                ExclusionStart::Unsupported {
                    reason: format!(
                        "the X11 agent cursor overlay did not hide within {} ms",
                        HOLD_ACK_TIMEOUT.as_millis()
                    ),
                }
            }
        }
    }

    fn restore(&self, _hidden: HiddenOverlay) {
        release_hold();
    }
}

fn release_hold() {
    CAPTURE_HOLD.release();
    crate::overlay::wake_x11_overlay();
}

/// Inspect a PNG captured while the overlay was hidden, scrubbing any pixels
/// the overlay's last frame left behind. Returns the (possibly re-encoded) PNG
/// and the verdict.
#[cfg(target_os = "linux")]
pub fn verify_hidden_capture(
    png: Vec<u8>,
    hidden: Option<&HiddenOverlay>,
) -> (Vec<u8>, cursor_overlay::capture_exclusion::ResidualCheck) {
    use cursor_overlay::capture_exclusion::ResidualCheck;
    let Some(hidden) = hidden.filter(|hidden| !hidden.residual.is_empty()) else {
        return (png, ResidualCheck::Clean);
    };
    let decoded = match image::load_from_memory(&png) {
        Ok(decoded) => decoded.to_rgba8(),
        Err(error) => {
            return (
                png,
                ResidualCheck::Residual {
                    reason: format!(
                        "the desktop capture could not be checked for leftover overlay pixels: \
                         {error}"
                    ),
                },
            )
        }
    };
    let (width, height) = decoded.dimensions();
    let mut rgba = decoded.into_raw();
    let replaced =
        scrub_residual_overlay(&mut rgba, width as usize, height as usize, &hidden.residual);
    if replaced == 0 {
        return (png, ResidualCheck::Clean);
    }
    match cua_driver_core::image_utils::encode_rgba_to_png(&rgba, width, height) {
        Ok(repaired) => (
            repaired,
            ResidualCheck::Repaired {
                method: X11_SAVE_UNDER_REPAIR,
            },
        ),
        Err(error) => (
            png,
            ResidualCheck::Residual {
                reason: format!(
                    "{replaced} leftover overlay pixels were found but the repaired capture \
                     could not be encoded: {error}"
                ),
            },
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bgrx(pixels: &[[u8; 3]]) -> Vec<u8> {
        pixels
            .iter()
            .flat_map(|[r, g, b]| [*b, *g, *r, 0])
            .collect()
    }

    fn rgba(pixels: &[[u8; 3]]) -> Vec<u8> {
        pixels
            .iter()
            .flat_map(|[r, g, b]| [*r, *g, *b, 255])
            .collect()
    }

    const DESK: [u8; 3] = [200, 200, 200];
    const CURSOR: [u8; 3] = [120, 60, 220];
    const REPAINTED: [u8; 3] = [10, 120, 10];

    #[test]
    fn leftover_overlay_pixels_are_replaced_with_the_recorded_desktop() {
        // 3x1 screen; the patch painted the middle pixel.
        let mut capture = rgba(&[DESK, CURSOR, DESK]);
        let patch = ResidualPatch {
            x: 0,
            y: 0,
            width: 3,
            height: 1,
            under: bgrx(&[DESK, DESK, DESK]),
            uploaded: bgrx(&[DESK, CURSOR, DESK]),
        };
        assert_eq!(scrub_residual_overlay(&mut capture, 3, 1, &[patch]), 1);
        assert_eq!(capture, rgba(&[DESK, DESK, DESK]));
    }

    #[test]
    fn a_repainted_pixel_is_kept() {
        let mut capture = rgba(&[REPAINTED]);
        let patch = ResidualPatch {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            under: bgrx(&[DESK]),
            uploaded: bgrx(&[CURSOR]),
        };
        assert_eq!(scrub_residual_overlay(&mut capture, 1, 1, &[patch]), 0);
        assert_eq!(capture, rgba(&[REPAINTED]));
    }

    #[test]
    fn the_newest_painting_patch_decides_and_unpainted_pixels_fall_through() {
        // Newest patch painted pixel 0 only; pixel 1 is a backdrop copy there
        // (uploaded == under), so the older patch that painted pixel 1 speaks.
        let newest = ResidualPatch {
            x: 0,
            y: 0,
            width: 2,
            height: 1,
            under: bgrx(&[DESK, DESK]),
            uploaded: bgrx(&[CURSOR, DESK]),
        };
        let older = ResidualPatch {
            x: 0,
            y: 0,
            width: 2,
            height: 1,
            under: bgrx(&[DESK, DESK]),
            uploaded: bgrx(&[REPAINTED, CURSOR]),
        };
        // Pixel 0 shows the older frame's paint, which the newest frame
        // painted over: after the hide the window must have repainted it.
        let mut capture = rgba(&[REPAINTED, CURSOR]);
        assert_eq!(
            scrub_residual_overlay(&mut capture, 2, 1, &[newest, older]),
            1
        );
        assert_eq!(capture, rgba(&[REPAINTED, DESK]));
    }

    #[test]
    fn patches_are_clipped_to_the_screen() {
        let mut capture = rgba(&[CURSOR, DESK]);
        let patch = ResidualPatch {
            x: -1,
            y: 0,
            width: 2,
            height: 2,
            under: bgrx(&[DESK, DESK, DESK, DESK]),
            uploaded: bgrx(&[CURSOR, CURSOR, CURSOR, CURSOR]),
        };
        assert_eq!(scrub_residual_overlay(&mut capture, 2, 1, &[patch]), 1);
        assert_eq!(capture, rgba(&[DESK, DESK]));
    }

    #[test]
    fn malformed_inputs_change_nothing() {
        let mut capture = rgba(&[CURSOR]);
        let short = ResidualPatch {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            under: vec![0; 3],
            uploaded: bgrx(&[CURSOR]),
        };
        assert_eq!(scrub_residual_overlay(&mut capture, 1, 1, &[short]), 0);
        assert_eq!(scrub_residual_overlay(&mut capture, 2, 2, &[]), 0);
        assert_eq!(capture, rgba(&[CURSOR]));
    }

    #[test]
    fn hold_answers_reach_only_the_current_request() {
        let hold = CaptureHold::new();
        assert_eq!(hold.unanswered_request(), None);
        let first = hold.request();
        assert_eq!(hold.unanswered_request(), Some(first));
        assert!(hold.is_requested());
        hold.answer(first, HoldAnswer::Hidden(HiddenOverlay::default()));
        assert_eq!(hold.unanswered_request(), None);
        assert_eq!(
            hold.wait_answer(first, Duration::ZERO),
            Some(HoldAnswer::Hidden(HiddenOverlay::default()))
        );
        hold.release();
        assert!(!hold.is_requested());

        // An answer that arrives after the capture gave up is dropped.
        let second = hold.request();
        hold.release();
        hold.answer(second, HoldAnswer::Failed("late".into()));
        assert_eq!(hold.wait_answer(second, Duration::ZERO), None);

        // An answer for an older request never satisfies a newer one.
        let third = hold.request();
        hold.answer(second, HoldAnswer::Failed("stale".into()));
        assert_eq!(hold.wait_answer(third, Duration::ZERO), None);
        assert_eq!(hold.unanswered_request(), Some(third));
    }

    #[test]
    fn hold_wait_wakes_when_the_owner_answers_from_another_thread() {
        let hold = std::sync::Arc::new(CaptureHold::new());
        let generation = hold.request();
        let owner = {
            let hold = std::sync::Arc::clone(&hold);
            std::thread::spawn(move || {
                while hold.unanswered_request().is_none() {
                    std::thread::yield_now();
                }
                hold.answer(generation, HoldAnswer::Failed("owner".into()));
            })
        };
        assert_eq!(
            hold.wait_answer(generation, Duration::from_secs(5)),
            Some(HoldAnswer::Failed("owner".into()))
        );
        owner.join().unwrap();
    }
}

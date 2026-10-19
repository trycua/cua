//! What cursor shape the guest would show at an arbitrary point, for
//! multiplayer presence.
//!
//! [`crate::cursor_shape`] answers "what is the OS drawing right now", which
//! only describes the one real pointer. A shared desktop has many
//! participants, each hovering somewhere else, and only one of them (the one
//! whose input is being injected) owns the real pointer. This module answers
//! the question for every other point, three ways, cheapest first:
//!
//! 1. **Hit-test** ([`PointerShapeBackend::hit_test`]): the accessibility
//!    element under the point and its role (text field -> I-beam, link or
//!    button -> hand) plus window edges (-> resize). Never moves the pointer.
//! 2. **System** ([`PointerShapeBackend::system_shape`]): the real cursor,
//!    exact, but only meaningful for the participant whose input put the
//!    pointer where it is.
//! 3. **Probe** ([`probe_by_warp`]): while the real pointer is idle, move it
//!    to the point, read the real cursor, and put it back exactly. Exact
//!    (app-specific cursors included) at the cost of a brief hover.
//!
//! Each platform crate installs one [`PointerShapeBackend`] with
//! [`set_pointer_shape_backend`]; the vocabulary, the edge geometry and the
//! probe's idle gating and restore live here so they are shared and tested
//! once with a fake backend.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::cursor_shape::{ResizeAxis, SystemCursorShape};

/// A rectangle in the platform's global screen space (the space
/// [`PointerShapeBackend`] coordinates use: pixels on Linux and Windows,
/// points on macOS).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl ScreenRect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Whether `(x, y)` is inside (right and bottom edges exclusive).
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.width && y < self.y + self.height
    }

    /// Grown by `by` on every side.
    pub fn inflate(&self, by: f64) -> Self {
        Self::new(
            self.x - by,
            self.y - by,
            self.width + 2.0 * by,
            self.height + 2.0 * by,
        )
    }

    pub fn area(&self) -> f64 {
        self.width.max(0.0) * self.height.max(0.0)
    }
}

/// What an accessibility hit-test found under a point.
#[derive(Debug, Clone, PartialEq)]
pub struct HitTest {
    /// The shape the element implies. `Default` (arrow) when the element
    /// implies nothing in particular.
    pub shape: SystemCursorShape,
    /// The platform role that decided it (for example "terminal", "AXLink",
    /// "Hyperlink"), "edge" for a window edge, empty when nothing was hit.
    pub role: String,
    /// The top-level window under the point, when known.
    pub window: Option<ScreenRect>,
    /// The element that decided the shape, when known. Every point inside it
    /// gets the same answer, so callers may cache by it.
    pub element: Option<ScreenRect>,
}

impl HitTest {
    /// Nothing under the point (desktop background): the arrow.
    pub fn nothing() -> Self {
        Self {
            shape: SystemCursorShape::Default,
            role: String::new(),
            window: None,
            element: None,
        }
    }
}

/// Which mechanisms a backend has, by name, for capability reporting. Empty
/// means "not available".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BackendNames {
    /// Accessibility hit-test: "atspi", "ax", "uia", or "".
    pub hit_test: &'static str,
    /// Real cursor readout: "xfixes", "nscursor", "getcursorinfo", or "".
    pub system: &'static str,
    /// Pointer warp for [`probe_by_warp`]: "xtest", "cgwarp", "setcursorpos",
    /// or "".
    pub probe: &'static str,
}

/// One platform's pointer-shape mechanisms. Coordinates are global screen
/// coordinates in the platform's native unit (see [`ScreenRect`]).
///
/// Implementations must be cheap to call repeatedly and must never click,
/// press or scroll.
pub trait PointerShapeBackend: Send + Sync {
    /// What this backend can do.
    fn names(&self) -> BackendNames;

    /// The shape implied by what is under `(x, y)`, without moving the
    /// pointer. `None` when the hit-test is unavailable or failed (callers
    /// fall back to the arrow); `Some(HitTest::nothing())` when it worked and
    /// found nothing.
    fn hit_test(&self, x: f64, y: f64) -> Option<HitTest>;

    /// The real cursor the OS is drawing now.
    fn system_shape(&self) -> SystemCursorShape {
        crate::cursor_shape::current_system_cursor_shape()
    }

    /// Where the real pointer is.
    fn pointer_position(&self) -> Option<(f64, f64)>;

    /// Move the real pointer to `(x, y)` with no button or key state change.
    /// Returns false when this platform cannot (for example Wayland without
    /// a virtual-pointer protocol).
    fn warp_pointer(&self, x: f64, y: f64) -> bool;

    /// One sentence on what this backend cannot do here, for the capability's
    /// `limitation`. `None` when fully supported.
    fn limitation(&self) -> Option<String> {
        None
    }
}

static BACKEND: OnceLock<Box<dyn PointerShapeBackend>> = OnceLock::new();

/// Install the platform backend. Call once from the platform adapter; later
/// calls are ignored (returns false), matching `set_cursor_shape_probe`.
pub fn set_pointer_shape_backend(backend: impl PointerShapeBackend + 'static) -> bool {
    BACKEND.set(Box::new(backend)).is_ok()
}

/// The installed backend, if any.
pub fn pointer_shape_backend() -> Option<&'static dyn PointerShapeBackend> {
    BACKEND.get().map(|b| b.as_ref())
}

/// The resize shape for a point near the edge of a window frame, or `None`
/// when the point is not in the resize band.
///
/// The band extends `outside` pixels outside the frame (where most window
/// managers put invisible resize borders) and `inside` pixels inside it.
/// Corners win over edges within `corner` pixels of a corner along either
/// axis.
pub fn edge_resize_shape(
    frame: ScreenRect,
    x: f64,
    y: f64,
    inside: f64,
    outside: f64,
    corner: f64,
) -> Option<SystemCursorShape> {
    if frame.width <= 0.0 || frame.height <= 0.0 {
        return None;
    }
    let outer = frame.inflate(outside);
    if !outer.contains(x, y) {
        return None;
    }
    let left = x < frame.x + inside;
    let right = x >= frame.x + frame.width - inside;
    let top = y < frame.y + inside;
    let bottom = y >= frame.y + frame.height - inside;
    if !(left || right || top || bottom) {
        return None;
    }
    // Corner zones: near a vertical edge and within `corner` of a horizontal
    // one (or the other way round).
    let near_top = y < frame.y + corner;
    let near_bottom = y >= frame.y + frame.height - corner;
    let near_left = x < frame.x + corner;
    let near_right = x >= frame.x + frame.width - corner;
    let falling = (left && near_top)
        || (top && near_left)
        || (right && near_bottom)
        || (bottom && near_right);
    let rising = (right && near_top)
        || (top && near_right)
        || (left && near_bottom)
        || (bottom && near_left);
    let axis = if falling {
        ResizeAxis::NorthWestSouthEast
    } else if rising {
        ResizeAxis::NorthEastSouthWest
    } else if left || right {
        ResizeAxis::EastWest
    } else {
        ResizeAxis::NorthSouth
    };
    Some(SystemCursorShape::Resize(axis))
}

/// Tunables for [`probe_by_warp`].
#[derive(Debug, Clone, Copy)]
pub struct ProbeConfig {
    /// How long the pointer rests at the target before the cursor is read.
    /// Long enough for the app under it to set its cursor, short enough that
    /// hover effects (tooltips wait ~500 ms) never trigger.
    pub dwell: Duration,
    /// How often idleness is rechecked during the dwell.
    pub poll: Duration,
    /// How far a restored pointer may be from where it was (rounding).
    pub restore_tolerance: f64,
}

impl Default for ProbeConfig {
    fn default() -> Self {
        Self {
            dwell: Duration::from_millis(40),
            poll: Duration::from_millis(5),
            restore_tolerance: 1.0,
        }
    }
}

/// How a [`probe_by_warp`] ended.
#[derive(Debug, Clone, PartialEq)]
pub enum ProbeOutcome {
    /// The real cursor at the target.
    Shape(SystemCursorShape),
    /// Input or an agent action started (or was pending), so the probe did
    /// not run or was cut short. The pointer was restored if it had moved.
    Aborted,
    /// The platform cannot read or move the pointer. Nothing moved.
    Unsupported,
}

/// Read the real cursor shape at `target` by moving the idle pointer there
/// and back.
///
/// `still_idle` is asked before moving, repeatedly during the dwell, and
/// before trusting the reading; the first `false` aborts. The pointer always
/// ends where it started, unless restoring is impossible (a backend whose
/// warp fails on the way back); the caller should then stop probing.
///
/// The caller must serialize this with real input injection (hold the same
/// lock injection takes), so "abort and restore" never fights a real move.
pub fn probe_by_warp(
    backend: &dyn PointerShapeBackend,
    target: (f64, f64),
    config: ProbeConfig,
    still_idle: &dyn Fn() -> bool,
) -> ProbeOutcome {
    if backend.names().probe.is_empty() || backend.names().system.is_empty() {
        return ProbeOutcome::Unsupported;
    }
    if !still_idle() {
        return ProbeOutcome::Aborted;
    }
    let Some(home) = backend.pointer_position() else {
        return ProbeOutcome::Unsupported;
    };
    let restore = |backend: &dyn PointerShapeBackend| {
        backend.warp_pointer(home.0, home.1);
        // One retry when a platform rounds or drops the first move.
        if let Some((x, y)) = backend.pointer_position() {
            if (x - home.0).abs() > config.restore_tolerance
                || (y - home.1).abs() > config.restore_tolerance
            {
                backend.warp_pointer(home.0, home.1);
            }
        }
    };
    if !backend.warp_pointer(target.0, target.1) {
        // A partial move is still a move: put it back.
        restore(backend);
        return ProbeOutcome::Unsupported;
    }
    let started = Instant::now();
    while started.elapsed() < config.dwell {
        if !still_idle() {
            restore(backend);
            return ProbeOutcome::Aborted;
        }
        std::thread::sleep(
            config
                .poll
                .min(config.dwell.saturating_sub(started.elapsed())),
        );
    }
    let shape = backend.system_shape();
    restore(backend);
    if !still_idle() {
        return ProbeOutcome::Aborted;
    }
    match shape {
        SystemCursorShape::Unknown => ProbeOutcome::Unsupported,
        shape => ProbeOutcome::Shape(shape),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    /// A pointer on a fake screen: a text field at x < 100, a link beyond.
    struct Fake {
        pointer: Mutex<(f64, f64)>,
        moves: Mutex<Vec<(f64, f64)>>,
        can_warp: bool,
        drop_first_restore: Mutex<bool>,
    }

    impl Fake {
        fn new(can_warp: bool) -> Self {
            Self {
                pointer: Mutex::new((500.0, 500.0)),
                moves: Mutex::new(Vec::new()),
                can_warp,
                drop_first_restore: Mutex::new(false),
            }
        }
    }

    impl PointerShapeBackend for Fake {
        fn names(&self) -> BackendNames {
            BackendNames {
                hit_test: "fake",
                system: "fake",
                probe: if self.can_warp { "fake" } else { "" },
            }
        }
        fn hit_test(&self, _x: f64, _y: f64) -> Option<HitTest> {
            Some(HitTest::nothing())
        }
        fn system_shape(&self) -> SystemCursorShape {
            let (x, _) = *self.pointer.lock().unwrap();
            if x < 100.0 {
                SystemCursorShape::Text
            } else if x < 200.0 {
                SystemCursorShape::Pointer
            } else {
                SystemCursorShape::Default
            }
        }
        fn pointer_position(&self) -> Option<(f64, f64)> {
            Some(*self.pointer.lock().unwrap())
        }
        fn warp_pointer(&self, x: f64, y: f64) -> bool {
            if !self.can_warp {
                return false;
            }
            self.moves.lock().unwrap().push((x, y));
            let mut drop = self.drop_first_restore.lock().unwrap();
            if *drop && (x, y) == (500.0, 500.0) {
                *drop = false;
                return true;
            }
            *self.pointer.lock().unwrap() = (x, y);
            true
        }
    }

    fn fast() -> ProbeConfig {
        ProbeConfig {
            dwell: Duration::from_millis(10),
            poll: Duration::from_millis(1),
            restore_tolerance: 0.5,
        }
    }

    #[test]
    fn probe_reads_the_shape_at_the_target_and_restores_exactly() {
        let fake = Fake::new(true);
        let out = probe_by_warp(&fake, (50.0, 10.0), fast(), &|| true);
        assert_eq!(out, ProbeOutcome::Shape(SystemCursorShape::Text));
        assert_eq!(fake.pointer_position(), Some((500.0, 500.0)));
        assert_eq!(
            *fake.moves.lock().unwrap(),
            vec![(50.0, 10.0), (500.0, 500.0)]
        );
    }

    #[test]
    fn probe_never_moves_when_not_idle() {
        let fake = Fake::new(true);
        let out = probe_by_warp(&fake, (50.0, 10.0), fast(), &|| false);
        assert_eq!(out, ProbeOutcome::Aborted);
        assert!(fake.moves.lock().unwrap().is_empty());
    }

    #[test]
    fn input_during_the_dwell_aborts_and_restores() {
        let fake = Fake::new(true);
        let calls = AtomicUsize::new(0);
        // Idle for the first check, then input arrives.
        let out = probe_by_warp(&fake, (150.0, 10.0), fast(), &|| {
            calls.fetch_add(1, Ordering::SeqCst) == 0
        });
        assert_eq!(out, ProbeOutcome::Aborted);
        assert_eq!(fake.pointer_position(), Some((500.0, 500.0)));
    }

    #[test]
    fn a_dropped_restore_is_retried() {
        let fake = Fake::new(true);
        *fake.drop_first_restore.lock().unwrap() = true;
        let out = probe_by_warp(&fake, (150.0, 10.0), fast(), &|| true);
        assert_eq!(out, ProbeOutcome::Shape(SystemCursorShape::Pointer));
        assert_eq!(fake.pointer_position(), Some((500.0, 500.0)));
    }

    #[test]
    fn no_warp_means_unsupported_and_no_move() {
        let fake = Fake::new(false);
        let out = probe_by_warp(&fake, (50.0, 10.0), fast(), &|| true);
        assert_eq!(out, ProbeOutcome::Unsupported);
        assert_eq!(fake.pointer_position(), Some((500.0, 500.0)));
    }

    #[test]
    fn edges_map_to_resize_axes() {
        let frame = ScreenRect::new(100.0, 100.0, 400.0, 300.0);
        let at = |x, y| edge_resize_shape(frame, x, y, 3.0, 6.0, 12.0);
        let axis = |a| Some(SystemCursorShape::Resize(a));
        assert_eq!(at(300.0, 250.0), None, "interior");
        assert_eq!(at(50.0, 250.0), None, "far outside");
        assert_eq!(at(97.0, 250.0), axis(ResizeAxis::EastWest), "left, outside");
        assert_eq!(at(101.0, 250.0), axis(ResizeAxis::EastWest), "left, inside");
        assert_eq!(at(503.0, 250.0), axis(ResizeAxis::EastWest), "right");
        assert_eq!(at(300.0, 96.0), axis(ResizeAxis::NorthSouth), "top");
        assert_eq!(at(300.0, 402.0), axis(ResizeAxis::NorthSouth), "bottom");
        assert_eq!(
            at(97.0, 97.0),
            axis(ResizeAxis::NorthWestSouthEast),
            "top-left"
        );
        assert_eq!(
            at(503.0, 403.0),
            axis(ResizeAxis::NorthWestSouthEast),
            "bottom-right"
        );
        assert_eq!(
            at(503.0, 97.0),
            axis(ResizeAxis::NorthEastSouthWest),
            "top-right"
        );
        assert_eq!(
            at(97.0, 403.0),
            axis(ResizeAxis::NorthEastSouthWest),
            "bottom-left"
        );
        assert_eq!(
            at(98.0, 108.0),
            axis(ResizeAxis::NorthWestSouthEast),
            "left edge near top corner"
        );
    }

    #[test]
    fn rects() {
        let r = ScreenRect::new(0.0, 0.0, 10.0, 10.0);
        assert!(r.contains(0.0, 9.9));
        assert!(!r.contains(10.0, 5.0));
        assert!(r.inflate(2.0).contains(-2.0, 11.0));
        assert_eq!(r.area(), 100.0);
    }
}

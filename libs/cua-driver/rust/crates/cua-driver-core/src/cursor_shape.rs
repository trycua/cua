//! What the operating system is currently drawing as the pointer.
//!
//! This is the *system* cursor — the I-beam over a text field, the resize
//! cursor on a window edge, the pointing hand over a link. It is distinct from
//! the agent-cursor overlay (`cursor_overlay`), which draws Cua's own cursor,
//! and from [`crate::cursor_hook`], which reports where a cursor *is*. A remote
//! viewer needs both: position says where to draw, shape says what to draw.
//!
//! ## Shape of this module
//!
//! Per the cross-platform contract, the vocabulary and the dispatch live here
//! in the common crate and each platform adapter stays thin: an adapter
//! installs a probe with [`set_cursor_shape_probe`], and everything else —
//! the enum, the default, the "nobody answered" behaviour — is shared.
//!
//! A platform that cannot read the system cursor simply never installs a
//! probe, and [`current_system_cursor_shape`] reports
//! [`SystemCursorShape::Unknown`]. `Unknown` is deliberately distinct from
//! [`SystemCursorShape::Default`]: it means "this host cannot tell you", not
//! "the pointer is an arrow", so a consumer can publish the limitation instead
//! of rendering a confidently wrong arrow.

use std::sync::OnceLock;

/// A platform-neutral system cursor shape.
///
/// Intentionally a small closed vocabulary plus a `Custom` escape. Consumers
/// map each variant onto their own native cursor, which keeps the pointer
/// crisp at any scale and lets the viewer honour its own accessibility
/// settings. Only a cursor with no portable equivalent needs to ship pixels.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum SystemCursorShape {
    /// The ordinary arrow.
    #[default]
    Default,
    /// Text insertion.
    Text,
    /// Vertical-text insertion.
    VerticalText,
    /// A link or other clickable affordance.
    Pointer,
    /// Open hand over a draggable surface.
    Grab,
    /// Closed hand, drag in progress.
    Grabbing,
    /// Crosshair.
    Crosshair,
    /// Busy.
    Wait,
    /// The action is not permitted here.
    NotAllowed,
    /// A resize affordance on the given edge or corner.
    Resize(ResizeAxis),
    /// A cursor with no portable equivalent. `png` is the cursor image,
    /// `hotspot` is in that image's pixel space, `scale` is device pixels per
    /// point.
    Custom {
        png: Vec<u8>,
        hotspot_x: f64,
        hotspot_y: f64,
        scale: f64,
    },
    /// This host cannot report the system cursor shape. Not the same as
    /// `Default`.
    Unknown,
}

/// Which edge or corner a [`SystemCursorShape::Resize`] refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeAxis {
    NorthSouth,
    EastWest,
    NorthEastSouthWest,
    NorthWestSouthEast,
    All,
    Column,
    Row,
}

type ProbeFn = Box<dyn Fn() -> SystemCursorShape + Send + Sync>;
static PROBE: OnceLock<ProbeFn> = OnceLock::new();

/// Install the platform probe. Call once, from the platform adapter's tool
/// registration. Subsequent calls are ignored, matching `set_cursor_hook_fn`.
pub fn set_cursor_shape_probe(probe: impl Fn() -> SystemCursorShape + Send + Sync + 'static) {
    let _ = PROBE.set(Box::new(probe));
}

/// Whether this build can report the system cursor shape at all.
///
/// Callers should use this to publish the limitation explicitly rather than
/// letting `Unknown` be mistaken for a transient read failure.
pub fn cursor_shape_supported() -> bool {
    PROBE.get().is_some()
}

/// The shape the OS is drawing right now, or [`SystemCursorShape::Unknown`]
/// when this platform installed no probe.
pub fn current_system_cursor_shape() -> SystemCursorShape {
    match PROBE.get() {
        Some(probe) => probe(),
        None => SystemCursorShape::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The distinction the whole module rests on: an unsupported platform must
    /// report `Unknown`, never `Default`. Reporting `Default` would render a
    /// confident arrow on a host that has no idea what the pointer looks like.
    #[test]
    fn unknown_is_not_default() {
        assert_ne!(SystemCursorShape::Unknown, SystemCursorShape::Default);
        assert_eq!(SystemCursorShape::default(), SystemCursorShape::Default);
    }

    /// With no probe installed the accessor must say so rather than guess.
    /// This runs in a fresh test process for platforms whose adapter is not
    /// linked into this crate's tests (which is all of them: core never
    /// depends on a platform crate).
    #[test]
    fn no_probe_reports_unknown_and_unsupported() {
        assert!(!cursor_shape_supported());
        assert_eq!(current_system_cursor_shape(), SystemCursorShape::Unknown);
    }
}

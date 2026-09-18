//! macOS adapter for [`cua_driver_core::cursor_shape`].
//!
//! Thin by design: the vocabulary, the dispatch and the "unsupported" story
//! live in the common crate. All this file does is read AppKit's current
//! system cursor and classify it.
//!
//! ## How the classification works, and why it is not pointer equality
//!
//! `+[NSCursor currentSystemCursor]` builds a *new* `NSCursor` describing
//! whatever the Window Server is drawing. The standard cursors
//! (`+[NSCursor IBeamCursor]` and friends) are cached singletons. So the
//! obvious `currentSystemCursor == IBeamCursor` pointer test is always false
//! and would silently report `Default` for every shape — the exact
//! "check that cannot fail" trap. There is a unit test below pinning that.
//!
//! Instead each standard cursor is fingerprinted once by its hot spot and its
//! image's TIFF bytes, and the current cursor is matched against that table.
//! An unmatched cursor is reported as `Custom` with its own PNG, so a bespoke
//! application cursor still reaches the viewer rather than degrading to an
//! arrow.

use std::collections::HashMap;
use std::sync::OnceLock;

use cua_driver_core::cursor_shape::{ResizeAxis, SystemCursorShape};
use objc2::rc::Retained;
use objc2_app_kit::{NSCursor, NSImage};
use objc2_foundation::NSData;

/// A cursor's identity: hot spot rounded to whole pixels plus a hash of its
/// image bytes. Two cursors with the same fingerprint are the same cursor.
type Fingerprint = (i64, i64, u64);

fn hash_bytes(bytes: &[u8]) -> u64 {
    // FNV-1a. The table is tiny and built once; a cryptographic hash would be
    // pointless here and a collision only mislabels one cursor.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Fetch one standard cursor, or `None` if AppKit will not supply it.
///
/// `+[NSCursor currentSystemCursor]` reads Window Server state and works
/// early, but the standard singletons (`arrowCursor` and friends) stay NULL
/// until an `NSApplication` exists -- the two are NOT equivalent, and gating
/// the singletons on `currentSystemCursor` aborts the process. objc2's
/// generated accessors `expect()` non-NULL, so this goes through a raw
/// `msg_send!` and null-checks the result itself.
macro_rules! standard_cursor {
    ($selector:ident) => {{
        let raw: *mut objc2::runtime::AnyObject =
            unsafe { objc2::msg_send![objc2::class!(NSCursor), $selector] };
        if raw.is_null() {
            None
        } else {
            unsafe { Retained::retain(raw.cast::<NSCursor>()) }
        }
    }};
}

/// True when AppKit will hand out the standard cursor singletons, which is
/// what the fingerprint table is built from.
pub fn appkit_cursors_available() -> bool {
    standard_cursor!(arrowCursor).is_some()
}

/// `-[NSCursor image]` as an `Option`.
///
/// objc2's generated `image()` accessor `expect()`s a non-NULL result and so
/// aborts the process if AppKit hands back nil -- which it does for a cursor
/// built from a Window Server description that carries no image. Every read of
/// the image goes through here so a missing image degrades to `Unknown` or
/// `None`, never a crash in the embedder.
fn cursor_image(cursor: &NSCursor) -> Option<Retained<NSImage>> {
    unsafe { objc2::msg_send_id![cursor, image] }
}

/// `-[NSImage TIFFRepresentation]` as an `Option`. Nil for an image with no
/// bitmap representation.
fn tiff_data(image: &NSImage) -> Option<Retained<NSData>> {
    unsafe { objc2::msg_send_id![image, TIFFRepresentation] }
}

fn tiff_bytes(cursor: &NSCursor) -> Option<Vec<u8>> {
    let image = cursor_image(cursor)?;
    let data = tiff_data(&image)?;
    Some(unsafe { data.bytes() }.to_vec())
}

fn fingerprint(cursor: &NSCursor) -> Option<Fingerprint> {
    let bytes = tiff_bytes(cursor)?;
    let spot = unsafe { cursor.hotSpot() };
    Some((
        spot.x.round() as i64,
        spot.y.round() as i64,
        hash_bytes(&bytes),
    ))
}

/// The standard-cursor fingerprint table, built once on first use.
///
/// Built from AppKit's own accessors rather than from hardcoded bytes, so it
/// tracks whatever the running OS version actually draws (cursor artwork has
/// changed across releases) and across accessibility cursor-size settings, as
/// long as the size does not change mid-process.
fn standard_table() -> &'static HashMap<Fingerprint, SystemCursorShape> {
    static TABLE: OnceLock<HashMap<Fingerprint, SystemCursorShape>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = HashMap::new();
        // Each entry: an AppKit accessor and the neutral shape it means.
        // `resizeUpDown`/`resizeLeftRight` are the portable resize cursors;
        // the diagonal ones are not public API on macOS, so a window-corner
        // resize falls through to `Custom` and still renders correctly.
        // Nothing to fingerprint until AppKit is up. Leaving the table
        // empty here is what makes `current_shape` report Unknown rather than
        // mislabelling every cursor as Custom.
        if !appkit_cursors_available() {
            return table;
        }
        let entries: Vec<(Option<Retained<NSCursor>>, SystemCursorShape)> = vec![
            (standard_cursor!(arrowCursor), SystemCursorShape::Default),
            (standard_cursor!(IBeamCursor), SystemCursorShape::Text),
            (
                standard_cursor!(IBeamCursorForVerticalLayout),
                SystemCursorShape::VerticalText,
            ),
            (
                standard_cursor!(pointingHandCursor),
                SystemCursorShape::Pointer,
            ),
            (standard_cursor!(openHandCursor), SystemCursorShape::Grab),
            (
                standard_cursor!(closedHandCursor),
                SystemCursorShape::Grabbing,
            ),
            (
                standard_cursor!(crosshairCursor),
                SystemCursorShape::Crosshair,
            ),
            (
                standard_cursor!(operationNotAllowedCursor),
                SystemCursorShape::NotAllowed,
            ),
            (
                standard_cursor!(resizeUpDownCursor),
                SystemCursorShape::Resize(ResizeAxis::NorthSouth),
            ),
            (
                standard_cursor!(resizeLeftRightCursor),
                SystemCursorShape::Resize(ResizeAxis::EastWest),
            ),
            (
                standard_cursor!(resizeUpCursor),
                SystemCursorShape::Resize(ResizeAxis::NorthSouth),
            ),
            (
                standard_cursor!(resizeDownCursor),
                SystemCursorShape::Resize(ResizeAxis::NorthSouth),
            ),
            (
                standard_cursor!(resizeLeftCursor),
                SystemCursorShape::Resize(ResizeAxis::EastWest),
            ),
            (
                standard_cursor!(resizeRightCursor),
                SystemCursorShape::Resize(ResizeAxis::EastWest),
            ),
        ];
        for (cursor, shape) in entries {
            let Some(cursor) = cursor else { continue };
            if let Some(print) = fingerprint(&cursor) {
                // First writer wins: `resizeUpCursor` and `resizeDownCursor`
                // can share artwork with `resizeUpDownCursor`, and they all
                // mean the same neutral shape anyway.
                table.entry(print).or_insert(shape);
            }
        }
        table
    })
}

/// Read the shape the Window Server is currently drawing.
pub fn current_shape() -> SystemCursorShape {
    let Some(cursor) = (unsafe { NSCursor::currentSystemCursor() }) else {
        // No Window Server session, or AppKit declined to describe the
        // cursor. "I cannot tell" -- not "it is an arrow".
        return SystemCursorShape::Unknown;
    };
    let Some(print) = fingerprint(&cursor) else {
        return SystemCursorShape::Unknown;
    };
    if standard_table().is_empty() {
        // AppKit came up after the table was first built, or fingerprinting
        // failed wholesale. Reporting Custom for every cursor here would look
        // like it worked while being useless, so say Unknown.
        return SystemCursorShape::Unknown;
    }
    if let Some(shape) = standard_table().get(&print) {
        return shape.clone();
    }
    // An application-specific cursor. Ship its pixels so the viewer can draw
    // the real thing instead of falling back to an arrow.
    match custom_shape(&cursor) {
        Some(shape) => shape,
        None => SystemCursorShape::Unknown,
    }
}

fn custom_shape(cursor: &NSCursor) -> Option<SystemCursorShape> {
    unsafe {
        // Every step here can legitimately return nil -- a cursor with no
        // image, an image with no bitmap representation, bytes AppKit declines
        // to decode. Each is `Option` + `?` so an unclassifiable cursor falls
        // back to `Unknown` rather than aborting the host process. `custom_shape`
        // already returns `Option` for exactly this reason.
        let image = cursor_image(cursor)?;
        let tiff = tiff_data(&image)?;
        // NSBitmapImageRep -> PNG. NSBitmapImageFileTypePNG = 4.
        let rep: Option<Retained<objc2::runtime::AnyObject>> = objc2::msg_send_id![
            objc2::class!(NSBitmapImageRep),
            imageRepWithData: &*tiff
        ];
        let rep = rep?;
        let empty: Option<Retained<objc2_foundation::NSDictionary>> =
            objc2::msg_send_id![objc2::class!(NSDictionary), dictionary];
        let empty = empty?;
        let png: Option<Retained<NSData>> = objc2::msg_send_id![
            &*rep,
            representationUsingType: 4usize,
            properties: &*empty
        ];
        let png = png?.bytes().to_vec();
        let spot = cursor.hotSpot();
        let size = image.size();
        let pixels_wide: i64 = objc2::msg_send![&*rep, pixelsWide];
        // Device pixels per point. `size` is in points; `pixelsWide` is the
        // backing store, so their ratio is the cursor's scale.
        let scale = if size.width > 0.0 {
            pixels_wide as f64 / size.width
        } else {
            1.0
        };
        Some(SystemCursorShape::Custom {
            png,
            hotspot_x: spot.x * scale,
            hotspot_y: spot.y * scale,
            scale,
        })
    }
}

/// Install this adapter as the process-wide probe.
pub fn install() {
    cua_driver_core::cursor_shape::set_cursor_shape_probe(current_shape);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AppKit's cursor class methods return NULL until an `NSApplication`
    /// exists. Rust's test harness runs each test on a spawned thread with no
    /// AppKit, so the classification tests below cannot run here -- they are
    /// proven in-guest against the real daemon instead (see the commit
    /// message). What IS provable here, and what actually regressed during
    /// development, is that the probe DEGRADES rather than panics.
    ///
    /// This test is meaningful precisely because AppKit is unavailable: the
    /// first implementation used objc2's generated accessors, which `expect()`
    /// a non-NULL result, and this exact call aborted the process.
    #[test]
    fn probe_degrades_to_unknown_without_appkit() {
        if appkit_cursors_available() {
            // An AppKit-hosted runner: then the probe must return a real
            // answer rather than Unknown, which is the opposite assertion.
            assert_ne!(current_shape(), SystemCursorShape::Unknown);
            return;
        }
        assert_eq!(current_shape(), SystemCursorShape::Unknown);
        assert!(standard_table().is_empty());
    }

    /// The fingerprint must discriminate: if arrow and I-beam hashed alike,
    /// every shape would collapse to one value and the probe would be a
    /// confident liar.
    ///
    /// `#[ignore]` because this needs a live `NSApplication`, which the Rust
    /// test harness does not provide. It is marked rather than early-returned
    /// so CI reports it as *ignored* instead of *passed*: a test that returns
    /// before its first assertion still prints "ok", which reads as coverage
    /// that does not exist. Run it on an AppKit-hosted machine with
    /// `cargo test -p platform-macos -- --ignored`.
    #[test]
    #[ignore = "requires a live NSApplication; run with --ignored on an AppKit host"]
    fn fingerprint_distinguishes_arrow_from_ibeam() {
        assert!(
            appkit_cursors_available(),
            "this test was run without AppKit; it can only assert anything \
             where the standard cursor singletons are available"
        );
        let (Some(arrow), Some(ibeam)) =
            (standard_cursor!(arrowCursor), standard_cursor!(IBeamCursor))
        else {
            return;
        };
        let arrow_print = fingerprint(&arrow).expect("arrow fingerprint");
        let ibeam_print = fingerprint(&ibeam).expect("ibeam fingerprint");
        assert_ne!(
            arrow_print, ibeam_print,
            "arrow and I-beam fingerprint identically; every shape would \
             collapse to one value"
        );
        assert_eq!(
            standard_table().get(&arrow_print),
            Some(&SystemCursorShape::Default)
        );
        assert_eq!(
            standard_table().get(&ibeam_print),
            Some(&SystemCursorShape::Text)
        );
    }
}

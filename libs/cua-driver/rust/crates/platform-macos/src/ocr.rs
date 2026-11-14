// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! On-device text recognition with Apple's Vision framework.
//!
//! Used by `get_window_state` for windows whose content is missing from the
//! accessibility tree (see [`cua_driver_core::opaque_text`]). Vision ships
//! with macOS, runs locally and needs no permission of its own: it reads the
//! screenshot the read already took.

use std::collections::HashMap;
use std::ffi::CStr;
use std::sync::Mutex;

use cua_driver_core::opaque_text::{content_is_opaque, NodeFacts, OcrFragment, OcrRow};
use objc2::rc::{autoreleasepool, Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, Bool};
use objc2::{msg_send, msg_send_id};
use objc2_foundation::{NSData, NSRect, NSString};

#[link(name = "Vision", kind = "framework")]
extern "C" {}

/// `VNRequestTextRecognitionLevelAccurate`.
const RECOGNITION_LEVEL_ACCURATE: isize = 0;

const VISION_FRAMEWORK: &CStr = c"/System/Library/Frameworks/Vision.framework/Vision";

fn vision_class(name: &str) -> anyhow::Result<&'static AnyClass> {
    if let Some(class) = AnyClass::get(name) {
        return Ok(class);
    }
    // The framework is linked, but load it explicitly in case the linker
    // dropped the reference (nothing names a Vision symbol directly).
    // SAFETY: a constant, NUL-terminated system framework path.
    unsafe { libc::dlopen(VISION_FRAMEWORK.as_ptr(), libc::RTLD_LAZY) };
    AnyClass::get(name).ok_or_else(|| anyhow::anyhow!("Vision class {name} is unavailable"))
}

/// A window screenshot read for text: the fragments, in the screenshot's own
/// pixels, and its luminance plane for
/// [`cua_driver_core::opaque_text::stamp_ink`].
pub struct TextPage {
    pub fragments: Vec<OcrFragment>,
    pub luma: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Images up to this long edge are upsampled 2x before recognition: Vision
/// misses most one-letter cells (a reconcile flag, a column of check marks)
/// at 1x window resolution and finds more of them at 2x.
const UPSAMPLE_MAX_EDGE: u32 = 2000;

/// Encode for Vision with the cheapest PNG settings: the bytes are decoded
/// once and dropped.
fn fast_png(image: &image::RgbImage) -> anyhow::Result<Vec<u8>> {
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    use image::ImageEncoder;
    let mut png = Vec::new();
    PngEncoder::new_with_quality(&mut png, CompressionType::Fast, FilterType::NoFilter)
        .write_image(
            image.as_raw(),
            image.width(),
            image.height(),
            image::ExtendedColorType::Rgb8,
        )?;
    Ok(png)
}

/// Read the text of an encoded screenshot (PNG or JPEG).
pub fn read_page(image: &[u8]) -> anyhow::Result<TextPage> {
    let decoded = image::load_from_memory(image)?;
    let (width, height) = (decoded.width(), decoded.height());
    let luma = decoded.to_luma8().into_raw();
    let factor = if width.max(height) <= UPSAMPLE_MAX_EDGE {
        2
    } else {
        1
    };
    let big;
    let (input, w, h) = if factor == 1 {
        (image, width, height)
    } else {
        let upsampled = decoded.resize_exact(
            width * factor,
            height * factor,
            image::imageops::FilterType::CatmullRom,
        );
        big = fast_png(&upsampled.to_rgb8())?;
        (big.as_slice(), width * factor, height * factor)
    };
    let first = recognize_text(input, w, h)?;
    // The current recognizer drops a glyph alone in a narrow cell at any
    // scale; the previous revision reads it. A second pass costs about as
    // much as the first and only fills gaps the first left.
    let second = recognize_text_with_revision(input, w, h, Some(SECOND_PASS_REVISION))
        .unwrap_or_else(|error| {
            tracing::debug!("second text recognition pass failed: {error}");
            Vec::new()
        });
    let mut fragments = cua_driver_core::opaque_text::merge_second_pass(first, second);
    let factor = f64::from(factor);
    for fragment in &mut fragments {
        fragment.x /= factor;
        fragment.y /= factor;
        fragment.width /= factor;
        fragment.height /= factor;
    }
    Ok(TextPage {
        fragments,
        luma,
        width,
        height,
    })
}

/// The first recognition after boot loads Vision's text models, which takes
/// tens of seconds in a VM; later ones take a fraction of a second, in any
/// process. Pay that once in the background when the daemon starts, instead
/// of in the first read of a window that needs it.
pub fn warm_up_in_background() {
    let _ = std::thread::Builder::new()
        .name("ocr-warm-up".into())
        .spawn(|| {
            let image = image::RgbImage::from_pixel(64, 32, image::Rgb([255, 255, 255]));
            if let Ok(png) = fast_png(&image) {
                let started = std::time::Instant::now();
                let result = recognize_text(&png, 64, 32);
                tracing::debug!(
                    "text recognition warm-up: {:?} in {} ms",
                    result.map(|f| f.len()),
                    started.elapsed().as_millis()
                );
            }
        });
}

/// `VNRecognizeTextRequestRevision2`: the recognizer before the current
/// default, which still reads lone glyphs (see [`read_page`]).
const SECOND_PASS_REVISION: usize = 2;

/// Recognize text in an encoded image (PNG or JPEG) of `width` x
/// `height` pixels. Fragments come back in those pixels, top-left origin.
pub fn recognize_text(image: &[u8], width: u32, height: u32) -> anyhow::Result<Vec<OcrFragment>> {
    recognize_text_with_revision(image, width, height, None)
}

/// [`recognize_text`] with a given `VNRecognizeTextRequest` revision
/// (`None`: the system default).
///
/// Vision answers repeated text requests on one image from its first result,
/// whatever their revision, so each call builds its own request handler.
pub fn recognize_text_with_revision(
    image: &[u8],
    width: u32,
    height: u32,
    revision: Option<usize>,
) -> anyhow::Result<Vec<OcrFragment>> {
    if image.is_empty() || width == 0 || height == 0 {
        anyhow::bail!("no image to recognize text in");
    }
    autoreleasepool(|_| {
        // SAFETY: every message below is sent to a live Vision object of the
        // class that declares it, with the argument types its header gives;
        // autoreleased results stay alive inside this pool.
        unsafe { recognize(image, f64::from(width), f64::from(height), revision) }
    })
}

unsafe fn recognize(
    image: &[u8],
    width: f64,
    height: f64,
    revision: Option<usize>,
) -> anyhow::Result<Vec<OcrFragment>> {
    let handler_class = vision_class("VNImageRequestHandler")?;
    let request_class = vision_class("VNRecognizeTextRequest")?;
    let data = NSData::with_bytes(image);
    let options: Option<Retained<AnyObject>> =
        msg_send_id![objc2::class!(NSDictionary), dictionary];
    let options = options.ok_or_else(|| anyhow::anyhow!("empty NSDictionary unavailable"))?;
    let handler: Allocated<AnyObject> = msg_send_id![handler_class, alloc];
    let handler: Option<Retained<AnyObject>> =
        msg_send_id![handler, initWithData: &*data, options: &*options];
    let handler = handler.ok_or_else(|| anyhow::anyhow!("VNImageRequestHandler init failed"))?;
    let request: Option<Retained<AnyObject>> = msg_send_id![request_class, new];
    let request = request.ok_or_else(|| anyhow::anyhow!("VNRecognizeTextRequest init failed"))?;
    if let Some(revision) = revision {
        let revisions: *mut AnyObject = msg_send![request_class, supportedRevisions];
        let supported: Bool = if revisions.is_null() {
            Bool::NO
        } else {
            msg_send![revisions, containsIndex: revision]
        };
        if !supported.as_bool() {
            anyhow::bail!("text recognition revision {revision} is unavailable");
        }
        let _: () = msg_send![&*request, setRevision: revision];
    }
    let _: () = msg_send![&*request, setRecognitionLevel: RECOGNITION_LEVEL_ACCURATE];
    // UI text is codes, amounts and short labels, not prose: language
    // correction rewrites them ("GNC-512" -> "GNC-5I2").
    let _: () = msg_send![&*request, setUsesLanguageCorrection: Bool::NO];
    // The default floor (1/32 of the image height) drops one-letter cells
    // such as a register's reconcile flag; UI text is all small.
    let _: () = msg_send![&*request, setMinimumTextHeight: 0.0f32];
    let requests: Option<Retained<AnyObject>> =
        msg_send_id![objc2::class!(NSArray), arrayWithObject: &*request];
    let requests = requests.ok_or_else(|| anyhow::anyhow!("NSArray unavailable"))?;
    let mut error: *mut AnyObject = std::ptr::null_mut();
    let ok: Bool = msg_send![
        &*handler,
        performRequests: &*requests,
        error: &mut error as *mut *mut AnyObject
    ];
    if !ok.as_bool() {
        let reason = if error.is_null() {
            "unknown error".to_owned()
        } else {
            let description: *mut NSString = msg_send![error, localizedDescription];
            if description.is_null() {
                "unknown error".to_owned()
            } else {
                (*description).to_string()
            }
        };
        anyhow::bail!("Vision text recognition failed: {reason}");
    }
    let results: *mut AnyObject = msg_send![&*request, results];
    if results.is_null() {
        return Ok(Vec::new());
    }
    let count: usize = msg_send![results, count];
    let mut fragments = Vec::with_capacity(count);
    for i in 0..count {
        let observation: *mut AnyObject = msg_send![results, objectAtIndex: i];
        if observation.is_null() {
            continue;
        }
        let candidates: *mut AnyObject = msg_send![observation, topCandidates: 1usize];
        if candidates.is_null() {
            continue;
        }
        let top: *mut AnyObject = msg_send![candidates, firstObject];
        if top.is_null() {
            continue;
        }
        let text: *mut NSString = msg_send![top, string];
        if text.is_null() {
            continue;
        }
        let confidence: f32 = msg_send![top, confidence];
        // Normalized, origin at the bottom-left.
        let bbox: NSRect = msg_send![observation, boundingBox];
        fragments.push(OcrFragment {
            text: (*text).to_string(),
            x: bbox.origin.x * width,
            y: (1.0 - bbox.origin.y - bbox.size.height) * height,
            width: bbox.size.width * width,
            height: bbox.size.height * height,
            confidence,
        });
    }
    Ok(fragments)
}

/// Last verdict per (pid, window): a read whose walk stops early (a small
/// `max_elements`) cannot tell an opaque window from a cut-off one, so it
/// reuses the last complete read's answer.
fn verdicts() -> &'static Mutex<HashMap<(i32, u32), bool>> {
    static VERDICTS: std::sync::OnceLock<Mutex<HashMap<(i32, u32), bool>>> =
        std::sync::OnceLock::new();
    VERDICTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The first non-empty of a node's title, value and description.
fn first_text(values: [Option<&String>; 3]) -> Option<&str> {
    values
        .into_iter()
        .flatten()
        .map(String::as_str)
        .find(|text| !text.trim().is_empty())
}

/// Whether `window_id`'s content is missing from the accessibility tree,
/// judged from this read's walk or, when it stopped early, the last one.
pub fn window_content_is_opaque(
    pid: i32,
    window_id: u32,
    tree: &crate::ax::tree::TreeWalkResult,
) -> bool {
    let facts = tree.nodes.iter().map(|node| NodeFacts {
        depth: node.depth,
        role: &node.role,
        subrole: node.subrole.as_deref(),
        text: first_text([
            node.title.as_ref(),
            node.value.as_ref(),
            node.description.as_ref(),
        ]),
    });
    let mut verdicts = verdicts().lock().unwrap_or_else(|e| e.into_inner());
    match content_is_opaque(facts, !tree.truncated) {
        Some(verdict) => {
            if verdicts.len() > 512 {
                verdicts.clear();
            }
            verdicts.insert((pid, window_id), verdict);
            verdict
        }
        None => verdicts.get(&(pid, window_id)).copied().unwrap_or(false),
    }
}

/// Whether the last complete read of `window_id` found its content missing
/// from the accessibility tree (or the window was opened by an action on such
/// a window and has not been read yet).
pub fn known_opaque(pid: i32, window_id: u32) -> bool {
    verdicts()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&(pid, window_id))
        .copied()
        .unwrap_or(false)
}

/// Treat a window an opaque window's action opened (a dialog of the same
/// toolkit) as opaque until a read of its own decides.
pub fn assume_opaque(pid: i32, window_id: u32) {
    verdicts()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry((pid, window_id))
        .or_insert(true);
}

/// A window's text rows as last read, and the pixel frame they are in.
#[derive(Clone, Debug)]
pub struct TextBaseline {
    pub rows: Vec<OcrRow>,
    pub width: u32,
    pub height: u32,
    /// Native capture width / `width`: the screenshot scale of this frame
    /// (1.0 when the rows were not read off a fresh capture).
    pub capture_scale: f64,
}

fn baselines() -> &'static Mutex<HashMap<(i32, u32), TextBaseline>> {
    static BASELINES: std::sync::OnceLock<Mutex<HashMap<(i32, u32), TextBaseline>>> =
        std::sync::OnceLock::new();
    BASELINES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Remember `rows` as the latest text of `window_id`, read in a
/// `width` x `height` frame; the next action on the window diffs against it.
pub fn remember_rows(pid: i32, window_id: u32, baseline: TextBaseline) {
    let mut map = baselines().lock().unwrap_or_else(|e| e.into_inner());
    if map.len() > 64 {
        map.clear();
    }
    map.insert((pid, window_id), baseline);
}

/// The latest remembered text of `window_id`.
pub fn baseline(pid: i32, window_id: u32) -> Option<TextBaseline> {
    baselines()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&(pid, window_id))
        .cloned()
}

/// Group and fingerprint a page's text into rows.
pub fn page_rows(page: TextPage) -> TextBaseline {
    let mut rows = cua_driver_core::opaque_text::group_rows(page.fragments);
    cua_driver_core::opaque_text::mark_unread_ink(&mut rows, &page.luma, page.width, page.height);
    cua_driver_core::opaque_text::stamp_ink(&mut rows, &page.luma, page.width, page.height);
    TextBaseline {
        rows,
        width: page.width,
        height: page.height,
        capture_scale: 1.0,
    }
}

/// The pixel frame a window's text is read in.
#[derive(Clone, Copy, Debug)]
pub enum RowFrame {
    /// The longer edge is this many pixels (an earlier read's frame).
    MaxEdge(u32),
    /// The native capture divided by this factor (the frame of the window's
    /// latest screenshot, which pixel actions use).
    Downscale(f64),
}

/// Capture `window_id` now and read its text rows in `frame`.
pub fn read_window_rows(window_id: u32, frame: RowFrame) -> anyhow::Result<TextBaseline> {
    let raw = crate::capture::screenshot_window_image(window_id)?;
    let (native_width, native_height) = raw.dimensions()?;
    let max_edge = match frame {
        RowFrame::MaxEdge(edge) => edge,
        RowFrame::Downscale(factor) => (f64::from(native_width.max(native_height))
            / factor.max(1e-3))
        .round()
        .max(1.0) as u32,
    };
    let (png, _, _) = raw.encode(
        max_edge,
        cua_driver_core::image_utils::ScreenshotEncoding::Png,
    )?;
    let mut text = page_rows(read_page(&png)?);
    if text.width > 0 {
        text.capture_scale = f64::from(native_width) / f64::from(text.width);
    }
    Ok(text)
}

/// The pixel rectangle `[x, y, width, height]` of `tree`'s picture region
/// (see [`cua_driver_core::opaque_text::picture_region`]) in a screenshot of
/// `width` x `height` pixels taken of a window at `bounds` (screen points).
/// `None` when the walk was cut short: a missing subtree would look empty.
pub fn picture_region_px(
    tree: &crate::ax::tree::TreeWalkResult,
    bounds: &crate::windows::WindowBounds,
    width: u32,
    height: u32,
) -> Option<[u32; 4]> {
    use cua_driver_core::opaque_text::{picture_region, RegionFacts};
    if tree.truncated || tree.depth_truncated || bounds.width <= 0.0 || bounds.height <= 0.0 {
        return None;
    }
    let facts = tree.nodes.iter().map(|node| RegionFacts {
        role: &node.role,
        has_text: first_text([
            node.title.as_ref(),
            node.value.as_ref(),
            node.description.as_ref(),
        ])
        .is_some(),
        frame: node.frame,
    });
    let region = picture_region(facts, [bounds.x, bounds.y, bounds.width, bounds.height])?;
    let (sx, sy) = (
        f64::from(width) / bounds.width,
        f64::from(height) / bounds.height,
    );
    let x0 = ((region[0] - bounds.x) * sx)
        .floor()
        .clamp(0.0, f64::from(width)) as u32;
    let y0 = ((region[1] - bounds.y) * sy)
        .floor()
        .clamp(0.0, f64::from(height)) as u32;
    let x1 = ((region[0] + region[2] - bounds.x) * sx)
        .ceil()
        .clamp(0.0, f64::from(width)) as u32;
    let y1 = ((region[1] + region[3] - bounds.y) * sy)
        .ceil()
        .clamp(0.0, f64::from(height)) as u32;
    (x1 > x0 + 8 && y1 > y0 + 8).then_some([x0, y0, x1 - x0, y1 - y0])
}

/// Read the text rows of `region` (pixels) of an encoded screenshot, in the
/// whole screenshot's pixels. Empty when the region is blank: a blank region
/// costs a decode, not a recognition.
pub fn read_region_rows(image: &[u8], region: [u32; 4]) -> anyhow::Result<Vec<OcrRow>> {
    let decoded = image::load_from_memory(image)?;
    let [x, y, w, h] = region;
    let x = x.min(decoded.width());
    let y = y.min(decoded.height());
    let w = w.min(decoded.width() - x);
    let h = h.min(decoded.height() - y);
    if w == 0 || h == 0 {
        return Ok(Vec::new());
    }
    let crop = decoded.crop_imm(x, y, w, h).to_rgb8();
    let luma = image::DynamicImage::ImageRgb8(crop.clone()).to_luma8();
    let mean =
        (luma.as_raw().iter().map(|&l| u64::from(l)).sum::<u64>() / u64::from(w * h).max(1)) as i16;
    if !luma
        .as_raw()
        .iter()
        .any(|&l| (i16::from(l) - mean).abs() > 64)
    {
        return Ok(Vec::new());
    }
    let mut text = page_rows(read_page(&fast_png(&crop)?)?);
    cua_driver_core::opaque_text::offset_rows(&mut text.rows, f64::from(x), f64::from(y));
    Ok(text.rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_text_in_a_rendered_image() {
        use core_graphics::base::{kCGBitmapByteOrder32Big, kCGImageAlphaPremultipliedLast};
        use core_graphics::color_space::CGColorSpace;
        use core_graphics::context::CGContext;

        // A blank white image has no text; Vision must still answer cleanly.
        let ctx = CGContext::create_bitmap_context(
            None,
            64,
            32,
            8,
            256,
            &CGColorSpace::create_device_rgb(),
            kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big,
        );
        let mut ctx = ctx;
        ctx.data().fill(255);
        let png =
            cua_driver_core::image_utils::encode_rgba_to_png(ctx.data(), 64, 32).expect("encode");
        let fragments = recognize_text(&png, 64, 32).expect("vision answers");
        assert!(fragments.is_empty(), "{fragments:?}");
    }

    #[test]
    fn a_page_keeps_the_screenshot_size_and_luminance() {
        let image = image::RgbImage::from_pixel(40, 20, image::Rgb([255, 255, 255]));
        let mut png = Vec::new();
        image
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .expect("encode");
        let page = read_page(&png).expect("read");
        assert_eq!((page.width, page.height), (40, 20));
        assert_eq!(page.luma.len(), 800);
        assert!(page.fragments.is_empty());
    }

    /// Print the rows read from the image at `OCR_PROBE_IMAGE` (manual probe).
    #[test]
    #[ignore = "manual probe: OCR_PROBE_IMAGE=<png> cargo test -- --ignored --nocapture"]
    fn probe_rows_of_an_image() {
        let path = std::env::var("OCR_PROBE_IMAGE").expect("OCR_PROBE_IMAGE");
        let bytes = std::fs::read(path).expect("read image");
        let started = std::time::Instant::now();
        let _ = read_page(&bytes).expect("warm");
        let warm = started.elapsed();
        let started = std::time::Instant::now();
        let text = page_rows(read_page(&bytes).expect("read"));
        println!(
            "warm-up {} ms, read {} ms",
            warm.as_millis(),
            started.elapsed().as_millis()
        );
        for row in &text.rows {
            println!("{}", cua_driver_core::opaque_text::render_row(row));
        }
    }

    #[test]
    fn a_blank_region_is_not_recognized() {
        let image = image::RgbImage::from_pixel(80, 60, image::Rgb([240, 240, 240]));
        let mut png = Vec::new();
        image
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .expect("encode");
        assert!(read_region_rows(&png, [10, 10, 50, 40])
            .expect("read")
            .is_empty());
        // A region past the edge is clipped, not an error.
        assert!(read_region_rows(&png, [70, 50, 50, 40])
            .expect("read")
            .is_empty());
    }

    #[test]
    fn the_second_pass_revision_is_available() {
        let image = image::RgbImage::from_pixel(64, 32, image::Rgb([255, 255, 255]));
        let png = fast_png(&image).expect("encode");
        let fragments = recognize_text_with_revision(&png, 64, 32, Some(SECOND_PASS_REVISION))
            .expect("revision 2 answers");
        assert!(fragments.is_empty());
        assert!(recognize_text_with_revision(&png, 64, 32, Some(999)).is_err());
    }

    #[test]
    fn empty_input_is_refused() {
        assert!(recognize_text(&[], 10, 10).is_err());
    }
}

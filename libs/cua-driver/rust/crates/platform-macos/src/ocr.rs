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

use cua_driver_core::opaque_text::{content_is_opaque, NodeFacts, OcrFragment};
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
    let mut fragments = if factor == 1 {
        recognize_text(image, width, height)?
    } else {
        let big = decoded.resize_exact(
            width * factor,
            height * factor,
            image::imageops::FilterType::CatmullRom,
        );
        let png = fast_png(&big.to_rgb8())?;
        recognize_text(&png, width * factor, height * factor)?
    };
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

/// Recognize text in an encoded image (PNG or JPEG) of `width` x
/// `height` pixels. Fragments come back in those pixels, top-left origin.
pub fn recognize_text(image: &[u8], width: u32, height: u32) -> anyhow::Result<Vec<OcrFragment>> {
    if image.is_empty() || width == 0 || height == 0 {
        anyhow::bail!("no image to recognize text in");
    }
    autoreleasepool(|_| {
        // SAFETY: every message below is sent to a live Vision object of the
        // class that declares it, with the argument types its header gives;
        // autoreleased results stay alive inside this pool.
        unsafe { recognize(image, f64::from(width), f64::from(height)) }
    })
}

unsafe fn recognize(image: &[u8], width: f64, height: f64) -> anyhow::Result<Vec<OcrFragment>> {
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

fn has_text(value: Option<&String>) -> bool {
    value.is_some_and(|text| !text.trim().is_empty())
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
        has_text: has_text(node.title.as_ref())
            || has_text(node.value.as_ref())
            || has_text(node.description.as_ref()),
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

    #[test]
    fn empty_input_is_refused() {
        assert!(recognize_text(&[], 10, 10).is_err());
    }
}

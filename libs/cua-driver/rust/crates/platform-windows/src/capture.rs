//! Window screenshot via PrintWindow + GDI BitBlt on Windows.
//!
//! `PW_RENDERFULLCONTENT` (0x2) renders the window contents even if it is
//! occluded or off-screen for GDI-backed surfaces. The result is encoded as
//! base64 PNG in memory.
//!
//! ## UWP / DirectComposition fallback (CUA-542)
//!
//! `PrintWindow` doesn't capture DirectComposition-backed surfaces —
//! modern UWP / WinUI3 apps (Calculator, Photos, Settings, Win 11
//! Notepad) render directly to the GPU compositor and have no GDI back
//! buffer for `PrintWindow` to copy from. Result is an all-black image.
//!
//! When the PrintWindow result comes back mostly-black (sentinel for
//! that case), we fall back to a **screen-region BitBlt**: read the
//! window's on-screen bounds via `GetWindowRect`, BitBlt the matching
//! pixels off the desktop DC. This is the same approach the Windows
//! Snipping Tool's "Window" mode uses. Trade-off: only works when the
//! window is actually on-screen and not occluded by another window.
//! For our daemon-driven agent flow that's the common case anyway —
//! the daemon lives in the user's interactive session and the target
//! is typically a visible window the agent just launched.
//!
//! The full proper fix (Windows.Graphics.Capture, which works for
//! occluded / off-screen UWP windows too) is tracked separately on
//! CUA-542; the screen-region fallback covers the same common
//! ground at a fraction of the implementation cost.

use anyhow::{bail, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
    SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, RGBQUAD, SRCCOPY,
};
use windows::Win32::Graphics::Gdi::{GetWindowDC, ReleaseDC};
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
const PW_RENDERFULLCONTENT: PRINT_WINDOW_FLAGS = PRINT_WINDOW_FLAGS(2u32);

/// After GetDIBits we have BGRA bytes from PrintWindow. If essentially every
/// pixel is fully-transparent black or fully-opaque black, treat the capture
/// as "PrintWindow didn't render this surface" and let the caller fall back
/// to the screen-region BitBlt path.
///
/// We sample sparsely (every 64th pixel) so the heuristic is cheap even on
/// 4K windows. The threshold is intentionally aggressive — UWP apps return
/// all-zeros bitmaps, not just dark frames — so legitimate dark UI doesn't
/// trip the fallback.
fn is_mostly_black_bgra(bgra: &[u8]) -> bool {
    if bgra.len() < 16 {
        return true;
    }
    let pixel_count = bgra.len() / 4;
    if pixel_count == 0 {
        return true;
    }
    let stride = (pixel_count / 1024).max(1);
    let mut sampled = 0usize;
    let mut black = 0usize;
    for i in (0..pixel_count).step_by(stride) {
        let off = i * 4;
        // BGRA layout. We consider a pixel "black" when B+G+R == 0,
        // regardless of alpha — that's the all-zero pattern UWP /
        // DirectComposition leaves behind.
        if bgra[off] == 0 && bgra[off + 1] == 0 && bgra[off + 2] == 0 {
            black += 1;
        }
        sampled += 1;
    }
    // > 99.5% of sampled pixels are black → treat as failed render.
    sampled > 0 && (black * 200) >= (sampled * 199)
}

/// Probe whether `target` is currently obscured by some other window.
/// Samples `WindowFromPoint` at the target's center + four corners (insetting
/// 2 px from each corner so we don't catch invisible 1-px frame slop). If any
/// sample returns a window whose root ancestor isn't `target` itself, we
/// consider the target occluded. Used by the screen-region BitBlt path to
/// warn callers that the bitmap they're about to receive does NOT actually
/// show the target's pixels — it shows whatever's covering it. Without this
/// signal, the BitBlt result is silently misleading (see the regression
/// captured during the autonomous-test journal: a backgrounded Notepad
/// screenshot returned LibreOffice's pixels).
unsafe fn target_is_obscured(target: HWND) -> bool {
    use windows::Win32::Foundation::{POINT, RECT};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetAncestor, GetWindowRect, WindowFromPoint, GA_ROOT,
    };

    if target.0.is_null() {
        return false;
    }
    let mut rect = RECT::default();
    if GetWindowRect(target, &mut rect).is_err() {
        return false;
    }
    let w = rect.right - rect.left;
    let h = rect.bottom - rect.top;
    if w <= 4 || h <= 4 {
        return false;
    }
    // 5 sample points: 4 corners (inset 2 px) + center.
    let pts: [(i32, i32); 5] = [
        (rect.left + 2, rect.top + 2),
        (rect.right - 3, rect.top + 2),
        (rect.left + 2, rect.bottom - 3),
        (rect.right - 3, rect.bottom - 3),
        ((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2),
    ];
    let target_root = GetAncestor(target, GA_ROOT);
    let mut covered_samples = 0;
    for (x, y) in &pts {
        let p = POINT { x: *x, y: *y };
        let owner = WindowFromPoint(p);
        if owner.0.is_null() {
            continue;
        }
        let owner_root = GetAncestor(owner, GA_ROOT);
        if owner_root != target_root {
            covered_samples += 1;
        }
    }
    // Threshold of 2 of 5: a single corner being "covered" can be a layered
    // overlay (e.g. the cua-driver agent cursor) that's not actually opaque
    // over the content; we don't want to false-positive on that. Two or more
    // sample points missing means real content is being covered.
    covered_samples >= 2
}

/// Fallback capture path: BitBlt the desktop DC over the rectangle covered
/// by `hwnd`'s on-screen bounds. Works for UWP / WinUI3 / DirectComposition
/// surfaces that PrintWindow can't reach, as long as the window is on-screen
/// (the daemon's typical case — see module docs).
unsafe fn screenshot_via_screen_region(hwnd: HWND) -> Result<(Vec<u8>, i32, i32)> {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

    let mut rect = RECT::default();
    GetWindowRect(hwnd, &mut rect)?;

    // Under Per-Monitor V2 DPI awareness, GetWindowRect already returns
    // PHYSICAL pixels (coordinate virtualization only applies to
    // DPI-unaware/system-aware processes), and BitBlt operates in physical
    // pixels too — use the rect as-is. Scaling by DPI/96 here would shift
    // and oversize the captured screen region (issue #1879).
    let physical_left = rect.left;
    let physical_top = rect.top;

    let w = rect.right - rect.left;
    let h = rect.bottom - rect.top;
    if w <= 0 || h <= 0 {
        bail!("screen-region fallback: window has zero/negative bounds: {w}x{h}");
    }

    let screen_dc = GetDC(HWND(std::ptr::null_mut())); // NULL HWND → desktop DC
    let mem_dc = CreateCompatibleDC(screen_dc);
    let bitmap = CreateCompatibleBitmap(screen_dc, w, h);
    let old_bitmap = SelectObject(mem_dc, bitmap);

    // Copy from physical screen coords into our memory DC at (0, 0).
    let blt_ok = BitBlt(
        mem_dc,
        0,
        0,
        w,
        h,
        screen_dc,
        physical_left,
        physical_top,
        SRCCOPY,
    );

    let mut bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h, // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            biSizeImage: (w * h * 4) as u32,
            ..Default::default()
        },
        bmiColors: [RGBQUAD::default(); 1],
    };
    let pixel_count = (w * h) as usize;
    let mut pixels = vec![0u8; pixel_count * 4];
    let ok = GetDIBits(
        mem_dc,
        bitmap,
        0,
        h as u32,
        Some(pixels.as_mut_ptr() as *mut _),
        &mut bmi,
        DIB_RGB_COLORS,
    );

    SelectObject(mem_dc, old_bitmap);
    let _ = DeleteObject(bitmap);
    let _ = DeleteDC(mem_dc);
    ReleaseDC(HWND(std::ptr::null_mut()), screen_dc);

    if blt_ok.is_err() {
        bail!("screen-region fallback: BitBlt failed: {:?}", blt_ok);
    }
    if ok == 0 {
        bail!("screen-region fallback: GetDIBits returned 0");
    }
    Ok((pixels, w, h))
}

/// Capture a window by HWND, returning raw PNG bytes.
pub fn screenshot_window_bytes(hwnd: u64) -> Result<Vec<u8>> {
    screenshot_window_bytes_with_occlusion(hwnd).map(|(b, _)| b)
}

/// Capture a window by HWND. Returns `(png_bytes, occluded_flag)` — the
/// boolean is `true` when the capture had to fall through to the
/// screen-region BitBlt path AND another window was visibly covering the
/// target at sample time. In that case the bitmap reflects the COVERING
/// window's pixels, not the target's. Callers that surface the image to a
/// user / LLM should attach an explicit warning. See `target_is_obscured`
/// for the sampling heuristic.
pub fn screenshot_window_bytes_with_occlusion(hwnd: u64) -> Result<(Vec<u8>, bool)> {
    match unsafe { screenshot_window_bytes_with_occlusion_unsafe(hwnd) } {
        Ok(capture) => Ok(capture),
        Err(primary_error) => {
            if primary_error.to_string().contains("minimized window") {
                return Err(primary_error);
            }
            // A freshly restored DirectComposition window can temporarily have
            // no usable GDI surface even though DWM is already rendering it.
            // WGC reads the compositor-owned frame and is therefore the right
            // first fallback for this class of capture failure.
            match crate::wgc::screenshot_window_via_wgc(hwnd) {
                Ok((pixels, width, height)) => Ok((
                    cua_driver_core::image_utils::encode_bgra_to_png(&pixels, width, height)?,
                    false,
                )),
                Err(wgc_error) => {
                    // Headless/virtualized Windows sessions can expose DWM but
                    // no WGC-compatible hardware device. Once a window is
                    // visible, a desktop-region crop remains a truthful final
                    // fallback; report whether another window covered it.
                    let target = HWND(hwnd as *mut _);
                    let occluded = unsafe { target_is_obscured(target) };
                    match unsafe { screenshot_via_screen_region(target) } {
                        Ok((pixels, width, height)) => Ok((
                            cua_driver_core::image_utils::encode_bgra_to_png(
                                &pixels,
                                width as u32,
                                height as u32,
                            )?,
                            occluded,
                        )),
                        Err(screen_error) => Err(primary_error.context(format!(
                            "Windows.Graphics.Capture fallback failed: {wgc_error}; \
                             desktop-region fallback failed: {screen_error}"
                        ))),
                    }
                }
            }
        }
    }
}

/// Capture a window by HWND, returning (base64_png, width, height).
pub fn screenshot_window(hwnd: u64) -> Result<(String, u32, u32)> {
    let png_bytes = screenshot_window_bytes(hwnd)?;
    let (w, h) = {
        if png_bytes.len() < 24 {
            bail!("PNG too small");
        }
        let w = u32::from_be_bytes([png_bytes[16], png_bytes[17], png_bytes[18], png_bytes[19]]);
        let h = u32::from_be_bytes([png_bytes[20], png_bytes[21], png_bytes[22], png_bytes[23]]);
        (w, h)
    };
    Ok((BASE64.encode(&png_bytes), w, h))
}

unsafe fn screenshot_window_bytes_with_occlusion_unsafe(hwnd: u64) -> Result<(Vec<u8>, bool)> {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, IsIconic};

    let hwnd_raw = hwnd;
    let hwnd = HWND(hwnd as *mut _);

    // Bail loudly on minimized (iconic) windows BEFORE attempting any
    // capture path. On Windows, `GetWindowRect` on an iconic HWND returns
    // the off-screen "iconic position" (typically `(-32000, -32000,
    // -31840, -31972)` i.e. w=160, h=28, both positive) and `PrintWindow`
    // paints nothing into the bitmap. The result is a heavily-compressed
    // all-black ~28x160 PNG (~300 bytes) that an upstream agent can't tell
    // apart from a real "blank screen" capture — wasting model turns
    // retrying against a window that's literally minimized to the taskbar.
    //
    // The WGC sibling path at `wgc.rs:58` already short-circuits this case;
    // the GDI/PrintWindow fallback below + the screen-region BitBlt fallback
    // both happily produced the degenerate PNG. Guarding here covers both
    // and matches the WGC error shape so callers can `list_windows` and
    // restore the window before retrying.
    if IsIconic(hwnd).as_bool() {
        bail!(
            "cannot capture minimized window 0x{hwnd_raw:x}: it has no \
             rendered content. Call bring_to_front with this window_id to \
             restore it first. The PrintWindow GDI path and the screen-region \
             BitBlt fallback both return an all-black bitmap for iconic \
             windows."
        );
    }

    // CUA-542 routing: for known XAML / WinUI3 / UWP targets, the
    // PrintWindow GDI path returns black (DirectComposition isn't in
    // the GDI back-buffer). We try Windows.Graphics.Capture (WGC)
    // first — that's the only API that returns the target's actual
    // composited pixels even when it's occluded by another window
    // (the Calculator-behind-terminal regression captured in the
    // autonomous-test journal). On older Windows / GPU stalls / cloaked
    // windows WGC may fail; fall back to the screen-region BitBlt
    // path, then to PrintWindow as a last resort.
    if crate::input::is_xaml_host_hwnd(hwnd_raw) {
        match crate::wgc::screenshot_window_via_wgc(hwnd_raw) {
            Ok((pixels, w, h)) => {
                return Ok((
                    cua_driver_core::image_utils::encode_bgra_to_png(&pixels, w, h)?,
                    false, // WGC reads target's own pixels — never occluded by definition
                ));
            }
            Err(e) => {
                tracing::warn!(
                    target: "cua-driver",
                    "screenshot: WGC failed for XAML target hwnd 0x{hwnd_raw:x}: {e}; \
                     falling back to screen-region BitBlt (may be occluded)."
                );
            }
        }
        let occluded = target_is_obscured(hwnd);
        match screenshot_via_screen_region(hwnd) {
            Ok((pixels, w, h)) => {
                return Ok((
                    cua_driver_core::image_utils::encode_bgra_to_png(&pixels, w as u32, h as u32)?,
                    occluded,
                ));
            }
            Err(e) => {
                // Screen-region failed — fall through and try PrintWindow as a
                // last resort so the caller at least gets *something*.
                tracing::warn!(
                    target: "cua-driver",
                    "screenshot: XAML target screen-region path failed: {e}; \
                     falling back to PrintWindow (likely all-black)."
                );
            }
        }
    }

    // Size the capture buffer to the WHOLE window (GetWindowRect), not
    // just the client area (GetClientRect). PrintWindow draws the entire
    // window at 1:1 starting at (0,0) — if the buffer is sized to the
    // client area only, anything in the non-client region clips. The
    // surprising case is VCL/SAL dialogs (LibreOffice's "Document
    // Recovery", any of its Confirmation modals): VCL puts the bottom
    // button strip OUTSIDE the standard Win32 client area, so a
    // client-sized buffer loses the Save/Cancel/OK row at the bottom.
    // Window-sized buffer captures title bar + body + non-client trim
    // correctly.
    //
    // Under Per-Monitor V2 DPI awareness, GetWindowRect returns PHYSICAL
    // pixels — the same unit GetWindowDC and PrintWindow/BitBlt work in.
    // Use the dimensions as-is; scaling by DPI/96 would allocate an
    // oversized bitmap with the content in its top-left corner and a
    // black margin around it (issue #1879). It also kept the
    // DWMWA_EXTENDED_FRAME_BOUNDS crop below (physical pixels) from
    // matching the bitmap.
    let mut win_rect = RECT::default();
    GetWindowRect(hwnd, &mut win_rect)?;
    let w = win_rect.right - win_rect.left;
    let h = win_rect.bottom - win_rect.top;
    if w <= 0 || h <= 0 {
        bail!("Window has zero/negative size: {}x{}", w, h);
    }

    let screen_dc = GetWindowDC(hwnd);
    let mem_dc = CreateCompatibleDC(screen_dc);
    let bitmap = CreateCompatibleBitmap(screen_dc, w, h);
    let old_bitmap = SelectObject(mem_dc, bitmap);

    let pw_ok = PrintWindow(hwnd, mem_dc, PW_RENDERFULLCONTENT);
    if !pw_ok.as_bool() {
        BitBlt(mem_dc, 0, 0, w, h, screen_dc, 0, 0, SRCCOPY)?;
    }

    // Compute the DWM-extended-frame bounds. On Win10+ the OS draws an
    // invisible drop-shadow margin around every top-level window that
    // GetWindowRect counts in its dimensions but PrintWindow does NOT
    // actually paint into — leaving a black trim around the body in the
    // captured bitmap (except the title bar, which spans the full width
    // and so doesn't pick up the artifact). DWMWA_EXTENDED_FRAME_BOUNDS
    // returns the rectangle WITHOUT the shadow margin. We compute the
    // offsets relative to GetWindowRect and crop the bitmap below.
    //
    // Best-effort: if the DWM call fails (e.g. very old Windows, hooked
    // by some shell extension), we keep the full-window bitmap as-is —
    // user sees a small dark border but no clipping.
    let dwm_rect: Option<RECT> = {
        use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
        let mut r = RECT::default();
        let hr = DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut r as *mut _ as *mut _,
            std::mem::size_of::<RECT>() as u32,
        );
        hr.ok().map(|_| r)
    };

    let mut bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            biSizeImage: (w * h * 4) as u32,
            ..Default::default()
        },
        bmiColors: [RGBQUAD::default(); 1],
    };

    let pixel_count = (w * h) as usize;
    let mut pixels = vec![0u8; pixel_count * 4];
    let ok = GetDIBits(
        mem_dc,
        bitmap,
        0,
        h as u32,
        Some(pixels.as_mut_ptr() as *mut _),
        &mut bmi,
        DIB_RGB_COLORS,
    );

    SelectObject(mem_dc, old_bitmap);
    let _ = DeleteObject(bitmap);
    let _ = DeleteDC(mem_dc);
    ReleaseDC(hwnd, screen_dc);

    if ok == 0 {
        bail!("GetDIBits returned 0");
    }

    // Crop the bitmap to the DWM extended-frame bounds (computed above)
    // to remove the invisible-shadow margin PrintWindow doesn't paint.
    // Skipped on the DWM-failed branch — caller sees a thin dark trim
    // around the body but the body itself is intact.
    //
    // INSET: DWMWA_EXTENDED_FRAME_BOUNDS reports the rect WITHOUT the
    // invisible shadow margin, but Win11 dialogs paint a 1-2 px dark
    // stroke at the rounded-corner edge that ends up at the very edge
    // of the DWM-extended-frame rect. The visual artifact is a thin
    // black hairline along (typically) the bottom or right edge of the
    // captured bitmap. A 1-px inset on each side removes the hairline
    // without losing actual UI content — anything that close to the
    // edge is window-frame chrome, not content.
    const DWM_CROP_INSET_PX: i32 = 1;
    let (pixels, w, h) = if let Some(dwm) = dwm_rect {
        let off_x = (dwm.left - win_rect.left) as i32 + DWM_CROP_INSET_PX;
        let off_y = (dwm.top - win_rect.top) as i32 + DWM_CROP_INSET_PX;
        let crop_w = (dwm.right - dwm.left) as i32 - 2 * DWM_CROP_INSET_PX;
        let crop_h = (dwm.bottom - dwm.top) as i32 - 2 * DWM_CROP_INSET_PX;
        if off_x >= 0
            && off_y >= 0
            && crop_w > 0
            && crop_h > 0
            && off_x + crop_w <= w
            && off_y + crop_h <= h
        {
            let stride_full = (w * 4) as usize;
            let stride_crop = (crop_w * 4) as usize;
            let mut cropped = vec![0u8; (crop_w * crop_h * 4) as usize];
            for row in 0..crop_h as usize {
                let src_row = (off_y as usize + row) * stride_full + (off_x as usize) * 4;
                let dst_row = row * stride_crop;
                cropped[dst_row..dst_row + stride_crop]
                    .copy_from_slice(&pixels[src_row..src_row + stride_crop]);
            }
            (cropped, crop_w, crop_h)
        } else {
            (pixels, w, h)
        }
    } else {
        (pixels, w, h)
    };

    // CUA-542: detect the all-black bitmap PrintWindow returns for
    // DirectComposition-backed UWP / WinUI3 surfaces. Recovery order:
    //   1. WGC (occlusion-immune; works for UWP).
    //   2. Screen-region BitBlt (works when target is on-screen and
    //      not covered).
    // The WGC-first ordering covers backgrounded UWP targets the
    // screen-region path mishandles (returns covering window's pixels).
    if is_mostly_black_bgra(&pixels) {
        match crate::wgc::screenshot_window_via_wgc(hwnd_raw) {
            Ok((alt_pixels, w, h)) => {
                return Ok((
                    cua_driver_core::image_utils::encode_bgra_to_png(&alt_pixels, w, h)?,
                    false,
                ));
            }
            Err(e) => {
                tracing::warn!(
                    target: "cua-driver",
                    "screenshot: PrintWindow returned black AND WGC failed for hwnd 0x{hwnd_raw:x}: {e}; \
                     trying screen-region BitBlt next."
                );
            }
        }
        let occluded = target_is_obscured(hwnd);
        match screenshot_via_screen_region(hwnd) {
            Ok((alt_pixels, alt_w, alt_h)) => {
                return Ok((
                    cua_driver_core::image_utils::encode_bgra_to_png(
                        &alt_pixels,
                        alt_w as u32,
                        alt_h as u32,
                    )?,
                    occluded,
                ));
            }
            Err(e) => {
                // Screen-region path failed too — return the (black) PrintWindow
                // result with an explanatory log rather than erroring outright.
                // Caller still gets an image; the fact that it's black is now
                // visible in the bytes themselves.
                tracing::warn!(
                    target: "cua-driver",
                    "screenshot: PrintWindow returned a mostly-black bitmap (UWP / \
                     DirectComposition target?); screen-region fallback failed: {e}"
                );
            }
        }
    }

    // BGRA → PNG via the shared `image_utils::encode_bgra_to_png`
    // helper (extracted from this file 2026-05; was a hand-rolled
    // uncompressed-PNG path that produced ~5x larger output. The
    // `image` crate's encoder is already a workspace dep so the
    // smaller output is free).
    // PrintWindow itself reads from the target's own DC, so the bitmap
    // we return here is the target's pixels even when occluded — no
    // occluded warning needed on this path.
    Ok((
        cua_driver_core::image_utils::encode_bgra_to_png(&pixels, w as u32, h as u32)?,
        false,
    ))
}

// NOTE: previously this module carried a hand-rolled
// `write_uncompressed_png` + `write_png_chunk` + `zlib_store` +
// `adler32` + `crc32_ieee` (~110 lines) plus a local
// `encode_bgra_to_png` that used them. All of that is replaced by
// `cua_driver_core::image_utils::encode_bgra_to_png` which goes through
// the `image` crate's PNG encoder — already a workspace dep,
// produces ~5x smaller files than the uncompressed-store path.
//
// Same extraction applies to the four pub helpers below
// (`png_bytes_to_jpeg`, `resize_png_if_needed`, `crosshair_png_bytes`,
// `png_dimensions_pub`). They're now thin re-exports of the shared
// `cua_driver_core::image_utils::*` so all three platform crates call the
// same code. See `CUA_DRIVER_RS_DEDUP_AUDIT.md` for the full audit.

/// Capture the primary display (full screen), returning raw PNG bytes.
pub fn screenshot_display_bytes() -> Result<Vec<u8>> {
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
        // Under Per-Monitor V2 DPI awareness, GetSystemMetrics returns
        // PHYSICAL pixels — the same unit BitBlt captures in. Scaling by
        // DPI/96 would allocate an oversized bitmap with black margins
        // (issue #1879).
        let w = GetSystemMetrics(SM_CXSCREEN);
        let h = GetSystemMetrics(SM_CYSCREEN);
        if w <= 0 || h <= 0 {
            bail!("Could not get screen metrics");
        }
        let screen_dc = GetDC(HWND::default());
        let mem_dc = CreateCompatibleDC(screen_dc);
        let bitmap = CreateCompatibleBitmap(screen_dc, w, h);
        let old_bitmap = SelectObject(mem_dc, bitmap);
        BitBlt(mem_dc, 0, 0, w, h, screen_dc, 0, 0, SRCCOPY)?;
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                biSizeImage: (w * h * 4) as u32,
                ..Default::default()
            },
            bmiColors: [RGBQUAD::default(); 1],
        };
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        let ok = GetDIBits(
            mem_dc,
            bitmap,
            0,
            h as u32,
            Some(pixels.as_mut_ptr() as *mut _),
            &mut bmi,
            DIB_RGB_COLORS,
        );
        SelectObject(mem_dc, old_bitmap);
        let _ = DeleteObject(bitmap);
        let _ = DeleteDC(mem_dc);
        ReleaseDC(HWND::default(), screen_dc);
        if ok == 0 {
            bail!("GetDIBits returned 0");
        }
        cua_driver_core::image_utils::encode_bgra_to_png(&pixels, w as u32, h as u32)
    }
}

/// Capture primary display, returning (base64_png, width, height).
pub fn screenshot_display() -> Result<(String, u32, u32)> {
    let png_bytes = screenshot_display_bytes()?;
    let (w, h) = cua_driver_core::image_utils::png_dimensions(&png_bytes)?;
    Ok((BASE64.encode(&png_bytes), w, h))
}

// PNG/JPEG/resize/crosshair helpers — re-exports of the shared
// `cua_driver_core::image_utils` module. The previous file-local copies were
// near-identical to the macOS and Linux versions; the dedup-audit
// (2026-05) moved them all to one place.

/// Convert PNG bytes to JPEG at the given quality (1–95).
pub fn png_bytes_to_jpeg(png_bytes: &[u8], quality: u8) -> Result<Vec<u8>> {
    cua_driver_core::image_utils::png_bytes_to_jpeg(png_bytes, quality)
}

/// Downscale `png_bytes` so neither dimension exceeds `max_dim`.
/// If `max_dim == 0` or the image already fits, returns a copy of the
/// original bytes unchanged.
pub fn resize_png_if_needed(png_bytes: &[u8], max_dim: u32) -> Result<Vec<u8>> {
    cua_driver_core::image_utils::resize_png_if_needed(png_bytes, max_dim)
}

/// Draw a red crosshair at pixel (cx, cy) on a PNG image and return
/// modified PNG bytes. Used by recording's click-marker callback to
/// produce click.png.
pub fn crosshair_png_bytes(png_bytes: &[u8], cx: f64, cy: f64) -> Result<Vec<u8>> {
    cua_driver_core::image_utils::crosshair_png_bytes(png_bytes, cx, cy)
}

/// Parse width and height from a PNG IHDR chunk.
///
/// Suffixed `_pub` because an older private `png_dimensions` predated
/// the `_pub` export; the public alias is what callers use today.
pub fn png_dimensions_pub(data: &[u8]) -> Result<(u32, u32)> {
    cua_driver_core::image_utils::png_dimensions(data)
}

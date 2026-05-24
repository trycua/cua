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
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
    GetDIBits, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
    RGBQUAD, SRCCOPY,
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
    if bgra.len() < 16 { return true; }
    let pixel_count = bgra.len() / 4;
    if pixel_count == 0 { return true; }
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

/// Fallback capture path: BitBlt the desktop DC over the rectangle covered
/// by `hwnd`'s on-screen bounds. Works for UWP / WinUI3 / DirectComposition
/// surfaces that PrintWindow can't reach, as long as the window is on-screen
/// (the daemon's typical case — see module docs).
unsafe fn screenshot_via_screen_region(hwnd: HWND) -> Result<(Vec<u8>, i32, i32)> {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

    let mut rect = RECT::default();
    GetWindowRect(hwnd, &mut rect)?;
    let w = rect.right - rect.left;
    let h = rect.bottom - rect.top;
    if w <= 0 || h <= 0 {
        bail!("screen-region fallback: window has zero/negative bounds: {w}x{h}");
    }

    let screen_dc = GetDC(HWND(std::ptr::null_mut())); // NULL HWND → desktop DC
    let mem_dc = CreateCompatibleDC(screen_dc);
    let bitmap = CreateCompatibleBitmap(screen_dc, w, h);
    let old_bitmap = SelectObject(mem_dc, bitmap);

    // Copy from screen coords (rect.left, rect.top) into our memory DC at (0, 0).
    let blt_ok = BitBlt(mem_dc, 0, 0, w, h, screen_dc, rect.left, rect.top, SRCCOPY);

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
        mem_dc, bitmap, 0, h as u32,
        Some(pixels.as_mut_ptr() as *mut _), &mut bmi, DIB_RGB_COLORS,
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
    unsafe { screenshot_window_bytes_unsafe(hwnd) }
}

/// Capture a window by HWND, returning (base64_png, width, height).
pub fn screenshot_window(hwnd: u64) -> Result<(String, u32, u32)> {
    let png_bytes = screenshot_window_bytes(hwnd)?;
    let (w, h) = {
        if png_bytes.len() < 24 { bail!("PNG too small"); }
        let w = u32::from_be_bytes([png_bytes[16], png_bytes[17], png_bytes[18], png_bytes[19]]);
        let h = u32::from_be_bytes([png_bytes[20], png_bytes[21], png_bytes[22], png_bytes[23]]);
        (w, h)
    };
    Ok((BASE64.encode(&png_bytes), w, h))
}

unsafe fn screenshot_window_bytes_unsafe(hwnd: u64) -> Result<Vec<u8>> {
    use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
    use windows::Win32::Foundation::RECT;

    let hwnd_raw = hwnd;
    let hwnd = HWND(hwnd as *mut _);

    // CUA-542 routing: for known XAML / WinUI3 / UWP targets, skip
    // PrintWindow entirely and go straight to the screen-region BitBlt
    // path. PrintWindow either returns all-black bitmaps for those
    // surfaces or — as observed for backgrounded Calculator — a tiny
    // clipped capture of the window's collapsed client rect. The
    // screen-region path reads from the live desktop DC, which has the
    // real composited image.
    if crate::input::is_xaml_host_hwnd(hwnd_raw) {
        match screenshot_via_screen_region(hwnd) {
            Ok((pixels, w, h)) => {
                return encode_bgra_to_png(&pixels, w as u32, h as u32);
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

    let mut rect = RECT::default();
    GetClientRect(hwnd, &mut rect)?;
    let w = (rect.right - rect.left) as i32;
    let h = (rect.bottom - rect.top) as i32;
    if w <= 0 || h <= 0 {
        bail!("Window has zero/negative client size: {}x{}", w, h);
    }

    let screen_dc = GetWindowDC(hwnd);
    let mem_dc = CreateCompatibleDC(screen_dc);
    let bitmap = CreateCompatibleBitmap(screen_dc, w, h);
    let old_bitmap = SelectObject(mem_dc, bitmap);

    let pw_ok = PrintWindow(hwnd, mem_dc, PW_RENDERFULLCONTENT);
    if !pw_ok.as_bool() {
        BitBlt(mem_dc, 0, 0, w, h, screen_dc, 0, 0, SRCCOPY)?;
    }

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
    let ok = GetDIBits(mem_dc, bitmap, 0, h as u32, Some(pixels.as_mut_ptr() as *mut _), &mut bmi, DIB_RGB_COLORS);

    SelectObject(mem_dc, old_bitmap);
    let _ = DeleteObject(bitmap);
    let _ = DeleteDC(mem_dc);
    ReleaseDC(hwnd, screen_dc);

    if ok == 0 { bail!("GetDIBits returned 0"); }

    // CUA-542: detect the all-black bitmap PrintWindow returns for
    // DirectComposition-backed UWP / WinUI3 surfaces and retry via
    // screen-region BitBlt. See module docs for the rationale.
    if is_mostly_black_bgra(&pixels) {
        match screenshot_via_screen_region(hwnd) {
            Ok((alt_pixels, alt_w, alt_h)) => {
                return encode_bgra_to_png(&alt_pixels, alt_w as u32, alt_h as u32);
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

    encode_bgra_to_png(&pixels, w as u32, h as u32)
}


/// Minimal BGRA→PNG encoder (no external dependency on image/lodepng).
fn encode_bgra_to_png(bgra: &[u8], w: u32, h: u32) -> Result<Vec<u8>> {
    // Convert BGRA to RGBA.
    let mut rgba = bgra.to_vec();
    for px in rgba.chunks_exact_mut(4) {
        px.swap(0, 2); // B↔R
    }

    // Build PNG in-memory using the flate2 + adler32 approach manually,
    // or simply call out to a temp file via GDI+ / stb_image_write.
    // For simplicity, use the `image` crate if available, otherwise write raw
    // uncompressed PNG (which most tools accept).
    write_uncompressed_png(&rgba, w, h)
}

/// Write a minimal uncompressed PNG (IDAT with zlib level 0 = store).
fn write_uncompressed_png(rgba: &[u8], w: u32, h: u32) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(rgba.len() + 4096);

    // PNG signature.
    out.extend_from_slice(b"\x89PNG\r\n\x1a\n");

    // IHDR.
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.push(8); // bit depth
    ihdr.push(2); // color type: RGB (we'll drop alpha for simplicity) — actually use 6 for RGBA
    ihdr[9] = 6;  // RGBA
    ihdr.push(0); // compression
    ihdr.push(0); // filter
    ihdr.push(0); // interlace
    // Rewrite properly.
    let ihdr: [u8; 13] = [
        (w >> 24) as u8, (w >> 16) as u8, (w >> 8) as u8, w as u8,
        (h >> 24) as u8, (h >> 16) as u8, (h >> 8) as u8, h as u8,
        8,   // bit depth
        6,   // RGBA
        0,   // deflate
        0,   // adaptive filter
        0,   // no interlace
    ];
    write_png_chunk(&mut out, b"IHDR", &ihdr);

    // IDAT: zlib-wrap with store (DEFLATE BTYPE=00).
    // Build raw scanlines: [filter_byte(0), row_pixels...]
    let row_bytes = (w * 4) as usize;
    let mut raw = Vec::with_capacity((row_bytes + 1) * h as usize);
    for row in 0..h as usize {
        raw.push(0u8); // filter = None
        raw.extend_from_slice(&rgba[row * row_bytes..(row + 1) * row_bytes]);
    }
    let zlib_data = zlib_store(&raw);
    write_png_chunk(&mut out, b"IDAT", &zlib_data);

    // IEND.
    write_png_chunk(&mut out, b"IEND", &[]);

    Ok(out)
}

fn write_png_chunk(out: &mut Vec<u8>, name: &[u8; 4], data: &[u8]) {
    let len = data.len() as u32;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(name);
    out.extend_from_slice(data);
    let crc = crc32_ieee(name, data);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// zlib store wrapper (BTYPE=00 non-compressed blocks, max 65535 bytes/block).
fn zlib_store(data: &[u8]) -> Vec<u8> {
    let adler = adler32(data);
    let mut out = Vec::new();
    // zlib header: CMF=0x78, FLG=0x01 (no dict, check bits).
    out.push(0x78);
    out.push(0x01);
    // DEFLATE non-compressed blocks.
    let mut pos = 0;
    while pos < data.len() || data.is_empty() {
        let end = (pos + 65535).min(data.len());
        let is_last = end == data.len();
        let blen = (end - pos) as u16;
        out.push(if is_last { 1 } else { 0 }); // BFINAL | BTYPE=00
        out.extend_from_slice(&blen.to_le_bytes());
        out.extend_from_slice(&(!blen).to_le_bytes());
        out.extend_from_slice(&data[pos..end]);
        pos = end;
        if data.is_empty() { break; }
    }
    // Adler-32 checksum (big-endian).
    out.extend_from_slice(&adler.to_be_bytes());
    out
}

fn adler32(data: &[u8]) -> u32 {
    let mut s1: u32 = 1;
    let mut s2: u32 = 0;
    for &b in data {
        s1 = (s1 + b as u32) % 65521;
        s2 = (s2 + s1) % 65521;
    }
    (s2 << 16) | s1
}

/// Capture the primary display (full screen), returning raw PNG bytes.
pub fn screenshot_display_bytes() -> Result<Vec<u8>> {
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
        let w = GetSystemMetrics(SM_CXSCREEN);
        let h = GetSystemMetrics(SM_CYSCREEN);
        if w <= 0 || h <= 0 { bail!("Could not get screen metrics"); }
        let screen_dc = GetDC(HWND::default());
        let mem_dc = CreateCompatibleDC(screen_dc);
        let bitmap = CreateCompatibleBitmap(screen_dc, w, h);
        let old_bitmap = SelectObject(mem_dc, bitmap);
        BitBlt(mem_dc, 0, 0, w, h, screen_dc, 0, 0, SRCCOPY)?;
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32,
                biCompression: BI_RGB.0,
                biSizeImage: (w * h * 4) as u32, ..Default::default()
            },
            bmiColors: [RGBQUAD::default(); 1],
        };
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        let ok = GetDIBits(mem_dc, bitmap, 0, h as u32, Some(pixels.as_mut_ptr() as *mut _), &mut bmi, DIB_RGB_COLORS);
        SelectObject(mem_dc, old_bitmap);
        let _ = DeleteObject(bitmap);
        let _ = DeleteDC(mem_dc);
        ReleaseDC(HWND::default(), screen_dc);
        if ok == 0 { bail!("GetDIBits returned 0"); }
        encode_bgra_to_png(&pixels, w as u32, h as u32)
    }
}

/// Capture primary display, returning (base64_png, width, height).
pub fn screenshot_display() -> Result<(String, u32, u32)> {
    let png_bytes = screenshot_display_bytes()?;
    if png_bytes.len() < 24 { bail!("PNG too small"); }
    let w = u32::from_be_bytes([png_bytes[16], png_bytes[17], png_bytes[18], png_bytes[19]]);
    let h = u32::from_be_bytes([png_bytes[20], png_bytes[21], png_bytes[22], png_bytes[23]]);
    Ok((BASE64.encode(&png_bytes), w, h))
}

/// Convert PNG bytes to JPEG at the given quality (1–95).
pub fn png_bytes_to_jpeg(png_bytes: &[u8], quality: u8) -> Result<Vec<u8>> {
    let img = image::load_from_memory_with_format(png_bytes, image::ImageFormat::Png)?;
    let mut buf = Vec::new();
    {
        let mut cursor = std::io::Cursor::new(&mut buf);
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cursor, quality);
        img.write_with_encoder(encoder)?;
    }
    Ok(buf)
}

/// Downscale `png_bytes` so neither dimension exceeds `max_dim`.
/// If `max_dim == 0` or the image already fits, returns a copy of the original bytes unchanged.
pub fn resize_png_if_needed(png_bytes: &[u8], max_dim: u32) -> Result<Vec<u8>> {
    if max_dim == 0 {
        return Ok(png_bytes.to_vec());
    }
    if png_bytes.len() < 24 { bail!("PNG too small"); }
    let w = u32::from_be_bytes([png_bytes[16], png_bytes[17], png_bytes[18], png_bytes[19]]);
    let h = u32::from_be_bytes([png_bytes[20], png_bytes[21], png_bytes[22], png_bytes[23]]);
    if w <= max_dim && h <= max_dim {
        return Ok(png_bytes.to_vec());
    }
    let scale = max_dim as f64 / w.max(h) as f64;
    let new_w = (w as f64 * scale).round() as u32;
    let new_h = (h as f64 * scale).round() as u32;
    let img = image::load_from_memory_with_format(png_bytes, image::ImageFormat::Png)?;
    let resized = img.resize(new_w, new_h, image::imageops::FilterType::Lanczos3);
    let mut out = Vec::new();
    resized.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)?;
    Ok(out)
}

/// Draw a red crosshair at pixel (cx, cy) on a PNG image and return modified PNG bytes.
/// Used by recording's click-marker callback to produce click.png.
pub fn crosshair_png_bytes(png_bytes: &[u8], cx: f64, cy: f64) -> Result<Vec<u8>> {
    let img = image::load_from_memory_with_format(png_bytes, image::ImageFormat::Png)?;
    let (w, h) = (img.width(), img.height());
    let mut img = img.to_rgba8();

    let arm_len = (w as f64 / 40.0).max(12.0) as i32;
    let line_w  = ((w as f64 / 400.0).max(1.5)) as i32;
    let red     = image::Rgba([255u8, 26, 26, 242]);
    let cx = cx as i32;
    let cy = cy as i32;

    for lw in 0..=line_w {
        let off = lw - line_w / 2;
        for dx in -arm_len..=arm_len {
            if let Some(p) = img.get_pixel_mut_checked(
                (cx + dx).clamp(0, w as i32 - 1) as u32,
                (cy + off).clamp(0, h as i32 - 1) as u32,
            ) { *p = red; }
        }
        for dy in -arm_len..=arm_len {
            if let Some(p) = img.get_pixel_mut_checked(
                (cx + off).clamp(0, w as i32 - 1) as u32,
                (cy + dy).clamp(0, h as i32 - 1) as u32,
            ) { *p = red; }
        }
    }

    let mut out = Vec::new();
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)?;
    Ok(out)
}

/// Parse width and height from a PNG IHDR chunk.
pub fn png_dimensions_pub(data: &[u8]) -> Result<(u32, u32)> {
    if data.len() < 24 { bail!("PNG data too small"); }
    if &data[0..8] != b"\x89PNG\r\n\x1a\n" { bail!("Not a PNG"); }
    let w = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
    let h = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
    Ok((w, h))
}

fn crc32_ieee(name: &[u8], data: &[u8]) -> u32 {
    const TABLE: [u32; 256] = {
        let mut t = [0u32; 256];
        let mut i = 0usize;
        while i < 256 {
            let mut c = i as u32;
            let mut j = 0;
            while j < 8 {
                c = if c & 1 != 0 { 0xEDB88320 ^ (c >> 1) } else { c >> 1 };
                j += 1;
            }
            t[i] = c;
            i += 1;
        }
        t
    };
    let mut crc: u32 = !0u32;
    for &b in name.iter().chain(data.iter()) {
        crc = TABLE[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

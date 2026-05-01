//! Window screenshot on Linux.
//!
//! Strategy (in order of preference):
//! 1. `xwd -id <xid> -silent | xwdtopnm | pnmtopng` (X11, no focus change)
//! 2. `import -window <xid> png:-` (ImageMagick, widely available)
//! 3. `scrot -u <file>` (focused window fallback)
//! 4. XGetImage via x11rb (pure Rust, no subprocess)

use anyhow::{bail, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use std::process::Command;

/// Capture a window by X11 XID. Returns raw PNG bytes.
pub fn screenshot_window_bytes(xid: u64) -> Result<Vec<u8>> {
    // Try `import -window <xid> png:-` (ImageMagick).
    if let Ok(bytes) = capture_via_import(xid) {
        return Ok(bytes);
    }
    // Fallback: x11rb XGetImage → returns (b64, w, h); decode the b64 back.
    let (b64, _, _) = capture_via_xgetimage(xid)?;
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD.decode(&b64)?;
    Ok(bytes)
}

/// Capture a window by X11 XID. Returns (base64_png, width, height).
pub fn screenshot_window(xid: u64) -> Result<(String, u32, u32)> {
    // Try `import -window <xid> png:-` (ImageMagick).
    if let Ok(bytes) = capture_via_import(xid) {
        let (w, h) = png_dimensions(&bytes)?;
        return Ok((BASE64.encode(&bytes), w, h));
    }

    // Fallback: x11rb XGetImage.
    capture_via_xgetimage(xid)
}

fn capture_via_import(xid: u64) -> Result<Vec<u8>> {
    let out = Command::new("import")
        .args(["-window", &xid.to_string(), "png:-"])
        .output()?;
    if !out.status.success() || out.stdout.is_empty() {
        bail!("import failed");
    }
    Ok(out.stdout)
}

fn capture_via_xgetimage(xid: u64) -> Result<(String, u32, u32)> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::*;
    use x11rb::rust_connection::RustConnection;

    let (conn, _) = RustConnection::connect(None)?;
    let window = xid as u32;

    let geom = conn.get_geometry(window)?.reply()?;
    let w = geom.width as u32;
    let h = geom.height as u32;

    let img = conn.get_image(
        ImageFormat::Z_PIXMAP,
        window,
        0, 0,
        w as u16, h as u16,
        !0u32,
    )?.reply()?;

    // The raw data is BGRA or BGRX depending on depth.
    // Encode as a minimal PNG.
    let bytes = img.data;
    let (bpp, has_alpha) = match img.depth {
        32 => (4usize, true),
        24 => (4usize, false),
        _  => bail!("Unsupported depth: {}", img.depth),
    };

    // Convert to RGBA.
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for chunk in bytes.chunks_exact(bpp) {
        let (b, g, r) = (chunk[0], chunk[1], chunk[2]);
        let a = if has_alpha { chunk[3] } else { 255 };
        rgba.extend_from_slice(&[r, g, b, a]);
    }

    let png = write_uncompressed_png(&rgba, w, h)?;
    Ok((BASE64.encode(&png), w, h))
}

/// Public version of png_dimensions for use in tool code.
pub fn png_dimensions_pub(data: &[u8]) -> Result<(u32, u32)> {
    png_dimensions(data)
}

fn png_dimensions(data: &[u8]) -> Result<(u32, u32)> {
    if data.len() < 24 { bail!("PNG too small"); }
    // Signature (8) + IHDR length (4) + "IHDR" (4) + width (4) + height (4)
    let w = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
    let h = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
    Ok((w, h))
}

// Inline minimal uncompressed PNG writer (same as platform-windows/capture.rs).
fn write_uncompressed_png(rgba: &[u8], w: u32, h: u32) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(rgba.len() + 4096);
    out.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let ihdr: [u8; 13] = [
        (w >> 24) as u8, (w >> 16) as u8, (w >> 8) as u8, w as u8,
        (h >> 24) as u8, (h >> 16) as u8, (h >> 8) as u8, h as u8,
        8, 6, 0, 0, 0,
    ];
    write_png_chunk(&mut out, b"IHDR", &ihdr);
    let row_bytes = (w * 4) as usize;
    let mut raw = Vec::with_capacity((row_bytes + 1) * h as usize);
    for row in 0..h as usize {
        raw.push(0u8);
        raw.extend_from_slice(&rgba[row * row_bytes..(row + 1) * row_bytes]);
    }
    let zlib_data = zlib_store(&raw);
    write_png_chunk(&mut out, b"IDAT", &zlib_data);
    write_png_chunk(&mut out, b"IEND", &[]);
    Ok(out)
}

fn write_png_chunk(out: &mut Vec<u8>, name: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(name);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc32_ieee(name, data).to_be_bytes());
}

fn zlib_store(data: &[u8]) -> Vec<u8> {
    let adler = adler32(data);
    let mut out = vec![0x78, 0x01];
    let mut pos = 0;
    loop {
        let end = (pos + 65535).min(data.len());
        let is_last = end == data.len();
        let blen = (end - pos) as u16;
        out.push(if is_last { 1 } else { 0 });
        out.extend_from_slice(&blen.to_le_bytes());
        out.extend_from_slice(&(!blen).to_le_bytes());
        out.extend_from_slice(&data[pos..end]);
        pos = end;
        if pos >= data.len() { break; }
    }
    out.extend_from_slice(&adler.to_be_bytes());
    out
}

fn adler32(data: &[u8]) -> u32 {
    let (mut s1, mut s2) = (1u32, 0u32);
    for &b in data { s1 = (s1 + b as u32) % 65521; s2 = (s2 + s1) % 65521; }
    (s2 << 16) | s1
}

/// Capture the primary display (root window) as raw PNG bytes.
pub fn screenshot_display_bytes() -> Result<Vec<u8>> {
    // Try `import -window root png:-` (ImageMagick).
    let out = Command::new("import")
        .args(["-window", "root", "png:-"])
        .output();
    if let Ok(o) = out {
        if o.status.success() && !o.stdout.is_empty() {
            return Ok(o.stdout);
        }
    }
    // Fallback: x11rb XGetImage on the root window.
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::*;
    use x11rb::rust_connection::RustConnection;
    let (conn, screen_num) = RustConnection::connect(None)?;
    let root = conn.setup().roots[screen_num].root;
    // Get root geometry.
    let geom = conn.get_geometry(root)?.reply()?;
    let w = geom.width as u32;
    let h = geom.height as u32;
    let img = conn.get_image(ImageFormat::Z_PIXMAP, root, 0, 0, w as u16, h as u16, !0u32)?.reply()?;
    let bytes = img.data;
    let bpp = match img.depth { 32 | 24 => 4usize, _ => anyhow::bail!("Unsupported depth") };
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for chunk in bytes.chunks_exact(bpp) {
        let (b, g, r) = (chunk[0], chunk[1], chunk[2]);
        rgba.extend_from_slice(&[r, g, b, 255]);
    }
    write_uncompressed_png(&rgba, w, h)
}

/// Capture the primary display, returning (base64_png, width, height).
pub fn screenshot_display() -> Result<(String, u32, u32)> {
    let png_bytes = screenshot_display_bytes()?;
    let (w, h) = png_dimensions(&png_bytes)?;
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
    let (w, h) = png_dimensions_pub(png_bytes)?;
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

fn crc32_ieee(name: &[u8], data: &[u8]) -> u32 {
    const T: [u32; 256] = {
        let mut t = [0u32; 256];
        let mut i = 0usize;
        while i < 256 {
            let mut c = i as u32;
            let mut j = 0;
            while j < 8 { c = if c & 1 != 0 { 0xEDB88320 ^ (c >> 1) } else { c >> 1 }; j += 1; }
            t[i] = c; i += 1;
        }
        t
    };
    let mut crc = !0u32;
    for &b in name.iter().chain(data.iter()) { crc = T[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8); }
    !crc
}

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
        let (w, h) = mcp_server::image_utils::png_dimensions(&bytes)?;
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

    let png = mcp_server::image_utils::encode_rgba_to_png(&rgba, w, h)?;
    Ok((BASE64.encode(&png), w, h))
}

/// Public version of png_dimensions for use in tool code.
pub fn png_dimensions_pub(data: &[u8]) -> Result<(u32, u32)> {
    mcp_server::image_utils::png_dimensions(data)
}

// NOTE: the previously-inline `png_dimensions`, `write_uncompressed_png`,
// `write_png_chunk`, `zlib_store`, `adler32` (and `crc32_ieee` below)
// were extracted to `mcp_server::image_utils` in the 2026-05 dedup
// audit so all three platforms call the same code. See
// `CUA_DRIVER_RS_DEDUP_AUDIT.md`. RGBA-encoding callers below now go
// through `mcp_server::image_utils::encode_rgba_to_png`.

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
    mcp_server::image_utils::encode_rgba_to_png(&rgba, w, h)
}

/// Capture the primary display, returning (base64_png, width, height).
pub fn screenshot_display() -> Result<(String, u32, u32)> {
    let png_bytes = screenshot_display_bytes()?;
    let (w, h) = mcp_server::image_utils::png_dimensions(&png_bytes)?;
    Ok((BASE64.encode(&png_bytes), w, h))
}

// PNG/JPEG/resize/crosshair helpers — re-exports of the shared
// `mcp_server::image_utils` module. The previous file-local copies were
// near-identical to the macOS and Windows versions; the dedup-audit
// (2026-05) moved them all to one place.

/// Convert PNG bytes to JPEG at the given quality (1–95).
pub fn png_bytes_to_jpeg(png_bytes: &[u8], quality: u8) -> Result<Vec<u8>> {
    mcp_server::image_utils::png_bytes_to_jpeg(png_bytes, quality)
}

/// Downscale `png_bytes` so neither dimension exceeds `max_dim`.
/// If `max_dim == 0` or the image already fits, returns a copy of the
/// original bytes unchanged.
pub fn resize_png_if_needed(png_bytes: &[u8], max_dim: u32) -> Result<Vec<u8>> {
    mcp_server::image_utils::resize_png_if_needed(png_bytes, max_dim)
}

/// Draw a red crosshair at pixel (cx, cy) on a PNG image and return
/// modified PNG bytes. Used by recording's click-marker callback to
/// produce click.png.
pub fn crosshair_png_bytes(png_bytes: &[u8], cx: f64, cy: f64) -> Result<Vec<u8>> {
    mcp_server::image_utils::crosshair_png_bytes(png_bytes, cx, cy)
}


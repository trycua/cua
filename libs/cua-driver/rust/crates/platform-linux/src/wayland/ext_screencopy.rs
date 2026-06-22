//! Output capture via `ext-image-copy-capture-v1` (staging protocol).
//!
//! The wlroots `zwlr_screencopy_manager_v1` capture path in `mod.rs` works
//! on wlroots compositors (sway, labwc) but is missing from compositors
//! that don't speak wlroots protocols (GNOME mutter, KDE Plasma <6.2).
//! The staging `ext-image-copy-capture-v1` protocol replaces wlr-screencopy
//! across the ecosystem — sway 1.10+, labwc 0.8+, niri 0.1.6+, hyprland
//! 0.45+, KDE Plasma 6.2+, GNOME mutter 47+ all implement it.
//!
//! This module captures the FIRST `wl_output` advertised by the compositor
//! via the chain:
//!   ext_output_image_capture_source_manager_v1.create_source(wl_output)
//!     → ext_image_copy_capture_manager_v1.create_session(source, options)
//!     → session emits buffer_size + done
//!     → allocate wl_shm pool + wl_buffer matching the size/format
//!     → session.create_frame() → frame.attach_buffer(buffer) → frame.capture()
//!     → frame emits ready (success) or failed (error)
//!     → read pixels out of the mmap'd buffer, PNG-encode, destroy
//!
//! Per-window capture (ext_foreign_toplevel_image_capture_source_manager_v1)
//! is a follow-up — needs the existing `list_windows` to migrate from
//! `zwlr_foreign_toplevel_manager_v1` to `ext_foreign_toplevel_list_v1` so
//! the window_id handles match up.

use std::time::Duration;

use wayland_client::{
    protocol::{
        wl_buffer::WlBuffer,
        wl_output::WlOutput,
        wl_registry,
        wl_shm::{self, WlShm},
        wl_shm_pool::WlShmPool,
    },
    Connection, Dispatch, Proxy, QueueHandle,
};
use wayland_protocols::ext::image_capture_source::v1::client::{
    ext_image_capture_source_v1::ExtImageCaptureSourceV1,
    ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1,
};
use wayland_protocols::ext::image_copy_capture::v1::client::{
    ext_image_copy_capture_frame_v1::{self, ExtImageCopyCaptureFrameV1},
    ext_image_copy_capture_manager_v1::{self, ExtImageCopyCaptureManagerV1},
    ext_image_copy_capture_session_v1::{self, ExtImageCopyCaptureSessionV1},
};

#[derive(Default)]
struct CapState {
    output: Option<WlOutput>,
    source_mgr: Option<ExtOutputImageCaptureSourceManagerV1>,
    copy_mgr: Option<ExtImageCopyCaptureManagerV1>,
    shm: Option<WlShm>,
    // Buffer specs reported by the session before we allocate.
    fmt: Option<u32>,
    width: u32,
    height: u32,
    stride: u32,
    done: bool,
    // Frame outcome
    ready: bool,
    failed: bool,
    fail_reason: Option<u32>,
}

/// Capture the first `wl_output` via ext-image-copy-capture-v1, return raw
/// PNG bytes. Falls back to the caller's next tier when the compositor
/// doesn't expose the protocol — surface a typed anyhow::Error so the
/// cascade in `screenshot_display_dispatch` can branch.
pub fn screenshot_via_ext_copy() -> anyhow::Result<Vec<u8>> {
    let conn = Connection::connect_to_env()?;
    let mut queue = conn.new_event_queue::<CapState>();
    let qh = queue.handle();
    conn.display().get_registry(&qh, ());

    let mut state = CapState::default();
    queue.roundtrip(&mut state)?;
    for _ in 0..3 {
        queue.roundtrip(&mut state)?;
    }

    let output = state
        .output
        .clone()
        .ok_or_else(|| anyhow::anyhow!("compositor exposed no wl_output"))?;
    let source_mgr = state.source_mgr.clone().ok_or_else(|| {
        anyhow::anyhow!("compositor does not expose ext_output_image_capture_source_manager_v1")
    })?;
    let copy_mgr = state.copy_mgr.clone().ok_or_else(|| {
        anyhow::anyhow!("compositor does not expose ext_image_copy_capture_manager_v1")
    })?;
    let shm = state
        .shm
        .clone()
        .ok_or_else(|| anyhow::anyhow!("compositor does not expose wl_shm"))?;

    // Create source + session.
    let source: ExtImageCaptureSourceV1 = source_mgr.create_source(&output, &qh, ());
    let session: ExtImageCopyCaptureSessionV1 = copy_mgr.create_session(
        &source,
        ext_image_copy_capture_manager_v1::Options::empty(),
        &qh,
        (),
    );

    // Wait for the session to announce buffer dimensions + format + done.
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while !state.done && std::time::Instant::now() < deadline {
        queue.roundtrip(&mut state)?;
        if state.width > 0 && state.height > 0 && state.fmt.is_some() {
            // Some compositors don't emit `done` explicitly — proceed once
            // we have geometry + format.
            state.done = true;
        }
    }
    if !state.done || state.width == 0 || state.height == 0 || state.fmt.is_none() {
        anyhow::bail!(
            "ext-image-copy-capture session never reported buffer specs (w={}, h={}, fmt={:?})",
            state.width, state.height, state.fmt
        );
    }

    // Checked arithmetic on compositor-provided sizes. A malicious or
    // buggy compositor could announce dimensions that overflow `usize` on
    // multiplication — we refuse to allocate in that case rather than
    // wrapping and reading past the buffer.
    let stride = if state.stride > 0 {
        state.stride
    } else {
        state.width
            .checked_mul(4)
            .ok_or_else(|| anyhow::anyhow!("compositor advertised width that overflows stride: {}", state.width))?
    };
    if stride < state.width.saturating_mul(4) {
        anyhow::bail!(
            "compositor stride {} is smaller than width*4 {}; refusing to allocate (would alias rows)",
            stride, state.width.saturating_mul(4)
        );
    }
    let size = (stride as usize)
        .checked_mul(state.height as usize)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "compositor buffer size overflows usize: stride={} height={}",
                stride, state.height
            )
        })?;

    // Allocate the wl_shm buffer.
    let (fd, ptr) = super::anon_shm(size)?;
    use std::os::fd::AsFd as _;
    let pool_fd = unsafe { super::borrowed_fd(fd) };
    let pool: WlShmPool = shm.create_pool(pool_fd.as_fd(), size as i32, &qh, ());
    let fmt: wl_shm::Format = wl_shm::Format::try_from(state.fmt.unwrap()).map_err(|_| {
        anyhow::anyhow!("compositor advertised unsupported wl_shm format {:#x}", state.fmt.unwrap())
    })?;
    let buffer: WlBuffer = pool.create_buffer(
        0,
        state.width as i32,
        state.height as i32,
        stride as i32,
        fmt,
        &qh,
        (),
    );

    // Create + run the frame.
    let frame: ExtImageCopyCaptureFrameV1 = session.create_frame(&qh, ());
    frame.attach_buffer(&buffer);
    frame.capture();
    queue.roundtrip(&mut state)?;

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !state.ready && !state.failed && std::time::Instant::now() < deadline {
        queue.roundtrip(&mut state)?;
    }
    if state.failed {
        super::cleanup_mmap(ptr, size, fd);
        anyhow::bail!(
            "ext-image-copy-capture frame failed (reason={:?})",
            state.fail_reason
        );
    }
    if !state.ready {
        super::cleanup_mmap(ptr, size, fd);
        anyhow::bail!("ext-image-copy-capture frame timed out without ready event");
    }

    // Copy + encode + cleanup.
    let pixels: &[u8] = unsafe { std::slice::from_raw_parts(ptr as *const u8, size) };
    let png = encode_buffer_to_png(pixels, state.width, state.height, stride, fmt)?;

    frame.destroy();
    session.destroy();
    source.destroy();
    pool.destroy();
    queue.roundtrip(&mut state).ok();
    super::cleanup_mmap(ptr, size, fd);

    Ok(png)
}

fn encode_buffer_to_png(
    pixels: &[u8],
    width: u32,
    height: u32,
    stride: u32,
    fmt: wl_shm::Format,
) -> anyhow::Result<Vec<u8>> {
    // The image crate expects RGBA8888 row-packed. wl_shm gives us Argb8888
    // / Xrgb8888 which in little-endian memory layout is BGRA / BGRX. Swap
    // channels into RGBA and pack rows.
    let mut rgba: Vec<u8> = Vec::with_capacity((width as usize) * (height as usize) * 4);
    for y in 0..(height as usize) {
        let row_start = y * (stride as usize);
        for x in 0..(width as usize) {
            let i = row_start + x * 4;
            let b = pixels[i];
            let g = pixels[i + 1];
            let r = pixels[i + 2];
            let a = match fmt {
                wl_shm::Format::Xrgb8888 => 0xFF,
                _ => pixels[i + 3],
            };
            rgba.extend_from_slice(&[r, g, b, a]);
        }
    }

    use image::{ImageBuffer, Rgba, codecs::png::PngEncoder};
    let img: ImageBuffer<Rgba<u8>, _> =
        ImageBuffer::from_raw(width, height, rgba).ok_or_else(|| {
            anyhow::anyhow!("internal: buffer dims mismatch ({}x{})", width, height)
        })?;
    let mut out: Vec<u8> = Vec::new();
    use image::ImageEncoder;
    PngEncoder::new(&mut out).write_image(
        img.as_raw(),
        width,
        height,
        image::ExtendedColorType::Rgba8,
    )?;
    Ok(out)
}

// ── Wayland Dispatch impls ───────────────────────────────────────────────

impl Dispatch<wl_registry::WlRegistry, ()> for CapState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global { name, interface, version } = event {
            match interface.as_str() {
                "wl_output" => {
                    if state.output.is_none() {
                        state.output = Some(registry.bind::<WlOutput, _, _>(name, version.min(4), qh, ()));
                    }
                }
                "wl_shm" => {
                    state.shm = Some(registry.bind::<WlShm, _, _>(name, 1, qh, ()));
                }
                "ext_output_image_capture_source_manager_v1" => {
                    state.source_mgr = Some(registry.bind::<ExtOutputImageCaptureSourceManagerV1, _, _>(
                        name, 1, qh, ()));
                }
                "ext_image_copy_capture_manager_v1" => {
                    state.copy_mgr = Some(registry.bind::<ExtImageCopyCaptureManagerV1, _, _>(
                        name, 1, qh, ()));
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<WlOutput, ()> for CapState {
    fn event(_: &mut Self, _: &WlOutput, _: <WlOutput as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<WlShm, ()> for CapState {
    fn event(_: &mut Self, _: &WlShm, _: <WlShm as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<WlShmPool, ()> for CapState {
    fn event(_: &mut Self, _: &WlShmPool, _: <WlShmPool as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<WlBuffer, ()> for CapState {
    fn event(_: &mut Self, _: &WlBuffer, _: <WlBuffer as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<ExtOutputImageCaptureSourceManagerV1, ()> for CapState {
    fn event(_: &mut Self, _: &ExtOutputImageCaptureSourceManagerV1, _: <ExtOutputImageCaptureSourceManagerV1 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<ExtImageCaptureSourceV1, ()> for CapState {
    fn event(_: &mut Self, _: &ExtImageCaptureSourceV1, _: <ExtImageCaptureSourceV1 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<ExtImageCopyCaptureManagerV1, ()> for CapState {
    fn event(_: &mut Self, _: &ExtImageCopyCaptureManagerV1, _: <ExtImageCopyCaptureManagerV1 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<ExtImageCopyCaptureSessionV1, ()> for CapState {
    fn event(
        state: &mut Self,
        _: &ExtImageCopyCaptureSessionV1,
        event: <ExtImageCopyCaptureSessionV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use ext_image_copy_capture_session_v1::Event as Ev;
        match event {
            Ev::BufferSize { width, height } => {
                state.width = width;
                state.height = height;
            }
            Ev::ShmFormat { format } => {
                if state.fmt.is_none() {
                    if let Ok(f) = format.into_result() {
                        state.fmt = Some(f as u32);
                    }
                }
            }
            Ev::Done => {
                state.done = true;
            }
            _ => {}
        }
    }
}
impl Dispatch<ExtImageCopyCaptureFrameV1, ()> for CapState {
    fn event(
        state: &mut Self,
        _: &ExtImageCopyCaptureFrameV1,
        event: <ExtImageCopyCaptureFrameV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use ext_image_copy_capture_frame_v1::Event as Ev;
        match event {
            Ev::Ready => state.ready = true,
            Ev::Failed { reason } => {
                state.failed = true;
                state.fail_reason = reason.into_result().ok().map(|r| r as u32);
            }
            _ => {}
        }
    }
}

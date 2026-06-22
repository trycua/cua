//! Per-window capture via `xdg-desktop-portal` ScreenCast + PipeWire.
//!
//! Reaches GNOME/Mutter and KDE/KWin per-window selection — the
//! cross-DE complement to wlroots `ext-image-copy-capture-v1`. Sequence:
//!
//! 1. ashpd ScreenCast::create_session.
//! 2. select_sources with types=Window|Monitor, cursor_mode=Embedded,
//!    persist_mode=Application (consent dialog fires ONCE per process,
//!    cached for the lifetime of the requesting binary).
//! 3. start(session, parent_window) — user picks the source window in
//!    the portal's system dialog the first time; subsequent calls within
//!    the same process reuse the cached choice.
//! 4. open_pipe_wire_remote returns a Unix file descriptor; Streams
//!    carries the published node ids.
//! 5. Connect a pipewire client to the fd, open a Stream targeting the
//!    advertised node id, negotiate Video/Raw format, dequeue one frame
//!    with `STREAM_FLAG_MAP_BUFFERS`, copy pixels, PNG-encode, stop.
//!
//! The session + restore_token are persisted in a process-local
//! `OnceLock<PortalScreencastSession>` so only the first call across the
//! process lifetime triggers the consent dialog; later calls reuse the
//! same PipeWire node id.

use std::os::fd::AsRawFd;
use std::sync::OnceLock;

use ashpd::desktop::screencast::{
    CursorMode, OpenPipeWireRemoteOptions, Screencast,
    SelectSourcesOptions, SourceType, StartCastOptions, Streams,
};
use ashpd::desktop::{CreateSessionOptions, PersistMode, Session};
use ashpd::enumflags2::BitFlags;

/// A live portal ScreenCast session held across calls. Expensive to
/// create (system consent dialog) and cheap to reuse — the OnceLock
/// keeps it alive until the requesting process exits.
struct PortalScreencastSession {
    _session: Session<Screencast>,
    streams: Streams,
    fd: std::os::fd::OwnedFd,
}

unsafe impl Send for PortalScreencastSession {}
unsafe impl Sync for PortalScreencastSession {}

static SESSION: OnceLock<PortalScreencastSession> = OnceLock::new();

/// Capture the user-selected window via `xdg-desktop-portal` ScreenCast.
/// On first invocation per process the portal dialog asks the user to
/// pick a window or monitor; subsequent calls reuse the same node
/// transparently. Returns PNG bytes of the latest frame on the stream.
pub fn screenshot_window_via_portal() -> anyhow::Result<Vec<u8>> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| anyhow::anyhow!("failed to build tokio runtime for ashpd: {e}"))?;

    if SESSION.get().is_none() {
        let sess = rt.block_on(create_portal_session())?;
        let _ = SESSION.set(sess);
    }
    let session = SESSION
        .get()
        .ok_or_else(|| anyhow::anyhow!("portal ScreenCast session unavailable"))?;

    let node_id = session
        .streams
        .streams()
        .iter()
        .next()
        .map(|s| s.pipe_wire_node_id())
        .ok_or_else(|| anyhow::anyhow!("portal advertised no streams"))?;

    capture_one_frame(session.fd.as_raw_fd(), node_id)
}

async fn create_portal_session() -> anyhow::Result<PortalScreencastSession> {
    let proxy = Screencast::new()
        .await
        .map_err(|e| anyhow::anyhow!("portal ScreenCast proxy unreachable: {e}. Install xdg-desktop-portal-gnome / xdg-desktop-portal-kde."))?;

    let session = proxy
        .create_session(CreateSessionOptions::default())
        .await
        .map_err(|e| anyhow::anyhow!("portal create_session failed: {e}"))?;

    let select_opts = SelectSourcesOptions::default()
        .set_sources(BitFlags::<SourceType>::from(SourceType::Window) | SourceType::Monitor)
        .set_cursor_mode(CursorMode::Embedded)
        .set_persist_mode(PersistMode::Application);

    proxy
        .select_sources(&session, select_opts)
        .await
        .map_err(|e| anyhow::anyhow!("portal select_sources failed: {e}"))?
        .response()
        .map_err(|e| anyhow::anyhow!("portal select_sources response error: {e}"))?;

    let streams = proxy
        .start(&session, None, StartCastOptions::default())
        .await
        .map_err(|e| anyhow::anyhow!("portal start failed: {e}"))?
        .response()
        .map_err(|e| anyhow::anyhow!("portal start response error (user denied?): {e}"))?;

    let fd = proxy
        .open_pipe_wire_remote(&session, OpenPipeWireRemoteOptions::default())
        .await
        .map_err(|e| anyhow::anyhow!("portal open_pipe_wire_remote failed: {e}"))?;

    Ok(PortalScreencastSession {
        _session: session,
        streams,
        fd,
    })
}

/// Pull one frame off the PipeWire stream advertised by the portal,
/// encode it as PNG, and return the bytes.
///
/// Pipeline:
/// 1. `pipewire::MainLoop` + `Context` + `Core::connect_fd(portal_fd)` —
///    join the portal-published PipeWire remote on the same fd ashpd
///    returned from `open_pipe_wire_remote`.
/// 2. `Stream::new` with `media.type=Video, media.category=Capture,
///    media.role=Screen` so PipeWire knows to route the cast properly.
/// 3. Build an `EnumFormat` SPA pod listing the channel-orders we accept
///    (BGRx, BGRA, RGBx, RGBA) at any size up to 8K — the compositor
///    picks one and announces it via `param_changed`. We track w/h/fmt
///    from that callback.
/// 4. The `process` callback dequeues exactly one buffer, copies the
///    pixels out into a shared `Arc<Mutex<...>>`, and quits the main
///    loop. Subsequent buffers are dropped (queued back via Buffer::Drop)
///    once `frame.is_some()`.
/// 5. After `mainloop.run()` returns, channel-swap the captured bytes
///    into RGBA and PNG-encode them via the `image` crate.
fn capture_one_frame(pipewire_fd: i32, node_id: u32) -> anyhow::Result<Vec<u8>> {
    use std::os::fd::FromRawFd;
    use std::sync::{Arc, Mutex};

    use pipewire as pw;
    use pw::{properties::properties, spa};
    use spa::pod::Pod;

    pw::init();

    let mainloop = pw::main_loop::MainLoop::new(None)
        .map_err(|e| anyhow::anyhow!("pipewire MainLoop::new failed: {e}"))?;
    let context = pw::context::Context::new(&mainloop)
        .map_err(|e| anyhow::anyhow!("pipewire Context::new failed: {e}"))?;

    // dup the portal fd so the OwnedFd we hand to connect_fd has an
    // independent lifecycle from the PortalScreencastSession's master copy.
    let dup_fd = unsafe { libc::dup(pipewire_fd) };
    if dup_fd < 0 {
        anyhow::bail!(
            "dup of portal PipeWire fd failed: {}",
            std::io::Error::last_os_error()
        );
    }
    let owned_fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(dup_fd) };
    let core = context
        .connect_fd(owned_fd, None)
        .map_err(|e| anyhow::anyhow!("pipewire connect_fd failed: {e}"))?;

    // Shared state populated by the param_changed + process callbacks.
    // FrameBuf carries the negotiated geometry alongside the pixel data so
    // the PNG encoder picks the right stride / channel order on exit.
    struct FrameBuf {
        bytes: Vec<u8>,
        width: u32,
        height: u32,
        stride: u32,
        format: spa::param::video::VideoFormat,
    }
    let frame: Arc<Mutex<Option<FrameBuf>>> = Arc::new(Mutex::new(None));
    let neg_format: Arc<Mutex<spa::param::video::VideoInfoRaw>> =
        Arc::new(Mutex::new(Default::default()));

    let stream = pw::stream::Stream::new(
        &core,
        "cua-driver-capture",
        properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )
    .map_err(|e| anyhow::anyhow!("pipewire Stream::new failed: {e}"))?;

    let mainloop_for_process = mainloop.clone();
    let neg_format_for_param = neg_format.clone();
    let frame_for_process = frame.clone();
    let neg_format_for_process = neg_format.clone();

    let _listener = stream
        .add_local_listener::<()>()
        .param_changed(move |_stream, _user, id, param| {
            // The compositor confirms the negotiated VideoInfoRaw via the
            // Format param event right after stream.connect — capture it
            // so the process callback knows the width/height/format.
            let Some(param) = param else { return };
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }
            let (media_type, media_subtype) = match spa::param::format_utils::parse_format(param) {
                Ok(v) => v,
                Err(_) => return,
            };
            if media_type != spa::param::format::MediaType::Video
                || media_subtype != spa::param::format::MediaSubtype::Raw
            {
                return;
            }
            let mut info = spa::param::video::VideoInfoRaw::default();
            if info.parse(param).is_ok() {
                if let Ok(mut g) = neg_format_for_param.lock() {
                    *g = info;
                }
            }
        })
        .process(move |stream, _user| {
            // Already captured? Drop further buffers (they'll be queued
            // back via Buffer::Drop) so the run loop can finish.
            if frame_for_process.lock().ok().map(|g| g.is_some()).unwrap_or(false) {
                let _ = stream.dequeue_buffer();
                return;
            }
            let mut buffer = match stream.dequeue_buffer() {
                Some(b) => b,
                None => return,
            };
            let datas = buffer.datas_mut();
            if datas.is_empty() {
                return;
            }
            let info = match neg_format_for_process.lock() {
                Ok(g) => *g,
                Err(_) => return,
            };
            let size_struct = info.size();
            let width = size_struct.width;
            let height = size_struct.height;
            if width == 0 || height == 0 {
                return;
            }
            let chunk_stride = datas[0].chunk().stride();
            let chunk_size = datas[0].chunk().size();
            // PipeWire sometimes leaves stride at 0 (when the producer
            // doesn't fill it in); fall back to width*4 for the packed
            // BGRx/BGRA/RGBx/RGBA formats we negotiate.
            let stride = if chunk_stride > 0 {
                chunk_stride as u32
            } else if chunk_size > 0 && height > 0 {
                chunk_size / height
            } else {
                width * 4
            };
            let payload = match datas[0].data() {
                Some(p) => p,
                None => return,
            };
            let payload_len = (stride as usize) * (height as usize);
            if payload.len() < payload_len {
                return;
            }
            let bytes = payload[..payload_len].to_vec();
            if let Ok(mut g) = frame_for_process.lock() {
                *g = Some(FrameBuf {
                    bytes,
                    width,
                    height,
                    stride,
                    format: info.format(),
                });
            }
            // Quit the main loop so the caller can encode + return.
            mainloop_for_process.quit();
        })
        .register()
        .map_err(|e| anyhow::anyhow!("pipewire stream listener register failed: {e}"))?;

    // Build an EnumFormat SPA pod listing the formats + sizes we accept.
    // Compositors typically pick BGRx/BGRA on Linux desktops; we list a
    // few alternates so negotiation succeeds on more sources.
    let obj = pw::spa::pod::object!(
        pw::spa::utils::SpaTypes::ObjectParamFormat,
        pw::spa::param::ParamType::EnumFormat,
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::MediaType,
            Id,
            pw::spa::param::format::MediaType::Video
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::MediaSubtype,
            Id,
            pw::spa::param::format::MediaSubtype::Raw
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            pw::spa::param::video::VideoFormat::BGRx,
            pw::spa::param::video::VideoFormat::BGRx,
            pw::spa::param::video::VideoFormat::BGRA,
            pw::spa::param::video::VideoFormat::RGBx,
            pw::spa::param::video::VideoFormat::RGBA,
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            pw::spa::utils::Rectangle { width: 1920, height: 1080 },
            pw::spa::utils::Rectangle { width: 1, height: 1 },
            pw::spa::utils::Rectangle { width: 7680, height: 4320 }
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            pw::spa::utils::Fraction { num: 30, denom: 1 },
            pw::spa::utils::Fraction { num: 0, denom: 1 },
            pw::spa::utils::Fraction { num: 1000, denom: 1 }
        ),
    );
    let values: Vec<u8> = pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(obj),
    )
    .map_err(|e| anyhow::anyhow!("EnumFormat pod serialization failed: {e}"))?
    .0
    .into_inner();
    let mut params = [Pod::from_bytes(&values)
        .ok_or_else(|| anyhow::anyhow!("EnumFormat pod parse-back failed"))?];

    stream
        .connect(
            spa::utils::Direction::Input,
            Some(node_id),
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .map_err(|e| anyhow::anyhow!("pipewire stream.connect failed: {e}"))?;

    // Install a 10-second timeout source so a producer that never delivers
    // a frame (eg the user closed the consent dialog mid-cast) doesn't
    // hang the main loop forever.
    let timeout_loop = mainloop.clone();
    let _timeout_source = mainloop
        .loop_()
        .add_timer(move |_expirations: u64| timeout_loop.quit());
    _timeout_source
        .update_timer(Some(std::time::Duration::from_secs(10)), None)
        .into_result()
        .map_err(|e| anyhow::anyhow!("pipewire timeout source failed: {e:?}"))?;

    mainloop.run();

    // Tear down the stream before we read the frame, so the producer
    // can't keep mutating the buffer underneath us.
    drop(_listener);
    drop(stream);

    let frame = frame
        .lock()
        .map_err(|_| anyhow::anyhow!("frame mutex poisoned"))?
        .take()
        .ok_or_else(|| anyhow::anyhow!("portal ScreenCast: no frame arrived within timeout"))?;

    encode_frame_to_png(&frame.bytes, frame.width, frame.height, frame.stride, frame.format)
}

/// Channel-swap a PipeWire video frame into RGBA8888 and PNG-encode it.
///
/// Mirrors `ext_screencopy::encode_buffer_to_png` — the input layout
/// depends on the negotiated VideoFormat: BGRx/BGRA are little-endian
/// BGRA (so swap R↔B), RGBx/RGBA are already RGBA in memory.
fn encode_frame_to_png(
    pixels: &[u8],
    width: u32,
    height: u32,
    stride: u32,
    fmt: libspa::param::video::VideoFormat,
) -> anyhow::Result<Vec<u8>> {
    use libspa::param::video::VideoFormat;
    let mut rgba: Vec<u8> = Vec::with_capacity((width as usize) * (height as usize) * 4);
    for y in 0..(height as usize) {
        let row_start = y * (stride as usize);
        for x in 0..(width as usize) {
            let i = row_start + x * 4;
            let (r, g, b, a) = match fmt {
                VideoFormat::BGRx => (pixels[i + 2], pixels[i + 1], pixels[i], 0xFF),
                VideoFormat::BGRA => (pixels[i + 2], pixels[i + 1], pixels[i], pixels[i + 3]),
                VideoFormat::RGBx => (pixels[i], pixels[i + 1], pixels[i + 2], 0xFF),
                VideoFormat::RGBA => (pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]),
                // Fall through: treat unknown formats as BGRA (the most
                // common compositor output on Linux).
                _ => (pixels[i + 2], pixels[i + 1], pixels[i], pixels[i + 3]),
            };
            rgba.extend_from_slice(&[r, g, b, a]);
        }
    }
    use image::{codecs::png::PngEncoder, ImageBuffer, ImageEncoder, Rgba};
    let img: ImageBuffer<Rgba<u8>, _> =
        ImageBuffer::from_raw(width, height, rgba).ok_or_else(|| {
            anyhow::anyhow!("internal: buffer dims mismatch ({}x{})", width, height)
        })?;
    let mut out: Vec<u8> = Vec::new();
    PngEncoder::new(&mut out).write_image(
        img.as_raw(),
        width,
        height,
        image::ExtendedColorType::Rgba8,
    )?;
    Ok(out)
}

/// Probe whether the portal ScreenCast interface is reachable on the
/// session bus without taking an actual capture (avoids cached consent).
pub fn probe_screencast_portal() -> anyhow::Result<bool> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| anyhow::anyhow!("failed to build tokio runtime: {e}"))?;
    rt.block_on(async {
        let conn = zbus::Connection::session()
            .await
            .map_err(|e| anyhow::anyhow!("session bus unreachable: {e}"))?;
        let proxy = zbus::fdo::DBusProxy::new(&conn)
            .await
            .map_err(|e| anyhow::anyhow!("dbus proxy creation failed: {e}"))?;
        let bus_name: zbus::names::BusName = "org.freedesktop.portal.Desktop"
            .try_into()
            .map_err(|e| anyhow::anyhow!("bus name parse failed: {e}"))?;
        let has_owner = proxy
            .name_has_owner(bus_name)
            .await
            .map_err(|e| anyhow::anyhow!("name_has_owner failed: {e}"))?;
        Ok(has_owner)
    })
}

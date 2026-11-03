//! Compositor-agnostic capture via `xdg-desktop-portal` ScreenCast + PipeWire.
//!
//! Reaches GNOME/Mutter and KDE/KWin, which expose no wlroots capture
//! protocols to ordinary clients. Two primitives share one consent path:
//!
//! - [`ScreencastSession`] owns the portal side: create the session, select
//!   sources, wait for the user's consent (or reuse a persisted restore
//!   token), start the cast, and hand out the PipeWire remote fd plus node id.
//! - [`run_frame_stream`] owns the PipeWire side: connect to the remote,
//!   negotiate a raw BGRx/BGRA/RGBx/RGBA format, and deliver dequeued frames
//!   to a [`FrameSink`] on the calling thread until the sink asks to stop.
//!
//! [`screenshot_window_via_portal`] composes the two for one frame; the
//! Wayland video backend composes them for a paced full-desktop recording.

use std::cell::RefCell;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use ashpd::desktop::screencast::{
    CursorMode, OpenPipeWireRemoteOptions, Screencast, SelectSourcesOptions, SourceType,
    StartCastOptions, Streams,
};
use ashpd::desktop::{CreateSessionOptions, PersistMode, Session};
use ashpd::enumflags2::BitFlags;
use libspa::param::video::VideoFormat;

use super::portal::RestoreToken;

/// How long to wait for the user to answer the portal's consent dialog when no
/// valid restore token skips it.
pub const CONSENT_TIMEOUT: Duration = Duration::from_secs(60);

/// A live portal ScreenCast session. The portal tears the cast down when the
/// owning D-Bus connection goes away, so the session keeps its own runtime
/// alive (one worker thread keeps the zbus connection driven, see #2105).
pub struct ScreencastSession {
    runtime: tokio::runtime::Runtime,
    session: Session<Screencast>,
    streams: Streams,
    fd: OwnedFd,
}

impl ScreencastSession {
    /// Open a ScreenCast session for `sources`. When `restore` is given, the
    /// persisted token is offered so the desktop can skip its consent dialog,
    /// and the token the portal returns is persisted for the next session.
    pub fn open(
        sources: BitFlags<SourceType>,
        restore: Option<RestoreToken>,
        consent_timeout: Duration,
    ) -> anyhow::Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .map_err(|e| anyhow::anyhow!("failed to build tokio runtime for ashpd: {e}"))?;
        let (session, streams, fd) = runtime.block_on(async {
            let connection = super::portal::fresh_session_connection().await?;
            let proxy = Screencast::with_connection(connection).await.map_err(|e| {
                anyhow::anyhow!(
                    "portal ScreenCast proxy unreachable: {e}. Install \
                     xdg-desktop-portal-gnome / xdg-desktop-portal-kde."
                )
            })?;
            let session = proxy
                .create_session(CreateSessionOptions::default())
                .await
                .map_err(|e| anyhow::anyhow!("portal create_session failed: {e}"))?;

            let mut select_opts = SelectSourcesOptions::default()
                .set_sources(sources)
                .set_multiple(false)
                .set_cursor_mode(CursorMode::Embedded)
                .set_persist_mode(PersistMode::ExplicitlyRevoked);
            let saved_token = restore.and_then(RestoreToken::read);
            if let Some(token) = saved_token.as_deref() {
                select_opts = select_opts.set_restore_token(Some(token));
            }
            proxy
                .select_sources(&session, select_opts)
                .await
                .map_err(|e| anyhow::anyhow!("portal select_sources failed: {e}"))?
                .response()
                .map_err(|e| anyhow::anyhow!("portal select_sources response error: {e}"))?;

            // `start` resolves only once the user answers the consent dialog
            // (or the restore token skips it); bound that wait here.
            let start = proxy.start(&session, None, StartCastOptions::default());
            let streams = match tokio::time::timeout(consent_timeout, start).await {
                Ok(request) => request
                    .map_err(|e| anyhow::anyhow!("portal start failed: {e}"))?
                    .response()
                    .map_err(|e| {
                        anyhow::anyhow!(
                            "portal ScreenCast start was refused (consent denied or dialog dismissed): {e}"
                        )
                    })?,
                Err(_) => {
                    let _ = session.close().await;
                    anyhow::bail!(
                        "portal ScreenCast consent was not granted within {}s",
                        consent_timeout.as_secs()
                    );
                }
            };
            if let (Some(restore), Some(token)) = (restore, streams.restore_token()) {
                if saved_token.as_deref() != Some(token) {
                    if let Err(error) = restore.write(token) {
                        tracing::warn!("could not persist portal ScreenCast restore token: {error}");
                    }
                }
            }

            let fd = proxy
                .open_pipe_wire_remote(&session, OpenPipeWireRemoteOptions::default())
                .await
                .map_err(|e| anyhow::anyhow!("portal open_pipe_wire_remote failed: {e}"))?;
            anyhow::Ok((session, streams, fd))
        })?;
        Ok(Self {
            runtime,
            session,
            streams,
            fd,
        })
    }

    /// PipeWire node id of the first stream the portal published.
    pub fn node_id(&self) -> anyhow::Result<u32> {
        self.streams
            .streams()
            .first()
            .map(|stream| stream.pipe_wire_node_id())
            .ok_or_else(|| anyhow::anyhow!("portal advertised no ScreenCast streams"))
    }

    /// An independently owned duplicate of the PipeWire remote fd, suitable for
    /// `pipewire::context::Context::connect_fd`, which consumes its fd.
    pub fn pipewire_fd(&self) -> anyhow::Result<OwnedFd> {
        let dup_fd = unsafe { libc::dup(self.fd.as_raw_fd()) };
        if dup_fd < 0 {
            anyhow::bail!(
                "dup of portal PipeWire fd failed: {}",
                std::io::Error::last_os_error()
            );
        }
        Ok(unsafe { OwnedFd::from_raw_fd(dup_fd) })
    }

    /// End the cast. The compositor stops streaming as soon as the portal
    /// session closes, so callers should stop consuming frames first.
    pub fn close(self) {
        let _ = self.runtime.block_on(self.session.close());
    }
}

/// One dequeued video frame. `pixels` covers exactly `stride * height` bytes
/// in the negotiated `format`; rows may carry padding beyond `width * 4`.
pub struct Frame<'a> {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: VideoFormat,
    pub pixels: &'a [u8],
}

/// Whether [`run_frame_stream`] keeps pumping after a sink callback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameFlow {
    Continue,
    Stop,
}

/// Consumer of a PipeWire capture stream. Both hooks run on the thread that
/// called [`run_frame_stream`]; an error from either ends the stream and is
/// returned to that caller.
pub trait FrameSink {
    fn frame(&mut self, frame: Frame<'_>) -> anyhow::Result<FrameFlow>;

    /// Periodic hook, called every [`run_frame_stream`] `tick_interval`.
    fn tick(&mut self) -> anyhow::Result<FrameFlow> {
        Ok(FrameFlow::Continue)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct StreamOptions {
    /// Give up when the producer delivers no frame at all within this window
    /// (for example when the user closed the consent dialog mid-cast).
    pub first_frame_timeout: Duration,
    pub tick_interval: Duration,
}

impl Default for StreamOptions {
    fn default() -> Self {
        Self {
            first_frame_timeout: Duration::from_secs(10),
            tick_interval: Duration::from_millis(100),
        }
    }
}

struct Pump<S> {
    sink: S,
    negotiated: libspa::param::video::VideoInfoRaw,
    started_at: Instant,
    saw_frame: bool,
    outcome: Option<anyhow::Result<()>>,
}

impl<S: FrameSink> Pump<S> {
    fn finish(&mut self, mainloop: &pipewire::main_loop::MainLoopRc, outcome: anyhow::Result<()>) {
        if self.outcome.is_none() {
            self.outcome = Some(outcome);
        }
        mainloop.quit();
    }

    fn apply(
        &mut self,
        mainloop: &pipewire::main_loop::MainLoopRc,
        result: anyhow::Result<FrameFlow>,
    ) {
        match result {
            Ok(FrameFlow::Continue) => {}
            Ok(FrameFlow::Stop) => self.finish(mainloop, Ok(())),
            Err(error) => self.finish(mainloop, Err(error)),
        }
    }
}

/// Connect to the PipeWire remote behind a portal ScreenCast and feed frames
/// from `node_id` to `sink` until the sink stops, the sink or the stream
/// errors, or no first frame arrives in time. Blocks the calling thread.
///
/// Only raw shared-memory formats are offered (no DMA-BUF modifier
/// negotiation) so the compositor falls back to memfd buffers that
/// `MAP_BUFFERS` can expose as plain byte slices.
pub fn run_frame_stream<S: FrameSink + 'static>(
    fd: OwnedFd,
    node_id: u32,
    options: StreamOptions,
    sink: S,
) -> anyhow::Result<S> {
    use pipewire as pw;
    use pw::{properties::properties, spa};
    use spa::pod::Pod;

    let mainloop = pw::main_loop::MainLoopRc::new(None)
        .map_err(|e| anyhow::anyhow!("pipewire MainLoop::new failed: {e}"))?;
    let context = pw::context::ContextRc::new(&mainloop, None)
        .map_err(|e| anyhow::anyhow!("pipewire Context::new failed: {e}"))?;
    let core = context
        .connect_fd_rc(fd, None)
        .map_err(|e| anyhow::anyhow!("pipewire connect_fd failed: {e}"))?;

    let pump = Rc::new(RefCell::new(Pump {
        sink,
        negotiated: Default::default(),
        started_at: Instant::now(),
        saw_frame: false,
        outcome: None,
    }));

    let stream = pw::stream::StreamBox::new(
        &core,
        "cua-driver-capture",
        properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )
    .map_err(|e| anyhow::anyhow!("pipewire Stream::new failed: {e}"))?;

    let listener = stream
        .add_local_listener::<()>()
        .state_changed({
            let pump = pump.clone();
            let mainloop = mainloop.clone();
            move |_stream, _user, _old, new| {
                if let pw::stream::StreamState::Error(error) = new {
                    pump.borrow_mut().finish(
                        &mainloop,
                        Err(anyhow::anyhow!("portal ScreenCast stream failed: {error}")),
                    );
                }
            }
        })
        .param_changed({
            let pump = pump.clone();
            move |_stream, _user, id, param| {
                let Some(param) = param else { return };
                if id != spa::param::ParamType::Format.as_raw() {
                    return;
                }
                let Ok((media_type, media_subtype)) = spa::param::format_utils::parse_format(param)
                else {
                    return;
                };
                if media_type != spa::param::format::MediaType::Video
                    || media_subtype != spa::param::format::MediaSubtype::Raw
                {
                    return;
                }
                let mut info = spa::param::video::VideoInfoRaw::default();
                if info.parse(param).is_ok() {
                    pump.borrow_mut().negotiated = info;
                }
            }
        })
        .process({
            let pump = pump.clone();
            let mainloop = mainloop.clone();
            move |stream, _user| {
                // Keep only the newest queued buffer; dropping a buffer
                // requeues it to the producer.
                let Some(mut buffer) = stream.dequeue_buffer() else {
                    return;
                };
                while let Some(newer) = stream.dequeue_buffer() {
                    buffer = newer;
                }
                let mut pump = pump.borrow_mut();
                if pump.outcome.is_some() {
                    return;
                }
                let info = pump.negotiated;
                let size = info.size();
                let (width, height) = (size.width, size.height);
                if width == 0 || height == 0 {
                    return;
                }
                let datas = buffer.datas_mut();
                let Some(data) = datas.first_mut() else {
                    return;
                };
                let chunk = data.chunk();
                let (offset, chunk_size, chunk_stride) =
                    (chunk.offset() as usize, chunk.size(), chunk.stride());
                // Producers may leave stride unset; derive it for the packed
                // 32-bit formats negotiated below.
                let stride = if chunk_stride > 0 {
                    chunk_stride as u32
                } else if chunk_size > 0 {
                    chunk_size / height
                } else {
                    width * 4
                };
                let payload_len = stride as usize * height as usize;
                let Some(payload) = data
                    .data()
                    .and_then(|bytes| bytes.get(offset..offset + payload_len))
                else {
                    return;
                };
                pump.saw_frame = true;
                let result = pump.sink.frame(Frame {
                    width,
                    height,
                    stride,
                    format: info.format(),
                    pixels: payload,
                });
                pump.apply(&mainloop, result);
            }
        })
        .register()
        .map_err(|e| anyhow::anyhow!("pipewire stream listener register failed: {e}"))?;

    // Compositors typically pick BGRx/BGRA on Linux desktops; the alternates
    // let negotiation succeed on more sources.
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
            VideoFormat::BGRx,
            VideoFormat::BGRx,
            VideoFormat::BGRA,
            VideoFormat::RGBx,
            VideoFormat::RGBA,
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            pw::spa::utils::Rectangle {
                width: 1920,
                height: 1080
            },
            pw::spa::utils::Rectangle {
                width: 1,
                height: 1
            },
            pw::spa::utils::Rectangle {
                width: 7680,
                height: 4320
            }
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            pw::spa::utils::Fraction { num: 30, denom: 1 },
            pw::spa::utils::Fraction { num: 0, denom: 1 },
            pw::spa::utils::Fraction {
                num: 1000,
                denom: 1
            }
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

    let ticker = mainloop.loop_().add_timer({
        let pump = pump.clone();
        let mainloop = mainloop.clone();
        move |_expirations| {
            let mut pump = pump.borrow_mut();
            if pump.outcome.is_some() {
                return;
            }
            if !pump.saw_frame && pump.started_at.elapsed() >= options.first_frame_timeout {
                pump.finish(
                    &mainloop,
                    Err(anyhow::anyhow!(
                        "portal ScreenCast delivered no frame within {}s",
                        options.first_frame_timeout.as_secs()
                    )),
                );
                return;
            }
            let result = pump.sink.tick();
            pump.apply(&mainloop, result);
        }
    });
    ticker
        .update_timer(Some(options.tick_interval), Some(options.tick_interval))
        .into_result()
        .map_err(|e| anyhow::anyhow!("pipewire timer source failed: {e:?}"))?;

    mainloop.run();

    // Tear the stream down before handing the sink back so the producer
    // cannot touch buffers the sink may still reference.
    drop(ticker);
    drop(listener);
    drop(stream);
    drop(core);
    drop(context);

    let pump = Rc::try_unwrap(pump)
        .map_err(|_| anyhow::anyhow!("pipewire callbacks outlived the capture loop"))?
        .into_inner();
    match pump.outcome {
        Some(Ok(())) | None => Ok(pump.sink),
        Some(Err(error)) => Err(error),
    }
}

/// Channel-swap a PipeWire video frame into RGBA8888 and PNG-encode it.
///
/// Mirrors `ext_screencopy::encode_buffer_to_png`: BGRx/BGRA are
/// little-endian BGRA (so swap R and B), RGBx/RGBA are already RGBA in memory.
pub fn encode_frame_to_png(frame: &Frame<'_>) -> anyhow::Result<Vec<u8>> {
    let (width, height, stride) = (frame.width, frame.height, frame.stride as usize);
    let pixels = frame.pixels;
    let mut rgba: Vec<u8> = Vec::with_capacity((width as usize) * (height as usize) * 4);
    for y in 0..(height as usize) {
        let row_start = y * stride;
        for x in 0..(width as usize) {
            let i = row_start + x * 4;
            let (r, g, b, a) = match frame.format {
                VideoFormat::BGRx => (pixels[i + 2], pixels[i + 1], pixels[i], 0xFF),
                VideoFormat::BGRA => (pixels[i + 2], pixels[i + 1], pixels[i], pixels[i + 3]),
                VideoFormat::RGBx => (pixels[i], pixels[i + 1], pixels[i + 2], 0xFF),
                VideoFormat::RGBA => (pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]),
                // Treat unknown formats as BGRA, the most common compositor
                // output on Linux.
                _ => (pixels[i + 2], pixels[i + 1], pixels[i], pixels[i + 3]),
            };
            rgba.extend_from_slice(&[r, g, b, a]);
        }
    }
    use image::{codecs::png::PngEncoder, ImageBuffer, ImageEncoder, Rgba};
    let img: ImageBuffer<Rgba<u8>, _> = ImageBuffer::from_raw(width, height, rgba)
        .ok_or_else(|| anyhow::anyhow!("internal: buffer dims mismatch ({}x{})", width, height))?;
    let mut out: Vec<u8> = Vec::new();
    PngEncoder::new(&mut out).write_image(
        img.as_raw(),
        width,
        height,
        image::ExtendedColorType::Rgba8,
    )?;
    Ok(out)
}

/// Consent is expensive (system dialog) and reuse is cheap, so the window
/// screenshot session lives for the rest of the process.
static WINDOW_SESSION: OnceLock<ScreencastSession> = OnceLock::new();

struct FirstFramePng(Option<anyhow::Result<Vec<u8>>>);

impl FrameSink for FirstFramePng {
    fn frame(&mut self, frame: Frame<'_>) -> anyhow::Result<FrameFlow> {
        self.0 = Some(encode_frame_to_png(&frame));
        Ok(FrameFlow::Stop)
    }
}

/// Capture the user-selected window or monitor via `xdg-desktop-portal`
/// ScreenCast. On first invocation per process the portal dialog asks the
/// user to pick a source; subsequent calls reuse the same node
/// transparently. Returns PNG bytes of the latest frame on the stream.
pub fn screenshot_window_via_portal() -> anyhow::Result<Vec<u8>> {
    if WINDOW_SESSION.get().is_none() {
        let session = ScreencastSession::open(
            BitFlags::<SourceType>::from(SourceType::Window) | SourceType::Monitor,
            None,
            CONSENT_TIMEOUT,
        )?;
        let _ = WINDOW_SESSION.set(session);
    }
    let session = WINDOW_SESSION
        .get()
        .ok_or_else(|| anyhow::anyhow!("portal ScreenCast session unavailable"))?;
    let sink = run_frame_stream(
        session.pipewire_fd()?,
        session.node_id()?,
        StreamOptions::default(),
        FirstFramePng(None),
    )?;
    sink.0
        .ok_or_else(|| anyhow::anyhow!("portal ScreenCast: no frame arrived within timeout"))?
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

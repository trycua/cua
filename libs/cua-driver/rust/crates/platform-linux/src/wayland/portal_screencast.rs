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

/// The packed 32-bit layouts [`run_frame_stream`] offers during format
/// negotiation; every [`Frame`] carries one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawFormat {
    BGRx,
    BGRA,
    RGBx,
    RGBA,
}

impl RawFormat {
    pub fn from_video_format(format: VideoFormat) -> Option<Self> {
        match format {
            VideoFormat::BGRx => Some(Self::BGRx),
            VideoFormat::BGRA => Some(Self::BGRA),
            VideoFormat::RGBx => Some(Self::RGBx),
            VideoFormat::RGBA => Some(Self::RGBA),
            _ => None,
        }
    }

    /// One pixel of this layout as RGBA8888. `x`-suffixed layouts carry no
    /// alpha, so their pixels read as opaque.
    fn rgba(self, px: &[u8]) -> [u8; 4] {
        match self {
            Self::BGRx => [px[2], px[1], px[0], 0xFF],
            Self::BGRA => [px[2], px[1], px[0], px[3]],
            Self::RGBx => [px[0], px[1], px[2], 0xFF],
            Self::RGBA => [px[0], px[1], px[2], px[3]],
        }
    }
}

const BYTES_PER_PIXEL: usize = 4;

/// One dequeued video frame whose geometry has been checked against its
/// payload, so consumers can index rows without bounds failures.
pub struct Frame<'a> {
    width: u32,
    height: u32,
    stride: usize,
    format: RawFormat,
    pixels: &'a [u8],
}

impl<'a> Frame<'a> {
    /// Rejects frames a producer described inconsistently: an unsupported
    /// format, empty dimensions, rows narrower than `width` pixels, or a
    /// payload shorter than `stride * height`.
    pub fn new(
        width: u32,
        height: u32,
        stride: u32,
        format: VideoFormat,
        pixels: &'a [u8],
    ) -> anyhow::Result<Self> {
        let format = RawFormat::from_video_format(format).ok_or_else(|| {
            anyhow::anyhow!("portal ScreenCast negotiated unsupported pixel format {format:?}")
        })?;
        if width == 0 || height == 0 {
            anyhow::bail!("portal ScreenCast produced an empty {width}x{height} frame");
        }
        let stride = stride as usize;
        let row = width as usize * BYTES_PER_PIXEL;
        if stride < row {
            anyhow::bail!(
                "portal ScreenCast frame stride {stride} is narrower than its {width}-pixel rows \
                 ({row} bytes)"
            );
        }
        let needed = stride * height as usize;
        if pixels.len() < needed {
            anyhow::bail!(
                "portal ScreenCast buffer holds {} bytes but {width}x{height} rows of {stride} \
                 bytes need {needed}",
                pixels.len()
            );
        }
        Ok(Self {
            width,
            height,
            stride,
            format,
            pixels,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn format(&self) -> RawFormat {
        self.format
    }

    /// Copy the visible pixels into `out`, dropping any row padding so the
    /// result is `width * 4` bytes per row.
    pub fn pack_rows(&self, out: &mut Vec<u8>) {
        let row = self.width as usize * BYTES_PER_PIXEL;
        out.clear();
        out.reserve(row * self.height as usize);
        for line in self.pixels.chunks(self.stride).take(self.height as usize) {
            out.extend_from_slice(&line[..row]);
        }
    }
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
    /// Most recent error the remote core reported against the connection
    /// itself; libpipewire only turns it into an `Unconnected` transition,
    /// dropping the message.
    core_error: Option<String>,
    outcome: Option<anyhow::Result<()>>,
}

/// Why a `pw_stream` state transition ends the capture, if it does.
///
/// libpipewire moves a stream to `Unconnected` without any `Error` when the
/// server destroys its node (`proxy_removed`) or the core connection drops
/// (`on_core_error` with `-EPIPE`), and a server-sent node error arrives as
/// an `Error` transition. `Paused` is not terminal: the compositor pauses a
/// source it may resume, and a damage-free desktop stays `Streaming`.
fn stream_loss(new: &pipewire::stream::StreamState, core_error: Option<&str>) -> Option<String> {
    use pipewire::stream::StreamState;
    match new {
        StreamState::Error(error) => Some(format!("portal ScreenCast stream failed: {error}")),
        StreamState::Unconnected => Some(match core_error {
            Some(error) => format!("portal ScreenCast stream disconnected: {error}"),
            None => "portal ScreenCast stream disconnected: the compositor removed the source \
                 (session closed or sharing revoked)"
                .to_owned(),
        }),
        StreamState::Connecting | StreamState::Paused | StreamState::Streaming => None,
    }
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
/// from `node_id` to `sink` until the sink stops, the sink errors, the
/// stream errors or disconnects, a producer delivers an inconsistent frame,
/// or no first frame arrives in time. Blocks the calling thread.
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
        core_error: None,
        outcome: None,
    }));

    let core_listener = core
        .add_listener_local()
        .error({
            let pump = pump.clone();
            move |id, _seq, res, message| {
                if id == pw::core::PW_ID_CORE {
                    pump.borrow_mut().core_error = Some(format!("{message} ({res})"));
                }
            }
        })
        .register();

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
                let mut pump = pump.borrow_mut();
                if let Some(reason) = stream_loss(&new, pump.core_error.as_deref()) {
                    pump.finish(&mainloop, Err(anyhow::anyhow!(reason)));
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
                let (offset, chunk_size, chunk_stride, chunk_flags) = (
                    chunk.offset() as usize,
                    chunk.size(),
                    chunk.stride(),
                    chunk.flags(),
                );
                // A producer marks buffers it could not fill; they carry no
                // frame and go straight back to it.
                if chunk_flags.contains(spa::buffer::ChunkFlags::CORRUPTED) {
                    return;
                }
                // Producers may leave stride unset; derive it for the packed
                // 32-bit formats negotiated below.
                let stride = if chunk_stride > 0 {
                    chunk_stride as u32
                } else if chunk_size > 0 {
                    chunk_size / height
                } else {
                    width * 4
                };
                let data_type = data.type_();
                let frame = data
                    .data()
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "portal ScreenCast delivered a {data_type:?} buffer that is not \
                             memory-mapped; only shared-memory frames are supported"
                        )
                    })
                    .and_then(|bytes| {
                        let payload = bytes.get(offset..).unwrap_or(&[]);
                        Frame::new(width, height, stride, info.format(), payload)
                    });
                let result = match frame {
                    Ok(frame) => {
                        pump.saw_frame = true;
                        pump.sink.frame(frame)
                    }
                    Err(error) => Err(error),
                };
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

    // `DONT_RECONNECT` binds the stream to the portal's node: when the
    // compositor removes that node, the session manager destroys this
    // stream (surfacing as `Unconnected`) instead of parking it `Paused`
    // while it waits to relink, possibly to an unrelated video source.
    stream
        .connect(
            spa::utils::Direction::Input,
            Some(node_id),
            pw::stream::StreamFlags::AUTOCONNECT
                | pw::stream::StreamFlags::MAP_BUFFERS
                | pw::stream::StreamFlags::DONT_RECONNECT,
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
    drop(core_listener);
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
    let (width, height) = (frame.width, frame.height);
    let mut packed = Vec::new();
    frame.pack_rows(&mut packed);
    let rgba: Vec<u8> = packed
        .chunks_exact(BYTES_PER_PIXEL)
        .flat_map(|px| frame.format.rgba(px))
        .collect();
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

#[cfg(test)]
mod tests {
    use super::*;
    use pipewire::stream::StreamState;

    #[test]
    fn frame_rejects_rows_narrower_than_its_width() {
        let pixels = [0u8; 8];
        let error = Frame::new(2, 1, 4, VideoFormat::BGRx, &pixels)
            .err()
            .expect("stride 4 cannot hold two 4-byte pixels");
        assert!(error.to_string().contains("stride 4"), "{error}");
    }

    #[test]
    fn frame_rejects_payloads_shorter_than_its_geometry() {
        let pixels = [0u8; 15];
        let error = Frame::new(2, 2, 8, VideoFormat::BGRx, &pixels)
            .err()
            .expect("15 bytes cannot hold 2 rows of 8");
        let message = error.to_string();
        assert!(message.contains("holds 15 bytes"), "{message}");
        assert!(message.contains("need 16"), "{message}");
    }

    #[test]
    fn frame_rejects_formats_outside_the_negotiated_set() {
        let pixels = [0u8; 4];
        let error = Frame::new(1, 1, 4, VideoFormat::NV12, &pixels)
            .err()
            .expect("NV12 is never offered to the producer");
        assert!(error.to_string().contains("NV12"), "{error}");
        assert!(Frame::new(0, 1, 4, VideoFormat::BGRx, &pixels).is_err());
        assert!(Frame::new(1, 0, 4, VideoFormat::BGRx, &pixels).is_err());
    }

    #[test]
    fn packing_drops_row_padding_and_trailing_bytes() {
        // Two 2-pixel rows with 4 bytes of padding each, plus a trailing byte
        // the producer left in the buffer.
        let pixels: Vec<u8> = (1u8..=25).collect();
        let frame = Frame::new(2, 2, 12, VideoFormat::BGRA, &pixels).unwrap();
        let mut packed = Vec::new();
        frame.pack_rows(&mut packed);
        assert_eq!(
            packed,
            [1, 2, 3, 4, 5, 6, 7, 8, 13, 14, 15, 16, 17, 18, 19, 20]
        );
    }

    #[test]
    fn png_encoding_swaps_bgr_channels_and_reads_x_layouts_as_opaque() {
        use image::GenericImageView;
        let pixels = [0x10u8, 0x20, 0x30, 0x00];
        let bgrx = Frame::new(1, 1, 4, VideoFormat::BGRx, &pixels).unwrap();
        let png = encode_frame_to_png(&bgrx).unwrap();
        let image = image::load_from_memory(&png).unwrap();
        assert_eq!(image.get_pixel(0, 0).0, [0x30, 0x20, 0x10, 0xFF]);

        let rgba = Frame::new(1, 1, 4, VideoFormat::RGBA, &pixels).unwrap();
        let png = encode_frame_to_png(&rgba).unwrap();
        let image = image::load_from_memory(&png).unwrap();
        assert_eq!(image.get_pixel(0, 0).0, [0x10, 0x20, 0x30, 0x00]);
    }

    #[test]
    fn stream_errors_and_disconnects_end_the_capture() {
        let error = stream_loss(&StreamState::Error("target not found".into()), None)
            .expect("server-sent node errors are terminal");
        assert!(error.contains("target not found"), "{error}");

        let removed = stream_loss(&StreamState::Unconnected, None)
            .expect("a stream that drops to Unconnected never reconnects");
        assert!(removed.contains("removed the source"), "{removed}");

        let lost = stream_loss(&StreamState::Unconnected, Some("connection error (-32)"))
            .expect("core errors explain the disconnect");
        assert!(lost.contains("connection error (-32)"), "{lost}");
    }

    #[test]
    fn pauses_and_connection_progress_keep_the_capture_alive() {
        for state in [
            StreamState::Connecting,
            StreamState::Paused,
            StreamState::Streaming,
        ] {
            assert_eq!(stream_loss(&state, Some("stale core error")), None);
        }
    }
}

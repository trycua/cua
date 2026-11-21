// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

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
//! The Wayland video backend composes them for a paced full-desktop recording.

use std::cell::RefCell;
use std::ffi::{c_char, c_int, c_void, CStr};
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};
use std::ptr;
use std::time::{Duration, Instant};

use ashpd::desktop::screencast::{
    CursorMode, OpenPipeWireRemoteOptions, Screencast, SelectSourcesOptions, SourceType,
    StartCastOptions, Streams,
};
use ashpd::desktop::{CreateSessionOptions, PersistMode, Session};
use ashpd::enumflags2::BitFlags;
use libspa::buffer::{ChunkFlags, Data};
use libspa::param::format::{FormatProperties, MediaSubtype, MediaType};
use libspa::param::video::{VideoFormat, VideoInfoRaw};
use libspa::param::{format_utils, ParamType};
use libspa::pod::serialize::PodSerializer;
use libspa::pod::Pod;
use libspa::spa_interface_call_method;
use libspa::sys as spa_sys;
use libspa::utils::{Fraction, Rectangle, SpaTypes};
use pipewire_sys as pw_sys;

use super::libpipewire::{libpipewire, PipeWire};
use super::portal::RestoreToken;

/// How long to wait for the user to answer the portal's consent dialog when no
/// valid restore token skips it.
pub const CONSENT_TIMEOUT: Duration = Duration::from_secs(60);

/// Bound on each stretch of portal calls that needs no user answer, so a
/// portal that never replies fails the start instead of parking its caller.
pub const PORTAL_CALL_TIMEOUT: Duration = Duration::from_secs(10);

fn portal_unresponsive(step: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "xdg-desktop-portal did not answer {step} within {}s",
        PORTAL_CALL_TIMEOUT.as_secs()
    )
}

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
            let saved_token = restore.and_then(RestoreToken::read);
            let setup = tokio::time::timeout(PORTAL_CALL_TIMEOUT, async {
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
                if let Some(token) = saved_token.as_deref() {
                    select_opts = select_opts.set_restore_token(Some(token));
                }
                proxy
                    .select_sources(&session, select_opts)
                    .await
                    .map_err(|e| anyhow::anyhow!("portal select_sources failed: {e}"))?
                    .response()
                    .map_err(|e| anyhow::anyhow!("portal select_sources response error: {e}"))?;
                anyhow::Ok((proxy, session))
            });
            let (proxy, session) = setup
                .await
                .map_err(|_| portal_unresponsive("the ScreenCast session setup"))??;

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

            let remote = proxy.open_pipe_wire_remote(&session, OpenPipeWireRemoteOptions::default());
            let fd = match tokio::time::timeout(PORTAL_CALL_TIMEOUT, remote).await {
                Ok(fd) => fd.map_err(|e| anyhow::anyhow!("portal open_pipe_wire_remote failed: {e}"))?,
                Err(_) => {
                    let _ = session.close().await;
                    return Err(portal_unresponsive("open_pipe_wire_remote"));
                }
            };
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
    /// `pw_context_connect_fd`, which consumes its fd.
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
    Bgrx,
    Bgra,
    Rgbx,
    Rgba,
}

impl RawFormat {
    pub fn from_video_format(format: VideoFormat) -> Option<Self> {
        match format {
            VideoFormat::BGRx => Some(Self::Bgrx),
            VideoFormat::BGRA => Some(Self::Bgra),
            VideoFormat::RGBx => Some(Self::Rgbx),
            VideoFormat::RGBA => Some(Self::Rgba),
            _ => None,
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

/// A `pw_stream` state, as its `state_changed` event reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum StreamState {
    Error(String),
    Unconnected,
    Connecting,
    Paused,
    Streaming,
}

impl StreamState {
    /// # Safety
    /// `error` is null or points to a NUL-terminated string.
    unsafe fn from_raw(state: pw_sys::pw_stream_state, error: *const c_char) -> Self {
        match state {
            pw_sys::pw_stream_state_PW_STREAM_STATE_UNCONNECTED => Self::Unconnected,
            pw_sys::pw_stream_state_PW_STREAM_STATE_CONNECTING => Self::Connecting,
            pw_sys::pw_stream_state_PW_STREAM_STATE_PAUSED => Self::Paused,
            pw_sys::pw_stream_state_PW_STREAM_STATE_STREAMING => Self::Streaming,
            _ => Self::Error(lossy(error)),
        }
    }
}

/// # Safety
/// `text` is null or points to a NUL-terminated string.
unsafe fn lossy(text: *const c_char) -> String {
    if text.is_null() {
        String::new()
    } else {
        CStr::from_ptr(text).to_string_lossy().into_owned()
    }
}

/// Capture state shared by every libpipewire callback of one
/// [`run_frame_stream`]. All of them run on the thread inside
/// `pw_main_loop_run`.
struct Pump<S> {
    pw: &'static PipeWire,
    mainloop: *mut pw_sys::pw_main_loop,
    stream: *mut pw_sys::pw_stream,
    node_id: u32,
    options: StreamOptions,
    sink: S,
    negotiated: VideoInfoRaw,
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
fn stream_loss(new: &StreamState, core_error: Option<&str>) -> Option<String> {
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

/// Why removing registry global `removed` ends a capture of `node_id`, if it
/// does: only the portal's own node going away means the source is gone.
fn source_removed(removed: u32, node_id: u32) -> Option<&'static str> {
    (removed == node_id).then_some(
        "portal ScreenCast stream disconnected: the compositor removed the source \
         (session closed or sharing revoked)",
    )
}

impl<S: FrameSink> Pump<S> {
    /// Records the outcome; the callback quits the loop once its borrow ends
    /// (see [`settle`]).
    fn finish(&mut self, outcome: anyhow::Result<()>) {
        if self.outcome.is_none() {
            self.outcome = Some(outcome);
        }
    }

    fn apply(&mut self, result: anyhow::Result<FrameFlow>) {
        match result {
            Ok(FrameFlow::Continue) => {}
            Ok(FrameFlow::Stop) => self.finish(Ok(())),
            Err(error) => self.finish(Err(error)),
        }
    }

    fn negotiate(&mut self, param: &Pod) {
        let Ok((media_type, media_subtype)) = format_utils::parse_format(param) else {
            return;
        };
        if media_type != MediaType::Video || media_subtype != MediaSubtype::Raw {
            return;
        }
        let mut info = VideoInfoRaw::default();
        if info.parse(param).is_ok() {
            self.negotiated = info;
        }
    }

    /// # Safety
    /// `buffer` belongs to a `pw_buffer` just dequeued from this pump's stream.
    unsafe fn deliver(&mut self, buffer: *mut spa_sys::spa_buffer) {
        if self.outcome.is_some() {
            return;
        }
        let info = self.negotiated;
        let size = info.size();
        let (width, height) = (size.width, size.height);
        if width == 0 || height == 0 {
            return;
        }
        if buffer.is_null() || (*buffer).n_datas == 0 || (*buffer).datas.is_null() {
            return;
        }
        // `Data` is a transparent wrapper around `spa_data`.
        let data = &mut *(*buffer).datas.cast::<Data>();
        let chunk = data.chunk();
        let (offset, chunk_size, chunk_stride, chunk_flags) = (
            chunk.offset() as usize,
            chunk.size(),
            chunk.stride(),
            chunk.flags(),
        );
        // A producer marks buffers it could not fill; they carry no
        // frame and go straight back to it.
        if chunk_flags.contains(ChunkFlags::CORRUPTED) {
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
                self.saw_frame = true;
                self.sink.frame(frame)
            }
            Err(error) => Err(error),
        };
        self.apply(result);
    }

    fn tick(&mut self) {
        if self.outcome.is_some() {
            return;
        }
        if !self.saw_frame && self.started_at.elapsed() >= self.options.first_frame_timeout {
            let timeout = self.options.first_frame_timeout.as_secs();
            self.finish(Err(anyhow::anyhow!(
                "portal ScreenCast delivered no frame within {timeout}s"
            )));
            return;
        }
        let result = self.sink.tick();
        self.apply(result);
    }
}

/// # Safety
/// `data` is the pump pointer a listener was registered with.
unsafe fn pump<'a, S>(data: *mut c_void) -> &'a RefCell<Pump<S>> {
    &*data.cast::<RefCell<Pump<S>>>()
}

/// Quits the loop once the capture has an outcome. Callers hold no borrow:
/// quitting from the loop thread can dispatch queued events, which re-enter
/// the callbacks.
///
/// # Safety
/// The pump's main loop is still alive.
unsafe fn settle<S>(pump: &RefCell<Pump<S>>) {
    let (pw, mainloop, done) = {
        let pump = pump.borrow();
        (pump.pw, pump.mainloop, pump.outcome.is_some())
    };
    if done {
        (pw.pw_main_loop_quit)(mainloop);
    }
}

unsafe extern "C" fn on_core_error<S>(
    data: *mut c_void,
    id: u32,
    _seq: c_int,
    res: c_int,
    message: *const c_char,
) {
    if id == pw_sys::PW_ID_CORE {
        pump::<S>(data).borrow_mut().core_error = Some(format!("{} ({res})", lossy(message)));
    }
}

unsafe extern "C" fn on_global_remove<S: FrameSink>(data: *mut c_void, id: u32) {
    let pump = pump::<S>(data);
    {
        let mut pump = pump.borrow_mut();
        if let Some(reason) = source_removed(id, pump.node_id) {
            pump.finish(Err(anyhow::anyhow!(reason)));
        }
    }
    settle(pump);
}

unsafe extern "C" fn on_state_changed<S: FrameSink>(
    data: *mut c_void,
    _old: pw_sys::pw_stream_state,
    new: pw_sys::pw_stream_state,
    error: *const c_char,
) {
    let new = StreamState::from_raw(new, error);
    let pump = pump::<S>(data);
    {
        let mut pump = pump.borrow_mut();
        if let Some(reason) = stream_loss(&new, pump.core_error.as_deref()) {
            pump.finish(Err(anyhow::anyhow!(reason)));
        }
    }
    settle(pump);
}

unsafe extern "C" fn on_param_changed<S: FrameSink>(
    data: *mut c_void,
    id: u32,
    param: *const spa_sys::spa_pod,
) {
    if !param.is_null() && id == ParamType::Format.as_raw() {
        pump::<S>(data).borrow_mut().negotiate(Pod::from_raw(param));
    }
}

unsafe extern "C" fn on_process<S: FrameSink>(data: *mut c_void) {
    let pump = pump::<S>(data);
    let (pw, stream) = {
        let pump = pump.borrow();
        (pump.pw, pump.stream)
    };
    // Keep only the newest queued buffer and hand the older ones back.
    let mut buffer = (pw.pw_stream_dequeue_buffer)(stream);
    if buffer.is_null() {
        return;
    }
    loop {
        let newer = (pw.pw_stream_dequeue_buffer)(stream);
        if newer.is_null() {
            break;
        }
        (pw.pw_stream_queue_buffer)(stream, buffer);
        buffer = newer;
    }
    pump.borrow_mut().deliver((*buffer).buffer);
    (pw.pw_stream_queue_buffer)(stream, buffer);
    settle(pump);
}

unsafe extern "C" fn on_tick<S: FrameSink>(data: *mut c_void, _expirations: u64) {
    let pump = pump::<S>(data);
    pump.borrow_mut().tick();
    settle(pump);
}

/// Runs one teardown step on drop, so an early return releases everything
/// created before it in reverse order.
struct Release<F: FnMut()>(F);

impl<F: FnMut()> Drop for Release<F> {
    fn drop(&mut self) {
        (self.0)()
    }
}

/// A listener registered with libpipewire. The hook and the events table it
/// points at stay at a fixed address until the hook is unlinked on drop.
struct Listener<E> {
    hook: Box<spa_sys::spa_hook>,
    events: Box<E>,
}

impl<E> Listener<E> {
    /// # Safety
    /// `add` registers the hook and events table it is given.
    unsafe fn add(events: E, add: impl FnOnce(*mut spa_sys::spa_hook, *const E)) -> Self {
        let mut listener = Self {
            hook: Box::new(std::mem::zeroed()),
            events: Box::new(events),
        };
        add(&mut *listener.hook, &*listener.events);
        listener
    }
}

impl<E> Drop for Listener<E> {
    fn drop(&mut self) {
        libspa::utils::hook::remove(*self.hook);
    }
}

fn timespec(duration: Duration) -> spa_sys::timespec {
    // SAFETY: timespec is plain data; zeroing also clears any padding fields.
    let mut timespec: spa_sys::timespec = unsafe { std::mem::zeroed() };
    timespec.tv_sec = duration.as_secs() as _;
    timespec.tv_nsec = duration.subsec_nanos() as _;
    timespec
}

fn creation_failed(what: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "pipewire {what} failed: {}",
        std::io::Error::last_os_error()
    )
}

/// Connect to the PipeWire remote behind a portal ScreenCast and feed frames
/// from `node_id` to `sink` until the sink stops, the sink errors, the
/// stream errors or disconnects, a producer delivers an inconsistent frame,
/// or no first frame arrives in time. Blocks the calling thread.
///
/// Only raw shared-memory formats are offered (no DMA-BUF modifier
/// negotiation) so the compositor falls back to memfd buffers that
/// `MAP_BUFFERS` can expose as plain byte slices.
pub fn run_frame_stream<S: FrameSink>(
    fd: OwnedFd,
    node_id: u32,
    options: StreamOptions,
    sink: S,
) -> anyhow::Result<S> {
    let pw = libpipewire()?;

    // Compositors typically pick BGRx/BGRA on Linux desktops; the alternates
    // let negotiation succeed on more sources.
    let obj = libspa::pod::object!(
        SpaTypes::ObjectParamFormat,
        ParamType::EnumFormat,
        libspa::pod::property!(FormatProperties::MediaType, Id, MediaType::Video),
        libspa::pod::property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
        libspa::pod::property!(
            FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            VideoFormat::BGRx,
            VideoFormat::BGRx,
            VideoFormat::BGRA,
            VideoFormat::RGBx,
            VideoFormat::RGBA,
        ),
        libspa::pod::property!(
            FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            Rectangle {
                width: 1920,
                height: 1080
            },
            Rectangle {
                width: 1,
                height: 1
            },
            Rectangle {
                width: 7680,
                height: 4320
            }
        ),
        libspa::pod::property!(
            FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            Fraction { num: 30, denom: 1 },
            Fraction { num: 0, denom: 1 },
            Fraction {
                num: 1000,
                denom: 1
            }
        ),
    );
    let values: Vec<u8> = PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &libspa::pod::Value::Object(obj),
    )
    .map_err(|e| anyhow::anyhow!("EnumFormat pod serialization failed: {e}"))?
    .0
    .into_inner();
    let enum_format = Pod::from_bytes(&values)
        .ok_or_else(|| anyhow::anyhow!("EnumFormat pod parse-back failed"))?;

    let pump = Box::new(RefCell::new(Pump {
        pw,
        mainloop: ptr::null_mut(),
        stream: ptr::null_mut(),
        node_id,
        options,
        sink,
        negotiated: Default::default(),
        started_at: Instant::now(),
        saw_frame: false,
        core_error: None,
        outcome: None,
    }));
    let data: *mut c_void = ptr::from_ref(&*pump).cast_mut().cast();

    // SAFETY: every object below is created, used and destroyed on this
    // thread; each `Release` and `Listener` drops before what it refers to,
    // and all of them drop before `pump`, which every callback points at.
    unsafe {
        let mainloop = (pw.pw_main_loop_new)(ptr::null());
        if mainloop.is_null() {
            return Err(creation_failed("MainLoop::new"));
        }
        let _mainloop = Release(move || (pw.pw_main_loop_destroy)(mainloop));
        pump.borrow_mut().mainloop = mainloop;
        let loop_ = (pw.pw_main_loop_get_loop)(mainloop);

        let context = (pw.pw_context_new)(loop_, ptr::null_mut(), 0);
        if context.is_null() {
            return Err(creation_failed("Context::new"));
        }
        let _context = Release(move || (pw.pw_context_destroy)(context));

        let core = (pw.pw_context_connect_fd)(context, fd.into_raw_fd(), ptr::null_mut(), 0);
        if core.is_null() {
            return Err(creation_failed("connect_fd"));
        }
        let _core = Release(move || {
            (pw.pw_core_disconnect)(core);
        });

        let mut core_events: pw_sys::pw_core_events = std::mem::zeroed();
        core_events.version = pw_sys::PW_VERSION_CORE_EVENTS;
        core_events.error = Some(on_core_error::<S>);
        let _core_listener = Listener::add(core_events, |hook, events| {
            spa_interface_call_method!(
                core,
                pw_sys::pw_core_methods,
                add_listener,
                hook,
                events,
                data
            );
        });

        // A producer that stops casting removes its node, but libpipewire parks a
        // stream linked to it in `Paused` until the session manager gets round to
        // destroying it, which can take long enough to record a frozen tail.
        // Watching the registry ends the capture as soon as the source is gone.
        let registry: *mut pw_sys::pw_registry = spa_interface_call_method!(
            core,
            pw_sys::pw_core_methods,
            get_registry,
            pw_sys::PW_VERSION_REGISTRY,
            0
        );
        if registry.is_null() {
            return Err(creation_failed("get_registry"));
        }
        let _registry = Release(move || (pw.pw_proxy_destroy)(registry.cast()));
        let mut registry_events: pw_sys::pw_registry_events = std::mem::zeroed();
        registry_events.version = pw_sys::PW_VERSION_REGISTRY_EVENTS;
        registry_events.global_remove = Some(on_global_remove::<S>);
        let _registry_listener = Listener::add(registry_events, |hook, events| {
            spa_interface_call_method!(
                registry,
                pw_sys::pw_registry_methods,
                add_listener,
                hook,
                events,
                data
            );
        });

        let item = |key: &'static [u8], value: &'static CStr| spa_sys::spa_dict_item {
            key: key.as_ptr().cast(),
            value: value.as_ptr(),
        };
        let items = [
            item(pw_sys::PW_KEY_MEDIA_TYPE, c"Video"),
            item(pw_sys::PW_KEY_MEDIA_CATEGORY, c"Capture"),
            item(pw_sys::PW_KEY_MEDIA_ROLE, c"Screen"),
        ];
        let props = spa_sys::spa_dict {
            flags: 0,
            n_items: items.len() as u32,
            items: items.as_ptr(),
        };
        // The stream takes ownership of the properties.
        let props = (pw.pw_properties_new_dict)(&props);
        let stream = (pw.pw_stream_new)(core, c"cua-driver-capture".as_ptr(), props);
        if stream.is_null() {
            return Err(creation_failed("Stream::new"));
        }
        let _stream = Release(move || (pw.pw_stream_destroy)(stream));
        pump.borrow_mut().stream = stream;

        let mut stream_events: pw_sys::pw_stream_events = std::mem::zeroed();
        stream_events.version = pw_sys::PW_VERSION_STREAM_EVENTS;
        stream_events.state_changed = Some(on_state_changed::<S>);
        stream_events.param_changed = Some(on_param_changed::<S>);
        stream_events.process = Some(on_process::<S>);
        let _stream_listener = Listener::add(stream_events, |hook, events| {
            (pw.pw_stream_add_listener)(stream, hook, events, data);
        });

        // `DONT_RECONNECT` binds the stream to the portal's node so the session
        // manager never relinks it to an unrelated video source once that node
        // is gone; the registry watch above reports the removal itself.
        let mut params = [enum_format.as_raw_ptr().cast_const()];
        let res = (pw.pw_stream_connect)(
            stream,
            spa_sys::SPA_DIRECTION_INPUT,
            node_id,
            pw_sys::pw_stream_flags_PW_STREAM_FLAG_AUTOCONNECT
                | pw_sys::pw_stream_flags_PW_STREAM_FLAG_MAP_BUFFERS
                | pw_sys::pw_stream_flags_PW_STREAM_FLAG_DONT_RECONNECT,
            params.as_mut_ptr(),
            params.len() as u32,
        );
        if res < 0 {
            anyhow::bail!(
                "pipewire stream.connect failed: {}",
                std::io::Error::from_raw_os_error(-res)
            );
        }

        let utils: *mut spa_sys::spa_interface = ptr::addr_of_mut!((*(*loop_).utils).iface);
        let ticker: *mut spa_sys::spa_source = spa_interface_call_method!(
            utils,
            spa_sys::spa_loop_utils_methods,
            add_timer,
            Some(on_tick::<S>),
            data
        );
        if ticker.is_null() {
            return Err(creation_failed("timer source"));
        }
        let _ticker = Release(move || {
            spa_interface_call_method!(
                utils,
                spa_sys::spa_loop_utils_methods,
                destroy_source,
                ticker
            )
        });
        let (mut value, mut interval) = (
            timespec(options.tick_interval),
            timespec(options.tick_interval),
        );
        let res = spa_interface_call_method!(
            utils,
            spa_sys::spa_loop_utils_methods,
            update_timer,
            ticker,
            &mut value,
            &mut interval,
            false
        );
        if res < 0 {
            anyhow::bail!(
                "pipewire timer source failed: {}",
                std::io::Error::from_raw_os_error(-res)
            );
        }

        (pw.pw_main_loop_run)(mainloop);
        // Leaving this block tears the stream down before the sink is handed
        // back, so the producer cannot touch buffers the sink may reference.
    }

    let pump = (*pump).into_inner();
    match pump.outcome {
        Some(Ok(())) | None => Ok(pump.sink),
        Some(Err(error)) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn only_the_cast_node_leaving_the_registry_ends_the_capture() {
        let reason = source_removed(44, 44).expect("the cast node was removed");
        assert!(reason.contains("removed the source"), "{reason}");
        assert_eq!(source_removed(47, 44), None, "other globals come and go");
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

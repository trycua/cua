//! Streams: targets, media tickets, and a Rust media session.
//!
//! - [`Space::windows`] / [`Space::stream_targets`]: `StreamService.ListTargets`.
//! - [`Space::open_stream`]: `StreamService.OpenMedia` → a [`StreamTicket`]
//!   (ticket + absolute `ws(s)://…/media?ticket=…` URL). The ticket is short
//!   lived, bound to one session, and safe to hand to a webview or put in a
//!   URL; it is never the Space's token. This is what `stream_endpoint`
//!   returns and what the operator display draws from.
//! - [`Space::stream_session`]: attaches to the media plane (rcdp wire v2)
//!   through the streaming client that ships with Cua Spaces
//!   (source-available, FSL-1.1-MIT, in `cua-spaces-ext`), registered with
//!   [`register_stream_client`]. It enforces the decoder invariants
//!   (keyframe gating per codec epoch, geometry epochs, per-track audio
//!   loss) and delivers encoded frames and audio packets to [`FrameSink`] /
//!   [`AudioSink`] callbacks on one delivery thread; decoding stays with the
//!   consumer. Without a registered client it fails with
//!   `host_capability_missing`. Tickets ([`Space::open_stream`]) need no
//!   client: any RCDP consumer (a web viewer, your own) can attach to them.

use crate::error::{Error, Result};
use crate::space::Space;
use cua_spacesd_client::pb;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// Apps whose windows are never auto-picked (the driver's own capture
/// window shows up as a target but is never what a user wants).
const NEVER_AUTO_PICK: [&str; 2] = ["cua driver", "cua-spacesd"];

/// Which surface to stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamTarget {
    /// A display (`None` = primary).
    Display(Option<String>),
    /// One window by handle.
    Window(String),
}

/// A window target as listed.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct WindowTarget {
    /// Window handle (`WindowRef.id`).
    pub window_id: String,
    /// Window generation.
    pub epoch: u64,
    /// Title.
    pub title: String,
    /// Owning app name.
    pub app_name: String,
    /// Owning app id.
    pub app_id: String,
    /// Owning pid.
    pub pid: u32,
    /// Bounds in global logical points: `[x, y, width, height]`.
    pub bounds: [f64; 4],
    /// On screen (not minimized or hidden).
    pub on_screen: bool,
    /// Focused.
    pub focused: bool,
    /// Streamable right now.
    pub available: bool,
    /// Why not, when not.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub limitation: String,
}

/// A window narrower or shorter than this (logical points) is a strip, a
/// badge or an off-screen placeholder, not something to watch.
pub const MIN_STREAMABLE_SIDE: f64 = 40.0;

/// Guest system UI that owns normal-layer windows (overlays, pickers, the
/// cursor and input helpers) but never a window a person streams.
const SYSTEM_UI_APPS: [&str; 10] = [
    "window server",
    "windowmanager",
    "dock",
    "control center",
    "notification center",
    "notificationcenter",
    "systemuiserver",
    "spotlight",
    "cursoruiviewservice",
    "textinputmenuagent",
];

impl WindowTarget {
    fn area(&self) -> f64 {
        self.bounds[2].max(0.0) * self.bounds[3].max(0.0)
    }

    /// Whether a viewer can stream this window now: the guest lists only
    /// normal-layer windows, and this one is on screen and streamable
    /// (not minimized, hidden or on another desktop), has a real size, and
    /// does not belong to the guest's system UI.
    pub fn is_streamable(&self) -> bool {
        let app = self.app_name.trim().to_lowercase();
        self.on_screen
            && self.available
            && self.bounds[2] >= MIN_STREAMABLE_SIDE
            && self.bounds[3] >= MIN_STREAMABLE_SIDE
            && !SYSTEM_UI_APPS.contains(&app.as_str())
    }
}

/// Options for [`Space::open_stream`].
#[derive(Clone, Debug, Default)]
pub struct StreamOptions {
    /// Acceptable codecs in preference order (empty = any).
    pub codecs: Vec<pb::MediaCodec>,
    /// FPS cap (0 = 30).
    pub max_fps: u32,
    /// Long-edge cap (0 = native).
    pub max_dimension: u32,
    /// Request the paired audio track.
    pub audio: bool,
    /// Ticket lifetime (default 60 s, at most 600 s).
    pub ticket_ttl: Option<Duration>,
    /// Input policy (default view-only).
    pub policy: Option<pb::SessionPolicy>,
}

/// A minted media session: everything a client needs to attach.
#[derive(Clone, Debug, serde::Serialize)]
pub struct StreamTicket {
    /// Space id.
    pub space: String,
    /// Media session id (for `CloseMedia`).
    pub media_session_id: String,
    /// Absolute media WebSocket URL, ticket included.
    pub ws_url: String,
    /// The bare ticket (also accepted as subprotocol `cua.ticket.<ticket>`).
    pub ticket: String,
    /// When the ticket stops being accepted for new attaches (RFC 3339).
    pub ticket_expires_at: Option<String>,
    /// Negotiated codec: `h264`, `bgra` or `png`.
    pub codec: String,
    /// Media wire version (2).
    pub wire_version: u32,
    /// Initial frame size `[width, height]`.
    pub frame_size: [u32; 2],
    /// Whether attaching needs extra headers ([`Space::websocket_headers`];
    /// Fleet gateway). A browser cannot set them; use the daemon's media
    /// bridge then.
    pub needs_gateway_headers: bool,
    /// The whole `OpenMediaResponse` minus the ticket.
    #[serde(skip)]
    pub raw: pb::OpenMediaResponse,
}

/// Memory and storage use (`SystemService.Metrics`). A size is the guest's
/// own limit only when its `*_limited` flag says so (a VM, a container's
/// cgroup limit); otherwise it is the host's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Usage {
    /// Memory in use, bytes.
    pub memory_used: u64,
    /// Memory size, bytes.
    pub memory_total: u64,
    /// `memory_total` is the guest's limit.
    pub memory_limited: bool,
    /// Home filesystem bytes in use.
    pub disk_used: u64,
    /// Home filesystem size, bytes.
    pub disk_total: u64,
    /// `disk_total` is the guest's own disk.
    pub disk_limited: bool,
}

impl From<pb::MetricsResponse> for Usage {
    fn from(m: pb::MetricsResponse) -> Self {
        Usage {
            memory_used: m.memory_used_bytes,
            memory_total: m.memory_total_bytes,
            memory_limited: m.memory_limited,
            disk_used: m.disk_used_bytes,
            disk_total: m.disk_total_bytes,
            disk_limited: m.disk_limited,
        }
    }
}

/// One of the Space's displays (`ComputerService.ListDisplays`).
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct DisplayInfo {
    /// Display id.
    pub id: String,
    /// Name ("XVFB-0", "Built-in Retina Display").
    pub name: String,
    /// The primary display.
    pub primary: bool,
    /// Framebuffer width in physical pixels.
    pub width_px: u32,
    /// Framebuffer height in physical pixels.
    pub height_px: u32,
    /// Physical pixels per logical point.
    pub scale_factor: f64,
}

impl From<pb::Display> for DisplayInfo {
    fn from(d: pb::Display) -> Self {
        let size = d.native_size.unwrap_or_default();
        DisplayInfo {
            id: d.id,
            name: d.name,
            primary: d.primary,
            width_px: size.width,
            height_px: size.height,
            scale_factor: d.scale_factor,
        }
    }
}

fn codec_name(codec: i32) -> String {
    match pb::MediaCodec::try_from(codec).unwrap_or_default() {
        pb::MediaCodec::H264 => "h264",
        pb::MediaCodec::Bgra => "bgra",
        pb::MediaCodec::Png => "png",
        pb::MediaCodec::Unspecified => "unspecified",
    }
    .into()
}

impl Space {
    /// The Space's displays, primary first (`ComputerService.ListDisplays`):
    /// what a "Desktop (1280×800)" label reads its resolution from.
    pub async fn displays(&self) -> Result<Vec<DisplayInfo>> {
        let mut list: Vec<DisplayInfo> = self
            .spacesd()?
            .displays()
            .await?
            .into_iter()
            .map(DisplayInfo::from)
            .collect();
        list.sort_by_key(|d| !d.primary);
        Ok(list)
    }

    /// A small JPEG preview of one of the Space's windows (at most
    /// `max_dimension` px), through the SDK's short-lived preview cache;
    /// `None` when the guest captured nothing.
    pub async fn window_thumbnail(
        &self,
        window_id: &str,
        epoch: u64,
        max_dimension: u32,
    ) -> Result<Option<Vec<u8>>> {
        let key = format!("guest:{}:{window_id}:{epoch}:{max_dimension}", self.id());
        let cache = cua_icon_cache::Thumbnails::shared();
        if let Some(hit) = cache.get(&key) {
            return Ok(hit.map(|p| p.as_ref().clone()));
        }
        let shot = self
            .spacesd()?
            .computer()
            .screenshot(pb::ScreenshotRequest {
                source: Some(pb::screenshot_request::Source::Window(pb::WindowRef {
                    id: window_id.into(),
                    epoch,
                })),
                format: pb::ImageFormat::Jpeg as i32,
                quality: 70,
                max_dimension,
                ..Default::default()
            })
            .await
            .map_err(cua_spacesd_client::Error::from)?
            .into_inner();
        let image = (!shot.image.is_empty()).then(|| shot.image.to_vec());
        Ok(cache.put(&key, image).map(|p| p.as_ref().clone()))
    }

    /// Memory and storage use now (`SystemService.Metrics`).
    pub async fn usage(&self) -> Result<Usage> {
        Ok(self.spacesd()?.metrics().await?.into())
    }

    /// `StreamService.ListTargets`.
    pub async fn stream_targets(&self, include_windows: bool) -> Result<pb::ListTargetsResponse> {
        self.require_any(&["desktop_stream", "window_stream"])?;
        Ok(self
            .spacesd()?
            .stream()
            .list_targets(pb::ListTargetsRequest {
                include_windows,
                window_filter: None,
            })
            .await
            .map_err(cua_spacesd_client::Error::from)?
            .into_inner())
    }

    /// The windows a viewer can stream ([`WindowTarget::is_streamable`]):
    /// on screen, sized, normal app windows. Optionally only those whose app
    /// name contains `app` (case-insensitive). Every client lists windows
    /// through this, so none shows invisible system windows.
    pub async fn windows(&self, app: Option<&str>) -> Result<Vec<WindowTarget>> {
        Ok(self
            .all_windows(app)
            .await?
            .into_iter()
            .filter(WindowTarget::is_streamable)
            .collect())
    }

    /// Every window target the guest lists, hidden, minimized and helper
    /// windows included (each with `available` and `limitation`). Opt in to
    /// this only for diagnostics; viewers use [`Space::windows`].
    pub async fn all_windows(&self, app: Option<&str>) -> Result<Vec<WindowTarget>> {
        self.require_any(&["window_stream", "windows"])?;
        let resp = self
            .spacesd()?
            .stream()
            .list_targets(pb::ListTargetsRequest {
                include_windows: true,
                window_filter: None,
            })
            .await
            .map_err(cua_spacesd_client::Error::from)?
            .into_inner();
        let wanted = app.map(str::to_lowercase).filter(|s| !s.is_empty());
        Ok(resp
            .targets
            .into_iter()
            .filter_map(|t| match t.target {
                Some(pb::stream_target::Target::Window(w)) => Some((w, t.available, t.limitation)),
                _ => None,
            })
            .map(|(w, available, limitation)| {
                let app = w.app.clone().unwrap_or_default();
                let b = w.bounds.unwrap_or_default();
                let r = w.r#ref.clone().unwrap_or_default();
                WindowTarget {
                    window_id: r.id,
                    epoch: r.epoch,
                    title: w.title,
                    app_name: app.name,
                    app_id: app.app_id,
                    pid: app.pid,
                    bounds: [b.x, b.y, b.width, b.height],
                    on_screen: w.on_screen,
                    focused: w.focused,
                    available,
                    limitation,
                }
            })
            .filter(|w| {
                wanted
                    .as_ref()
                    .is_none_or(|a| w.app_name.to_lowercase().contains(a))
            })
            .collect())
    }

    /// The largest visible window of an app (case-insensitive substring),
    /// skipping the driver's own windows: the content window, not a small
    /// dialog.
    pub async fn find_window(&self, app: &str) -> Result<WindowTarget> {
        let wanted = app.to_lowercase();
        self.windows(Some(app))
            .await?
            .into_iter()
            .filter(|w| {
                let a = w.app_name.to_lowercase();
                !NEVER_AUTO_PICK.iter().any(|n| a.contains(n)) && a.contains(&wanted)
            })
            .max_by(|a, b| a.area().total_cmp(&b.area()))
            .ok_or_else(|| Error::NotFound(format!("a visible {app:?} window in {}", self.id())))
    }

    /// Opens a media session and returns its ticket and URL.
    pub async fn open_stream(
        &self,
        target: StreamTarget,
        options: StreamOptions,
    ) -> Result<StreamTicket> {
        let target = match target {
            StreamTarget::Display(d) => {
                self.require("desktop_stream")?;
                pb::media_target::Target::DisplayId(d.unwrap_or_else(|| "primary".into()))
            }
            StreamTarget::Window(id) => {
                self.require("window_stream")?;
                pb::media_target::Target::Window(pb::WindowRef { id, epoch: 0 })
            }
        };
        if options.audio {
            self.require("audio.desktop")?;
        }
        let raw = self
            .spacesd()?
            .stream()
            .open_media(pb::OpenMediaRequest {
                target: Some(pb::MediaTarget {
                    target: Some(target),
                }),
                codecs: options.codecs.iter().map(|c| *c as i32).collect(),
                max_fps: options.max_fps,
                max_dimension: options.max_dimension,
                bitrate_kbps: 0,
                policy: options.policy.map(|p| p as i32).unwrap_or(0),
                geometry_control: 0,
                ticket_ttl: options.ticket_ttl.map(|d| pbjson_types::Duration {
                    seconds: d.as_secs().min(600) as i64,
                    nanos: 0,
                }),
                prefer_quic: false,
                audio: options.audio.then(|| pb::AudioOptions {
                    enabled: true,
                    ..Default::default()
                }),
                disable_video: false,
                presence_participant_id: self.presence_participant().unwrap_or_default(),
            })
            .await
            .map_err(cua_spacesd_client::Error::from)?
            .into_inner();
        let ws_path = if raw.ws_path.is_empty() {
            format!(
                "{}?ticket={}",
                cua_proto::metadata::MEDIA_WS_PATH,
                raw.ticket
            )
        } else {
            raw.ws_path.clone()
        };
        let size = raw
            .geometry
            .as_ref()
            .and_then(|g| g.frame_size.as_ref())
            .map(|s| [s.width, s.height])
            .unwrap_or([0, 0]);
        let mut shown = raw.clone();
        shown.ticket.clear();
        Ok(StreamTicket {
            space: self.id().to_string(),
            media_session_id: raw.media_session_id.clone(),
            ws_url: self.websocket_url(&ws_path)?,
            ticket: raw.ticket.clone(),
            ticket_expires_at: raw.ticket_expires_at.as_ref().map(|t| {
                humantime::format_rfc3339_seconds(
                    std::time::UNIX_EPOCH + Duration::from_secs(t.seconds.max(0) as u64),
                )
                .to_string()
            }),
            codec: codec_name(raw.codec),
            wire_version: raw.wire_version,
            frame_size: size,
            needs_gateway_headers: matches!(self.provider(), crate::Provider::Cloud),
            raw: shown,
        })
    }

    /// `StreamService.CloseMedia`.
    pub async fn close_stream(&self, media_session_id: &str) -> Result<()> {
        self.spacesd()?
            .stream()
            .close_media(pb::CloseMediaRequest {
                media_session_id: media_session_id.into(),
            })
            .await
            .map_err(cua_spacesd_client::Error::from)?;
        Ok(())
    }

    /// Opens a media session and attaches to it.
    pub async fn stream_session(
        &self,
        target: StreamTarget,
        options: StreamOptions,
        frames: Arc<dyn FrameSink>,
        audio: Option<Arc<dyn AudioSink>>,
    ) -> Result<StreamSession> {
        let ticket = self.open_stream(target, options).await?;
        let headers = self.websocket_headers().await?;
        match StreamSession::attach(&ticket, &headers, frames, audio).await {
            Ok(mut s) => {
                s.rpc = Some((self.clone(), ticket.media_session_id.clone()));
                Ok(s)
            }
            Err(e) => {
                let _ = self.close_stream(&ticket.media_session_id).await;
                Err(e)
            }
        }
    }
}

/// One encoded video access unit, keyframe-gated by the client state
/// machine: the first frame a sink sees after attaching, after a codec epoch
/// change, or after a drop is always a keyframe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoFrame {
    /// Frame sequence.
    pub sequence: u64,
    /// `h264`, `bgra` or `png`.
    pub codec: String,
    /// Random-access point (IDR with SPS/PPS for H.264).
    pub keyframe: bool,
    /// Payload width.
    pub width: u32,
    /// Payload height.
    pub height: u32,
    /// Media-clock capture time (µs).
    pub capture_timestamp_us: u64,
    /// Decoder configuration generation.
    pub codec_epoch: u64,
    /// Coordinate generation.
    pub geometry_epoch: u64,
    /// Payload.
    pub data: Vec<u8>,
}

/// One audio packet of a negotiated track.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioPacket {
    /// Track id.
    pub track_id: u16,
    /// Per-track sequence.
    pub sequence: u32,
    /// Media-clock time of the first sample (µs), shared with video.
    pub pts_us: u64,
    /// Samples per channel.
    pub frame_samples: u16,
    /// Packets lost before this one (conceal or FEC-decode that many).
    pub lost: u32,
    /// DTX frame.
    pub dtx: bool,
    /// One Opus packet or s16le PCM.
    pub data: Vec<u8>,
}

/// A control event.
#[derive(Clone, Debug, PartialEq)]
pub enum StreamEvent {
    /// `session_opened`, as JSON.
    Opened(serde_json::Value),
    /// An audio track was (re)configured, as JSON.
    AudioConfigured(serde_json::Value),
    /// Window lifecycle (geometry changed, closed, ...), as JSON.
    Lifecycle(serde_json::Value),
    /// Server stats, as JSON.
    Stats(serde_json::Value),
    /// Any other server message, as JSON.
    Message(serde_json::Value),
    /// The socket closed.
    Closed {
        /// Close code (1006 when the socket just ended).
        code: u16,
        /// Reason.
        reason: String,
    },
}

/// Receives video frames and control events. Runs on the session's delivery
/// thread; must hand decoding off rather than block for long.
pub trait FrameSink: Send + Sync {
    /// A video access unit.
    fn on_frame(&self, frame: VideoFrame);
    /// A control event.
    fn on_event(&self, _event: StreamEvent) {}
}

/// Receives audio packets on the session's delivery thread.
pub trait AudioSink: Send + Sync {
    /// An audio packet.
    fn on_audio(&self, packet: AudioPacket);
}

/// Delivery counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct StreamStats {
    /// Frames delivered.
    pub frames: u64,
    /// Keyframes delivered.
    pub keyframes: u64,
    /// Frames dropped because the sink fell behind (each forces a keyframe
    /// resync).
    pub frames_dropped: u64,
    /// Frames the client state machine held back while awaiting a keyframe.
    pub frames_gated: u64,
    /// Keyframe requests sent.
    pub keyframe_requests: u64,
    /// Audio packets delivered.
    pub audio_packets: u64,
    /// Audio packets reported lost.
    pub audio_lost: u64,
    /// Control events delivered.
    pub events: u64,
    /// Malformed messages ignored.
    pub malformed: u64,
}

/// Bound of the delivery queue. Video beyond it is dropped (and forces a
/// keyframe resync); audio and events wait.
pub const DELIVERY_QUEUE: usize = 256;

/// The streaming client behind [`StreamSession`]: it attaches to a ticket's
/// media socket and runs the RCDP client state machine. Cua Spaces
/// (source-available, FSL-1.1-MIT) ships the implementation in
/// `cua-spaces-ext`; this crate only defines the seam.
pub trait StreamClient: Send + Sync + 'static {
    /// Attaches to `ticket`'s media socket with `headers`.
    fn attach<'a>(
        &'a self,
        ticket: &'a StreamTicket,
        headers: &'a [(String, String)],
        frames: Arc<dyn FrameSink>,
        audio: Option<Arc<dyn AudioSink>>,
    ) -> crate::extension::BoxFuture<'a, Result<Box<dyn StreamConnection>>>;
}

/// One attached media socket (see [`StreamClient`]).
pub trait StreamConnection: Send + Sync {
    /// The media session id.
    fn media_session_id(&self) -> &str;
    /// Negotiated codec.
    fn codec(&self) -> &str;
    /// True once the socket has closed.
    fn is_closed(&self) -> bool;
    /// True once `hello` and `session_opened` have arrived.
    fn is_open(&self) -> bool;
    /// Counters.
    fn stats(&self) -> StreamStats;
    /// Asks for a keyframe.
    fn request_keyframe(&self) -> Result<()>;
    /// Sends interactive input (the RCDP `InteractiveInputEvent`s, as
    /// JSON).
    fn send_input(&self, events: Vec<serde_json::Value>) -> Result<()>;
    /// Sends a raw client text message.
    fn send_text(&self, json: String) -> Result<()>;
    /// Closes the socket, waiting (bounded) for the delivery thread.
    fn close(&mut self, wait: Duration) -> crate::extension::BoxFuture<'_, ()>;
}

static STREAM_CLIENT: OnceLock<Arc<dyn StreamClient>> = OnceLock::new();

/// Registers the streaming client for this process (once; later calls are
/// ignored). Cua Spaces registers its own.
pub fn register_stream_client(client: Arc<dyn StreamClient>) {
    let _ = STREAM_CLIENT.set(client);
}

/// Whether a streaming client is registered in this process.
pub fn has_stream_client() -> bool {
    STREAM_CLIENT.get().is_some()
}

/// A live media session.
pub struct StreamSession {
    inner: Box<dyn StreamConnection>,
    rpc: Option<(Space, String)>,
}

impl std::fmt::Debug for StreamSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamSession")
            .field("media_session_id", &self.media_session_id())
            .field("codec", &self.codec())
            .field("stats", &self.stats())
            .finish()
    }
}

/// What a guest whose cua-spacesd predates desktop-stream input on macOS
/// answers every click and key with.
const OLD_GUEST_DESKTOP_INPUT: &str = "whole-display stream is not implemented";

/// The words a person sees for input the Space refused on an
/// `interactive_input_acknowledgement`, when the guest's own words would
/// mislead: an old cua-spacesd reports a missing feature, which only a newer
/// image fixes. The guest's message stays under `error.detail`.
pub fn input_refusal_text(message: &str) -> Option<&'static str> {
    message.contains(OLD_GUEST_DESKTOP_INPUT).then_some(
        "this Space's cua-spacesd is too old for input on its desktop stream; \
         recreate the Space from the current image",
    )
}

/// Rewrites a refused input acknowledgement's message (`payload.error`)
/// with [`input_refusal_text`], in place.
#[doc(hidden)]
pub fn explain_input_refusal(v: &mut serde_json::Value) {
    if v.get("type").and_then(serde_json::Value::as_str)
        != Some("interactive_input_acknowledgement")
    {
        return;
    }
    let Some(error) = v
        .get_mut("payload")
        .and_then(|p| p.get_mut("error"))
        .and_then(serde_json::Value::as_object_mut)
    else {
        return;
    };
    let original = error
        .get("message")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    if let Some(text) = input_refusal_text(&original) {
        error.insert("message".into(), text.into());
        error.insert("detail".into(), original.into());
    }
}

impl StreamSession {
    /// Attaches to an already-minted ticket (for example one a daemon or the
    /// MCP `stream_endpoint` tool handed out), through the registered
    /// [`StreamClient`].
    pub async fn attach(
        ticket: &StreamTicket,
        headers: &[(String, String)],
        frames: Arc<dyn FrameSink>,
        audio: Option<Arc<dyn AudioSink>>,
    ) -> Result<StreamSession> {
        let client = STREAM_CLIENT
            .get()
            .ok_or_else(|| Error::needs_cua_spaces("streaming"))?;
        let inner = client.attach(ticket, headers, frames, audio).await?;
        Ok(StreamSession { inner, rpc: None })
    }

    /// The media session id.
    pub fn media_session_id(&self) -> &str {
        self.inner.media_session_id()
    }

    /// Negotiated codec.
    pub fn codec(&self) -> &str {
        self.inner.codec()
    }

    /// True once the socket has closed.
    pub fn is_closed(&self) -> bool {
        self.inner.is_closed()
    }

    /// True once `hello` and `session_opened` have arrived.
    pub fn is_open(&self) -> bool {
        self.inner.is_open()
    }

    /// Counters.
    pub fn stats(&self) -> StreamStats {
        self.inner.stats()
    }

    /// Asks for a keyframe (rate limited to one per second).
    pub fn request_keyframe(&self) -> Result<()> {
        self.inner.request_keyframe()
    }

    /// Sends interactive input (the session policy must allow it): RCDP
    /// `InteractiveInputEvent`s, or anything that serializes to them.
    pub fn send_input<E: serde::Serialize>(&self, events: Vec<E>) -> Result<()> {
        let events = events
            .iter()
            .map(serde_json::to_value)
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| Error::Stream(format!("input: {e}")))?;
        self.inner.send_input(events)
    }

    /// Sends a raw client text message (advanced).
    pub fn send_text(&self, json: String) -> Result<()> {
        self.inner.send_text(json)
    }

    /// Closes the socket and the media session.
    pub async fn close(mut self) -> Result<StreamStats> {
        self.inner.close(Duration::from_secs(5)).await;
        if let Some((space, id)) = self.rpc.take() {
            let _ = space.close_stream(&id).await;
        }
        Ok(self.stats())
    }
}

#[cfg(test)]
mod input_refusal_tests {
    use super::*;

    #[test]
    fn an_old_guest_refusal_says_to_recreate_the_space() {
        let old = "interactive input on a whole-display stream is not implemented on macOS; \
                   use the Computer service or stream a window";
        let mut v = serde_json::json!({
            "type": "interactive_input_acknowledgement",
            "payload": {"session_id": "s", "through_sequence": 3, "delivered": false,
                        "error": {"code": "unsupported", "message": old, "current_geometry_epoch": null}}
        });
        explain_input_refusal(&mut v);
        let e = &v["payload"]["error"];
        assert!(e["message"].as_str().unwrap().contains("too old"), "{v}");
        assert!(
            e["message"]
                .as_str()
                .unwrap()
                .contains("recreate the Space"),
            "{v}"
        );
        assert_eq!(e["detail"], old);
        assert_eq!(e["code"], "unsupported");
    }

    #[test]
    fn other_refusals_keep_the_guest_words() {
        let mut v = serde_json::json!({
            "type": "interactive_input_acknowledgement",
            "payload": {"delivered": false, "error": {"code": "view_only", "message": "view only"}}
        });
        let before = v.clone();
        explain_input_refusal(&mut v);
        assert_eq!(v, before);
        let mut other = serde_json::json!({"type": "stats", "payload": {}});
        explain_input_refusal(&mut other);
        assert_eq!(other, serde_json::json!({"type": "stats", "payload": {}}));
    }
}

#[cfg(test)]
mod streamable_tests {
    use super::*;

    fn w(app: &str, bounds: [f64; 4], on_screen: bool) -> WindowTarget {
        WindowTarget {
            window_id: "1".into(),
            epoch: 1,
            title: String::new(),
            app_name: app.into(),
            app_id: String::new(),
            pid: 1,
            bounds,
            on_screen,
            focused: false,
            available: on_screen,
            limitation: String::new(),
        }
    }

    /// The windows a fresh macOS 26 guest lists with Calculator, TextEdit
    /// and a Finder window open: only those three are streamable.
    #[test]
    fn only_visible_sized_app_windows_are_streamable() {
        let listed = [
            w(
                "Open and Save Panel Service",
                [0.0, 268.0, 500.0, 500.0],
                false,
            ),
            w("Finder", [0.0, 0.0, 1024.0, 30.0], false),
            w("Spotlight", [0.0, 268.0, 64.0, 64.0], false),
            w("CursorUIViewService", [0.0, 268.0, 64.0, 64.0], false),
            w("Accessibility", [0.0, 268.0, 500.0, 500.0], false),
            w("TextEdit", [0.0, 0.0, 1024.0, 30.0], false),
            w("Calculator", [100.0, 100.0, 230.0, 408.0], true),
            w("TextEdit", [200.0, 100.0, 603.0, 505.0], true),
            w("Finder", [50.0, 80.0, 920.0, 436.0], true),
        ];
        let shown: Vec<_> = listed.iter().filter(|w| w.is_streamable()).collect();
        assert_eq!(shown.len(), 3);
        assert!(shown.iter().all(|w| w.on_screen));
    }

    #[test]
    fn strips_and_system_ui_are_not_streamable() {
        assert!(!w("Finder", [0.0, 0.0, 1024.0, 30.0], true).is_streamable());
        assert!(!w("Finder", [0.0, 0.0, 0.0, 0.0], true).is_streamable());
        assert!(!w("Spotlight", [0.0, 0.0, 600.0, 400.0], true).is_streamable());
        assert!(!w("Control Center", [0.0, 0.0, 300.0, 400.0], true).is_streamable());
        let mut busy = w("Safari", [0.0, 0.0, 800.0, 600.0], true);
        assert!(busy.is_streamable());
        busy.available = false;
        assert!(!busy.is_streamable());
    }
}

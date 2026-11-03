// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Media sessions: one capture per target, fanned out to attached viewers.
//!
//! A session is created by `StreamService.OpenMedia` and outlives any single
//! socket: up to [`MAX_SOCKETS_PER_SESSION`] sockets attach with the session's
//! ticket, each as its own [`Viewer`] with its own queues and flow control. The
//! encoder is shared; a slow viewer never slows the encoder or other viewers
//! because its queue drops to the next keyframe instead of growing.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use cua_media_protocol::v2::{
    self, AudioCodecName, AudioConfig, AudioDirection, AudioSourceRef, AudioTrackState,
    AudioTrackStateKind, AudioTrackStats, MediaTargetRef, ServerMessage, MAX_MEDIA_SESSIONS,
    MAX_SOCKETS_PER_SESSION,
};
use cua_media_protocol::{
    ActionCapability, CodecEpoch, FrameSequence, GeometryEpoch, SessionPolicy, SurfaceGeometry,
    SuspensionReason, TargetEpoch, TargetHandle, VideoCodec, VideoFrameDescriptor,
    WindowGeometryControl, WindowLifecycleEvent, WindowSessionId,
};
use cua_media_transport::audio::{encode_audio_packet, AudioPacketHeader};
use cua_spacesd_provider_api::{
    AccessibilityProvider, ActionProvider, CaptureConfig, CaptureEvent, CaptureLease,
    CaptureProvider, CaptureSink, DisplayProvider, InteractiveInputLease, InteractiveInputProvider,
    OwnedFrame, PixelFormat, ProviderError, ProviderErrorCode, ProviderTarget, TargetProvider,
    WindowGeometryProvider,
};

use super::audio::{
    normalise_config, quic_bitrate_cap, AudioFrame, AudioFrameSink, AudioProvider, AudioSourceInfo,
    AudioTrackConfig, AudioTrackLease, AudioUplinkSink,
};
use super::leases::InputLeases;
use super::rate::{CongestionSample, RateController, RATE_WINDOW};
use super::viewer::{Viewer, ViewerShared};

/// Default H.264 bitrate when the client leaves it to the server.
pub const DEFAULT_BITRATE_KBPS: u32 = 4_000;
/// Default frame-rate cap.
pub const DEFAULT_MAX_FPS: u16 = 30;
/// A cached GOP longer than this is not replayed on attach; the session
/// forces a fresh IDR instead.
const MAX_GOP_REPLAY_FRAMES: usize = 90;
const MAX_GOP_REPLAY_BYTES: usize = 8 * 1024 * 1024;
/// How long `open` waits for the first frame so the response carries the
/// authoritative geometry.
const FIRST_FRAME_WAIT: Duration = Duration::from_millis(1_500);
/// Session maintenance cadence (correlation deadlines, stall probes, reaping).
const TICK: Duration = Duration::from_millis(100);
/// Default idle refresh: a video session that sent no frame for this long
/// re-sends the current picture as an IDR, so a viewer that missed the last
/// keyframe (late join, loss, a still desktop that produced one frame)
/// recovers without asking. `CUA_ENV_MEDIA_IDLE_KEYFRAME_MS` overrides it
/// (`0` disables); [`MediaRuntime::set_idle_keyframe_interval`] too.
pub const DEFAULT_IDLE_KEYFRAME_INTERVAL: Duration = Duration::from_secs(3);

fn idle_keyframe_from_env() -> Option<Duration> {
    match std::env::var("CUA_ENV_MEDIA_IDLE_KEYFRAME_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
    {
        Some(0) => None,
        Some(ms) => Some(Duration::from_millis(ms.clamp(250, 600_000))),
        None => Some(DEFAULT_IDLE_KEYFRAME_INTERVAL),
    }
}

/// Every backend a media session may use.
#[derive(Clone)]
pub struct MediaProviders {
    pub targets: Arc<dyn TargetProvider>,
    pub displays: Arc<dyn DisplayProvider>,
    pub captures: Arc<dyn CaptureProvider>,
    pub actions: Arc<dyn ActionProvider>,
    pub accessibility: Arc<dyn AccessibilityProvider>,
    pub geometry: Arc<dyn WindowGeometryProvider>,
    pub inputs: Arc<dyn InteractiveInputProvider>,
    pub audio: Option<Arc<dyn AudioProvider>>,
}

/// Who opened a session (from the gRPC principal).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaPrincipal {
    pub id: String,
    pub name: String,
    pub color: String,
    pub agent: bool,
}

impl MediaPrincipal {
    pub fn anonymous() -> Self {
        Self {
            id: "anonymous".into(),
            name: "Anonymous".into(),
            color: "#888888".into(),
            agent: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaTargetSpec {
    Window {
        handle: TargetHandle,
        epoch: TargetEpoch,
    },
    Display {
        display_id: String,
    },
}

/// Uplink request, already checked against the caller's permission.
#[derive(Debug, Clone)]
pub struct UplinkRequest {
    pub config: AudioTrackConfig,
    pub virtual_source_name: String,
    /// `Err(reason)` when policy denies the uplink before the provider is
    /// asked.
    pub permitted: Result<(), String>,
}

#[derive(Debug, Clone)]
pub struct AudioRequest {
    /// Send downlink tracks. False with an uplink means uplink only.
    pub downlink: bool,
    /// Sources to capture; empty means the source paired with the target.
    pub source_ids: Vec<String>,
    /// Downlink encoding; `source_id` is ignored.
    pub config: AudioTrackConfig,
    pub uplink: Option<UplinkRequest>,
}

#[derive(Debug, Clone)]
pub struct OpenMediaParams {
    pub target: MediaTargetSpec,
    /// Acceptable codecs in client preference order; empty means any.
    pub codecs: Vec<VideoCodec>,
    pub max_fps: u16,
    pub max_dimension: u32,
    pub bitrate_kbps: Option<u32>,
    pub policy: SessionPolicy,
    pub geometry_control: WindowGeometryControl,
    pub principal: MediaPrincipal,
    pub disable_video: bool,
    pub audio: Option<AudioRequest>,
    /// Session may be attached over QUIC datagrams: audio bitrate is capped
    /// so every packet fits one datagram.
    pub quic: bool,
    /// New attaches are refused after this instant. A session with no viewer
    /// past it is reaped.
    pub attach_deadline: Instant,
}

/// A negotiated downlink track.
#[derive(Debug, Clone)]
pub struct NegotiatedTrack {
    pub track_id: u16,
    pub source: AudioSourceInfo,
}

#[derive(Debug, Clone)]
pub struct UplinkGrant {
    pub granted: bool,
    pub denied_reason: Option<String>,
    pub track_id: u16,
    pub config: AudioTrackConfig,
    pub virtual_source_name: String,
}

#[derive(Debug, Clone)]
pub struct NegotiatedAudioInfo {
    pub tracks: Vec<NegotiatedTrack>,
    pub config: AudioTrackConfig,
    pub uplink: Option<UplinkGrant>,
}

/// Result of [`MediaRuntime::open`].
pub struct OpenedMedia {
    pub session: Arc<MediaSession>,
    pub opened: v2::SessionOpened,
    /// Target bounds in global logical points, when known.
    pub logical_bounds: Option<(f64, f64, f64, f64)>,
    pub audio: Option<NegotiatedAudioInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaError {
    NotFound(String),
    InvalidArgument(String),
    FailedPrecondition(String),
    ResourceExhausted(String),
    Unavailable(String),
    Provider(ProviderError),
}

impl std::fmt::Display for MediaError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(message)
            | Self::InvalidArgument(message)
            | Self::FailedPrecondition(message)
            | Self::ResourceExhausted(message)
            | Self::Unavailable(message) => formatter.write_str(message),
            Self::Provider(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for MediaError {}

impl From<ProviderError> for MediaError {
    fn from(error: ProviderError) -> Self {
        Self::Provider(error)
    }
}

/// All media sessions of one driver.
pub struct MediaRuntime {
    providers: MediaProviders,
    leases: Arc<InputLeases>,
    sessions: Mutex<HashMap<String, Arc<MediaSession>>>,
    geometry_owners: Mutex<HashMap<String, String>>,
    next_session: AtomicU64,
    idle_keyframe: Mutex<Option<Duration>>,
}

impl MediaRuntime {
    /// The idle refresh interval for sessions opened from now on (`None`
    /// disables it). See [`DEFAULT_IDLE_KEYFRAME_INTERVAL`].
    pub fn set_idle_keyframe_interval(&self, interval: Option<Duration>) {
        *self
            .idle_keyframe
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = interval;
    }

    pub fn new(providers: MediaProviders, leases: Arc<InputLeases>) -> Arc<Self> {
        Arc::new(Self {
            providers,
            leases,
            sessions: Mutex::new(HashMap::new()),
            geometry_owners: Mutex::new(HashMap::new()),
            next_session: AtomicU64::new(1),
            idle_keyframe: Mutex::new(idle_keyframe_from_env()),
        })
    }

    pub fn providers(&self) -> &MediaProviders {
        &self.providers
    }

    pub fn leases(&self) -> &Arc<InputLeases> {
        &self.leases
    }

    pub fn session(&self, id: &str) -> Option<Arc<MediaSession>> {
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(id)
            .cloned()
    }

    pub fn session_count(&self) -> usize {
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// Close one session. Attached sockets close with `code` (4404 for
    /// `CloseMedia`).
    pub fn close(&self, id: &str, code: u16) -> bool {
        let session = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(id);
        match session {
            Some(session) => {
                session.shutdown(code);
                self.release_geometry(&session);
                true
            }
            None => false,
        }
    }

    /// Close every session (root token rotated: 4401).
    pub fn close_all(&self, code: u16) {
        let sessions = std::mem::take(
            &mut *self
                .sessions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        for session in sessions.into_values() {
            session.shutdown(code);
            self.release_geometry(&session);
        }
    }

    fn release_geometry(&self, session: &MediaSession) {
        self.geometry_owners
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|_, owner| owner != &session.id);
    }

    fn resolve_target(&self, spec: &MediaTargetSpec) -> Result<ResolvedTarget, MediaError> {
        match spec {
            MediaTargetSpec::Window { handle, epoch } => {
                // Refresh the catalog so a newly created window resolves.
                let _ = self
                    .providers
                    .targets
                    .enumerate(&cua_spacesd_provider_api::TargetQuery::default());
                let target = self.providers.targets.resolve(handle, *epoch)?;
                Ok((
                    target,
                    MediaTargetRef::Window {
                        handle: handle.clone(),
                        epoch: *epoch,
                    },
                    None,
                ))
            }
            MediaTargetSpec::Display { display_id } => {
                let displays = self.providers.displays.displays()?;
                let display = displays
                    .iter()
                    .find(|display| {
                        display.id == *display_id || (display_id == "primary" && display.primary)
                    })
                    .ok_or_else(|| MediaError::NotFound(format!("unknown display {display_id}")))?;
                let target = self.providers.displays.display_target(&display.id)?;
                Ok((
                    target,
                    MediaTargetRef::Display {
                        display_id: display.id.clone(),
                    },
                    Some(display.bounds),
                ))
            }
        }
    }

    /// Create a media session and start capturing.
    pub fn open(self: &Arc<Self>, params: OpenMediaParams) -> Result<OpenedMedia, MediaError> {
        if self.session_count() >= MAX_MEDIA_SESSIONS {
            return Err(MediaError::ResourceExhausted(format!(
                "at most {MAX_MEDIA_SESSIONS} media sessions per driver"
            )));
        }
        let (target, target_ref, logical_bounds) = self.resolve_target(&params.target)?;
        if target_ref.is_display()
            && params.geometry_control == WindowGeometryControl::Bidirectional
        {
            return Err(MediaError::InvalidArgument(
                "bidirectional geometry control applies to window targets only".into(),
            ));
        }
        let id = format!(
            "media-{}-{:016x}",
            self.next_session.fetch_add(1, Ordering::Relaxed),
            random_u64()
        );
        let target_key = target_key(&target_ref);
        if params.geometry_control == WindowGeometryControl::Bidirectional {
            let mut owners = self
                .geometry_owners
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if owners.contains_key(&target_key) {
                return Err(MediaError::FailedPrecondition(
                    "another session already controls this window's geometry".into(),
                ));
            }
            owners.insert(target_key.clone(), id.clone());
        }

        let formats = if params.disable_video {
            Vec::new()
        } else {
            self.providers.captures.formats(&target.id)?
        };
        let codec = if params.disable_video {
            VideoCodec::H264
        } else {
            select_codec(&params.codecs, &formats).ok_or_else(|| {
                MediaError::FailedPrecondition(
                    "no acceptable codec: the target offers none of the requested codecs".into(),
                )
            })?
        };
        let max_fps = if params.max_fps == 0 {
            DEFAULT_MAX_FPS
        } else {
            params.max_fps.min(240)
        };
        let bitrate_kbps = params
            .bitrate_kbps
            .filter(|kbps| *kbps > 0)
            .unwrap_or(DEFAULT_BITRATE_KBPS)
            .clamp(250, 100_000);
        let action_capabilities = match &target_ref {
            MediaTargetRef::Window { .. } => self
                .providers
                .actions
                .capabilities(&target.id)
                .unwrap_or_default(),
            MediaTargetRef::Display { .. } => Vec::new(),
        };
        let initial_geometry = target.descriptor.geometry.clone();
        let session = Arc::new(MediaSession {
            id: id.clone(),
            runtime: Arc::downgrade(self),
            target: target.clone(),
            target_ref: target_ref.clone(),
            target_key,
            principal: params.principal.clone(),
            policy: params.policy,
            geometry_control: params.geometry_control,
            codec: codec.clone(),
            video_enabled: !params.disable_video,
            attach_deadline: params.attach_deadline,
            action_capabilities,
            closed: AtomicBool::new(false),
            max_fps: AtomicU16::new(max_fps),
            max_dimension: AtomicU32::new(params.max_dimension),
            bitrate_kbps: AtomicU32::new(bitrate_kbps),
            video: Mutex::new(VideoState {
                next_sequence: random_u64() >> 24 | 1,
                geometry_epoch: 1,
                geometry: initial_geometry,
                wire_codec_epoch: 0,
                provider_epoch: None,
                gop: VecDeque::new(),
                gop_bytes: 0,
                last_frame: None,
                last_frame_at: None,
                frames_emitted: 0,
                bytes_emitted: 0,
                keyframe_requests: 0,
                idle_keyframes: 0,
                preference_updates: 0,
                capture_generation: 0,
                first_frame: false,
                suspended: None,
            }),
            capture: Mutex::new(None),
            capture_generation: Arc::new(AtomicU64::new(0)),
            viewers: Mutex::new(Vec::new()),
            next_viewer: AtomicU64::new(1),
            input: Mutex::new(None),
            rate: Mutex::new(RateController::new(bitrate_kbps, max_fps)),
            audio_tracks: Mutex::new(Vec::new()),
            uplink: Mutex::new(None),
            first_frame_signal: (Mutex::new(false), std::sync::Condvar::new()),
            last_keyframe_request: Mutex::new(None),
            geometry_revision: AtomicU64::new(0),
            capture_paused: std::sync::atomic::AtomicBool::new(false),
            idle_keyframe: *self
                .idle_keyframe
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            last_idle_keyframe: Mutex::new(None),
        });

        if session.video_enabled {
            session.start_capture(&formats)?;
        } else {
            *session.first_frame_signal.0.lock().unwrap() = true;
        }
        let audio = match params.audio.as_ref() {
            Some(request) => session.start_audio(request, params.quic, &target)?,
            None => None,
        };
        // Authoritative geometry: wait briefly for the first frame.
        session.wait_first_frame(FIRST_FRAME_WAIT);

        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id.clone(), session.clone());
        session.spawn_maintenance();
        let opened = session.session_opened();
        Ok(OpenedMedia {
            session,
            opened,
            logical_bounds: logical_bounds.or_else(|| {
                let geometry = &target.descriptor.geometry;
                Some((
                    0.0,
                    0.0,
                    f64::from(geometry.width_px) / geometry.scale_factor.max(0.01),
                    f64::from(geometry.height_px) / geometry.scale_factor.max(0.01),
                ))
            }),
            audio,
        })
    }
}

pub(crate) fn target_key(target: &MediaTargetRef) -> String {
    match target {
        MediaTargetRef::Window { handle, .. } => format!("window:{}", handle.0),
        MediaTargetRef::Display { display_id } => format!("display:{display_id}"),
    }
}

fn select_codec(accepted: &[VideoCodec], formats: &[PixelFormat]) -> Option<VideoCodec> {
    let preference: Vec<VideoCodec> = if accepted.is_empty() {
        vec![VideoCodec::H264, VideoCodec::Png, VideoCodec::Bgra]
    } else {
        accepted.to_vec()
    };
    preference.into_iter().find(|codec| match codec {
        VideoCodec::H264 => formats.contains(&PixelFormat::H264AnnexB),
        VideoCodec::Png => formats.contains(&PixelFormat::Png),
        VideoCodec::Bgra => formats.contains(&PixelFormat::Bgra8),
        VideoCodec::Unknown => false,
    })
}

pub(crate) fn random_u64() -> u64 {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default(),
    );
    hasher.finish()
}

/// A resolved target, its wire reference and its logical bounds.
type ResolvedTarget = (ProviderTarget, MediaTargetRef, Option<(f64, f64, f64, f64)>);

/// One encoded or packed video frame ready for the wire.
#[derive(Debug)]
pub struct VideoPacket {
    pub descriptor: VideoFrameDescriptor,
    pub payload: Arc<[u8]>,
    pub keyframe: bool,
    pub captured_at: Instant,
}

impl VideoPacket {
    pub fn sequence(&self) -> u64 {
        self.descriptor.sequence.0
    }

    /// The v2 binary packet: length-prefixed JSON header plus payload.
    pub fn encode(&self) -> Vec<u8> {
        cua_media_transport::encode_packet(
            &cua_media_protocol::WireHeader::Video(self.descriptor.clone()),
            &self.payload,
        )
        .expect("video descriptors are small and payloads are bounded")
    }
}

pub(crate) struct VideoState {
    pub(crate) next_sequence: u64,
    pub(crate) geometry_epoch: u64,
    pub(crate) geometry: SurfaceGeometry,
    wire_codec_epoch: u64,
    provider_epoch: Option<(u64, u64)>,
    pub(crate) gop: VecDeque<Arc<VideoPacket>>,
    gop_bytes: usize,
    pub(crate) last_frame: Option<Arc<VideoPacket>>,
    pub(crate) last_frame_at: Option<Instant>,
    pub(crate) frames_emitted: u64,
    pub(crate) bytes_emitted: u64,
    pub(crate) keyframe_requests: u64,
    /// Idle refresh IDRs requested by the server itself.
    pub(crate) idle_keyframes: u64,
    pub(crate) preference_updates: u64,
    capture_generation: u64,
    first_frame: bool,
    pub(crate) suspended: Option<String>,
}

pub(crate) struct AudioTrackRuntime {
    pub(crate) track_id: u16,
    pub(crate) source: AudioSourceInfo,
    pub(crate) config: Mutex<AudioTrackConfig>,
    pub(crate) config_epoch: AtomicU8,
    next_sequence: AtomicU32,
    pub(crate) lease: Mutex<Option<Arc<dyn AudioTrackLease>>>,
    pub(crate) paused: AtomicBool,
    pub(crate) state: Mutex<AudioTrackStateKind>,
    pub(crate) packets: AtomicU64,
    pub(crate) bytes: AtomicU64,
    pub(crate) opus_pre_skip: AtomicU16,
}

pub(crate) struct UplinkRuntime {
    pub(crate) grant: UplinkGrant,
    pub(crate) sink: Arc<dyn AudioUplinkSink>,
    pub(crate) muted: AtomicBool,
    pub(crate) last_sequence: Mutex<Option<u32>>,
    pub(crate) packets: AtomicU64,
    pub(crate) lost: AtomicU64,
    pub(crate) late: AtomicU64,
}

/// One media session.
pub struct MediaSession {
    pub(crate) id: String,
    pub(crate) runtime: Weak<MediaRuntime>,
    pub(crate) target: ProviderTarget,
    pub(crate) target_ref: MediaTargetRef,
    pub(crate) target_key: String,
    pub(crate) principal: MediaPrincipal,
    pub(crate) policy: SessionPolicy,
    pub(crate) geometry_control: WindowGeometryControl,
    pub(crate) codec: VideoCodec,
    pub(crate) video_enabled: bool,
    attach_deadline: Instant,
    pub(crate) action_capabilities: Vec<ActionCapability>,
    closed: AtomicBool,
    pub(crate) max_fps: AtomicU16,
    pub(crate) max_dimension: AtomicU32,
    pub(crate) bitrate_kbps: AtomicU32,
    pub(crate) video: Mutex<VideoState>,
    capture: Mutex<Option<Arc<dyn CaptureLease>>>,
    capture_generation: Arc<AtomicU64>,
    pub(crate) viewers: Mutex<Vec<Arc<ViewerShared>>>,
    next_viewer: AtomicU64,
    pub(crate) input: Mutex<Option<Arc<dyn InteractiveInputLease>>>,
    rate: Mutex<RateController>,
    pub(crate) audio_tracks: Mutex<Vec<Arc<AudioTrackRuntime>>>,
    pub(crate) uplink: Mutex<Option<Arc<UplinkRuntime>>>,
    first_frame_signal: (Mutex<bool>, std::sync::Condvar),
    last_keyframe_request: Mutex<Option<Instant>>,
    pub(crate) geometry_revision: AtomicU64,
    /// Idle refresh interval (None: off).
    idle_keyframe: Option<Duration>,
    /// Capture paused because the last socket detached.
    capture_paused: std::sync::atomic::AtomicBool,
    last_idle_keyframe: Mutex<Option<Instant>>,
}

impl std::fmt::Debug for MediaSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MediaSession")
            .field("id", &self.id)
            .field("target", &self.target_ref)
            .finish_non_exhaustive()
    }
}

struct SessionCaptureSink {
    session: Weak<MediaSession>,
    generation: u64,
    active_generation: Arc<AtomicU64>,
}

impl CaptureSink for SessionCaptureSink {
    fn on_event(&self, event: CaptureEvent) {
        if self.active_generation.load(Ordering::Acquire) != self.generation {
            return;
        }
        if let Some(session) = self.session.upgrade() {
            session.on_capture_event(self.generation, event);
        }
    }
}

struct SessionAudioSink {
    session: Weak<MediaSession>,
    track: Arc<AudioTrackRuntime>,
}

impl AudioFrameSink for SessionAudioSink {
    fn on_frame(&self, frame: AudioFrame) {
        if let Some(session) = self.session.upgrade() {
            session.on_audio_frame(&self.track, frame);
        }
    }

    fn on_state(&self, state: AudioTrackStateKind, reason: Option<String>) {
        if let Some(session) = self.session.upgrade() {
            *self
                .track
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = state;
            session.broadcast_control(ServerMessage::AudioTrackState(AudioTrackState {
                track_id: self.track.track_id,
                state,
                reason,
            }));
        }
    }
}

impl MediaSession {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn target_ref(&self) -> &MediaTargetRef {
        &self.target_ref
    }

    pub fn target_key(&self) -> &str {
        &self.target_key
    }

    pub fn principal(&self) -> &MediaPrincipal {
        &self.principal
    }

    pub fn policy(&self) -> SessionPolicy {
        self.policy
    }

    pub fn codec(&self) -> VideoCodec {
        self.codec.clone()
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    pub fn attach_deadline(&self) -> Instant {
        self.attach_deadline
    }

    pub fn viewer_count(&self) -> usize {
        self.viewers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    pub fn encoder_name(&self) -> Option<String> {
        self.capture
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(|lease| lease.encoder_name())
    }

    pub(crate) fn providers(&self) -> Option<MediaProviders> {
        self.runtime
            .upgrade()
            .map(|runtime| runtime.providers.clone())
    }

    pub(crate) fn leases(&self) -> Option<Arc<InputLeases>> {
        self.runtime.upgrade().map(|runtime| runtime.leases.clone())
    }

    fn capture_config(&self, formats: &[PixelFormat]) -> CaptureConfig {
        let wanted = match self.codec {
            VideoCodec::H264 => PixelFormat::H264AnnexB,
            VideoCodec::Png => PixelFormat::Png,
            _ => PixelFormat::Bgra8,
        };
        let _ = formats;
        CaptureConfig {
            max_fps: self.max_fps.load(Ordering::Relaxed),
            max_dimension: self.max_dimension.load(Ordering::Relaxed),
            target_bitrate_kbps: Some(self.bitrate_kbps.load(Ordering::Relaxed)),
            accepted_formats: vec![wanted],
        }
    }

    fn start_capture(self: &Arc<Self>, formats: &[PixelFormat]) -> Result<(), MediaError> {
        let providers = self
            .providers()
            .ok_or_else(|| MediaError::Unavailable("driver is shutting down".into()))?;
        let generation = self.capture_generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.video
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .capture_generation = generation;
        let sink = Arc::new(SessionCaptureSink {
            session: Arc::downgrade(self),
            generation,
            active_generation: self.capture_generation.clone(),
        });
        let lease =
            providers
                .captures
                .start(&self.target.id, &self.capture_config(formats), sink)?;
        let previous = self
            .capture
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .replace(lease);
        if let Some(previous) = previous {
            previous.stop();
        }
        Ok(())
    }

    fn wait_first_frame(&self, timeout: Duration) {
        let (lock, signal) = &self.first_frame_signal;
        let guard = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = signal
            .wait_timeout_while(guard, timeout, |seen| !*seen)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
    }

    /// The `session_opened` payload as of now (current geometry and epochs).
    pub fn session_opened(&self) -> v2::SessionOpened {
        let video = self
            .video
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        v2::SessionOpened {
            session_id: WindowSessionId(self.id.clone()),
            target: self.target_ref.clone(),
            target_epoch: self.target.id.epoch,
            geometry_epoch: GeometryEpoch(video.geometry_epoch),
            codec_epoch: CodecEpoch(video.wire_codec_epoch.max(1)),
            geometry: video.geometry.clone(),
            codec: self.codec.clone(),
            video: self.video_enabled,
            max_fps: self.max_fps.load(Ordering::Relaxed),
            max_dimension: self.max_dimension.load(Ordering::Relaxed),
            target_bitrate_kbps: (self.codec == VideoCodec::H264)
                .then(|| self.bitrate_kbps.load(Ordering::Relaxed)),
            capabilities: session_capabilities(self.policy),
            action_capabilities: self.action_capabilities.clone(),
            policy: self.policy,
            geometry_control: self.geometry_control,
        }
    }

    /// Current geometry and its epoch.
    pub fn geometry(&self) -> (u64, SurfaceGeometry) {
        let video = self
            .video
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (video.geometry_epoch, video.geometry.clone())
    }

    /// Audio config messages for every track (sent after `session_opened`).
    pub fn audio_configs(&self) -> Vec<AudioConfig> {
        let mut configs: Vec<AudioConfig> = self
            .audio_tracks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(|track| track_audio_config(track, AudioDirection::Down))
            .collect();
        if let Some(uplink) = self
            .uplink
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            let config = &uplink.grant.config;
            configs.push(AudioConfig {
                track_id: uplink.grant.track_id,
                config_epoch: 1,
                direction: AudioDirection::Up,
                codec: config.codec,
                sample_rate_hz: config.sample_rate_hz,
                channels: config.channels,
                frame_ms: config.frame_ms,
                bitrate_kbps: config.bitrate_kbps,
                fec: config.fec,
                dtx: config.dtx,
                opus_pre_skip: 0,
                source: AudioSourceRef {
                    source_id: uplink.grant.virtual_source_name.clone(),
                    kind: "uplink".into(),
                    desktop_fallback: false,
                },
            });
        }
        configs
    }

    /// Attach a new socket. Fails with the close code the socket should use.
    pub fn attach(self: &Arc<Self>) -> Result<Viewer, u16> {
        if self.is_closed() {
            return Err(v2::close_code::SESSION_CLOSED);
        }
        let shared = {
            let mut viewers = self
                .viewers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if viewers.len() >= MAX_SOCKETS_PER_SESSION {
                return Err(v2::close_code::TOO_MANY_SOCKETS);
            }
            let shared = Arc::new(ViewerShared::new(
                self.next_viewer.fetch_add(1, Ordering::Relaxed),
                &self.id,
                self.codec.clone(),
                self.bitrate_kbps.load(Ordering::Relaxed),
            ));
            viewers.push(shared.clone());
            shared
        };
        // Greeting: hello, session_opened, one audio_config per track.
        shared.push_control(ServerMessage::Hello(v2::Hello::server(hello_capabilities(
            self.providers()
                .is_some_and(|providers| providers.audio.is_some()),
        ))));
        shared.push_control(ServerMessage::SessionOpened(self.session_opened()));
        for config in self.audio_configs() {
            shared.push_control(ServerMessage::AudioConfig(config));
        }
        // The first socket after an idle spell resumes a paused capture; the
        // GOP from before the pause is stale, so it asks for a fresh IDR.
        let was_idle = self.capture_paused.swap(false, Ordering::AcqRel);
        if was_idle {
            self.set_capture_paused(false);
        }
        // Keyframe on attach: replay the current GOP when it is short, else
        // force a fresh IDR. Either way the first video packet is a keyframe.
        if self.video_enabled {
            let replay = {
                let video = self
                    .video
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                match self.codec {
                    VideoCodec::H264 => {
                        let usable = !was_idle
                            && video.gop.front().is_some_and(|first| first.keyframe)
                            && video.gop.len() <= MAX_GOP_REPLAY_FRAMES
                            && video.gop_bytes <= MAX_GOP_REPLAY_BYTES;
                        usable.then(|| video.gop.iter().cloned().collect::<Vec<_>>())
                    }
                    _ => video.last_frame.clone().map(|frame| vec![frame]),
                }
            };
            match replay {
                Some(packets) if !packets.is_empty() => {
                    for packet in packets {
                        shared.push_video(&packet, true);
                    }
                }
                _ => {
                    shared.set_awaiting_keyframe();
                    self.request_keyframe_now();
                }
            }
        }
        Ok(Viewer::new(self.clone(), shared))
    }

    pub(crate) fn detach(&self, viewer_id: u64) {
        let now_idle = {
            let mut viewers = self
                .viewers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            viewers.retain(|viewer| viewer.id != viewer_id);
            viewers.is_empty()
        };
        // Nobody is watching: stop capturing and encoding until a socket
        // attaches again (the session itself lives until its ticket expires).
        if now_idle && !self.capture_paused.swap(true, Ordering::AcqRel) {
            self.set_capture_paused(true);
        }
    }

    fn set_capture_paused(&self, paused: bool) {
        if let Some(lease) = self
            .capture
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            lease.set_paused(paused);
        }
    }

    /// Ask the encoder for an IDR (used by `RequestKeyframe`, attach and
    /// viewer recovery).
    pub fn request_keyframe_now(&self) {
        {
            let mut video = self
                .video
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            video.keyframe_requests += 1;
        }
        *self
            .last_keyframe_request
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Instant::now());
        if let Some(lease) = self
            .capture
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            lease.request_keyframe();
        }
    }

    /// Idle refresh: when an H.264 session has sent nothing for its idle
    /// interval, ask the capture for an IDR of the current picture (the
    /// encoder re-encodes its last frame when nothing new arrives). At most
    /// one per interval; not counted as a client keyframe request.
    pub(crate) fn refresh_if_idle(&self) {
        let Some(every) = self.idle_keyframe else {
            return;
        };
        if !self.video_enabled || self.codec != VideoCodec::H264 {
            return;
        }
        let quiet = {
            let video = self
                .video
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            video.suspended.is_none() && video.last_frame_at.is_some_and(|at| at.elapsed() >= every)
        };
        if !quiet {
            return;
        }
        {
            let mut last = self
                .last_idle_keyframe
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if last.is_some_and(|at| at.elapsed() < every) {
                return;
            }
            *last = Some(Instant::now());
        }
        {
            let mut video = self
                .video
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            video.idle_keyframes += 1;
        }
        tracing::debug!(target: "cua_spacesd_client::media", session = %self.id, "idle refresh keyframe");
        if let Some(lease) = self
            .capture
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            lease.request_keyframe();
        }
    }

    /// Rate-limited keyframe request on behalf of one struggling viewer: at
    /// most one per second per session, so one bad client cannot flood a
    /// shared stream with IDRs.
    pub(crate) fn request_keyframe_limited(&self) {
        let recent = self
            .last_keyframe_request
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some_and(|at| at.elapsed() < Duration::from_secs(1));
        if !recent {
            self.request_keyframe_now();
        }
    }

    pub(crate) fn broadcast_control(&self, message: ServerMessage) {
        let viewers = self
            .viewers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        for viewer in viewers {
            viewer.push_control(message.clone());
        }
    }

    fn broadcast_lifecycle(&self, event: WindowLifecycleEvent) {
        self.broadcast_control(ServerMessage::Lifecycle {
            session_id: WindowSessionId(self.id.clone()),
            event,
        });
    }

    fn on_capture_event(self: &Arc<Self>, generation: u64, event: CaptureEvent) {
        match event {
            CaptureEvent::Frame(frame) => self.on_frame(generation, frame),
            CaptureEvent::GeometryChanged(geometry) => {
                let changed = {
                    let mut video = self
                        .video
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if video.geometry.scale_factor != geometry.scale_factor {
                        video.geometry.scale_factor = geometry.scale_factor;
                        video.geometry_epoch += 1;
                        Some((video.geometry_epoch, video.geometry.clone()))
                    } else {
                        None
                    }
                };
                if let Some((epoch, geometry)) = changed {
                    self.broadcast_lifecycle(WindowLifecycleEvent::GeometryChanged {
                        geometry_epoch: GeometryEpoch(epoch),
                        geometry,
                    });
                }
            }
            CaptureEvent::TitleChanged(title) => {
                if matches!(self.target_ref, MediaTargetRef::Window { .. }) {
                    self.broadcast_lifecycle(WindowLifecycleEvent::TitleChanged { title });
                }
            }
            CaptureEvent::Suspended(reason) => {
                self.video
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .suspended = Some(reason.clone());
                let reason = match reason.as_str() {
                    "minimized" if !self.target_ref.is_display() => SuspensionReason::Minimized,
                    "consent_required" => SuspensionReason::ConsentRequired,
                    "consent_revoked" => SuspensionReason::ConsentRevoked,
                    "window_unavailable" => SuspensionReason::WindowUnavailable,
                    "occluded" => SuspensionReason::OccludedNoFrames,
                    _ => SuspensionReason::CaptureFailed,
                };
                self.broadcast_lifecycle(WindowLifecycleEvent::Suspended { reason });
            }
            CaptureEvent::Resumed => {
                self.video
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .suspended = None;
                self.broadcast_lifecycle(WindowLifecycleEvent::Resumed);
            }
            CaptureEvent::Closed => {
                self.broadcast_lifecycle(WindowLifecycleEvent::Closed);
                if let Some(runtime) = self.runtime.upgrade() {
                    runtime.close(&self.id, v2::close_code::TARGET_GONE);
                } else {
                    self.shutdown(v2::close_code::TARGET_GONE);
                }
            }
        }
    }

    fn on_frame(self: &Arc<Self>, generation: u64, frame: OwnedFrame) {
        if frame.validate().is_err() {
            tracing::warn!(target: "cua_spacesd_client::media", session = %self.id, "dropping invalid frame");
            return;
        }
        let (packet, geometry_change) = {
            let mut video = self
                .video
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if generation != video.capture_generation {
                return;
            }
            // Codec epochs: monotonic across capture restarts.
            let provider_epoch = (generation, frame.codec_epoch);
            if video.provider_epoch != Some(provider_epoch) {
                if frame.format == PixelFormat::H264AnnexB && !frame.keyframe {
                    // A new decoder configuration must start with an IDR.
                    return;
                }
                video.provider_epoch = Some(provider_epoch);
                video.wire_codec_epoch += 1;
                video.gop.clear();
                video.gop_bytes = 0;
            }
            let mut geometry_change = None;
            if video.geometry.width_px != frame.width_px
                || video.geometry.height_px != frame.height_px
            {
                video.geometry.width_px = frame.width_px;
                video.geometry.height_px = frame.height_px;
                if video.first_frame {
                    video.geometry_epoch += 1;
                    geometry_change = Some((video.geometry_epoch, video.geometry.clone()));
                }
            }
            video.first_frame = true;
            video.suspended = None;
            let sequence = video.next_sequence;
            video.next_sequence += 1;
            let codec = match frame.format {
                PixelFormat::H264AnnexB => VideoCodec::H264,
                PixelFormat::Png => VideoCodec::Png,
                PixelFormat::Bgra8 => VideoCodec::Bgra,
            };
            let packet = Arc::new(VideoPacket {
                descriptor: VideoFrameDescriptor {
                    session_id: WindowSessionId(self.id.clone()),
                    sequence: FrameSequence(sequence),
                    geometry_epoch: GeometryEpoch(video.geometry_epoch),
                    codec_epoch: CodecEpoch(video.wire_codec_epoch),
                    width_px: frame.width_px,
                    height_px: frame.height_px,
                    capture_timestamp_us: frame.capture_timestamp_us,
                    encode_duration_us: frame.encode_duration_us,
                    codec,
                    keyframe: frame.keyframe,
                },
                payload: frame.bytes.clone(),
                keyframe: frame.keyframe,
                captured_at: Instant::now(),
            });
            video.frames_emitted += 1;
            video.bytes_emitted += frame.bytes.len() as u64;
            video.last_frame_at = Some(Instant::now());
            if packet.keyframe {
                video.gop.clear();
                video.gop_bytes = 0;
            }
            if frame.format == PixelFormat::H264AnnexB {
                video.gop_bytes += packet.payload.len();
                video.gop.push_back(packet.clone());
                // Bounded: past the replay limits the cache is useless (attach
                // forces an IDR instead), so keep it no larger than twice them.
                while video.gop.len() > MAX_GOP_REPLAY_FRAMES * 2
                    || video.gop_bytes > MAX_GOP_REPLAY_BYTES * 2
                {
                    match video.gop.pop_front() {
                        Some(dropped) => {
                            video.gop_bytes = video.gop_bytes.saturating_sub(dropped.payload.len())
                        }
                        None => break,
                    }
                }
            } else {
                video.last_frame = Some(packet.clone());
            }
            (packet, geometry_change)
        };
        {
            let (lock, signal) = &self.first_frame_signal;
            let mut seen = lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !*seen {
                *seen = true;
                signal.notify_all();
            }
        }
        if let Some((epoch, geometry)) = geometry_change {
            self.broadcast_lifecycle(WindowLifecycleEvent::GeometryChanged {
                geometry_epoch: GeometryEpoch(epoch),
                geometry,
            });
        }
        let viewers = self
            .viewers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let mut needs_keyframe = false;
        for viewer in viewers {
            needs_keyframe |= viewer.push_video(&packet, false);
        }
        if needs_keyframe {
            self.request_keyframe_limited();
        }
    }

    /// Apply new stream preferences (from the socket or gRPC). Zero keeps a
    /// value. Returns the effective values.
    pub fn set_preferences(
        self: &Arc<Self>,
        max_fps: u16,
        max_dimension: u32,
        bitrate_kbps: u32,
    ) -> Result<(u16, u32, u32), MediaError> {
        let old_dimension = self.max_dimension.load(Ordering::Relaxed);
        if max_fps > 0 {
            self.max_fps.store(max_fps.min(240), Ordering::Relaxed);
        }
        if bitrate_kbps > 0 {
            self.bitrate_kbps
                .store(bitrate_kbps.clamp(250, 100_000), Ordering::Relaxed);
        }
        if max_dimension > 0 {
            self.max_dimension.store(max_dimension, Ordering::Relaxed);
        }
        let fps = self.max_fps.load(Ordering::Relaxed);
        let kbps = self.bitrate_kbps.load(Ordering::Relaxed);
        let dimension = self.max_dimension.load(Ordering::Relaxed);
        self.rate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_targets(kbps, fps);
        self.video
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .preference_updates += 1;
        if self.video_enabled {
            if max_dimension > 0 && max_dimension != old_dimension {
                // Size changes need a new capture configuration.
                let formats = self
                    .providers()
                    .and_then(|providers| providers.captures.formats(&self.target.id).ok())
                    .unwrap_or_default();
                self.start_capture(&formats)?;
            } else if let Some(lease) = self
                .capture
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
            {
                lease.set_max_fps(fps);
                lease.set_target_bitrate_kbps(kbps);
            }
        }
        Ok((fps, dimension, kbps))
    }

    fn start_audio(
        self: &Arc<Self>,
        request: &AudioRequest,
        quic: bool,
        target: &ProviderTarget,
    ) -> Result<Option<NegotiatedAudioInfo>, MediaError> {
        let Some(provider) = self.providers().and_then(|providers| providers.audio) else {
            return Ok(None);
        };
        let mut config = request.config.clone();
        let codecs = provider.codecs();
        if !codecs.contains(&config.codec) {
            config.codec = *codecs.first().ok_or_else(|| {
                MediaError::FailedPrecondition("the audio provider offers no codec".into())
            })?;
        }
        let mut config = normalise_config(config);
        if quic && config.codec == AudioCodecName::Opus {
            config.bitrate_kbps = config.bitrate_kbps.min(quic_bitrate_cap(config.frame_ms));
        }
        let sources = provider.sources();
        let mut selected = Vec::new();
        if !request.downlink {
            // Uplink only.
        } else if request.source_ids.is_empty() {
            let pid = match &self.target_ref {
                MediaTargetRef::Window { .. } => window_pid(target),
                MediaTargetRef::Display { .. } => None,
            };
            if let Some(source) = provider.source_for_pid(pid) {
                selected.push(source);
            }
        } else {
            for source_id in &request.source_ids {
                let source = sources
                    .iter()
                    .find(|source| &source.source_id == source_id)
                    .cloned()
                    .ok_or_else(|| {
                        MediaError::NotFound(format!("unknown audio source {source_id}"))
                    })?;
                selected.push(source);
            }
        }
        let mut negotiated = Vec::new();
        for (index, source) in selected.into_iter().enumerate() {
            let track_id = u16::try_from(index + 1).unwrap_or(u16::MAX);
            let track_config = AudioTrackConfig {
                source_id: source.source_id.clone(),
                ..config.clone()
            };
            let track = Arc::new(AudioTrackRuntime {
                track_id,
                source: source.clone(),
                config: Mutex::new(track_config.clone()),
                config_epoch: AtomicU8::new(1),
                next_sequence: AtomicU32::new(random_u64() as u32),
                lease: Mutex::new(None),
                paused: AtomicBool::new(false),
                state: Mutex::new(AudioTrackStateKind::Active),
                packets: AtomicU64::new(0),
                bytes: AtomicU64::new(0),
                opus_pre_skip: AtomicU16::new(0),
            });
            let sink = Arc::new(SessionAudioSink {
                session: Arc::downgrade(self),
                track: track.clone(),
            });
            let lease = provider.start(&track_config, sink)?;
            track
                .opus_pre_skip
                .store(lease.opus_pre_skip(), Ordering::Relaxed);
            *track.lease.lock().unwrap() = Some(lease);
            self.audio_tracks.lock().unwrap().push(track);
            negotiated.push(NegotiatedTrack { track_id, source });
        }
        let uplink = match request.uplink.as_ref() {
            None => None,
            Some(uplink) => {
                let track_id = u16::try_from(negotiated.len() + 1).unwrap_or(u16::MAX);
                let uplink_config = normalise_config(AudioTrackConfig {
                    source_id: uplink.virtual_source_name.clone(),
                    ..uplink.config.clone()
                });
                let denied = |reason: String| UplinkGrant {
                    granted: false,
                    denied_reason: Some(reason),
                    track_id: 0,
                    config: uplink_config.clone(),
                    virtual_source_name: uplink.virtual_source_name.clone(),
                };
                let grant = if let Err(reason) = &uplink.permitted {
                    denied(reason.clone())
                } else if self.policy == SessionPolicy::ViewOnly {
                    denied("view_only session".into())
                } else {
                    match provider.open_uplink(&uplink_config, &uplink.virtual_source_name) {
                        Ok(Some(sink)) => {
                            let grant = UplinkGrant {
                                granted: true,
                                denied_reason: None,
                                track_id,
                                config: uplink_config.clone(),
                                virtual_source_name: uplink.virtual_source_name.clone(),
                            };
                            *self.uplink.lock().unwrap() = Some(Arc::new(UplinkRuntime {
                                grant: grant.clone(),
                                sink,
                                muted: AtomicBool::new(false),
                                last_sequence: Mutex::new(None),
                                packets: AtomicU64::new(0),
                                lost: AtomicU64::new(0),
                                late: AtomicU64::new(0),
                            }));
                            grant
                        }
                        Ok(None) => denied("no virtual audio device".into()),
                        Err(error) => denied(error.message),
                    }
                };
                Some(grant)
            }
        };
        Ok(Some(NegotiatedAudioInfo {
            tracks: negotiated,
            config,
            uplink,
        }))
    }

    fn on_audio_frame(&self, track: &AudioTrackRuntime, frame: AudioFrame) {
        if track.paused.load(Ordering::Relaxed) || self.is_closed() {
            return;
        }
        let (sample_rate, frame_ms) = {
            let config = track.config.lock().unwrap();
            (config.sample_rate_hz, config.frame_ms)
        };
        let sequence = track.next_sequence.fetch_add(1, Ordering::Relaxed);
        let packet = encode_audio_packet(
            &AudioPacketHeader {
                discontinuity: false,
                dtx: frame.dtx,
                track_id: track.track_id,
                sequence,
                pts_us: frame.pts_us,
                frame_samples: frame.frame_samples,
                config_epoch: track.config_epoch.load(Ordering::Relaxed),
            },
            &frame.payload,
        );
        track.packets.fetch_add(1, Ordering::Relaxed);
        track
            .bytes
            .fetch_add(packet.len() as u64, Ordering::Relaxed);
        let duration_us = if sample_rate > 0 {
            u64::from(frame.frame_samples) * 1_000_000 / u64::from(sample_rate)
        } else {
            u64::from(frame_ms) * 1_000
        };
        let packet: Arc<[u8]> = Arc::from(packet);
        let viewers = self
            .viewers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        for viewer in viewers {
            viewer.push_audio(track.track_id, packet.clone(), duration_us);
        }
    }

    /// Change audio settings (from `SetPreferences`). Frame-size changes
    /// bump the track's config epoch and re-send `audio_config`.
    pub fn set_audio_preferences(&self, update: AudioPreferenceUpdate) -> Option<AudioTrackConfig> {
        let tracks = self.audio_tracks.lock().unwrap().clone();
        let mut effective = None;
        for track in tracks {
            if let Some(enabled) = update.enabled {
                track.paused.store(!enabled, Ordering::Relaxed);
                if let Some(lease) = track.lease.lock().unwrap().as_ref() {
                    lease.set_paused(!enabled);
                }
                let state = if enabled {
                    AudioTrackStateKind::Active
                } else {
                    AudioTrackStateKind::Paused
                };
                *track.state.lock().unwrap() = state;
                self.broadcast_control(ServerMessage::AudioTrackState(AudioTrackState {
                    track_id: track.track_id,
                    state,
                    reason: None,
                }));
            }
            let mut config = track.config.lock().unwrap().clone();
            let before = config.clone();
            if update.bitrate_kbps > 0 {
                config.bitrate_kbps = update.bitrate_kbps;
            }
            if let Some(fec) = update.fec {
                config.fec = fec;
            }
            if let Some(dtx) = update.dtx {
                config.dtx = dtx;
            }
            if update.frame_ms > 0 {
                config.frame_ms = update.frame_ms;
            }
            if let Some(loss) = update.expected_loss_percent {
                config.expected_loss_percent = loss;
            }
            let config = normalise_config(config);
            if config != before {
                let applied = track
                    .lease
                    .lock()
                    .unwrap()
                    .as_ref()
                    .map(|lease| lease.reconfigure(&config).is_ok())
                    .unwrap_or(false);
                if applied {
                    *track.config.lock().unwrap() = config.clone();
                    if config.frame_ms != before.frame_ms || config.codec != before.codec {
                        track.config_epoch.fetch_add(1, Ordering::Relaxed);
                    }
                    self.broadcast_control(ServerMessage::AudioConfig(track_audio_config(
                        &track,
                        AudioDirection::Down,
                    )));
                }
            }
            effective = Some(track.config.lock().unwrap().clone());
        }
        if let Some(muted) = update.uplink_muted {
            if let Some(uplink) = self.uplink.lock().unwrap().as_ref() {
                uplink.muted.store(muted, Ordering::Relaxed);
                uplink.sink.set_muted(muted);
            }
        }
        effective
    }

    pub fn audio_enabled(&self) -> bool {
        self.audio_tracks
            .lock()
            .unwrap()
            .iter()
            .any(|track| !track.paused.load(Ordering::Relaxed))
    }

    pub fn uplink_muted(&self) -> bool {
        self.uplink
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|uplink| uplink.muted.load(Ordering::Relaxed))
    }

    pub(crate) fn audio_stats(&self) -> Vec<AudioTrackStats> {
        let mut stats: Vec<AudioTrackStats> = self
            .audio_tracks
            .lock()
            .unwrap()
            .iter()
            .map(|track| AudioTrackStats {
                track_id: track.track_id,
                direction: Some(AudioDirection::Down),
                packets: track.packets.load(Ordering::Relaxed),
                bytes: track.bytes.load(Ordering::Relaxed),
                ..AudioTrackStats::default()
            })
            .collect();
        if let Some(uplink) = self.uplink.lock().unwrap().as_ref() {
            stats.push(AudioTrackStats {
                track_id: uplink.grant.track_id,
                direction: Some(AudioDirection::Up),
                packets: uplink.packets.load(Ordering::Relaxed),
                packets_lost: uplink.lost.load(Ordering::Relaxed),
                packets_late: uplink.late.load(Ordering::Relaxed),
                ..AudioTrackStats::default()
            });
        }
        stats
    }

    /// Stop capture and close every attached socket.
    pub(crate) fn shutdown(&self, code: u16) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        self.capture_generation.fetch_add(1, Ordering::AcqRel);
        if let Some(lease) = self
            .capture
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            lease.stop();
        }
        for track in self.audio_tracks.lock().unwrap().iter() {
            if let Some(lease) = track.lease.lock().unwrap().take() {
                lease.stop();
            }
        }
        if let Some(uplink) = self.uplink.lock().unwrap().take() {
            uplink.sink.close();
        }
        if let Some(input) = self.input.lock().unwrap().take() {
            input.release_all();
        }
        let viewers = std::mem::take(
            &mut *self
                .viewers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        for viewer in viewers {
            viewer.close(code);
        }
        let (lock, signal) = &self.first_frame_signal;
        *lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        signal.notify_all();
    }

    fn spawn_maintenance(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        handle.spawn(async move {
            let mut interval = tokio::time::interval(TICK);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut last_rate = Instant::now();
            let mut last_frames = 0u64;
            loop {
                interval.tick().await;
                let Some(session) = weak.upgrade() else {
                    return;
                };
                if session.is_closed() {
                    return;
                }
                let viewers = session
                    .viewers
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                let mut needs_keyframe = false;
                for viewer in &viewers {
                    needs_keyframe |= viewer.tick();
                }
                if needs_keyframe {
                    session.request_keyframe_limited();
                } else if !viewers.is_empty() {
                    session.refresh_if_idle();
                }
                if viewers.is_empty() && Instant::now() > session.attach_deadline {
                    tracing::debug!(target: "cua_spacesd_client::media", session = %session.id, "reaping unattached session");
                    if let Some(runtime) = session.runtime.upgrade() {
                        runtime.close(&session.id, v2::close_code::SESSION_CLOSED);
                    }
                    return;
                }
                if last_rate.elapsed() >= RATE_WINDOW && session.codec == VideoCodec::H264 {
                    last_rate = Instant::now();
                    let frames = session.video.lock().unwrap().frames_emitted;
                    let mut sample = CongestionSample {
                        active: frames != last_frames,
                        ..CongestionSample::default()
                    };
                    last_frames = frames;
                    for viewer in &viewers {
                        let signal = viewer.take_congestion();
                        sample.queue_delay_ms = sample.queue_delay_ms.max(signal.queue_delay_ms);
                        sample.ack_excess_ms = sample.ack_excess_ms.max(signal.ack_excess_ms);
                        sample.dropped_frames += signal.dropped_frames;
                    }
                    let decision = session.rate.lock().unwrap().on_window(sample);
                    if let Some(decision) = decision {
                        tracing::debug!(
                            target: "cua_spacesd_client::media",
                            session = %session.id,
                            bitrate_kbps = decision.bitrate_kbps,
                            max_fps = decision.max_fps,
                            "rate control"
                        );
                        if let Some(lease) = session.capture.lock().unwrap().as_ref() {
                            lease.set_target_bitrate_kbps(decision.bitrate_kbps);
                            lease.set_max_fps(decision.max_fps);
                        }
                    }
                }
            }
        });
    }

    pub(crate) fn rate_decision(&self) -> super::rate::RateDecision {
        self.rate.lock().unwrap().current()
    }

    /// Open (once) the native interactive input lease for this session.
    pub(crate) fn input_lease(
        &self,
    ) -> Result<Option<Arc<dyn InteractiveInputLease>>, ProviderError> {
        let mut input = self.input.lock().unwrap();
        if let Some(lease) = input.as_ref() {
            return Ok(Some(lease.clone()));
        }
        if self.policy == SessionPolicy::ViewOnly {
            return Ok(None);
        }
        let providers = self.providers().ok_or_else(|| {
            ProviderError::new(ProviderErrorCode::Internal, "driver is shutting down")
        })?;
        let principal = self.principal();
        let owner = cua_spacesd_provider_api::InputOwner {
            id: principal.id.clone(),
            name: principal.name.clone(),
            agent: principal.agent,
        };
        let lease = providers
            .inputs
            .open_as(&self.target.id, self.policy, &owner)?;
        *input = lease.clone();
        Ok(lease)
    }
}

/// Audio settings from `SetPreferences`; zero/None keeps a value.
#[derive(Debug, Clone, Default)]
pub struct AudioPreferenceUpdate {
    pub enabled: Option<bool>,
    pub bitrate_kbps: u32,
    pub fec: Option<bool>,
    pub dtx: Option<bool>,
    pub frame_ms: u16,
    pub expected_loss_percent: Option<u8>,
    pub uplink_muted: Option<bool>,
}

fn track_audio_config(track: &AudioTrackRuntime, direction: AudioDirection) -> AudioConfig {
    let config = track.config.lock().unwrap();
    AudioConfig {
        track_id: track.track_id,
        config_epoch: track.config_epoch.load(Ordering::Relaxed),
        direction,
        codec: config.codec,
        sample_rate_hz: config.sample_rate_hz,
        channels: config.channels,
        frame_ms: config.frame_ms,
        bitrate_kbps: config.bitrate_kbps,
        fec: config.fec,
        dtx: config.dtx,
        opus_pre_skip: track.opus_pre_skip.load(Ordering::Relaxed),
        source: AudioSourceRef {
            source_id: track.source.source_id.clone(),
            kind: track.source.kind.as_str().into(),
            desktop_fallback: track.source.desktop_fallback,
        },
    }
}

fn window_pid(_target: &ProviderTarget) -> Option<u32> {
    // Native identity stays inside the provider; per-app pairing is resolved
    // by the audio provider from the window's application instead.
    None
}

fn session_capabilities(policy: SessionPolicy) -> Vec<String> {
    let mut capabilities = vec![
        v2::capability::KEYFRAME_ON_ATTACH.to_owned(),
        v2::capability::STATS.to_owned(),
        v2::capability::FRAME_ACK.to_owned(),
    ];
    if policy != SessionPolicy::ViewOnly {
        capabilities.push(v2::capability::INPUT_INTERACTIVE.to_owned());
    }
    capabilities
}

pub(crate) fn hello_capabilities(audio: bool) -> Vec<String> {
    let mut capabilities = vec![
        v2::capability::INPUT_INTERACTIVE.to_owned(),
        v2::capability::DESKTOP.to_owned(),
        v2::capability::KEYFRAME_ON_ATTACH.to_owned(),
        v2::capability::FRAME_ACK.to_owned(),
        v2::capability::STATS.to_owned(),
    ];
    if audio {
        capabilities.push(v2::capability::AUDIO.to_owned());
    }
    capabilities
}

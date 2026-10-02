// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Transport-neutral protocol types for streaming and controlling one window.
//!
//! This crate deliberately contains no capture buffers, operating-system
//! handles, codecs, sockets, or runtime state. A local in-process consumer and
//! a future WebRTC or QUIC binding use the same lifecycle and action semantics.

use serde::{Deserialize, Serialize};
pub use serde_json::Value;

pub mod presence;
pub mod v2;

pub const PROTOCOL_NAME: &str = "cua-media";
pub const PROTOCOL_VERSION: u16 = 1;
pub const MAX_INTERACTIVE_INPUT_EVENTS: usize = 256;
pub const MAX_INTERACTIVE_INPUT_TEXT_BYTES: usize = 64 * 1024;
pub const MAX_CLIPBOARD_TEXT_BYTES: usize = 1024 * 1024;
pub const MAX_CLIPBOARD_FILES: usize = 16;
pub const MAX_CLIPBOARD_FILE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_CLIPBOARD_FILES_BYTES: usize = 32 * 1024 * 1024;

pub fn build_revision() -> &'static str {
    option_env!("CUA_ENV_BUILD_REVISION").unwrap_or(env!("CARGO_PKG_VERSION"))
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TargetHandle(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TargetEpoch(pub u64);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TargetGrant(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WindowSessionId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GeometryEpoch(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FrameSequence(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CodecEpoch(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AccessibilitySnapshotId(pub u32);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SurfaceGeometry {
    pub width_px: u32,
    pub height_px: u32,
    pub scale_factor: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowDescriptor {
    pub window: TargetHandle,
    pub target_epoch: TargetEpoch,
    pub app_name: String,
    pub title: String,
    pub geometry: SurfaceGeometry,
    pub visible: bool,
}

/// Metadata for one application icon transferred as a binary protocol asset.
/// The bytes travel in the packet payload rather than being repeated or
/// base64-encoded inside every window descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppIconDescriptor {
    pub window: TargetHandle,
    pub target_epoch: TargetEpoch,
    pub media_type: String,
    pub byte_len: u64,
}

/// One regular file carried by the clipboard. Only a leaf filename crosses
/// the wire; client and host filesystem paths are never exposed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipboardFile {
    pub name: String,
    pub offset: u64,
    pub byte_len: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoCodec {
    /// Packed, top-down BGRA fallback. Remote peers should prefer H.264.
    Bgra,
    Png,
    H264,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub protocol_name: String,
    pub protocol_versions: Vec<u16>,
    /// Extensible dotted capability names. Unknown names must be ignored.
    pub capabilities: Vec<String>,
    /// Source revision for local dogfood correlation. Older peers omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_revision: Option<String>,
}

impl Default for Hello {
    fn default() -> Self {
        Self {
            protocol_name: PROTOCOL_NAME.to_owned(),
            protocol_versions: vec![PROTOCOL_VERSION],
            capabilities: Vec::new(),
            build_revision: Some(build_revision().into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenSession {
    pub window: TargetHandle,
    pub target_epoch: TargetEpoch,
    /// Ordered codecs the client can decode. The server chooses one and
    /// reports it in `SessionOpened`; every frame in the session uses it.
    pub accepted_codecs: Vec<VideoCodec>,
    /// Initial stream preferences. The server reports the effective values in
    /// `SessionOpened`; clients may revise them at runtime.
    pub max_fps: u16,
    pub max_dimension: u32,
    /// Requested H.264 target bitrate. Older clients omit it and providers
    /// retain their resolution/FPS-derived default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_bitrate_kbps: Option<u32>,
    pub policy: SessionPolicy,
    /// Whether this session may resize the shared host window. Observe-only is
    /// the compatibility default; bidirectional control must be explicitly
    /// requested and granted by the server.
    #[serde(
        default,
        skip_serializing_if = "WindowGeometryControl::is_observe_only"
    )]
    pub geometry_control: WindowGeometryControl,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamPreferences {
    pub session_id: WindowSessionId,
    pub max_fps: u16,
    pub max_dimension: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_bitrate_kbps: Option<u32>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowGeometryControl {
    #[default]
    ObserveOnly,
    Bidirectional,
    #[serde(other)]
    Unknown,
}

impl WindowGeometryControl {
    fn is_observe_only(&self) -> bool {
        *self == Self::ObserveOnly
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowGeometryRequest {
    pub session_id: WindowSessionId,
    /// Strictly increasing within the session. The server rejects stale or
    /// duplicate revisions so delayed resize events cannot roll geometry back.
    pub revision: u64,
    /// Desired host-window size in host logical points.
    pub width_points: u32,
    pub height_points: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowGeometryResult {
    pub session_id: WindowSessionId,
    pub revision: u64,
    pub applied: bool,
    /// Actual host-window size after provider clamping, when known.
    pub width_points: u32,
    pub height_points: u32,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionOpened {
    pub session_id: WindowSessionId,
    pub target_epoch: TargetEpoch,
    pub geometry_epoch: GeometryEpoch,
    pub codec_epoch: CodecEpoch,
    pub geometry: SurfaceGeometry,
    pub codec: VideoCodec,
    pub max_fps: u16,
    pub max_dimension: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_bitrate_kbps: Option<u32>,
    pub capabilities: Vec<String>,
    pub action_capabilities: Vec<ActionCapability>,
    pub policy: SessionPolicy,
    #[serde(
        default,
        skip_serializing_if = "WindowGeometryControl::is_observe_only"
    )]
    pub geometry_control: WindowGeometryControl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionPolicy {
    ViewOnly,
    BackgroundOnly,
    AllowActivation,
}

impl SessionPolicy {
    /// Returns whether this server-side policy ceiling admits `requested`.
    pub fn allows(self, requested: Self) -> bool {
        fn rank(policy: SessionPolicy) -> u8 {
            match policy {
                SessionPolicy::ViewOnly => 0,
                SessionPolicy::BackgroundOnly => 1,
                SessionPolicy::AllowActivation => 2,
            }
        }

        rank(requested) <= rank(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionDeliveryGuarantee {
    Background,
    MayActivate,
    Unsupported,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionCapability {
    pub action: String,
    pub guarantee: ActionDeliveryGuarantee,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuspensionReason {
    Minimized,
    OccludedNoFrames,
    ConsentRequired,
    ConsentRevoked,
    CaptureFailed,
    WindowUnavailable,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WindowLifecycleEvent {
    GeometryChanged {
        geometry_epoch: GeometryEpoch,
        geometry: SurfaceGeometry,
    },
    TitleChanged {
        title: String,
    },
    Suspended {
        reason: SuspensionReason,
    },
    Resumed,
    Closed,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoFrameDescriptor {
    pub session_id: WindowSessionId,
    pub sequence: FrameSequence,
    pub geometry_epoch: GeometryEpoch,
    /// Encoder configuration generation. A change means decoder state must be
    /// reset before consuming the next keyframe.
    pub codec_epoch: CodecEpoch,
    /// Encoded or packed payload dimensions. BGRA is always tightly packed,
    /// top-down, with a row stride of `width_px * 4`.
    pub width_px: u32,
    pub height_px: u32,
    /// Source-monotonic capture time in microseconds. Its origin is local to
    /// the capture session; `sequence` remains the authoritative ordering.
    pub capture_timestamp_us: u64,
    /// Host encoder submission-to-output duration when the provider can
    /// measure it on one monotonic clock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encode_duration_us: Option<u32>,
    pub codec: VideoCodec,
    pub keyframe: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowState {
    pub session_id: WindowSessionId,
    pub snapshot_id: AccessibilitySnapshotId,
    pub state: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActionBasis {
    Pixel {
        geometry_epoch: GeometryEpoch,
        frame_sequence: FrameSequence,
    },
    Accessibility {
        snapshot_id: AccessibilitySnapshotId,
    },
    None,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionRequest {
    pub action_id: String,
    pub session_id: WindowSessionId,
    /// Adapter-defined action name, such as `click` or `type_text`.
    pub tool: String,
    /// Adapter-defined arguments. Platform-neutral semantics stay canonical
    /// in the action provider rather than being reimplemented in a transport.
    pub arguments: Value,
    pub basis: ActionBasis,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionErrorCode {
    StaleTarget,
    StaleGeometry,
    StaleAccessibilitySnapshot,
    ViewOnly,
    WouldRequireActivation,
    NativeTargetRejected,
    Unsupported,
    PermissionDenied,
    WindowUnavailable,
    RateLimited,
    DeliveryFailed,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionError {
    pub code: ActionErrorCode,
    pub message: String,
    pub current_geometry_epoch: Option<GeometryEpoch>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionResult {
    pub action_id: String,
    pub delivered: bool,
    pub error: Option<ActionError>,
    /// First frame captured after dispatch when it was already available at
    /// delivery time. Otherwise `ActionFrameCorrelation` follows
    /// asynchronously. This is an ordering marker, not a visible-change claim.
    pub first_frame_sequence_after: Option<FrameSequence>,
}

/// Completes action-to-frame ordering without delaying `ActionResult`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionFrameCorrelation {
    pub action_id: String,
    pub session_id: WindowSessionId,
    /// First frame captured after dispatch, or `None` when the bounded
    /// correlation deadline elapsed without a frame.
    pub first_frame_sequence_after: Option<FrameSequence>,
}

/// A modifier snapshot accompanying one physical input event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputModifier {
    Command,
    Shift,
    Option,
    Control,
    Function,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputKeyState {
    Down,
    Up,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputPointerButton {
    Left,
    Right,
    Middle,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputPointerPhase {
    Move,
    Down,
    Up,
    Cancel,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputGesturePhase {
    None,
    MayBegin,
    Began,
    Changed,
    Ended,
    Cancelled,
    #[serde(other)]
    Unknown,
}

/// One device-semantic input event. Pointer coordinates are normalized to the
/// current streamed window, keeping native geometry and identifiers private.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InteractiveInputEvent {
    TextCommit {
        text: String,
    },
    Key {
        key: String,
        state: InputKeyState,
        modifiers: Vec<InputModifier>,
        repeat: bool,
    },
    Pointer {
        phase: InputPointerPhase,
        button: Option<InputPointerButton>,
        x_normalized: f64,
        y_normalized: f64,
        modifiers: Vec<InputModifier>,
    },
    Scroll {
        x_normalized: f64,
        y_normalized: f64,
        delta_x: f64,
        delta_y: f64,
        phase: InputGesturePhase,
        momentum_phase: InputGesturePhase,
        precise: bool,
    },
}

/// A contiguous input sequence. Small batches amortize framing and JSON costs
/// without replacing live pointer/scroll samples with automation gestures.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InteractiveInputBatch {
    pub session_id: WindowSessionId,
    pub first_sequence: u64,
    pub events: Vec<InteractiveInputEvent>,
}

impl InteractiveInputBatch {
    /// Validate transport-level bounds and return the batch's final sequence.
    pub fn validate(&self) -> Result<u64, String> {
        if self.events.is_empty() {
            return Err("interactive input batch must not be empty".into());
        }
        if self.events.len() > MAX_INTERACTIVE_INPUT_EVENTS {
            return Err(format!(
                "interactive input batch exceeds {MAX_INTERACTIVE_INPUT_EVENTS} events"
            ));
        }
        let through_sequence = self
            .first_sequence
            .checked_add(self.events.len() as u64 - 1)
            .ok_or_else(|| "interactive input sequence overflow".to_owned())?;
        let mut text_bytes = 0usize;
        for event in &self.events {
            match event {
                InteractiveInputEvent::TextCommit { text } => {
                    if text.is_empty() {
                        return Err("text commits must not be empty".into());
                    }
                    text_bytes = text_bytes
                        .checked_add(text.len())
                        .ok_or_else(|| "interactive input text size overflow".to_owned())?;
                }
                InteractiveInputEvent::Key {
                    key,
                    state,
                    modifiers,
                    ..
                } => {
                    if key.is_empty() {
                        return Err("input key names must not be empty".into());
                    }
                    if *state == InputKeyState::Unknown
                        || modifiers.contains(&InputModifier::Unknown)
                    {
                        return Err("unknown key semantics are not dispatchable".into());
                    }
                }
                InteractiveInputEvent::Pointer {
                    phase,
                    button,
                    x_normalized,
                    y_normalized,
                    modifiers,
                } => {
                    if *phase == InputPointerPhase::Unknown
                        || button == &Some(InputPointerButton::Unknown)
                        || modifiers.contains(&InputModifier::Unknown)
                    {
                        return Err("unknown pointer semantics are not dispatchable".into());
                    }
                    validate_normalized(*x_normalized, *y_normalized)?;
                }
                InteractiveInputEvent::Scroll {
                    x_normalized,
                    y_normalized,
                    delta_x,
                    delta_y,
                    phase,
                    momentum_phase,
                    ..
                } => {
                    validate_normalized(*x_normalized, *y_normalized)?;
                    if !delta_x.is_finite() || !delta_y.is_finite() {
                        return Err("scroll deltas must be finite".into());
                    }
                    if *phase == InputGesturePhase::Unknown
                        || *momentum_phase == InputGesturePhase::Unknown
                    {
                        return Err("unknown scroll semantics are not dispatchable".into());
                    }
                }
            }
        }
        if text_bytes > MAX_INTERACTIVE_INPUT_TEXT_BYTES {
            return Err(format!(
                "interactive input text exceeds {MAX_INTERACTIVE_INPUT_TEXT_BYTES} bytes"
            ));
        }
        Ok(through_sequence)
    }
}

fn validate_normalized(x: f64, y: f64) -> Result<(), String> {
    if x.is_finite() && y.is_finite() && (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y) {
        Ok(())
    } else {
        Err("input coordinates must be finite and within [0, 1]".into())
    }
}

/// Cumulative acknowledgement that the host native API accepted all events
/// through `through_sequence`. It does not claim that the app visibly changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InteractiveInputAcknowledgement {
    pub session_id: WindowSessionId,
    pub through_sequence: u64,
    pub delivered: bool,
    pub error: Option<ActionError>,
    /// Time spent in the provider/native dispatch path for this batch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_dispatch_us: Option<u64>,
}

/// One connected client identity for multi-user presence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresenceUser {
    pub user_id: String,
    pub name: String,
    /// `#RRGGBB` display color chosen by the server when the client does not
    /// supply one.
    pub color: String,
}

/// What the guest's operating system is currently drawing as the pointer.
///
/// This is deliberately a small closed vocabulary of *semantic* shapes plus a
/// `Custom` escape, not a bitmap-by-default. The viewer maps each variant onto
/// its own native cursor, so an I-beam stays crisp at any scale factor, follows
/// the viewer's own theme and accessibility size, and costs a few bytes on the
/// wire. Only a cursor with no portable equivalent needs to ship pixels.
///
/// ## Why metadata rather than baking the cursor into the video
///
/// The macOS capture path configures ScreenCaptureKit to exclude the cursor
/// (`stream_configuration`, `macos_capture.rs`), and that stays true. Baking
/// the pointer into captured pixels would:
///
/// - blur it, since frames are scaled to `max_dimension` and re-encoded lossily;
/// - make it un-hideable and un-styleable at the viewer;
/// - and, decisively for multi-participant sessions, render *one* pointer for
///   everyone. The origin desktop has exactly one physical cursor. Presence
///   cursors are per-participant overlays; a baked pointer cannot be.
///
/// So shape rides as metadata on [`CursorState`], next to the position it
/// belongs to.
// No `Eq`: the `Custom` variant carries f64 hotspot/scale.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CursorShape {
    /// The ordinary arrow.
    #[default]
    Default,
    /// Text insertion (`NSCursor.iBeam`, `IDC_IBEAM`, `xterm`).
    Text,
    /// Vertical-text insertion.
    VerticalText,
    /// A link or other clickable affordance (`pointingHand`, `IDC_HAND`).
    Pointer,
    /// Open hand / draggable surface.
    Grab,
    /// Closed hand / drag in progress.
    Grabbing,
    /// Crosshair.
    Crosshair,
    /// Busy.
    Wait,
    /// Action is not permitted here.
    NotAllowed,
    /// Resize affordances. `axis` is the edge or corner being resized.
    Resize { axis: ResizeAxis },
    /// A cursor with no portable equivalent, shipped as pixels.
    ///
    /// `png` is a PNG image of the cursor at `scale` device pixels per point;
    /// `hotspot_x`/`hotspot_y` are in the image's own pixel space. Bounded by
    /// [`MAX_CURSOR_IMAGE_BYTES`] at the daemon boundary.
    Custom {
        png: Vec<u8>,
        hotspot_x: f64,
        hotspot_y: f64,
        scale: f64,
    },
    /// The host could not determine the shape. Distinct from `Default`: it
    /// means "unknown", not "arrow". Viewers should keep the previous shape
    /// rather than snapping to an arrow.
    Unknown,
    /// Forward-compatible sink for a shape introduced after v1.
    #[serde(other)]
    Unsupported,
}

/// Which edge or corner a [`CursorShape::Resize`] refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResizeAxis {
    /// Vertical (north-south).
    NorthSouth,
    /// Horizontal (east-west).
    EastWest,
    /// Both, from the north-east/south-west diagonal.
    NorthEastSouthWest,
    /// Both, from the north-west/south-east diagonal.
    NorthWestSouthEast,
    /// Omnidirectional move.
    All,
    /// A column separator (between table columns).
    Column,
    /// A row separator.
    Row,
}

/// Upper bound on a [`CursorShape::Custom`] PNG. A pointer bitmap is tiny;
/// this exists so a hostile or broken provider cannot push large payloads
/// through the presence channel, which is otherwise all small control messages.
pub const MAX_CURSOR_IMAGE_BYTES: usize = 64 * 1024;

impl CursorShape {
    /// Whether this shape carries a bitmap payload.
    pub fn is_custom(&self) -> bool {
        matches!(self, CursorShape::Custom { .. })
    }

    /// Reject a `Custom` shape whose image exceeds [`MAX_CURSOR_IMAGE_BYTES`],
    /// degrading it to `Unknown` rather than forwarding an unbounded payload.
    /// `Unknown` is correct here: the host *has* a shape, we just declined to
    /// carry it, so the viewer should hold its previous shape rather than
    /// snapping to an arrow.
    pub fn bounded(self) -> Self {
        match &self {
            CursorShape::Custom { png, .. } if png.len() > MAX_CURSOR_IMAGE_BYTES => {
                CursorShape::Unknown
            }
            _ => self,
        }
    }
}

/// A remote user's cursor state, broadcast to every other connection.
/// Coordinates are window-local pixels in the same space as video frames for
/// that window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CursorState {
    pub user_id: String,
    pub name: String,
    pub color: String,
    pub window: Option<TargetHandle>,
    pub x: f64,
    pub y: f64,
    pub visible: bool,
    pub pressed: bool,
    /// What the guest OS is drawing as the pointer under this cursor.
    ///
    /// The origin desktop has one physical pointer, so this is authoritative
    /// only for whichever participant currently owns it. For a participant
    /// whose position is not the real pointer position the host reports
    /// [`CursorShape::Unknown`] rather than guessing — hit-testing another
    /// point's cursor shape is not something any of the supported platforms
    /// exposes.
    ///
    /// Older peers omit the field entirely and decode as `Default`.
    #[serde(default)]
    pub shape: CursorShape,
}

/// One launchable application in the daemon's configured app menu. The launch
/// path stays on the server; clients only see the opaque `app_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppEntry {
    pub app_id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamStats {
    pub session_id: WindowSessionId,
    pub frames_emitted: u64,
    pub frames_replaced: u64,
    pub keyframe_requests: u64,
    pub pending_frames: u8,
    /// Encoded or packed video payload bytes handed to the transport.
    #[serde(default)]
    pub bytes_emitted: u64,
    /// Actions accepted for provider dispatch in this session.
    #[serde(default)]
    pub actions_dispatched: u64,
    /// Delivered actions for which no subsequent capture frame arrived before
    /// the correlation deadline.
    #[serde(default)]
    pub action_frame_timeouts: u64,
    /// Successful runtime capture preference changes.
    #[serde(default)]
    pub preference_updates: u64,
    /// Device-semantic input events accepted by the native provider.
    #[serde(default)]
    pub input_events_dispatched: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServerErrorCode {
    HelloRequired,
    UnsupportedProtocol,
    UnsupportedMessage,
    AlreadyAuthenticated,
    AlreadyNegotiated,
    DiscoveryFailed,
    CodecUnavailable,
    InvalidOpen,
    UnknownWindow,
    EncoderFailed,
    CaptureFailed,
    UnknownSession,
    WindowStateFailed,
    Unsupported,
    InvalidFrame,
    RateLimited,
    Internal,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Used by transport bindings that authenticate with a protocol token.
    /// Bindings authenticated out of band, including Tailscale Serve, send
    /// `Authenticated` before the client starts with `Hello`.
    Authenticate {
        token: String,
    },
    Hello(Hello),
    ListWindows {
        on_screen_only: bool,
    },
    /// Fetch the owning application's icon for a currently discoverable
    /// target. Native process identifiers and server filesystem paths never
    /// cross the protocol boundary.
    GetAppIcon {
        window: TargetHandle,
        target_epoch: TargetEpoch,
    },
    PickWindow {
        prompt: Option<String>,
    },
    RestoreWindow {
        grant: TargetGrant,
    },
    OpenSession(OpenSession),
    CloseSession {
        session_id: WindowSessionId,
    },
    InteractiveInput(InteractiveInputBatch),
    Action(ActionRequest),
    RequestKeyframe {
        session_id: WindowSessionId,
    },
    SetStreamPreferences(StreamPreferences),
    SetWindowGeometry(WindowGeometryRequest),
    GetWindowState {
        session_id: WindowSessionId,
    },
    GetStats {
        session_id: WindowSessionId,
    },
    /// Poll the host text clipboard. The host omits the text payload when its
    /// native generation matches the last generation observed by this client.
    GetClipboard {
        known_generation: Option<u64>,
    },
    /// Replace the host text clipboard. Clipboard payloads are bounded by
    /// `MAX_CLIPBOARD_TEXT_BYTES` at the daemon boundary.
    SetClipboard {
        text: String,
    },
    /// Poll the host file clipboard. File contents are bounded and sent only
    /// when the native clipboard generation changed.
    GetClipboardFiles {
        known_generation: Option<u64>,
    },
    /// Replace the host file clipboard with bounded regular files.
    SetClipboardFiles {
        files: Vec<ClipboardFile>,
        byte_len: u64,
    },
    /// Announce this connection's user identity for multi-user presence.
    Join {
        name: String,
        color: Option<String>,
    },
    /// Report this user's cursor so other clients (and the host desktop
    /// overlay) can render it. Window-local pixel coordinates.
    Cursor {
        window: Option<TargetHandle>,
        x: f64,
        y: f64,
        visible: bool,
        pressed: bool,
    },
    /// List the daemon's configured launchable applications.
    ListApps,
    /// Launch a configured application in the background. The launched window
    /// must not take foreground focus on the host desktop.
    LaunchApp {
        app_id: String,
    },
    /// Forward-compatible sink for a message type introduced after v1.
    #[serde(other)]
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum ServerMessage {
    Authenticated,
    Hello(Hello),
    Windows {
        windows: Vec<WindowDescriptor>,
    },
    WindowSelected {
        window: WindowDescriptor,
        grant: Option<TargetGrant>,
    },
    SessionOpened(SessionOpened),
    SessionClosed {
        session_id: WindowSessionId,
    },
    KeyframeRequested {
        session_id: WindowSessionId,
    },
    StreamPreferencesApplied(StreamPreferences),
    WindowGeometryResult(WindowGeometryResult),
    Lifecycle {
        session_id: WindowSessionId,
        event: WindowLifecycleEvent,
    },
    InteractiveInputAcknowledgement(InteractiveInputAcknowledgement),
    ActionResult(ActionResult),
    ActionFrameCorrelation(ActionFrameCorrelation),
    WindowState(WindowState),
    Stats(StreamStats),
    /// Current host text clipboard. `None` means the generation is unchanged
    /// or the clipboard has no bounded UTF-8 text representation.
    Clipboard {
        generation: u64,
        text: Option<String>,
    },
    ClipboardFiles {
        generation: u64,
        files: Option<Vec<ClipboardFile>>,
        byte_len: u64,
    },
    ConsentRequired {
        message: String,
    },
    /// Acknowledges `Join` with this connection's assigned identity and the
    /// current roster.
    Joined {
        user: PresenceUser,
        users: Vec<PresenceUser>,
    },
    /// Roster change broadcast to every connection.
    Presence {
        users: Vec<PresenceUser>,
    },
    /// Another user's cursor moved.
    RemoteCursor(CursorState),
    Apps {
        apps: Vec<AppEntry>,
    },
    AppLaunched {
        app_id: String,
        /// The launched application's main window, when it materialized in
        /// time to be discovered.
        window: Option<WindowDescriptor>,
    },
    Error {
        code: ServerErrorCode,
        message: String,
    },
    /// Clients must ignore server message types introduced after v1.
    #[serde(other)]
    Unsupported,
}

/// Header for the length-prefixed local transport. Video bytes are carried as
/// the packet payload rather than embedded in JSON; control packets have an
/// empty payload. The same split maps directly onto WebSocket binary frames.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "direction", content = "message", rename_all = "snake_case")]
pub enum WireHeader {
    Client(ClientMessage),
    Server(ServerMessage),
    Video(VideoFrameDescriptor),
    AppIcon(AppIconDescriptor),
}

impl<'de> Deserialize<'de> for WireHeader {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawHeader {
            direction: String,
            message: Value,
        }

        let raw = RawHeader::deserialize(deserializer)?;
        match raw.direction.as_str() {
            "client" => {
                let message_type = raw
                    .message
                    .get("type")
                    .and_then(Value::as_str)
                    .ok_or_else(|| serde::de::Error::custom("client message has no string type"))?;
                if !matches!(
                    message_type,
                    "authenticate"
                        | "hello"
                        | "list_windows"
                        | "get_app_icon"
                        | "pick_window"
                        | "restore_window"
                        | "open_session"
                        | "close_session"
                        | "interactive_input"
                        | "action"
                        | "request_keyframe"
                        | "set_stream_preferences"
                        | "set_window_geometry"
                        | "get_window_state"
                        | "get_stats"
                        | "get_clipboard"
                        | "set_clipboard"
                        | "get_clipboard_files"
                        | "set_clipboard_files"
                        | "join"
                        | "cursor"
                        | "list_apps"
                        | "launch_app"
                        | "unsupported"
                ) {
                    return Ok(Self::Client(ClientMessage::Unsupported));
                }
                serde_json::from_value(raw.message)
                    .map(Self::Client)
                    .map_err(serde::de::Error::custom)
            }
            "server" => {
                let message_type = raw
                    .message
                    .get("type")
                    .and_then(Value::as_str)
                    .ok_or_else(|| serde::de::Error::custom("server message has no string type"))?;
                if !matches!(
                    message_type,
                    "authenticated"
                        | "hello"
                        | "windows"
                        | "window_selected"
                        | "session_opened"
                        | "session_closed"
                        | "keyframe_requested"
                        | "stream_preferences_applied"
                        | "window_geometry_result"
                        | "lifecycle"
                        | "interactive_input_acknowledgement"
                        | "action_result"
                        | "action_frame_correlation"
                        | "window_state"
                        | "stats"
                        | "clipboard"
                        | "clipboard_files"
                        | "consent_required"
                        | "joined"
                        | "presence"
                        | "remote_cursor"
                        | "apps"
                        | "app_launched"
                        | "error"
                        | "unsupported"
                ) {
                    return Ok(Self::Server(ServerMessage::Unsupported));
                }
                serde_json::from_value(raw.message)
                    .map(Self::Server)
                    .map_err(serde::de::Error::custom)
            }
            "video" => serde_json::from_value(raw.message)
                .map(Self::Video)
                .map_err(serde::de::Error::custom),
            "app_icon" => serde_json::from_value(raw.message)
                .map(Self::AppIcon)
                .map_err(serde::de::Error::custom),
            other => Err(serde::de::Error::custom(format!(
                "unknown wire direction {other}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_basis_keeps_visual_and_accessibility_freshness_separate() {
        let pixel = ActionBasis::Pixel {
            geometry_epoch: GeometryEpoch(4),
            frame_sequence: FrameSequence(19),
        };
        let ax = ActionBasis::Accessibility {
            snapshot_id: AccessibilitySnapshotId(7),
        };

        assert_ne!(
            serde_json::to_value(pixel).unwrap(),
            serde_json::to_value(ax).unwrap()
        );
    }

    #[test]
    fn client_message_round_trips_without_platform_handles() {
        let message = ClientMessage::Action(ActionRequest {
            action_id: "a-1".into(),
            session_id: WindowSessionId("session-opaque".into()),
            tool: "click".into(),
            arguments: serde_json::json!({"x": 12, "y": 34}),
            basis: ActionBasis::Pixel {
                geometry_epoch: GeometryEpoch(2),
                frame_sequence: FrameSequence(9),
            },
        });

        let encoded = serde_json::to_vec(&message).unwrap();
        let decoded: ClientMessage = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, message);
        let text = String::from_utf8(encoded).unwrap();
        assert!(!text.contains("pid"));
        assert!(!text.contains("window_id"));
    }

    #[test]
    fn v1_golden_messages_pin_the_wire_shape() {
        let open = WireHeader::Client(ClientMessage::OpenSession(OpenSession {
            window: TargetHandle("w-opaque".into()),
            target_epoch: TargetEpoch(4),
            accepted_codecs: vec![VideoCodec::H264, VideoCodec::Bgra],
            max_fps: 15,
            max_dimension: 1280,
            target_bitrate_kbps: None,
            policy: SessionPolicy::BackgroundOnly,
            geometry_control: WindowGeometryControl::ObserveOnly,
        }));
        assert_eq!(
            serde_json::to_string(&open).unwrap(),
            r#"{"direction":"client","message":{"type":"open_session","payload":{"window":"w-opaque","target_epoch":4,"accepted_codecs":["h264","bgra"],"max_fps":15,"max_dimension":1280,"policy":"background_only"}}}"#
        );

        let video = WireHeader::Video(VideoFrameDescriptor {
            session_id: WindowSessionId("s-opaque".into()),
            sequence: FrameSequence(9),
            geometry_epoch: GeometryEpoch(2),
            codec_epoch: CodecEpoch(3),
            width_px: 800,
            height_px: 600,
            capture_timestamp_us: 123_456,
            encode_duration_us: None,
            codec: VideoCodec::H264,
            keyframe: true,
        });
        assert_eq!(
            serde_json::to_string(&video).unwrap(),
            r#"{"direction":"video","message":{"session_id":"s-opaque","sequence":9,"geometry_epoch":2,"codec_epoch":3,"width_px":800,"height_px":600,"capture_timestamp_us":123456,"codec":"h264","keyframe":true}}"#
        );

        let app_icon = WireHeader::AppIcon(AppIconDescriptor {
            window: TargetHandle("w-opaque".into()),
            target_epoch: TargetEpoch(4),
            media_type: "application/x-apple-icns".into(),
            byte_len: 42,
        });
        assert_eq!(
            serde_json::to_string(&app_icon).unwrap(),
            r#"{"direction":"app_icon","message":{"window":"w-opaque","target_epoch":4,"media_type":"application/x-apple-icns","byte_len":42}}"#
        );
    }

    #[test]
    fn unknown_message_types_are_forward_compatible() {
        let client: WireHeader = serde_json::from_str(
            r#"{"direction":"client","message":{"type":"future_request","payload":{"x":1}}}"#,
        )
        .unwrap();
        assert_eq!(client, WireHeader::Client(ClientMessage::Unsupported));

        let server: WireHeader = serde_json::from_str(
            r#"{"direction":"server","message":{"type":"future_notice","payload":{"x":1}}}"#,
        )
        .unwrap();
        assert_eq!(server, WireHeader::Server(ServerMessage::Unsupported));
    }

    #[test]
    fn geometry_control_is_backward_compatible_and_explicit_on_the_wire() {
        let legacy: OpenSession = serde_json::from_str(
            r#"{"window":"w","target_epoch":1,"accepted_codecs":["bgra"],"max_fps":15,"max_dimension":1280,"policy":"background_only"}"#,
        )
        .unwrap();
        assert_eq!(legacy.geometry_control, WindowGeometryControl::ObserveOnly);

        let mut controlled = legacy;
        controlled.geometry_control = WindowGeometryControl::Bidirectional;
        assert_eq!(
            serde_json::to_value(controlled).unwrap()["geometry_control"],
            "bidirectional"
        );
    }

    #[test]
    fn every_v1_control_message_round_trips() {
        let session_id = WindowSessionId("s-opaque".into());
        let window = TargetHandle("w-opaque".into());
        let geometry = SurfaceGeometry {
            width_px: 800,
            height_px: 600,
            scale_factor: 2.0,
        };
        let client_messages = vec![
            ClientMessage::Authenticate {
                token: "token".into(),
            },
            ClientMessage::Hello(Hello::default()),
            ClientMessage::ListWindows {
                on_screen_only: true,
            },
            ClientMessage::GetAppIcon {
                window: window.clone(),
                target_epoch: TargetEpoch(4),
            },
            ClientMessage::PickWindow {
                prompt: Some("Choose a window".into()),
            },
            ClientMessage::RestoreWindow {
                grant: TargetGrant("grant-opaque".into()),
            },
            ClientMessage::OpenSession(OpenSession {
                window: window.clone(),
                target_epoch: TargetEpoch(4),
                accepted_codecs: vec![VideoCodec::H264],
                max_fps: 15,
                max_dimension: 1280,
                target_bitrate_kbps: None,
                policy: SessionPolicy::BackgroundOnly,
                geometry_control: WindowGeometryControl::Bidirectional,
            }),
            ClientMessage::CloseSession {
                session_id: session_id.clone(),
            },
            ClientMessage::InteractiveInput(InteractiveInputBatch {
                session_id: session_id.clone(),
                first_sequence: 1,
                events: vec![InteractiveInputEvent::Pointer {
                    phase: InputPointerPhase::Move,
                    button: None,
                    x_normalized: 0.25,
                    y_normalized: 0.75,
                    modifiers: Vec::new(),
                }],
            }),
            ClientMessage::Action(ActionRequest {
                action_id: "a-1".into(),
                session_id: session_id.clone(),
                tool: "click".into(),
                arguments: serde_json::json!({"x": 1, "y": 2}),
                basis: ActionBasis::None,
            }),
            ClientMessage::RequestKeyframe {
                session_id: session_id.clone(),
            },
            ClientMessage::SetStreamPreferences(StreamPreferences {
                session_id: session_id.clone(),
                max_fps: 24,
                max_dimension: 1_024,
                target_bitrate_kbps: None,
            }),
            ClientMessage::SetWindowGeometry(WindowGeometryRequest {
                session_id: session_id.clone(),
                revision: 3,
                width_points: 960,
                height_points: 640,
            }),
            ClientMessage::GetWindowState {
                session_id: session_id.clone(),
            },
            ClientMessage::GetStats {
                session_id: session_id.clone(),
            },
            ClientMessage::GetClipboard {
                known_generation: Some(12),
            },
            ClientMessage::SetClipboard {
                text: "clipboard text".into(),
            },
            ClientMessage::GetClipboardFiles {
                known_generation: Some(12),
            },
            ClientMessage::SetClipboardFiles {
                files: vec![ClipboardFile {
                    name: "note.txt".into(),
                    offset: 0,
                    byte_len: 4,
                    sha256: "digest".into(),
                }],
                byte_len: 4,
            },
            ClientMessage::Join {
                name: "dillon".into(),
                color: Some("#ff8800".into()),
            },
            ClientMessage::Cursor {
                window: Some(window.clone()),
                x: 12.5,
                y: 40.0,
                visible: true,
                pressed: false,
            },
            ClientMessage::ListApps,
            ClientMessage::LaunchApp {
                app_id: "test-pad".into(),
            },
        ];
        for message in client_messages {
            let header = WireHeader::Client(message);
            let encoded = serde_json::to_vec(&header).unwrap();
            assert_eq!(
                serde_json::from_slice::<WireHeader>(&encoded).unwrap(),
                header
            );
        }

        let server_messages = vec![
            ServerMessage::Authenticated,
            ServerMessage::Hello(Hello::default()),
            ServerMessage::Windows {
                windows: vec![WindowDescriptor {
                    window: window.clone(),
                    target_epoch: TargetEpoch(4),
                    app_name: "Notes".into(),
                    title: "Draft".into(),
                    geometry: geometry.clone(),
                    visible: true,
                }],
            },
            ServerMessage::WindowSelected {
                window: WindowDescriptor {
                    window,
                    target_epoch: TargetEpoch(4),
                    app_name: "Notes".into(),
                    title: "Draft".into(),
                    geometry: geometry.clone(),
                    visible: true,
                },
                grant: Some(TargetGrant("grant-opaque".into())),
            },
            ServerMessage::SessionOpened(SessionOpened {
                session_id: session_id.clone(),
                target_epoch: TargetEpoch(4),
                geometry_epoch: GeometryEpoch(2),
                codec_epoch: CodecEpoch(3),
                geometry: geometry.clone(),
                codec: VideoCodec::H264,
                max_fps: 15,
                max_dimension: 1280,
                target_bitrate_kbps: None,
                capabilities: vec!["video.h264".into()],
                action_capabilities: vec![ActionCapability {
                    action: "type_text".into(),
                    guarantee: ActionDeliveryGuarantee::Background,
                }],
                policy: SessionPolicy::BackgroundOnly,
                geometry_control: WindowGeometryControl::Bidirectional,
            }),
            ServerMessage::SessionClosed {
                session_id: session_id.clone(),
            },
            ServerMessage::KeyframeRequested {
                session_id: session_id.clone(),
            },
            ServerMessage::StreamPreferencesApplied(StreamPreferences {
                session_id: session_id.clone(),
                max_fps: 24,
                max_dimension: 1_024,
                target_bitrate_kbps: None,
            }),
            ServerMessage::WindowGeometryResult(WindowGeometryResult {
                session_id: session_id.clone(),
                revision: 3,
                applied: true,
                width_points: 960,
                height_points: 640,
                error: None,
            }),
            ServerMessage::Lifecycle {
                session_id: session_id.clone(),
                event: WindowLifecycleEvent::Suspended {
                    reason: SuspensionReason::Minimized,
                },
            },
            ServerMessage::InteractiveInputAcknowledgement(InteractiveInputAcknowledgement {
                session_id: session_id.clone(),
                through_sequence: 1,
                delivered: true,
                error: None,
                host_dispatch_us: Some(150),
            }),
            ServerMessage::ActionResult(ActionResult {
                action_id: "a-1".into(),
                delivered: true,
                error: None,
                first_frame_sequence_after: Some(FrameSequence(9)),
            }),
            ServerMessage::ActionFrameCorrelation(ActionFrameCorrelation {
                action_id: "a-1".into(),
                session_id: session_id.clone(),
                first_frame_sequence_after: Some(FrameSequence(9)),
            }),
            ServerMessage::WindowState(WindowState {
                session_id: session_id.clone(),
                snapshot_id: AccessibilitySnapshotId(4),
                state: serde_json::json!({"elements": []}),
            }),
            ServerMessage::Stats(StreamStats {
                session_id,
                frames_emitted: 10,
                frames_replaced: 2,
                keyframe_requests: 1,
                pending_frames: 1,
                bytes_emitted: 42_000,
                actions_dispatched: 3,
                action_frame_timeouts: 1,
                preference_updates: 2,
                input_events_dispatched: 7,
            }),
            ServerMessage::Clipboard {
                generation: 13,
                text: Some("clipboard text".into()),
            },
            ServerMessage::ClipboardFiles {
                generation: 13,
                files: Some(vec![ClipboardFile {
                    name: "note.txt".into(),
                    offset: 0,
                    byte_len: 4,
                    sha256: "digest".into(),
                }]),
                byte_len: 4,
            },
            ServerMessage::ConsentRequired {
                message: "grant Screen Recording".into(),
            },
            ServerMessage::Joined {
                user: PresenceUser {
                    user_id: "u-1".into(),
                    name: "dillon".into(),
                    color: "#ff8800".into(),
                },
                users: vec![PresenceUser {
                    user_id: "u-1".into(),
                    name: "dillon".into(),
                    color: "#ff8800".into(),
                }],
            },
            ServerMessage::Presence { users: Vec::new() },
            ServerMessage::RemoteCursor(CursorState {
                user_id: "u-2".into(),
                name: "guest".into(),
                color: "#00ccff".into(),
                window: Some(TargetHandle("w-opaque".into())),
                x: 3.0,
                y: 4.0,
                visible: true,
                pressed: true,
                // A structured variant, so the round-trip exercises the
                // tagged-enum encoding rather than only the unit variants.
                shape: CursorShape::Resize {
                    axis: ResizeAxis::NorthWestSouthEast,
                },
            }),
            ServerMessage::Apps {
                apps: vec![AppEntry {
                    app_id: "test-pad".into(),
                    name: "RCDP Test Pad".into(),
                }],
            },
            ServerMessage::AppLaunched {
                app_id: "test-pad".into(),
                window: None,
            },
            ServerMessage::Error {
                code: ServerErrorCode::UnknownSession,
                message: "session does not exist".into(),
            },
        ];
        for message in server_messages {
            let header = WireHeader::Server(message);
            let encoded = serde_json::to_vec(&header).unwrap();
            assert_eq!(
                serde_json::from_slice::<WireHeader>(&encoded).unwrap(),
                header
            );
        }
    }

    #[test]
    fn stream_stats_accepts_peers_without_performance_counters() {
        let stats: StreamStats = serde_json::from_str(
            r#"{"session_id":"s-legacy","frames_emitted":10,"frames_replaced":2,"keyframe_requests":1,"pending_frames":0}"#,
        )
        .unwrap();
        assert_eq!(stats.bytes_emitted, 0);
        assert_eq!(stats.actions_dispatched, 0);
        assert_eq!(stats.action_frame_timeouts, 0);
        assert_eq!(stats.preference_updates, 0);
        assert_eq!(stats.input_events_dispatched, 0);
    }

    #[test]
    fn interactive_input_validation_preserves_order_and_native_privacy() {
        let batch = InteractiveInputBatch {
            session_id: WindowSessionId("s-opaque".into()),
            first_sequence: 41,
            events: vec![
                InteractiveInputEvent::Key {
                    key: "a".into(),
                    state: InputKeyState::Down,
                    modifiers: vec![InputModifier::Command],
                    repeat: false,
                },
                InteractiveInputEvent::Key {
                    key: "a".into(),
                    state: InputKeyState::Up,
                    modifiers: vec![InputModifier::Command],
                    repeat: false,
                },
            ],
        };
        assert_eq!(batch.validate().unwrap(), 42);
        let encoded = serde_json::to_string(&batch).unwrap();
        assert!(!encoded.contains("pid"));
        assert!(!encoded.contains("window_id"));
    }
}

#[cfg(test)]
mod cursor_shape_wire_tests {
    use super::*;

    /// Wire compatibility: a peer built before cursor shapes existed sends a
    /// `CursorState` with no `shape` key at all. It must still decode.
    ///
    /// Before `shape` carried `#[serde(default)]` this failed with "missing
    /// field `shape`" -- exactly how every pre-existing peer would have broken.
    #[test]
    fn a_cursor_state_without_shape_still_decodes() {
        let legacy = serde_json::json!({
            "user_id": "u-1", "name": "dillon", "color": "#ff8800",
            "window": "w-opaque", "x": 1.0, "y": 2.0,
            "visible": true, "pressed": false
        });
        let state: CursorState =
            serde_json::from_value(legacy).expect("legacy cursor state must decode");
        assert_eq!(state.shape, CursorShape::Default);
    }

    /// `Unknown` must survive the wire distinctly from `Default`. Collapsing
    /// them would make "this host cannot tell you" indistinguishable from
    /// "it is an arrow", which is the whole point of having both.
    #[test]
    fn unknown_and_default_are_distinct_on_the_wire() {
        let unknown = serde_json::to_value(CursorShape::Unknown).unwrap();
        let default = serde_json::to_value(CursorShape::Default).unwrap();
        assert_ne!(unknown, default);
        assert_eq!(
            serde_json::from_value::<CursorShape>(unknown).unwrap(),
            CursorShape::Unknown
        );
    }

    /// A shape introduced after v1 must not fail the whole message.
    #[test]
    fn an_unknown_future_shape_decodes_as_unsupported() {
        let future = serde_json::json!({ "kind": "holographic_pointer" });
        assert_eq!(
            serde_json::from_value::<CursorShape>(future).unwrap(),
            CursorShape::Unsupported
        );
    }

    /// An oversized custom cursor must be dropped rather than forwarded
    /// through the presence channel, and must degrade to `Unknown` (hold the
    /// previous shape) rather than `Default` (snap to an arrow).
    #[test]
    fn an_oversized_custom_cursor_is_bounded_away() {
        let big = CursorShape::Custom {
            png: vec![0u8; MAX_CURSOR_IMAGE_BYTES + 1],
            hotspot_x: 1.0,
            hotspot_y: 2.0,
            scale: 2.0,
        };
        assert_eq!(big.bounded(), CursorShape::Unknown);

        let ok = CursorShape::Custom {
            png: vec![0u8; 16],
            hotspot_x: 1.0,
            hotspot_y: 2.0,
            scale: 2.0,
        };
        assert!(ok.clone().bounded().is_custom(), "a small cursor survives");
        // And it round-trips with its payload intact.
        let encoded = serde_json::to_value(&ok).unwrap();
        assert_eq!(serde_json::from_value::<CursorShape>(encoded).unwrap(), ok);
    }
}

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! rcdp wire version 2 (the media plane of cua-spacesd).
//!
//! The authoritative description is `libs/cua/proto/MEDIA.md`. This module
//! only holds the message shapes; it reuses every v1 type whose meaning did not
//! change (video descriptors, lifecycle events, interactive input batches,
//! actions, epochs).
//!
//! Differences from v1 that matter to code using this module:
//!
//! - Session setup lives in gRPC (`StreamService.OpenMedia`). The media socket
//!   is attached to an existing session with a ticket, so there is no client
//!   `Hello`/`OpenSession` here. The server speaks first: `hello`, then
//!   `session_opened`, then one `audio_config` per audio track.
//! - Text control frames carry a bare `{"type": ..., "payload": ...}` object.
//!   [`decode_client_text`] and [`decode_server_text`] also accept the v1
//!   `{"direction": ..., "message": ...}` envelope so older tooling can talk to
//!   a v2 peer during migration.
//! - Binary video packets are unchanged from v1 (two big-endian `u32` lengths,
//!   a JSON `WireHeader::Video` header, then the payload). Binary audio packets
//!   start with the `RAU2` magic; see `cua_media_transport::audio`.

use serde::{Deserialize, Serialize};

use crate::{
    ActionCapability, ActionFrameCorrelation, ActionRequest, ActionResult, CodecEpoch,
    GeometryEpoch, InteractiveInputAcknowledgement, InteractiveInputBatch, SessionPolicy,
    StreamPreferences, StreamStats, SurfaceGeometry, TargetEpoch, TargetHandle, Value, VideoCodec,
    WindowGeometryControl, WindowGeometryRequest, WindowGeometryResult, WindowLifecycleEvent,
    WindowSessionId, WindowState,
};

/// Media wire version implemented by this module.
pub const WIRE_VERSION: u16 = 2;
/// Protocol name announced in `hello`.
pub const PROTOCOL: &str = "rcdp";
/// WebSocket subprotocol for wire v2.
pub const WS_SUBPROTOCOL: &str = "rcdp.v2";
/// Prefix of the header-free ticket subprotocol (`cua.ticket.<ticket>`),
/// the same form every cua-spacesd ticket route accepts. Clients send it
/// alongside [`WS_SUBPROTOCOL`].
pub const WS_TICKET_SUBPROTOCOL_PREFIX: &str = "cua.ticket.";
/// Legacy ticket subprotocol prefix (`rcdp.v2.ticket.<ticket>`). Servers
/// still accept it on the media socket; clients must not send it.
pub const LEGACY_WS_TICKET_SUBPROTOCOL_PREFIX: &str = "rcdp.v2.ticket.";
/// QUIC ALPN id for wire v2.
pub const QUIC_ALPN: &[u8] = b"rcdp/2";
/// At most this many sockets may attach to one media session.
pub const MAX_SOCKETS_PER_SESSION: usize = 8;
/// At most this many media sessions may exist per driver.
pub const MAX_MEDIA_SESSIONS: usize = 64;
/// Server keepalive ping interval on the WebSocket binding.
pub const PING_INTERVAL_SECS: u64 = 20;
/// A socket that has not answered a ping for this long is closed with 4408.
pub const IDLE_TIMEOUT_SECS: u64 = 60;
/// Most audio kept queued per track per socket before the oldest is dropped.
pub const MAX_QUEUED_AUDIO_MS: u64 = 200;

/// Capability strings announced in `hello`. Unknown strings must be ignored.
pub mod capability {
    /// Interactive input batches with adopt-first sequencing (MEDIA.md §6).
    pub const INPUT_INTERACTIVE: &str = "input.interactive.v2";
    /// Whole-display targets (MEDIA.md §4).
    pub const DESKTOP: &str = "desktop.v1";
    /// The first packet on every socket is a current keyframe (MEDIA.md §5).
    pub const KEYFRAME_ON_ATTACH: &str = "keyframe_on_attach.v1";
    /// Audio tracks (MEDIA.md §12).
    pub const AUDIO: &str = "audio.v1";
    /// Optional client `frame_ack` messages feed per-socket flow control and
    /// the encoder rate controller. Clients that never ack still work; they
    /// are paced by the send queue instead.
    pub const FRAME_ACK: &str = "frame_ack.v1";
    /// `stats` carries the v2 health counters (MEDIA.md §8).
    pub const STATS: &str = "stats.v2";
}

/// WebSocket close codes (MEDIA.md §10). QUIC application error codes use the
/// same numbers in hexadecimal (`4401` -> `0x401`).
pub mod close_code {
    pub const NORMAL: u16 = 1000;
    pub const PROTOCOL_VIOLATION: u16 = 4400;
    pub const TICKET_INVALID: u16 = 4401;
    pub const SESSION_CLOSED: u16 = 4404;
    pub const IDLE_TIMEOUT: u16 = 4408;
    pub const TARGET_GONE: u16 = 4410;
    pub const TOO_MANY_SOCKETS: u16 = 4429;
    pub const INTERNAL: u16 = 4500;

    /// The QUIC application error code that mirrors a WebSocket close code.
    pub const fn quic_error(code: u16) -> u32 {
        if code < 4000 {
            return 0;
        }
        // 4401 -> 0x401: the three decimal digits after "4" read as hex.
        let digits = (code - 4000) as u32;
        ((digits / 100) << 8) | (((digits / 10) % 10) << 4) | (digits % 10)
    }
}

/// Server greeting, the first message on every attached socket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub protocol: String,
    pub versions: Vec<u16>,
    pub selected_version: u16,
    pub capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_revision: Option<String>,
}

impl Hello {
    pub fn server(capabilities: Vec<String>) -> Self {
        Self {
            protocol: PROTOCOL.to_owned(),
            versions: vec![WIRE_VERSION],
            selected_version: WIRE_VERSION,
            capabilities,
            build_revision: Some(crate::build_revision().to_owned()),
        }
    }

    pub fn has_capability(&self, name: &str) -> bool {
        self.capabilities
            .iter()
            .any(|capability| capability == name)
    }
}

/// What a media session streams (MEDIA.md §4).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MediaTargetRef {
    Window {
        handle: TargetHandle,
        epoch: TargetEpoch,
    },
    Display {
        display_id: String,
    },
}

impl MediaTargetRef {
    pub fn is_display(&self) -> bool {
        matches!(self, Self::Display { .. })
    }
}

/// `session_opened` payload. `session_id` equals
/// `OpenMediaResponse.media_session_id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionOpened {
    pub session_id: WindowSessionId,
    pub target: MediaTargetRef,
    pub target_epoch: TargetEpoch,
    pub geometry_epoch: GeometryEpoch,
    pub codec_epoch: CodecEpoch,
    pub geometry: SurfaceGeometry,
    pub codec: VideoCodec,
    /// False for audio-only sessions (`OpenMediaRequest.disable_video`).
    #[serde(default = "default_true")]
    pub video: bool,
    pub max_fps: u16,
    pub max_dimension: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_bitrate_kbps: Option<u32>,
    pub capabilities: Vec<String>,
    pub action_capabilities: Vec<ActionCapability>,
    pub policy: SessionPolicy,
    #[serde(default)]
    pub geometry_control: WindowGeometryControl,
}

fn default_true() -> bool {
    true
}

/// Error codes of the v2 media plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// A v1 message whose job moved to gRPC, or a message this server does
    /// not implement.
    UnsupportedOperation,
    /// An interactive input batch did not continue the sequence. The payload
    /// carries `expected_sequence`.
    InputSequenceGap,
    /// Uplink audio for a track that was not granted.
    AudioUplinkDenied,
    /// A malformed or out-of-bounds control message.
    InvalidMessage,
    /// The message named a session other than the attached one.
    UnknownSession,
    ViewOnly,
    StaleTarget,
    StaleGeometry,
    WouldRequireActivation,
    DeliveryFailed,
    CaptureFailed,
    EncoderFailed,
    WindowStateFailed,
    Unsupported,
    RateLimited,
    Internal,
    #[serde(other)]
    Unknown,
}

/// Per-track audio direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioDirection {
    Down,
    Up,
}

/// Audio codec names on the media plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioCodecName {
    Opus,
    PcmS16le,
    #[serde(other)]
    Unknown,
}

/// What an audio track captures.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioSourceRef {
    pub source_id: String,
    /// `desktop` or `application`.
    pub kind: String,
    #[serde(default)]
    pub desktop_fallback: bool,
}

/// `audio_config` (server to client), MEDIA.md §12.3.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioConfig {
    pub track_id: u16,
    pub config_epoch: u8,
    pub direction: AudioDirection,
    pub codec: AudioCodecName,
    pub sample_rate_hz: u32,
    pub channels: u8,
    pub frame_ms: u16,
    pub bitrate_kbps: u32,
    pub fec: bool,
    pub dtx: bool,
    #[serde(default)]
    pub opus_pre_skip: u16,
    pub source: AudioSourceRef,
}

/// Audio track state (the audio counterpart of video lifecycle).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioTrackStateKind {
    Active,
    Silent,
    Paused,
    Suspended,
    Fallback,
    Ended,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioTrackState {
    pub track_id: u16,
    pub state: AudioTrackStateKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Per-track audio counters (MEDIA.md §12.7).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AudioTrackStats {
    pub track_id: u16,
    pub direction: Option<AudioDirection>,
    pub packets: u64,
    pub bytes: u64,
    pub packets_lost: u64,
    pub packets_recovered_fec: u64,
    pub packets_concealed: u64,
    pub packets_late: u64,
    pub discontinuities: u64,
    pub jitter_ms: f64,
    pub buffer_target_ms: f64,
}

/// v2 stream statistics: the v1 counters plus the health fields of
/// MEDIA.md §8 and the per-socket flow-control counters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stats {
    #[serde(flatten)]
    pub stream: StreamStats,
    /// Frames delivered on this socket.
    pub frames_since_attach: u64,
    /// Time since the last frame was captured for this target, delivered or
    /// not. `None` before the first frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_frame_age_ms: Option<u64>,
    /// Capture is healthy but the target has not changed.
    pub target_idle: bool,
    /// Keyframes delivered on this socket.
    #[serde(default)]
    pub keyframes_sent: u64,
    /// Queued dependent frames discarded because this socket fell behind.
    #[serde(default)]
    pub frames_dropped: u64,
    /// Bytes waiting in this socket's video queue.
    #[serde(default)]
    pub queued_video_bytes: u64,
    /// Round trip measured from `frame_ack`, when the client acks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ack_rtt_ms: Option<f64>,
    /// Current encoder target bitrate, when the codec has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoder_bitrate_kbps: Option<u32>,
    /// Current capture frame-rate cap after rate control.
    #[serde(default)]
    pub effective_max_fps: u16,
    /// Encoder in use, for example `openh264` or `videotoolbox`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoder: Option<String>,
    #[serde(default)]
    pub audio_tracks: Vec<AudioTrackStats>,
}

/// Optional client acknowledgement of a decoded frame
/// (capability [`capability::FRAME_ACK`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameAck {
    pub session_id: WindowSessionId,
    /// Newest frame sequence the client has decoded (or presented).
    pub sequence: u64,
    /// Client decode time for that frame, when measured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decode_us: Option<u32>,
    /// Frames waiting in the client's decode queue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decode_queue: Option<u32>,
}

/// Messages a client sends on an attached media socket.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum ClientMessage {
    /// First message on the QUIC reliable stream (MEDIA.md §2).
    Ticket {
        ticket: String,
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
    AudioUplinkState {
        track_id: u16,
        muted: bool,
    },
    FrameAck(FrameAck),
    Ping {
        nonce: u64,
    },
    /// v1 messages whose job moved to gRPC and anything unknown. The server
    /// answers `error{code: unsupported_operation}`.
    #[serde(other)]
    Unsupported,
}

/// Messages the server sends on an attached media socket.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum ServerMessage {
    Hello(Hello),
    SessionOpened(SessionOpened),
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
    Stats(Box<Stats>),
    AudioConfig(AudioConfig),
    AudioTrackState(AudioTrackState),
    Pong {
        nonce: u64,
    },
    Error {
        code: ErrorCode,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_sequence: Option<u64>,
    },
    #[serde(other)]
    Unsupported,
}

impl ServerMessage {
    pub fn error(code: ErrorCode, message: impl Into<String>) -> Self {
        Self::Error {
            code,
            message: message.into(),
            expected_sequence: None,
        }
    }
}

fn unwrap_envelope(value: Value, direction: &str) -> Result<Value, String> {
    match value {
        Value::Object(mut object) if object.contains_key("direction") => {
            let found = object
                .get("direction")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            if found != direction {
                return Err(format!(
                    "expected a {direction} control message, got direction {found}"
                ));
            }
            object
                .remove("message")
                .ok_or_else(|| "v1 envelope has no message".to_owned())
        }
        other => Ok(other),
    }
}

/// Encode a server control message as one WebSocket text frame.
pub fn encode_server_text(message: &ServerMessage) -> String {
    serde_json::to_string(message).expect("v2 server messages always serialize")
}

/// Encode a client control message as one WebSocket text frame.
pub fn encode_client_text(message: &ClientMessage) -> String {
    serde_json::to_string(message).expect("v2 client messages always serialize")
}

const CLIENT_TYPES: &[&str] = &[
    "ticket",
    "interactive_input",
    "action",
    "request_keyframe",
    "set_stream_preferences",
    "set_window_geometry",
    "get_window_state",
    "get_stats",
    "audio_uplink_state",
    "frame_ack",
    "ping",
];

const SERVER_TYPES: &[&str] = &[
    "hello",
    "session_opened",
    "keyframe_requested",
    "stream_preferences_applied",
    "window_geometry_result",
    "lifecycle",
    "interactive_input_acknowledgement",
    "action_result",
    "action_frame_correlation",
    "window_state",
    "stats",
    "audio_config",
    "audio_track_state",
    "pong",
    "error",
];

fn message_type(value: &Value) -> Result<&str, String> {
    value
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| "control message has no string type".to_owned())
}

/// Decode a client text frame (bare or v1-enveloped). Unknown and v1-only
/// message types decode as [`ClientMessage::Unsupported`].
pub fn decode_client_text(text: &str) -> Result<ClientMessage, String> {
    let value: Value = serde_json::from_str(text).map_err(|error| error.to_string())?;
    decode_client_value(value)
}

/// Decode an already-parsed client control message.
pub fn decode_client_value(value: Value) -> Result<ClientMessage, String> {
    let value = unwrap_envelope(value, "client")?;
    if !CLIENT_TYPES.contains(&message_type(&value)?) {
        return Ok(ClientMessage::Unsupported);
    }
    serde_json::from_value(value).map_err(|error| error.to_string())
}

/// Decode a server text frame (bare or v1-enveloped). Unknown message types
/// decode as [`ServerMessage::Unsupported`] so clients stay forward compatible.
pub fn decode_server_text(text: &str) -> Result<ServerMessage, String> {
    let value: Value = serde_json::from_str(text).map_err(|error| error.to_string())?;
    let value = unwrap_envelope(value, "server")?;
    if !SERVER_TYPES.contains(&message_type(&value)?) {
        return Ok(ServerMessage::Unsupported);
    }
    serde_json::from_value(value).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{InputKeyState, InteractiveInputEvent};

    #[test]
    fn hello_matches_media_md_shape() {
        let hello = Hello::server(vec![capability::DESKTOP.into()]);
        let text = encode_server_text(&ServerMessage::Hello(hello));
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["type"], "hello");
        assert_eq!(value["payload"]["protocol"], "rcdp");
        assert_eq!(value["payload"]["versions"], serde_json::json!([2]));
        assert_eq!(value["payload"]["selected_version"], 2);
        assert_eq!(value["payload"]["capabilities"][0], "desktop.v1");
    }

    #[test]
    fn display_target_has_kind_discriminator() {
        let target = MediaTargetRef::Display {
            display_id: "primary".into(),
        };
        assert_eq!(
            serde_json::to_value(&target).unwrap(),
            serde_json::json!({"kind": "display", "display_id": "primary"})
        );
        let window = MediaTargetRef::Window {
            handle: TargetHandle("w1".into()),
            epoch: TargetEpoch(3),
        };
        assert_eq!(
            serde_json::to_value(&window).unwrap(),
            serde_json::json!({"kind": "window", "handle": "w1", "epoch": 3})
        );
    }

    #[test]
    fn v1_only_messages_decode_as_unsupported() {
        for text in [
            r#"{"type":"list_windows","payload":{"on_screen_only":true}}"#,
            r#"{"type":"open_session","payload":{}}"#,
            r#"{"type":"join","payload":{"name":"x"}}"#,
            r#"{"direction":"client","message":{"type":"get_clipboard","payload":{}}}"#,
        ] {
            assert_eq!(
                decode_client_text(text).unwrap(),
                ClientMessage::Unsupported,
                "{text}"
            );
        }
    }

    #[test]
    fn enveloped_and_bare_client_messages_are_equivalent() {
        let batch = ClientMessage::InteractiveInput(InteractiveInputBatch {
            session_id: WindowSessionId("m1".into()),
            first_sequence: 900,
            events: vec![InteractiveInputEvent::Key {
                key: "a".into(),
                state: InputKeyState::Down,
                modifiers: vec![],
                repeat: false,
            }],
        });
        let bare = encode_client_text(&batch);
        let enveloped = format!(r#"{{"direction":"client","message":{bare}}}"#);
        assert_eq!(decode_client_text(&bare).unwrap(), batch);
        assert_eq!(decode_client_text(&enveloped).unwrap(), batch);
        assert!(decode_client_text(
            r#"{"direction":"server","message":{"type":"pong","payload":{"nonce":1}}}"#
        )
        .is_err());
    }

    #[test]
    fn sequence_gap_error_carries_expected_sequence() {
        let message = ServerMessage::Error {
            code: ErrorCode::InputSequenceGap,
            message: "gap".into(),
            expected_sequence: Some(41),
        };
        let value: Value = serde_json::from_str(&encode_server_text(&message)).unwrap();
        assert_eq!(value["payload"]["code"], "input_sequence_gap");
        assert_eq!(value["payload"]["expected_sequence"], 41);
        assert_eq!(
            decode_server_text(&encode_server_text(&message)).unwrap(),
            message
        );
    }

    #[test]
    fn stats_flatten_v1_counters_and_add_health_fields() {
        let stats = Stats {
            stream: StreamStats {
                session_id: WindowSessionId("m1".into()),
                frames_emitted: 3,
                frames_replaced: 0,
                keyframe_requests: 1,
                pending_frames: 0,
                bytes_emitted: 10,
                actions_dispatched: 0,
                action_frame_timeouts: 0,
                preference_updates: 0,
                input_events_dispatched: 0,
            },
            frames_since_attach: 2,
            last_frame_age_ms: Some(5),
            target_idle: true,
            keyframes_sent: 1,
            frames_dropped: 0,
            queued_video_bytes: 0,
            ack_rtt_ms: None,
            encoder_bitrate_kbps: Some(2000),
            effective_max_fps: 30,
            encoder: Some("openh264".into()),
            audio_tracks: vec![],
        };
        let value = serde_json::to_value(ServerMessage::Stats(Box::new(stats))).unwrap();
        assert_eq!(value["payload"]["frames_emitted"], 3);
        assert_eq!(value["payload"]["frames_since_attach"], 2);
        assert_eq!(value["payload"]["target_idle"], true);
        assert_eq!(value["payload"]["last_frame_age_ms"], 5);
    }

    #[test]
    fn close_codes_map_to_quic_errors() {
        assert_eq!(close_code::quic_error(close_code::NORMAL), 0);
        assert_eq!(close_code::quic_error(close_code::TICKET_INVALID), 0x401);
        assert_eq!(close_code::quic_error(close_code::TOO_MANY_SOCKETS), 0x429);
        assert_eq!(close_code::quic_error(close_code::INTERNAL), 0x500);
    }

    #[test]
    fn audio_config_round_trips() {
        let config = ServerMessage::AudioConfig(AudioConfig {
            track_id: 1,
            config_epoch: 1,
            direction: AudioDirection::Down,
            codec: AudioCodecName::PcmS16le,
            sample_rate_hz: 48_000,
            channels: 2,
            frame_ms: 20,
            bitrate_kbps: 0,
            fec: false,
            dtx: false,
            opus_pre_skip: 0,
            source: AudioSourceRef {
                source_id: "desktop".into(),
                kind: "desktop".into(),
                desktop_fallback: false,
            },
        });
        let text = encode_server_text(&config);
        assert!(text.contains("\"pcm_s16le\""));
        assert_eq!(decode_server_text(&text).unwrap(), config);
    }
}

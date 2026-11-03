// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Transport-independent client state for rcdp wire v2 (MEDIA.md).
//!
//! A v2 socket is attached to an existing media session with a ticket, so
//! this state machine starts at [`MediaPhase::Attaching`] and waits for the
//! server's `hello` and `session_opened`. Feed every text frame to
//! [`MediaClient::apply_text`] and every binary frame to
//! [`MediaClient::apply_binary`]; send what the builders return.
//!
//! The client keeps the decoder-side invariants: it drops dependent video
//! until a keyframe of the current codec epoch arrives, asks for a keyframe
//! (at most once per second) when it has to, tracks the authoritative
//! geometry epoch, detects audio loss per track, and keeps interactive
//! input contiguous (re-syncing on `input_sequence_gap`).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use cua_media_protocol::v2::{
    decode_server_text, encode_client_text, AudioConfig, ClientMessage, ErrorCode, FrameAck, Hello,
    ServerMessage, SessionOpened, Stats, WIRE_VERSION,
};
use cua_media_protocol::{
    InteractiveInputBatch, InteractiveInputEvent, StreamPreferences, SurfaceGeometry,
    VideoFrameDescriptor, WindowLifecycleEvent, WindowSessionId, WireHeader,
    MAX_INTERACTIVE_INPUT_EVENTS,
};
use cua_media_transport::audio::{
    decode_audio_packet, is_audio_packet, sequence_after, AudioPacketHeader,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaPhase {
    /// Socket open, waiting for `hello`.
    Attaching,
    /// `hello` received, waiting for `session_opened`.
    Greeted,
    /// Streaming.
    Open,
    /// The target closed or the session ended.
    Closed,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MediaEvent {
    Opened(SessionOpened),
    AudioConfigured(AudioConfig),
    Lifecycle(WindowLifecycleEvent),
    /// A video packet to hand to the decoder (already keyframe-gated).
    Video {
        descriptor: VideoFrameDescriptor,
        payload: Vec<u8>,
    },
    /// An audio packet for a configured track. `lost` counts packets missing
    /// before this one (decode FEC or conceal that many frames first).
    Audio {
        header: AudioPacketHeader,
        payload: Vec<u8>,
        lost: u32,
    },
    Stats(Box<Stats>),
    /// Any other server message (acks, action results, errors...).
    Message(ServerMessage),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaClientError(pub String);

impl std::fmt::Display for MediaClientError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for MediaClientError {}

#[derive(Debug)]
struct TrackState {
    config: AudioConfig,
    last_sequence: Option<u32>,
}

#[derive(Debug)]
pub struct MediaClient {
    phase: MediaPhase,
    hello: Option<Hello>,
    session: Option<SessionOpened>,
    geometry: SurfaceGeometry,
    geometry_epoch: u64,
    codec_epoch: Option<u64>,
    awaiting_keyframe: bool,
    last_sequence: Option<u64>,
    last_keyframe_request: Option<Instant>,
    next_input_sequence: u64,
    tracks: HashMap<u16, TrackState>,
}

impl Default for MediaClient {
    fn default() -> Self {
        Self::new(1)
    }
}

impl MediaClient {
    /// `first_input_sequence` may be any value; the server adopts it.
    pub fn new(first_input_sequence: u64) -> Self {
        Self {
            phase: MediaPhase::Attaching,
            hello: None,
            session: None,
            geometry: SurfaceGeometry {
                width_px: 0,
                height_px: 0,
                scale_factor: 1.0,
            },
            geometry_epoch: 0,
            codec_epoch: None,
            awaiting_keyframe: true,
            last_sequence: None,
            last_keyframe_request: None,
            next_input_sequence: first_input_sequence.max(1),
            tracks: HashMap::new(),
        }
    }

    pub fn phase(&self) -> MediaPhase {
        self.phase
    }

    pub fn hello(&self) -> Option<&Hello> {
        self.hello.as_ref()
    }

    pub fn session(&self) -> Option<&SessionOpened> {
        self.session.as_ref()
    }

    /// Authoritative frame geometry and its epoch (MEDIA.md §7).
    pub fn geometry(&self) -> (u64, &SurfaceGeometry) {
        (self.geometry_epoch, &self.geometry)
    }

    fn session_id(&self) -> WindowSessionId {
        self.session
            .as_ref()
            .map(|session| session.session_id.clone())
            .unwrap_or_else(|| WindowSessionId(String::new()))
    }

    /// Apply one text frame from the server.
    pub fn apply_text(&mut self, text: &str) -> Result<MediaEvent, MediaClientError> {
        let message = decode_server_text(text).map_err(MediaClientError)?;
        self.apply(message)
    }

    pub fn apply(&mut self, message: ServerMessage) -> Result<MediaEvent, MediaClientError> {
        match message {
            ServerMessage::Hello(hello) => {
                if self.phase != MediaPhase::Attaching {
                    return Err(MediaClientError("duplicate hello".into()));
                }
                if hello.selected_version != WIRE_VERSION {
                    return Err(MediaClientError(format!(
                        "server selected wire version {}",
                        hello.selected_version
                    )));
                }
                self.hello = Some(hello.clone());
                self.phase = MediaPhase::Greeted;
                Ok(MediaEvent::Message(ServerMessage::Hello(hello)))
            }
            ServerMessage::SessionOpened(opened) => {
                if self.phase != MediaPhase::Greeted {
                    return Err(MediaClientError("session_opened before hello".into()));
                }
                self.geometry = opened.geometry.clone();
                self.geometry_epoch = opened.geometry_epoch.0;
                self.session = Some(opened.clone());
                self.phase = MediaPhase::Open;
                Ok(MediaEvent::Opened(opened))
            }
            ServerMessage::AudioConfig(config) => {
                self.tracks.insert(
                    config.track_id,
                    TrackState {
                        config: config.clone(),
                        last_sequence: None,
                    },
                );
                Ok(MediaEvent::AudioConfigured(config))
            }
            ServerMessage::Lifecycle { event, .. } => {
                match &event {
                    WindowLifecycleEvent::GeometryChanged {
                        geometry_epoch,
                        geometry,
                    } => {
                        self.geometry_epoch = geometry_epoch.0;
                        self.geometry = geometry.clone();
                    }
                    WindowLifecycleEvent::Closed => self.phase = MediaPhase::Closed,
                    _ => {}
                }
                Ok(MediaEvent::Lifecycle(event))
            }
            ServerMessage::Stats(stats) => Ok(MediaEvent::Stats(stats)),
            ServerMessage::Error {
                code: ErrorCode::InputSequenceGap,
                expected_sequence: Some(expected),
                ..
            } => {
                self.next_input_sequence = expected;
                Ok(MediaEvent::Message(ServerMessage::error(
                    ErrorCode::InputSequenceGap,
                    format!("input re-synced to {expected}"),
                )))
            }
            other => Ok(MediaEvent::Message(other)),
        }
    }

    /// Apply one binary frame. Returns `None` for packets the decoder must
    /// not see (dependent video before a keyframe, stale audio epochs).
    pub fn apply_binary(&mut self, bytes: &[u8]) -> Result<Option<MediaEvent>, MediaClientError> {
        if is_audio_packet(bytes) {
            let (header, payload) =
                decode_audio_packet(bytes).map_err(|error| MediaClientError(error.to_string()))?;
            let Some(track) = self.tracks.get_mut(&header.track_id) else {
                return Ok(None);
            };
            if track.config.config_epoch != header.config_epoch {
                return Ok(None);
            }
            let lost = match track.last_sequence {
                Some(previous) if !sequence_after(header.sequence, previous) => return Ok(None),
                Some(previous) => header.sequence.wrapping_sub(previous).wrapping_sub(1),
                None => 0,
            };
            track.last_sequence = Some(header.sequence);
            return Ok(Some(MediaEvent::Audio {
                header,
                payload: payload.to_vec(),
                lost,
            }));
        }
        let (header, payload) = cua_media_transport::decode_packet(bytes)
            .map_err(|error| MediaClientError(error.to_string()))?;
        let WireHeader::Video(descriptor) = header else {
            return Ok(None);
        };
        if self
            .session
            .as_ref()
            .is_some_and(|session| session.session_id != descriptor.session_id)
        {
            return Ok(None);
        }
        if self.codec_epoch != Some(descriptor.codec_epoch.0) {
            if !descriptor.keyframe {
                self.awaiting_keyframe = true;
                return Ok(None);
            }
            self.codec_epoch = Some(descriptor.codec_epoch.0);
        }
        if let Some(last) = self.last_sequence {
            if descriptor.sequence.0 <= last {
                return Ok(None);
            }
        }
        if descriptor.keyframe {
            self.awaiting_keyframe = false;
        } else if self.awaiting_keyframe {
            return Ok(None);
        }
        self.last_sequence = Some(descriptor.sequence.0);
        Ok(Some(MediaEvent::Video {
            descriptor,
            payload,
        }))
    }

    /// True while dependent frames are being dropped.
    pub fn awaiting_keyframe(&self) -> bool {
        self.awaiting_keyframe
    }

    /// Call after a decode error: drop until the next keyframe.
    pub fn decoder_failed(&mut self) {
        self.awaiting_keyframe = true;
        self.codec_epoch = None;
    }

    /// A keyframe request, rate limited to one per second.
    pub fn request_keyframe(&mut self, now: Instant) -> Option<String> {
        if self
            .last_keyframe_request
            .is_some_and(|last| now.duration_since(last) < Duration::from_secs(1))
        {
            return None;
        }
        self.last_keyframe_request = Some(now);
        Some(encode_client_text(&ClientMessage::RequestKeyframe {
            session_id: self.session_id(),
        }))
    }

    /// Acknowledge a decoded frame (optional flow control).
    pub fn frame_ack(&self, sequence: u64, decode_us: Option<u32>) -> String {
        encode_client_text(&ClientMessage::FrameAck(FrameAck {
            session_id: self.session_id(),
            sequence,
            decode_us,
            decode_queue: None,
        }))
    }

    /// One interactive input batch per call (split beyond 256 events).
    pub fn input(
        &mut self,
        events: Vec<InteractiveInputEvent>,
    ) -> Result<Vec<String>, MediaClientError> {
        let mut frames = Vec::new();
        for chunk in events.chunks(MAX_INTERACTIVE_INPUT_EVENTS) {
            let batch = InteractiveInputBatch {
                session_id: self.session_id(),
                first_sequence: self.next_input_sequence,
                events: chunk.to_vec(),
            };
            let through = batch.validate().map_err(MediaClientError)?;
            self.next_input_sequence = through + 1;
            frames.push(encode_client_text(&ClientMessage::InteractiveInput(batch)));
        }
        Ok(frames)
    }

    pub fn set_preferences(
        &self,
        max_fps: u16,
        max_dimension: u32,
        bitrate_kbps: Option<u32>,
    ) -> String {
        encode_client_text(&ClientMessage::SetStreamPreferences(StreamPreferences {
            session_id: self.session_id(),
            max_fps,
            max_dimension,
            target_bitrate_kbps: bitrate_kbps,
        }))
    }

    pub fn get_stats(&self) -> String {
        encode_client_text(&ClientMessage::GetStats {
            session_id: self.session_id(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_media_protocol::v2::{encode_server_text, MediaTargetRef};
    use cua_media_protocol::{
        CodecEpoch, FrameSequence, GeometryEpoch, InputKeyState, SessionPolicy, TargetEpoch,
        VideoCodec, WindowGeometryControl,
    };

    fn opened() -> SessionOpened {
        SessionOpened {
            session_id: WindowSessionId("m1".into()),
            target: MediaTargetRef::Display {
                display_id: "0".into(),
            },
            target_epoch: TargetEpoch(1),
            geometry_epoch: GeometryEpoch(3),
            codec_epoch: CodecEpoch(1),
            geometry: SurfaceGeometry {
                width_px: 640,
                height_px: 480,
                scale_factor: 1.0,
            },
            codec: VideoCodec::H264,
            video: true,
            max_fps: 30,
            max_dimension: 0,
            target_bitrate_kbps: None,
            capabilities: vec![],
            action_capabilities: vec![],
            policy: SessionPolicy::BackgroundOnly,
            geometry_control: WindowGeometryControl::ObserveOnly,
        }
    }

    fn video(sequence: u64, epoch: u64, keyframe: bool) -> Vec<u8> {
        cua_media_transport::encode_packet(
            &WireHeader::Video(VideoFrameDescriptor {
                session_id: WindowSessionId("m1".into()),
                sequence: FrameSequence(sequence),
                geometry_epoch: GeometryEpoch(3),
                codec_epoch: CodecEpoch(epoch),
                width_px: 640,
                height_px: 480,
                capture_timestamp_us: sequence,
                encode_duration_us: None,
                codec: VideoCodec::H264,
                keyframe,
            }),
            &[0, 0, 0, 1, 0x65],
        )
        .unwrap()
    }

    fn attached() -> MediaClient {
        let mut client = MediaClient::new(5000);
        client
            .apply_text(&encode_server_text(&ServerMessage::Hello(Hello::server(
                vec![],
            ))))
            .unwrap();
        client
            .apply_text(&encode_server_text(&ServerMessage::SessionOpened(opened())))
            .unwrap();
        client
    }

    #[test]
    fn server_speaks_first_and_order_is_enforced() {
        let mut client = MediaClient::new(1);
        assert!(client
            .apply(ServerMessage::SessionOpened(opened()))
            .is_err());
        let client = attached();
        assert_eq!(client.phase(), MediaPhase::Open);
        assert_eq!(client.geometry().0, 3);
    }

    #[test]
    fn video_is_gated_until_a_keyframe_and_sequences_may_start_anywhere() {
        let mut client = attached();
        assert!(client
            .apply_binary(&video(900, 1, false))
            .unwrap()
            .is_none());
        assert!(matches!(
            client.apply_binary(&video(901, 1, true)).unwrap(),
            Some(MediaEvent::Video { .. })
        ));
        assert!(client
            .apply_binary(&video(902, 1, false))
            .unwrap()
            .is_some());
        // A new codec epoch needs its own keyframe.
        assert!(client
            .apply_binary(&video(903, 2, false))
            .unwrap()
            .is_none());
        assert!(client.apply_binary(&video(904, 2, true)).unwrap().is_some());
        client.decoder_failed();
        assert!(client
            .apply_binary(&video(905, 2, false))
            .unwrap()
            .is_none());
        let now = Instant::now();
        assert!(client.request_keyframe(now).is_some());
        assert!(client
            .request_keyframe(now + Duration::from_millis(10))
            .is_none());
    }

    #[test]
    fn audio_loss_is_counted_per_track_and_stale_epochs_dropped() {
        let mut client = attached();
        client
            .apply(ServerMessage::AudioConfig(AudioConfig {
                track_id: 1,
                config_epoch: 1,
                direction: cua_media_protocol::v2::AudioDirection::Down,
                codec: cua_media_protocol::v2::AudioCodecName::Opus,
                sample_rate_hz: 48_000,
                channels: 2,
                frame_ms: 20,
                bitrate_kbps: 64,
                fec: true,
                dtx: true,
                opus_pre_skip: 312,
                source: cua_media_protocol::v2::AudioSourceRef {
                    source_id: "desktop".into(),
                    kind: "desktop".into(),
                    desktop_fallback: false,
                },
            }))
            .unwrap();
        let packet = |sequence: u32, epoch: u8| {
            cua_media_transport::audio::encode_audio_packet(
                &AudioPacketHeader {
                    discontinuity: false,
                    dtx: false,
                    track_id: 1,
                    sequence,
                    pts_us: u64::from(sequence) * 20_000,
                    frame_samples: 960,
                    config_epoch: epoch,
                },
                &[1, 2, 3],
            )
        };
        assert!(matches!(
            client.apply_binary(&packet(u32::MAX, 1)).unwrap(),
            Some(MediaEvent::Audio { lost: 0, .. })
        ));
        assert!(matches!(
            client.apply_binary(&packet(2, 1)).unwrap(),
            Some(MediaEvent::Audio { lost: 2, .. })
        ));
        assert!(
            client.apply_binary(&packet(1, 1)).unwrap().is_none(),
            "late packets are dropped"
        );
        assert!(
            client.apply_binary(&packet(3, 9)).unwrap().is_none(),
            "stale config epoch"
        );
    }

    #[test]
    fn input_is_contiguous_and_resyncs_on_gap() {
        let mut client = attached();
        let key = |state| InteractiveInputEvent::Key {
            key: "a".into(),
            state,
            modifiers: vec![],
            repeat: false,
        };
        let frames = client
            .input(vec![key(InputKeyState::Down), key(InputKeyState::Up)])
            .unwrap();
        assert!(frames[0].contains("\"first_sequence\":5000"));
        let next = client.input(vec![key(InputKeyState::Down)]).unwrap();
        assert!(next[0].contains("\"first_sequence\":5002"));
        client
            .apply(ServerMessage::Error {
                code: ErrorCode::InputSequenceGap,
                message: "gap".into(),
                expected_sequence: Some(42),
            })
            .unwrap();
        let resynced = client.input(vec![key(InputKeyState::Up)]).unwrap();
        assert!(resynced[0].contains("\"first_sequence\":42"));
    }
}

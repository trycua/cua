// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `cua.env.v1.StreamService`: target discovery, media session negotiation
//! and ticket minting. Frames flow on the `/media` WebSocket (rcdp wire v2).

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use cua_media_protocol::v2::AudioCodecName;
use cua_media_protocol::{
    SessionPolicy as WireSessionPolicy, TargetEpoch, TargetHandle, VideoCodec,
    WindowGeometryControl,
};
use cua_proto::env::v1::{
    media_target, stream_service_server::StreamService, stream_target, AppInfo, AudioCodec,
    AudioEncoding, AudioSource, AudioSourceKind, AudioTrack, AudioUplinkGrant, CloseMediaRequest,
    CloseMediaResponse, GeometryControl, ListTargetsRequest, ListTargetsResponse, MediaCodec,
    MediaGeometry, NegotiatedAudio, NegotiatedAudioEncoding, OpenMediaRequest, OpenMediaResponse,
    PixelSize, Principal, PrincipalKind, Rect, RequestKeyframeRequest, RequestKeyframeResponse,
    SessionPolicy, SetPreferencesRequest, SetPreferencesResponse, StreamTarget,
};
use cua_spacesd_session::media::audio::{
    AudioSourceInfo, AudioSourceKind as SessionSourceKind, AudioTrackConfig,
};
use cua_spacesd_session::media::{
    AudioPreferenceUpdate, AudioRequest, MediaPrincipal, MediaTargetSpec, OpenMediaParams,
    UplinkRequest,
};
use tonic::{Request, Response, Status};

use super::caller_principal;
use super::status::{self, invalid, join_error, not_found};
use super::windows::{matches, window_info};
use super::{proto_display, DesktopState};
use cua_spacesd_server::TicketScope;

pub(crate) struct Stream(pub Arc<DesktopState>);

const DEFAULT_TICKET_TTL: Duration = Duration::from_secs(60);
const MAX_TICKET_TTL: Duration = Duration::from_secs(600);

fn media_codec(codec: &VideoCodec) -> MediaCodec {
    match codec {
        VideoCodec::H264 => MediaCodec::H264,
        VideoCodec::Bgra => MediaCodec::Bgra,
        VideoCodec::Png => MediaCodec::Png,
        VideoCodec::Unknown => MediaCodec::Unspecified,
    }
}

fn audio_codec(codec: AudioCodecName) -> AudioCodec {
    match codec {
        AudioCodecName::Opus => AudioCodec::Opus,
        AudioCodecName::PcmS16le => AudioCodec::PcmS16le,
        AudioCodecName::Unknown => AudioCodec::Unspecified,
    }
}

fn audio_source(source: &AudioSourceInfo) -> AudioSource {
    AudioSource {
        source_id: source.source_id.clone(),
        kind: match source.kind {
            SessionSourceKind::Desktop => AudioSourceKind::Desktop,
            SessionSourceKind::Application => AudioSourceKind::Application,
        } as i32,
        name: source.name.clone(),
        app: source.pid.map(|pid| AppInfo {
            name: source.name.clone(),
            app_id: String::new(),
            pid,
        }),
        windows: Vec::new(),
        available: source.available,
        limitation: source.limitation.clone().unwrap_or_default(),
        desktop_fallback: source.desktop_fallback,
    }
}

fn negotiated_encoding(config: &AudioTrackConfig) -> NegotiatedAudioEncoding {
    NegotiatedAudioEncoding {
        codec: audio_codec(config.codec) as i32,
        sample_rate_hz: config.sample_rate_hz,
        channels: u32::from(config.channels),
        bitrate_kbps: config.bitrate_kbps,
        fec: config.fec,
        dtx: config.dtx,
        frame_ms: u32::from(config.frame_ms),
    }
}

fn track_config(encoding: Option<&AudioEncoding>, uplink: bool) -> AudioTrackConfig {
    let encoding = encoding.cloned().unwrap_or_default();
    let codec = encoding
        .codecs
        .iter()
        .filter_map(|codec| AudioCodec::try_from(*codec).ok())
        .find_map(|codec| match codec {
            AudioCodec::Opus => Some(AudioCodecName::Opus),
            AudioCodec::PcmS16le => Some(AudioCodecName::PcmS16le),
            AudioCodec::Unspecified => None,
        })
        .unwrap_or(AudioCodecName::Opus);
    let fec = encoding.fec.unwrap_or(true);
    AudioTrackConfig {
        source_id: String::new(),
        codec,
        sample_rate_hz: encoding.sample_rate_hz,
        channels: match encoding.channels {
            0 if uplink => 1,
            0 => 2,
            channels => channels.min(2) as u8,
        },
        frame_ms: encoding.frame_ms as u16,
        bitrate_kbps: encoding.bitrate_kbps,
        fec,
        dtx: encoding.dtx.unwrap_or(true),
        expected_loss_percent: match encoding.expected_loss_percent {
            0 if fec => 10,
            loss => loss.min(100) as u8,
        },
    }
}

/// The principal a media session's input is attributed to: the viewer's
/// presence participant when it names one (`presence_participant_id`), else
/// the connection's. An authenticated caller may only name a participant
/// with its own principal id, so it cannot borrow someone else's identity.
pub(super) fn viewer_principal(
    presence: &super::presence::PresenceHub,
    connection: Option<Principal>,
    authenticated: Option<&Principal>,
    participant_id: &str,
) -> Option<Principal> {
    if participant_id.is_empty() {
        return connection;
    }
    match presence.principal(participant_id) {
        Some(participant) if authenticated.is_none_or(|caller| caller.id == participant.id) => {
            Some(participant)
        }
        _ => connection,
    }
}

fn media_principal(principal: &Option<Principal>) -> MediaPrincipal {
    match principal {
        Some(principal) if !principal.id.is_empty() => MediaPrincipal {
            id: principal.id.clone(),
            name: if principal.display_name.is_empty() {
                principal.id.clone()
            } else {
                principal.display_name.clone()
            },
            color: principal.color.clone(),
            agent: principal.kind == PrincipalKind::Agent as i32,
        },
        _ => MediaPrincipal::anonymous(),
    }
}

fn percent_encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

#[tonic::async_trait]
impl StreamService for Stream {
    async fn list_targets(
        &self,
        request: Request<ListTargetsRequest>,
    ) -> Result<Response<ListTargetsResponse>, Status> {
        let principal = caller_principal(&request);
        let request = request.into_inner();
        let state = self.0.clone();
        let (displays, windows) = tokio::task::spawn_blocking(move || {
            let displays = state.backend.displays();
            let windows = if request.include_windows {
                state.backend.windows()
            } else {
                Ok(Vec::new())
            };
            (displays, windows)
        })
        .await
        .map_err(join_error)?;
        let providers = self.0.media.providers();
        let audio_sources: Vec<AudioSourceInfo> = providers
            .audio
            .as_ref()
            .map(|audio| audio.sources())
            .unwrap_or_default();
        let desktop_audio = audio_sources
            .iter()
            .find(|source| source.kind == SessionSourceKind::Desktop)
            .map(audio_source);
        let mut targets = Vec::new();
        match displays {
            Ok(displays) => {
                let mut displays = displays;
                displays.sort_by_key(|display| !display.primary);
                for display in displays {
                    targets.push(StreamTarget {
                        target: Some(stream_target::Target::Display(proto_display(display))),
                        available: true,
                        limitation: String::new(),
                        audio: desktop_audio.clone(),
                    });
                }
            }
            Err(error) => {
                // Platforms without desktop targets still list windows.
                tracing::debug!(target: "cua_spacesd_client::stream", %error, "no display targets");
            }
        }
        let filter = request.window_filter.unwrap_or_default();
        for window in windows.map_err(status::provider)? {
            let mut window_filter = filter.clone();
            window_filter.include_system = false;
            if !matches(&window_filter, &window) {
                continue;
            }
            let paired = audio_sources
                .iter()
                .find(|source| source.pid == Some(window.pid) && window.pid != 0)
                .map(audio_source)
                .or_else(|| {
                    desktop_audio.clone().map(|mut desktop| {
                        desktop.desktop_fallback = true;
                        desktop.limitation =
                            "no per-app audio for this window; this is the desktop mix".into();
                        desktop
                    })
                });
            targets.push(StreamTarget {
                available: window.on_screen,
                limitation: if window.on_screen {
                    String::new()
                } else {
                    "the window is minimized or hidden".into()
                },
                target: Some(stream_target::Target::Window(window_info(&window))),
                audio: paired,
            });
        }
        let codecs = vec![MediaCodec::H264 as i32, MediaCodec::Bgra as i32];
        let audio_codecs = providers
            .audio
            .as_ref()
            .map(|audio| {
                audio
                    .codecs()
                    .into_iter()
                    .map(|codec| audio_codec(codec) as i32)
                    .collect()
            })
            .unwrap_or_default();
        let uplink_allowed =
            providers.audio.is_some() && self.0.ctx.audio_uplink_allowed(principal.as_ref());
        Ok(Response::new(ListTargetsResponse {
            targets,
            codecs,
            audio_sources: audio_sources.iter().map(audio_source).collect(),
            audio_codecs,
            audio_uplink_allowed: uplink_allowed,
            audio_uplink_limitation: if uplink_allowed {
                String::new()
            } else if providers.audio.is_none() {
                "no audio backend on this guest".into()
            } else {
                "this principal is not allowed to open an audio uplink (SystemService.Init audio_uplink)".into()
            },
        }))
    }

    async fn open_media(
        &self,
        request: Request<OpenMediaRequest>,
    ) -> Result<Response<OpenMediaResponse>, Status> {
        let principal = caller_principal(&request);
        let viewer = cua_spacesd_server::caller(&request).viewer;
        // A principal the server authenticated (relay-asserted or a viewer
        // ticket), as opposed to one the caller merely declared.
        let authenticated = cua_spacesd_server::caller(&request).principal;
        let mut request = request.into_inner();
        // A viewer ticket caps the session policy (and the uplink, below).
        if let Some(grant) = viewer.as_deref() {
            request.policy = grant.clamp_policy(request.policy);
        }
        let target = match request.target.and_then(|target| target.target) {
            Some(media_target::Target::DisplayId(id)) => MediaTargetSpec::Display {
                display_id: if id.is_empty() { "primary".into() } else { id },
            },
            Some(media_target::Target::Window(window)) => {
                let record = self.0.window(&window)?;
                MediaTargetSpec::Window {
                    handle: TargetHandle(record.handle.0),
                    epoch: TargetEpoch(record.epoch.0),
                }
            }
            None => return Err(invalid("target is required")),
        };
        let codecs = request
            .codecs
            .iter()
            .filter_map(|codec| MediaCodec::try_from(*codec).ok())
            .filter_map(|codec| match codec {
                MediaCodec::H264 => Some(VideoCodec::H264),
                MediaCodec::Bgra => Some(VideoCodec::Bgra),
                MediaCodec::Png => Some(VideoCodec::Png),
                MediaCodec::Unspecified => None,
            })
            .collect::<Vec<_>>();
        let policy =
            match SessionPolicy::try_from(request.policy).unwrap_or(SessionPolicy::Unspecified) {
                SessionPolicy::BackgroundOnly => WireSessionPolicy::BackgroundOnly,
                SessionPolicy::AllowActivation => WireSessionPolicy::AllowActivation,
                SessionPolicy::ViewOnly | SessionPolicy::Unspecified => WireSessionPolicy::ViewOnly,
            };
        let geometry_control = match GeometryControl::try_from(request.geometry_control)
            .unwrap_or(GeometryControl::Unspecified)
        {
            GeometryControl::Bidirectional => WindowGeometryControl::Bidirectional,
            _ => WindowGeometryControl::ObserveOnly,
        };
        if request.bitrate_kbps != 0 && !(250..=100_000).contains(&request.bitrate_kbps) {
            return Err(invalid("bitrate_kbps must be between 250 and 100000"));
        }
        let ttl = request
            .ticket_ttl
            .and_then(|duration| Duration::try_from(duration).ok())
            .filter(|duration| !duration.is_zero())
            .unwrap_or(DEFAULT_TICKET_TTL)
            .min(MAX_TICKET_TTL);
        // A window's paired audio source is its app's stream when the audio
        // backend can capture per app; otherwise the desktop mix.
        let paired_source = match (&request.audio, &target) {
            (Some(audio), MediaTargetSpec::Window { handle, .. })
                if audio.source_ids.is_empty() =>
            {
                let pid = self.0.backend.windows().ok().and_then(|windows| {
                    windows
                        .into_iter()
                        .find(|window| window.handle == *handle)
                        .map(|window| window.pid)
                });
                self.0
                    .media
                    .providers()
                    .audio
                    .as_ref()
                    .and_then(|provider| {
                        provider
                            .sources()
                            .into_iter()
                            .find(|source| source.pid.is_some() && source.pid == pid)
                            .map(|source| source.source_id)
                    })
            }
            _ => None,
        };
        let audio = request
            .audio
            .as_ref()
            .filter(|audio| audio.enabled || audio.uplink.as_ref().is_some_and(|u| u.enabled))
            .map(|audio| AudioRequest {
                downlink: audio.enabled,
                source_ids: if audio.source_ids.is_empty() {
                    paired_source.clone().into_iter().collect()
                } else {
                    audio.source_ids.clone()
                },
                config: track_config(audio.encoding.as_ref(), false),
                uplink: audio
                    .uplink
                    .as_ref()
                    .filter(|uplink| uplink.enabled)
                    .map(|uplink| UplinkRequest {
                        config: track_config(uplink.encoding.as_ref(), true),
                        virtual_source_name: if uplink.virtual_source_name.is_empty() {
                            "cua-uplink".into()
                        } else {
                            uplink.virtual_source_name.clone()
                        },
                        permitted: if viewer.as_deref().is_some_and(|grant| !grant.audio_uplink) {
                            Err("the viewer ticket does not grant the microphone".into())
                        } else if self.0.ctx.audio_uplink_allowed(principal.as_ref()) {
                            Ok(())
                        } else {
                            Err("principal not allowed".into())
                        },
                    }),
            });
        let audio_downlink = request.audio.as_ref().is_some_and(|audio| audio.enabled);
        let params = OpenMediaParams {
            target,
            codecs,
            max_fps: request.max_fps.min(240) as u16,
            max_dimension: request.max_dimension,
            bitrate_kbps: (request.bitrate_kbps != 0).then_some(request.bitrate_kbps),
            policy,
            geometry_control,
            principal: media_principal(&viewer_principal(
                &self.0.presence,
                principal.clone(),
                authenticated.as_ref(),
                &request.presence_participant_id,
            )),
            disable_video: request.disable_video,
            audio,
            quic: request.prefer_quic && self.0.quic.lock().unwrap().is_some(),
            attach_deadline: Instant::now() + ttl,
        };
        let media = self.0.media.clone();
        let opened = tokio::task::spawn_blocking(move || media.open(params))
            .await
            .map_err(join_error)?
            .map_err(status::media)?;
        let session_id = opened.session.id().to_owned();
        let (ticket, expires_at) =
            self.0
                .ctx
                .mint_ticket(TicketScope::Media, &session_id, principal.as_ref(), ttl);
        self.0
            .ctx
            .media_sessions()
            .store(self.0.media.session_count() as u32, Ordering::Relaxed);
        let (x, y, width, height) = opened.logical_bounds.unwrap_or_default();
        let geometry = &opened.opened.geometry;
        let audio = opened.audio.as_ref().map(|audio| NegotiatedAudio {
            tracks: if audio_downlink {
                audio
                    .tracks
                    .iter()
                    .map(|track| AudioTrack {
                        track_id: u32::from(track.track_id),
                        source: Some(audio_source(&track.source)),
                    })
                    .collect()
            } else {
                Vec::new()
            },
            encoding: Some(negotiated_encoding(&audio.config)),
            uplink: audio.uplink.as_ref().map(|grant| AudioUplinkGrant {
                granted: grant.granted,
                denied_reason: grant.denied_reason.clone().unwrap_or_default(),
                track_id: u32::from(grant.track_id),
                encoding: Some(negotiated_encoding(&grant.config)),
                virtual_source_name: grant.virtual_source_name.clone(),
            }),
        });
        Ok(Response::new(OpenMediaResponse {
            media_session_id: session_id,
            ws_path: format!(
                "{}?ticket={}",
                cua_proto::metadata::MEDIA_WS_PATH,
                percent_encode(&ticket)
            ),
            ticket,
            ticket_expires_at: Some(super::timestamp(expires_at.max(SystemTime::UNIX_EPOCH))),
            quic: if request.prefer_quic {
                self.0
                    .quic
                    .lock()
                    .unwrap()
                    .as_ref()
                    .map(|quic| cua_proto::env::v1::QuicEndpoint {
                        port: u32::from(quic.port),
                        certificate_sha256: quic.certificate_sha256.clone(),
                        alpn: String::from_utf8_lossy(cua_media_protocol::v2::QUIC_ALPN)
                            .into_owned(),
                    })
            } else {
                None
            },
            codec: media_codec(&opened.opened.codec) as i32,
            max_fps: u32::from(opened.opened.max_fps),
            max_dimension: opened.opened.max_dimension,
            bitrate_kbps: opened.opened.target_bitrate_kbps.unwrap_or(0),
            policy: match opened.opened.policy {
                WireSessionPolicy::ViewOnly => SessionPolicy::ViewOnly,
                WireSessionPolicy::BackgroundOnly => SessionPolicy::BackgroundOnly,
                WireSessionPolicy::AllowActivation => SessionPolicy::AllowActivation,
            } as i32,
            geometry_control: match opened.opened.geometry_control {
                WindowGeometryControl::Bidirectional => GeometryControl::Bidirectional,
                _ => GeometryControl::ObserveOnly,
            } as i32,
            geometry: Some(MediaGeometry {
                frame_size: Some(PixelSize {
                    width: geometry.width_px,
                    height: geometry.height_px,
                }),
                scale: if width > 0.0 {
                    f64::from(geometry.width_px) / width
                } else {
                    geometry.scale_factor
                },
                logical_bounds: Some(Rect {
                    x,
                    y,
                    width,
                    height,
                }),
                geometry_epoch: opened.opened.geometry_epoch.0,
            }),
            wire_version: cua_proto::MEDIA_WIRE_VERSION,
            audio,
        }))
    }

    async fn set_preferences(
        &self,
        request: Request<SetPreferencesRequest>,
    ) -> Result<Response<SetPreferencesResponse>, Status> {
        let request = request.into_inner();
        let session = self
            .0
            .media
            .session(&request.media_session_id)
            .ok_or_else(|| not_found("unknown media session"))?;
        if request.bitrate_kbps != 0 && !(250..=100_000).contains(&request.bitrate_kbps) {
            return Err(invalid("bitrate_kbps must be between 250 and 100000"));
        }
        if request.audio_frame_ms != 0 && ![10, 20, 40, 60].contains(&request.audio_frame_ms) {
            return Err(invalid("audio_frame_ms must be 10, 20, 40 or 60"));
        }
        let video_session = session.clone();
        let (max_fps, max_dimension, bitrate) = tokio::task::spawn_blocking(move || {
            video_session.set_preferences(
                request.max_fps.min(240) as u16,
                request.max_dimension,
                request.bitrate_kbps,
            )
        })
        .await
        .map_err(join_error)?
        .map_err(status::media)?;
        let audio = session.set_audio_preferences(AudioPreferenceUpdate {
            enabled: request.audio_enabled,
            bitrate_kbps: request.audio_bitrate_kbps,
            fec: request.audio_fec,
            dtx: request.audio_dtx,
            frame_ms: request.audio_frame_ms as u16,
            expected_loss_percent: request
                .audio_expected_loss_percent
                .map(|loss| loss.min(100) as u8),
            uplink_muted: request.audio_uplink_muted,
        });
        Ok(Response::new(SetPreferencesResponse {
            max_fps: u32::from(max_fps),
            max_dimension,
            bitrate_kbps: bitrate,
            audio_enabled: session.audio_enabled(),
            audio_encoding: audio.as_ref().map(negotiated_encoding),
            audio_uplink_muted: session.uplink_muted(),
        }))
    }

    async fn request_keyframe(
        &self,
        request: Request<RequestKeyframeRequest>,
    ) -> Result<Response<RequestKeyframeResponse>, Status> {
        let id = request.into_inner().media_session_id;
        let session = self
            .0
            .media
            .session(&id)
            .ok_or_else(|| not_found("unknown media session"))?;
        session.request_keyframe_now();
        Ok(Response::new(RequestKeyframeResponse {}))
    }

    async fn close_media(
        &self,
        request: Request<CloseMediaRequest>,
    ) -> Result<Response<CloseMediaResponse>, Status> {
        let id = request.into_inner().media_session_id;
        if !self
            .0
            .media
            .close(&id, cua_media_protocol::v2::close_code::SESSION_CLOSED)
        {
            return Err(not_found("unknown media session"));
        }
        self.0
            .ctx
            .media_sessions()
            .store(self.0.media.session_count() as u32, Ordering::Relaxed);
        Ok(Response::new(CloseMediaResponse {}))
    }
}

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Loopback fake v2 server (gRPC StreamService + `/media` WebSocket + QUIC)
//! driving [`V2Link`]. Host-safe: no display, no audio device.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::ws::{Message as AxumMessage, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use cua_media_protocol::v2::{self, encode_server_text, AudioConfig};
use cua_media_protocol::{
    ClientMessage, CodecEpoch, FrameSequence, GeometryEpoch, OpenSession, SessionPolicy,
    TargetEpoch, TargetHandle, VideoCodec, VideoFrameDescriptor, WindowGeometryControl,
    WindowSessionId, WireHeader,
};
use cua_media_transport::audio::{encode_audio_packet, AudioPacketHeader};
use cua_proto::env::v1 as pb;

use super::*;

const TOKEN: &str = "secret-token";
const TICKET: &str = "ticket-123";
const SESSION: &str = "media-1";

#[derive(Default)]
struct Recorded {
    opens: Vec<pb::OpenMediaRequest>,
    closes: Vec<String>,
    client: Vec<v2::ClientMessage>,
}

#[derive(Clone)]
struct Fake {
    recorded: Arc<Mutex<Recorded>>,
    quic: Arc<Mutex<Option<(u16, String)>>>,
}

fn authorized<T>(request: &tonic::Request<T>) -> Result<(), tonic::Status> {
    let expected = format!("Bearer {TOKEN}");
    match request
        .metadata()
        .get("authorization")
        .and_then(|value| value.to_str().ok())
    {
        Some(value) if value == expected => Ok(()),
        _ => Err(tonic::Status::unauthenticated("bad token")),
    }
}

#[tonic::async_trait]
impl pb::stream_service_server::StreamService for Fake {
    async fn list_targets(
        &self,
        request: tonic::Request<pb::ListTargetsRequest>,
    ) -> Result<tonic::Response<pb::ListTargetsResponse>, tonic::Status> {
        authorized(&request)?;
        Ok(tonic::Response::new(pb::ListTargetsResponse {
            targets: vec![
                pb::StreamTarget {
                    target: Some(pb::stream_target::Target::Display(pb::Display {
                        id: "0".into(),
                        name: "Main".into(),
                        primary: true,
                        native_size: Some(pb::PixelSize {
                            width: 1280,
                            height: 720,
                        }),
                        scale_factor: 1.0,
                        ..Default::default()
                    })),
                    available: true,
                    ..Default::default()
                },
                pb::StreamTarget {
                    target: Some(pb::stream_target::Target::Window(pb::WindowInfo {
                        r#ref: Some(pb::WindowRef {
                            id: "w1".into(),
                            epoch: 3,
                        }),
                        title: "Editor".into(),
                        app: Some(pb::AppInfo {
                            name: "Fixture".into(),
                            ..Default::default()
                        }),
                        bounds: Some(pb::Rect {
                            x: 10.0,
                            y: 20.0,
                            width: 400.0,
                            height: 300.0,
                        }),
                        on_screen: true,
                        ..Default::default()
                    })),
                    available: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        }))
    }

    async fn open_media(
        &self,
        request: tonic::Request<pb::OpenMediaRequest>,
    ) -> Result<tonic::Response<pb::OpenMediaResponse>, tonic::Status> {
        authorized(&request)?;
        let request = request.into_inner();
        let quic = if request.prefer_quic {
            self.quic
                .lock()
                .unwrap()
                .clone()
                .map(|(port, certificate_sha256)| pb::QuicEndpoint {
                    port: u32::from(port),
                    certificate_sha256,
                    alpn: "rcdp/2".into(),
                })
        } else {
            None
        };
        self.recorded.lock().unwrap().opens.push(request);
        Ok(tonic::Response::new(pb::OpenMediaResponse {
            media_session_id: SESSION.into(),
            ticket: TICKET.into(),
            ws_path: format!("/media?ticket={TICKET}"),
            quic,
            codec: pb::MediaCodec::Bgra as i32,
            wire_version: 2,
            ..Default::default()
        }))
    }

    async fn set_preferences(
        &self,
        _: tonic::Request<pb::SetPreferencesRequest>,
    ) -> Result<tonic::Response<pb::SetPreferencesResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("fake"))
    }

    async fn request_keyframe(
        &self,
        _: tonic::Request<pb::RequestKeyframeRequest>,
    ) -> Result<tonic::Response<pb::RequestKeyframeResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("fake"))
    }

    async fn close_media(
        &self,
        request: tonic::Request<pb::CloseMediaRequest>,
    ) -> Result<tonic::Response<pb::CloseMediaResponse>, tonic::Status> {
        authorized(&request)?;
        self.recorded
            .lock()
            .unwrap()
            .closes
            .push(request.into_inner().media_session_id);
        Ok(tonic::Response::new(pb::CloseMediaResponse::default()))
    }
}

// ------------------------------------------------------------ media script

fn session_id() -> WindowSessionId {
    WindowSessionId(SESSION.into())
}

fn greeting() -> Vec<v2::ServerMessage> {
    vec![
        v2::ServerMessage::Hello(v2::Hello::server(vec!["input.interactive.v1".into()])),
        v2::ServerMessage::SessionOpened(v2::SessionOpened {
            session_id: session_id(),
            target: v2::MediaTargetRef::Display {
                display_id: "0".into(),
            },
            target_epoch: TargetEpoch(1),
            geometry_epoch: GeometryEpoch(1),
            codec_epoch: CodecEpoch(1),
            geometry: cua_media_protocol::SurfaceGeometry {
                width_px: 8,
                height_px: 8,
                scale_factor: 1.0,
            },
            codec: VideoCodec::Bgra,
            video: true,
            max_fps: 30,
            max_dimension: 1920,
            target_bitrate_kbps: None,
            capabilities: vec!["video.preferences.runtime".into()],
            action_capabilities: Vec::new(),
            policy: SessionPolicy::BackgroundOnly,
            geometry_control: WindowGeometryControl::ObserveOnly,
        }),
        v2::ServerMessage::AudioConfig(AudioConfig {
            track_id: 1,
            config_epoch: 1,
            direction: v2::AudioDirection::Down,
            codec: v2::AudioCodecName::Opus,
            sample_rate_hz: 48_000,
            channels: 2,
            frame_ms: 20,
            bitrate_kbps: 64,
            fec: false,
            dtx: false,
            opus_pre_skip: 312,
            source: v2::AudioSourceRef {
                source_id: "desktop".into(),
                kind: "desktop".into(),
                desktop_fallback: false,
            },
        }),
    ]
}

fn keyframe(sequence: u64) -> Vec<u8> {
    cua_media_transport::encode_packet(
        &WireHeader::Video(VideoFrameDescriptor {
            session_id: session_id(),
            sequence: FrameSequence(sequence),
            geometry_epoch: GeometryEpoch(1),
            codec_epoch: CodecEpoch(1),
            width_px: 8,
            height_px: 8,
            capture_timestamp_us: sequence,
            encode_duration_us: None,
            codec: VideoCodec::Bgra,
            keyframe: true,
        }),
        &[7u8; 8 * 8 * 4],
    )
    .unwrap()
}

/// Ten 20 ms Opus packets with sequence 4 missing (one concealed frame).
fn audio_packets() -> Vec<Vec<u8>> {
    use cua_media_codec::audio::codec::{open_audio_encoder, AudioEncodingConfig};
    let mut encoder = open_audio_encoder(&AudioEncodingConfig {
        dtx: false,
        fec: false,
        ..AudioEncodingConfig::downlink()
    })
    .unwrap();
    (0u32..10)
        .filter(|sequence| *sequence != 4)
        .map(|sequence| {
            let pcm: Vec<i16> = (0..960 * 2)
                .map(|index| {
                    (((index / 2) as f32 / 48_000.0 * 440.0 * std::f32::consts::TAU).sin()
                        * 8_000.0) as i16
                })
                .collect();
            let encoded = encoder.encode(&pcm).unwrap();
            encode_audio_packet(
                &AudioPacketHeader {
                    discontinuity: false,
                    dtx: false,
                    track_id: 1,
                    sequence,
                    pts_us: u64::from(sequence) * 20_000,
                    frame_samples: 960,
                    config_epoch: 1,
                },
                &encoded.data,
            )
        })
        .collect()
}

/// What the fake sends back for a client message.
fn reply(message: &v2::ClientMessage) -> Option<Vec<u8>> {
    matches!(message, v2::ClientMessage::RequestKeyframe { .. }).then(|| keyframe(2))
}

fn marker() -> String {
    encode_server_text(&v2::ServerMessage::KeyframeRequested {
        session_id: session_id(),
    })
}

// ------------------------------------------------------------ WebSocket

async fn media(
    State(fake): State<Fake>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    let offered = headers
        .get("sec-websocket-protocol")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    if !offered
        .split(',')
        .any(|protocol| protocol.trim() == format!("cua.ticket.{TICKET}"))
    {
        return (StatusCode::UNAUTHORIZED, "bad ticket").into_response();
    }
    upgrade
        .protocols(["rcdp.v2"])
        .on_upgrade(move |socket| serve_ws(fake, socket))
        .into_response()
}

async fn serve_ws(fake: Fake, mut socket: WebSocket) {
    for message in greeting() {
        let _ = socket
            .send(AxumMessage::Text(encode_server_text(&message).into()))
            .await;
    }
    let _ = socket.send(AxumMessage::Binary(keyframe(1).into())).await;
    for packet in audio_packets() {
        let _ = socket.send(AxumMessage::Binary(packet.into())).await;
    }
    let _ = socket.send(AxumMessage::Text(marker().into())).await;
    for _ in 0..64 {
        let Ok(Some(Ok(AxumMessage::Text(text)))) =
            tokio::time::timeout(Duration::from_secs(10), socket.recv()).await
        else {
            return;
        };
        let Ok(message) = v2::decode_client_text(&text) else {
            return;
        };
        let answer = reply(&message);
        fake.recorded.lock().unwrap().client.push(message);
        if let Some(packet) = answer {
            let _ = socket.send(AxumMessage::Binary(packet.into())).await;
        }
    }
}

// ------------------------------------------------------------ QUIC

async fn serve_quic(fake: Fake, endpoint: quinn::Endpoint) {
    while let Some(incoming) = endpoint.accept().await {
        let fake = fake.clone();
        tokio::spawn(async move {
            let Ok(connection) = incoming.await else {
                return;
            };
            let Ok((mut send, mut receive)) = connection.accept_bi().await else {
                return;
            };
            let first = cua_media_transport::quic::read_message(&mut receive)
                .await
                .ok()
                .flatten()
                .unwrap_or_default();
            if v2::decode_client_text(&String::from_utf8_lossy(&first))
                != Ok(v2::ClientMessage::Ticket {
                    ticket: TICKET.into(),
                })
            {
                connection.close(
                    v2::close_code::quic_error(v2::close_code::TICKET_INVALID).into(),
                    b"",
                );
                return;
            }
            for message in greeting() {
                let _ = cua_media_transport::quic::write_message(
                    &mut send,
                    encode_server_text(&message).as_bytes(),
                )
                .await;
            }
            let max = connection.max_datagram_size().unwrap_or(1200);
            let send_video = |packet_id: u64, packet: Vec<u8>| {
                for fragment in
                    cua_media_transport::fragment_video_packet_v2(packet_id, true, &packet, max)
                        .unwrap()
                {
                    let _ = connection.send_datagram(bytes::Bytes::from(fragment));
                }
            };
            send_video(1, keyframe(1));
            for packet in audio_packets() {
                let _ = connection.send_datagram(bytes::Bytes::from(packet));
            }
            // Datagrams are unordered with the stream: give them a head start.
            tokio::time::sleep(Duration::from_millis(100)).await;
            let _ = cua_media_transport::quic::write_message(&mut send, marker().as_bytes()).await;
            for _ in 0..64 {
                let Ok(Ok(Some(bytes))) = tokio::time::timeout(
                    Duration::from_secs(10),
                    cua_media_transport::quic::read_message(&mut receive),
                )
                .await
                else {
                    return;
                };
                let Ok(message) = v2::decode_client_text(&String::from_utf8_lossy(&bytes)) else {
                    return;
                };
                let answer = reply(&message);
                fake.recorded.lock().unwrap().client.push(message);
                if let Some(packet) = answer {
                    send_video(2, packet);
                }
            }
        });
    }
}

// ------------------------------------------------------------ harness

async fn start() -> (String, Fake) {
    let fake = Fake {
        recorded: Arc::default(),
        quic: Arc::default(),
    };
    let identity = cua_media_transport::quic::QuicIdentity::generate().unwrap();
    let endpoint = quinn::Endpoint::server(
        identity.server_config().unwrap(),
        "127.0.0.1:0".parse().unwrap(),
    )
    .unwrap();
    *fake.quic.lock().unwrap() = Some((
        endpoint.local_addr().unwrap().port(),
        identity.certificate_sha256(),
    ));
    tokio::spawn(serve_quic(fake.clone(), endpoint));
    let grpc = tonic::service::Routes::new(pb::stream_service_server::StreamServiceServer::new(
        fake.clone(),
    ))
    .into_axum_router();
    let router = axum::Router::new()
        .route("/media", axum::routing::get(media))
        .with_state(fake.clone())
        .merge(grpc);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (format!("http://{address}"), fake)
}

type Written = (usize, Option<(u32, u16)>);

#[derive(Clone, Default)]
struct Collect(Arc<Mutex<Written>>);

impl AudioOutput for Collect {
    fn write(&mut self, samples: &[i16], sample_rate: u32, channels: u16) {
        let mut state = self.0.lock().unwrap();
        state.0 += samples.len();
        state.1 = Some((sample_rate, channels));
    }
}

async fn next(link: &mut V2Link) -> V2Event {
    tokio::time::timeout(Duration::from_secs(10), link.next())
        .await
        .expect("event within 10 s")
        .unwrap()
}

async fn next_video(link: &mut V2Link) -> VideoFrameDescriptor {
    for _ in 0..32 {
        if let V2Event::Packet(WireHeader::Video(descriptor), payload) = next(link).await {
            assert_eq!(payload.len(), 8 * 8 * 4);
            return descriptor;
        }
    }
    panic!("no video within 32 events");
}

fn open_session(window: &str, epoch: u64) -> ClientMessage {
    ClientMessage::OpenSession(OpenSession {
        window: TargetHandle(window.into()),
        target_epoch: TargetEpoch(epoch),
        accepted_codecs: vec![VideoCodec::H264, VideoCodec::Bgra],
        max_fps: 30,
        max_dimension: 1920,
        target_bitrate_kbps: Some(4_000),
        policy: SessionPolicy::BackgroundOnly,
        geometry_control: WindowGeometryControl::ObserveOnly,
    })
}

async fn full_session(url: &str, fake: &Fake, window: &str, epoch: u64) {
    let output = Collect::default();
    let mut link = V2Link::connect(url, Some(TOKEN), Some(Box::new(output.clone())))
        .await
        .unwrap();
    link.send(ClientMessage::Authenticate {
        token: TOKEN.into(),
    })
    .await
    .unwrap();
    assert!(matches!(
        next(&mut link).await,
        V2Event::Server(ServerMessage::Authenticated)
    ));
    link.send(ClientMessage::Hello(Hello::default()))
        .await
        .unwrap();
    assert!(matches!(
        next(&mut link).await,
        V2Event::Server(ServerMessage::Hello(_))
    ));
    link.send(ClientMessage::ListWindows {
        on_screen_only: true,
    })
    .await
    .unwrap();
    let V2Event::Server(ServerMessage::Windows { windows }) = next(&mut link).await else {
        panic!("expected windows")
    };
    let handles: Vec<_> = windows
        .iter()
        .map(|window| window.window.0.as_str())
        .collect();
    assert_eq!(handles, ["display:0", "w1"]);

    link.send(open_session(window, epoch)).await.unwrap();
    let V2Event::Server(ServerMessage::SessionOpened(opened)) = next(&mut link).await else {
        panic!("expected session_opened")
    };
    assert_eq!(opened.session_id, session_id());
    assert!(opened
        .capabilities
        .iter()
        .any(|capability| capability == "input.interactive.v1"));

    // Keyframe on attach, then audio decoded into the output.
    let first = next_video(&mut link).await;
    assert!(first.keyframe);
    assert!(matches!(
        next(&mut link).await,
        V2Event::Server(ServerMessage::KeyframeRequested { .. })
    ));
    let stats = link.audio_stats();
    assert_eq!(stats.packets, 9, "{stats:?}");
    assert_eq!(stats.concealed_frames, 1, "{stats:?}");
    let (samples, format) = *output.0.lock().unwrap();
    assert_eq!(
        samples,
        10 * 960 * 2,
        "nine decoded frames plus one concealed"
    );
    assert_eq!(format, Some((48_000, 2)));

    // Keyframe request round trip.
    link.send(ClientMessage::RequestKeyframe {
        session_id: session_id(),
    })
    .await
    .unwrap();
    let again = next_video(&mut link).await;
    assert!(again.keyframe);
    assert_eq!(again.sequence, FrameSequence(2));

    link.send(ClientMessage::CloseSession {
        session_id: session_id(),
    })
    .await
    .unwrap();
    let recorded = fake.recorded.lock().unwrap();
    assert_eq!(recorded.closes, [SESSION]);
    assert!(recorded
        .client
        .iter()
        .any(|message| matches!(message, v2::ClientMessage::RequestKeyframe { .. })));
    let open = recorded.opens.last().unwrap();
    assert_eq!(
        open.codecs,
        [pb::MediaCodec::H264 as i32, pb::MediaCodec::Bgra as i32]
    );
    assert!(open.audio.as_ref().is_some_and(|audio| audio.enabled));
}

#[tokio::test]
async fn websocket_display_session_attaches_with_ticket_plays_audio_and_requests_keyframes() {
    let (url, fake) = start().await;
    full_session(&url, &fake, "display:0", 1).await;
    let open = fake.recorded.lock().unwrap().opens[0].clone();
    assert_eq!(
        open.target.unwrap().target,
        Some(pb::media_target::Target::DisplayId("0".into()))
    );
    assert!(!open.prefer_quic);
}

#[tokio::test]
async fn quic_window_session_uses_the_pinned_endpoint_and_datagrams() {
    let (url, fake) = start().await;
    full_session(&format!("{url}/?quic=1"), &fake, "w1", 3).await;
    let open = fake.recorded.lock().unwrap().opens[0].clone();
    assert!(open.prefer_quic);
    assert_eq!(
        open.target.unwrap().target,
        Some(pb::media_target::Target::Window(pb::WindowRef {
            id: "w1".into(),
            epoch: 3
        }))
    );
}

#[tokio::test]
async fn a_wrong_token_is_refused_by_the_server() {
    let (url, _) = start().await;
    let mut link = V2Link::connect(&url, Some("wrong"), None).await.unwrap();
    let error = link
        .send(ClientMessage::ListWindows {
            on_screen_only: true,
        })
        .await
        .unwrap_err();
    assert!(error.to_string().contains("bad token"), "{error}");
}

#[test]
fn options_parse_quic_and_audio_flags() {
    let options = V2Options::parse("http://127.0.0.1:3211/?quic=1&audio=0").unwrap();
    assert_eq!(options.server, "http://127.0.0.1:3211");
    assert!(options.prefer_quic && !options.audio && !options.secure);
    assert!(V2Options::parse("ws://x:1").is_err());
    let secure = V2Options::parse("https://relay.example").unwrap();
    assert_eq!(secure.server, "https://relay.example:443");
}

#[test]
fn quic_close_codes_map_back_to_websocket_codes() {
    assert_eq!(v2::close_code::quic_error(4401), 0x401);
    let digits = format!("{:x}", 0x404u32).parse::<u16>().unwrap();
    assert_eq!(4000 + digits, 4404);
}

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

#[cfg(unix)]
use std::error::Error;
#[cfg(unix)]
use std::path::Path;
#[cfg(unix)]
use std::sync::Arc;
#[cfg(unix)]
use std::time::Duration;

#[cfg(unix)]
use cua_media_protocol::WireHeader;
#[cfg(unix)]
use cua_media_transport::{read_packet, write_packet};
#[cfg(unix)]
use cua_spacesd_session::{Connection, OutboundPacket, ServerRuntime};

/// Default TCP port of the cua-spacesd daemon ("cua": c=3, u=21, a=1).
/// Used by `--listen` when no address is given and by the Linux default mode.
pub const DEFAULT_PORT: u16 = 3211;

pub mod quic;
mod remote;

pub use remote::{serve_remote, serve_remote_listener, RemoteShareConfig};
pub mod mcp_serve;
pub mod presence;
pub mod ws;

#[cfg(unix)]
pub async fn serve_unix(
    socket_path: &Path,
    runtime: Arc<ServerRuntime>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    use std::os::unix::fs::{FileTypeExt as _, PermissionsExt as _};
    use tokio::net::UnixListener;

    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if let Ok(metadata) = std::fs::symlink_metadata(socket_path) {
        if !metadata.file_type().is_socket() {
            return Err(format!(
                "refusing to replace non-socket path {}",
                socket_path.display()
            )
            .into());
        }
        std::fs::remove_file(socket_path)?;
    }
    let listener = UnixListener::bind(socket_path)?;
    std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o600))?;
    loop {
        let (stream, _) = listener.accept().await?;
        let connection = runtime.connect();
        tokio::spawn(async move {
            if let Err(error) = serve_connection(stream, connection).await {
                tracing::debug!(%error, "RCDP connection ended");
            }
        });
    }
}

#[cfg(unix)]
async fn serve_connection(
    stream: tokio::net::UnixStream,
    mut connection: Connection,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let (mut reader, mut writer) = stream.into_split();
    let outbound_ready = connection.outbound_ready();
    let mut deadline_tick = tokio::time::interval(Duration::from_millis(50));
    deadline_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            packet = read_packet(&mut reader) => {
                let Some((header, payload)) = packet? else {
                    return Ok(());
                };
                let WireHeader::Client(message) = header else {
                    return Err("client sent a non-client packet".into());
                };
                write_outbound(&mut writer, connection.handle_packet(message, payload).await).await?;
            }
            _ = outbound_ready.notified() => {
                write_outbound(&mut writer, connection.drain_outbound()).await?;
            }
            _ = deadline_tick.tick() => {
                write_outbound(&mut writer, connection.drain_outbound()).await?;
            }
        }
    }
}

#[cfg(unix)]
async fn write_outbound<W>(
    writer: &mut W,
    packets: Vec<OutboundPacket>,
) -> Result<(), Box<dyn Error + Send + Sync>>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    for packet in packets {
        match packet {
            OutboundPacket::Control(message) => {
                write_packet(writer, &WireHeader::Server(message), &[]).await?;
            }
            OutboundPacket::Video {
                descriptor,
                payload,
            } => {
                write_packet(writer, &WireHeader::Video(descriptor), &payload).await?;
            }
            OutboundPacket::AppIcon {
                descriptor,
                payload,
            } => {
                write_packet(writer, &WireHeader::AppIcon(descriptor), &payload).await?;
            }
            OutboundPacket::ClipboardFiles { message, payload } => {
                write_packet(writer, &WireHeader::Server(message), &payload).await?;
            }
        }
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use cua_media_protocol::{
        ActionCapability, ActionDeliveryGuarantee, ClientMessage, Hello, OpenSession,
        ServerMessage, SessionOpened, SessionPolicy, SurfaceGeometry, TargetEpoch, TargetHandle,
        VideoCodec, VideoFrameDescriptor, WindowDescriptor, WindowGeometryControl, WireHeader,
    };
    use cua_spacesd_provider_api::{
        AccessibilityProvider, AccessibilitySnapshot, ActionInvocation, ActionOutcome,
        ActionProvider, AppliedWindowGeometry, BackendTargetKey, CaptureConfig, CaptureEvent,
        CaptureLease, CaptureProvider, CaptureSink, OwnedFrame, PickTargetRequest, PixelFormat,
        ProviderError, ProviderErrorCode, ProviderFuture, ProviderTarget, ProviderTargetId,
        TargetProvider, TargetQuery, WindowGeometryProvider,
    };
    use futures_util::{SinkExt as _, StreamExt as _};
    use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
    use tokio_tungstenite::tungstenite::http::HeaderValue;
    use tokio_tungstenite::tungstenite::Message;

    use super::*;

    struct Fixture {
        target: ProviderTarget,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                target: ProviderTarget {
                    id: ProviderTargetId {
                        key: BackendTargetKey::new("native-secret"),
                        epoch: TargetEpoch(1),
                    },
                    descriptor: WindowDescriptor {
                        window: TargetHandle("opaque-fixture".into()),
                        target_epoch: TargetEpoch(1),
                        app_name: "Fixture".into(),
                        title: "Window".into(),
                        geometry: SurfaceGeometry {
                            width_px: 2,
                            height_px: 2,
                            scale_factor: 1.0,
                        },
                        visible: true,
                    },
                    grant: None,
                },
            }
        }
    }

    impl TargetProvider for Fixture {
        fn enumerate(&self, _: &TargetQuery) -> Result<Vec<ProviderTarget>, ProviderError> {
            Ok(vec![self.target.clone()])
        }

        fn pick(&self, _: &PickTargetRequest) -> Result<ProviderTarget, ProviderError> {
            Ok(self.target.clone())
        }

        fn restore(
            &self,
            _: &cua_media_protocol::TargetGrant,
        ) -> Result<ProviderTarget, ProviderError> {
            Err(ProviderError::new(
                ProviderErrorCode::Unsupported,
                "fixture has no restoration grant",
            ))
        }

        fn resolve(
            &self,
            handle: &TargetHandle,
            epoch: TargetEpoch,
        ) -> Result<ProviderTarget, ProviderError> {
            if handle == &self.target.descriptor.window && epoch == self.target.id.epoch {
                Ok(self.target.clone())
            } else {
                Err(ProviderError::new(
                    ProviderErrorCode::StaleTarget,
                    "stale fixture target",
                ))
            }
        }
    }

    struct FixtureLease(AtomicBool);

    impl CaptureLease for FixtureLease {
        fn stop(&self) {
            self.0.store(true, Ordering::Release);
        }
    }

    impl CaptureProvider for Fixture {
        fn formats(&self, _: &ProviderTargetId) -> Result<Vec<PixelFormat>, ProviderError> {
            Ok(vec![PixelFormat::H264AnnexB, PixelFormat::Bgra8])
        }

        fn start(
            &self,
            _: &ProviderTargetId,
            config: &CaptureConfig,
            sink: Arc<dyn CaptureSink>,
        ) -> Result<Arc<dyn CaptureLease>, ProviderError> {
            let h264 = config.accepted_formats.contains(&PixelFormat::H264AnnexB);
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(20));
                sink.on_event(CaptureEvent::Frame(OwnedFrame {
                    bytes: if h264 {
                        Arc::from(
                            [
                                0, 0, 0, 1, 0x67, 0x42, 0xe0, 0x1f, 0, 0, 0, 1, 0x68, 0xce, 0, 0,
                                0, 1, 0x65, 7,
                            ]
                            .as_slice(),
                        )
                    } else {
                        Arc::from(vec![7; 16])
                    },
                    format: if h264 {
                        PixelFormat::H264AnnexB
                    } else {
                        PixelFormat::Bgra8
                    },
                    width_px: 2,
                    height_px: 2,
                    bytes_per_row: (!h264).then_some(8),
                    capture_timestamp_us: 1,
                    encode_duration_us: None,
                    codec_epoch: 1,
                    keyframe: true,
                }));
            });
            Ok(Arc::new(FixtureLease(AtomicBool::new(false))))
        }
    }

    impl ActionProvider for Fixture {
        fn capabilities(
            &self,
            _: &ProviderTargetId,
        ) -> Result<Vec<ActionCapability>, ProviderError> {
            Ok(vec![ActionCapability {
                action: "type_text".into(),
                guarantee: ActionDeliveryGuarantee::Background,
            }])
        }

        fn perform<'a>(
            &'a self,
            _: &'a ProviderTargetId,
            _: ActionInvocation,
            _: SessionPolicy,
        ) -> ProviderFuture<'a, Result<ActionOutcome, ProviderError>> {
            Box::pin(async {
                Ok(ActionOutcome {
                    delivered: true,
                    detail: None,
                })
            })
        }
    }

    impl AccessibilityProvider for Fixture {
        fn snapshot<'a>(
            &'a self,
            _: &'a ProviderTargetId,
        ) -> ProviderFuture<'a, Result<AccessibilitySnapshot, ProviderError>> {
            Box::pin(async {
                Ok(AccessibilitySnapshot {
                    snapshot_id: cua_media_protocol::AccessibilitySnapshotId(1),
                    state: serde_json::json!({"role": "window"}),
                })
            })
        }
    }

    impl WindowGeometryProvider for Fixture {
        fn supports(&self, _: &ProviderTargetId) -> Result<bool, ProviderError> {
            Ok(true)
        }

        fn resize<'a>(
            &'a self,
            _: &'a ProviderTargetId,
            width_points: u32,
            height_points: u32,
        ) -> ProviderFuture<'a, Result<AppliedWindowGeometry, ProviderError>> {
            Box::pin(async move {
                Ok(AppliedWindowGeometry {
                    width_points,
                    height_points,
                })
            })
        }
    }

    pub(crate) fn fixture_runtime() -> Arc<ServerRuntime> {
        fixture_runtime_with_policy(SessionPolicy::AllowActivation)
    }

    fn fixture_runtime_with_policy(policy: SessionPolicy) -> Arc<ServerRuntime> {
        let fixture = Arc::new(Fixture::new());
        Arc::new(ServerRuntime::new_with_policy_ceiling(
            fixture.clone(),
            fixture.clone(),
            fixture.clone(),
            fixture.clone(),
            fixture,
            policy,
        ))
    }

    async fn send(
        stream: &mut tokio::net::UnixStream,
        message: ClientMessage,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        write_packet(stream, &WireHeader::Client(message), &[]).await?;
        Ok(())
    }

    async fn receive(
        stream: &mut tokio::net::UnixStream,
    ) -> Result<(WireHeader, Vec<u8>), Box<dyn Error + Send + Sync>> {
        tokio::time::timeout(Duration::from_secs(2), read_packet(stream))
            .await
            .map_err(|_| "timed out waiting for daemon packet")??
            .ok_or_else(|| "daemon closed fixture connection".into())
    }

    #[tokio::test]
    async fn unix_daemon_negotiates_discovers_and_streams_binary_frame(
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        use std::os::unix::fs::PermissionsExt as _;

        let path = std::env::temp_dir().join(format!("cua-spacesd-f-{}.sock", std::process::id()));
        let server_path = path.clone();
        let server = tokio::spawn(async move { serve_unix(&server_path, fixture_runtime()).await });
        for _ in 0..100 {
            if path.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let mode = std::fs::metadata(&path)?.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let mut stream = tokio::net::UnixStream::connect(&path).await?;
        send(&mut stream, ClientMessage::Hello(Hello::default())).await?;
        assert!(matches!(
            receive(&mut stream).await?.0,
            WireHeader::Server(ServerMessage::Hello(_))
        ));
        send(
            &mut stream,
            ClientMessage::ListWindows {
                on_screen_only: true,
            },
        )
        .await?;
        let WireHeader::Server(ServerMessage::Windows { windows }) = receive(&mut stream).await?.0
        else {
            panic!("expected window discovery response");
        };
        assert_eq!(windows[0].window.0, "opaque-fixture");
        send(
            &mut stream,
            ClientMessage::OpenSession(OpenSession {
                window: windows[0].window.clone(),
                target_epoch: windows[0].target_epoch,
                accepted_codecs: vec![VideoCodec::Bgra],
                max_fps: 15,
                max_dimension: 1280,
                target_bitrate_kbps: None,
                policy: SessionPolicy::BackgroundOnly,
                geometry_control: WindowGeometryControl::ObserveOnly,
            }),
        )
        .await?;
        assert!(matches!(
            receive(&mut stream).await?.0,
            WireHeader::Server(ServerMessage::SessionOpened(_))
        ));
        loop {
            let (header, payload) = receive(&mut stream).await?;
            if let WireHeader::Video(frame) = header {
                assert_eq!(frame.codec, VideoCodec::Bgra);
                assert_eq!(payload, vec![7; 16]);
                break;
            }
        }
        server.abort();
        let _ = server.await;
        let _ = std::fs::remove_file(path);
        Ok::<(), Box<dyn Error + Send + Sync>>(())
    }

    #[tokio::test]
    async fn remote_share_authenticates_discovers_streams_and_caps_policy(
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let server = tokio::spawn(async move {
            serve_remote_listener(
                listener,
                fixture_runtime_with_policy(SessionPolicy::BackgroundOnly),
                RemoteShareConfig {
                    name: "codex".into(),
                    application_id: "com.openai.codex".into(),
                    allowed_users: vec!["user@example.com".into()],
                    required_capability: Some("trycua.com/cap/rcdp".into()),
                },
            )
            .await
        });

        let mut request = format!("ws://{address}/v1/connect").into_client_request()?;
        request.headers_mut().insert(
            "tailscale-user-login",
            HeaderValue::from_static("user@example.com"),
        );
        request.headers_mut().insert(
            "tailscale-app-capabilities",
            HeaderValue::from_static(r#"{"trycua.com/cap/rcdp":[{}]}"#),
        );
        let (mut socket, _) = tokio_tungstenite::connect_async(request).await?;

        let Message::Text(authenticated) = socket.next().await.ok_or("missing auth")?? else {
            panic!("expected authentication control message");
        };
        assert!(matches!(
            serde_json::from_str(&authenticated)?,
            WireHeader::Server(ServerMessage::Authenticated)
        ));

        for message in [
            ClientMessage::Hello(Hello::default()),
            ClientMessage::ListWindows {
                on_screen_only: true,
            },
        ] {
            socket
                .send(Message::Text(serde_json::to_string(&WireHeader::Client(
                    message,
                ))?))
                .await?;
        }
        let Message::Text(hello) = socket.next().await.ok_or("missing hello")?? else {
            panic!("expected hello control message");
        };
        assert!(matches!(
            serde_json::from_str(&hello)?,
            WireHeader::Server(ServerMessage::Hello(_))
        ));
        let Message::Text(windows) = socket.next().await.ok_or("missing windows")?? else {
            panic!("expected windows control message");
        };
        let WireHeader::Server(ServerMessage::Windows { windows }) =
            serde_json::from_str(&windows)?
        else {
            panic!("expected window discovery response");
        };
        let target = windows[0].clone();

        socket
            .send(Message::Text(serde_json::to_string(&WireHeader::Client(
                ClientMessage::OpenSession(OpenSession {
                    window: target.window.clone(),
                    target_epoch: target.target_epoch,
                    accepted_codecs: vec![VideoCodec::H264],
                    max_fps: 15,
                    max_dimension: 1280,
                    target_bitrate_kbps: None,
                    policy: SessionPolicy::BackgroundOnly,
                    geometry_control: WindowGeometryControl::ObserveOnly,
                }),
            ))?))
            .await?;
        let Message::Text(opened) = socket.next().await.ok_or("missing open")?? else {
            panic!("expected session-opened control message");
        };
        assert!(matches!(
            serde_json::from_str(&opened)?,
            WireHeader::Server(ServerMessage::SessionOpened(SessionOpened {
                codec: VideoCodec::H264,
                ..
            }))
        ));
        loop {
            let message = tokio::time::timeout(Duration::from_secs(2), socket.next())
                .await?
                .ok_or("remote daemon closed")??;
            if let Message::Binary(packet) = message {
                let (header, payload) = cua_media_transport::decode_packet(&packet)?;
                assert!(matches!(
                    header,
                    WireHeader::Video(VideoFrameDescriptor {
                        codec: VideoCodec::H264,
                        keyframe: true,
                        codec_epoch: cua_media_protocol::CodecEpoch(1),
                        ..
                    })
                ));
                assert_eq!(
                    payload,
                    [
                        0, 0, 0, 1, 0x67, 0x42, 0xe0, 0x1f, 0, 0, 0, 1, 0x68, 0xce, 0, 0, 0, 1,
                        0x65, 7,
                    ]
                );
                break;
            }
        }

        socket
            .send(Message::Text(serde_json::to_string(&WireHeader::Client(
                ClientMessage::OpenSession(OpenSession {
                    window: target.window,
                    target_epoch: target.target_epoch,
                    accepted_codecs: vec![VideoCodec::H264],
                    max_fps: 15,
                    max_dimension: 1280,
                    target_bitrate_kbps: None,
                    policy: SessionPolicy::AllowActivation,
                    geometry_control: WindowGeometryControl::ObserveOnly,
                }),
            ))?))
            .await?;
        let Message::Text(rejected) = socket.next().await.ok_or("missing rejection")?? else {
            panic!("expected policy rejection control message");
        };
        assert!(matches!(
            serde_json::from_str(&rejected)?,
            WireHeader::Server(ServerMessage::Error {
                code: cua_media_protocol::ServerErrorCode::InvalidOpen,
                ..
            })
        ));

        server.abort();
        let _ = server.await;
        Ok(())
    }

    #[tokio::test]
    async fn unix_daemon_refuses_to_replace_a_regular_file(
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        let path = std::env::temp_dir().join(format!("cua-spacesd-n-{}", std::process::id()));
        std::fs::write(&path, b"preserve me")?;
        let error = serve_unix(&path, fixture_runtime()).await.unwrap_err();
        assert!(error.to_string().contains("refusing to replace non-socket"));
        assert_eq!(std::fs::read(&path)?, b"preserve me");
        std::fs::remove_file(path)?;
        Ok::<(), Box<dyn Error + Send + Sync>>(())
    }
}

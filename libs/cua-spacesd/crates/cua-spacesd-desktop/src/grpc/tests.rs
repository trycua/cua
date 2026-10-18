// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Service tests over a fake backend (host-safe: no real desktop, no apps).
//! The server runs on loopback; clients are the generated tonic clients and
//! a tokio-tungstenite WebSocket.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cua_media_protocol::v2::{decode_server_text, ServerMessage};
use cua_media_protocol::{
    ActionCapability, SessionPolicy as WirePolicy, SurfaceGeometry, TargetEpoch, TargetGrant,
    TargetHandle, WindowDescriptor,
};
use cua_proto::env::v1::{
    accessibility_service_client::AccessibilityServiceClient,
    computer_service_client::ComputerServiceClient, join_response, key_input, keyboard_request,
    media_target, pointer_request, presence_service_client::PresenceServiceClient,
    stream_service_client::StreamServiceClient, windows_service_client::WindowsServiceClient,
    AccessibilityAction, AccessibilityQuery, ActRequest, CloseMediaRequest, CoordinateSpace,
    CursorPosition, Delivery, ElementRef, FindRequest, GetTreeRequest, ImageFormat, InputTarget,
    JoinRequest, Key, KeyInput, KeyboardPress, KeyboardRequest, ListTargetsRequest,
    ListWindowsRequest, MediaCodec, MediaTarget, MinimizeWindowRequest, OpenMediaRequest, Point,
    PointerClick, PointerRequest, Principal, PrincipalKind, ScreenshotRequest, SessionPolicy,
    UpdateCursorRequest, WindowFilter, WindowRef,
};
use cua_spacesd_provider_api::*;
use cua_spacesd_session::media::MediaProviders;
use futures_util::StreamExt as _;
use prost::Message as _;
use tokio_tungstenite::tungstenite;

use super::backend::*;
use super::DesktopServiceProvider;
use cua_spacesd_server::{ServerConfig, ServerContext};

// ---------------------------------------------------------------- fakes

type PointerCall = (Option<String>, DeliveryRequest, (f64, f64), PointerAction);

#[derive(Default)]
struct Calls {
    pointer: Vec<PointerCall>,
    keyboard: Vec<(Option<String>, KeyAction)>,
    window_actions: Vec<(String, WindowAction)>,
    a11y_acts: Vec<(String, A11yActionKind)>,
    clipboard: ClipboardData,
}

struct FakeBackend {
    calls: Arc<Mutex<Calls>>,
    capture: Arc<FakeCapture>,
    a11y_generation: Mutex<u64>,
}

fn window(id: &str, title: &str, kind: WindowKind, x: f64) -> WindowRecord {
    WindowRecord {
        handle: TargetHandle(id.into()),
        epoch: TargetEpoch(1),
        title: title.into(),
        app_name: "Fixture".into(),
        app_id: "fixture".into(),
        pid: 100,
        bounds: (x, 50.0, 200.0, 100.0),
        display_id: "0".into(),
        state: WindowStateKind::Normal,
        kind,
        focused: id == "w1",
        on_screen: true,
        z_order: 0,
    }
}

fn display() -> ProviderDisplay {
    ProviderDisplay {
        id: "0".into(),
        name: "fake".into(),
        primary: true,
        bounds: (0.0, 0.0, 640.0, 480.0),
        native_width_px: 640,
        native_height_px: 480,
        scale_factor: 1.0,
        refresh_rate_hz: 60,
    }
}

impl DesktopBackend for FakeBackend {
    fn displays(&self) -> Result<Vec<ProviderDisplay>, ProviderError> {
        Ok(vec![display()])
    }

    fn windows(&self) -> Result<Vec<WindowRecord>, ProviderError> {
        Ok(vec![
            window("w1", "One", WindowKind::Standard, 10.0),
            window("w2", "Two", WindowKind::Standard, 300.0),
            window("phantom", "", WindowKind::Phantom, 0.0),
            window("dock", "Dock", WindowKind::System, 0.0),
        ])
    }

    fn capture_display(&self, _display_id: &str) -> Result<CapturedImage, ProviderError> {
        Ok(CapturedImage {
            bgra: [255, 0, 0, 255].repeat(640 * 480),
            width: 640,
            height: 480,
            logical_bounds: (0.0, 0.0, 640.0, 480.0),
            display_id: "0".into(),
        })
    }

    fn capture_window(&self, window: &WindowRecord) -> Result<CapturedImage, ProviderError> {
        Ok(CapturedImage {
            bgra: [0, 255, 0, 255].repeat(200 * 100),
            width: 200,
            height: 100,
            logical_bounds: window.bounds,
            display_id: "0".into(),
        })
    }

    fn pointer(
        &self,
        window: Option<&WindowRecord>,
        delivery: DeliveryRequest,
        point: (f64, f64),
        action: &PointerAction,
    ) -> Result<DeliveryResult, ProviderError> {
        self.calls.lock().unwrap().pointer.push((
            window.map(|w| w.handle.0.clone()),
            delivery,
            point,
            action.clone(),
        ));
        Ok(DeliveryResult {
            delivery: if delivery == DeliveryRequest::Foreground {
                DeliveryUsed::Foreground
            } else {
                DeliveryUsed::Background
            },
            focus_changed: false,
            pointer_moved: delivery == DeliveryRequest::Foreground,
            detail: "fake".into(),
        })
    }

    fn keyboard(
        &self,
        window: Option<&WindowRecord>,
        _delivery: DeliveryRequest,
        action: &KeyAction,
    ) -> Result<DeliveryResult, ProviderError> {
        self.calls
            .lock()
            .unwrap()
            .keyboard
            .push((window.map(|w| w.handle.0.clone()), action.clone()));
        Ok(DeliveryResult {
            delivery: DeliveryUsed::Background,
            focus_changed: false,
            pointer_moved: false,
            detail: "fake".into(),
        })
    }

    fn cursor_position(&self) -> Result<(f64, f64), ProviderError> {
        Ok((1.0, 2.0))
    }

    fn clipboard_get(&self) -> Result<ClipboardData, ProviderError> {
        Ok(self.calls.lock().unwrap().clipboard.clone())
    }

    fn clipboard_set(&self, data: ClipboardData) -> Result<(), ProviderError> {
        self.calls.lock().unwrap().clipboard = data;
        Ok(())
    }

    fn window_action(
        &self,
        window: &WindowRecord,
        action: WindowAction,
    ) -> Result<(), ProviderError> {
        self.calls
            .lock()
            .unwrap()
            .window_actions
            .push((window.handle.0.clone(), action));
        Ok(())
    }

    fn set_window_bounds(
        &self,
        _: &WindowRecord,
        _: Option<(f64, f64)>,
        _: Option<(f64, f64)>,
    ) -> Result<(), ProviderError> {
        Ok(())
    }

    fn launch(&self, _: &LaunchRequest) -> Result<u32, ProviderError> {
        Ok(0)
    }

    fn open(&self, _: &OpenTarget, _: Option<&AppSpecData>, _: bool) -> Result<u32, ProviderError> {
        Ok(0)
    }

    fn a11y_tree(&self, _: &WindowRecord, _: u32, _: u32) -> Result<A11yTree, ProviderError> {
        let mut generation = self.a11y_generation.lock().unwrap();
        *generation += 1;
        Ok(A11yTree {
            snapshot: *generation,
            nodes: vec![
                A11yNode {
                    element_id: "0".into(),
                    role: "window".into(),
                    name: "One".into(),
                    ..A11yNode::default()
                },
                A11yNode {
                    element_id: "1".into(),
                    parent_id: "0".into(),
                    depth: 1,
                    role: "button".into(),
                    native_role: "push button".into(),
                    name: "Press Me".into(),
                    actions: vec!["click".into()],
                    states: vec!["enabled".into()],
                    ..A11yNode::default()
                },
            ],
            truncated: false,
        })
    }

    fn a11y_act(
        &self,
        _: &WindowRecord,
        _: u64,
        element_id: &str,
        action: A11yActionKind,
        _: &str,
    ) -> Result<(), ProviderError> {
        self.calls
            .lock()
            .unwrap()
            .a11y_acts
            .push((element_id.into(), action));
        Ok(())
    }

    fn media_providers(&self) -> MediaProviders {
        media_over(Arc::new(FakeProviders {
            capture: self.capture.clone(),
            scale: 1.0,
        }))
    }

    fn features(&self) -> Vec<FeatureStatus> {
        vec![FeatureStatus::yes("desktop_stream")]
    }
}

/// Fake media providers (one display at `scale`, BGRA capture) for backend tests.
pub(super) fn fake_media_providers(scale: f64) -> MediaProviders {
    media_over(Arc::new(FakeProviders {
        capture: Arc::new(FakeCapture::default()),
        scale,
    }))
}

fn media_over(fake: Arc<FakeProviders>) -> MediaProviders {
    {
        MediaProviders {
            targets: fake.clone(),
            displays: fake.clone(),
            captures: fake.clone(),
            actions: fake.clone(),
            accessibility: fake.clone(),
            geometry: fake.clone(),
            inputs: Arc::new(UnsupportedInteractiveInputProvider),
            audio: None,
        }
    }
}

#[derive(Default)]
struct FakeCapture {
    sinks: Mutex<Vec<Arc<dyn CaptureSink>>>,
}

impl FakeCapture {
    fn emit(&self) {
        self.emit_frame(true);
    }

    fn emit_frame(&self, keyframe: bool) {
        let sinks = self.sinks.lock().unwrap().clone();
        for sink in sinks {
            sink.on_event(CaptureEvent::Frame(OwnedFrame {
                bytes: Arc::from([10u8, 20, 30, 255].repeat(64 * 48)),
                format: PixelFormat::Bgra8,
                width_px: 64,
                height_px: 48,
                bytes_per_row: Some(256),
                capture_timestamp_us: cua_spacesd_session::media::clock::now_us(),
                encode_duration_us: None,
                codec_epoch: 1,
                keyframe,
            }));
        }
    }
}

struct FakeLease {
    capture: Arc<FakeCapture>,
    stopped: AtomicBool,
}

impl CaptureLease for FakeLease {
    fn request_keyframe(&self) {
        if !self.stopped.load(Ordering::Relaxed) {
            self.capture.emit();
        }
    }

    fn stop(&self) {
        self.stopped.store(true, Ordering::Relaxed);
    }
}

struct FakeProviders {
    capture: Arc<FakeCapture>,
    scale: f64,
}

fn provider_target(handle: &str) -> ProviderTarget {
    ProviderTarget {
        id: ProviderTargetId {
            key: BackendTargetKey::new(format!("fake:{handle}")),
            epoch: TargetEpoch(1),
        },
        descriptor: WindowDescriptor {
            window: TargetHandle(handle.into()),
            target_epoch: TargetEpoch(1),
            app_name: "Fixture".into(),
            title: handle.into(),
            geometry: SurfaceGeometry {
                width_px: 64,
                height_px: 48,
                scale_factor: 1.0,
            },
            visible: true,
        },
        grant: Some(TargetGrant("g".into())),
    }
}

impl TargetProvider for FakeProviders {
    fn enumerate(&self, _: &TargetQuery) -> Result<Vec<ProviderTarget>, ProviderError> {
        Ok(vec![provider_target("w1"), provider_target("w2")])
    }
    fn pick(&self, _: &PickTargetRequest) -> Result<ProviderTarget, ProviderError> {
        Ok(provider_target("w1"))
    }
    fn restore(&self, _: &TargetGrant) -> Result<ProviderTarget, ProviderError> {
        Ok(provider_target("w1"))
    }
    fn resolve(
        &self,
        handle: &TargetHandle,
        _: TargetEpoch,
    ) -> Result<ProviderTarget, ProviderError> {
        Ok(provider_target(&handle.0))
    }
}

impl DisplayProvider for FakeProviders {
    fn displays(&self) -> Result<Vec<ProviderDisplay>, ProviderError> {
        Ok(vec![ProviderDisplay {
            scale_factor: self.scale,
            ..display()
        }])
    }
    fn display_target(&self, display_id: &str) -> Result<ProviderTarget, ProviderError> {
        Ok(provider_target(&format!("display-{display_id}")))
    }
}

impl CaptureProvider for FakeProviders {
    fn formats(&self, _: &ProviderTargetId) -> Result<Vec<PixelFormat>, ProviderError> {
        Ok(vec![PixelFormat::Bgra8])
    }
    fn start(
        &self,
        _: &ProviderTargetId,
        _: &CaptureConfig,
        sink: Arc<dyn CaptureSink>,
    ) -> Result<Arc<dyn CaptureLease>, ProviderError> {
        self.capture.sinks.lock().unwrap().push(sink);
        self.capture.emit();
        Ok(Arc::new(FakeLease {
            capture: self.capture.clone(),
            stopped: AtomicBool::new(false),
        }))
    }
}

impl ActionProvider for FakeProviders {
    fn capabilities(&self, _: &ProviderTargetId) -> Result<Vec<ActionCapability>, ProviderError> {
        Ok(Vec::new())
    }
    fn perform<'a>(
        &'a self,
        _: &'a ProviderTargetId,
        _: ActionInvocation,
        _: WirePolicy,
    ) -> ProviderFuture<'a, Result<ActionOutcome, ProviderError>> {
        Box::pin(async {
            Ok(ActionOutcome {
                delivered: true,
                detail: None,
            })
        })
    }
}

impl AccessibilityProvider for FakeProviders {
    fn snapshot<'a>(
        &'a self,
        _: &'a ProviderTargetId,
    ) -> ProviderFuture<'a, Result<AccessibilitySnapshot, ProviderError>> {
        Box::pin(async { Err(ProviderError::new(ProviderErrorCode::Unsupported, "fake")) })
    }
}

impl WindowGeometryProvider for FakeProviders {
    fn supports(&self, _: &ProviderTargetId) -> Result<bool, ProviderError> {
        Ok(false)
    }
    fn resize<'a>(
        &'a self,
        _: &'a ProviderTargetId,
        _: u32,
        _: u32,
    ) -> ProviderFuture<'a, Result<AppliedWindowGeometry, ProviderError>> {
        Box::pin(async { Err(ProviderError::new(ProviderErrorCode::Unsupported, "fake")) })
    }
}

// ---------------------------------------------------------------- harness

struct Server {
    ctx: ServerContext,
    provider: DesktopServiceProvider,
    capture: Arc<FakeCapture>,
    address: SocketAddr,
    calls: Arc<Mutex<Calls>>,
    channel: tonic::transport::Channel,
}

async fn server() -> Server {
    let calls = Arc::new(Mutex::new(Calls::default()));
    let capture = Arc::new(FakeCapture::default());
    let backend = Arc::new(FakeBackend {
        calls: calls.clone(),
        capture: capture.clone(),
        a11y_generation: Mutex::new(0),
    });
    let ctx = ServerContext::new(ServerConfig::default(), None);
    let provider = DesktopServiceProvider::new(backend, ctx.clone());
    let router = provider.standalone_router();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let channel = tonic::transport::Channel::from_shared(format!("http://{address}"))
        .unwrap()
        .connect()
        .await
        .unwrap();
    Server {
        ctx,
        provider,
        capture,
        address,
        calls,
        channel,
    }
}

fn with_principal<T>(message: T, principal: &Principal) -> tonic::Request<T> {
    let mut request = tonic::Request::new(message);
    request.metadata_mut().insert_bin(
        cua_proto::metadata::PRINCIPAL_BIN,
        tonic::metadata::MetadataValue::from_bytes(&principal.encode_to_vec()),
    );
    request
}

fn principal(id: &str, kind: PrincipalKind) -> Principal {
    Principal {
        id: id.into(),
        display_name: id.into(),
        color: String::new(),
        kind: kind as i32,
    }
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Read at most `limit` messages (bounded: a runaway server fails the test).
async fn read_until<T>(
    socket: &mut Socket,
    limit: usize,
    mut pick: impl FnMut(&tungstenite::Message) -> Option<T>,
) -> T {
    for _ in 0..limit {
        let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .expect("message within 5 s")
            .expect("socket open")
            .expect("valid frame");
        if let Some(found) = pick(&message) {
            return found;
        }
    }
    panic!("wanted message not seen within {limit} messages");
}

// ---------------------------------------------------------------- tests

#[tokio::test]
async fn open_media_ticket_attaches_the_websocket_with_server_first_hello_and_a_keyframe() {
    let server = server().await;
    let mut stream = StreamServiceClient::new(server.channel.clone());
    let targets = stream
        .list_targets(ListTargetsRequest {
            include_windows: true,
            window_filter: None,
        })
        .await
        .unwrap()
        .into_inner();
    // One display, two windows; phantom and system windows are never listed.
    assert_eq!(targets.targets.len(), 3);
    let opened = stream
        .open_media(OpenMediaRequest {
            target: Some(MediaTarget {
                target: Some(media_target::Target::DisplayId("primary".into())),
            }),
            codecs: vec![MediaCodec::Bgra as i32],
            policy: SessionPolicy::ViewOnly as i32,
            ..OpenMediaRequest::default()
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(opened.wire_version, 2);
    assert_eq!(opened.codec, MediaCodec::Bgra as i32);
    assert!(opened.ws_path.starts_with("/media?ticket="));
    let (mut socket, _) =
        tokio_tungstenite::connect_async(format!("ws://{}{}", server.address, opened.ws_path))
            .await
            .unwrap();
    let first = read_until(&mut socket, 1, |message| match message {
        tungstenite::Message::Text(text) => Some(decode_server_text(text).unwrap()),
        _ => None,
    })
    .await;
    assert!(matches!(first, ServerMessage::Hello(ref hello) if hello.selected_version == 2));
    let second = read_until(&mut socket, 1, |message| match message {
        tungstenite::Message::Text(text) => Some(decode_server_text(text).unwrap()),
        _ => None,
    })
    .await;
    match second {
        ServerMessage::SessionOpened(session) => {
            assert_eq!(session.session_id.0, opened.media_session_id)
        }
        other => panic!("unexpected {other:?}"),
    }
    let keyframe = read_until(&mut socket, 8, |message| match message {
        tungstenite::Message::Binary(bytes) => {
            let (header, payload) = cua_media_transport::decode_packet(bytes).unwrap();
            match header {
                cua_media_protocol::WireHeader::Video(descriptor) => {
                    Some((descriptor, payload.len()))
                }
                _ => None,
            }
        }
        _ => None,
    })
    .await;
    assert!(keyframe.0.keyframe);
    assert_eq!(keyframe.1, 64 * 48 * 4);

    // A bad ticket is refused at the upgrade with 401.
    let refused =
        tokio_tungstenite::connect_async(format!("ws://{}/media?ticket=nope", server.address))
            .await;
    match refused {
        Err(tungstenite::Error::Http(response)) => assert_eq!(response.status(), 401),
        other => panic!("expected 401, got {other:?}"),
    }

    // CloseMedia closes attached sockets with 4404; the ticket then gets 410.
    stream
        .close_media(CloseMediaRequest {
            media_session_id: opened.media_session_id.clone(),
        })
        .await
        .unwrap();
    let code = read_until(&mut socket, 64, |message| match message {
        tungstenite::Message::Close(Some(frame)) => Some(u16::from(frame.code)),
        _ => None,
    })
    .await;
    assert_eq!(code, 4404);
    match tokio_tungstenite::connect_async(format!("ws://{}{}", server.address, opened.ws_path))
        .await
    {
        Err(tungstenite::Error::Http(response)) => assert_eq!(response.status(), 410),
        other => panic!("expected 410, got {other:?}"),
    }
}

#[tokio::test]
async fn pointer_converts_coordinate_spaces_and_reports_delivery() {
    let server = server().await;
    let mut computer = ComputerServiceClient::new(server.channel.clone());
    let target = |space: CoordinateSpace, delivery: Delivery| InputTarget {
        window: Some(WindowRef {
            id: "w2".into(),
            epoch: 1,
        }),
        display_id: String::new(),
        delivery: delivery as i32,
        space: space as i32,
        screenshot_id: String::new(),
    };
    let click = |x: f64, y: f64| {
        Some(pointer_request::Action::Click(PointerClick {
            position: Some(Point { x, y }),
            button: 0,
            count: 1,
            modifiers: vec![],
        }))
    };
    let response = computer
        .pointer(PointerRequest {
            target: Some(target(CoordinateSpace::Normalized, Delivery::Background)),
            action: click(0.5, 0.5),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        response.report.unwrap().delivery,
        Delivery::Background as i32
    );
    computer
        .pointer(PointerRequest {
            target: Some(target(CoordinateSpace::Window, Delivery::Foreground)),
            action: click(10.0, 20.0),
        })
        .await
        .unwrap();
    // Screenshot pixel space: the window screenshot is 200x100 at scale 1.
    let shot = computer
        .screenshot(ScreenshotRequest {
            source: Some(cua_proto::env::v1::screenshot_request::Source::Window(
                WindowRef {
                    id: "w2".into(),
                    epoch: 1,
                },
            )),
            format: ImageFormat::Png as i32,
            max_dimension: 100,
            ..ScreenshotRequest::default()
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(shot.image_size.unwrap().width, 100);
    assert_eq!(shot.native_size.unwrap().width, 200);
    assert!((shot.scale - 0.5).abs() < 1e-9);
    assert!(shot.image.starts_with(&[0x89, b'P', b'N', b'G']));
    let mut screenshot_target = target(CoordinateSpace::Screenshot, Delivery::Auto);
    screenshot_target.screenshot_id = shot.screenshot_id;
    computer
        .pointer(PointerRequest {
            target: Some(screenshot_target),
            action: click(50.0, 25.0),
        })
        .await
        .unwrap();
    let calls = server.calls.lock().unwrap();
    assert_eq!(calls.pointer[0].2, (400.0, 100.0));
    assert_eq!(calls.pointer[1].2, (310.0, 70.0));
    assert_eq!(calls.pointer[2].2, (400.0, 100.0));
    assert_eq!(calls.pointer[2].1, DeliveryRequest::Auto);
}

#[tokio::test]
async fn window_input_is_leased_per_principal() {
    let server = server().await;
    let mut computer = ComputerServiceClient::new(server.channel.clone());
    let press = |window: &str| KeyboardRequest {
        target: Some(InputTarget {
            window: Some(WindowRef {
                id: window.into(),
                epoch: 1,
            }),
            ..InputTarget::default()
        }),
        action: Some(keyboard_request::Action::Press(KeyboardPress {
            key: Some(KeyInput {
                key: Some(key_input::Key::Named(Key::Enter as i32)),
            }),
            modifiers: vec![Key::Control as i32],
            repeat: 1,
        })),
    };
    let alice = principal("alice", PrincipalKind::Human);
    let bob = principal("bob", PrincipalKind::Human);
    computer
        .keyboard(with_principal(press("w1"), &alice))
        .await
        .unwrap();
    // Bob may drive another window at the same time...
    computer
        .keyboard(with_principal(press("w2"), &bob))
        .await
        .unwrap();
    // ...but not Alice's.
    let refused = computer
        .keyboard(with_principal(press("w1"), &bob))
        .await
        .unwrap_err();
    assert_eq!(refused.code(), tonic::Code::Aborted);
    let calls = server.calls.lock().unwrap();
    assert_eq!(calls.keyboard.len(), 2);
    assert_eq!(
        calls.keyboard[0].1,
        KeyAction::Press {
            key: "return".into(),
            modifiers: vec!["ctrl".into()],
            repeat: 1
        }
    );
}

#[tokio::test]
async fn presence_clients_see_each_other_and_agent_input_publishes_an_agent_cursor() {
    let server = server().await;
    let mut alice_client = PresenceServiceClient::new(server.channel.clone());
    let mut bob_client = PresenceServiceClient::new(server.channel.clone());
    let mut alice = alice_client
        .join(JoinRequest {
            principal: Some(principal("alice", PrincipalKind::Human)),
            keepalive_interval: None,
            ..JoinRequest::default()
        })
        .await
        .unwrap()
        .into_inner();
    let alice_joined = match alice.next().await.unwrap().unwrap().event {
        Some(join_response::Event::Joined(joined)) => joined.participant.unwrap(),
        other => panic!("unexpected {other:?}"),
    };
    let mut bob = bob_client
        .join(JoinRequest {
            principal: Some(principal("bob", PrincipalKind::Human)),
            keepalive_interval: None,
            ..JoinRequest::default()
        })
        .await
        .unwrap()
        .into_inner();
    let roster = match bob.next().await.unwrap().unwrap().event {
        Some(join_response::Event::Joined(joined)) => joined.roster,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(roster.len(), 1);
    let bob_color = roster[0]
        .participant
        .as_ref()
        .unwrap()
        .principal
        .as_ref()
        .unwrap()
        .color
        .clone();
    assert!(bob_color.starts_with('#'));
    alice_client
        .update_cursor(UpdateCursorRequest {
            participant_id: alice_joined.participant_id.clone(),
            cursor: Some(CursorPosition {
                display_id: "0".into(),
                window: None,
                position: Some(Point { x: 0.1, y: 0.2 }),
                visible: true,
                ..CursorPosition::default()
            }),
        })
        .await
        .unwrap();
    let mut saw_alice = false;
    for _ in 0..10 {
        let event = tokio::time::timeout(Duration::from_secs(2), bob.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if let Some(join_response::Event::CursorMoved(moved)) = event.event {
            assert_eq!(moved.participant_id, alice_joined.participant_id);
            saw_alice = true;
            break;
        }
    }
    assert!(saw_alice);
    // An agent principal driving ComputerService shows up as an agent cursor.
    let mut computer = ComputerServiceClient::new(server.channel.clone());
    computer
        .pointer(with_principal(
            PointerRequest {
                target: Some(InputTarget {
                    space: CoordinateSpace::Screen as i32,
                    ..InputTarget::default()
                }),
                action: Some(pointer_request::Action::Click(PointerClick {
                    position: Some(Point { x: 320.0, y: 240.0 }),
                    button: 0,
                    count: 1,
                    modifiers: vec![],
                })),
            },
            &principal("agent-7", PrincipalKind::Agent),
        ))
        .await
        .unwrap();
    let mut agent_joined = false;
    let mut agent_cursor = false;
    for _ in 0..20 {
        let Ok(Some(Ok(event))) = tokio::time::timeout(Duration::from_secs(2), bob.next()).await
        else {
            break;
        };
        match event.event {
            Some(join_response::Event::ParticipantJoined(participant)) => {
                agent_joined |= participant.principal.unwrap().kind == PrincipalKind::Agent as i32;
            }
            Some(join_response::Event::CursorMoved(moved))
                if moved.participant_id != alice_joined.participant_id =>
            {
                let position = moved.cursor.unwrap().position.unwrap();
                assert!((position.x - 0.5).abs() < 1e-9 && (position.y - 0.5).abs() < 1e-9);
                agent_cursor = true;
            }
            _ => {}
        }
        if agent_joined && agent_cursor {
            break;
        }
    }
    assert!(agent_joined && agent_cursor);
}

#[tokio::test]
async fn accessibility_find_then_act_and_stale_snapshots_are_refused() {
    let server = server().await;
    let mut a11y = AccessibilityServiceClient::new(server.channel.clone());
    let found = a11y
        .find(FindRequest {
            window: Some(WindowRef {
                id: "w1".into(),
                epoch: 1,
            }),
            query: Some(AccessibilityQuery {
                role: "button".into(),
                name: "Press Me".into(),
                ..AccessibilityQuery::default()
            }),
            max_results: 0,
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(found.nodes.len(), 1);
    assert_eq!(
        found.nodes[0].actions,
        vec![AccessibilityAction::Press as i32]
    );
    let element = ElementRef {
        snapshot_id: found.snapshot_id.clone(),
        element_id: found.nodes[0].element_id.clone(),
    };
    a11y.act(ActRequest {
        element: Some(element.clone()),
        action: AccessibilityAction::Press as i32,
        ..ActRequest::default()
    })
    .await
    .unwrap();
    assert_eq!(
        server.calls.lock().unwrap().a11y_acts,
        vec![("1".to_owned(), A11yActionKind::Press)]
    );
    a11y.get_tree(GetTreeRequest {
        window: Some(WindowRef {
            id: "w1".into(),
            epoch: 1,
        }),
        ..GetTreeRequest::default()
    })
    .await
    .unwrap();
    let stale = a11y
        .act(ActRequest {
            element: Some(element),
            action: AccessibilityAction::Press as i32,
            ..ActRequest::default()
        })
        .await
        .unwrap_err();
    assert_eq!(stale.code(), tonic::Code::FailedPrecondition);
}

#[tokio::test]
async fn windows_hide_phantom_and_system_windows_unless_asked_and_manage_windows() {
    let server = server().await;
    let mut windows = WindowsServiceClient::new(server.channel.clone());
    let listed = windows
        .list_windows(ListWindowsRequest { filter: None })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(listed.windows.len(), 2);
    let all = windows
        .list_windows(ListWindowsRequest {
            filter: Some(WindowFilter {
                include_system: true,
                ..WindowFilter::default()
            }),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(all.windows.len(), 4);
    windows
        .minimize_window(MinimizeWindowRequest {
            window: Some(WindowRef {
                id: "w2".into(),
                epoch: 1,
            }),
        })
        .await
        .unwrap();
    let stale = windows
        .minimize_window(MinimizeWindowRequest {
            window: Some(WindowRef {
                id: "w2".into(),
                epoch: 9,
            }),
        })
        .await
        .unwrap_err();
    assert_eq!(stale.code(), tonic::Code::FailedPrecondition);
    assert_eq!(
        server.calls.lock().unwrap().window_actions,
        vec![("w2".to_owned(), WindowAction::Minimize)]
    );
    let _ = HashMap::<(), ()>::new();
}

#[tokio::test]
async fn v2_ticket_subprotocol_attaches_and_token_rotation_closes_with_4401() {
    ticket_subprotocol_attaches_and_token_rotation_closes_with_4401(
        cua_media_protocol::v2::WS_TICKET_SUBPROTOCOL_PREFIX,
    )
    .await;
}

#[tokio::test]
async fn legacy_rcdp_ticket_subprotocol_still_attaches() {
    ticket_subprotocol_attaches_and_token_rotation_closes_with_4401(
        cua_media_protocol::v2::LEGACY_WS_TICKET_SUBPROTOCOL_PREFIX,
    )
    .await;
}

#[tokio::test]
async fn ticket_subprotocol_without_wire_subprotocol_is_echoed() {
    for prefix in [
        cua_media_protocol::v2::WS_TICKET_SUBPROTOCOL_PREFIX,
        cua_media_protocol::v2::LEGACY_WS_TICKET_SUBPROTOCOL_PREFIX,
    ] {
        let server = server().await;
        let opened = StreamServiceClient::new(server.channel.clone())
            .open_media(OpenMediaRequest {
                target: Some(MediaTarget {
                    target: Some(media_target::Target::DisplayId("primary".into())),
                }),
                codecs: vec![MediaCodec::Bgra as i32],
                ..OpenMediaRequest::default()
            })
            .await
            .unwrap()
            .into_inner();
        use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
        let protocol = format!("{prefix}{}", opened.ticket);
        let mut request = format!("ws://{}/media", server.address)
            .into_client_request()
            .unwrap();
        request
            .headers_mut()
            .insert("sec-websocket-protocol", protocol.parse().unwrap());
        let (_socket, response) = tokio_tungstenite::connect_async(request).await.unwrap();
        assert_eq!(
            response.headers().get("sec-websocket-protocol").unwrap(),
            protocol.as_str()
        );
        // A wrong ticket in either form is refused during the upgrade.
        let mut request = format!("ws://{}/media", server.address)
            .into_client_request()
            .unwrap();
        request.headers_mut().insert(
            "sec-websocket-protocol",
            format!("rcdp.v2, {prefix}not-a-ticket").parse().unwrap(),
        );
        match tokio_tungstenite::connect_async(request).await {
            Err(tungstenite::Error::Http(response)) => assert_eq!(response.status(), 401),
            other => panic!("expected 401, got {:?}", other.map(|(_, r)| r.status())),
        }
    }
}

async fn ticket_subprotocol_attaches_and_token_rotation_closes_with_4401(prefix: &str) {
    let server = server().await;
    let mut stream = StreamServiceClient::new(server.channel.clone());
    let opened = stream
        .open_media(OpenMediaRequest {
            target: Some(MediaTarget {
                target: Some(media_target::Target::DisplayId("primary".into())),
            }),
            codecs: vec![MediaCodec::Bgra as i32],
            ..OpenMediaRequest::default()
        })
        .await
        .unwrap()
        .into_inner();
    // Header-free form from MEDIA.md: `rcdp.v2` + `<prefix><ticket>`.
    use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
    let mut request = format!("ws://{}/media", server.address)
        .into_client_request()
        .unwrap();
    request.headers_mut().insert(
        "sec-websocket-protocol",
        format!("rcdp.v2, {prefix}{}", opened.ticket)
            .parse()
            .unwrap(),
    );
    let (mut socket, response) = tokio_tungstenite::connect_async(request).await.unwrap();
    assert_eq!(
        response.headers().get("sec-websocket-protocol").unwrap(),
        "rcdp.v2"
    );
    let hello = read_until(&mut socket, 1, |message| match message {
        tungstenite::Message::Text(text) => Some(decode_server_text(text).unwrap()),
        _ => None,
    })
    .await;
    assert!(matches!(hello, ServerMessage::Hello(_)));
    server.ctx.notify_token_rotated();
    let code = read_until(&mut socket, 64, |message| match message {
        tungstenite::Message::Close(Some(frame)) => Some(u16::from(frame.code)),
        _ => None,
    })
    .await;
    assert_eq!(code, 4401);
}

// ---------------------------------------------------------------- QUIC media

async fn quic_connect(port: u16, pin: &str) -> Result<quinn::Connection, String> {
    let pin = cua_media_transport::quic::parse_sha256(pin).ok_or("bad pin")?;
    let mut endpoint = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap())
        .map_err(|error| error.to_string())?;
    endpoint.set_default_client_config(cua_media_transport::quic::client_config(pin)?);
    let connecting = endpoint
        .connect(
            SocketAddr::from(([127, 0, 0, 1], port)),
            "cua-spacesd.local",
        )
        .map_err(|error| error.to_string())?;
    tokio::time::timeout(Duration::from_secs(5), connecting)
        .await
        .map_err(|_| "connect timeout".to_string())?
        .map_err(|error| error.to_string())
}

async fn quic_send(send: &mut quinn::SendStream, message: &cua_media_protocol::v2::ClientMessage) {
    let text = cua_media_protocol::v2::encode_client_text(message);
    cua_media_transport::quic::write_message(send, text.as_bytes())
        .await
        .unwrap();
}

async fn quic_read_control(receive: &mut quinn::RecvStream) -> ServerMessage {
    let bytes = tokio::time::timeout(
        Duration::from_secs(5),
        cua_media_transport::quic::read_message(receive),
    )
    .await
    .expect("control message within 5 s")
    .unwrap()
    .expect("stream open");
    decode_server_text(&String::from_utf8(bytes).unwrap()).unwrap()
}

/// Tiny deterministic PRNG for the simulated loss.
fn next_random(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

#[tokio::test]
async fn quic_media_pins_the_certificate_requires_a_ticket_and_recovers_from_datagram_loss_with_a_keyframe(
) {
    use cua_media_protocol::v2::{close_code, ClientMessage};
    let server = server().await;
    let quic = server
        .provider
        .start_quic("127.0.0.1:0".parse().unwrap())
        .unwrap();
    let mut stream = StreamServiceClient::new(server.channel.clone());
    let opened = stream
        .open_media(OpenMediaRequest {
            target: Some(MediaTarget {
                target: Some(media_target::Target::DisplayId("primary".into())),
            }),
            codecs: vec![MediaCodec::Bgra as i32],
            prefer_quic: true,
            ..OpenMediaRequest::default()
        })
        .await
        .unwrap()
        .into_inner();
    let endpoint = opened
        .quic
        .clone()
        .expect("QUIC endpoint offered when prefer_quic");
    assert_eq!(endpoint.port, u32::from(quic.port));
    assert_eq!(endpoint.certificate_sha256, quic.certificate_sha256);

    // A wrong pin fails the handshake.
    assert!(quic_connect(quic.port, &"00".repeat(32)).await.is_err());

    // A bad ticket closes with the QUIC mirror of 4401.
    let connection = quic_connect(quic.port, &endpoint.certificate_sha256)
        .await
        .unwrap();
    let (mut send, _receive) = connection.open_bi().await.unwrap();
    quic_send(
        &mut send,
        &ClientMessage::Ticket {
            ticket: "nope".into(),
        },
    )
    .await;
    let reason = tokio::time::timeout(Duration::from_secs(5), connection.closed())
        .await
        .unwrap();
    match reason {
        quinn::ConnectionError::ApplicationClosed(close) => {
            assert_eq!(
                close.error_code.into_inner(),
                u64::from(close_code::quic_error(close_code::TICKET_INVALID))
            );
        }
        other => panic!("expected application close, got {other:?}"),
    }

    // The real ticket: server-first greeting on the reliable stream, video in RVD2 datagrams.
    let connection = quic_connect(quic.port, &endpoint.certificate_sha256)
        .await
        .unwrap();
    let (mut send, mut receive) = connection.open_bi().await.unwrap();
    quic_send(
        &mut send,
        &ClientMessage::Ticket {
            ticket: opened.ticket.clone(),
        },
    )
    .await;
    assert!(
        matches!(quic_read_control(&mut receive).await, ServerMessage::Hello(ref hello) if hello.selected_version == 2)
    );
    let session_id = match quic_read_control(&mut receive).await {
        ServerMessage::SessionOpened(session) => {
            assert_eq!(session.session_id.0, opened.media_session_id);
            session.session_id
        }
        other => panic!("unexpected {other:?}"),
    };
    // Drain the reliable stream in the background (bounded by the connection lifetime).
    tokio::spawn(async move {
        for _ in 0..10_000 {
            if cua_media_transport::quic::read_message(&mut receive)
                .await
                .ok()
                .flatten()
                .is_none()
            {
                return;
            }
        }
    });

    // Deltas stream continuously while the client drops ~8% of datagrams.
    let capture = server.capture.clone();
    let emitter = tokio::spawn(async move {
        for _ in 0..500 {
            capture.emit_frame(false);
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    });
    let mut reassembler = cua_media_transport::VideoDatagramReassembler::default();
    let mut random = 0x9e37_79b9_7f4a_7c15u64;
    let (mut first_keyframe, mut losses, mut dropped_datagrams, mut recovered) =
        (false, 0u64, 0u64, false);
    let mut keyframe_requests = 0u32;
    for _ in 0..20_000 {
        let datagram = tokio::time::timeout(Duration::from_secs(5), connection.read_datagram())
            .await
            .expect("datagram within 5 s")
            .unwrap();
        // Simulated loss only after the attach keyframe arrived.
        if first_keyframe && next_random(&mut random) % 100 < 8 {
            dropped_datagrams += 1;
            continue;
        }
        let update = reassembler
            .push(&datagram, std::time::Instant::now())
            .unwrap();
        if update.dropped_incomplete > 0 {
            losses += update.dropped_incomplete;
            if keyframe_requests < 50 {
                keyframe_requests += 1;
                quic_send(
                    &mut send,
                    &ClientMessage::RequestKeyframe {
                        session_id: session_id.clone(),
                    },
                )
                .await;
            }
        }
        let Some(packet) = update.packet else {
            continue;
        };
        let (header, payload) = cua_media_transport::decode_packet(&packet).unwrap();
        let cua_media_protocol::WireHeader::Video(descriptor) = header else {
            panic!("datagrams carry only video here")
        };
        assert_eq!(payload.len(), 64 * 48 * 4);
        if !first_keyframe {
            assert!(descriptor.keyframe, "attach starts with a keyframe");
            first_keyframe = true;
        } else if descriptor.keyframe && losses > 0 {
            recovered = true;
            break;
        }
    }
    emitter.abort();
    assert!(first_keyframe);
    assert!(
        dropped_datagrams > 0 && losses > 0,
        "loss was simulated ({dropped_datagrams} datagrams, {losses} packets)"
    );
    assert!(recovered, "a keyframe arrived after the loss");

    // CloseMedia closes the QUIC connection with the mirror of 4404.
    stream
        .close_media(CloseMediaRequest {
            media_session_id: opened.media_session_id.clone(),
        })
        .await
        .unwrap();
    match tokio::time::timeout(Duration::from_secs(5), connection.closed())
        .await
        .unwrap()
    {
        quinn::ConnectionError::ApplicationClosed(close) => {
            assert_eq!(
                close.error_code.into_inner(),
                u64::from(close_code::quic_error(close_code::SESSION_CLOSED))
            );
        }
        other => panic!("expected application close, got {other:?}"),
    }
    quic.close();
}

#[tokio::test]
async fn health_reports_the_desktop_component() {
    use cua_proto::env::v1::HealthStatus;
    use cua_spacesd_server::ServiceProvider as _;
    let server = server().await;
    let components = server.provider.health();
    let desktop = components
        .iter()
        .find(|c| c.name == "desktop")
        .expect("a desktop component");
    assert_eq!(desktop.status, HealthStatus::Serving as i32);
    assert!(desktop.detail.is_empty());

    let starting = super::desktop_component(Err("no window manager is running yet".into()));
    assert_eq!(starting.name, "desktop");
    assert_eq!(starting.status, HealthStatus::NotServing as i32);
    assert_eq!(starting.detail, "no window manager is running yet");
}

// ------------------------------------------------ presence v5 (shapes)

/// A hit-test-only backend over the fake 640x480 display: the left half is a
/// text field, the right half a link. Never moves anything.
struct HitTestOnly;

impl cua_driver_core::pointer_shape::PointerShapeBackend for HitTestOnly {
    fn names(&self) -> cua_driver_core::pointer_shape::BackendNames {
        cua_driver_core::pointer_shape::BackendNames {
            hit_test: "fake",
            system: "",
            probe: "",
        }
    }
    fn hit_test(&self, x: f64, _y: f64) -> Option<cua_driver_core::pointer_shape::HitTest> {
        use cua_driver_core::cursor_shape::SystemCursorShape;
        Some(cua_driver_core::pointer_shape::HitTest {
            shape: if x < 320.0 {
                SystemCursorShape::Text
            } else {
                SystemCursorShape::Pointer
            },
            role: "fake".into(),
            window: None,
            element: None,
        })
    }
    fn pointer_position(&self) -> Option<(f64, f64)> {
        None
    }
    fn warp_pointer(&self, _x: f64, _y: f64) -> bool {
        false
    }
    fn limitation(&self) -> Option<String> {
        Some("fake backend".into())
    }
}

/// Events of a Join stream until `pick` matches, bounded by count and time.
async fn next_matching<T>(
    stream: &mut tonic::Streaming<cua_proto::env::v1::JoinResponse>,
    limit: usize,
    mut pick: impl FnMut(&join_response::Event) -> Option<T>,
) -> Option<T> {
    for _ in 0..limit {
        let Ok(Some(Ok(event))) = tokio::time::timeout(Duration::from_secs(3), stream.next()).await
        else {
            return None;
        };
        if let Some(found) = event.event.as_ref().and_then(&mut pick) {
            return Some(found);
        }
    }
    None
}

#[tokio::test]
async fn presence_shapes_are_negotiated_and_old_clients_are_untouched() {
    use cua_proto::env::v1::{CursorShape, CursorShapeSource, LeaveReason};
    use cua_spacesd_server::ServiceProvider as _;
    let server = server().await;
    let feature = |provider: &DesktopServiceProvider| {
        provider
            .capabilities()
            .into_iter()
            .find(|feature| feature.name == "presence.cursor_shape")
            .unwrap()
    };
    assert!(
        !feature(&server.provider).supported,
        "no backend, no shapes"
    );
    server.provider.with_pointer_shapes(Arc::new(HitTestOnly));
    let shapes = feature(&server.provider);
    assert!(shapes.supported);
    assert_eq!(shapes.attributes["hit_test"], "fake");
    assert_eq!(shapes.attributes["probe"], "none");
    assert_eq!(shapes.limitation, "fake backend");

    let mut old_client = PresenceServiceClient::new(server.channel.clone());
    let mut new_client = PresenceServiceClient::new(server.channel.clone());
    let mut old = old_client
        .join(JoinRequest {
            principal: Some(principal("old", PrincipalKind::Human)),
            ..JoinRequest::default()
        })
        .await
        .unwrap()
        .into_inner();
    let old_me = next_matching(&mut old, 1, |e| match e {
        join_response::Event::Joined(joined) => joined.participant.clone(),
        _ => None,
    })
    .await
    .unwrap();
    let mut new = new_client
        .join(JoinRequest {
            principal: Some(principal("new", PrincipalKind::Human)),
            cursor_shapes: true,
            cursor_batches: true,
            roster_interval: Some(cua_proto::wkt::Duration {
                seconds: 1,
                nanos: 0,
            }),
            ..JoinRequest::default()
        })
        .await
        .unwrap()
        .into_inner();
    let new_me = next_matching(&mut new, 1, |e| match e {
        join_response::Event::Joined(joined) => {
            assert!(joined.datagrams.is_none(), "not requested");
            joined.participant.clone()
        }
        _ => None,
    })
    .await
    .unwrap();
    // The new client's own cursor gets a shape (sent to itself).
    new_client
        .update_cursor(UpdateCursorRequest {
            participant_id: new_me.participant_id.clone(),
            cursor: Some(CursorPosition {
                display_id: "0".into(),
                position: Some(Point { x: 0.25, y: 0.5 }),
                visible: true,
                ..CursorPosition::default()
            }),
        })
        .await
        .unwrap();
    let own = next_matching(&mut new, 20, |e| match e {
        join_response::Event::CursorShapeChanged(changed)
            if changed.participant_id == new_me.participant_id =>
        {
            Some(changed.clone())
        }
        _ => None,
    })
    .await
    .expect("own shape");
    assert_eq!(own.shape, CursorShape::Text as i32);
    assert_eq!(own.source, CursorShapeSource::HitTest as i32);
    // The old client moves; the new client sees it shaped, in a batch.
    old_client
        .update_cursor(UpdateCursorRequest {
            participant_id: old_me.participant_id.clone(),
            cursor: Some(CursorPosition {
                display_id: "0".into(),
                position: Some(Point { x: 0.75, y: 0.5 }),
                visible: true,
                ..CursorPosition::default()
            }),
        })
        .await
        .unwrap();
    // Moves go out on the next tick; the shape follows when the prober has
    // it (in `CursorShapeChanged`, and in every later move).
    let (mut batched, mut shaped) = (false, false);
    next_matching(&mut new, 40, |e| {
        match e {
            join_response::Event::CursorBatch(batch) => {
                batched |= batch
                    .moves
                    .iter()
                    .any(|m| m.participant_id == old_me.participant_id);
            }
            join_response::Event::CursorShapeChanged(changed)
                if changed.participant_id == old_me.participant_id =>
            {
                shaped |= changed.shape == CursorShape::Pointer as i32;
            }
            join_response::Event::CursorMoved(_) => panic!("batched clients get no singles"),
            _ => {}
        }
        (batched && shaped).then_some(())
    })
    .await
    .expect("the old client's move in a batch, and its shape");
    let beat = next_matching(&mut new, 60, |e| match e {
        join_response::Event::RosterHeartbeat(beat) => Some(beat.clone()),
        _ => None,
    })
    .await
    .expect("a heartbeat within the interval");
    assert!(beat.participant_ids.contains(&old_me.participant_id));
    // The old client never sees shapes or the new events.
    let new_cursor = next_matching(&mut old, 20, |e| match e {
        join_response::Event::CursorMoved(moved) => moved.cursor.clone(),
        join_response::Event::CursorShapeChanged(_)
        | join_response::Event::CursorBatch(_)
        | join_response::Event::RosterHeartbeat(_) => panic!("old client got {e:?}"),
        _ => None,
    })
    .await
    .expect("the new client's cursor");
    assert_eq!(new_cursor.shape, 0);
    // The old client goes away: an explicit leave with a reason.
    drop(old);
    let left = next_matching(&mut new, 60, |e| match e {
        join_response::Event::ParticipantLeft(left)
            if left.participant_id == old_me.participant_id =>
        {
            Some(left.reason)
        }
        _ => None,
    })
    .await;
    assert_eq!(left, Some(LeaveReason::Disconnected as i32));
}

#[tokio::test]
async fn an_agent_run_ending_removes_its_cursor() {
    use cua_proto::env::v1::LeaveReason;
    let server = server().await;
    server.provider.install_agent_cursor_hook();
    let mut client = PresenceServiceClient::new(server.channel.clone());
    let mut events = client
        .join(JoinRequest {
            principal: Some(principal("watcher", PrincipalKind::Human)),
            ..JoinRequest::default()
        })
        .await
        .unwrap()
        .into_inner();
    let run = "agent-run-presence-e2e-41c9";
    server.provider.presence().agent_cursor(
        &format!("cua-driver:{run}"),
        "Agent",
        "0",
        None,
        (0.5, 0.5),
    );
    let agent = next_matching(&mut events, 10, |e| match e {
        join_response::Event::ParticipantJoined(p) => Some(p.participant_id.clone()),
        _ => None,
    })
    .await
    .unwrap();
    cua_driver_core::session::end_session(run);
    let left = next_matching(&mut events, 20, |e| match e {
        join_response::Event::ParticipantLeft(left) if left.participant_id == agent => {
            Some(left.reason)
        }
        _ => None,
    })
    .await;
    assert_eq!(left, Some(LeaveReason::RunEnded as i32));
}

#[tokio::test]
async fn join_with_datagrams_hands_out_the_quic_channel() {
    let server = server().await;
    let quic = server
        .provider
        .start_quic("127.0.0.1:0".parse().unwrap())
        .unwrap();
    let mut client = PresenceServiceClient::new(server.channel.clone());
    let mut events = client
        .join(JoinRequest {
            principal: Some(principal("dg", PrincipalKind::Human)),
            cursor_datagrams: true,
            ..JoinRequest::default()
        })
        .await
        .unwrap()
        .into_inner();
    let datagrams = next_matching(&mut events, 1, |e| match e {
        join_response::Event::Joined(joined) => joined.datagrams.clone(),
        _ => None,
    })
    .await
    .expect("datagram channel offered");
    let endpoint = datagrams.endpoint.unwrap();
    assert_eq!(endpoint.port, u32::from(quic.port));
    assert_eq!(endpoint.alpn, "cua-presence/1");
    assert_eq!(endpoint.certificate_sha256, quic.certificate_sha256);
    assert!(!datagrams.ticket.is_empty());
    // The ticket works on the real listener (the media ALPN still does too).
    let mut client_endpoint = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
    client_endpoint.set_default_client_config(
        cua_media_transport::quic::client_config_with_alpn(
            cua_media_transport::quic::parse_sha256(&endpoint.certificate_sha256).unwrap(),
            b"cua-presence/1",
        )
        .unwrap(),
    );
    let connection = client_endpoint
        .connect(quic.local_addr().unwrap(), "cua-spacesd.local")
        .unwrap()
        .await
        .unwrap();
    let (mut send, mut receive) = connection.open_bi().await.unwrap();
    let mut line = serde_json::to_vec(
        &cua_media_protocol::presence::PresenceControl::PresenceTicket {
            ticket: datagrams.ticket,
        },
    )
    .unwrap();
    line.push(b'\n');
    send.write_all(&line).await.unwrap();
    let mut buffer = vec![0u8; 4096];
    let read = tokio::time::timeout(Duration::from_secs(5), receive.read(&mut buffer))
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let text = String::from_utf8_lossy(&buffer[..read]);
    assert!(text.starts_with(r#"{"type":"slots""#), "{text}");
    connection.close(0u32.into(), b"");
    quic.close();
}

/// Regression (Omarchy): a human clicking through a media stream showed a
/// second cursor named "CUA agent". The viewer's media input must be
/// attributed to its own presence participant, and the driver's cursor
/// moves for that input must never publish an agent participant.
#[tokio::test]
async fn human_media_input_is_attributed_to_the_viewer_and_never_an_agent() {
    let server = server().await;
    let mut client = PresenceServiceClient::new(server.channel.clone());
    let mut dana_events = client
        .join(JoinRequest {
            principal: Some(Principal {
                id: "user:dana@example.com".into(),
                display_name: "Dana".into(),
                color: String::new(),
                kind: PrincipalKind::Human as i32,
            }),
            ..JoinRequest::default()
        })
        .await
        .unwrap()
        .into_inner();
    let dana = match dana_events.next().await.unwrap().unwrap().event {
        Some(join_response::Event::Joined(joined)) => joined.participant.unwrap(),
        other => panic!("unexpected {other:?}"),
    };
    let mut watcher = client
        .join(JoinRequest {
            principal: Some(principal("watcher", PrincipalKind::Human)),
            ..JoinRequest::default()
        })
        .await
        .unwrap()
        .into_inner();
    next_matching(&mut watcher, 4, |e| match e {
        join_response::Event::Joined(_) => Some(()),
        _ => None,
    })
    .await
    .unwrap();

    // The viewer names its participant when it opens the stream; the
    // connection itself carries no principal (the apps' local path).
    let mut stream = StreamServiceClient::new(server.channel.clone());
    let open = |participant: &str| OpenMediaRequest {
        target: Some(MediaTarget {
            target: Some(media_target::Target::DisplayId("primary".into())),
        }),
        codecs: vec![MediaCodec::Bgra as i32],
        policy: SessionPolicy::AllowActivation as i32,
        presence_participant_id: participant.into(),
        ..OpenMediaRequest::default()
    };
    let opened = stream
        .open_media(open(&dana.participant_id))
        .await
        .unwrap()
        .into_inner();
    let session = server
        .provider
        .state
        .media
        .session(&opened.media_session_id)
        .unwrap();
    assert_eq!(session.principal().id, "user:dana@example.com");
    assert_eq!(session.principal().name, "Dana");
    assert!(!session.principal().agent);
    // An unknown participant falls back to the connection's principal.
    let unknown = stream
        .open_media(open("p-nobody"))
        .await
        .unwrap()
        .into_inner();
    let unknown = server
        .provider
        .state
        .media
        .session(&unknown.media_session_id)
        .unwrap();
    assert_eq!(unknown.principal().id, "anonymous");
    assert!(!unknown.principal().agent);

    // Dana's input runs in her human-origin driver session: the driver's
    // cursor moves for it publish nothing.
    let human = format!(
        "__cua_runtime_test:{}",
        crate::human_input_session("user:dana@example.com")
    );
    cua_driver_core::agent_cursor::set_input_origin(
        &human,
        cua_driver_core::agent_cursor::InputOrigin::Human,
    );
    let move_to = |cursor_id: &str| cua_driver_core::cursor_hook::CursorHookEvent {
        cursor_id: cursor_id.into(),
        x: 320.0,
        y: 240.0,
        pressed: true,
    };
    super::publish_driver_cursor(&server.provider.state, &move_to(&human));
    // An agent's driver session still shows as an agent, which also proves
    // the watcher would have seen a human-origin cursor had it published.
    super::publish_driver_cursor(&server.provider.state, &move_to("agent-run-human-cursor-7"));
    let joined = next_matching(&mut watcher, 10, |e| match e {
        join_response::Event::ParticipantJoined(p) => Some(p.principal.clone().unwrap()),
        _ => None,
    })
    .await
    .unwrap();
    assert_eq!(joined.kind, PrincipalKind::Agent as i32);
    assert_eq!(joined.id, "cua-driver:agent-run-human-cursor-7");
    let agents = server
        .provider
        .presence()
        .principals()
        .into_iter()
        .filter(|principal| principal.kind == PrincipalKind::Agent as i32)
        .map(|principal| principal.id)
        .collect::<Vec<_>>();
    assert_eq!(
        agents,
        vec!["cua-driver:agent-run-human-cursor-7".to_owned()]
    );
    cua_driver_core::agent_cursor::forget_input_origin(&human);
}

/// A caller the server authenticated cannot attribute its media input to
/// someone else's presence participant.
#[test]
fn an_authenticated_caller_cannot_borrow_another_participant() {
    let hub = super::presence::PresenceHub::new();
    let (alice, _, _receiver) = hub.join(principal("alice", PrincipalKind::Human));
    let bob = principal("bob", PrincipalKind::Human);
    let resolve = |authenticated: Option<&Principal>| {
        super::stream::viewer_principal(
            &hub,
            Some(bob.clone()),
            authenticated,
            &alice.participant_id,
        )
        .unwrap()
        .id
    };
    assert_eq!(resolve(None), "alice");
    assert_eq!(resolve(Some(&bob)), "bob");
    assert_eq!(
        resolve(Some(&principal("alice", PrincipalKind::Human))),
        "alice"
    );
}

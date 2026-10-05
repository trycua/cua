// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Linux desktop conformance tests (plan §2.5): desktop and per-window
//! streaming, background input, presence and accessibility against a real X
//! server.
//!
//! These run ONLY inside the test container (`tests/docker/run-tests.sh`:
//! Xvfb + openbox + AT-SPI + PulseAudio). They are skipped unless
//! `CUA_ENV_LINUX_DESKTOP_TESTS=1`, and the whole file is Linux-only, so a
//! developer's host desktop is never driven.
//!
//! Oracles are independent of the code under test: fixture JSONL logs
//! (what the fixture window actually received), `xdotool` for the focused
//! window and pointer position, and an OpenH264 decode of the streamed video.
#![cfg(target_os = "linux")]

use std::net::SocketAddr;
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use cua_media_protocol::v2::{decode_server_text, ServerMessage};
use cua_media_protocol::{VideoFrameDescriptor, WindowLifecycleEvent, WireHeader};
use cua_proto::env::v1::{
    accessibility_service_client::AccessibilityServiceClient,
    computer_service_client::ComputerServiceClient, join_response, keyboard_request, media_target,
    pointer_request, presence_service_client::PresenceServiceClient, process_data, process_event,
    process_service_client::ProcessServiceClient, stream_service_client::StreamServiceClient,
    windows_service_client::WindowsServiceClient, AccessibilityAction, AccessibilityQuery,
    ActRequest, CloseWindowRequest, CoordinateSpace, CursorPosition, Delivery, ElementRef,
    FindRequest, InputTarget, JoinRequest, KeyboardRequest, KeyboardType, ListTargetsRequest,
    ListWindowsRequest, MediaCodec, MediaTarget, OpenMediaRequest, Point, PointerClick,
    PointerMove, PointerRequest, Principal, PrincipalKind, ProcessConfig, SessionPolicy,
    SetWindowBoundsRequest, StartProcessRequest, UpdateCursorRequest, WindowFilter, WindowInfo,
    WindowRef,
};
use cua_spacesd_desktop::grpc::DesktopServiceProvider;
use cua_spacesd_server::{ServerConfig, ServerContext};
use futures_util::StreamExt as _;
use prost::Message as _;
use tokio_tungstenite::tungstenite;

// ------------------------------------------------------------------ harness

/// Decoded width, height and BGRA.
type DecodedPicture = (usize, usize, Vec<u8>);

fn enabled() -> bool {
    if std::env::var_os("CUA_ENV_LINUX_DESKTOP_TESTS").is_some() {
        return true;
    }
    eprintln!("skipped: set CUA_ENV_LINUX_DESKTOP_TESTS=1 (run tests/docker/run-tests.sh)");
    false
}

/// A running driver to test against: `CUA_ENV_TEST_TARGET` (for example
/// `http://127.0.0.1:3211`, with `CUA_ENV_TEST_TOKEN`) targets a real
/// `cua-spacesd` binary, whose fixtures are started and inspected through
/// its own ProcessService; otherwise an in-process driver is started.
fn remote_target() -> Option<String> {
    std::env::var("CUA_ENV_TEST_TARGET")
        .ok()
        .filter(|value| !value.is_empty())
}

/// Base URL (`http://host:port`) of the driver under test.
fn base_url() -> String {
    match remote_target() {
        Some(target) => target.trim_end_matches('/').to_owned(),
        None => format!("http://{}", server()),
    }
}

fn ws_url(path: &str) -> String {
    format!("{}{path}", base_url().replacen("http", "ws", 1))
}

/// One in-process driver for the whole test binary, on its own runtime.
fn server() -> SocketAddr {
    static ADDRESS: OnceLock<SocketAddr> = OnceLock::new();
    *ADDRESS.get_or_init(|| {
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(4)
                .enable_all()
                .build()
                .expect("runtime");
            runtime.block_on(async move {
                let provider = DesktopServiceProvider::for_host(ServerContext::new(
                    ServerConfig::default(),
                    None,
                ))
                .expect("Linux desktop backend");
                let router = provider.standalone_router();
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                    .await
                    .expect("bind");
                sender
                    .send(listener.local_addr().expect("address"))
                    .expect("send");
                axum::serve(listener, router).await.expect("serve");
            });
        });
        receiver
            .recv_timeout(Duration::from_secs(30))
            .expect("driver started")
    })
}

/// Adds the root token (`CUA_ENV_TEST_TOKEN`) to every call.
#[derive(Clone)]
struct Auth(Option<tonic::metadata::MetadataValue<tonic::metadata::Ascii>>);

impl tonic::service::Interceptor for Auth {
    fn call(
        &mut self,
        mut request: tonic::Request<()>,
    ) -> Result<tonic::Request<()>, tonic::Status> {
        if let Some(value) = &self.0 {
            request
                .metadata_mut()
                .insert("authorization", value.clone());
        }
        Ok(request)
    }
}

type Ch = tonic::service::interceptor::InterceptedService<tonic::transport::Channel, Auth>;

async fn channel() -> Ch {
    channel_to(&base_url()).await
}

async fn channel_to(url: &str) -> Ch {
    let channel = tonic::transport::Channel::from_shared(url.to_owned())
        .unwrap()
        .connect()
        .await
        .expect("connect to driver");
    let token = std::env::var("CUA_ENV_TEST_TOKEN")
        .ok()
        .filter(|token| !token.is_empty())
        .map(|token| format!("Bearer {token}").parse().expect("ascii token"));
    tonic::service::interceptor::InterceptedService::new(channel, Auth(token))
}

fn principal(id: &str, kind: PrincipalKind) -> Principal {
    Principal {
        id: id.into(),
        display_name: id.into(),
        color: String::new(),
        kind: kind as i32,
    }
}

fn as_principal<T>(message: T, principal: &Principal) -> tonic::Request<T> {
    let mut request = tonic::Request::new(message);
    request.metadata_mut().insert_bin(
        cua_proto::metadata::PRINCIPAL_BIN,
        tonic::metadata::MetadataValue::from_bytes(&principal.encode_to_vec()),
    );
    request
}

/// Run a command where the desktop is (locally, or in the target through
/// its ProcessService) and return its stdout.
async fn run(program: &str, args: &[&str]) -> String {
    if remote_target().is_none() {
        let output = Command::new(program)
            .args(args)
            .output()
            .expect("run command");
        return String::from_utf8_lossy(&output.stdout).trim().to_owned();
    }
    let mut process = ProcessServiceClient::new(channel().await);
    let mut events = process
        .start_process(StartProcessRequest {
            config: Some(ProcessConfig {
                command: program.into(),
                args: args.iter().map(|arg| (*arg).to_owned()).collect(),
                ..ProcessConfig::default()
            }),
            ..StartProcessRequest::default()
        })
        .await
        .expect("start remote process")
        .into_inner();
    let mut stdout = Vec::new();
    for _ in 0..10_000 {
        let Ok(Some(Ok(event))) =
            tokio::time::timeout(Duration::from_secs(20), events.next()).await
        else {
            break;
        };
        match event.event.and_then(|event| event.event) {
            Some(process_event::Event::Data(data)) => {
                if let Some(process_data::Output::Stdout(bytes)) = data.output {
                    stdout.extend_from_slice(&bytes);
                    assert!(stdout.len() < 16 * 1024 * 1024, "remote output too large");
                }
            }
            Some(process_event::Event::End(_)) => break,
            _ => {}
        }
    }
    String::from_utf8_lossy(&stdout).trim().to_owned()
}

/// Start a long-running fixture (detached in the target).
async fn spawn_fixture(program: &str, args: &[String]) -> Option<Child> {
    if remote_target().is_none() {
        return Some(
            Command::new(program)
                .args(args)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("start fixture"),
        );
    }
    let mut process = ProcessServiceClient::new(channel().await);
    let mut events = process
        .start_process(StartProcessRequest {
            config: Some(ProcessConfig {
                command: program.into(),
                args: args.to_vec(),
                ..ProcessConfig::default()
            }),
            ..StartProcessRequest::default()
        })
        .await
        .expect("start remote fixture")
        .into_inner();
    // Processes are detached: dropping the stream leaves the fixture running.
    let _ = tokio::time::timeout(Duration::from_secs(10), events.next()).await;
    None
}

/// A fixture process, stopped when dropped (we started it; nothing else is
/// ever signalled).
struct Fixture {
    child: Option<Child>,
    log: String,
    title: String,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
            return;
        }
        // Remote: stop it through the driver from a helper thread.
        let title = self.title.clone();
        let _ = std::thread::spawn(move || {
            if let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                runtime.block_on(async move {
                    run("pkill", &["-f", &title]).await;
                });
            }
        })
        .join();
    }
}

impl Fixture {
    async fn events(&self) -> Vec<serde_json::Value> {
        let text = if remote_target().is_some() {
            run("cat", &[&self.log]).await
        } else {
            std::fs::read_to_string(&self.log).unwrap_or_default()
        };
        text.lines()
            .take(10_000)
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    async fn wait_for(&self, event: &str, timeout: Duration) -> serde_json::Value {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(found) = self
                .events()
                .await
                .into_iter()
                .find(|value| value["event"] == event)
            {
                return found;
            }
            assert!(
                Instant::now() < deadline,
                "{}: no {event} event within {timeout:?}",
                self.title
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn typed(&self) -> String {
        self.events()
            .await
            .iter()
            .filter(|value| value["event"] == "key")
            .filter_map(|value| value["char"].as_str().map(str::to_owned))
            .collect()
    }
}

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}-{}",
        std::process::id(),
        Instant::now().elapsed().as_nanos() % 100_000 + rand_suffix()
    )
}

fn rand_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
        % 1_000_000
}

async fn pad(title_prefix: &str, args: &[&str]) -> Fixture {
    let title = unique(title_prefix);
    let log = format!("/tmp/{title}.jsonl");
    let binary =
        std::env::var("CUA_ENV_X11_PAD").expect("CUA_ENV_X11_PAD points at cua-spacesd-x11-pad");
    let mut full: Vec<String> = vec!["--title".into(), title.clone(), "--log".into(), log.clone()];
    full.extend(args.iter().map(|arg| (*arg).to_owned()));
    let child = spawn_fixture(&binary, &full).await;
    let fixture = Fixture { child, log, title };
    fixture.wait_for("ready", Duration::from_secs(15)).await;
    fixture
}

async fn xdotool(args: &[&str]) -> String {
    run("xdotool", args).await
}

/// Logical size of the primary display (frames are scaled from it).
async fn display_size(channel: &Ch) -> (f64, f64) {
    let displays = ComputerServiceClient::new(channel.clone())
        .list_displays(cua_proto::env::v1::ListDisplaysRequest {})
        .await
        .unwrap()
        .into_inner()
        .displays;
    let display = displays
        .iter()
        .find(|display| display.primary)
        .or(displays.first())
        .expect("a display");
    let bounds = display.bounds.as_ref().expect("bounds");
    (bounds.width, bounds.height)
}

async fn find_window(channel: &Ch, title: &str) -> WindowInfo {
    let mut windows = WindowsServiceClient::new(channel.clone());
    for _ in 0..100 {
        let listed = windows
            .list_windows(ListWindowsRequest {
                filter: Some(WindowFilter {
                    title_contains: title.into(),
                    ..WindowFilter::default()
                }),
            })
            .await
            .unwrap()
            .into_inner();
        if let Some(window) = listed.windows.into_iter().next() {
            return window;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("window {title} never listed");
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

enum Item {
    Control(ServerMessage),
    Video(VideoFrameDescriptor, Vec<u8>),
    Audio(Vec<u8>),
    Closed(u16),
}

/// Next media-plane item (bounded wait).
async fn next(socket: &mut Socket, timeout: Duration) -> Option<Item> {
    loop {
        let message = tokio::time::timeout(timeout, socket.next())
            .await
            .ok()??
            .ok()?;
        return Some(match message {
            tungstenite::Message::Text(text) => {
                Item::Control(decode_server_text(&text).expect("valid control"))
            }
            tungstenite::Message::Binary(bytes) if bytes.starts_with(b"RAU2") => {
                Item::Audio(bytes.to_vec())
            }
            tungstenite::Message::Binary(bytes) => {
                let (header, payload) =
                    cua_media_transport::decode_packet(&bytes).expect("valid packet");
                match header {
                    WireHeader::Video(descriptor) => Item::Video(descriptor, payload),
                    _ => continue,
                }
            }
            tungstenite::Message::Close(frame) => {
                Item::Closed(frame.map(|f| u16::from(f.code)).unwrap_or(1005))
            }
            _ => continue,
        });
    }
}

async fn open_and_attach(
    channel: &Ch,
    target: media_target::Target,
    codec: MediaCodec,
) -> (String, Socket, Instant) {
    let mut stream = StreamServiceClient::new(channel.clone());
    let opened = stream
        .open_media(OpenMediaRequest {
            target: Some(MediaTarget {
                target: Some(target),
            }),
            codecs: vec![codec as i32],
            max_fps: 30,
            bitrate_kbps: 2_000,
            policy: SessionPolicy::ViewOnly as i32,
            ..OpenMediaRequest::default()
        })
        .await
        .expect("open media")
        .into_inner();
    let attached_at = Instant::now();
    let (socket, _) = tokio_tungstenite::connect_async(ws_url(&opened.ws_path))
        .await
        .expect("attach");
    (opened.media_session_id, socket, attached_at)
}

/// Decode an H.264 access unit to RGB with OpenH264 (independent decoder).
struct Decoder(openh264::decoder::Decoder);

impl Decoder {
    fn new() -> Self {
        Self(openh264::decoder::Decoder::new().expect("decoder"))
    }

    fn decode(&mut self, access_unit: &[u8]) -> Option<(usize, usize, Vec<u8>)> {
        use openh264::formats::YUVSource;
        let picture = self.0.decode(access_unit).ok()??;
        let (width, height) = picture.dimensions();
        let mut rgb = vec![0u8; width * height * 3];
        picture.write_rgb8(&mut rgb);
        Some((width, height, rgb))
    }
}

fn pixel(frame: &(usize, usize, Vec<u8>), x: usize, y: usize) -> [u8; 3] {
    let index = (y.min(frame.1 - 1) * frame.0 + x.min(frame.0 - 1)) * 3;
    [frame.2[index], frame.2[index + 1], frame.2[index + 2]]
}

fn close_to(actual: [u8; 3], expected: [u8; 3]) -> bool {
    actual
        .iter()
        .zip(expected)
        .all(|(a, e)| (i16::from(*a) - i16::from(e)).abs() <= 48)
}

// ------------------------------------------------------------------ tests

#[tokio::test]
async fn desktop_stream_keyframe_on_attach_colors_and_static_bandwidth() {
    if !enabled() {
        return;
    }
    let channel = channel().await;
    let grid = pad(
        "grid",
        &[
            "--grid", "--x", "200", "--y", "150", "--width", "400", "--height", "300",
        ],
    )
    .await;
    let window = find_window(&channel, &grid.title).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (_, mut socket, attached_at) = open_and_attach(
        &channel,
        media_target::Target::DisplayId("primary".into()),
        MediaCodec::H264,
    )
    .await;
    let mut decoder = Decoder::new();
    let mut first: Option<(VideoFrameDescriptor, DecodedPicture)> = None;
    let mut control = Vec::new();
    for _ in 0..50 {
        match next(&mut socket, Duration::from_secs(3))
            .await
            .expect("media item")
        {
            Item::Control(message) => control.push(message),
            Item::Video(descriptor, payload) => {
                let latency = attached_at.elapsed();
                assert!(
                    descriptor.keyframe,
                    "the first video packet after attach is a keyframe"
                );
                assert!(
                    latency < Duration::from_secs(1),
                    "keyframe after {latency:?}"
                );
                first = Some((
                    descriptor,
                    decoder.decode(&payload).expect("keyframe decodes"),
                ));
                break;
            }
            other => panic!("unexpected {:?}", std::mem::discriminant(&other)),
        }
    }
    assert!(matches!(control.first(), Some(ServerMessage::Hello(_))));
    assert!(matches!(
        control.get(1),
        Some(ServerMessage::SessionOpened(_))
    ));
    let (descriptor, frame) = first.expect("a keyframe");
    // Sample the center of each grid quadrant (fixture paints R, G / B, W).
    let bounds = window.bounds.unwrap();
    let (x, y, w, h) = (bounds.x, bounds.y, bounds.width, bounds.height);
    let (display_w, display_h) = display_size(&channel).await;
    let scale_x = frame.0 as f64 / display_w;
    let scale_y = frame.1 as f64 / display_h;
    let sample = |fx: f64, fy: f64| {
        pixel(
            &frame,
            ((x + w * fx) * scale_x) as usize,
            ((y + h * fy) * scale_y) as usize,
        )
    };
    for (point, expected) in [
        ((0.25, 0.25), [255, 0, 0]),
        ((0.75, 0.25), [0, 255, 0]),
        ((0.25, 0.75), [0, 0, 255]),
        ((0.75, 0.75), [255, 255, 255]),
    ] {
        let actual = sample(point.0, point.1);
        assert!(
            close_to(actual, expected),
            "quadrant {point:?}: got {actual:?}, want {expected:?}"
        );
    }
    // Static screen: after the IDR, bytes stay under 50 KB/s.
    let window_started = Instant::now();
    let mut bytes = 0usize;
    let mut last_sequence = descriptor.sequence.0;
    while window_started.elapsed() < Duration::from_secs(3) {
        if let Some(Item::Video(descriptor, payload)) =
            next(&mut socket, Duration::from_millis(200)).await
        {
            assert!(descriptor.sequence.0 > last_sequence);
            last_sequence = descriptor.sequence.0;
            bytes += payload.len();
        }
    }
    let rate = bytes as f64 / window_started.elapsed().as_secs_f64();
    assert!(rate < 50_000.0, "static desktop used {rate:.0} B/s");
    // A change produces new frames (the frame counter advances).
    let mut windows = WindowsServiceClient::new(channel.clone());
    windows
        .set_window_bounds(SetWindowBoundsRequest {
            window: window.r#ref.clone(),
            position: Some(Point { x: 260.0, y: 180.0 }),
            width: None,
            height: None,
        })
        .await
        .unwrap();
    let mut advanced = false;
    for _ in 0..50 {
        if let Some(Item::Video(descriptor, _)) = next(&mut socket, Duration::from_secs(2)).await {
            advanced = descriptor.sequence.0 > last_sequence;
            break;
        }
    }
    assert!(advanced, "moving a window produced a new frame");
}

#[tokio::test]
async fn window_stream_is_occlusion_free_and_reports_resize_title_and_close() {
    if !enabled() {
        return;
    }
    let channel = channel().await;
    let a = pad(
        "win-a",
        &[
            "--color",
            "ff0000",
            "--x",
            "100",
            "--y",
            "100",
            "--width",
            "400",
            "--height",
            "300",
            "--rename-after-ms",
            "4000",
        ],
    )
    .await;
    let window_a = find_window(&channel, &a.title).await;
    let b = pad(
        "win-b",
        &[
            "--color", "0000ff", "--x", "250", "--y", "200", "--width", "400", "--height", "300",
        ],
    )
    .await;
    let _window_b = find_window(&channel, &b.title).await;
    // Phantom windows (the driver's own overlay, input-only helpers) are
    // never streamable targets.
    let mut stream = StreamServiceClient::new(channel.clone());
    let targets = stream
        .list_targets(ListTargetsRequest {
            include_windows: true,
            window_filter: None,
        })
        .await
        .unwrap()
        .into_inner();
    for target in &targets.targets {
        if let Some(cua_proto::env::v1::stream_target::Target::Window(window)) = &target.target {
            assert_ne!(
                window.app.as_ref().map(|app| app.pid),
                Some(std::process::id()),
                "own overlay listed"
            );
            let bounds = window.bounds.as_ref().unwrap();
            assert!(
                bounds.width > 2.0 && bounds.height > 2.0,
                "degenerate window listed: {window:?}"
            );
        }
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (_, mut socket, _) = open_and_attach(
        &channel,
        media_target::Target::Window(window_a.r#ref.clone().unwrap()),
        MediaCodec::H264,
    )
    .await;
    let mut decoder = Decoder::new();
    let mut geometry_epoch = 0;
    let frame = loop {
        match next(&mut socket, Duration::from_secs(3))
            .await
            .expect("media item")
        {
            Item::Control(ServerMessage::SessionOpened(opened)) => {
                geometry_epoch = opened.geometry_epoch.0
            }
            Item::Video(descriptor, payload) => {
                assert!(descriptor.keyframe);
                break decoder.decode(&payload).expect("decodes");
            }
            _ => {}
        }
    };
    // B covers A's lower-right area; A's own pixels are still red there.
    let overlapped = pixel(&frame, frame.0 * 3 / 4, frame.1 * 3 / 4);
    assert!(
        close_to(overlapped, [255, 0, 0]),
        "occluded region is {overlapped:?}, want red (Composite)"
    );
    // Resize: a new geometry epoch, then an IDR at the new size.
    let mut windows = WindowsServiceClient::new(channel.clone());
    windows
        .set_window_bounds(SetWindowBoundsRequest {
            window: window_a.r#ref.clone(),
            position: None,
            width: Some(500.0),
            height: Some(360.0),
        })
        .await
        .unwrap();
    let mut saw_geometry = false;
    let mut saw_idr = false;
    for _ in 0..200 {
        match next(&mut socket, Duration::from_secs(3))
            .await
            .expect("media item")
        {
            Item::Control(ServerMessage::Lifecycle {
                event:
                    WindowLifecycleEvent::GeometryChanged {
                        geometry_epoch: epoch,
                        geometry,
                    },
                ..
            }) => {
                assert!(epoch.0 > geometry_epoch);
                assert_eq!((geometry.width_px, geometry.height_px), (500, 360));
                saw_geometry = true;
            }
            Item::Video(descriptor, _) if saw_geometry => {
                assert!(descriptor.keyframe, "first frame at the new size is an IDR");
                assert_eq!(descriptor.width_px, 500);
                saw_idr = true;
                break;
            }
            _ => {}
        }
    }
    assert!(saw_geometry && saw_idr);
    // Title change (the fixture renames itself) is a lifecycle event.
    let mut renamed = false;
    for _ in 0..200 {
        if let Some(Item::Control(ServerMessage::Lifecycle {
            event: WindowLifecycleEvent::TitleChanged { title },
            ..
        })) = next(&mut socket, Duration::from_secs(6)).await
        {
            renamed = title.ends_with("(renamed)");
            break;
        }
    }
    assert!(renamed, "title change reported");
    // Closing the window ends the stream: lifecycle closed, then 4410.
    windows
        .close_window(CloseWindowRequest {
            window: window_a.r#ref.clone(),
            force: false,
        })
        .await
        .unwrap();
    let mut closed_event = false;
    let mut close_code = None;
    for _ in 0..200 {
        match next(&mut socket, Duration::from_secs(5)).await {
            Some(Item::Control(ServerMessage::Lifecycle {
                event: WindowLifecycleEvent::Closed,
                ..
            })) => closed_event = true,
            Some(Item::Closed(code)) => {
                close_code = Some(code);
                break;
            }
            None => break,
            _ => {}
        }
    }
    assert!(closed_event, "lifecycle closed");
    assert_eq!(close_code, Some(4410));
    drop(b);
}

#[tokio::test]
async fn background_input_from_two_principals_leaves_focus_and_pointer_alone() {
    if !enabled() {
        return;
    }
    let channel = channel().await;
    let a = pad(
        "bg-a",
        &[
            "--color", "202020", "--x", "40", "--y", "40", "--width", "300", "--height", "200",
        ],
    )
    .await;
    let b = pad(
        "bg-b",
        &[
            "--color", "404040", "--x", "400", "--y", "40", "--width", "300", "--height", "200",
        ],
    )
    .await;
    let c = pad(
        "bg-c",
        &[
            "--color", "606060", "--x", "40", "--y", "400", "--width", "300", "--height", "200",
        ],
    )
    .await;
    let window_a = find_window(&channel, &a.title).await;
    let window_b = find_window(&channel, &b.title).await;
    let window_c = find_window(&channel, &c.title).await;
    // Focus C and park the pointer on it (foreground setup, then oracle).
    let mut windows = WindowsServiceClient::new(channel.clone());
    windows
        .activate_window(cua_proto::env::v1::ActivateWindowRequest {
            window: window_c.r#ref.clone(),
        })
        .await
        .unwrap();
    let mut computer = ComputerServiceClient::new(channel.clone());
    computer
        .pointer(PointerRequest {
            target: Some(InputTarget {
                delivery: Delivery::Foreground as i32,
                space: CoordinateSpace::Screen as i32,
                ..InputTarget::default()
            }),
            action: Some(pointer_request::Action::Move(PointerMove {
                position: Some(Point { x: 150.0, y: 500.0 }),
                duration: None,
            })),
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let focused_before = xdotool(&["getactivewindow"]).await;
    let pointer_before = xdotool(&["getmouselocation"]).await;
    let drive = |window: WindowInfo, text: &'static str, who: Principal| {
        let mut computer = ComputerServiceClient::new(channel.clone());
        async move {
            let target = InputTarget {
                window: window.r#ref.clone(),
                delivery: Delivery::Background as i32,
                space: CoordinateSpace::Normalized as i32,
                ..InputTarget::default()
            };
            let clicked = computer
                .pointer(as_principal(
                    PointerRequest {
                        target: Some(target.clone()),
                        action: Some(pointer_request::Action::Click(PointerClick {
                            position: Some(Point { x: 0.5, y: 0.5 }),
                            button: 0,
                            count: 1,
                            modifiers: vec![],
                        })),
                    },
                    &who,
                ))
                .await
                .unwrap()
                .into_inner();
            let report = clicked.report.unwrap();
            assert_eq!(report.delivery, Delivery::Background as i32);
            assert!(!report.pointer_moved && !report.focus_changed);
            computer
                .keyboard(as_principal(
                    KeyboardRequest {
                        target: Some(target),
                        action: Some(keyboard_request::Action::Type(KeyboardType {
                            text: text.into(),
                            mode: 0,
                            delay: None,
                        })),
                    },
                    &who,
                ))
                .await
                .unwrap();
        }
    };
    tokio::join!(
        drive(
            window_a.clone(),
            "alpha",
            principal("alice", PrincipalKind::Human)
        ),
        drive(
            window_b.clone(),
            "bravo",
            principal("bob", PrincipalKind::Agent)
        ),
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(a.typed().await, "alpha");
    assert_eq!(b.typed().await, "bravo");
    assert_eq!(c.typed().await, "", "the focused window received nothing");
    for fixture in [&a, &b] {
        let events = fixture.events().await;
        let buttons: Vec<_> = events
            .iter()
            .filter(|event| event["event"] == "button")
            .collect();
        assert_eq!(buttons.len(), 1, "{}: exactly one click", fixture.title);
        assert!(events
            .iter()
            .filter(|event| event["event"] == "key" || event["event"] == "button")
            .all(|event| event["send_event"] == true));
    }
    assert_eq!(
        xdotool(&["getactivewindow"]).await,
        focused_before,
        "focus unchanged"
    );
    assert_eq!(
        xdotool(&["getmouselocation"]).await,
        pointer_before,
        "pointer unchanged"
    );
}

#[tokio::test]
async fn presence_two_clients_and_an_agent_cursor() {
    if !enabled() {
        return;
    }
    let channel = channel().await;
    let mut alice = PresenceServiceClient::new(channel.clone());
    let mut bob = PresenceServiceClient::new(channel.clone());
    let mut alice_events = alice
        .join(JoinRequest {
            principal: Some(principal("alice-p", PrincipalKind::Human)),
            keepalive_interval: None,
            ..JoinRequest::default()
        })
        .await
        .unwrap()
        .into_inner();
    let alice_participant = match alice_events.next().await.unwrap().unwrap().event {
        Some(join_response::Event::Joined(joined)) => joined.participant.unwrap(),
        other => panic!("{other:?}"),
    };
    let mut bob_events = bob
        .join(JoinRequest {
            principal: Some(principal("bob-p", PrincipalKind::Human)),
            keepalive_interval: None,
            ..JoinRequest::default()
        })
        .await
        .unwrap()
        .into_inner();
    let bob_participant = match bob_events.next().await.unwrap().unwrap().event {
        Some(join_response::Event::Joined(joined)) => joined.participant.unwrap(),
        other => panic!("{other:?}"),
    };
    assert_ne!(
        alice_participant.principal.as_ref().unwrap().color,
        bob_participant.principal.as_ref().unwrap().color,
        "server-assigned colors differ"
    );
    bob.update_cursor(UpdateCursorRequest {
        participant_id: bob_participant.participant_id.clone(),
        cursor: Some(CursorPosition {
            display_id: "0".into(),
            window: None,
            position: Some(Point { x: 0.3, y: 0.4 }),
            visible: true,
            ..CursorPosition::default()
        }),
    })
    .await
    .unwrap();
    let mut computer = ComputerServiceClient::new(channel.clone());
    computer
        .pointer(as_principal(
            PointerRequest {
                target: Some(InputTarget {
                    delivery: Delivery::Foreground as i32,
                    space: CoordinateSpace::Normalized as i32,
                    ..InputTarget::default()
                }),
                action: Some(pointer_request::Action::Move(PointerMove {
                    position: Some(Point { x: 0.5, y: 0.5 }),
                    duration: None,
                })),
            },
            &principal("agent-x", PrincipalKind::Agent),
        ))
        .await
        .unwrap();
    let mut saw_bob = false;
    let mut saw_agent = false;
    for _ in 0..50 {
        let Ok(Some(Ok(event))) =
            tokio::time::timeout(Duration::from_secs(3), alice_events.next()).await
        else {
            break;
        };
        match event.event {
            Some(join_response::Event::CursorMoved(moved))
                if moved.participant_id == bob_participant.participant_id =>
            {
                saw_bob = true
            }
            Some(join_response::Event::CursorMoved(moved))
                if moved.participant_id != alice_participant.participant_id =>
            {
                saw_agent = true
            }
            _ => {}
        }
        if saw_bob && saw_agent {
            break;
        }
    }
    assert!(saw_bob, "alice sees bob's cursor");
    assert!(saw_agent, "alice sees the agent's cursor");
}

#[tokio::test]
async fn accessibility_finds_and_presses_a_gtk_button() {
    if !enabled() {
        return;
    }
    let channel = channel().await;
    let title = unique("gtk");
    let log = format!("/tmp/{title}.jsonl");
    let script = std::env::var("CUA_ENV_GTK_FIXTURE").expect("CUA_ENV_GTK_FIXTURE");
    let child = spawn_fixture(
        "python3",
        &[
            script,
            "--title".into(),
            title.clone(),
            "--log".into(),
            log.clone(),
        ],
    )
    .await;
    let fixture = Fixture {
        child,
        log,
        title: title.clone(),
    };
    fixture.wait_for("ready", Duration::from_secs(15)).await;
    let window = find_window(&channel, &title).await;
    let mut a11y = AccessibilityServiceClient::new(channel.clone());
    let mut found = None;
    for _ in 0..30 {
        let response = a11y
            .find(FindRequest {
                window: window.r#ref.clone(),
                query: Some(AccessibilityQuery {
                    name_contains: "Press Me".into(),
                    ..AccessibilityQuery::default()
                }),
                max_results: 5,
            })
            .await;
        if let Ok(response) = response {
            let response = response.into_inner();
            if let Some(node) = response
                .nodes
                .into_iter()
                .find(|node| node.role == "button")
            {
                found = Some((response.snapshot_id, node));
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    let (snapshot_id, node) = found.expect("the GTK button is visible over AT-SPI");
    a11y.act(ActRequest {
        element: Some(ElementRef {
            snapshot_id,
            element_id: node.element_id,
        }),
        action: AccessibilityAction::Press as i32,
        ..ActRequest::default()
    })
    .await
    .expect("press");
    fixture.wait_for("clicked", Duration::from_secs(5)).await;
    let _: Option<WindowRef> = None;
}

/// GTK3 drops synthetic (XSendEvent) input. The driver must deliver or
/// refuse, never report a click the widget did not get: an AUTO click lands
/// on the button (the fixture's JSONL log is the oracle), and an explicit
/// BACKGROUND click is refused with FAILED_PRECONDITION / WOULD_REQUIRE_ACTIVATION
/// and leaves the log unchanged.
#[tokio::test]
async fn input_to_gtk_is_delivered_or_refused_never_faked() {
    if !enabled() {
        return;
    }
    let channel = channel().await;
    let title = unique("gtk-input");
    let log = format!("/tmp/{title}.jsonl");
    let script = std::env::var("CUA_ENV_GTK_FIXTURE").expect("CUA_ENV_GTK_FIXTURE");
    let child = spawn_fixture(
        "python3",
        &[
            script,
            "--title".into(),
            title.clone(),
            "--log".into(),
            log.clone(),
        ],
    )
    .await;
    let fixture = Fixture {
        child,
        log,
        title: title.clone(),
    };
    fixture.wait_for("ready", Duration::from_secs(15)).await;
    let window = find_window(&channel, &title).await;
    let mut a11y = AccessibilityServiceClient::new(channel.clone());
    let mut bounds = None;
    for _ in 0..30 {
        if let Ok(response) = a11y
            .find(FindRequest {
                window: window.r#ref.clone(),
                query: Some(AccessibilityQuery {
                    name_contains: "Press Me".into(),
                    ..AccessibilityQuery::default()
                }),
                max_results: 5,
            })
            .await
        {
            bounds = response
                .into_inner()
                .nodes
                .into_iter()
                .find(|node| node.role == "button")
                .and_then(|node| node.bounds)
                .filter(|b| b.width > 0.0 && b.height > 0.0);
            if bounds.is_some() {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    let b = bounds.expect("the GTK button's screen bounds over AT-SPI");
    let center = Point {
        x: b.x + b.width / 2.0,
        y: b.y + b.height / 2.0,
    };
    let clicks = |events: &[serde_json::Value]| {
        events
            .iter()
            .filter(|event| event["event"] == "clicked")
            .count()
    };
    let click = |delivery: Delivery, window: Option<WindowRef>| PointerRequest {
        target: Some(InputTarget {
            window,
            delivery: delivery as i32,
            space: CoordinateSpace::Screen as i32,
            ..InputTarget::default()
        }),
        action: Some(pointer_request::Action::Click(PointerClick {
            position: Some(center),
            button: 0,
            count: 1,
            modifiers: vec![],
        })),
    };
    let mut computer = ComputerServiceClient::new(channel.clone());

    // BACKGROUND to the window: refused, nothing delivered.
    let refused = computer
        .pointer(click(Delivery::Background, window.r#ref.clone()))
        .await
        .expect_err("background input to GTK must not report success");
    assert_eq!(
        refused.code(),
        tonic::Code::FailedPrecondition,
        "{refused:?}"
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(clicks(&fixture.events().await), 0, "nothing reached GTK");

    // AUTO, with the window and with only the point: delivered, and the
    // report says how.
    for (i, window) in [window.r#ref.clone(), None].into_iter().enumerate() {
        let report = computer
            .pointer(click(Delivery::Auto, window))
            .await
            .expect("auto click")
            .into_inner()
            .report
            .expect("report");
        assert_eq!(report.delivery, Delivery::Foreground as i32, "{report:?}");
        assert!(report.pointer_moved, "{report:?}");
        let deadline = Instant::now() + Duration::from_secs(5);
        while clicks(&fixture.events().await) < i + 1 {
            assert!(
                Instant::now() < deadline,
                "AUTO click #{} never reached the GTK button: {:?}",
                i + 1,
                fixture.events().await
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    assert_eq!(clicks(&fixture.events().await), 2, "exactly one click each");
}

/// A media-plane `action` click (pixel basis) on a GTK window reaches the
/// widget, per the fixture's JSONL log, and `ActionResult.delivered` is only
/// true once the X server processed the input. Regression: the XTest path
/// flushed and dropped its connection, the server discarded the fake input
/// under runc, and the result still said delivered.
#[tokio::test]
async fn media_action_click_reaches_gtk() {
    use cua_media_protocol::v2::{encode_client_text, ClientMessage};
    use cua_media_protocol::{ActionBasis, ActionRequest};
    use futures_util::SinkExt as _;
    if !enabled() {
        return;
    }
    let channel = channel().await;
    let title = unique("gtk-action");
    let log = format!("/tmp/{title}.jsonl");
    let script = std::env::var("CUA_ENV_GTK_FIXTURE").expect("CUA_ENV_GTK_FIXTURE");
    let child = spawn_fixture(
        "python3",
        &[
            script,
            "--title".into(),
            title.clone(),
            "--log".into(),
            log.clone(),
        ],
    )
    .await;
    let fixture = Fixture {
        child,
        log,
        title: title.clone(),
    };
    fixture.wait_for("ready", Duration::from_secs(15)).await;
    let window = find_window(&channel, &title).await;
    let window_bounds = window.bounds.expect("window bounds");
    let mut a11y = AccessibilityServiceClient::new(channel.clone());
    let mut button = None;
    for _ in 0..30 {
        if let Ok(response) = a11y
            .find(FindRequest {
                window: window.r#ref.clone(),
                query: Some(AccessibilityQuery {
                    name_contains: "Press Me".into(),
                    ..AccessibilityQuery::default()
                }),
                max_results: 5,
            })
            .await
        {
            button = response
                .into_inner()
                .nodes
                .into_iter()
                .find(|node| node.role == "button")
                .and_then(|node| node.bounds)
                .filter(|b| b.width > 0.0 && b.height > 0.0);
            if button.is_some() {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    let button = button.expect("the GTK button's screen bounds over AT-SPI");

    let mut stream = StreamServiceClient::new(channel.clone());
    let opened = stream
        .open_media(OpenMediaRequest {
            target: Some(MediaTarget {
                target: Some(media_target::Target::Window(window.r#ref.clone().unwrap())),
            }),
            codecs: vec![MediaCodec::H264 as i32],
            max_fps: 30,
            bitrate_kbps: 2_000,
            policy: SessionPolicy::AllowActivation as i32,
            ..OpenMediaRequest::default()
        })
        .await
        .expect("open media")
        .into_inner();
    let (mut socket, _) = tokio_tungstenite::connect_async(ws_url(&opened.ws_path))
        .await
        .expect("attach");
    let descriptor = loop {
        match next(&mut socket, Duration::from_secs(5))
            .await
            .expect("media item")
        {
            Item::Video(descriptor, _) => break descriptor,
            Item::Closed(code) => panic!("media closed ({code})"),
            _ => {}
        }
    };
    // Button center in the streamed frame's pixel space.
    let sx = f64::from(descriptor.width_px) / window_bounds.width;
    let sy = f64::from(descriptor.height_px) / window_bounds.height;
    let x = ((button.x + button.width / 2.0) - window_bounds.x) * sx;
    let y = ((button.y + button.height / 2.0) - window_bounds.y) * sy;
    let clicks = |events: &[serde_json::Value]| {
        events
            .iter()
            .filter(|event| event["event"] == "clicked")
            .count()
    };
    for round in 1..=3usize {
        let action_id = format!("gtk-{round}");
        let message = ClientMessage::Action(ActionRequest {
            action_id: action_id.clone(),
            session_id: descriptor.session_id.clone(),
            tool: "click".into(),
            arguments: serde_json::json!({"x": x.round(), "y": y.round()}),
            basis: ActionBasis::Pixel {
                geometry_epoch: descriptor.geometry_epoch,
                frame_sequence: descriptor.sequence,
            },
        });
        socket
            .send(tungstenite::Message::Text(encode_client_text(&message)))
            .await
            .expect("send action");
        let mut result = None;
        for _ in 0..500 {
            match next(&mut socket, Duration::from_secs(5)).await {
                Some(Item::Control(ServerMessage::ActionResult(r))) if r.action_id == action_id => {
                    result = Some(r);
                    break;
                }
                Some(Item::Closed(code)) => panic!("media closed ({code})"),
                Some(_) => {}
                None => break,
            }
        }
        let result = result.expect("an ActionResult");
        assert!(result.delivered, "{result:?}");
        // The oracle: the widget saw exactly this click, promptly.
        let deadline = Instant::now() + Duration::from_secs(3);
        while clicks(&fixture.events().await) < round {
            assert!(
                Instant::now() < deadline,
                "action {action_id} reported delivered but GTK logged {:?}",
                fixture.events().await
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    assert_eq!(clicks(&fixture.events().await), 3);
}

/// Open a media session on `window` with `policy` and return its socket and
/// media session id (from the first video descriptor).
async fn open_input_session(
    channel: &Ch,
    window: &WindowInfo,
    policy: SessionPolicy,
) -> (Socket, cua_media_protocol::WindowSessionId) {
    let mut stream = StreamServiceClient::new(channel.clone());
    let opened = stream
        .open_media(OpenMediaRequest {
            target: Some(MediaTarget {
                target: Some(media_target::Target::Window(window.r#ref.clone().unwrap())),
            }),
            codecs: vec![MediaCodec::H264 as i32],
            max_fps: 30,
            bitrate_kbps: 2_000,
            policy: policy as i32,
            ..OpenMediaRequest::default()
        })
        .await
        .expect("open media")
        .into_inner();
    let (mut socket, _) = tokio_tungstenite::connect_async(ws_url(&opened.ws_path))
        .await
        .expect("attach");
    let session_id = loop {
        match next(&mut socket, Duration::from_secs(5))
            .await
            .expect("media item")
        {
            Item::Video(descriptor, _) => break descriptor.session_id,
            Item::Closed(code) => panic!("media closed ({code})"),
            _ => {}
        }
    };
    (socket, session_id)
}

/// Send one interactive batch and wait (bounded) for its acknowledgement.
async fn send_input(
    socket: &mut Socket,
    session_id: &cua_media_protocol::WindowSessionId,
    first_sequence: u64,
    events: Vec<cua_media_protocol::InteractiveInputEvent>,
) -> cua_media_protocol::InteractiveInputAcknowledgement {
    use cua_media_protocol::v2::{encode_client_text, ClientMessage};
    use futures_util::SinkExt as _;
    let through = first_sequence + events.len() as u64 - 1;
    let message = ClientMessage::InteractiveInput(cua_media_protocol::InteractiveInputBatch {
        session_id: session_id.clone(),
        first_sequence,
        events,
    });
    socket
        .send(tungstenite::Message::Text(encode_client_text(&message)))
        .await
        .expect("send input");
    for _ in 0..500 {
        match next(socket, Duration::from_secs(5)).await {
            Some(Item::Control(ServerMessage::InteractiveInputAcknowledgement(ack)))
                if ack.through_sequence == through =>
            {
                return ack;
            }
            Some(Item::Control(ServerMessage::Error { code, message, .. })) => {
                panic!("input rejected: {code:?} {message}")
            }
            Some(Item::Closed(code)) => panic!("media closed ({code})"),
            Some(_) => {}
            None => break,
        }
    }
    panic!("no acknowledgement for input through {through}");
}

fn click_and_text(text: &str) -> Vec<cua_media_protocol::InteractiveInputEvent> {
    use cua_media_protocol::{InputPointerButton, InputPointerPhase, InteractiveInputEvent};
    let pointer = |phase| InteractiveInputEvent::Pointer {
        phase,
        button: Some(InputPointerButton::Left),
        x_normalized: 0.5,
        y_normalized: 0.5,
        modifiers: Vec::new(),
    };
    vec![
        pointer(InputPointerPhase::Down),
        pointer(InputPointerPhase::Up),
        InteractiveInputEvent::TextCommit { text: text.into() },
    ]
}

/// Media-plane interactive batches are delivered by cua-driver's native
/// session: ALLOW_ACTIVATION delivers real (XTest) input, BACKGROUND_ONLY
/// delivers target-addressed (XSendEvent) input without moving focus or the
/// pointer, and a background session on a toolkit that drops synthetic input
/// is refused instead of acknowledged as delivered.
#[tokio::test]
async fn interactive_input_is_delivered_by_cua_driver_or_refused() {
    if !enabled() {
        return;
    }
    let channel = channel().await;

    // Foreground: real input.
    let fg = pad(
        "ia-fg",
        &[
            "--color", "305030", "--x", "40", "--y", "40", "--width", "300", "--height", "200",
        ],
    )
    .await;
    let window = find_window(&channel, &fg.title).await;
    let (mut socket, session_id) =
        open_input_session(&channel, &window, SessionPolicy::AllowActivation).await;
    let ack = send_input(&mut socket, &session_id, 1, click_and_text("fg")).await;
    assert!(ack.delivered, "{ack:?}");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(fg.typed().await, "fg");
    let events = fg.events().await;
    let buttons: Vec<_> = events
        .iter()
        .filter(|event| event["event"] == "button")
        .collect();
    assert!(!buttons.is_empty(), "the pad saw the click: {events:?}");
    assert!(
        events
            .iter()
            .filter(|event| event["event"] == "key" || event["event"] == "button")
            .all(|event| event["send_event"] == false),
        "foreground input is real (XTest) input"
    );
    drop(socket);

    // Background: target-addressed, focus and pointer untouched.
    let bg = pad(
        "ia-bg",
        &[
            "--color", "503030", "--x", "400", "--y", "40", "--width", "300", "--height", "200",
        ],
    )
    .await;
    let other = pad(
        "ia-other",
        &[
            "--color", "303050", "--x", "40", "--y", "400", "--width", "300", "--height", "200",
        ],
    )
    .await;
    let bg_window = find_window(&channel, &bg.title).await;
    let other_window = find_window(&channel, &other.title).await;
    WindowsServiceClient::new(channel.clone())
        .activate_window(cua_proto::env::v1::ActivateWindowRequest {
            window: other_window.r#ref.clone(),
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let focused_before = xdotool(&["getactivewindow"]).await;
    let pointer_before = xdotool(&["getmouselocation"]).await;
    let (mut socket, session_id) =
        open_input_session(&channel, &bg_window, SessionPolicy::BackgroundOnly).await;
    let ack = send_input(&mut socket, &session_id, 1, click_and_text("bg")).await;
    assert!(ack.delivered, "{ack:?}");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(bg.typed().await, "bg");
    assert_eq!(
        other.typed().await,
        "",
        "the focused window received nothing"
    );
    assert!(bg
        .events()
        .await
        .iter()
        .filter(|event| event["event"] == "key" || event["event"] == "button")
        .all(|event| event["send_event"] == true));
    assert_eq!(xdotool(&["getactivewindow"]).await, focused_before);
    assert_eq!(xdotool(&["getmouselocation"]).await, pointer_before);
    drop(socket);

    // Background to GTK: refused, never faked.
    let title = unique("ia-gtk");
    let log = format!("/tmp/{title}.jsonl");
    let script = std::env::var("CUA_ENV_GTK_FIXTURE").expect("CUA_ENV_GTK_FIXTURE");
    let child = spawn_fixture(
        "python3",
        &[
            script,
            "--title".into(),
            title.clone(),
            "--log".into(),
            log.clone(),
        ],
    )
    .await;
    let gtk = Fixture {
        child,
        log,
        title: title.clone(),
    };
    gtk.wait_for("ready", Duration::from_secs(15)).await;
    let gtk_window = find_window(&channel, &title).await;
    let (mut socket, session_id) =
        open_input_session(&channel, &gtk_window, SessionPolicy::BackgroundOnly).await;
    let ack = send_input(&mut socket, &session_id, 1, click_and_text("x")).await;
    assert!(
        !ack.delivered,
        "background input to GTK must be refused: {ack:?}"
    );
    let error = ack.error.expect("refusal carries an error");
    assert_eq!(
        error.code,
        cua_media_protocol::ActionErrorCode::WouldRequireActivation,
        "{error:?}"
    );
}

// ------------------------------------------------------------------ audio

/// Goertzel power of `frequency` in mono `samples` at 48 kHz.
fn goertzel(samples: &[f64], frequency: f64) -> f64 {
    let coefficient = 2.0 * (std::f64::consts::TAU * frequency / 48_000.0).cos();
    let (mut previous, mut before) = (0.0, 0.0);
    for sample in samples {
        let current = sample + coefficient * previous - before;
        before = previous;
        previous = current;
    }
    previous * previous + before * before - coefficient * previous * before
}

struct AudioPacket {
    header: cua_media_transport::audio::AudioPacketHeader,
    mono: Vec<f64>,
}

fn pcm_packet(bytes: &[u8]) -> AudioPacket {
    let (header, payload) =
        cua_media_transport::audio::decode_audio_packet(bytes).expect("valid RAU2");
    let mono = payload
        .as_chunks::<4>()
        .0
        .iter()
        .map(|frame| {
            (f64::from(i16::from_le_bytes([frame[0], frame[1]]))
                + f64::from(i16::from_le_bytes([frame[2], frame[3]])))
                / 2.0
        })
        .collect();
    AudioPacket { header, mono }
}

async fn open_audio(
    channel: &Ch,
    codec: cua_proto::env::v1::AudioCodec,
    video: bool,
) -> Option<(cua_proto::env::v1::OpenMediaResponse, Socket)> {
    let mut stream = StreamServiceClient::new(channel.clone());
    let targets = stream
        .list_targets(ListTargetsRequest {
            include_windows: false,
            window_filter: None,
        })
        .await
        .unwrap()
        .into_inner();
    if targets.audio_sources.is_empty() {
        eprintln!("no audio backend: {}", targets.audio_uplink_limitation);
        return None;
    }
    let opened = stream
        .open_media(OpenMediaRequest {
            target: Some(MediaTarget {
                target: Some(media_target::Target::DisplayId("primary".into())),
            }),
            codecs: vec![MediaCodec::H264 as i32],
            max_fps: 60,
            policy: SessionPolicy::ViewOnly as i32,
            disable_video: !video,
            audio: Some(cua_proto::env::v1::AudioOptions {
                enabled: true,
                source_ids: vec![],
                encoding: Some(cua_proto::env::v1::AudioEncoding {
                    codecs: vec![codec as i32],
                    sample_rate_hz: 48_000,
                    channels: 2,
                    frame_ms: 10,
                    ..cua_proto::env::v1::AudioEncoding::default()
                }),
                uplink: None,
            }),
            ..OpenMediaRequest::default()
        })
        .await
        .expect("open media with audio")
        .into_inner();
    let (socket, _) = tokio_tungstenite::connect_async(ws_url(&opened.ws_path))
        .await
        .expect("attach");
    Some((opened, socket))
}

#[tokio::test]
async fn desktop_audio_carries_a_tone_with_contiguous_timestamps() {
    if !enabled() {
        return;
    }
    let channel = channel().await;
    let _tone = pad(
        "tone",
        &[
            "--color",
            "101010",
            "--x",
            "900",
            "--y",
            "500",
            "--width",
            "100",
            "--height",
            "100",
            "--tone-hz",
            "440",
        ],
    )
    .await;
    let Some((opened, mut socket)) =
        open_audio(&channel, cua_proto::env::v1::AudioCodec::PcmS16le, false).await
    else {
        panic!("the test container runs PulseAudio; audio must be available");
    };
    let audio = opened.audio.expect("negotiated audio");
    assert_eq!(audio.tracks.len(), 1);
    let track_id = audio.tracks[0].track_id as u16;
    let mut saw_config = false;
    let mut packets = Vec::new();
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(4) && packets.len() < 300 {
        match next(&mut socket, Duration::from_secs(2)).await {
            Some(Item::Control(ServerMessage::AudioConfig(config))) => {
                assert_eq!(config.track_id, track_id);
                assert_eq!(config.sample_rate_hz, 48_000);
                saw_config = true;
            }
            Some(Item::Audio(bytes)) => {
                assert!(saw_config, "audio_config precedes audio packets");
                packets.push(pcm_packet(&bytes));
            }
            Some(_) => {}
            None => break,
        }
    }
    assert!(
        packets.len() >= 100,
        "received {} audio packets",
        packets.len()
    );
    // Contiguous media-clock timestamps: pts advances by exactly one frame.
    let mut contiguous = 0;
    for pair in packets.windows(2) {
        if pair[1].header.sequence == pair[0].header.sequence.wrapping_add(1) {
            let expected = u64::from(pair[0].header.frame_samples) * 1_000_000 / 48_000;
            if pair[1].header.pts_us - pair[0].header.pts_us == expected {
                contiguous += 1;
            }
        }
    }
    assert!(
        contiguous * 10 >= (packets.len() - 1) * 9,
        "{contiguous} of {} steps contiguous",
        packets.len() - 1
    );
    // The captured audio is the 440 Hz tone (skip the first 200 ms).
    let samples: Vec<f64> = packets
        .iter()
        .skip(20)
        .flat_map(|packet| packet.mono.iter().copied())
        .collect();
    let tone = goertzel(&samples, 440.0);
    let off = goertzel(&samples, 1_000.0).max(goertzel(&samples, 700.0));
    assert!(
        tone > off * 100.0,
        "440 Hz power {tone:.3e} vs off-tone {off:.3e}"
    );
}

fn monotonic_us() -> u64 {
    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: valid pointer to a timespec.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) };
    now.tv_sec as u64 * 1_000_000 + now.tv_nsec as u64 / 1_000
}

/// A capture backend that beeps (1 kHz, 100 ms) on a CLOCK_MONOTONIC
/// schedule and stamps every frame with its exact media-clock time. It stands
/// in for the sound server so the A/V test measures the driver's clock
/// plumbing, not the container's (unreported) PulseAudio buffering.
struct ScheduledBeeps {
    start_monotonic_us: u64,
    period_us: u64,
}

struct BeepStream {
    source: cua_media_codec::audio::AudioSourceInfo,
    running: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl cua_media_codec::audio::AudioStream for BeepStream {
    fn source(&self) -> &cua_media_codec::audio::AudioSourceInfo {
        &self.source
    }
    fn stop(&mut self) {
        self.running
            .store(false, std::sync::atomic::Ordering::Release);
    }
}

impl cua_media_codec::audio::AudioCapture for ScheduledBeeps {
    fn backend(&self) -> cua_media_codec::audio::AudioBackend {
        cua_media_codec::audio::AudioBackend::Fake
    }
    fn sources(&self) -> cua_media_codec::Result<Vec<cua_media_codec::audio::AudioSourceInfo>> {
        Ok(vec![cua_media_codec::audio::AudioSourceInfo::desktop(
            "Scheduled beeps",
        )])
    }
    fn start(
        &self,
        request: &cua_media_codec::audio::AudioCaptureRequest,
        sink: std::sync::Arc<dyn cua_media_codec::audio::AudioFrameSink>,
    ) -> cua_media_codec::Result<Box<dyn cua_media_codec::audio::AudioStream>> {
        let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let flag = running.clone();
        let format = request.format;
        let (start, period) = (self.start_monotonic_us, self.period_us);
        std::thread::spawn(move || {
            // media clock = monotonic + offset (same process as the driver).
            let offset = cua_media_codec::media_clock_us() as i64 - monotonic_us() as i64;
            let chunk = 480usize;
            let mut next_media = cua_media_codec::media_clock_us();
            while flag.load(std::sync::atomic::Ordering::Acquire) {
                let wait = next_media as i64 + 10_000 - cua_media_codec::media_clock_us() as i64;
                if wait > 0 {
                    std::thread::sleep(Duration::from_micros(wait as u64));
                }
                let mut samples = Vec::with_capacity(chunk * usize::from(format.channels));
                for index in 0..chunk {
                    let media = next_media + index as u64 * 1_000_000 / 48_000;
                    let monotonic = (media as i64 - offset) as u64;
                    let beeping = monotonic >= start && (monotonic - start) % period < 100_000;
                    let value = if beeping {
                        ((std::f64::consts::TAU * 1_000.0 * index as f64 / 48_000.0).sin()
                            * 12_000.0) as i16
                    } else {
                        0
                    };
                    for _ in 0..format.channels {
                        samples.push(value);
                    }
                }
                sink.on_audio(cua_media_codec::audio::AudioFrame {
                    pts_us: next_media,
                    format,
                    samples,
                });
                next_media += 10_000;
            }
        });
        Ok(Box::new(BeepStream {
            source: cua_media_codec::audio::AudioSourceInfo::desktop("Scheduled beeps"),
            running,
        }))
    }
}

/// Stream a flashing (and, with `beep`, beeping) fixture on `address` and
/// return video-minus-audio skews in microseconds, one per flash.
/// `start_ms` is an absolute CLOCK_MONOTONIC time when the fixture shares
/// this machine's clock, or `None` to let the fixture start 2.5 s after
/// launch on its own clock (a remote or gVisor sandbox).
async fn av_skews(base: &str, start_ms: Option<u64>, period_ms: u64, beep: bool) -> Vec<i64> {
    let channel = channel_to(base).await;
    let (display_w, display_h) = display_size(&channel).await;
    let (start_flag, start_arg) = match start_ms {
        Some(start) => ("--flash-start-monotonic-ms", start.to_string()),
        None => ("--flash-start-in-ms", "2500".to_owned()),
    };
    let period_arg = period_ms.to_string();
    let mut flasher_args = vec![
        "--color",
        "000000",
        "--x",
        "300",
        "--y",
        "200",
        "--width",
        "300",
        "--height",
        "200",
        start_flag,
        &start_arg,
        "--flash-every-ms",
        &period_arg,
    ];
    if beep {
        flasher_args.push("--beep");
    }
    let flasher = pad("flash", &flasher_args).await;
    let window = find_window(&channel, &flasher.title).await;
    let bounds = window.bounds.unwrap();
    let mut stream = StreamServiceClient::new(channel.clone());
    let opened = stream
        .open_media(OpenMediaRequest {
            target: Some(MediaTarget {
                target: Some(media_target::Target::DisplayId("primary".into())),
            }),
            codecs: vec![MediaCodec::H264 as i32],
            max_fps: 60,
            policy: SessionPolicy::ViewOnly as i32,
            audio: Some(cua_proto::env::v1::AudioOptions {
                enabled: true,
                source_ids: vec![],
                encoding: Some(cua_proto::env::v1::AudioEncoding {
                    codecs: vec![cua_proto::env::v1::AudioCodec::PcmS16le as i32],
                    sample_rate_hz: 48_000,
                    channels: 2,
                    frame_ms: 10,
                    ..cua_proto::env::v1::AudioEncoding::default()
                }),
                uplink: None,
            }),
            ..OpenMediaRequest::default()
        })
        .await
        .unwrap()
        .into_inner();
    let (mut socket, _) = tokio_tungstenite::connect_async(format!(
        "{}{}",
        base.replacen("http", "ws", 1),
        opened.ws_path
    ))
    .await
    .unwrap();
    let mut decoder = Decoder::new();
    let mut flashes: Vec<u64> = Vec::new();
    let mut beeps: Vec<u64> = Vec::new();
    let mut bright = false;
    let mut loud = false;
    let started = Instant::now();
    for _ in 0..20_000 {
        if started.elapsed() > Duration::from_secs(8) || (flashes.len() >= 3 && beeps.len() >= 3) {
            break;
        }
        match next(&mut socket, Duration::from_secs(2)).await {
            Some(Item::Video(descriptor, payload)) => {
                if let Some(frame) = decoder.decode(&payload) {
                    let x = ((bounds.x + bounds.width / 2.0) * frame.0 as f64 / display_w) as usize;
                    let y =
                        ((bounds.y + bounds.height / 2.0) * frame.1 as f64 / display_h) as usize;
                    let now_bright = pixel(&frame, x, y)[0] > 200;
                    if now_bright && !bright {
                        flashes.push(descriptor.capture_timestamp_us);
                    }
                    bright = now_bright;
                }
            }
            Some(Item::Audio(bytes)) => {
                let packet = pcm_packet(&bytes);
                let onset = packet.mono.iter().position(|sample| sample.abs() > 4_000.0);
                match onset {
                    Some(index) if !loud => {
                        beeps.push(packet.header.pts_us + index as u64 * 1_000_000 / 48_000);
                        loud = true;
                    }
                    None => loud = false,
                    _ => {}
                }
            }
            Some(_) => {}
            None => break,
        }
    }
    assert!(
        flashes.len() >= 2 && beeps.len() >= 2,
        "flashes {flashes:?} beeps {beeps:?}"
    );
    // Pair each flash with its beep (same schedule slot: within 400 ms);
    // flashes before audio started have no partner and are skipped.
    let skews: Vec<i64> = flashes
        .iter()
        .filter_map(|flash| {
            beeps
                .iter()
                .map(|beep| *flash as i64 - *beep as i64)
                .min_by_key(|skew| skew.abs())
        })
        .filter(|skew| skew.abs() < 400_000)
        .collect();
    assert!(
        skews.len() >= 2,
        "at least two flash/beep pairs: flashes {flashes:?} beeps {beeps:?}"
    );
    drop(flasher);
    skews
}

#[tokio::test]
async fn audio_and_video_share_one_clock_within_40_ms() {
    if !enabled() {
        return;
    }
    if remote_target().is_some() {
        eprintln!("skipped against a remote target: needs an in-process scheduled audio source");
        return;
    }
    // A dedicated driver whose audio source beeps on the fixture's schedule.
    let start_ms = monotonic_us() / 1_000 + 2_500;
    let period_ms = 1_000u64;
    let address = {
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("runtime");
            runtime.block_on(async move {
                let audio = cua_spacesd_desktop::codec_audio::CodecAudioProvider::with_capture(
                    Box::new(ScheduledBeeps {
                        start_monotonic_us: start_ms * 1_000,
                        period_us: period_ms * 1_000,
                    }),
                );
                let provider = DesktopServiceProvider::for_host_with_audio(
                    ServerContext::new(ServerConfig::default(), None),
                    Some(std::sync::Arc::new(audio)),
                )
                .expect("Linux desktop backend");
                let router = provider.standalone_router();
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                    .await
                    .expect("bind");
                sender
                    .send(listener.local_addr().expect("address"))
                    .expect("send");
                axum::serve(listener, router).await.expect("serve");
            });
        });
        receiver
            .recv_timeout(Duration::from_secs(30))
            .expect("driver started")
    };
    let skews = av_skews(
        &format!("http://{address}"),
        Some(start_ms),
        period_ms,
        false,
    )
    .await;
    eprintln!("A/V skews (us): {skews:?}");
    for skew in &skews {
        assert!(skew.abs() <= 40_000, "A/V skew {skew} us exceeds ±40 ms");
    }
}

#[tokio::test]
async fn real_pulse_audio_and_video_stay_within_40_ms() {
    if !enabled() {
        return;
    }
    // The shared driver captures the real PulseAudio monitor; the fixture
    // flashes and plays a beep through PulseAudio on the same schedule.
    let skews = av_skews(&base_url(), None, 1_000, true).await;
    eprintln!("real Pulse A/V skews (us): {skews:?}");
    for skew in &skews {
        assert!(skew.abs() <= 40_000, "A/V skew {skew} us exceeds ±40 ms");
    }
}

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! End-to-end Windows validation over the real daemon stack.
//!
//! Each test starts the actual provider bundle + WebSocket daemon in-process,
//! drives it with a WebSocket client exactly like the HTML5 demo client, and
//! targets freshly launched `cua-spacesd-test-pad` fixture windows so no real
//! application state is touched.
//!
//! The tests serialize on a global lock: they assert about the interactive
//! desktop's foreground window and sweep fixture processes, which is global
//! state, so they must not overlap.

#![cfg(target_os = "windows")]

use std::error::Error;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::Child;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cua_media_protocol::{
    ActionBasis, ActionRequest, ClientMessage, Hello, OpenSession, ServerMessage, SessionPolicy,
    VideoCodec, VideoFrameDescriptor, WindowDescriptor, WindowGeometryControl,
    WindowGeometryRequest, WindowLifecycleEvent, WireHeader,
};
use cua_spacesd::presence::{AppConfig, PresenceHub};
use cua_spacesd::ws::{serve_ws, WsDaemon};
use cua_spacesd_desktop::CuaProviderBundle;
use cua_spacesd_session::ServerRuntime;
use futures_util::{SinkExt as _, StreamExt as _};
use tokio_tungstenite::tungstenite::Message;

type TestResult = Result<(), Box<dyn Error + Send + Sync>>;

/// Serializes the tests: each asserts about desktop-global state (foreground
/// window, every fixture pad process), so overlapping runs corrupt each other.
static DESKTOP_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

// ── fixture management ───────────────────────────────────────────────────────

fn target_dir() -> PathBuf {
    // .../target/debug/deps/this_test.exe -> .../target/debug
    let mut path = std::env::current_exe().expect("test exe path");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path
}

fn test_pad_exe() -> PathBuf {
    let exe = target_dir().join("cua-spacesd-test-pad.exe");
    if !exe.exists() {
        let status = std::process::Command::new(env!("CARGO"))
            .args(["build", "-p", "cua-spacesd-test-apps"])
            .status()
            .expect("cargo build cua-spacesd-test-apps");
        assert!(status.success(), "failed to build cua-spacesd-test-pad");
    }
    assert!(exe.exists(), "missing {}", exe.display());
    exe
}

struct Pad {
    child: Child,
    log_path: PathBuf,
    title: String,
}

impl Pad {
    fn spawn(tag: &str) -> Self {
        let title = format!("RCDP E2E {tag} {}", std::process::id());
        let log_path = std::env::temp_dir().join(format!(
            "cua-env-e2e-{tag}-{}-{}.jsonl",
            std::process::id(),
            unique()
        ));
        let child = std::process::Command::new(test_pad_exe())
            .args(["--title", &title, "--log"])
            .arg(&log_path)
            .spawn()
            .expect("spawn cua-spacesd-test-pad");
        Self {
            child,
            log_path,
            title,
        }
    }

    fn log_events(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(&self.log_path)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    async fn wait_for_event<F>(&self, timeout: Duration, matcher: F) -> Option<serde_json::Value>
    where
        F: Fn(&serde_json::Value) -> bool,
    {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Some(event) = self.log_events().into_iter().find(|event| matcher(event)) {
                return Some(event);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        None
    }
}

impl Drop for Pad {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.log_path);
    }
}

/// Kill every fixture pad process, no matter which test (or earlier failed
/// run) spawned or launched it. Runs at the start of each test and inside
/// every cleanup guard so no fixture window outlives the tests.
fn kill_all_pads() {
    let _ = std::process::Command::new("taskkill")
        .args(["/F", "/IM", "cua-spacesd-test-pad.exe"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// Drop guard that removes every fixture pad even when the test panics.
struct PadReaper;

impl Drop for PadReaper {
    fn drop(&mut self) {
        kill_all_pads();
    }
}

fn unique() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
        + std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos() as u64
}

fn foreground_window() -> isize {
    unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow().0 as isize }
}

/// Walk the desktop's top-level z-order (top to bottom) and report whether
/// this process's cursor-overlay window sits above `below_hwnd`. EnumWindows
/// enumerates in z-order, so the first of the two encountered is on top.
fn overlay_is_above(below_hwnd: u64) -> bool {
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClassNameW, GetWindowThreadProcessId,
    };

    struct Walk {
        below_hwnd: u64,
        own_pid: u32,
        overlay_first: Option<bool>,
    }

    extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let walk = unsafe { &mut *(lparam.0 as *mut Walk) };
        if hwnd.0 as u64 == walk.below_hwnd {
            walk.overlay_first.get_or_insert(false);
            return false.into();
        }
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if pid == walk.own_pid {
            let mut class = [0u16; 64];
            let length = unsafe { GetClassNameW(hwnd, &mut class) };
            let class = String::from_utf16_lossy(&class[..length.max(0) as usize]);
            if class == "Cua.AgentCursorOverlay" {
                walk.overlay_first.get_or_insert(true);
                return false.into();
            }
        }
        true.into()
    }

    let mut walk = Walk {
        below_hwnd,
        own_pid: std::process::id(),
        overlay_first: None,
    };
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM(&mut walk as *mut Walk as isize));
    }
    walk.overlay_first == Some(true)
}

fn describe_window(hwnd: isize) -> String {
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowTextW, GetWindowThreadProcessId};
    unsafe {
        let handle = windows::Win32::Foundation::HWND(hwnd as *mut _);
        let mut title = [0u16; 256];
        let length = GetWindowTextW(handle, &mut title);
        let mut pid = 0u32;
        GetWindowThreadProcessId(handle, Some(&mut pid));
        format!(
            "hwnd={hwnd} pid={pid} title={:?}",
            String::from_utf16_lossy(&title[..length.max(0) as usize])
        )
    }
}

// ── daemon + client harness ──────────────────────────────────────────────────

fn start_daemon(
    port: u16,
    apps: Vec<AppConfig>,
) -> (SocketAddr, Arc<cua_spacesd_desktop::CuaPresenceOverlay>) {
    let providers = CuaProviderBundle::new().expect("provider bundle");
    let overlay = providers.presence.clone();
    let targets = providers.targets.clone();
    let runtime = Arc::new(ServerRuntime::new(
        providers.targets,
        providers.captures,
        providers.actions,
        providers.accessibility,
        providers.geometry,
    ));
    let presence = Arc::new(PresenceHub::new(
        providers.presence,
        providers.launcher,
        apps,
    ));
    cua_spacesd::presence::spawn_desktop_watchers(presence.clone(), targets, overlay.clone());
    let daemon = Arc::new(WsDaemon {
        runtime,
        presence,
        token: None,
    });
    let address: SocketAddr = format!("127.0.0.1:{port}").parse().expect("address");
    tokio::spawn(async move {
        let _ = serve_ws(address, daemon).await;
    });
    (address, overlay)
}

enum Event {
    Control(ServerMessage),
    Video(VideoFrameDescriptor, Vec<u8>),
}

struct Client {
    ws: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
}

impl Client {
    async fn connect(address: SocketAddr) -> Self {
        let url = format!("ws://{address}");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match tokio_tungstenite::connect_async(&url).await {
                Ok((ws, _)) => return Self { ws },
                Err(error) => {
                    assert!(
                        Instant::now() < deadline,
                        "could not connect to {url}: {error}"
                    );
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }
    }

    async fn send(&mut self, message: ClientMessage) {
        let text = serde_json::to_string(&WireHeader::Client(message)).expect("encode");
        self.ws.send(Message::Text(text)).await.expect("ws send");
    }

    async fn next_event(&mut self) -> Event {
        loop {
            let message = tokio::time::timeout(Duration::from_secs(10), self.ws.next())
                .await
                .expect("timed out waiting for daemon message")
                .expect("daemon closed the connection")
                .expect("websocket error");
            match message {
                Message::Text(text) => {
                    let header: WireHeader = serde_json::from_str(&text).expect("wire header");
                    if let WireHeader::Server(server) = header {
                        return Event::Control(server);
                    }
                }
                Message::Binary(bytes) => {
                    assert!(bytes.len() >= 8, "short binary packet");
                    let header_len = u32::from_be_bytes(bytes[0..4].try_into().unwrap()) as usize;
                    let payload_len = u32::from_be_bytes(bytes[4..8].try_into().unwrap()) as usize;
                    assert_eq!(8 + header_len + payload_len, bytes.len());
                    let header: WireHeader =
                        serde_json::from_slice(&bytes[8..8 + header_len]).expect("video header");
                    if let WireHeader::Video(descriptor) = header {
                        return Event::Video(descriptor, bytes[8 + header_len..].to_vec());
                    }
                }
                _ => {}
            }
        }
    }

    /// Drain events until the matcher accepts one, with a global timeout.
    async fn expect<T>(
        &mut self,
        what: &str,
        timeout: Duration,
        matcher: impl Fn(Event) -> Option<T>,
    ) -> T {
        let deadline = Instant::now() + timeout;
        loop {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            if let Some(value) = matcher(self.next_event().await) {
                return value;
            }
        }
    }

    async fn handshake(&mut self, name: &str) {
        self.send(ClientMessage::Authenticate {
            token: String::new(),
        })
        .await;
        self.expect("authenticated", Duration::from_secs(5), |event| {
            matches!(event, Event::Control(ServerMessage::Authenticated)).then_some(())
        })
        .await;
        self.send(ClientMessage::Hello(Hello::default())).await;
        self.expect("hello", Duration::from_secs(5), |event| {
            matches!(event, Event::Control(ServerMessage::Hello(_))).then_some(())
        })
        .await;
        self.send(ClientMessage::Join {
            name: name.into(),
            color: None,
        })
        .await;
        self.expect("joined", Duration::from_secs(5), |event| {
            matches!(event, Event::Control(ServerMessage::Joined { .. })).then_some(())
        })
        .await;
    }

    async fn find_window(&mut self, title: &str) -> WindowDescriptor {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            self.send(ClientMessage::ListWindows {
                on_screen_only: true,
            })
            .await;
            let windows = self
                .expect("windows", Duration::from_secs(5), |event| match event {
                    Event::Control(ServerMessage::Windows { windows }) => Some(windows),
                    _ => None,
                })
                .await;
            if let Some(descriptor) = windows
                .into_iter()
                .find(|descriptor| descriptor.title.contains(title))
            {
                return descriptor;
            }
            assert!(
                Instant::now() < deadline,
                "window titled {title:?} never appeared"
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    async fn open_session(
        &mut self,
        descriptor: &WindowDescriptor,
    ) -> cua_media_protocol::SessionOpened {
        self.open_session_with(
            descriptor,
            vec![VideoCodec::Bgra],
            SessionPolicy::BackgroundOnly,
            WindowGeometryControl::ObserveOnly,
        )
        .await
    }

    async fn open_session_with(
        &mut self,
        descriptor: &WindowDescriptor,
        accepted_codecs: Vec<VideoCodec>,
        policy: SessionPolicy,
        geometry_control: WindowGeometryControl,
    ) -> cua_media_protocol::SessionOpened {
        self.send(ClientMessage::OpenSession(OpenSession {
            window: descriptor.window.clone(),
            target_epoch: descriptor.target_epoch,
            accepted_codecs,
            max_fps: 20,
            max_dimension: 4096,
            target_bitrate_kbps: None,
            policy,
            geometry_control,
        }))
        .await;
        self.expect(
            "session_opened",
            Duration::from_secs(10),
            |event| match event {
                Event::Control(ServerMessage::SessionOpened(opened)) => Some(opened),
                Event::Control(ServerMessage::Error { code, message }) => {
                    panic!("open_session failed: {code:?} {message}")
                }
                _ => None,
            },
        )
        .await
    }

    async fn action(
        &mut self,
        session: &cua_media_protocol::SessionOpened,
        tool: &str,
        args: serde_json::Value,
    ) {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let action_id = format!(
            "e2e-{}",
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        self.send(ClientMessage::Action(ActionRequest {
            action_id: action_id.clone(),
            session_id: session.session_id.clone(),
            tool: tool.into(),
            arguments: args,
            basis: ActionBasis::None,
        }))
        .await;
        let result = self
            .expect(
                "action_result",
                Duration::from_secs(15),
                |event| match event {
                    Event::Control(ServerMessage::ActionResult(result))
                        if result.action_id == action_id =>
                    {
                        Some(result)
                    }
                    _ => None,
                },
            )
            .await;
        assert!(
            result.delivered && result.error.is_none(),
            "{tool} was not delivered: {:?}",
            result.error
        );
    }
}

// ── tests ────────────────────────────────────────────────────────────────────

/// Native-client parity: Windows negotiates the compressed path, applies a
/// client-driven host-window resize without stealing focus, and restarts the
/// codec at a fresh independently decodable IDR matching the new geometry.
#[tokio::test]
#[ignore = "requires an interactive Windows desktop"]
async fn h264_stream_resizes_bidirectionally() -> TestResult {
    let _desktop = DESKTOP_LOCK.lock().await;
    kill_all_pads();
    let _reaper = PadReaper;
    let pad = Pad::spawn("h264-resize");
    tokio::time::sleep(Duration::from_millis(800)).await;

    let (address, _overlay) = start_daemon(18764, Vec::new());
    let mut client = Client::connect(address).await;
    client.handshake("native-windows").await;

    let descriptor = client.find_window(&pad.title).await;
    let session = client
        .open_session_with(
            &descriptor,
            vec![VideoCodec::H264],
            SessionPolicy::AllowActivation,
            WindowGeometryControl::Bidirectional,
        )
        .await;
    assert_eq!(session.codec, VideoCodec::H264);
    assert_eq!(
        session.geometry_control,
        WindowGeometryControl::Bidirectional
    );

    let (initial_frame, initial_payload) = client
        .expect(
            "initial H.264 frame",
            Duration::from_secs(15),
            |event| match event {
                Event::Video(frame, payload) if frame.session_id == session.session_id => {
                    Some((frame, payload))
                }
                _ => None,
            },
        )
        .await;
    assert_eq!(initial_frame.codec, VideoCodec::H264);
    assert!(initial_frame.keyframe);
    assert!(has_annex_b_nal(&initial_payload, 7));
    assert!(has_annex_b_nal(&initial_payload, 8));
    assert!(has_annex_b_nal(&initial_payload, 5));

    let foreground_before_resize = foreground_window();
    client
        .send(ClientMessage::SetWindowGeometry(WindowGeometryRequest {
            session_id: session.session_id.clone(),
            revision: 1,
            width_points: 700,
            height_points: 520,
        }))
        .await;
    let applied = client
        .expect(
            "window geometry result",
            Duration::from_secs(10),
            |event| match event {
                Event::Control(ServerMessage::WindowGeometryResult(result))
                    if result.session_id == session.session_id && result.revision == 1 =>
                {
                    Some(result)
                }
                _ => None,
            },
        )
        .await;
    assert!(
        applied.applied && applied.error.is_none(),
        "Windows rejected the resize: {:?}",
        applied.error
    );
    assert_eq!(foreground_window(), foreground_before_resize);

    let (geometry_epoch, geometry) = client
        .expect(
            "resized lifecycle geometry",
            Duration::from_secs(15),
            |event| match event {
                Event::Control(ServerMessage::Lifecycle {
                    session_id,
                    event:
                        WindowLifecycleEvent::GeometryChanged {
                            geometry_epoch,
                            geometry,
                        },
                }) if session_id == session.session_id => Some((geometry_epoch, geometry)),
                _ => None,
            },
        )
        .await;
    assert!(geometry_epoch.0 > initial_frame.geometry_epoch.0);
    assert_ne!(
        (geometry.width_px, geometry.height_px),
        (initial_frame.width_px, initial_frame.height_px)
    );
    assert_eq!(geometry.width_px % 2, 0);
    assert_eq!(geometry.height_px % 2, 0);

    let (resized_frame, resized_payload) = client
        .expect(
            "resized H.264 keyframe",
            Duration::from_secs(15),
            |event| match event {
                Event::Video(frame, payload)
                    if frame.session_id == session.session_id
                        && frame.geometry_epoch == geometry_epoch =>
                {
                    Some((frame, payload))
                }
                _ => None,
            },
        )
        .await;
    assert_eq!(resized_frame.codec, VideoCodec::H264);
    assert_eq!(
        (resized_frame.width_px, resized_frame.height_px),
        (geometry.width_px, geometry.height_px)
    );
    assert!(resized_frame.codec_epoch.0 > initial_frame.codec_epoch.0);
    assert!(resized_frame.keyframe);
    assert!(has_annex_b_nal(&resized_payload, 7));
    assert!(has_annex_b_nal(&resized_payload, 8));
    assert!(has_annex_b_nal(&resized_payload, 5));

    // Revisions are monotonic, so a delayed duplicate cannot roll the host
    // window back after the successful resize.
    client
        .send(ClientMessage::SetWindowGeometry(WindowGeometryRequest {
            session_id: session.session_id.clone(),
            revision: 1,
            width_points: 640,
            height_points: 480,
        }))
        .await;
    let stale = client
        .expect(
            "stale window geometry result",
            Duration::from_secs(10),
            |event| match event {
                Event::Control(ServerMessage::WindowGeometryResult(result))
                    if result.session_id == session.session_id && result.revision == 1 =>
                {
                    Some(result)
                }
                _ => None,
            },
        )
        .await;
    assert!(!stale.applied);
    assert!(stale
        .error
        .as_deref()
        .is_some_and(|error| error.contains("stale or duplicate")));
    Ok(())
}

fn has_annex_b_nal(bytes: &[u8], expected_type: u8) -> bool {
    let mut offset = 0usize;
    while offset + 3 < bytes.len() {
        let start_code_len = if bytes[offset..].starts_with(&[0, 0, 0, 1]) {
            4
        } else if bytes[offset..].starts_with(&[0, 0, 1]) {
            3
        } else {
            offset += 1;
            continue;
        };
        let header = offset + start_code_len;
        if header < bytes.len() && bytes[header] & 0x1f == expected_type {
            return true;
        }
        offset = header.saturating_add(1);
    }
    false
}

/// Requirement 1 + 2: a launched window streams frames in real time, and
/// clicks/typing from the client land in that window as background input
/// without moving foreground focus.
#[tokio::test]
#[ignore = "requires an interactive Windows desktop"]
async fn streams_frames_and_delivers_background_input() -> TestResult {
    let _desktop = DESKTOP_LOCK.lock().await;
    kill_all_pads();
    let _reaper = PadReaper;
    let pad = Pad::spawn("stream");
    tokio::time::sleep(Duration::from_millis(800)).await;

    let (address, _overlay) = start_daemon(18761, Vec::new());
    let mut client = Client::connect(address).await;
    client.handshake("streamer").await;

    let descriptor = client.find_window(&pad.title).await;
    assert!(descriptor.geometry.width_px > 100);
    let session = client.open_session(&descriptor).await;
    assert_eq!(session.codec, VideoCodec::Bgra);

    // Collect three distinct frames; the pad repaints every 250 ms, so a live
    // stream must produce them quickly.
    let mut sequences = Vec::new();
    let mut last_payload = Vec::new();
    while sequences.len() < 3 {
        let (descriptor, payload) = client
            .expect(
                "video frame",
                Duration::from_secs(10),
                |event| match event {
                    Event::Video(descriptor, payload) => Some((descriptor, payload)),
                    _ => None,
                },
            )
            .await;
        assert_eq!(descriptor.codec, VideoCodec::Bgra);
        assert_eq!(
            payload.len(),
            (descriptor.width_px * descriptor.height_px * 4) as usize,
            "BGRA payload must match dimensions"
        );
        assert!(
            payload.iter().any(|byte| *byte != 0),
            "frame must not be all-black"
        );
        if sequences.last() != Some(&descriptor.sequence.0) {
            sequences.push(descriptor.sequence.0);
        }
        last_payload = payload;
    }
    assert!(
        sequences.windows(2).all(|pair| pair[0] < pair[1]),
        "frame sequences must increase: {sequences:?}"
    );
    assert!(!last_payload.is_empty());

    // Background click at a known frame coordinate.
    let baseline_foreground = foreground_window();
    let click = (
        (descriptor.geometry.width_px / 2) as i64,
        (descriptor.geometry.height_px / 2) as i64,
    );
    client
        .action(
            &session,
            "click",
            serde_json::json!({"x": click.0, "y": click.1, "button": "left", "count": 1}),
        )
        .await;
    let event = pad
        .wait_for_event(Duration::from_secs(10), |event| {
            event["event"] == "click" && event["button"] == "left"
        })
        .await
        .expect("the pad never received the click");
    let dx = (event["frame_rel_x"].as_i64().unwrap() - click.0).abs();
    let dy = (event["frame_rel_y"].as_i64().unwrap() - click.1).abs();
    assert!(
        dx <= 3 && dy <= 3,
        "click landed at offset ({dx}, {dy}) from the requested frame position: {event}"
    );

    // Background typing.
    client
        .action(
            &session,
            "type_text",
            serde_json::json!({"text": "cua_env_ok"}),
        )
        .await;
    pad.wait_for_event(Duration::from_secs(10), |event| {
        event["event"] == "char" && event["char"] == "k"
    })
    .await
    .expect("the pad never received typed text");
    let typed: String = pad
        .log_events()
        .iter()
        .filter(|event| event["event"] == "char")
        .filter_map(|event| event["char"].as_str().map(str::to_owned))
        .collect();
    assert!(
        typed.contains("cua_env_ok"),
        "typed stream {typed:?} does not contain cua_env_ok"
    );

    // The user's cursor and focus were never hijacked.
    assert_eq!(
        foreground_window(),
        baseline_foreground,
        "background input changed the foreground window"
    );
    let after_input = pad.log_events();
    let activations = after_input
        .iter()
        .filter(|event| event["event"] == "activate")
        .filter(|event| event["t_ms"].as_u64().unwrap_or(0) > 1500)
        .filter(|event| event["state"].as_u64().unwrap_or(0) != 0)
        .count();
    assert_eq!(
        activations, 0,
        "background input activated the pad window: {after_input:?}"
    );
    Ok(())
}

/// Requirement 3: launching an app from the client's app menu must not bring
/// the new window to the foreground, and the launched window must be
/// immediately addressable and streamable.
#[tokio::test]
#[ignore = "requires an interactive Windows desktop"]
async fn app_menu_launch_stays_in_background() -> TestResult {
    let _desktop = DESKTOP_LOCK.lock().await;
    kill_all_pads();
    let _reaper = PadReaper;
    let log_path = std::env::temp_dir().join(format!(
        "cua-env-e2e-launch-{}-{}.jsonl",
        std::process::id(),
        unique()
    ));
    let title = format!("RCDP E2E launch {}", std::process::id());
    let apps = vec![AppConfig {
        app_id: "test-pad".into(),
        name: "RCDP Test Pad".into(),
        path: test_pad_exe().to_string_lossy().into_owned(),
        args: vec![
            "--title".into(),
            title.clone(),
            "--log".into(),
            log_path.to_string_lossy().into_owned(),
        ],
    }];
    let (address, _overlay) = start_daemon(18762, apps);
    let mut client = Client::connect(address).await;
    client.handshake("launcher").await;

    // The app menu lists the configured app.
    client.send(ClientMessage::ListApps).await;
    let apps = client
        .expect("apps", Duration::from_secs(5), |event| match event {
            Event::Control(ServerMessage::Apps { apps }) => Some(apps),
            _ => None,
        })
        .await;
    assert_eq!(apps.len(), 1);
    assert_eq!(apps[0].app_id, "test-pad");

    let baseline_foreground = foreground_window();
    client
        .send(ClientMessage::LaunchApp {
            app_id: "test-pad".into(),
        })
        .await;
    let launched = client
        .expect(
            "app_launched",
            Duration::from_secs(30),
            |event| match event {
                Event::Control(ServerMessage::AppLaunched { window, .. }) => Some(window),
                Event::Control(ServerMessage::Error { code, message }) => {
                    panic!("launch failed: {code:?} {message}")
                }
                _ => None,
            },
        )
        .await;
    let launched = launched.expect("launched window did not materialize");
    assert!(
        launched.title.contains(&title),
        "unexpected launched window: {launched:?}"
    );

    // The launched pad must never have been activated, and the previously
    // focused window must still be focused.
    tokio::time::sleep(Duration::from_millis(600)).await;
    let current_foreground = foreground_window();
    assert_eq!(
        current_foreground,
        baseline_foreground,
        "launch_app changed the foreground window: before [{}], after [{}]",
        describe_window(baseline_foreground),
        describe_window(current_foreground)
    );
    let events: Vec<serde_json::Value> = std::fs::read_to_string(&log_path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    assert!(
        !events.is_empty(),
        "launched pad wrote no log at {}",
        log_path.display()
    );
    let activated = events
        .iter()
        .filter(|event| event["event"] == "activate")
        .any(|event| event["state"].as_u64().unwrap_or(0) != 0);
    assert!(
        !activated,
        "launched window was activated at some point: {events:?}"
    );

    // The launched window is streamable straight away.
    let session = client.open_session(&launched).await;
    client
        .expect(
            "video frame",
            Duration::from_secs(10),
            |event| match event {
                Event::Video(descriptor, _) if descriptor.session_id == session.session_id => {
                    Some(())
                }
                _ => None,
            },
        )
        .await;

    // Clean up the launched fixture without touching foreground state.
    client
        .action(
            &session,
            "press_key",
            serde_json::json!({"key": "f4", "modifiers": ["alt"]}),
        )
        .await;
    let _ = std::fs::remove_file(&log_path);
    Ok(())
}

/// Requirement 4: every connected user sees the others — roster updates and
/// live cursor positions are broadcast between clients.
#[tokio::test]
#[ignore = "requires an interactive Windows desktop"]
async fn presence_roster_and_cursors_broadcast() -> TestResult {
    let _desktop = DESKTOP_LOCK.lock().await;
    kill_all_pads();
    let _reaper = PadReaper;
    let pad = Pad::spawn("presence");
    tokio::time::sleep(Duration::from_millis(800)).await;

    let (address, overlay) = start_daemon(18763, Vec::new());
    let mut alice = Client::connect(address).await;
    alice.handshake("alice").await;
    let mut bob = Client::connect(address).await;
    bob.handshake("bob").await;

    // Alice learns about Bob through a presence broadcast.
    alice
        .expect(
            "presence with bob",
            Duration::from_secs(5),
            |event| match event {
                Event::Control(ServerMessage::Presence { users })
                    if users.iter().any(|user| user.name == "bob") =>
                {
                    Some(())
                }
                _ => None,
            },
        )
        .await;

    // Bob moves his cursor over the pad's window; Alice sees it with Bob's
    // identity, color, and coordinates.
    let descriptor = bob.find_window(&pad.title).await;
    bob.send(ClientMessage::Cursor {
        window: Some(descriptor.window.clone()),
        x: 120.0,
        y: 90.0,
        visible: true,
        pressed: false,
    })
    .await;
    let cursor = alice
        .expect(
            "remote cursor",
            Duration::from_secs(5),
            |event| match event {
                Event::Control(ServerMessage::RemoteCursor(cursor)) if cursor.name == "bob" => {
                    Some(cursor)
                }
                _ => None,
            },
        )
        .await;
    assert_eq!(cursor.window.as_ref(), Some(&descriptor.window));
    assert_eq!(cursor.x, 120.0);
    assert_eq!(cursor.y, 90.0);
    assert!(cursor.visible);
    assert!(!cursor.color.is_empty());

    // The daemon renders remote cursors on the host desktop. Overlay windows
    // are excluded from ordinary screen captures, so verify through the
    // overlay's render state: bob's desktop cursor must sit at the window's
    // frame origin plus his window-local coordinates. The pad logs its DWM
    // frame origin at startup, and the daemon adds the same one-pixel frame
    // inset the cua-driver pixel tools use.
    let geometry = pad
        .wait_for_event(Duration::from_secs(5), |event| event["event"] == "geometry")
        .await
        .expect("pad logged no geometry");
    let expected = (
        geometry["frame_origin"][0].as_f64().unwrap() + 1.0 + 120.0,
        geometry["frame_origin"][1].as_f64().unwrap() + 1.0 + 90.0,
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    let position = loop {
        if let Some(position) = overlay.cursor_screen_position(&cursor.user_id) {
            if (position.0 - expected.0).abs() <= 2.0 && (position.1 - expected.1).abs() <= 2.0 {
                break Some(position);
            }
        }
        if Instant::now() >= deadline {
            break None;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert!(
        position.is_some(),
        "desktop overlay cursor for {} never reached {expected:?} (last: {:?})",
        cursor.user_id,
        overlay.cursor_screen_position(&cursor.user_id)
    );

    // The overlay must actually be visible: pinned above the window the
    // cursor is over, not buried under windows created after the daemon.
    let pad_hwnd = geometry["hwnd"].as_u64().expect("pad logged its hwnd");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut above = overlay_is_above(pad_hwnd);
    while !above && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
        above = overlay_is_above(pad_hwnd);
    }
    assert!(
        above,
        "the overlay window is stacked below the pad, so cursors are invisible"
    );

    // Window-list pushes: a window appearing on the desktop must reach
    // connected clients without an explicit list_windows request, and its
    // disappearance must too.
    let second_pad = Pad::spawn("presence2");
    let second_title = second_pad.title.clone();
    alice
        .expect(
            "windows push with new pad",
            Duration::from_secs(15),
            |event| match event {
                Event::Control(ServerMessage::Windows { windows })
                    if windows
                        .iter()
                        .any(|window| window.title.contains(&second_title)) =>
                {
                    Some(())
                }
                _ => None,
            },
        )
        .await;
    drop(second_pad);
    alice
        .expect(
            "windows push without closed pad",
            Duration::from_secs(15),
            |event| match event {
                Event::Control(ServerMessage::Windows { windows })
                    if windows
                        .iter()
                        .all(|window| !window.title.contains(&second_title)) =>
                {
                    Some(())
                }
                _ => None,
            },
        )
        .await;

    // Bob disconnecting removes him from Alice's roster.
    drop(bob);
    alice
        .expect(
            "presence without bob",
            Duration::from_secs(5),
            |event| match event {
                Event::Control(ServerMessage::Presence { users })
                    if users.iter().all(|user| user.name != "bob") =>
                {
                    Some(())
                }
                _ => None,
            },
        )
        .await;
    Ok(())
}
